use crate::core::envelope::Envelope;
use crate::core::types::{LaneId, MessageId, TaskId};
use crate::runtime::RelayRuntime;
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};

/// Minimum tools exposed to Supervisor according to TR-DD-001 §6.3:
/// submit, get_report, ack, finish, status.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LaneStatusDto {
    pub lane_id: String,
    pub phase: String,
    pub binding_epoch: u64,
    pub active_task: Option<String>,
    pub pending_report: Option<String>,
    pub is_decision_required: bool,
    pub pause_requested: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubmitTaskRequest {
    pub lane_id: String,
    pub task_id: String,
    pub payload_json: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubmitTaskResponse {
    pub accepted: bool,
    pub message_id: String,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetReportResponse {
    pub report_available: bool,
    pub report_id: Option<String>,
    pub is_decision_required: bool,
    pub payload_utf8: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AckReportRequest {
    pub lane_id: String,
    pub report_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AckReportResponse {
    pub acked: bool,
    pub new_phase: String,
}

pub struct SupervisorAdapter {
    runtime: Arc<Mutex<RelayRuntime>>,
    allowed_lane_id: LaneId,
}

impl SupervisorAdapter {
    pub fn new(runtime: Arc<Mutex<RelayRuntime>>, allowed_lane_id: LaneId) -> Self {
        Self {
            runtime,
            allowed_lane_id,
        }
    }

    /// Check lane status (Read-only tool)
    pub fn status(&self, lane_id_str: &str) -> Result<LaneStatusDto, String> {
        if lane_id_str != self.allowed_lane_id.as_str() {
            return Err(format!("Unauthorized lane ID: {}", lane_id_str));
        }
        let rt = self.runtime.lock().map_err(|e| e.to_string())?;
        let state = rt
            .get_state(&self.allowed_lane_id)
            .ok_or_else(|| "Lane not registered in runtime".to_string())?;

        Ok(LaneStatusDto {
            lane_id: state.lane_id.as_str().to_string(),
            phase: format!("{:?}", state.phase),
            binding_epoch: state.binding_epoch,
            active_task: state.active_task.as_ref().map(|t| t.as_str().to_string()),
            pending_report: state
                .pending_report
                .as_ref()
                .map(|r| r.message_id.as_str().to_string()),
            is_decision_required: state.last_report_was_decision_required,
            pause_requested: state.pause_requested,
        })
    }

    /// Submit a task from supervisor (Write-capable tool)
    pub fn submit(&self, req: SubmitTaskRequest) -> Result<SubmitTaskResponse, String> {
        if req.lane_id != self.allowed_lane_id.as_str() {
            return Err(format!("Unauthorized lane ID: {}", req.lane_id));
        }

        // Validate payload JSON string
        if serde_json::from_str::<serde_json::Value>(&req.payload_json).is_err() {
            return Err("Invalid JSON payload".to_string());
        }

        let mut rt = self.runtime.lock().map_err(|e| e.to_string())?;
        let state = rt
            .get_state(&self.allowed_lane_id)
            .ok_or_else(|| "Lane not registered".to_string())?;

        let run_id = state
            .run_id
            .clone()
            .ok_or_else(|| "Run not started on lane".to_string())?;

        let msg_id = format!("msg-sub-{}", uuid_placeholder());
        let env = Envelope::new(
            self.allowed_lane_id.clone(),
            state.binding_epoch,
            run_id,
            MessageId::new(&msg_id).map_err(|e| format!("{:?}", e))?,
            TaskId::new(&req.task_id).map_err(|e| format!("{:?}", e))?,
            None,
            crate::core::envelope::MessageKind::Task,
            req.payload_json.as_str(),
        );

        match rt.handle_event(crate::core::reducer::Event::TaskReceived(env)) {
            Ok(_) => Ok(SubmitTaskResponse {
                accepted: true,
                message_id: msg_id,
                error: None,
            }),
            Err(e) => Ok(SubmitTaskResponse {
                accepted: false,
                message_id: msg_id,
                error: Some(format!("{:?}", e)),
            }),
        }
    }

    /// Retrieve the pending report if available
    pub fn get_report(&self, lane_id_str: &str) -> Result<GetReportResponse, String> {
        if lane_id_str != self.allowed_lane_id.as_str() {
            return Err(format!("Unauthorized lane ID: {}", lane_id_str));
        }
        let rt = self.runtime.lock().map_err(|e| e.to_string())?;
        let state = rt
            .get_state(&self.allowed_lane_id)
            .ok_or_else(|| "Lane not registered".to_string())?;

        if let Some(rep) = &state.pending_report {
            Ok(GetReportResponse {
                report_available: true,
                report_id: Some(rep.message_id.as_str().to_string()),
                is_decision_required: state.last_report_was_decision_required,
                payload_utf8: Some(rep.payload_utf8.to_string()),
            })
        } else {
            Ok(GetReportResponse {
                report_available: false,
                report_id: None,
                is_decision_required: false,
                payload_utf8: None,
            })
        }
    }

    /// Ack report
    pub fn ack(&self, req: AckReportRequest) -> Result<AckReportResponse, String> {
        if req.lane_id != self.allowed_lane_id.as_str() {
            return Err(format!("Unauthorized lane ID: {}", req.lane_id));
        }
        let mut rt = self.runtime.lock().map_err(|e| e.to_string())?;
        let (run_id, epoch) = {
            let state = rt
                .get_state(&self.allowed_lane_id)
                .ok_or_else(|| "Lane not registered".to_string())?;
            (
                state
                    .run_id
                    .clone()
                    .ok_or_else(|| "Run not started on lane".to_string())?,
                state.binding_epoch,
            )
        };
        let rep_id = MessageId::new(&req.report_id).map_err(|e| format!("{:?}", e))?;

        let (new_state, _) = rt
            .handle_event(crate::core::reducer::Event::SupervisorAckReceived {
                lane_id: self.allowed_lane_id.clone(),
                binding_epoch: epoch,
                run_id,
                report_id: rep_id,
            })
            .map_err(|e| format!("{:?}", e))?;

        Ok(AckReportResponse {
            acked: true,
            new_phase: format!("{:?}", new_state.phase),
        })
    }
}

fn uuid_placeholder() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(1)
}
