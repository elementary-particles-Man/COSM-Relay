use tuff_relay::core::*;

#[allow(clippy::too_many_arguments)]
fn make_test_envelope(
    lane_id: &str,
    epoch: u64,
    run_id: &str,
    task_id: &str,
    message_id: &str,
    in_reply_to: Option<MessageId>,
    kind: MessageKind,
    payload: &str,
) -> Envelope {
    Envelope::new(
        LaneId::new(lane_id).unwrap(),
        epoch,
        RunId::new(run_id).unwrap(),
        MessageId::new(message_id).unwrap(),
        TaskId::new(task_id).unwrap(),
        in_reply_to,
        kind,
        payload.to_string(),
    )
}

#[test]
fn test_envelope_creation_and_integrity() {
    let payload = r#"{"task": "日本語、Unicode (🚀)、改行\nを含むタスク"}"#;
    let env = make_test_envelope(
        "tuff-relay",
        1,
        "run-001",
        "task-01",
        "msg-01",
        None,
        MessageKind::Task,
        payload,
    );
    assert_eq!(env.protocol_version, PROTOCOL_VERSION);
    assert!(env.verify_integrity().is_ok());

    let mut corrupted = env.clone();
    corrupted.payload_utf8 = r#"{"task": "改ざんされたペイロード"}"#.into();
    assert!(corrupted.verify_integrity().is_err());
}

#[test]
fn test_json_roundtrip_sha256_preservation() {
    let payload = r#"{"complex": {"nested": [1, 2, 3], "japanese": "テストデータ", "escapes": "\"\\/\b\f\n\r\t"}}"#;
    let env = make_test_envelope(
        "tuff-relay",
        1,
        "run-001",
        "task-01",
        "msg-01",
        None,
        MessageKind::Task,
        payload,
    );
    let original_hash = env.payload_sha256;

    let serialized = serde_json::to_string(&env).unwrap();
    let deserialized: Envelope = serde_json::from_str(&serialized).unwrap();

    assert_eq!(deserialized.payload_sha256, original_hash);
    assert_eq!(&*deserialized.payload_utf8, payload);
    assert!(deserialized.verify_integrity().is_ok());
}

