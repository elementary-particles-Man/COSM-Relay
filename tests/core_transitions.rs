use tuff_relay::core::*;

fn make_test_envelope(
    lane_id: &str,
    epoch: u64,
    task_id: &str,
    kind: MessageKind,
    payload: &str,
) -> Envelope {
    Envelope::new(
        LaneId::new(lane_id).unwrap(),
        epoch,
        RunId::new("run-001").unwrap(),
        MessageId::new(format!("msg-{}", task_id)).unwrap(),
        TaskId::new(task_id).unwrap(),
        None,
        kind,
        payload.to_string(),
    )
}

#[test]
fn test_envelope_creation_and_integrity() {
    let payload = r#"{"task": "日本語、Unicode (🚀)、改行\nを含むタスク"}"#;
    let env = make_test_envelope("tuff-relay", 1, "task-01", MessageKind::Task, payload);
    assert_eq!(env.protocol_version, PROTOCOL_VERSION);
    assert!(env.verify_integrity().is_ok());

    let mut corrupted = env.clone();
    corrupted.payload_utf8 = r#"{"task": "改ざんされたペイロード"}"#.to_string();
    assert!(corrupted.verify_integrity().is_err());
}

#[test]
fn test_json_roundtrip_sha256_preservation() {
    let payload = r#"{"complex": {"nested": [1, 2, 3], "japanese": "テストデータ", "escapes": "\"\\/\b\f\n\r\t"}}"#;
    let env = make_test_envelope("tuff-relay", 1, "task-01", MessageKind::Task, payload);
    let original_hash = env.payload_sha256;

    let serialized = serde_json::to_string(&env).unwrap();
    let deserialized: Envelope = serde_json::from_str(&serialized).unwrap();

    assert_eq!(deserialized.payload_sha256, original_hash);
    assert_eq!(deserialized.payload_utf8, payload);
    assert!(deserialized.verify_integrity().is_ok());
}

#[test]
fn test_normal_cycle_transitions() {
    let lane_id = LaneId::new("tuff-relay").unwrap();
    let state = LaneState::new(lane_id.clone(), 1);
    assert_eq!(state.phase, Phase::Idle);

    // 1. StartRun
    let (state, effects) = reduce(
        &state,
        Event::StartRun {
            lane_id: lane_id.clone(),
            binding_epoch: 1,
            run_id: RunId::new("run-001").unwrap(),
        },
    )
    .unwrap();
    assert_eq!(state.phase, Phase::WaitTask);
    assert!(state.enabled);
    assert_eq!(effects.len(), 1);

    // 2. TaskReceived
    let task_payload = r#"{"action": "implement"}"#;
    let task_env = make_test_envelope("tuff-relay", 1, "task-01", MessageKind::Task, task_payload);
    let (state, _) = reduce(&state, Event::TaskReceived(task_env)).unwrap();
    assert_eq!(state.phase, Phase::Accepted);
    assert_eq!(state.active_task.as_ref().unwrap().as_str(), "task-01");

    // 3. DispatchIntentRecorded
    let (state, effects) = reduce(
        &state,
        Event::DispatchIntentRecorded {
            lane_id: lane_id.clone(),
            binding_epoch: 1,
            task_id: TaskId::new("task-01").unwrap(),
        },
    )
    .unwrap();
    assert_eq!(state.phase, Phase::Dispatching);
    assert!(matches!(effects[0], SideEffect::DispatchWorkerTask { .. }));

    // 4. WorkerTurnBound
    let turn_id = TurnId::new("turn-001").unwrap();
    let (state, _) = reduce(
        &state,
        Event::WorkerTurnBound {
            lane_id: lane_id.clone(),
            binding_epoch: 1,
            task_id: TaskId::new("task-01").unwrap(),
            turn_id: turn_id.clone(),
        },
    )
    .unwrap();
    assert_eq!(state.phase, Phase::Running);
    assert_eq!(state.worker_turn.as_ref().unwrap(), &turn_id);

    // 5. WorkerTurnCompleted (ReportReady)
    let report_payload = r#"{"status": "COMPLETED", "summary": "done"}"#;
    let report_env = make_test_envelope(
        "tuff-relay",
        1,
        "task-01",
        MessageKind::Report,
        report_payload,
    );
    let (state, _) = reduce(
        &state,
        Event::WorkerTurnCompleted {
            lane_id: lane_id.clone(),
            binding_epoch: 1,
            turn_id: turn_id.clone(),
            report_envelope: report_env.clone(),
        },
    )
    .unwrap();
    assert_eq!(state.phase, Phase::ReportReady);
    assert_eq!(
        state.pending_report.as_ref().unwrap(),
        &report_env.message_id
    );

    // 6. SupervisorAckReceived
    let (state, _) = reduce(
        &state,
        Event::SupervisorAckReceived {
            lane_id: lane_id.clone(),
            binding_epoch: 1,
            report_id: report_env.message_id.clone(),
        },
    )
    .unwrap();
    assert_eq!(state.phase, Phase::WaitTask);
    assert_eq!(state.active_task, None);
    assert_eq!(
        state.last_acked_report.as_ref().unwrap(),
        &report_env.message_id
    );
}

