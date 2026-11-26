use super::core::*;
use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
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

        let (new_state, effects) = reduce(current_state, event)?;

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
}
