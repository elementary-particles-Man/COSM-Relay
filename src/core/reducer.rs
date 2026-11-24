use super::envelope::{Envelope, MessageKind};
use super::state::{LaneState, Phase};
use super::types::{LaneId, MessageId, RunId, TaskId, TurnId, PROTOCOL_VERSION};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Event {
    StartRun {
        lane_id: LaneId,
        binding_epoch: u64,
        run_id: RunId,
    },
    TaskReceived(Envelope),
    DispatchIntentRecorded {
        lane_id: LaneId,
        binding_epoch: u64,
        task_id: TaskId,
    },
    WorkerTurnBound {
        lane_id: LaneId,
        binding_epoch: u64,
        task_id: TaskId,
        turn_id: TurnId,
    },
    DispatchUncertain {
        lane_id: LaneId,
        binding_epoch: u64,
        task_id: TaskId,
        reason: String,
    },
    WorkerTurnCompleted {
        lane_id: LaneId,
        binding_epoch: u64,
        turn_id: TurnId,
        report_envelope: Envelope,
    },
    WorkerTurnFailed {
        lane_id: LaneId,
        binding_epoch: u64,
        turn_id: TurnId,
        reason: String,
    },
    WorkerOutcomeUncertain {
        lane_id: LaneId,
        binding_epoch: u64,
        turn_id: TurnId,
        reason: String,
    },
    SupervisorAckReceived {
        lane_id: LaneId,
        binding_epoch: u64,
        report_id: MessageId,
    },
    DecisionResponseAccepted {
        lane_id: LaneId,
        binding_epoch: u64,
        continuation_task: Envelope,
    },
    PauseRequested {
        lane_id: LaneId,
    },
    ResumeRequested {
        lane_id: LaneId,
    },
    StopRequested {
        lane_id: LaneId,
    },
    Reconciled {
        lane_id: LaneId,
        binding_epoch: u64,
        target_phase: Phase,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SideEffect {
    PersistEvent {
        event_name: String,
        details: String,
    },
    DispatchWorkerTask {
        task_id: TaskId,
        payload_utf8: String,
        payload_sha256: [u8; 32],
    },
    NotifySupervisorReport {
        report: Envelope,
    },
    NotifyOperator {
        message: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReduceError {
    LaneMismatch,
    EpochMismatch,
    UnsupportedProtocolVersion,
    PayloadHashMismatch,
    InvalidJsonPayload,
    TaskConflictSameIdDifferentPayload,
    DuplicateTaskIgnored,
    InvalidPhaseTransition {
        current: Phase,
        attempted: &'static str,
    },
    PausedCannotDispatch,
    NotEnabled,
    TurnMismatch,
    ReportIdMismatch,
}

pub fn reduce(
    state: &LaneState,
    event: Event,
) -> Result<(LaneState, Vec<SideEffect>), ReduceError> {
    match event {
        Event::StartRun {
            lane_id,
            binding_epoch,
            run_id,
        } => {
            if lane_id != state.lane_id {
                return Err(ReduceError::LaneMismatch);
            }
            if binding_epoch != state.binding_epoch {
                return Err(ReduceError::EpochMismatch);
            }
            if state.phase != Phase::Idle && state.phase != Phase::Stopped {
                return Err(ReduceError::InvalidPhaseTransition {
                    current: state.phase,
                    attempted: "StartRun",
                });
            }
            let mut next = state.clone();
            next.phase = Phase::WaitTask;
            next.enabled = true;
            next.pause_requested = false;
            next.run_id = Some(run_id);
            next.active_task = None;
            next.active_task_hash = None;
            next.worker_turn = None;
            next.pending_report = None;
            next.journal_seq += 1;
            let effects = vec![SideEffect::PersistEvent {
                event_name: "RunStarted".to_string(),
                details: format!("run_id={}", next.run_id.as_ref().unwrap()),
            }];
            Ok((next, effects))
        }

        Event::TaskReceived(envelope) => {
            if envelope.protocol_version != PROTOCOL_VERSION {
                return Err(ReduceError::UnsupportedProtocolVersion);
            }
            if envelope.lane_id != state.lane_id {
                return Err(ReduceError::LaneMismatch);
            }
            if envelope.binding_epoch != state.binding_epoch {
                return Err(ReduceError::EpochMismatch);
            }
            envelope
                .verify_integrity()
                .map_err(|_| ReduceError::PayloadHashMismatch)?;

            // Verify payload is valid JSON
            if serde_json::from_str::<serde_json::Value>(&envelope.payload_utf8).is_err() {
                return Err(ReduceError::InvalidJsonPayload);
            }

            if state.phase == Phase::Accepted
                || state.phase == Phase::Dispatching
                || state.phase == Phase::Running
            {
                if let Some(active_id) = &state.active_task {
                    if active_id == &envelope.task_id {
                        if state.active_task_hash == Some(envelope.payload_sha256) {
                            // Idempotent resubmission of identical task
                            return Err(ReduceError::DuplicateTaskIgnored);
                        } else {
                            // Conflict: same ID, different payload
                            return Err(ReduceError::TaskConflictSameIdDifferentPayload);
                        }
                    }
                }
            }

            if state.phase != Phase::WaitTask {
                return Err(ReduceError::InvalidPhaseTransition {
                    current: state.phase,
                    attempted: "TaskReceived",
                });
            }

            let mut next = state.clone();
            next.phase = Phase::Accepted;
            next.active_task = Some(envelope.task_id.clone());
            next.active_task_hash = Some(envelope.payload_sha256);
            next.journal_seq += 1;

            let mut effects = vec![SideEffect::PersistEvent {
                event_name: "TaskAccepted".to_string(),
                details: format!("task_id={}", envelope.task_id),
            }];

            if !next.pause_requested && next.enabled {
                effects.push(SideEffect::PersistEvent {
                    event_name: "DispatchIntent".to_string(),
                    details: format!("task_id={}", envelope.task_id),
                });
            }

            Ok((next, effects))
        }

        Event::DispatchIntentRecorded {
            lane_id,
            binding_epoch,
            task_id,
        } => {
            if lane_id != state.lane_id {
                return Err(ReduceError::LaneMismatch);
            }
            if binding_epoch != state.binding_epoch {
                return Err(ReduceError::EpochMismatch);
            }
            if state.phase != Phase::Accepted {
                return Err(ReduceError::InvalidPhaseTransition {
                    current: state.phase,
                    attempted: "DispatchIntentRecorded",
                });
            }
            if state.pause_requested || !state.enabled {
                return Err(ReduceError::PausedCannotDispatch);
            }
            if state.active_task.as_ref() != Some(&task_id) {
                return Err(ReduceError::InvalidPhaseTransition {
                    current: state.phase,
                    attempted: "DispatchIntentRecorded(TaskMismatch)",
                });
            }

            let mut next = state.clone();
            next.phase = Phase::Dispatching;
            next.journal_seq += 1;

            let effects = vec![SideEffect::DispatchWorkerTask {
                task_id: task_id.clone(),
                payload_utf8: String::new(), // will be populated from journal in runtime
                payload_sha256: state.active_task_hash.unwrap_or([0u8; 32]),
            }];
            Ok((next, effects))
        }

        Event::WorkerTurnBound {
            lane_id,
            binding_epoch,
            task_id,
            turn_id,
        } => {
            if lane_id != state.lane_id {
                return Err(ReduceError::LaneMismatch);
            }
            if binding_epoch != state.binding_epoch {
                return Err(ReduceError::EpochMismatch);
            }
            if state.phase != Phase::Dispatching {
                return Err(ReduceError::InvalidPhaseTransition {
                    current: state.phase,
                    attempted: "WorkerTurnBound",
                });
            }
            if state.active_task.as_ref() != Some(&task_id) {
                return Err(ReduceError::InvalidPhaseTransition {
                    current: state.phase,
                    attempted: "WorkerTurnBound(TaskMismatch)",
                });
            }

            let mut next = state.clone();
            next.phase = Phase::Running;
            next.worker_turn = Some(turn_id.clone());
            next.journal_seq += 1;

            let effects = vec![SideEffect::PersistEvent {
                event_name: "WorkerBound".to_string(),
                details: format!("turn_id={}", turn_id),
            }];
            Ok((next, effects))
        }

        Event::DispatchUncertain {
            lane_id,
            binding_epoch,
            task_id: _,
            reason,
        } => {
            if lane_id != state.lane_id {
                return Err(ReduceError::LaneMismatch);
            }
            if binding_epoch != state.binding_epoch {
                return Err(ReduceError::EpochMismatch);
            }
            if state.phase != Phase::Dispatching && state.phase != Phase::Accepted {
                return Err(ReduceError::InvalidPhaseTransition {
                    current: state.phase,
                    attempted: "DispatchUncertain",
                });
            }

            let mut next = state.clone();
            next.phase = Phase::Reconcile;
            next.journal_seq += 1;

            let effects = vec![
                SideEffect::PersistEvent {
                    event_name: "Uncertain".to_string(),
                    details: format!("dispatch_uncertain: {}", reason),
                },
                SideEffect::NotifyOperator {
                    message: format!("Reconciliation required: {}", reason),
                },
            ];
            Ok((next, effects))
        }

        Event::WorkerTurnCompleted {
            lane_id,
            binding_epoch,
            turn_id,
            report_envelope,
        } => {
            if lane_id != state.lane_id {
                return Err(ReduceError::LaneMismatch);
            }
            if binding_epoch != state.binding_epoch {
                return Err(ReduceError::EpochMismatch);
            }
            if state.phase != Phase::Running {
                return Err(ReduceError::InvalidPhaseTransition {
                    current: state.phase,
                    attempted: "WorkerTurnCompleted",
                });
            }
            if state.worker_turn.as_ref() != Some(&turn_id) {
                return Err(ReduceError::TurnMismatch);
            }
            report_envelope
                .verify_integrity()
                .map_err(|_| ReduceError::PayloadHashMismatch)?;

            let is_decision_required = report_envelope.kind == MessageKind::DecisionRequired;

            let mut next = state.clone();
            next.phase = Phase::ReportReady;
            next.pending_report = Some(report_envelope.message_id.clone());
            next.journal_seq += 1;

            let event_name = if is_decision_required {
                "DecisionReportStored"
            } else {
                "ReportStored"
            };

            let effects = vec![
                SideEffect::PersistEvent {
                    event_name: event_name.to_string(),
                    details: format!("report_id={}", report_envelope.message_id),
                },
                SideEffect::NotifySupervisorReport {
                    report: report_envelope,
                },
            ];
            Ok((next, effects))
        }

        Event::WorkerTurnFailed {
            lane_id,
            binding_epoch,
            turn_id,
            reason,
        } => {
            if lane_id != state.lane_id {
                return Err(ReduceError::LaneMismatch);
            }
            if binding_epoch != state.binding_epoch {
                return Err(ReduceError::EpochMismatch);
            }
            if state.phase != Phase::Running {
                return Err(ReduceError::InvalidPhaseTransition {
                    current: state.phase,
                    attempted: "WorkerTurnFailed",
                });
            }
            if state.worker_turn.as_ref() != Some(&turn_id) {
                return Err(ReduceError::TurnMismatch);
            }

            let mut next = state.clone();
            next.phase = Phase::ReportReady;
            next.journal_seq += 1;

            let effects = vec![
                SideEffect::PersistEvent {
                    event_name: "WorkerFailed".to_string(),
                    details: reason.clone(),
                },
                SideEffect::NotifyOperator {
                    message: format!("Worker turn failed: {}", reason),
                },
            ];
            Ok((next, effects))
        }

        Event::WorkerOutcomeUncertain {
            lane_id,
            binding_epoch,
            turn_id,
            reason,
        } => {
            if lane_id != state.lane_id {
                return Err(ReduceError::LaneMismatch);
            }
            if binding_epoch != state.binding_epoch {
                return Err(ReduceError::EpochMismatch);
            }
            if state.phase != Phase::Running {
                return Err(ReduceError::InvalidPhaseTransition {
                    current: state.phase,
                    attempted: "WorkerOutcomeUncertain",
                });
            }
            if state.worker_turn.as_ref() != Some(&turn_id) {
                return Err(ReduceError::TurnMismatch);
            }

            let mut next = state.clone();
            next.phase = Phase::Reconcile;
            next.journal_seq += 1;

            let effects = vec![
                SideEffect::PersistEvent {
                    event_name: "Uncertain".to_string(),
                    details: format!("worker_outcome_uncertain: {}", reason),
                },
                SideEffect::NotifyOperator {
                    message: format!("Worker turn outcome uncertain: {}", reason),
                },
            ];
            Ok((next, effects))
        }

        Event::SupervisorAckReceived {
            lane_id,
            binding_epoch,
            report_id,
        } => {
            if lane_id != state.lane_id {
                return Err(ReduceError::LaneMismatch);
            }
            if binding_epoch != state.binding_epoch {
                return Err(ReduceError::EpochMismatch);
            }
            if state.phase != Phase::ReportReady {
                return Err(ReduceError::InvalidPhaseTransition {
                    current: state.phase,
                    attempted: "SupervisorAckReceived",
                });
            }
            if state.pending_report.as_ref() != Some(&report_id) {
                return Err(ReduceError::ReportIdMismatch);
            }

            let mut next = state.clone();
            next.last_acked_report = Some(report_id.clone());
            next.pending_report = None;
            next.active_task = None;
            next.active_task_hash = None;
            next.worker_turn = None;
            next.journal_seq += 1;

            // If pause was requested, move to Stopped, otherwise WaitTask
            if next.pause_requested {
                next.phase = Phase::Stopped;
            } else {
                next.phase = Phase::WaitTask;
            }

            let effects = vec![SideEffect::PersistEvent {
                event_name: "ReportAcked".to_string(),
                details: format!("report_id={}", report_id),
            }];
            Ok((next, effects))
        }

        Event::DecisionResponseAccepted {
            lane_id,
            binding_epoch,
            continuation_task,
        } => {
            if lane_id != state.lane_id {
                return Err(ReduceError::LaneMismatch);
            }
            if binding_epoch != state.binding_epoch {
                return Err(ReduceError::EpochMismatch);
            }
            if state.phase != Phase::WaitDecision {
                return Err(ReduceError::InvalidPhaseTransition {
                    current: state.phase,
                    attempted: "DecisionResponseAccepted",
                });
            }
            continuation_task
                .verify_integrity()
                .map_err(|_| ReduceError::PayloadHashMismatch)?;

            let mut next = state.clone();
            next.phase = Phase::Accepted;
            next.active_task = Some(continuation_task.task_id.clone());
            next.active_task_hash = Some(continuation_task.payload_sha256);
            next.journal_seq += 1;

            let effects = vec![SideEffect::PersistEvent {
                event_name: "DecisionAccepted".to_string(),
                details: format!("task_id={}", continuation_task.task_id),
            }];
            Ok((next, effects))
        }

        Event::PauseRequested { lane_id } => {
            if lane_id != state.lane_id {
                return Err(ReduceError::LaneMismatch);
            }
            let mut next = state.clone();
            next.pause_requested = true;
            next.journal_seq += 1;

            let effects = vec![SideEffect::PersistEvent {
                event_name: "PauseRequested".to_string(),
                details: format!("current_phase={:?}", state.phase),
            }];
            Ok((next, effects))
        }

        Event::ResumeRequested { lane_id } => {
            if lane_id != state.lane_id {
                return Err(ReduceError::LaneMismatch);
            }
            let mut next = state.clone();
            next.pause_requested = false;
            next.journal_seq += 1;

            let mut effects = vec![SideEffect::PersistEvent {
                event_name: "ResumeRequested".to_string(),
                details: format!("current_phase={:?}", state.phase),
            }];

            if next.phase == Phase::Accepted && next.enabled {
                effects.push(SideEffect::PersistEvent {
                    event_name: "DispatchIntent".to_string(),
                    details: format!("task_id={:?}", next.active_task),
                });
            }

            Ok((next, effects))
        }

        Event::StopRequested { lane_id } => {
            if lane_id != state.lane_id {
                return Err(ReduceError::LaneMismatch);
            }
            if state.phase != Phase::WaitTask
                && state.phase != Phase::Idle
                && state.phase != Phase::Stopped
            {
                return Err(ReduceError::InvalidPhaseTransition {
                    current: state.phase,
                    attempted: "StopRequestedWhileActive",
                });
            }

            let mut next = state.clone();
            next.phase = Phase::Stopped;
            next.enabled = false;
            next.journal_seq += 1;

            let effects = vec![SideEffect::PersistEvent {
                event_name: "RunStopped".to_string(),
                details: format!("run_id={:?}", next.run_id),
            }];
            Ok((next, effects))
        }

        Event::Reconciled {
            lane_id,
            binding_epoch,
            target_phase,
        } => {
            if lane_id != state.lane_id {
                return Err(ReduceError::LaneMismatch);
            }
            if binding_epoch != state.binding_epoch {
                return Err(ReduceError::EpochMismatch);
            }
            if state.phase != Phase::Reconcile {
                return Err(ReduceError::InvalidPhaseTransition {
                    current: state.phase,
                    attempted: "Reconciled",
                });
            }

            let mut next = state.clone();
            next.phase = target_phase;
            next.journal_seq += 1;

            let effects = vec![SideEffect::PersistEvent {
                event_name: "Reconciled".to_string(),
                details: format!("target_phase={:?}", target_phase),
            }];
            Ok((next, effects))
        }
    }
}