#[test]
fn test_normal_cycle_transitions_and_persistence_contract() {
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
    assert_eq!(state.journal_seq, 1);
    assert_eq!(effects.len(), 1);
    assert!(matches!(effects[0], SideEffect::PersistEvent(..)));

    // 2. TaskReceived
    let task_payload = r#"{"action": "implement"}"#;
    let task_env = make_test_envelope(
        "tuff-relay",
        1,
        "run-001",
        "task-01",
        "msg-01",
        None,
        MessageKind::Task,
        task_payload,
    );
    let (state, effects) = reduce(&state, Event::TaskReceived(task_env)).unwrap();
    assert_eq!(state.phase, Phase::Accepted);
    assert_eq!(state.active_task.as_ref().unwrap().as_str(), "task-01");
    assert_eq!(state.journal_seq, 2);
    // TaskAccepted only (DispatchIntent not emitted before explicit event)
    assert_eq!(effects.len(), 1);
    assert!(matches!(effects[0], SideEffect::PersistEvent(..)));

    // 3. DispatchIntentRecorded -> SideEffect contains actual task payload
    let (state, effects) = reduce(
        &state,
        Event::DispatchIntentRecorded {
            lane_id: lane_id.clone(),
            binding_epoch: 1,
            run_id: RunId::new("run-001").unwrap(),
            task_id: TaskId::new("task-01").unwrap(),
        },
    )
    .unwrap();
    assert_eq!(state.phase, Phase::Dispatching);
    assert_eq!(state.journal_seq, 3);
    assert_eq!(effects.len(), 2);
    assert!(matches!(effects[0], SideEffect::PersistEvent(..)));
    if let SideEffect::DispatchWorkerTask {
        task_id,
        payload_utf8,
        payload_sha256,
    } = &effects[1]
    {
        assert_eq!(task_id.as_str(), "task-01");
        assert_eq!(&**payload_utf8, task_payload);
        assert_eq!(payload_sha256, &Envelope::compute_sha256(task_payload));
    } else {
        panic!("Expected DispatchWorkerTask effect");
    }

    // 4. WorkerTurnBound
    let turn_id = TurnId::new("turn-001").unwrap();
    let (state, _) = reduce(
        &state,
        Event::WorkerTurnBound {
            lane_id: lane_id.clone(),
            binding_epoch: 1,
            run_id: RunId::new("run-001").unwrap(),
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
        "run-001",
        "task-01",
        "msg-report-01",
        None,
        MessageKind::Report,
        report_payload,
    );
    let (state, effects) = reduce(
        &state,
        Event::WorkerTurnCompleted {
            lane_id: lane_id.clone(),
            binding_epoch: 1,
            run_id: RunId::new("run-001").unwrap(),
            turn_id: turn_id.clone(),
            report_envelope: report_env.clone(),
        },
    )
    .unwrap();
    assert_eq!(state.phase, Phase::ReportReady);
    assert_eq!(
        state.pending_report.as_ref().unwrap().message_id,
        report_env.message_id
    );
    assert_eq!(effects.len(), 2);
    assert!(matches!(effects[0], SideEffect::PersistEvent(..)));
    assert!(matches!(
        effects[1],
        SideEffect::NotifySupervisorReport { .. }
    ));

    // 6. SupervisorAckReceived
    let (state, _) = reduce(
        &state,
        Event::SupervisorAckReceived {
            lane_id: lane_id.clone(),
            binding_epoch: 1,
            run_id: RunId::new("run-001").unwrap(),
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
    // Task history now records completed task
    assert!(state
        .task_history
        .contains_key(&TaskId::new("task-01").unwrap()));
}

#[test]
fn test_c01_decision_required_to_wait_decision_flow() {
    let lane_id = LaneId::new("tuff-relay").unwrap();
    let mut state = LaneState::new(lane_id.clone(), 1);
    state.phase = Phase::Running;
    state.enabled = true;
    state.run_id = Some(RunId::new("run-001").unwrap());
    state.active_task = Some(TaskId::new("task-01").unwrap());
    let turn_id = TurnId::new("turn-001").unwrap();
    state.worker_turn = Some(turn_id.clone());

    let dec_payload = r#"{"status": "DECISION_REQUIRED", "summary": "need guidance"}"#;
    let dec_env = make_test_envelope(
        "tuff-relay",
        1,
        "run-001",
        "task-01",
        "msg-dec-01",
        None,
        MessageKind::DecisionRequired,
        dec_payload,
    );

    // 1. WorkerTurnCompleted with DecisionRequired -> ReportReady
    let (state, _) = reduce(
        &state,
        Event::WorkerTurnCompleted {
            lane_id: lane_id.clone(),
            binding_epoch: 1,
            run_id: RunId::new("run-001").unwrap(),
            turn_id: turn_id.clone(),
            report_envelope: dec_env.clone(),
        },
    )
    .unwrap();
    assert_eq!(state.phase, Phase::ReportReady);
    assert!(state.last_report_was_decision_required);

    // 2. SupervisorAckReceived -> must transition to WaitDecision (not WaitTask!)
    let (state, _) = reduce(
        &state,
        Event::SupervisorAckReceived {
            lane_id: lane_id.clone(),
            binding_epoch: 1,
            run_id: RunId::new("run-001").unwrap(),
            report_id: dec_env.message_id.clone(),
        },
    )
    .unwrap();
    assert_eq!(state.phase, Phase::WaitDecision);

    // 3. Normal TaskReceived must be rejected in WaitDecision
    let normal_task = make_test_envelope(
        "tuff-relay",
        1,
        "run-001",
        "task-02",
        "msg-02",
        None,
        MessageKind::Task,
        r#"{"action": "normal"}"#,
    );
    let res = reduce(&state, Event::TaskReceived(normal_task));
    assert!(matches!(
        res,
        Err(ReduceError::InvalidPhaseTransition { .. })
    ));

    // 4. DecisionResponseAccepted with wrong in_reply_to must be rejected
    let bad_reply_task = make_test_envelope(
        "tuff-relay",
        1,
        "run-001",
        "task-02",
        "msg-02",
        Some(MessageId::new("wrong-reply-id").unwrap()),
        MessageKind::Task,
        r#"{"action": "continuation"}"#,
    );
    let res_reply = reduce(
        &state,
        Event::DecisionResponseAccepted {
            lane_id: lane_id.clone(),
            binding_epoch: 1,
            run_id: RunId::new("run-001").unwrap(),
            continuation_task: bad_reply_task,
        },
    );
    assert_eq!(res_reply, Err(ReduceError::InReplyToMismatch));

    // 5. DecisionResponseAccepted with valid in_reply_to -> Accepted
    let valid_continuation = make_test_envelope(
        "tuff-relay",
        1,
        "run-001",
        "task-02",
        "msg-02",
        Some(dec_env.message_id.clone()),
        MessageKind::Task,
        r#"{"action": "continuation"}"#,
    );
    let (state, _) = reduce(
        &state,
        Event::DecisionResponseAccepted {
            lane_id: lane_id.clone(),
            binding_epoch: 1,
            run_id: RunId::new("run-001").unwrap(),
            continuation_task: valid_continuation,
        },
    )
    .unwrap();
    assert_eq!(state.phase, Phase::Accepted);
    assert_eq!(state.active_task.as_ref().unwrap().as_str(), "task-02");
}

#[test]
fn test_c02_completed_task_history_and_duplicate_rejection_100x() {
    let lane_id = LaneId::new("tuff-relay").unwrap();
    let mut state = LaneState::new(lane_id.clone(), 1);
    state.phase = Phase::WaitTask;
    state.enabled = true;
    state.run_id = Some(RunId::new("run-001").unwrap());

    let payload = r#"{"action": "test"}"#;
    let env = make_test_envelope(
        "tuff-relay",
        1,
        "run-001",
        "task-01",
        "msg-01",
        None,
        MessageKind::Task,
        payload,
    );

    // Record into task_history as already completed
    state
        .task_history
        .insert(TaskId::new("task-01").unwrap(), env.payload_sha256);

    // 100 resubmissions of identical payload -> must yield 0 side effects and DuplicateTaskIgnored
    for _ in 0..100 {
        let res = reduce(&state, Event::TaskReceived(env.clone()));
        assert_eq!(res, Err(ReduceError::DuplicateTaskIgnored));
    }

    // Submission with same ID but different payload -> must be rejected as Conflict
    let diff_env = make_test_envelope(
        "tuff-relay",
        1,
        "run-001",
        "task-01",
        "msg-diff",
        None,
        MessageKind::Task,
        r#"{"action": "different"}"#,
    );
    let res_diff = reduce(&state, Event::TaskReceived(diff_env));
    assert_eq!(
        res_diff,
        Err(ReduceError::TaskConflictSameIdDifferentPayload)
    );

    // Duplicate ACK idempotency
    state.phase = Phase::WaitTask;
    state.last_acked_report = Some(MessageId::new("rep-01").unwrap());
    let res_ack = reduce(
        &state,
        Event::SupervisorAckReceived {
            lane_id: lane_id.clone(),
            binding_epoch: 1,
            run_id: RunId::new("run-001").unwrap(),
            report_id: MessageId::new("rep-01").unwrap(),
        },
    );
    assert_eq!(res_ack, Err(ReduceError::DuplicateAckIgnored));
}

#[test]
fn test_c03_envelope_and_event_mismatch_rejections() {
    let lane_id = LaneId::new("tuff-relay").unwrap();
    let mut state = LaneState::new(lane_id.clone(), 1);
    state.phase = Phase::WaitTask;
    state.enabled = true;
    state.run_id = Some(RunId::new("run-001").unwrap());

    // Run mismatch in task envelope
    let env_bad_run = make_test_envelope(
        "tuff-relay",
        1,
        "run-999",
        "task-01",
        "msg-01",
        None,
        MessageKind::Task,
        r#"{"a": 1}"#,
    );
    assert_eq!(
        reduce(&state, Event::TaskReceived(env_bad_run)),
        Err(ReduceError::RunMismatch)
    );

    // Epoch mismatch
    let env_bad_epoch = make_test_envelope(
        "tuff-relay",
        2,
        "run-001",
        "task-01",
        "msg-01",
        None,
        MessageKind::Task,
        r#"{"a": 1}"#,
    );
    assert_eq!(
        reduce(&state, Event::TaskReceived(env_bad_epoch)),
        Err(ReduceError::EpochMismatch)
    );

    // Lane mismatch
    let env_bad_lane = make_test_envelope(
        "other-lane",
        1,
        "run-001",
        "task-01",
        "msg-01",
        None,
        MessageKind::Task,
        r#"{"a": 1}"#,
    );
    assert_eq!(
        reduce(&state, Event::TaskReceived(env_bad_lane)),
        Err(ReduceError::LaneMismatch)
    );

    // MessageKind mismatch (sending Report as TaskReceived)
    let env_bad_kind = make_test_envelope(
        "tuff-relay",
        1,
        "run-001",
        "task-01",
        "msg-01",
        None,
        MessageKind::Report,
        r#"{"a": 1}"#,
    );
    assert_eq!(
        reduce(&state, Event::TaskReceived(env_bad_kind)),
        Err(ReduceError::MessageKindMismatch)
    );
}

#[test]
fn test_c04_dispatch_uncertain_task_validation() {
    let lane_id = LaneId::new("tuff-relay").unwrap();
    let mut state = LaneState::new(lane_id.clone(), 1);
    state.phase = Phase::Dispatching;
    state.enabled = true;
    state.run_id = Some(RunId::new("run-001").unwrap());
    state.active_task = Some(TaskId::new("task-01").unwrap());

    // Wrong task_id in DispatchUncertain
    let res_bad_task = reduce(
        &state,
        Event::DispatchUncertain {
            lane_id: lane_id.clone(),
            binding_epoch: 1,
            run_id: RunId::new("run-001").unwrap(),
            task_id: TaskId::new("wrong-task").unwrap(),
            reason: "timeout".to_string(),
        },
    );
    assert_eq!(res_bad_task, Err(ReduceError::TaskMismatch));

    // Matching task_id -> transitions to Reconcile, preserves active_task
    let (next, effects) = reduce(
        &state,
        Event::DispatchUncertain {
            lane_id: lane_id.clone(),
            binding_epoch: 1,
            run_id: RunId::new("run-001").unwrap(),
            task_id: TaskId::new("task-01").unwrap(),
            reason: "timeout".to_string(),
        },
    )
    .unwrap();
    assert_eq!(next.phase, Phase::Reconcile);
    assert_eq!(next.active_task.as_ref().unwrap().as_str(), "task-01");
    assert!(effects
        .iter()
        .any(|e| matches!(e, SideEffect::NotifyOperator { .. })));
}

#[test]
fn test_c05_worker_turn_failed_with_report_envelope() {
    let lane_id = LaneId::new("tuff-relay").unwrap();
    let mut state = LaneState::new(lane_id.clone(), 1);
    state.phase = Phase::Running;
    state.enabled = true;
    state.run_id = Some(RunId::new("run-001").unwrap());
    state.active_task = Some(TaskId::new("task-01").unwrap());
    let turn_id = TurnId::new("turn-001").unwrap();
    state.worker_turn = Some(turn_id.clone());

    let failure_env = make_test_envelope(
        "tuff-relay",
        1,
        "run-001",
        "task-01",
        "msg-fail-01",
        None,
        MessageKind::Report,
        r#"{"status": "FAILED", "error": "compile error"}"#,
    );

    let (state, effects) = reduce(
        &state,
        Event::WorkerTurnFailed {
            lane_id: lane_id.clone(),
            binding_epoch: 1,
            run_id: RunId::new("run-001").unwrap(),
            turn_id: turn_id.clone(),
            failure_report_envelope: failure_env.clone(),
        },
    )
    .unwrap();

    assert_eq!(state.phase, Phase::ReportReady);
    assert_eq!(
        state.pending_report.as_ref().unwrap().message_id,
        failure_env.message_id
    );
    assert!(effects
        .iter()
        .any(|e| matches!(e, SideEffect::NotifySupervisorReport { .. })));

    // Now SupervisorAckReceived can be acknowledged cleanly
    let (state, _) = reduce(
        &state,
        Event::SupervisorAckReceived {
            lane_id: lane_id.clone(),
            binding_epoch: 1,
            run_id: RunId::new("run-001").unwrap(),
            report_id: failure_env.message_id.clone(),
        },
    )
    .unwrap();
    assert_eq!(state.phase, Phase::WaitTask);
}

#[test]
fn test_c06_reconcile_outcomes_and_rejection_of_arbitrary_resume() {
    let lane_id = LaneId::new("tuff-relay").unwrap();
    let mut state = LaneState::new(lane_id.clone(), 1);
    state.phase = Phase::Reconcile;
    state.enabled = true;
    state.run_id = Some(RunId::new("run-001").unwrap());
    state.active_task = Some(TaskId::new("task-01").unwrap());

    // ResumeRequested in Reconcile cannot bypass Reconcile!
    let (resumed_state, _) = reduce(
        &state,
        Event::ResumeRequested {
            lane_id: lane_id.clone(),
        },
    )
    .unwrap();
    assert_eq!(resumed_state.phase, Phase::Reconcile);

    // Reconciled with TaskNotStarted -> Accepted
    let (state_accepted, _) = reduce(
        &state,
        Event::Reconciled {
            lane_id: lane_id.clone(),
            binding_epoch: 1,
            run_id: RunId::new("run-001").unwrap(),
            outcome: ReconcileOutcome::TaskNotStarted,
        },
    )
    .unwrap();
    assert_eq!(state_accepted.phase, Phase::Accepted);

    // Reconciled with TaskRunning -> Running
    let (state_running, _) = reduce(
        &state,
        Event::Reconciled {
            lane_id: lane_id.clone(),
            binding_epoch: 1,
            run_id: RunId::new("run-001").unwrap(),
            outcome: ReconcileOutcome::TaskRunning {
                turn_id: TurnId::new("turn-recon-01").unwrap(),
            },
        },
    )
    .unwrap();
    assert_eq!(state_running.phase, Phase::Running);
    assert_eq!(
        state_running.worker_turn.as_ref().unwrap().as_str(),
        "turn-recon-01"
    );

    // Reconciled with TaskCompleted with invalid envelope (wrong task) -> Rejected
    let bad_recon_report = make_test_envelope(
        "tuff-relay",
        1,
        "run-001",
        "wrong-task",
        "msg-recon-rep",
        None,
        MessageKind::Report,
        r#"{"status": "COMPLETED"}"#,
    );
    let res_bad_recon = reduce(
        &state,
        Event::Reconciled {
            lane_id: lane_id.clone(),
            binding_epoch: 1,
            run_id: RunId::new("run-001").unwrap(),
            outcome: ReconcileOutcome::TaskCompleted {
                report: bad_recon_report,
            },
        },
    );
    assert!(matches!(
        res_bad_recon,
        Err(ReduceError::InvalidReconciliationOutcome { .. })
    ));
}

#[test]
fn test_c07_serde_identifier_validation() {
    // Empty LaneId via serde
    let empty_json = r#""""#;
    assert!(serde_json::from_str::<LaneId>(empty_json).is_err());

    // Invalid characters in LaneId (uppercase, spaces)
    let bad_char_json = r#""Invalid_Lane""#;
    assert!(serde_json::from_str::<LaneId>(bad_char_json).is_err());

    // Length > 64 LaneId
    let long_lane = format!("\"{}\"", "a".repeat(65));
    assert!(serde_json::from_str::<LaneId>(&long_lane).is_err());

    // Valid LaneId
    let valid_lane_json = r#""tuff-relay-01""#;
    assert!(serde_json::from_str::<LaneId>(valid_lane_json).is_ok());

    // Empty TaskId / RunId / MessageId / TurnId
    assert!(serde_json::from_str::<TaskId>(empty_json).is_err());
    assert!(serde_json::from_str::<RunId>(empty_json).is_err());
    assert!(serde_json::from_str::<MessageId>(empty_json).is_err());
    assert!(serde_json::from_str::<TurnId>(empty_json).is_err());
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
