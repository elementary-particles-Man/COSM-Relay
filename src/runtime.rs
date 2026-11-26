use crate::adapters::codex::CodexAppServerClient;
use crate::core::envelope::Envelope;
use crate::core::reducer::{Event, ReduceError, SideEffect};
use crate::core::state::LaneState;
use crate::core::types::{LaneId, MessageId};
use crate::FileJournal;
use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct LaneConfig {
    pub id: LaneId,
    pub repo_root: PathBuf,
    pub binding_epoch: u64,
    pub worker_thread_id: Option<String>,
}

pub struct RelayRuntime {
    state_dir: PathBuf,
    lanes: HashMap<LaneId, LaneConfig>,
    states: HashMap<LaneId, LaneState>,
    journals: HashMap<LaneId, FileJournal>,
}

impl RelayRuntime {
    pub fn new(state_dir: impl AsRef<Path>) -> Self {
        Self {
            state_dir: state_dir.as_ref().to_path_buf(),
            lanes: HashMap::new(),
            states: HashMap::new(),
            journals: HashMap::new(),
        }
    }

    pub fn register_lane(&mut self, config: LaneConfig) -> io::Result<()> {
        // Enforce normalized paths and prevent duplicate lane registration
        if self.lanes.contains_key(&config.id) {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                format!("Lane {} is already registered", config.id.as_str()),
            ));
        }

        // Validate repo root exists
        if !config.repo_root.exists() {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("Repo root {:?} does not exist", config.repo_root),
            ));
        }

        let journal = FileJournal::open(&self.state_dir, config.id.clone(), config.binding_epoch)?;
        let replayed_state = journal.replay_state()?;
        self.states.insert(config.id.clone(), replayed_state);
        self.journals.insert(config.id.clone(), journal);
        self.lanes.insert(config.id.clone(), config);
        Ok(())
    }

    pub fn get_state(&self, lane_id: &LaneId) -> Option<&LaneState> {
        self.states.get(lane_id)
    }

    pub fn all_states(&self) -> &HashMap<LaneId, LaneState> {
        &self.states
    }

    pub fn lane_config(&self, lane_id: &LaneId) -> Option<&LaneConfig> {
        self.lanes.get(lane_id)
    }

    pub fn handle_event(
        &mut self,
        event: Event,
    ) -> Result<(LaneState, Vec<SideEffect>), ReduceError> {
        let lane_id = match &event {
            Event::StartRun { lane_id, .. } => lane_id.clone(),
            Event::TaskReceived(env) => env.lane_id.clone(),
            Event::DispatchIntentRecorded { lane_id, .. } => lane_id.clone(),
            Event::WorkerTurnBound { lane_id, .. } => lane_id.clone(),
            Event::DispatchUncertain { lane_id, .. } => lane_id.clone(),
            Event::WorkerTurnCompleted { lane_id, .. } => lane_id.clone(),
            Event::WorkerTurnFailed { lane_id, .. } => lane_id.clone(),
            Event::WorkerOutcomeUncertain { lane_id, .. } => lane_id.clone(),
            Event::SupervisorAckReceived { lane_id, .. } => lane_id.clone(),
            Event::DecisionResponseAccepted { lane_id, .. } => lane_id.clone(),
            Event::PauseRequested { lane_id } => lane_id.clone(),
            Event::ResumeRequested { lane_id } => lane_id.clone(),
            Event::StopRequested { lane_id } => lane_id.clone(),
            Event::Reconciled { lane_id, .. } => lane_id.clone(),
        };

        let current_state = self.states.get(&lane_id).ok_or(ReduceError::LaneMismatch)?;

        let (new_state, effects) = crate::core::reducer::reduce(current_state, event)?;

        // Persist events before executing external effects
        if let Some(journal) = self.journals.get_mut(&lane_id) {
            for effect in &effects {
                if let SideEffect::PersistEvent(durable) = effect {
                    journal
                        .append(durable)
                        .map_err(|_| ReduceError::InvalidPhaseTransition {
                            current: new_state.phase,
                            attempted: "StorageFailure",
                        })?;
                }
            }
        }

        self.states.insert(lane_id, new_state.clone());
        Ok((new_state, effects))
    }

    /// Process active dispatch side effect by invoking codex client turn using registered worker binding
    pub fn execute_dispatch_to_worker(
        &mut self,
        lane_id: &LaneId,
        codex_client: &mut CodexAppServerClient,
    ) -> Result<String, String> {
        let state = self.get_state(lane_id).ok_or("Lane not found")?.clone();
        let active_task = state.active_task.ok_or("No active task on lane")?;
        let task_env = state
            .active_task_envelope
            .ok_or("No active task envelope on lane")?;

        let thread_id = {
            let cfg = self.lane_config(lane_id).ok_or("Lane config not found")?;
            cfg.worker_thread_id
                .clone()
                .ok_or("No registered worker_thread_id for lane")?
        };

        // 1. Record dispatch intent
        self.handle_event(Event::DispatchIntentRecorded {
            lane_id: lane_id.clone(),
            binding_epoch: state.binding_epoch,
            run_id: state.run_id.clone().ok_or("No active run")?,
            task_id: active_task.clone(),
        })
        .map_err(|e| format!("{:?}", e))?;

        // 2. Start turn on Codex
        let turn_id_str = match codex_client.turn_start(&thread_id, &task_env.payload_utf8) {
            Ok(tid) => tid,
            Err(e) => {
                // If turn start failed or uncertain, transition to DispatchUncertain
                let _ = self.handle_event(Event::DispatchUncertain {
                    lane_id: lane_id.clone(),
                    binding_epoch: state.binding_epoch,
                    run_id: state
                        .run_id
                        .clone()
                        .unwrap_or_else(|| crate::core::types::RunId::new("unknown").unwrap()),
                    task_id: active_task.clone(),
                    reason: format!("Failed to start turn on worker: {:?}", e),
                });
                return Err(format!("Codex turn_start error: {:?}", e));
            }
        };

        let turn_id = crate::core::types::TurnId::new(&turn_id_str)
            .map_err(|e| format!("Invalid turn ID from worker: {:?}", e))?;

        // 3. Bind worker turn
        self.handle_event(Event::WorkerTurnBound {
            lane_id: lane_id.clone(),
            binding_epoch: state.binding_epoch,
            run_id: state.run_id.clone().ok_or("No active run")?,
            task_id: active_task.clone(),
            turn_id: turn_id.clone(),
        })
        .map_err(|e| format!("{:?}", e))?;

        // 4. Wait for turn completion
        let result = match codex_client.wait_turn_completion(&turn_id_str) {
            Ok(res) => res,
            Err(e) => {
                let _ = self.handle_event(Event::WorkerOutcomeUncertain {
                    lane_id: lane_id.clone(),
                    binding_epoch: state.binding_epoch,
                    run_id: state
                        .run_id
                        .clone()
                        .unwrap_or_else(|| crate::core::types::RunId::new("unknown").unwrap()),
                    turn_id: turn_id.clone(),
                    reason: format!("Worker turn completion uncertain: {:?}", e),
                });
                return Err(format!("Codex wait_turn_completion error: {:?}", e));
            }
        };

        // 5. Construct report envelope
        let report_msg_id = format!("rep-{}", turn_id_str);
        let is_decision_required = result.report_text.contains("DECISION_REQUIRED")
            || result.report_text.contains("decision_required");
        let kind = if is_decision_required {
            crate::core::envelope::MessageKind::DecisionRequired
        } else {
            crate::core::envelope::MessageKind::Report
        };

        let rep_env = Envelope::new(
            lane_id.clone(),
            state.binding_epoch,
            state.run_id.clone().ok_or("No active run")?,
            MessageId::new(&report_msg_id).map_err(|e| format!("{:?}", e))?,
            active_task,
            Some(task_env.message_id.clone()),
            kind,
            result.report_text.as_str(),
        );

        // 6. Complete worker turn
        self.handle_event(Event::WorkerTurnCompleted {
            lane_id: lane_id.clone(),
            binding_epoch: state.binding_epoch,
            run_id: state.run_id.clone().ok_or("No active run")?,
            turn_id,
            report_envelope: rep_env,
        })
        .map_err(|e| format!("{:?}", e))?;

        Ok(result.report_text)
    }
}

