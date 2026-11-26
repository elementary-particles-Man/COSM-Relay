use std::fs;
use std::sync::{Arc, Mutex};
use tuff_relay::adapters::http_gateway::HttpGateway;
use tuff_relay::adapters::supervisor::SupervisorAdapter;
use tuff_relay::core::*;
use tuff_relay::runtime::{LaneConfig, RelayRuntime};

#[test]
fn test_http_gateway_local_loopback() {
    let temp_dir = std::env::temp_dir().join(format!("tuff-http-test-{}", std::process::id()));
    let _ = fs::remove_dir_all(&temp_dir);
    fs::create_dir_all(&temp_dir).unwrap();

    let lane_id = LaneId::new("lane-http-01").unwrap();
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
    let adapter = Arc::new(SupervisorAdapter::new(runtime_arc.clone(), lane_id.clone()));

    // Start HTTP gateway bound to 127.0.0.1:0 (ephemeral port) with token
    let gateway =
        HttpGateway::start("127.0.0.1:0", adapter, Some("secret123".to_string())).unwrap();
    let addr = gateway.local_addr().to_string();

    // 1. Unauthorized request
    let unauth_resp = make_get(
        &format!("http://{}/status?lane_id=lane-http-01", addr),
        None,
    );
    assert!(unauth_resp.contains("401 Unauthorized"));

    // 2. Authorized status query
    let auth_resp = make_get(
        &format!("http://{}/status?lane_id=lane-http-01", addr),
        Some("secret123"),
    );
    assert!(auth_resp.contains("200 OK"));
    assert!(auth_resp.contains("Idle"));

    let _ = fs::remove_dir_all(&temp_dir);
}

fn make_get(url: &str, token: Option<&str>) -> String {
    use std::io::{Read, Write};
    use std::net::TcpStream;

    let parsed = url.strip_prefix("http://").unwrap();
    let (host_port, path) = parsed.split_once('/').unwrap();
    let path = format!("/{}", path);

    let mut stream = TcpStream::connect(host_port).unwrap();
    let auth_line = if let Some(t) = token {
        format!("Authorization: Bearer {}\r\n", t)
    } else {
        "".to_string()
    };
    let req = format!(
        "GET {} HTTP/1.1\r\nHost: {}\r\n{}Connection: close\r\n\r\n",
        path, host_port, auth_line
    );
    stream.write_all(req.as_bytes()).unwrap();

    let mut resp = String::new();
    stream.read_to_string(&mut resp).unwrap();
    resp
}
