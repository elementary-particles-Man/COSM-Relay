# COSM-Relay Local Runner Operations Guide (FASTPATH-007)

This document provides exact, reproducible commands for managing the local COSM-Relay daemon on the host. In accordance with strict security standards, no tokens or secrets are hardcoded in command-line arguments or saved to file history.

---

## 1. Prerequisites & Environment Setup

Set up a dedicated isolated state directory and configure secret tokens in protected environment variables (permissions `0600`):

```bash
# 1. Choose dedicated isolated state directory for PoC
export COSM_RELAY_STATE_DIR="/tmp/cosm-poc-fastpath-007"
mkdir -p "$COSM_RELAY_STATE_DIR"
chmod 700 "$COSM_RELAY_STATE_DIR"

# 2. Set authentication token via environment variable (never pass via CLI flag)
export RELAY_AUTH_TOKEN="your-secure-shared-secret-token"

# 3. Specify verified idle test thread and codex binary
export CODEX_TEST_THREAD_ID="01a20de6-d7d3-71e2-af86-fff5791dd860"
export CODEX_BIN="/home/flux/.local/bin/codex"
```

---

## 2. Launch Local Daemon

Launch the release binary on loopback port `127.0.0.1:18888`:

```bash
target/release/tuff-relay serve \
  --lane poc-fastpath-007 \
  --bind 127.0.0.1:18888
```

### Expected Startup Output
```text
HTTP Gateway listening on 127.0.0.1:18888
IPC Server listening on "/tmp/cosm-poc-fastpath-007/ipc/poc-fastpath-007.sock"
COSM-Relay daemon running for lane poc-fastpath-007 (PID: <PID>)
READY
```
> [!NOTE]
> `READY` is only printed after `CodexAppServerClient` has been spawned, initialized, and `thread/resume` on the target thread has succeeded. If thread resumption fails, the process exits immediately with an error before announcing `READY`.

---

## 3. Query Status & Control via Loopback HTTP

### Query Status (Read-Only)
```bash
curl -s -H "Authorization: Bearer $RELAY_AUTH_TOKEN" \
  "http://127.0.0.1:18888/status?lane_id=poc-fastpath-007" | jq .
```

### Query Status (IPC Socket)
```bash
# Query lane state via the 0600 Unix domain socket
target/release/tuff-relay status --lane poc-fastpath-007 --json
```

---

## 4. Graceful Shutdown

To gracefully stop the lane and daemon process:
```bash
curl -s -X POST -H "Authorization: Bearer $RELAY_AUTH_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"lane_id":"poc-fastpath-007"}' \
  "http://127.0.0.1:18888/finish" | jq .
```
The daemon will transition to `Stopped` and terminate with exit code `0`.
