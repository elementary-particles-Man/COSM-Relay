# COSM-RELAY-FASTPATH-003 Engineering & Verification Report

## 1. Executive Summary
- **Work ID**: `COSM-RELAY-FASTPATH-003`
- **Predecessor**: `TUFF-RELAY-FASTPATH-002`
- **Target Repository**: `elementary-particles-Man/COSM-Relay` (`main`)
- **Observed Remote Commit**: `120c052aceac8a1db7cead0433e20209c38615e6`
- **Result**: **Core, Runtime, Codex Live Turn, and Supervisor MCP Server fully implemented and verified in Pure Rust (19/19 tests passing).**
- **Critical Path Bottleneck Isolated**: Verified that ChatGPT Plus personal web chat cannot connect raw `localhost` custom MCP servers directly without Business/Enterprise tier or a published ChatGPT Site Plugin.

---

## 2. P0-SUPERVISOR-ROUTE Investigation Findings
Official OpenAI documentation was researched to identify valid connection routes:
1. **Direct Custom MCP Server** ([help.openai.com/en/articles/12584461](https://help.openai.com/en/articles/12584461)):
   - Restricted to **ChatGPT Business, Enterprise, and Edu** workspaces.
   - Requires admin RBAC permissions (*"Create plugins with MCPs"*) and a public HTTPS endpoint.
2. **ChatGPT Sites Plugin Route** ([help.openai.com/en/articles/20001547](https://help.openai.com/en/articles/20001547)):
   - Available on Plus/Pro/Business.
   - Workflow: Create a Site via `@Sites`, instruct ChatGPT/Codex to add MCP server tools, test, and publish the Site to create a plugin usable in chats.
   - Requires an outbound connector or public HTTPS gateway from the Site to the local Relay.
3. **Implication**:
   - The connection barrier is **not merely tunnel unavailability**, but the **ChatGPT subscription and client architecture tier**. Tunneling alone does not provide custom MCP settings to personal Plus accounts without custom GPT actions or ChatGPT Sites.

---

## 3. P1 & P2 Implementation Advances

### 3.1 P1-REAL-RUNNER Fixes
- **OS-level Exclusive Lock (`lane.lock`)**:
  - Implemented `flock(fd, LOCK_EX | LOCK_NB)` via `libc` bindings in [journal.rs](file:///mnt/thpdoc/Develop/TUFF-Relay/src/core/journal.rs).
  - Guarantees single-process exclusivity per lane; concurrent attempts return `ErrorKind::WouldBlock`.
- **Runtime Execution Loop (`src/runtime.rs`)**:
  - Added `execute_dispatch_to_worker`:
    - Dispatches accepted task to Codex app-server.
    - Transitions through `DispatchIntentRecorded` -> `WorkerTurnBound` -> `WorkerTurnCompleted`.
    - Persists events prior to any external side effect.
- **Operator CLI & Server (`src/main.rs`)**:
  - Real `doctor` validating environment, paths, toolchains, and Codex binary.
  - Added `mcp-server --lane <lane-id>` command launching the standard JSON-RPC 2.0 MCP server over stdio.
  - Subcommands `pause`, `resume`, `stop` wired directly into state machine events.

### 3.2 P2-CODEX-TURN-LIVE Verification
- Executed real `turn/start` on test thread `01a20de6-d7d3-71e2-af86-fff5791dd860` with prompt:
  ```json
  {"task_id": "FASTPATH-003-VERIFY", "instruction": "Return exact JSON without code fences: {\"result\": \"PASS\", \"marker\": \"003\"}"}
  ```
- Result:
  - Turn ID: `01a21889-96a8-7dc3-bc0e-02254feb5075`
  - Status: `completed`
  - Extracted Raw Report: `{"result":"PASS","marker":"003"}`
  - Verified in `tests/codex_live_turn.rs`.

### 3.3 P2-SUPERVISOR-TRANSPORT (MCP Server)
- Implemented `SupervisorMcpServer` in [mcp_server.rs](file:///mnt/thpdoc/Develop/TUFF-Relay/src/adapters/mcp_server.rs):
  - Conforms to Model Context Protocol (2024-11-05).
  - Handles `initialize`, `tools/list`, and `tools/call`.
  - Exposes TR-DD-001 §6.3 minimum tools: `status`, `submit`, `get_report`, and `ack`.
  - Verified in `tests/supervisor_mcp_server.rs`.

---

## 4. Authoritative Verification Matrix

### 4.1 Acceptance Criteria (TR-BD-001 §8)
- **AC-01 (Existing Chat/Worker 3 Rounds, 0 manual ops)**: `NOT_RUN / BLOCKED` (External ChatGPT Web UI connection blocked on human route selection; Mock loop verified 3 full rounds with 0 manual interventions).
- **AC-02 (Exact Raw Report Return)**: `PASS` (Preserves UTF-8 and SHA-256 byte-for-byte).
- **AC-03 (No Unapproved Progress on Decision Required)**: `PASS` (`WaitDecision` lock verified).
- **AC-04 (Duplicate Prevention & Crash Replay)**: `PASS` (`flock` + sequence verification + crash replay).
- **AC-05 (Multi-lane Independent Operation)**: `PASS` (Isolated state directories and journals).
- **AC-06 (Inspection, Pause, Resume, Stop)**: `PASS` (CLI subcommands verified).
- **AC-07 (Single Real Development Task Acceptance)**: `NOT_RUN` (Awaiting live supervisor route).
- **AC-08 (Long-running & Reconnect Testing)**: `BLOCKED` (Pending human route selection).

### 4.2 Quality Gate Summary
- `cargo fmt --check`: **PASS (exit code 0)**
- `cargo clippy --all-targets -- -D warnings`: **PASS (exit code 0)**
- `cargo test --all-targets`: **PASS (exit code 0, 19/19 tests passing)**
- `cargo build --release`: **PASS (exit code 0, size 646 KiB)**
- Design document diffs: **0 diffs** (`TUFF-Relay_基本設計書_Rev0.1-Draft.md`, `TUFF-Relay_詳細設計書_Rev0.1-Draft.md`).

---

## 5. Single Next Required Human Action
To resolve R-01 and connect ChatGPT to the Relay:
- **Decision & Setup Required**:
  1. **Option A (ChatGPT Sites)**: Use `@Sites` in ChatGPT to create a lightweight site, attach the Relay MCP tool definition, and publish it as a plugin.
  2. **Option B (Public HTTPS Tunnel / Custom Action)**: Launch a tunnel (e.g., `cloudflared tunnel run` or `ngrok`) pointing to `tuff-relay mcp-server` and register the public URL as an action or MCP server in ChatGPT.
