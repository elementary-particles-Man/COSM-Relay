use std::fs;
use tuff_relay::core::*;
use tuff_relay::runtime::{LaneConfig, RelayRuntime};

#[test]
fn test_multi_lane_independent_operation_and_isolation() {
    let temp_dir = std::env::temp_dir().join(format!("tuff-multi-test-{}", std::process::id()));
    let _ = fs::remove_dir_all(&temp_dir);
    fs::create_dir_all(&temp_dir).unwrap();

    let lane1 = LaneId::new("lane-alpha").unwrap();
    let lane2 = LaneId::new("lane-beta").unwrap();
    let epoch = 1;

    let mut runtime = RelayRuntime::new(&temp_dir);
    runtime
        .register_lane(LaneConfig {
            id: lane1.clone(),
            repo_root: temp_dir.join("alpha"),
            binding_epoch: epoch,
            worker_thread_id: None,
        })
        .unwrap();
    runtime
        .register_lane(LaneConfig {
            id: lane2.clone(),
            repo_root: temp_dir.join("beta"),
            binding_epoch: epoch,
            worker_thread_id: None,
        })
        .unwrap();

    // 1. StartRun on both lanes
    runtime
        .handle_event(Event::StartRun {
            lane_id: lane1.clone(),
            binding_epoch: epoch,
            run_id: RunId::new("run-a").unwrap(),
        })
        .unwrap();
    runtime
        .handle_event(Event::StartRun {
            lane_id: lane2.clone(),
            binding_epoch: epoch,
            run_id: RunId::new("run-b").unwrap(),
        })
        .unwrap();

    assert_eq!(runtime.get_state(&lane1).unwrap().phase, Phase::WaitTask);
    assert_eq!(runtime.get_state(&lane2).unwrap().phase, Phase::WaitTask);

    // 2. Submit task to lane1
    let env1 = Envelope::new(
        lane1.clone(),
        epoch,
        RunId::new("run-a").unwrap(),
        MessageId::new("msg-a1").unwrap(),
        TaskId::new("task-a1").unwrap(),
        None,
        MessageKind::Task,
        r#"{"lane":"alpha"}"#,
    );
    runtime.handle_event(Event::TaskReceived(env1)).unwrap();
    assert_eq!(runtime.get_state(&lane1).unwrap().phase, Phase::Accepted);
    assert_eq!(runtime.get_state(&lane2).unwrap().phase, Phase::WaitTask); // lane2 unaffected!

    // 3. Pause lane1, verify lane2 remains unaffected and can accept tasks
    runtime
        .handle_event(Event::PauseRequested {
            lane_id: lane1.clone(),
        })
        .unwrap();
    assert!(runtime.get_state(&lane1).unwrap().pause_requested);
    assert!(!runtime.get_state(&lane2).unwrap().pause_requested);

    let env2 = Envelope::new(
        lane2.clone(),
        epoch,
        RunId::new("run-b").unwrap(),
        MessageId::new("msg-b1").unwrap(),
        TaskId::new("task-b1").unwrap(),
        None,
        MessageKind::Task,
        r#"{"lane":"beta"}"#,
    );
    runtime.handle_event(Event::TaskReceived(env2)).unwrap();
    assert_eq!(runtime.get_state(&lane2).unwrap().phase, Phase::Accepted);

    let _ = fs::remove_dir_all(&temp_dir);
}
