use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fmt;
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Debug)]
pub enum CodexAdapterError {
    Io(std::io::Error),
    Json(serde_json::Error),
    Rpc { code: i64, message: String },
    ProcessTerminated,
    MissingField(String),
}

impl fmt::Display for CodexAdapterError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CodexAdapterError::Io(e) => write!(f, "I/O error: {}", e),
            CodexAdapterError::Json(e) => write!(f, "JSON error: {}", e),
            CodexAdapterError::Rpc { code, message } => {
                write!(f, "RPC error (code {}): {}", code, message)
            }
            CodexAdapterError::ProcessTerminated => write!(f, "Process terminated prematurely"),
            CodexAdapterError::MissingField(field) => {
                write!(f, "Missing expected field in response: {}", field)
            }
        }
    }
}

impl std::error::Error for CodexAdapterError {}

impl From<std::io::Error> for CodexAdapterError {
    fn from(err: std::io::Error) -> Self {
        CodexAdapterError::Io(err)
    }
}

impl From<serde_json::Error> for CodexAdapterError {
    fn from(err: serde_json::Error) -> Self {
        CodexAdapterError::Json(err)
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct JsonRpcRequest<T> {
    jsonrpc: String,
    id: u64,
    method: String,
    params: T,
}

#[derive(Debug, Serialize, Deserialize)]
struct JsonRpcNotification<T> {
    jsonrpc: String,
    method: String,
    params: T,
}

#[derive(Debug, Serialize, Deserialize)]
struct JsonRpcResponse<T> {
    jsonrpc: String,
    id: Option<u64>,
    result: Option<T>,
    error: Option<JsonRpcErrorObj>,
}

#[derive(Debug, Serialize, Deserialize)]
struct JsonRpcErrorObj {
    code: i64,
    message: String,
    data: Option<Value>,
}

pub struct CodexAppServerClient {
    child: Child,
    stdin: ChildStdin,
    reader: BufReader<ChildStdout>,
    next_id: AtomicU64,
}

impl CodexAppServerClient {
    pub fn spawn(executable_path: &str) -> Result<Self, CodexAdapterError> {
        let mut child = Command::new(executable_path)
            .arg("app-server")
            .arg("--stdio")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()?;

        let stdin = child.stdin.take().ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::BrokenPipe, "Failed to capture stdin")
        })?;
        let stdout = child.stdout.take().ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::BrokenPipe, "Failed to capture stdout")
        })?;
        let reader = BufReader::new(stdout);

        Ok(Self {
            child,
            stdin,
            reader,
            next_id: AtomicU64::new(1),
        })
    }

    fn next_request_id(&self) -> u64 {
        self.next_id.fetch_add(1, Ordering::SeqCst)
    }

    pub fn send_request<P: Serialize, R: for<'de> Deserialize<'de>>(
        &mut self,
        method: &str,
        params: P,
    ) -> Result<R, CodexAdapterError> {
        let req_id = self.next_request_id();
        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: req_id,
            method: method.to_string(),
            params,
        };
        let line = serde_json::to_string(&req)?;
        self.stdin.write_all(line.as_bytes())?;
        self.stdin.write_all(b"\n")?;
        self.stdin.flush()?;

        // Wait for response matching req_id
        let mut resp_line = String::new();
        loop {
            resp_line.clear();
            let bytes_read = self.reader.read_line(&mut resp_line)?;
            if bytes_read == 0 {
                return Err(CodexAdapterError::ProcessTerminated);
            }
            let trimmed = resp_line.trim();
            if trimmed.is_empty() {
                continue;
            }

            let val: Value = match serde_json::from_str(trimmed) {
                Ok(v) => v,
                Err(_) => continue, // non-json stdout or log lines ignored
            };

            if let Some(id) = val.get("id").and_then(|v| v.as_u64()) {
                if id == req_id {
                    let resp: JsonRpcResponse<R> = serde_json::from_value(val)?;
                    if let Some(err) = resp.error {
                        return Err(CodexAdapterError::Rpc {
                            code: err.code,
                            message: err.message,
                        });
                    }
                    if let Some(res) = resp.result {
                        return Ok(res);
                    }
                    return Err(CodexAdapterError::MissingField("result".to_string()));
                }
            }
        }
    }

    pub fn send_notification<P: Serialize>(
        &mut self,
        method: &str,
        params: P,
    ) -> Result<(), CodexAdapterError> {
        let notif = JsonRpcNotification {
            jsonrpc: "2.0".to_string(),
            method: method.to_string(),
            params,
        };
        let line = serde_json::to_string(&notif)?;
        self.stdin.write_all(line.as_bytes())?;
        self.stdin.write_all(b"\n")?;
        self.stdin.flush()?;
        Ok(())
    }

    pub fn initialize(&mut self) -> Result<Value, CodexAdapterError> {
        let res: Value = self.send_request(
            "initialize",
            serde_json::json!({
                "clientInfo": {
                    "name": "TUFF-Relay",
                    "version": "0.1.0"
                },
                "capabilities": {}
            }),
        )?;
        self.send_notification("initialized", serde_json::json!({}))?;
        Ok(res)
    }

    pub fn thread_start(&mut self, cwd: &str) -> Result<String, CodexAdapterError> {
        let res: Value = self.send_request(
            "thread/start",
            serde_json::json!({
                "cwd": cwd
            }),
        )?;
        let thread_id = res
            .get("threadId")
            .and_then(|v| v.as_str())
            .ok_or_else(|| CodexAdapterError::MissingField("threadId".to_string()))?;
        Ok(thread_id.to_string())
    }

    pub fn thread_resume(&mut self, thread_id: &str) -> Result<(), CodexAdapterError> {
        let _: Value = self.send_request(
            "thread/resume",
            serde_json::json!({
                "threadId": thread_id
            }),
        )?;
        Ok(())
    }

    pub fn turn_start(
        &mut self,
        thread_id: &str,
        prompt: &str,
    ) -> Result<String, CodexAdapterError> {
        let res: Value = self.send_request(
            "turn/start",
            serde_json::json!({
                "threadId": thread_id,
                "input": [{
                    "type": "text",
                    "text": prompt
                }]
            }),
        )?;
        let turn_id = res
            .get("turnId")
            .and_then(|v| v.as_str())
            .ok_or_else(|| CodexAdapterError::MissingField("turnId".to_string()))?;
        Ok(turn_id.to_string())
    }

    pub fn turn_interrupt(
        &mut self,
        thread_id: &str,
        turn_id: &str,
    ) -> Result<(), CodexAdapterError> {
        let _: Value = self.send_request(
            "turn/interrupt",
            serde_json::json!({
                "threadId": thread_id,
                "turnId": turn_id
            }),
        )?;
        Ok(())
    }

    /// Reads events until `turn/completed` for target `turn_id` is encountered.
    /// Collects final assistant message text from turn events.
    pub fn wait_turn_completion(
        &mut self,
        target_turn_id: &str,
    ) -> Result<String, CodexAdapterError> {
        let mut collected_text = String::new();
        let mut resp_line = String::new();

        loop {
            resp_line.clear();
            let bytes_read = self.reader.read_line(&mut resp_line)?;
            if bytes_read == 0 {
                return Err(CodexAdapterError::ProcessTerminated);
            }
            let trimmed = resp_line.trim();
            if trimmed.is_empty() {
                continue;
            }

            let val: Value = match serde_json::from_str(trimmed) {
                Ok(v) => v,
                Err(_) => continue,
            };

            if let Some(method) = val.get("method").and_then(|v| v.as_str()) {
                if method == "item/added" || method == "item/completed" {
                    if let Some(params) = val.get("params") {
                        if let Some(item) = params.get("item") {
                            if item.get("type").and_then(|v| v.as_str()) == Some("agent_message") {
                                if let Some(text) = item.get("text").and_then(|v| v.as_str()) {
                                    if !text.is_empty() {
                                        collected_text = text.to_string();
                                    }
                                }
                            }
                        }
                    }
                } else if method == "turn/completed" {
                    if let Some(params) = val.get("params") {
                        if let Some(turn_id) = params.get("turnId").and_then(|v| v.as_str()) {
                            if turn_id == target_turn_id {
                                return Ok(collected_text);
                            }
                        }
                    }
                }
            }
        }
    }
}

impl Drop for CodexAppServerClient {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
