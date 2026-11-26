# ChatGPT Sites Plugin Integration Guide for COSM-Relay

## 1. Background & Scope
This document specifies the exact configuration, endpoint contracts, and setup instructions for connecting an existing **ChatGPT Plus** Supervisor session to the local COSM-Relay daemon via the official **ChatGPT Sites** capability.

> [!IMPORTANT]
> - **Zero Rust Core Migration**: ChatGPT Sites functions strictly as a lightweight communication adapter routing HTTP requests to the local runner. The deterministic Rust state machine, journal persistence, and worktree isolation remain entirely local.
> - **Network Connectivity Reality**: OpenAI ChatGPT Sites execute on cloud infrastructure (`*.openai.com`). A local `127.0.0.1` loopback listener cannot be reached directly across the public Internet without an authenticated HTTPS reverse proxy or tunnel.
> - **Zero-Cost & Policy Compliance**: In accordance with user directives, no unauthorized external SaaS subscriptions or unapproved public tunnels are automatically deployed. Step 5 details the exact configuration required once an operator provides a TLS reverse proxy / tunnel endpoint.

---

## 2. Model Context Protocol (MCP) Tool Specifications (TR-DD-001 §6.3)

The Site registers 5 Model Context Protocol tools. Each maps directly to an authenticated HTTP endpoint on the COSM-Relay runner.

### 1. `status` (Read-only)
- **HTTP Method & Path**: `GET /status?lane_id={lane_id}`
- **Headers**: `Authorization: Bearer <SECRET_TOKEN>`
- **Query Parameters**:
  - `lane_id` (string, required): Registered lane identifier (e.g. `"tuff-relay"`).
- **Response Format (200 OK)**:
  ```json
  {
    "lane_id": "tuff-relay",
    "phase": "WaitTask",
    "binding_epoch": 1,
    "active_task": null,
    "pending_report": null,
    "is_decision_required": false,
    "pause_requested": false
  }
  ```

### 2. `submit` (Write)
- **HTTP Method & Path**: `POST /submit`
- **Headers**: `Authorization: Bearer <SECRET_TOKEN>`, `Content-Type: application/json`
- **Request Body**:
  ```json
  {
    "lane_id": "tuff-relay",
    "task_id": "TASK-20261127-001",
    "payload_json": "{\"prompt\":\"Execute task...\"}",
    "in_reply_to": null
  }
  ```
- **Response Format (200 OK)**:
  ```json
  {
    "accepted": true,
    "message_id": "msg-TASK-20261127-001-epoch-1",
    "error": null
  }
  ```

### 3. `get_report` (Read-only)
- **HTTP Method & Path**: `GET /report?lane_id={lane_id}`
- **Headers**: `Authorization: Bearer <SECRET_TOKEN>`
- **Query Parameters**:
  - `lane_id` (string, required)
- **Response Format (200 OK)**:
  ```json
  {
    "report_available": true,
    "report_id": "rep-01a218df-1892-7d10-9b81-6a7353ca00b5",
    "is_decision_required": false,
    "payload_utf8": "Task report output..."
  }
  ```

### 4. `ack` (Write)
- **HTTP Method & Path**: `POST /ack`
- **Headers**: `Authorization: Bearer <SECRET_TOKEN>`, `Content-Type: application/json`
- **Request Body**:
  ```json
  {
    "lane_id": "tuff-relay",
    "report_id": "rep-01a218df-1892-7d10-9b81-6a7353ca00b5"
  }
  ```
- **Response Format (200 OK)**:
  ```json
  {
    "acked": true,
    "new_phase": "WaitTask"
  }
  ```

### 5. `finish` (Write)
- **HTTP Method & Path**: `POST /finish`
- **Headers**: `Authorization: Bearer <SECRET_TOKEN>`, `Content-Type: application/json`
- **Request Body**:
  ```json
  {
    "lane_id": "tuff-relay"
  }
  ```
- **Response Format (200 OK)**:
  ```json
  {
    "finished": true,
    "new_phase": "Stopped"
  }
  ```

---

## 3. Step-by-Step Operator Setup Instructions

### Step 1: Start Local Runner in Secure Daemon Mode
On the local machine, launch the release binary:
```bash
target/release/tuff-relay serve --lane tuff-relay --bind 127.0.0.1:18888 --token "$RELAY_AUTH_TOKEN"
```
Verify the output confirms:
```text
HTTP Gateway listening on 127.0.0.1:18888
IPC Server listening on "/.../ipc/tuff-relay.sock"
COSM-Relay daemon running for lane tuff-relay (PID: ...)
READY
```

### Step 2: Open ChatGPT and Invoke `@Sites`
In your ChatGPT conversation (or a dedicated integration session):
```text
@Sites Create a new site named "COSM-Relay-Bridge"
```

### Step 3: Configure MCP Tools
Copy and paste the exact contents of [`SITE_PROMPT.txt`](file:///mnt/thpdoc/Develop/TUFF-Relay/SITE_PROMPT.txt) into the conversation with `@Sites`.

### Step 4: Test in Private Site Preview
In the private interactive preview of the Site:
1. Execute `status` for `tuff-relay` -> verify mock/live status response.
2. Confirm parameter validation works as expected.

### Step 5: Connect Outbound Transport & Publish
1. Provide the authenticated HTTPS endpoint URL (configured via user's approved reverse proxy / TLS tunnel) in the Site connection settings.
2. Configure header: `Authorization: Bearer <SECRET_TOKEN>`.
3. Under Publish options, select **Personal** plugin.
4. Return to the main ChatGPT conversation and activate the **COSM-Relay-Bridge** plugin.
