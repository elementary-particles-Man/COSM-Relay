use super::reducer::{DurableEventBody, DurableJournalEvent, ReconcileOutcome};
use super::state::{LaneState, Phase};
use super::types::LaneId;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

#[cfg(unix)]
extern "C" {
    #[link_name = "flock"]
    fn libc_flock(fd: std::os::raw::c_int, operation: std::os::raw::c_int) -> std::os::raw::c_int;
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PersistedJournalEntry {
    pub schema_version: u16,
    pub lane_id: LaneId,
    pub binding_epoch: u64,
    pub seq: u64,
    pub event_type: String,
    pub prev_event_sha256: [u8; 32],
    pub body: DurableEventBody,
    pub event_sha256: [u8; 32],
}

impl PersistedJournalEntry {
    pub fn compute_canonical_bytes(
        schema_version: u16,
        lane_id: &LaneId,
        binding_epoch: u64,
        seq: u64,
        event_type: &str,
        prev_event_sha256: &[u8; 32],
        body: &DurableEventBody,
    ) -> Result<Vec<u8>, serde_json::Error> {
        let body_bytes = serde_json::to_vec(body)?;
        let mut bytes = Vec::with_capacity(64 + body_bytes.len());
        bytes.extend_from_slice(&schema_version.to_be_bytes());
        bytes.extend_from_slice(lane_id.as_str().as_bytes());
        bytes.push(0);
        bytes.extend_from_slice(&binding_epoch.to_be_bytes());
        bytes.extend_from_slice(&seq.to_be_bytes());
        bytes.extend_from_slice(event_type.as_bytes());
        bytes.push(0);
        bytes.extend_from_slice(prev_event_sha256);
        bytes.extend_from_slice(&body_bytes);
        Ok(bytes)
    }

    pub fn new(
        schema_version: u16,
        lane_id: LaneId,
        binding_epoch: u64,
        seq: u64,
        event_type: String,
        prev_event_sha256: [u8; 32],
        body: DurableEventBody,
    ) -> Result<Self, io::Error> {
        let canonical_bytes = Self::compute_canonical_bytes(
            schema_version,
            &lane_id,
            binding_epoch,
            seq,
            &event_type,
            &prev_event_sha256,
            &body,
        )
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;

        let mut hasher = Sha256::new();
        hasher.update(&canonical_bytes);
        let event_sha256: [u8; 32] = hasher.finalize().into();

        Ok(Self {
            schema_version,
            lane_id,
            binding_epoch,
            seq,
            event_type,
            prev_event_sha256,
            body,
            event_sha256,
        })
    }

    pub fn verify_hash(&self) -> bool {
        match Self::compute_canonical_bytes(
            self.schema_version,
            &self.lane_id,
            self.binding_epoch,
            self.seq,
            &self.event_type,
            &self.prev_event_sha256,
            &self.body,
        ) {
            Ok(bytes) => {
                let mut hasher = Sha256::new();
                hasher.update(&bytes);
                let calculated: [u8; 32] = hasher.finalize().into();
                calculated == self.event_sha256
            }
            Err(_) => false,
        }
    }
}

pub struct FileJournal {
    state_dir: PathBuf,
    lane_id: LaneId,
    binding_epoch: u64,
    last_seq: u64,
    last_hash: [u8; 32],
    _lock_file: File,
}

impl FileJournal {
    pub fn open(
        state_dir: impl AsRef<Path>,
        lane_id: LaneId,
        binding_epoch: u64,
    ) -> io::Result<Self> {
        let state_dir = state_dir.as_ref().to_path_buf();
        let lane_dir = state_dir.join(lane_id.as_str());
        let events_dir = lane_dir.join("events");
        let tmp_dir = lane_dir.join("tmp");

        fs::create_dir_all(&events_dir)?;
        fs::create_dir_all(&tmp_dir)?;

        // Acquire exclusive lock using flock to prevent multi-process concurrent access
        let lock_path = lane_dir.join("lane.lock");
        let lock_file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lock_path)?;

        #[cfg(unix)]
        {
            use std::os::unix::io::AsRawFd;
            let fd = lock_file.as_raw_fd();
            let ret = unsafe { libc_flock(fd, 2 | 4) }; // LOCK_EX | LOCK_NB
            if ret != 0 {
                return Err(io::Error::new(
                    io::ErrorKind::WouldBlock,
                    format!(
                        "Lane {} is currently locked by another process",
                        lane_id.as_str()
                    ),
                ));
            }
        }

        // Simple single-process advisory lock: file exists check or open lock
        let mut journal = Self {
            state_dir,
            lane_id,
            binding_epoch,
            last_seq: 0,
            last_hash: [0u8; 32],
            _lock_file: lock_file,
        };

        // Scan existing events to determine last_seq and last_hash
        journal.recalculate_chain()?;
        Ok(journal)
    }

    fn events_dir(&self) -> PathBuf {
        self.state_dir.join(self.lane_id.as_str()).join("events")
    }

    fn tmp_dir(&self) -> PathBuf {
        self.state_dir.join(self.lane_id.as_str()).join("tmp")
    }

    fn recalculate_chain(&mut self) -> io::Result<()> {
        let entries = self.read_all_entries()?;
        let mut expected_seq = 1;
        let mut last_hash = [0u8; 32];

        for entry in entries {
            if entry.seq != expected_seq {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!(
                        "Journal sequence gap: expected {}, found {}",
                        expected_seq, entry.seq
                    ),
                ));
            }
            if entry.prev_event_sha256 != last_hash {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("Journal hash chain broken at seq {}", entry.seq),
                ));
            }
            if !entry.verify_hash() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!(
                        "Journal entry hash verification failed at seq {}",
                        entry.seq
                    ),
                ));
            }
            last_hash = entry.event_sha256;
            expected_seq += 1;
        }

        self.last_seq = expected_seq - 1;
        self.last_hash = last_hash;
        Ok(())
    }

    pub fn append(&mut self, event: &DurableJournalEvent) -> io::Result<PersistedJournalEntry> {
        if event.lane_id != self.lane_id || event.binding_epoch != self.binding_epoch {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Lane ID or binding epoch mismatch in append",
            ));
        }

        let next_seq = self.last_seq + 1;
        if event.seq != next_seq {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("Expected event sequence {}, got {}", next_seq, event.seq),
            ));
        }

        let event_type = match &event.body {
            DurableEventBody::RunStarted { .. } => "RunStarted",
            DurableEventBody::TaskAccepted { .. } => "TaskAccepted",
            DurableEventBody::DispatchIntent { .. } => "DispatchIntent",
            DurableEventBody::WorkerBound { .. } => "WorkerBound",
            DurableEventBody::ReportStored { .. } => "ReportStored",
            DurableEventBody::DecisionReportStored { .. } => "DecisionReportStored",
            DurableEventBody::WorkerFailed { .. } => "WorkerFailed",
            DurableEventBody::Uncertain { .. } => "Uncertain",
            DurableEventBody::ReportAcked { .. } => "ReportAcked",
            DurableEventBody::DecisionAccepted { .. } => "DecisionAccepted",
            DurableEventBody::PauseRequested { .. } => "PauseRequested",
            DurableEventBody::ResumeRequested { .. } => "ResumeRequested",
            DurableEventBody::RunStopped { .. } => "RunStopped",
            DurableEventBody::Reconciled { .. } => "Reconciled",
        }
        .to_string();

        let persisted = PersistedJournalEntry::new(
            1,
            self.lane_id.clone(),
            self.binding_epoch,
            next_seq,
            event_type,
            self.last_hash,
            event.body.clone(),
        )?;

        let serialized = serde_json::to_vec_pretty(&persisted)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;

        let filename = format!("{:020}.json", next_seq);
        let tmp_path = self.tmp_dir().join(format!("tmp-{}.json", next_seq));
        let final_path = self.events_dir().join(&filename);

        if final_path.exists() {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                format!(
                    "Event file {} already exists, overwrite forbidden",
                    filename
                ),
            ));
        }

        // 1. Write to tmp file on same filesystem
        let mut tmp_file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp_path)?;

        tmp_file.write_all(&serialized)?;
        // 2. sync_all
        tmp_file.sync_all()?;
        drop(tmp_file);

        // 3. Atomic rename (publish)
        fs::rename(&tmp_path, &final_path)?;

        // 4. Sync parent directory
        let events_dir_file = File::open(self.events_dir())?;
        events_dir_file.sync_all()?;

        self.last_seq = next_seq;
        self.last_hash = persisted.event_sha256;

        Ok(persisted)
    }

    pub fn read_all_entries(&self) -> io::Result<Vec<PersistedJournalEntry>> {
        let events_dir = self.events_dir();
        if !events_dir.exists() {
            return Ok(Vec::new());
        }

        let mut paths: Vec<PathBuf> = fs::read_dir(&events_dir)?
            .filter_map(|res| res.ok().map(|e| e.path()))
            .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("json"))
            .collect();

        paths.sort();

        let mut entries = Vec::with_capacity(paths.len());
        for p in paths {
            let mut file = File::open(p)?;
            let mut buf = Vec::new();
            file.read_to_end(&mut buf)?;
            let entry: PersistedJournalEntry = serde_json::from_slice(&buf)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
            entries.push(entry);
        }

        Ok(entries)
    }

    pub fn replay_state(&self) -> Result<LaneState, io::Error> {
        let entries = self.read_all_entries()?;
        let mut state = LaneState::new(self.lane_id.clone(), self.binding_epoch);

        for entry in entries {
            if !entry.verify_hash() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("Hash integrity check failed on replay at seq {}", entry.seq),
                ));
            }
            state.journal_seq = entry.seq;
            match entry.body {
                DurableEventBody::RunStarted { run_id } => {
                    state.phase = Phase::WaitTask;
                    state.enabled = true;
                    state.pause_requested = false;
                    state.run_id = Some(run_id);
                    state.active_task = None;
                    state.active_task_envelope = None;
                    state.worker_turn = None;
                    state.pending_report = None;
                }
                DurableEventBody::TaskAccepted { envelope } => {
                    state.phase = Phase::Accepted;
                    state.active_task = Some(envelope.task_id.clone());
                    state.active_task_envelope = Some(envelope);
                }
                DurableEventBody::DispatchIntent { .. } => {
                    state.phase = Phase::Dispatching;
                }
                DurableEventBody::WorkerBound { turn_id, .. } => {
                    state.phase = Phase::Running;
                    state.worker_turn = Some(turn_id);
                }
                DurableEventBody::ReportStored { report } => {
                    state.phase = Phase::ReportReady;
                    state.last_report_was_decision_required = false;
                    state.pending_report = Some(report);
                }
                DurableEventBody::DecisionReportStored { report } => {
                    state.phase = Phase::ReportReady;
                    state.last_report_was_decision_required = true;
                    state.pending_report = Some(report);
                }
                DurableEventBody::WorkerFailed { failure_report } => {
                    state.phase = Phase::ReportReady;
                    state.last_report_was_decision_required = false;
                    state.pending_report = Some(failure_report);
                }
                DurableEventBody::Uncertain { .. } => {
                    state.phase = Phase::Reconcile;
                }
                DurableEventBody::ReportAcked { report_id } => {
                    state.last_acked_report = Some(report_id);
                    state.pending_report = None;
                    if let Some(active_task_id) = state.active_task.take() {
                        if let Some(active_env) = state.active_task_envelope.take() {
                            state
                                .task_history
                                .insert(active_task_id, active_env.payload_sha256);
                        }
                    }
                    state.worker_turn = None;
                    if state.last_report_was_decision_required {
                        state.phase = Phase::WaitDecision;
                    } else if state.pause_requested {
                        state.phase = Phase::Stopped;
                    } else {
                        state.phase = Phase::WaitTask;
                    }
                }
                DurableEventBody::DecisionAccepted { continuation_task } => {
                    state.phase = Phase::Accepted;
                    state.active_task = Some(continuation_task.task_id.clone());
                    state.active_task_envelope = Some(continuation_task);
                    state.last_report_was_decision_required = false;
                }
                DurableEventBody::PauseRequested { .. } => {
                    state.pause_requested = true;
                }
                DurableEventBody::ResumeRequested { .. } => {
                    state.pause_requested = false;
                }
                DurableEventBody::RunStopped { .. } => {
                    state.phase = Phase::Stopped;
                    state.enabled = false;
                }
                DurableEventBody::Reconciled { outcome } => match outcome {
                    ReconcileOutcome::TaskNotStarted => {
                        state.phase = Phase::Accepted;
                        state.worker_turn = None;
                    }
                    ReconcileOutcome::TaskRunning { turn_id } => {
                        state.phase = Phase::Running;
                        state.worker_turn = Some(turn_id);
                    }
                    ReconcileOutcome::TaskCompleted { report } => {
                        state.phase = Phase::ReportReady;
                        state.last_report_was_decision_required =
                            report.kind == super::envelope::MessageKind::DecisionRequired;
                        state.pending_report = Some(report);
                    }
                    ReconcileOutcome::TaskFailed { report } => {
                        state.phase = Phase::ReportReady;
                        state.last_report_was_decision_required = false;
                        state.pending_report = Some(report);
                    }
                },
            }
        }

        Ok(state)
    }
}
