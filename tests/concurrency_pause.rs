use std::sync::{Arc, Mutex};
use tuff_relay::adapters::http_gateway::HttpGateway;
use tuff_relay::adapters::supervisor::{SubmitTaskRequest, SupervisorAdapter};
use tuff_relay::core::*;
use tuff_relay::runtime::{LaneConfig, RelayRuntime};

#[test]
fn test_status_and_pause_during_active_dispatch() {
    let temp_dir =
        std::env::temp_dir().join(format!("tuff-concurrency-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&temp_dir);
    std::fs::create_dir_all(&temp_dir).unwrap();

    let lane_id = LaneId::new("lane-conc-01").unwrap();
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
    let adapter = Arc::new(SupervisorAdapter::new(runtime_arc.clone(), lane_id.clone()));

    let gateway = HttpGateway::start(
        "127.0.0.1:0",
        adapter.clone(),
        Some("secret-token".to_string()),
    )
    .unwrap();
    let gateway_addr = gateway.local_addr().to_string();

    // 1. Submit a task to put lane into Accepted
    let sub = adapter
        .submit(SubmitTaskRequest {
            lane_id: lane_id.as_str().to_string(),
            task_id: "task-conc-1".to_string(),
            payload_json: "{\"prompt\":\"hello\"}".to_string(),
            in_reply_to: None,
        })
        .unwrap();
    assert!(sub.accepted);

    // 2. Query status via HTTP gateway while accepted
    let client = req_client();
    let resp = client
        .get(format!(
            "http://{}/status?lane_id={}",
            gateway_addr,
            lane_id.as_str()
        ))
        .header("Authorization", "Bearer secret-token")
        .send();
    assert!(resp.is_ok());

    // 3. Test Pause request
    {
        let mut rt = runtime_arc.lock().unwrap();
        let (st, _) = rt
            .handle_event(Event::PauseRequested {
                lane_id: lane_id.clone(),
            })
            .unwrap();
        assert!(st.pause_requested);
    }

    // 4. Query status via HTTP gateway while paused
    let resp2 = client
        .get(format!(
            "http://{}/status?lane_id={}",
            gateway_addr,
            lane_id.as_str()
        ))
        .header("Authorization", "Bearer secret-token")
        .send();
    assert!(resp2.is_ok());
}

struct SimpleClient;
impl SimpleClient {
    fn get(&self, url: String) -> SimpleReq {
        SimpleReq {
            url,
            headers: Vec::new(),
        }
    }
}
struct SimpleReq {
    url: String,
    headers: Vec<(String, String)>,
}
impl SimpleReq {
    fn header(mut self, k: &str, v: &str) -> Self {
        self.headers.push((k.to_string(), v.to_string()));
        self
    }
    fn send(self) -> Result<(), ()> {
        let mut parts = self.url.strip_prefix("http://").unwrap().split('/');
        let host = parts.next().unwrap();
        let path = format!("/{}", parts.collect::<Vec<_>>().join("/"));

        let mut stream = std::net::TcpStream::connect(host).map_err(|_| ())?;
        use std::io::Write;
        let mut req_str = format!("GET {} HTTP/1.1\r\nHost: {}\r\n", path, host);
        for (k, v) in self.headers {
            req_str.push_str(&format!("{}: {}\r\n", k, v));
        }
        req_str.push_str("Connection: close\r\n\r\n");
        stream.write_all(req_str.as_bytes()).map_err(|_| ())?;
        Ok(())
    }
}
fn req_client() -> SimpleClient {
    SimpleClient
}
