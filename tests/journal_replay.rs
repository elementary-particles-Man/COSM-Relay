use tuff_relay::core::*;

#[test]
fn test_journal_append_and_replay_cycle() {
    let temp_dir = std::env::temp_dir().join(format!("tuff-journal-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&temp_dir);

    let lane_id = LaneId::new("test-lane").unwrap();
    let epoch = 1;

    // 1. Open journal
    let mut journal = FileJournal::open(&temp_dir, lane_id.clone(), epoch).unwrap();

    // 2. Reduce StartRun and append
    let initial_state = LaneState::new(lane_id.clone(), epoch);
    let run_id = RunId::new("run-001").unwrap();
    let (state_1, effects_1) = reduce(
        &initial_state,
        Event::StartRun {
            lane_id: lane_id.clone(),
            binding_epoch: epoch,
            run_id: run_id.clone(),
        },
    )
    .unwrap();

    for effect in effects_1 {
        if let SideEffect::PersistEvent(durable) = effect {
            journal.append(&durable).unwrap();
        }
    }

    // 3. Reduce TaskReceived and append
    let env = Envelope::new(
        lane_id.clone(),
        epoch,
        run_id.clone(),
        MessageId::new("msg-01").unwrap(),
        TaskId::new("task-01").unwrap(),
        None,
        MessageKind::Task,
        r#"{"action": "test"}"#,
    );
    let (state_2, effects_2) = reduce(&state_1, Event::TaskReceived(env)).unwrap();
    for effect in effects_2 {
        if let SideEffect::PersistEvent(durable) = effect {
            journal.append(&durable).unwrap();
        }
    }

    // 4. Replay from disk and compare state
    let replayed = journal.replay_state().unwrap();
    assert_eq!(replayed.phase, state_2.phase);
    assert_eq!(replayed.journal_seq, state_2.journal_seq);
    assert_eq!(replayed.active_task, state_2.active_task);
    assert_eq!(replayed.run_id, state_2.run_id);

    // Clean up
    let _ = std::fs::remove_dir_all(&temp_dir);
}
