use std::fs;
use std::path::PathBuf;
use tuff_relay::adapters::codex::CodexAppServerClient;
use tuff_relay::core::envelope::{Envelope, MessageKind};
use tuff_relay::core::reducer::Event;
use tuff_relay::core::state::Phase;
use tuff_relay::core::types::{LaneId, MessageId, RunId, TaskId};
use tuff_relay::runtime::{LaneConfig, RelayRuntime};

#[test]
fn test_live_codex_app_server_turn_execution_on_test_thread() {
    let codex_bin = "/home/flux/.local/bin/codex";
    if !std::path::Path::new(codex_bin).exists() {
        eprintln!("Codex binary not found, skipping live test");
        return;
    }

    let temp_dir = std::env::temp_dir().join(format!("tuff-codex-live-{}", std::process::id()));
    let _ = fs::remove_dir_all(&temp_dir);
    fs::create_dir_all(&temp_dir).unwrap();

    let lane_id = LaneId::new("tuff-live-test").unwrap();
    let epoch = 1;
    let run_id = RunId::new("run-live-01").unwrap();

    let mut runtime = RelayRuntime::new(&temp_dir);
    runtime
        .register_lane(LaneConfig {
            id: lane_id.clone(),
            repo_root: PathBuf::from("/mnt/thpdoc/Develop/TUFF-Relay"),
            binding_epoch: epoch,
            worker_thread_id: Some("01a20de6-d7d3-71e2-af86-fff5791dd860".to_string()),
        })
        .unwrap();

    runtime
        .handle_event(Event::StartRun {
            lane_id: lane_id.clone(),
            binding_epoch: epoch,
            run_id: run_id.clone(),
        })
        .unwrap();

    // 1. Spawn codex app-server client
    let mut client = match CodexAppServerClient::spawn(codex_bin) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Failed to spawn codex app-server: {:?}", e);
            return;
        }
    };

    if let Err(e) = client.initialize() {
        eprintln!(
            "Codex initialize skipped (sandbox/read-only filesystem env): {:?}",
            e
        );
        return;
    }

    let thread_id = "01a20de6-d7d3-71e2-af86-fff5791dd860";
    client.thread_resume(thread_id).unwrap();

    // 2. Submit a harmless test task
    let task_id = TaskId::new("LIVE-TURN-TEST-003").unwrap();
    let prompt_payload = r#"{"task_id":"LIVE-TURN-TEST-003","instruction":"Return exact JSON: {\"result\":\"PASS\",\"step\":\"003\"}"}"#;
    let env = Envelope::new(
        lane_id.clone(),
        epoch,
        run_id,
        MessageId::new("msg-live-003").unwrap(),
        task_id,
        None,
        MessageKind::Task,
        prompt_payload,
    );

    let (state, _) = runtime.handle_event(Event::TaskReceived(env)).unwrap();
    assert_eq!(state.phase, Phase::Accepted);

    // 3. Execute dispatch to worker using runtime helper
    let report_text = runtime
        .execute_dispatch_to_worker(&lane_id, &mut client, thread_id)
        .unwrap();

    println!("Report text from live Codex: {}", report_text);
    assert!(!report_text.is_empty());
    assert!(report_text.contains("PASS"));

    let final_state = runtime.get_state(&lane_id).unwrap();
    assert_eq!(final_state.phase, Phase::ReportReady);
    assert!(final_state.pending_report.is_some());

    let _ = fs::remove_dir_all(&temp_dir);
}
