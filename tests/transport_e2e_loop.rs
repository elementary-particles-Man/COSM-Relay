use std::fs;
use std::sync::{Arc, Mutex};
use tuff_relay::adapters::supervisor::{AckReportRequest, SubmitTaskRequest, SupervisorAdapter};
use tuff_relay::core::*;
use tuff_relay::runtime::{LaneConfig, RelayRuntime};

#[test]
fn test_transport_e2e_three_rounds_via_adapter() {
    let temp_dir = std::env::temp_dir().join(format!("tuff-transport-e2e-{}", std::process::id()));
    let _ = fs::remove_dir_all(&temp_dir);
    fs::create_dir_all(&temp_dir).unwrap();

    let lane_id = LaneId::new("lane-transport-01").unwrap();
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
            run_id: RunId::new("run-001").unwrap(),
        })
        .unwrap();

    let runtime_arc = Arc::new(Mutex::new(runtime));
    let adapter = SupervisorAdapter::new(runtime_arc.clone(), lane_id.clone());

    for i in 1..=3 {
        let task_id_str = format!("task-e2e-{}", i);
        let payload_str = format!(r#"{{"round": {}}}"#, i);

        // 1. Submit via adapter
        let submit_res = adapter
            .submit(SubmitTaskRequest {
                lane_id: "lane-transport-01".to_string(),
                task_id: task_id_str.clone(),
                payload_json: payload_str,
                in_reply_to: None,
            })
            .unwrap();
        assert!(submit_res.accepted);
        assert!(!submit_res.message_id.is_empty());

        // 2. Simulate worker completion
        let rep_id_str = format!("rep-e2e-{}", i);
        {
            let mut rt = runtime_arc.lock().unwrap();
            let task_id = TaskId::new(&task_id_str).unwrap();
            let turn_id = TurnId::new(format!("turn-{}", i)).unwrap();

            rt.handle_event(Event::DispatchIntentRecorded {
                lane_id: lane_id.clone(),
                binding_epoch: epoch,
                run_id: RunId::new("run-001").unwrap(),
                task_id: task_id.clone(),
            })
            .unwrap();

            rt.handle_event(Event::WorkerTurnBound {
                lane_id: lane_id.clone(),
                binding_epoch: epoch,
                run_id: RunId::new("run-001").unwrap(),
                task_id: task_id.clone(),
                turn_id: turn_id.clone(),
            })
            .unwrap();

            let rep_payload = format!(r#"{{"result": "round_{}_done"}}"#, i);
            let rep_env = Envelope::new(
                lane_id.clone(),
                epoch,
                RunId::new("run-001").unwrap(),
                MessageId::new(&rep_id_str).unwrap(),
                task_id,
                None,
                MessageKind::Report,
                rep_payload.as_str(),
            );

            rt.handle_event(Event::WorkerTurnCompleted {
                lane_id: lane_id.clone(),
                binding_epoch: epoch,
                run_id: RunId::new("run-001").unwrap(),
                turn_id,
                report_envelope: rep_env,
            })
            .unwrap();
        }

        // 3. Supervisor reads report
        let rep = adapter.get_report("lane-transport-01").unwrap();
        assert!(rep.report_available);
        assert_eq!(rep.report_id, Some(rep_id_str.clone()));

        // 4. Supervisor ACKs report
        let ack_res = adapter
            .ack(AckReportRequest {
                lane_id: "lane-transport-01".to_string(),
                report_id: rep_id_str,
            })
            .unwrap();
        assert!(ack_res.acked);
        assert_eq!(ack_res.new_phase, "WaitTask");
    }

    let _ = fs::remove_dir_all(&temp_dir);
}
