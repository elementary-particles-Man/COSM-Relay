# ChatGPT Sites Plugin Integration Guide for COSM-Relay (R-01 Verification)

## 1. Background & Scope
This guide provides the exact single-copy instructions for establishing a connection between the existing ChatGPT Supervisor conversation (Plus account) and the local COSM-Relay runner via the official **ChatGPT Sites Plugin** capability.

> [!NOTE]
> - ChatGPT Sites allows users to host web apps and lightweight tools on OpenAI infrastructure.
> - Sites can attach Model Context Protocol (MCP) tools and be published as personal or workspace Plugins.
> - The Site acts strictly as a **communication adapter**; it does not replace or host the local Rust Relay Core or local repository worktree.

---

## 2. Prerequisites
1. **Account Tier**: ChatGPT Plus, Pro, Business, or Enterprise.
2. **Access**: Access to `@Sites` in the ChatGPT prompt interface.
3. **Local Runner**: COSM-Relay daemon running on the local host with an accessible HTTP endpoint (via secure TLS tunnel or authenticated webhook connector).

---

## 3. Step-by-Step User Setup Instructions

### Step 1: Open ChatGPT and Invoke `@Sites`
In your ChatGPT conversation interface (or a new session):
```text
@Sites Create a new site named "COSM-Relay-Bridge"
```

### Step 2: Configure MCP Tools on the Site
Provide the following instruction to ChatGPT to bind the official TR-DD-001 §6.3 minimum supervisor tools (5 operations: status, submit, get_report, ack, finish):
```text
Add the following Model Context Protocol (MCP) tool specifications to this site:

1. Tool Name: status
   Description: "Query current phase, epoch, active task, and pause status for a registered lane."
   Parameters:
     - lane_id (string, required): Registered lane identifier (e.g., "tuff-relay").

2. Tool Name: submit
   Description: "Submit a task JSON payload to a registered lane."
   Parameters:
     - lane_id (string, required): Registered lane identifier.
     - task_id (string, required): Unique task identifier.
     - payload_json (string, required): Raw JSON payload string of the task.
     - in_reply_to (string, optional): Message ID of previous report if continuing after decision.

3. Tool Name: get_report
   Description: "Retrieve pending worker report for a registered lane (Read-only)."
   Parameters:
     - lane_id (string, required): Registered lane identifier.

4. Tool Name: ack
   Description: "Acknowledge receipt of a completed or decision-required report."
   Parameters:
     - lane_id (string, required): Registered lane identifier.
     - report_id (string, required): Message ID of the report being acknowledged.

5. Tool Name: finish
   Description: "Gracefully stop and complete the current run on a registered lane."
   Parameters:
     - lane_id (string, required): Registered lane identifier.
```

### Step 3: Test with Harmless Sample Data
In the private interactive preview of the Site, test each tool using mock responses:
- Execute `status` for `tuff-relay` -> verify response format `{"lane_id":"tuff-relay","phase":"WaitTask",...}`.
- Execute `submit` with task `TEST-001` -> verify acceptance confirmation.
- **Do not send sensitive tokens, passwords, or production code during preview.**

### Step 4: Publish Site as Plugin
1. Click the **Publish** button in the Site management pane.
2. Under Sharing, select **Personal** (or workspace-internal if on Team/Enterprise).
3. Once published, navigate to your **Plugins** directory on ChatGPT and verify that **COSM-Relay-Bridge** appears under Installed Plugins.

### Step 5: Connect Outbound Transport to Local Relay
1. When prompted for the backend connection URL in the Plugin settings, enter the authenticated HTTPS endpoint exposed by the local COSM-Relay HTTP gateway.
2. Set the authentication token header matching the local relay config (`Authorization: Bearer <SHARED_SECRET>`).

---

## 4. Operational & Cost Assessment
- **Cost**: Creating and using Sites is included in existing ChatGPT Plus usage limits; no additional OpenAI API billing is incurred.
- **Confirmation Prompts**: By default, ChatGPT presents a confirmation dialog on the first write-action call (`submit`, `ack`, `finish`). Once confirmed by the user, calls in that active conversation proceed automatically.
- **Fallback**: If Plugin invocation fails or times out, the local Relay state remains safely in its current Phase without state loss or auto-dispatch clobbering.