#[test]
fn test_duplicate_task_idempotent_and_conflict() {
    let lane_id = LaneId::new("tuff-relay").unwrap();
    let mut state = LaneState::new(lane_id.clone(), 1);
    state.phase = Phase::Accepted;
    state.active_task = Some(TaskId::new("task-01").unwrap());
    let payload = r#"{"action": "test"}"#;
    let env = make_test_envelope("tuff-relay", 1, "task-01", MessageKind::Task, payload);
    state.active_task_hash = Some(env.payload_sha256);

    // Same ID, same payload -> DuplicateTaskIgnored
    let res = reduce(&state, Event::TaskReceived(env));
    assert_eq!(res, Err(ReduceError::DuplicateTaskIgnored));

    // Same ID, different payload -> TaskConflictSameIdDifferentPayload
    let env_diff = make_test_envelope(
        "tuff-relay",
        1,
        "task-01",
        MessageKind::Task,
        r#"{"action": "different"}"#,
    );
    let res2 = reduce(&state, Event::TaskReceived(env_diff));
    assert_eq!(res2, Err(ReduceError::TaskConflictSameIdDifferentPayload));
}

#[test]
fn test_lane_and_epoch_mismatch_rejection() {
    let lane_id = LaneId::new("tuff-relay").unwrap();
    let state = LaneState::new(lane_id.clone(), 1);

    // Wrong lane
    let wrong_lane = LaneId::new("wrong-lane").unwrap();
    let res = reduce(
        &state,
        Event::StartRun {
            lane_id: wrong_lane,
            binding_epoch: 1,
            run_id: RunId::new("run-001").unwrap(),
        },
    );
    assert_eq!(res, Err(ReduceError::LaneMismatch));

    // Wrong epoch
    let res_epoch = reduce(
        &state,
        Event::StartRun {
            lane_id: lane_id.clone(),
            binding_epoch: 2,
            run_id: RunId::new("run-001").unwrap(),
        },
    );
    assert_eq!(res_epoch, Err(ReduceError::EpochMismatch));
}

#[test]
fn test_dispatch_uncertain_leads_to_reconcile() {
    let lane_id = LaneId::new("tuff-relay").unwrap();
    let mut state = LaneState::new(lane_id.clone(), 1);
    state.phase = Phase::Dispatching;
    state.active_task = Some(TaskId::new("task-01").unwrap());

    let (state, effects) = reduce(
        &state,
        Event::DispatchUncertain {
            lane_id: lane_id.clone(),
            binding_epoch: 1,
            task_id: TaskId::new("task-01").unwrap(),
            reason: "connection dropped".to_string(),
        },
    )
    .unwrap();

    assert_eq!(state.phase, Phase::Reconcile);
    assert!(effects
        .iter()
        .any(|e| matches!(e, SideEffect::NotifyOperator { .. })));
}

#[test]
fn test_decision_required_flow() {
    let lane_id = LaneId::new("tuff-relay").unwrap();
    let mut state = LaneState::new(lane_id.clone(), 1);
    state.phase = Phase::Running;
    state.active_task = Some(TaskId::new("task-01").unwrap());
    let turn_id = TurnId::new("turn-001").unwrap();
    state.worker_turn = Some(turn_id.clone());

    let dec_payload = r#"{"status": "DECISION_REQUIRED", "summary": "need guidance"}"#;
    let dec_env = make_test_envelope(
        "tuff-relay",
        1,
        "task-01",
        MessageKind::DecisionRequired,
        dec_payload,
    );

    let (state, effects) = reduce(
        &state,
        Event::WorkerTurnCompleted {
            lane_id: lane_id.clone(),
            binding_epoch: 1,
            turn_id: turn_id.clone(),
            report_envelope: dec_env.clone(),
        },
    )
    .unwrap();

    assert_eq!(state.phase, Phase::ReportReady);
    assert!(effects.iter().any(|e| {
        if let SideEffect::PersistEvent { event_name, .. } = e {
            event_name == "DecisionReportStored"
        } else {
            false
        }
    }));
}

#[test]
fn test_pause_and_resume_semantics() {
    let lane_id = LaneId::new("tuff-relay").unwrap();
    let mut state = LaneState::new(lane_id.clone(), 1);
    state.phase = Phase::Running;
    state.enabled = true;

    // Pause while running: phase stays Running, pause_requested becomes true
    let (state, _) = reduce(
        &state,
        Event::PauseRequested {
            lane_id: lane_id.clone(),
        },
    )
    .unwrap();
    assert_eq!(state.phase, Phase::Running);
    assert!(state.pause_requested);

    // Resume: pause_requested becomes false
    let (state, _) = reduce(
        &state,
        Event::ResumeRequested {
            lane_id: lane_id.clone(),
        },
    )
    .unwrap();
    assert_eq!(state.phase, Phase::Running);
    assert!(!state.pause_requested);
}

#[test]
fn test_determinism_property() {
    let lane_id = LaneId::new("tuff-relay").unwrap();
    let initial_state = LaneState::new(lane_id.clone(), 1);

    let event = Event::StartRun {
        lane_id: lane_id.clone(),
        binding_epoch: 1,
        run_id: RunId::new("run-001").unwrap(),
    };

    let (state1, effects1) = reduce(&initial_state, event.clone()).unwrap();
    let (state2, effects2) = reduce(&initial_state, event).unwrap();

    assert_eq!(state1, state2);
    assert_eq!(effects1, effects2);
}
