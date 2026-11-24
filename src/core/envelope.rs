use super::types::{LaneId, MessageId, RunId, TaskId, PROTOCOL_VERSION};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageKind {
    Task,
    Report,
    DecisionRequired,
    Stop,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Envelope {
    pub protocol_version: u16,
    pub lane_id: LaneId,
    pub binding_epoch: u64,
    pub run_id: RunId,
    pub message_id: MessageId,
    pub task_id: TaskId,
    pub in_reply_to: Option<MessageId>,
    pub kind: MessageKind,
    pub payload_utf8: String,
    pub payload_sha256: [u8; 32],
}

impl Envelope {
    pub fn compute_sha256(payload: &str) -> [u8; 32] {
        let mut hasher = Sha256::new();
        hasher.update(payload.as_bytes());
        hasher.finalize().into()
    }

    #[allow(clippy::too_many_arguments)]
    pub fn new(
        lane_id: LaneId,
        binding_epoch: u64,
        run_id: RunId,
        message_id: MessageId,
        task_id: TaskId,
        in_reply_to: Option<MessageId>,
        kind: MessageKind,
        payload_utf8: String,
    ) -> Self {
        let payload_sha256 = Self::compute_sha256(&payload_utf8);
        Self {
            protocol_version: PROTOCOL_VERSION,
            lane_id,
            binding_epoch,
            run_id,
            message_id,
            task_id,
            in_reply_to,
            kind,
            payload_utf8,
            payload_sha256,
        }
    }

    pub fn verify_integrity(&self) -> Result<(), &'static str> {
        if self.protocol_version != PROTOCOL_VERSION {
            return Err("Unsupported protocol version");
        }
        let calculated = Self::compute_sha256(&self.payload_utf8);
        if calculated != self.payload_sha256 {
            return Err("Payload SHA-256 mismatch");
        }
        Ok(())
    }
}
