use std::fs;
use std::sync::{Arc, Mutex};
use tuff_relay::adapters::supervisor::*;
use tuff_relay::core::*;
use tuff_relay::runtime::{LaneConfig, RelayRuntime};

#[test]
fn test_supervisor_adapter_contract_and_security() {
    let temp_dir = std::env::temp_dir().join(format!("tuff-sup-test-{}", std::process::id()));
    let _ = fs::remove_dir_all(&temp_dir);
    fs::create_dir_all(&temp_dir).unwrap();

    let lane_id = LaneId::new("lane-auth-1").unwrap();
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

    let runtime_arc = Arc::new(Mutex::new(runtime));
    let adapter = SupervisorAdapter::new(runtime_arc.clone(), lane_id.clone());

    // 1. Security scope: Reject unauthorized lane
    let unauth_status = adapter.status("malicious-lane");
    assert!(unauth_status.is_err());

    let unauth_submit = adapter.submit(SubmitTaskRequest {
        lane_id: "other-lane".to_string(),
        task_id: "t1".to_string(),
        payload_json: r#"{"hello":"world"}"#.to_string(),
    });
    assert!(unauth_submit.is_err());

    // 2. Reject invalid JSON payload
    let bad_json_submit = adapter.submit(SubmitTaskRequest {
        lane_id: "lane-auth-1".to_string(),
        task_id: "t1".to_string(),
        payload_json: "not a valid json".to_string(),
    });
    assert!(bad_json_submit.is_err());

    // 3. Status before run
    let st = adapter.status("lane-auth-1").unwrap();
    assert_eq!(st.phase, "Idle");

    // 4. StartRun on runtime
    {
        let mut rt = runtime_arc.lock().unwrap();
        rt.handle_event(Event::StartRun {
            lane_id: lane_id.clone(),
            binding_epoch: epoch,
            run_id: RunId::new("run-001").unwrap(),
        })
        .unwrap();
    }

    let st = adapter.status("lane-auth-1").unwrap();
    assert_eq!(st.phase, "WaitTask");

    // 5. Submit valid task
    let submit_res = adapter
        .submit(SubmitTaskRequest {
            lane_id: "lane-auth-1".to_string(),
            task_id: "task-sup-01".to_string(),
            payload_json: r#"{"cmd":"check"}"#.to_string(),
        })
        .unwrap();
    assert!(submit_res.accepted);

    let st = adapter.status("lane-auth-1").unwrap();
    assert_eq!(st.phase, "Accepted");

    // 6. Simulate worker completion and report
    let rep_msg_id = MessageId::new("rep-sup-01").unwrap();
    {
        let mut rt = runtime_arc.lock().unwrap();
        rt.handle_event(Event::DispatchIntentRecorded {
            lane_id: lane_id.clone(),
            binding_epoch: epoch,
            run_id: RunId::new("run-001").unwrap(),
            task_id: TaskId::new("task-sup-01").unwrap(),
        })
        .unwrap();
        rt.handle_event(Event::WorkerTurnBound {
            lane_id: lane_id.clone(),
            binding_epoch: epoch,
            run_id: RunId::new("run-001").unwrap(),
            task_id: TaskId::new("task-sup-01").unwrap(),
            turn_id: TurnId::new("turn-sup-01").unwrap(),
        })
        .unwrap();

        let env_rep = Envelope::new(
            lane_id.clone(),
            epoch,
            RunId::new("run-001").unwrap(),
            rep_msg_id.clone(),
            TaskId::new("task-sup-01").unwrap(),
            None,
            MessageKind::Report,
            r#"{"output":"success"}"#,
        );
        rt.handle_event(Event::WorkerTurnCompleted {
            lane_id: lane_id.clone(),
            binding_epoch: epoch,
            run_id: RunId::new("run-001").unwrap(),
            turn_id: TurnId::new("turn-sup-01").unwrap(),
            report_envelope: env_rep,
        })
        .unwrap();
    }

    // 7. Supervisor gets report
    let rep_info = adapter.get_report("lane-auth-1").unwrap();
    assert!(rep_info.report_available);
    assert_eq!(rep_info.report_id, Some("rep-sup-01".to_string()));
    assert_eq!(
        rep_info.payload_utf8,
        Some(r#"{"output":"success"}"#.to_string())
    );

    // 8. Supervisor ACKs report
    let ack_res = adapter
        .ack(AckReportRequest {
            lane_id: "lane-auth-1".to_string(),
            report_id: "rep-sup-01".to_string(),
        })
        .unwrap();
    assert!(ack_res.acked);
    assert_eq!(ack_res.new_phase, "WaitTask");

    let _ = fs::remove_dir_all(&temp_dir);
}
