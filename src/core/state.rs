use super::types::{LaneId, MessageId, RunId, TaskId, TurnId};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Phase {
    Idle,
    WaitTask,
    Accepted,
    Dispatching,
    Running,
    ReportReady,
    WaitDecision,
    Reconcile,
    Stopped,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LaneState {
    pub lane_id: LaneId,
    pub binding_epoch: u64,
    pub phase: Phase,
    pub enabled: bool,
    pub pause_requested: bool,
    pub run_id: Option<RunId>,
    pub active_task: Option<TaskId>,
    pub active_task_hash: Option<[u8; 32]>,
    pub worker_turn: Option<TurnId>,
    pub pending_report: Option<MessageId>,
    pub last_acked_report: Option<MessageId>,
    pub journal_seq: u64,
}

impl LaneState {
    pub fn new(lane_id: LaneId, binding_epoch: u64) -> Self {
        Self {
            lane_id,
            binding_epoch,
            phase: Phase::Idle,
            enabled: false,
            pause_requested: false,
            run_id: None,
            active_task: None,
            active_task_hash: None,
            worker_turn: None,
            pending_report: None,
            last_acked_report: None,
            journal_seq: 0,
        }
    }
}
