use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum IpcRequest {
    Status { lane_id: Option<String> },
    Pause { lane_id: String },
    Resume { lane_id: String },
    Stop { lane_id: String },
    Interrupt { lane_id: String },
    ExportRaw { lane_id: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IpcResponse {
    pub success: bool,
    pub payload: serde_json::Value,
    pub error: Option<String>,
}

pub struct IpcServer {
    socket_path: PathBuf,
    running: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl IpcServer {
    pub fn start<H>(socket_path: PathBuf, handler: H) -> std::io::Result<Self>
    where
        H: Fn(IpcRequest) -> IpcResponse + Send + Sync + 'static,
    {
        if let Some(parent) = socket_path.parent() {
            fs::create_dir_all(parent)?;
            // Enforce 0700 permissions on IPC directory
            let perms = fs::Permissions::from_mode(0o700);
            let _ = fs::set_permissions(parent, perms);
        }

        // Safety check: if socket exists, test if a live process is listening
        if socket_path.exists() {
            if UnixStream::connect(&socket_path).is_ok() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::AddrInUse,
                    format!(
                        "IPC socket {:?} is actively in use by another process",
                        socket_path
                    ),
                ));
            }
            // Stale socket, remove it
            let _ = fs::remove_file(&socket_path);
        }

        let listener = UnixListener::bind(&socket_path)?;
        listener.set_nonblocking(true)?;

        // Enforce 0600 on the socket file
        let socket_perms = fs::Permissions::from_mode(0o600);
        let _ = fs::set_permissions(&socket_path, socket_perms);

        let running = Arc::new(AtomicBool::new(true));
        let running_clone = running.clone();
        let handler_arc = Arc::new(handler);

        let handle = thread::spawn(move || {
            while running_clone.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        let h = handler_arc.clone();
                        thread::spawn(move || {
                            let mut reader = BufReader::new(&stream);
                            let mut line = String::new();
                            if reader.read_line(&mut line).is_ok() {
                                let trimmed = line.trim();
                                if let Ok(req) = serde_json::from_str::<IpcRequest>(trimmed) {
                                    let resp = h(req);
                                    if let Ok(resp_json) = serde_json::to_string(&resp) {
                                        let mut writer = &stream;
                                        let _ = writer.write_all(resp_json.as_bytes());
                                        let _ = writer.write_all(b"\n");
                                        let _ = writer.flush();
                                    }
                                }
                            }
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
            socket_path,
            running,
            handle: Some(handle),
        })
    }

    pub fn stop(&mut self) {
        self.running.store(false, Ordering::Relaxed);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
        if self.socket_path.exists() {
            let _ = fs::remove_file(&self.socket_path);
        }
    }
}

impl Drop for IpcServer {
    fn drop(&mut self) {
        self.stop();
    }
}

pub struct IpcClient;

impl IpcClient {
    pub fn send(socket_path: &Path, req: IpcRequest) -> std::io::Result<IpcResponse> {
        let mut stream = UnixStream::connect(socket_path)?;
        let line = serde_json::to_string(&req)?;
        stream.write_all(line.as_bytes())?;
        stream.write_all(b"\n")?;
        stream.flush()?;

        let mut reader = BufReader::new(stream);
        let mut resp_line = String::new();
        reader.read_line(&mut resp_line)?;
        let resp: IpcResponse = serde_json::from_str(resp_line.trim())
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))?;
        Ok(resp)
    }
}
