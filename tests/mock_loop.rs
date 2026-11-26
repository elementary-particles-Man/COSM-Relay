use std::fs;
use tuff_relay::core::*;
use tuff_relay::runtime::{LaneConfig, RelayRuntime};

#[allow(clippy::too_many_arguments)]
fn make_envelope(
    lane_id: LaneId,
    epoch: u64,
    run_id: RunId,
    msg_id: &str,
    task_id: &str,
    in_reply_to: Option<MessageId>,
    kind: MessageKind,
    payload: &str,
) -> Envelope {
    Envelope::new(
        lane_id,
        epoch,
        run_id,
        MessageId::new(msg_id).unwrap(),
        TaskId::new(task_id).unwrap(),
        in_reply_to,
        kind,
        payload,
    )
}

#[test]
fn test_mock_loop_three_full_rounds() {
    let temp_dir = std::env::temp_dir().join(format!("tuff-mockloop-test-{}", std::process::id()));
    let _ = fs::remove_dir_all(&temp_dir);
    fs::create_dir_all(&temp_dir).unwrap();

    let lane_id = LaneId::new("lane-mock-1").unwrap();
    let run_id = RunId::new("run-1").unwrap();
    let epoch = 1;

    let mut runtime = RelayRuntime::new(&temp_dir);
    runtime
        .register_lane(LaneConfig {
            id: lane_id.clone(),
            repo_root: temp_dir.clone(),
            binding_epoch: epoch,
            worker_thread_id: None,
        })
        .unwrap();

    // StartRun
    let (state, _) = runtime
        .handle_event(Event::StartRun {
            lane_id: lane_id.clone(),
            binding_epoch: epoch,
            run_id: run_id.clone(),
        })
        .unwrap();
    assert_eq!(state.phase, Phase::WaitTask);

    // 3 complete rounds
    for i in 1..=3 {
        let task_id_str = format!("task-{}", i);
        let turn_id_str = format!("turn-{}", i);
        let msg_task_str = format!("msg-task-{}", i);
        let msg_rep_str = format!("msg-rep-{}", i);
        let task_id = TaskId::new(&task_id_str).unwrap();
        let turn_id = TurnId::new(&turn_id_str).unwrap();
        let report_id = MessageId::new(&msg_rep_str).unwrap();

        // 1. TaskReceived
        let env_task = make_envelope(
            lane_id.clone(),
            epoch,
            run_id.clone(),
            &msg_task_str,
            &task_id_str,
            None,
            MessageKind::Task,
            &format!(r#"{{"step": {}}}"#, i),
        );
        let (state, effects) = runtime
            .handle_event(Event::TaskReceived(env_task.clone()))
            .unwrap();
        assert_eq!(state.phase, Phase::Accepted);
        assert!(effects
            .iter()
            .any(|e| matches!(e, SideEffect::PersistEvent(_))));

        // 2. DispatchIntentRecorded
        let (state, effects) = runtime
            .handle_event(Event::DispatchIntentRecorded {
                lane_id: lane_id.clone(),
                binding_epoch: epoch,
                run_id: run_id.clone(),
                task_id: task_id.clone(),
            })
            .unwrap();
        assert_eq!(state.phase, Phase::Dispatching);
        assert!(effects
            .iter()
            .any(|e| matches!(e, SideEffect::DispatchWorkerTask { .. })));

        // 3. WorkerTurnBound
        let (state, _) = runtime
            .handle_event(Event::WorkerTurnBound {
                lane_id: lane_id.clone(),
                binding_epoch: epoch,
                run_id: run_id.clone(),
                task_id: task_id.clone(),
                turn_id: turn_id.clone(),
            })
            .unwrap();
        assert_eq!(state.phase, Phase::Running);

        // 4. WorkerTurnCompleted (Report)
        let rep_env = make_envelope(
            lane_id.clone(),
            epoch,
            run_id.clone(),
            &msg_rep_str,
            &task_id_str,
            Some(env_task.message_id.clone()),
            MessageKind::Report,
            &format!(r#"{{"result": "ok", "step": {}}}"#, i),
        );
        let (state, effects) = runtime
            .handle_event(Event::WorkerTurnCompleted {
                lane_id: lane_id.clone(),
                binding_epoch: epoch,
                run_id: run_id.clone(),
                turn_id: turn_id.clone(),
                report_envelope: rep_env,
            })
            .unwrap();
        assert_eq!(state.phase, Phase::ReportReady);
        assert!(effects
            .iter()
            .any(|e| matches!(e, SideEffect::NotifySupervisorReport { .. })));

        // 5. SupervisorAckReceived
        let (state, _) = runtime
            .handle_event(Event::SupervisorAckReceived {
                lane_id: lane_id.clone(),
                binding_epoch: epoch,
                run_id: run_id.clone(),
                report_id: report_id.clone(),
            })
            .unwrap();
        assert_eq!(state.phase, Phase::WaitTask);
        assert_eq!(state.active_task, None);
    }

    // Drop runtime to release flock before opening second journal
    let current_phase = runtime.get_state(&lane_id).unwrap().phase;
    let current_epoch = runtime.get_state(&lane_id).unwrap().binding_epoch;
    drop(runtime);

    // Verify journal and replay
    let journal = FileJournal::open(&temp_dir, lane_id.clone(), epoch).unwrap();
    let replayed = journal.replay_state().unwrap();
    assert_eq!(replayed.phase, current_phase);
    assert_eq!(replayed.binding_epoch, current_epoch);
    assert_eq!(replayed.task_history.len(), 3);

    let _ = fs::remove_dir_all(&temp_dir);
}

#[test]
fn test_mock_loop_decision_required_flow() {
    let temp_dir = std::env::temp_dir().join(format!("tuff-mockdec-test-{}", std::process::id()));
    let _ = fs::remove_dir_all(&temp_dir);
    fs::create_dir_all(&temp_dir).unwrap();

    let lane_id = LaneId::new("lane-decision-1").unwrap();
    let run_id = RunId::new("run-dec-1").unwrap();
    let epoch = 1;

    let mut runtime = RelayRuntime::new(&temp_dir);
    runtime
        .register_lane(LaneConfig {
            id: lane_id.clone(),
            repo_root: temp_dir.clone(),
            binding_epoch: epoch,
            worker_thread_id: None,
        })
        .unwrap();

    runtime
        .handle_event(Event::StartRun {
            lane_id: lane_id.clone(),
            binding_epoch: epoch,
            run_id: run_id.clone(),
        })
        .unwrap();

    let task_id = TaskId::new("task-dec-01").unwrap();
    let turn_id = TurnId::new("turn-dec-01").unwrap();
    let env_task = make_envelope(
        lane_id.clone(),
        epoch,
        run_id.clone(),
        "msg-dec-1",
        "task-dec-01",
        None,
        MessageKind::Task,
        r#"{"question": "Ambiguity encountered"}"#,
    );
    runtime
        .handle_event(Event::TaskReceived(env_task.clone()))
        .unwrap();
    runtime
        .handle_event(Event::DispatchIntentRecorded {
            lane_id: lane_id.clone(),
            binding_epoch: epoch,
            run_id: run_id.clone(),
            task_id: task_id.clone(),
        })
        .unwrap();
    runtime
        .handle_event(Event::WorkerTurnBound {
            lane_id: lane_id.clone(),
            binding_epoch: epoch,
            run_id: run_id.clone(),
            task_id: task_id.clone(),
            turn_id: turn_id.clone(),
        })
        .unwrap();

    // Worker returns DecisionRequired report
    let rep_dec = make_envelope(
        lane_id.clone(),
        epoch,
        run_id.clone(),
        "msg-rep-dec",
        "task-dec-01",
        Some(env_task.message_id.clone()),
        MessageKind::DecisionRequired,
        r#"{"decision_prompt": "Choose A or B"}"#,
    );
    let (state, _) = runtime
        .handle_event(Event::WorkerTurnCompleted {
            lane_id: lane_id.clone(),
            binding_epoch: epoch,
            run_id: run_id.clone(),
            turn_id: turn_id.clone(),
            report_envelope: rep_dec.clone(),
        })
        .unwrap();
    assert_eq!(state.phase, Phase::ReportReady);
    assert!(state.last_report_was_decision_required);

    // Supervisor ACKs the report -> transitions to WaitDecision
    let (state, _) = runtime
        .handle_event(Event::SupervisorAckReceived {
            lane_id: lane_id.clone(),
            binding_epoch: epoch,
            run_id: run_id.clone(),
            report_id: rep_dec.message_id.clone(),
        })
        .unwrap();
    assert_eq!(state.phase, Phase::WaitDecision);

    // Supervisor sends continuation Task with in_reply_to = rep_dec.message_id
    let env_decision = make_envelope(
        lane_id.clone(),
        epoch,
        run_id.clone(),
        "msg-dec-resp",
        "task-dec-cont",
        Some(rep_dec.message_id.clone()),
        MessageKind::Task,
        r#"{"decision": "Proceed with A"}"#,
    );
    let (state, _) = runtime
        .handle_event(Event::DecisionResponseAccepted {
            lane_id: lane_id.clone(),
            binding_epoch: epoch,
            run_id: run_id.clone(),
            continuation_task: env_decision,
        })
        .unwrap();
    assert_eq!(state.phase, Phase::Accepted);

    // Drop runtime to release flock
    drop(runtime);

    // Verify journal replay
    let journal = FileJournal::open(&temp_dir, lane_id.clone(), epoch).unwrap();
    let replayed = journal.replay_state().unwrap();
    assert_eq!(replayed.phase, Phase::Accepted);

    let _ = fs::remove_dir_all(&temp_dir);
}

#[test]
fn test_mock_loop_fault_injection_and_reconciliation() {
    let temp_dir = std::env::temp_dir().join(format!("tuff-mockfault-test-{}", std::process::id()));
    let _ = fs::remove_dir_all(&temp_dir);
    fs::create_dir_all(&temp_dir).unwrap();

    let lane_id = LaneId::new("lane-fault-1").unwrap();
    let run_id = RunId::new("run-fault-1").unwrap();
    let epoch = 1;

    let mut runtime = RelayRuntime::new(&temp_dir);
    runtime
        .register_lane(LaneConfig {
            id: lane_id.clone(),
            repo_root: temp_dir.clone(),
            binding_epoch: epoch,
            worker_thread_id: None,
        })
        .unwrap();

    runtime
        .handle_event(Event::StartRun {
            lane_id: lane_id.clone(),
            binding_epoch: epoch,
            run_id: run_id.clone(),
        })
        .unwrap();

    let task_id = TaskId::new("task-fault-01").unwrap();
    let env_task = make_envelope(
        lane_id.clone(),
        epoch,
        run_id.clone(),
        "msg-fault-1",
        "task-fault-01",
        None,
        MessageKind::Task,
        r#"{"action": "test"}"#,
    );
    runtime.handle_event(Event::TaskReceived(env_task)).unwrap();

    // Fault: DispatchUncertain
    let (state, _) = runtime
        .handle_event(Event::DispatchUncertain {
            lane_id: lane_id.clone(),
            binding_epoch: epoch,
            run_id: run_id.clone(),
            task_id: task_id.clone(),
            reason: "Network timeout".to_string(),
        })
        .unwrap();
    assert_eq!(state.phase, Phase::Reconcile);

    // Operator reconciles: TaskNotStarted returns phase to Accepted
    let (state, _) = runtime
        .handle_event(Event::Reconciled {
            lane_id: lane_id.clone(),
            binding_epoch: epoch,
            run_id: run_id.clone(),
            outcome: ReconcileOutcome::TaskNotStarted,
        })
        .unwrap();
    assert_eq!(state.phase, Phase::Accepted);

    // Drop runtime to release flock
    drop(runtime);

    // Verify replay state
    let journal = FileJournal::open(&temp_dir, lane_id.clone(), epoch).unwrap();
    let replayed = journal.replay_state().unwrap();
    assert_eq!(replayed.phase, Phase::Accepted);

    let _ = fs::remove_dir_all(&temp_dir);
}
