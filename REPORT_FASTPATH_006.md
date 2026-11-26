# COSM-Relay Fastpath-006 Implementation & Verification Report

## 1. Executive Summary
- **Work ID**: `COSM-RELAY-FASTPATH-006`
- **Execution Date**: 2026-11-26 (UTC) / 2026-11-27 (JST)
- **Status**: **F1 (Local Ready) COMPLETED** / **F2 (Site Connection Spec) COMPLETED** / **F3 (Live ChatGPT Call) BLOCKED on Operator Inbound HTTPS Configuration**
- **Release Binary**: `target/release/tuff-relay`
  - SHA-256: `e282f4cb26fb581ba53547b25417b029e9fb28be16810beb0f2ffd402fbcf50a`
- **Normative Design Specifications Preserved**:
  - `TUFF-Relay_基本設計書_Rev0.1-Draft.md` (TR-BD-001): **0 diffs**
  - `TUFF-Relay_詳細設計書_Rev0.1-Draft.md` (TR-DD-001): **0 diffs**

---

## 2. F1: Local Ready & Critical Code Hardening

All 5 critical findings identified in the task specification were rigorously addressed in the Rust source:

1. **Fail-Closed Authentication in `serve` Mode ([`src/main.rs`](file:///mnt/thpdoc/Develop/TUFF-Relay/src/main.rs))**:
   - `serve` subcommand now requires a non-empty `--token` argument or `RELAY_AUTH_TOKEN` environment variable.
   - Missing or empty tokens immediately abort process startup with exit code `1`.
2. **Runtime Mutex Release during Worker RPC ([`src/runtime.rs`](file:///mnt/thpdoc/Develop/TUFF-Relay/src/runtime.rs))**:
   - Split `execute_dispatch_to_worker` into `begin_worker_turn` and `complete_worker_turn`.
   - `LaneDriver` acquires the lock to record `DispatchIntent` and `WorkerTurnBound`, releases the Mutex across the blocking `codex_client.wait_turn_completion`, and re-acquires the lock only to record turn completion.
   - Allows concurrent HTTP/IPC `status`, `pause`, and other lane queries while worker turns are in flight.
   - Verified via unit test [`tests/concurrency_pause.rs`](file:///mnt/thpdoc/Develop/TUFF-Relay/tests/concurrency_pause.rs).
3. **IPC Interrupt Handling ([`src/main.rs`](file:///mnt/thpdoc/Develop/TUFF-Relay/src/main.rs))**:
   - Removed synthetic fake failure report generation.
   - Returns explicit `NOT_SUPPORTED` error requiring authoritative worker RPC completion rather than faking success.
4. **HTTP Gateway Bounds & Fail-Closed Protection ([`src/adapters/http_gateway.rs`](file:///mnt/thpdoc/Develop/TUFF-Relay/src/adapters/http_gateway.rs))**:
   - Added `MAX_REQUEST_LINE_LENGTH` (8 KiB) returning `414 URI Too Long`.
   - Added `MAX_HEADER_COUNT` (64) and `MAX_HEADER_LINE_LENGTH` (8 KiB) returning `431 Request Header Fields Too Large`.
   - Rejects missing/empty token configurations immediately with `401 Unauthorized`.
5. **TR-DD-001 §3.3 Explicit Decision Marker Contract ([`src/runtime.rs`](file:///mnt/thpdoc/Develop/TUFF-Relay/src/runtime.rs))**:
   - Replaced loose substring matching with `is_explicit_decision_required`.
   - Only recognises structured JSON with `"status": "DECISION_REQUIRED"` or exact uppercase token lines (`DECISION_REQUIRED`, `[DECISION_REQUIRED]`, etc.).

---

## 3. F2: ChatGPT Sites Plugin Integration Specification
- Source-extracted accurate 5-tool MCP schema matching TR-DD-001 §6.3:
  - `status`: `GET /status?lane_id={lane_id}` (Read-only)
  - `submit`: `POST /submit` (Write)
  - `get_report`: `GET /report?lane_id={lane_id}` (Read-only)
  - `ack`: `POST /ack` (Write)
  - `finish`: `POST /finish` (Write)
- Documents updated:
  - [`SITE_SETUP.md`](file:///mnt/thpdoc/Develop/TUFF-Relay/SITE_SETUP.md)
  - [`SITE_PROMPT.txt`](file:///mnt/thpdoc/Develop/TUFF-Relay/SITE_PROMPT.txt)

---

## 4. F3 & Acceptance Criteria Status
Per execution policy, simulated actions are never reported as live passes:
- **AC-01 (Real ChatGPT conversation 3 rounds)**: `NOT_RUN` (blocked pending human bridge setup).
- **AC-02 (Release binary out-of-process 3 rounds)**: `PASS` (PID: 1012874, 3 rounds executed against real Codex worker thread `01a20de6-d7d3-71e2-af86-fff5791dd860`).
- **AC-03 (Bounded transport & IPC)**: `PASS` (401 verified, limits enforced, 0600 socket verified).
- **AC-04 (Mutex release during RPC wait)**: `PASS` (verified by `tests/concurrency_pause.rs`).
- **AC-05 (Multi-lane isolation)**: `PASS` (`tests/multi_lane_isolation.rs`).
- **AC-06 (Normative spec 0 diffs)**: `PASS`.
- **AC-07 (Real development task acceptance)**: `NOT_RUN` (pending supervisor instruction).
- **AC-08 (Long-running reconnect resilience)**: `NOT_RUN`.

---

## 5. Stop Condition & Remaining Operator Action
To complete the bridge between the existing ChatGPT Plus conversation and the local runner:
1. **Provide an Inbound HTTPS URL**: Configure a secure reverse proxy or TLS tunnel mapping an inbound HTTPS domain to local port `127.0.0.1:18888`.
2. **Scaffold Site**: In ChatGPT, summon `@Sites` and paste the contents of [`SITE_PROMPT.txt`](file:///mnt/thpdoc/Develop/TUFF-Relay/SITE_PROMPT.txt).
3. **Set Backend URL & Bearer Token**: Input the HTTPS URL and token into the Site connector.
4. **Publish**: Publish as a **Personal** Plugin and invoke `status` from your active session.
