use crate::adapters::supervisor::{AckReportRequest, SubmitTaskRequest, SupervisorAdapter};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};

pub struct HttpGateway {
    addr: String,
    running: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl HttpGateway {
    pub fn start(
        bind_addr: &str,
        adapter: Arc<SupervisorAdapter>,
        auth_token: Option<String>,
    ) -> std::io::Result<Self> {
        let listener = TcpListener::bind(bind_addr)?;
        listener.set_nonblocking(true)?;
        let local_addr = listener.local_addr()?.to_string();

        let running = Arc::new(AtomicBool::new(true));
        let running_clone = running.clone();

        let handle = thread::spawn(move || {
            while running_clone.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        let ad = adapter.clone();
                        let token = auth_token.clone();
                        thread::spawn(move || {
                            let _ = handle_http_connection(stream, ad, token);
                        });
                    }
                    Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(std::time::Duration::from_millis(50));
                    }
                    Err(_) => {
                        break;
                    }
                }
            }
        });

        Ok(Self {
            addr: local_addr,
            running,
            handle: Some(handle),
        })
    }

    pub fn local_addr(&self) -> &str {
        &self.addr
    }

    pub fn stop(&mut self) {
        self.running.store(false, Ordering::Relaxed);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

impl Drop for HttpGateway {
    fn drop(&mut self) {
        self.stop();
    }
}

fn handle_http_connection(
    mut stream: TcpStream,
    adapter: Arc<SupervisorAdapter>,
    expected_token: Option<String>,
) -> std::io::Result<()> {
    let mut reader = BufReader::new(&stream);
    let mut request_line = String::new();
    reader.read_line(&mut request_line)?;
    let parts: Vec<&str> = request_line.split_whitespace().collect();
    if parts.len() < 2 {
        return Ok(());
    }
    let method = parts[0];
    let path = parts[1];

    let mut headers = std::collections::HashMap::new();
    let mut content_length = 0;
    loop {
        let mut line = String::new();
        reader.read_line(&mut line)?;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            break;
        }
        if let Some((k, v)) = trimmed.split_once(':') {
            let k_lower = k.trim().to_lowercase();
            let v_trim = v.trim().to_string();
            if k_lower == "content-length" {
                content_length = v_trim.parse::<usize>().unwrap_or(0);
            }
            headers.insert(k_lower, v_trim);
        }
    }

    // Verify Auth if configured
    if let Some(ref token) = expected_token {
        let auth_hdr = headers.get("authorization").cloned().unwrap_or_default();
        let expected_hdr = format!("Bearer {}", token);
        if auth_hdr != expected_hdr {
            let resp = "HTTP/1.1 401 Unauthorized\r\nContent-Length: 12\r\n\r\nUnauthorized";
            stream.write_all(resp.as_bytes())?;
            return Ok(());
        }
    }

    let mut body = vec![0u8; content_length];
    if content_length > 0 {
        reader.read_exact(&mut body)?;
    }
    let body_str = String::from_utf8_lossy(&body);

    let (status_code, body_resp) = match (method, path) {
        ("GET", p) if p.starts_with("/status") => {
            let lane_id = p
                .split('?')
                .nth(1)
                .and_then(|q| {
                    q.split('&').find_map(|pair| {
                        let mut s = pair.split('=');
                        if s.next() == Some("lane_id") {
                            s.next()
                        } else {
                            None
                        }
                    })
                })
                .unwrap_or("tuff-relay");

            match adapter.status(lane_id) {
                Ok(st) => ("200 OK", serde_json::to_string(&st).unwrap_or_default()),
                Err(e) => (
                    "400 Bad Request",
                    serde_json::json!({ "error": e }).to_string(),
                ),
            }
        }
        ("POST", "/submit") => match serde_json::from_str::<SubmitTaskRequest>(&body_str) {
            Ok(req) => match adapter.submit(req) {
                Ok(resp) => ("200 OK", serde_json::to_string(&resp).unwrap_or_default()),
                Err(e) => (
                    "400 Bad Request",
                    serde_json::json!({ "error": e }).to_string(),
                ),
            },
            Err(e) => (
                "400 Bad Request",
                serde_json::json!({ "error": e.to_string() }).to_string(),
            ),
        },
        ("GET", p) if p.starts_with("/report") => {
            let lane_id = p
                .split('?')
                .nth(1)
                .and_then(|q| {
                    q.split('&').find_map(|pair| {
                        let mut s = pair.split('=');
                        if s.next() == Some("lane_id") {
                            s.next()
                        } else {
                            None
                        }
                    })
                })
                .unwrap_or("tuff-relay");

            match adapter.get_report(lane_id) {
                Ok(rep) => ("200 OK", serde_json::to_string(&rep).unwrap_or_default()),
                Err(e) => (
                    "400 Bad Request",
                    serde_json::json!({ "error": e }).to_string(),
                ),
            }
        }
        ("POST", "/ack") => match serde_json::from_str::<AckReportRequest>(&body_str) {
            Ok(req) => match adapter.ack(req) {
                Ok(resp) => ("200 OK", serde_json::to_string(&resp).unwrap_or_default()),
                Err(e) => (
                    "400 Bad Request",
                    serde_json::json!({ "error": e }).to_string(),
                ),
            },
            Err(e) => (
                "400 Bad Request",
                serde_json::json!({ "error": e.to_string() }).to_string(),
            ),
        },
        _ => (
            "404 Not Found",
            serde_json::json!({ "error": "Endpoint not found" }).to_string(),
        ),
    };

    let full_resp = format!(
        "HTTP/1.1 {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        status_code,
        body_resp.len(),
        body_resp
    );
    stream.write_all(full_resp.as_bytes())?;
    stream.flush()?;
    Ok(())
}