/// Continuous Background Worker Driver for a registered Lane
pub struct LaneDriver {
    running: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl LaneDriver {
    pub fn spawn(
        runtime_arc: Arc<Mutex<RelayRuntime>>,
        lane_id: LaneId,
        codex_bin_path: String,
    ) -> Self {
        let running = Arc::new(AtomicBool::new(true));
        let running_clone = running.clone();

        let handle = thread::spawn(move || {
            let mut codex_client = match CodexAppServerClient::spawn(&codex_bin_path) {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("LaneDriver failed to spawn codex client: {:?}", e);
                    return;
                }
            };

            if let Err(e) = codex_client.initialize() {
                eprintln!("LaneDriver codex initialize failed: {:?}", e);
                return;
            }

            // Resume bound worker thread if configured
            {
                let rt = runtime_arc.lock().unwrap();
                if let Some(cfg) = rt.lane_config(&lane_id) {
                    if let Some(ref tid) = cfg.worker_thread_id {
                        let _ = codex_client.thread_resume(tid);
                    }
                }
            }

            while running_clone.load(Ordering::Relaxed) {
                let should_dispatch = {
                    let rt = runtime_arc.lock().unwrap();
                    if let Some(st) = rt.get_state(&lane_id) {
                        st.phase == crate::core::state::Phase::Accepted && !st.pause_requested
                    } else {
                        false
                    }
                };

                if should_dispatch {
                    let mut rt = runtime_arc.lock().unwrap();
                    let _ = rt.execute_dispatch_to_worker(&lane_id, &mut codex_client);
                } else {
                    thread::sleep(std::time::Duration::from_millis(100));
                }
            }
        });

        Self {
            running,
            handle: Some(handle),
        }
    }

    pub fn stop(&mut self) {
        self.running.store(false, Ordering::Relaxed);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

impl Drop for LaneDriver {
    fn drop(&mut self) {
        self.stop();
    }
}
