# COSM-Relay Fastpath-005 Acceptance Report (E2, E3, E4)

## 1. Executive Summary
- **Work ID**: `COSM-RELAY-FASTPATH-005`
- **Execution Date**: 2026-11-26 (UTC) / 2026-11-27 (JST)
- **Status**: **COMPLETE (SUCCESS)**
- **Release Binary**: `target/release/tuff-relay`
  - SHA-256: `ecb9efef074ce13dabbe492ecdcc0b8c74c5eda1b9efef98b43218952859446e`
  - Binary Size: ~650 KiB
- **Authoritative Specifications Preserved**:
  - `TUFF-Relay_基本設計書_Rev0.1-Draft.md` (TR-BD-001) - **0 diffs**
  - `TUFF-Relay_詳細設計書_Rev0.1-Draft.md` (TR-DD-001) - **0 diffs**

---

## 2. E2: Hardened Transport & Security Gateways
1. **Loopback HTTP Gateway**:
   - Strictly bounded request bodies (`MAX_REQUEST_BODY_SIZE = 10 MiB`).
   - Connection/read/write timeouts set to 30 seconds.
   - Strict UTF-8 validation rejecting malformed payloads before processing.
   - Constant-time Bearer token verification with `401 Unauthorized` rejection.
   - Fully supports the 5 TR-DD-001 §6.3 supervisor operations:
     - `GET /status?lane_id=<id>`
     - `POST /submit`
     - `GET /report?lane_id=<id>`
     - `POST /ack`
     - `POST /finish`
2. **IPC Unix Domain Socket**:
   - Enforced permissions: `0700` on parent socket directory, `0600` on socket file.
   - Liveness check on existing socket prevents clobbering active daemon processes.
   - Exposes operator commands: `Status`, `Pause`, `Resume`, `Stop`, `Interrupt`, `ExportRaw`.

---

## 3. E3: Out-of-Process Release Binary Acceptance (3-Round E2E)
- **Daemon Process ID**: `978118` (spawned via `target/release/tuff-relay serve --lane e3-verification-lane --bind 127.0.0.1:18899 --token test-e3-secret-token`)
- **State Directory**: `/tmp/cosm-e3-verification-state` (isolated, non-destructive)
- **Codex Worker Integration**: Tested against verified test thread `01a20de6-d7d3-71e2-af86-fff5791dd860`.
- **E2E 3-Round Execution Results**:
  | Round | Task ID | Message ID | Report ID | Duration | Report SHA-256 |
  |---|---|---|---|---|---|
  | 1 | `task-e3-round-1` | `msg-task-e3-round-1-epoch-1` | `rep-01a218bf-ef15-7362-9f47-e0d9b97f5e14` | 5.03s | `bef4aeef...` |
  | 2 | `task-e3-round-2` | `msg-task-e3-round-2-epoch-1` | `rep-01a218c0-00fb-7120-ac4c-5156f7d91b2f` | 1.38s | `ade6ccd2...` |
  | 3 | `task-e3-round-3` | `msg-task-e3-round-3-epoch-1` | `rep-01a218c0-0683-7063-98e4-c854b6f22740` | 1.71s | `81b79aaf...` |
- **Graceful Shutdown**:
  - `POST /finish` invoked -> transition to `Stopped` -> Daemon cleanly exited with code `0`.

---

## 4. E4: ChatGPT Sites Plugin Integration Artifacts
- **`SITE_SETUP.md`**: Complete step-by-step setup documentation covering account requirements (Plus supported), tool definitions, zero-cost confirmation, and connection parameters.
- **`SITE_PROMPT.txt`**: Exact single copy-paste prompt to instruct `@Sites` in ChatGPT to scaffold the site and register the 5 MCP tools.

---

## 5. Artifacts & Deliverables Matrix
- [release_process_e2e_evidence.json](file:///mnt/thpdoc/Develop/TUFF-Relay/release_process_e2e_evidence.json)
- [execution_evidence_fastpath_005.jsonl](file:///mnt/thpdoc/Develop/TUFF-Relay/execution_evidence_fastpath_005.jsonl)
- [RESULT_FASTPATH_005.json](file:///mnt/thpdoc/Develop/TUFF-Relay/RESULT_FASTPATH_005.json)
- [REPORT_FASTPATH_005.md](file:///mnt/thpdoc/Develop/TUFF-Relay/REPORT_FASTPATH_005.md)
- [SITE_SETUP.md](file:///mnt/thpdoc/Develop/TUFF-Relay/SITE_SETUP.md)
- [SITE_PROMPT.txt](file:///mnt/thpdoc/Develop/TUFF-Relay/SITE_PROMPT.txt)
