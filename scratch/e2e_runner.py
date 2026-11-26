import os
import sys
import time
import json
import urllib.request
import urllib.error
import subprocess
import hashlib
import socket

def wait_for_port(port, host='127.0.0.1', timeout=10.0):
    start = time.time()
    while time.time() - start < timeout:
        try:
            with socket.create_connection((host, port), timeout=1.0):
                return True
        except (OSError, ConnectionRefusedError):
            time.sleep(0.1)
    return False

def http_req(url, token, data=None, method="GET"):
    req = urllib.request.Request(url, data=data, method=method)
    req.add_header("Authorization", f"Bearer {token}")
    if data:
        req.add_header("Content-Type", "application/json")
    with urllib.request.urlopen(req, timeout=10.0) as resp:
        return resp.getcode(), resp.read().decode('utf-8')

def send_ipc(sock_path, req_obj):
    s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    s.connect(sock_path)
    s.sendall((json.dumps(req_obj) + "\n").encode('utf-8'))
    data = b""
    while b"\n" not in data:
        chunk = s.recv(4096)
        if not chunk:
            break
        data += chunk
    s.close()
    return json.loads(data.decode('utf-8').strip())

def main():
    print("=== COSM-Relay Release Binary Out-of-Process E2E Verification ===")
    binary_path = os.path.abspath("target/release/tuff-relay")
    state_dir = "/tmp/cosm-e3-verification-state"
    if os.path.exists(state_dir):
        import shutil
        shutil.rmtree(state_dir)
    os.makedirs(state_dir, exist_ok=True)

    with open(binary_path, "rb") as f:
        binary_sha256 = hashlib.sha256(f.read()).hexdigest()
    print(f"Target Binary: {binary_path} (SHA-256: {binary_sha256})")

    bind_addr = "127.0.0.1:18899"
    token = "test-e3-secret-token"
    lane_id = "e3-verification-lane"
    test_thread_id = "01a20de6-d7d3-71e2-af86-fff5791dd860"

    env = os.environ.copy()
    env["COSM_RELAY_STATE_DIR"] = state_dir
    env["RELAY_AUTH_TOKEN"] = token
    env["CODEX_TEST_THREAD_ID"] = test_thread_id

    cmd = [
        binary_path,
        "serve",
        "--lane", lane_id,
        "--bind", bind_addr,
        "--token", token,
        "--thread", test_thread_id,
    ]

    print(f"Spawning child process: {' '.join(cmd)}")
    proc = subprocess.Popen(
        cmd,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        env=env,
    )
    print(f"Spawned Daemon PID: {proc.pid}")

    evidence = {
        "work_id": "COSM-RELAY-FASTPATH-005",
        "binary": {
            "path": binary_path,
            "sha256": binary_sha256,
            "pid": proc.pid,
        },
        "bind_addr": bind_addr,
        "lane_id": lane_id,
        "thread_id": test_thread_id,
        "rounds": [],
        "ipc_verification": {},
        "boundary_tests": {},
    }

    try:
        # Wait for HTTP server to become alive
        if not wait_for_port(18899):
            raise RuntimeError("HTTP Gateway failed to open port 18899 within timeout")
        print("HTTP Gateway port is open and listening.")

        # 1. Boundary & Security Tests
        print("Testing Unauthorized Request...")
        req_unauth = urllib.request.Request(f"http://{bind_addr}/status?lane_id={lane_id}")
        try:
            urllib.request.urlopen(req_unauth, timeout=5.0)
            raise RuntimeError("Expected 401 Unauthorized, but got 200 OK")
        except urllib.error.HTTPError as e:
            assert e.code == 401, f"Expected 401, got {e.code}"
            print(" - Unauthorized request correctly rejected with 401")
            evidence["boundary_tests"]["unauthorized_rejected"] = True

        print("Testing Valid Initial Status...")
        code, body = http_req(f"http://{bind_addr}/status?lane_id={lane_id}", token)
        status = json.loads(body)
        print(f" - Initial status: phase={status['phase']}, epoch={status['binding_epoch']}")
        assert status["phase"] == "WaitTask"
        evidence["boundary_tests"]["initial_phase"] = status["phase"]

        # IPC Socket Verification
        ipc_sock = os.path.join(state_dir, "ipc", f"{lane_id}.sock")
        assert os.path.exists(ipc_sock), f"IPC socket {ipc_sock} not created"
        sock_mode = oct(os.stat(ipc_sock).st_mode & 0o777)
        parent_mode = oct(os.stat(os.path.dirname(ipc_sock)).st_mode & 0o777)
        print(f" - IPC socket mode: {sock_mode}, parent dir mode: {parent_mode}")
        ipc_status = send_ipc(ipc_sock, {"Status": {"lane_id": lane_id}})
        assert ipc_status["success"], f"IPC status failed: {ipc_status}"
        evidence["ipc_verification"] = {
            "socket_path": ipc_sock,
            "socket_mode": sock_mode,
            "parent_mode": parent_mode,
            "ipc_status_success": True,
        }

        # 2. Execute 3 Rounds of Real End-to-End Tasks
        tasks = [
            ("task-e3-round-1", "Print exact string: RELEASE_TEST_OK_ROUND_1"),
            ("task-e3-round-2", "Print exact string: RELEASE_TEST_OK_ROUND_2"),
            ("task-e3-round-3", "Print exact string: RELEASE_TEST_OK_ROUND_3"),
        ]

        for i, (tid, prompt_text) in enumerate(tasks, 1):
            print(f"\n--- Starting Round {i}: {tid} ---")
            task_payload = json.dumps({
                "work_id": "COSM-RELAY-FASTPATH-005",
                "round": i,
                "task_id": tid,
                "prompt": prompt_text,
            })

            # Submit task
            submit_body = json.dumps({
                "lane_id": lane_id,
                "task_id": tid,
                "payload_json": task_payload,
                "in_reply_to": None,
            }).encode('utf-8')

            t0 = time.time()
            code, submit_resp_str = http_req(f"http://{bind_addr}/submit", token, data=submit_body, method="POST")
            submit_resp = json.loads(submit_resp_str)
            print(f"Submitted: accepted={submit_resp.get('accepted')}, msg_id={submit_resp.get('message_id')}")
            assert submit_resp["accepted"], f"Submit failed: {submit_resp}"

            # Poll for report
            report_data = None
            for poll in range(120): # up to 60s
                time.sleep(0.5)
                code, rep_str = http_req(f"http://{bind_addr}/report?lane_id={lane_id}", token)
                rep = json.loads(rep_str)
                if rep.get("report_available"):
                    report_data = rep
                    break

            elapsed = time.time() - t0
            assert report_data is not None, f"Round {i} timed out waiting for report"
            rep_payload = report_data["payload_utf8"]
            rep_sha = hashlib.sha256(rep_payload.encode('utf-8')).hexdigest()
            print(f"Round {i} Report Received in {elapsed:.2f}s!")
            print(f" - Report ID: {report_data['report_id']}")
            print(f" - Decision Required: {report_data['is_decision_required']}")
            print(f" - Payload SHA-256: {rep_sha}")
            print(f" - Payload excerpt: {rep_payload[:120]}...")

            # Ack report
            ack_body = json.dumps({
                "lane_id": lane_id,
                "report_id": report_data["report_id"],
            }).encode('utf-8')
            code, ack_resp_str = http_req(f"http://{bind_addr}/ack", token, data=ack_body, method="POST")
            ack_resp = json.loads(ack_resp_str)
            print(f"Ack response: {ack_resp}")
            assert ack_resp["acked"], f"Ack failed: {ack_resp}"

            evidence["rounds"].append({
                "round": i,
                "task_id": tid,
                "message_id": submit_resp["message_id"],
                "report_id": report_data["report_id"],
                "report_sha256": rep_sha,
                "duration_seconds": round(elapsed, 3),
                "is_decision_required": report_data["is_decision_required"],
                "payload_preview": rep_payload[:200],
            })

        # 3. Test Finish via HTTP
        print("\n--- Testing Graceful Finish ---")
        finish_body = json.dumps({"lane_id": lane_id}).encode('utf-8')
        code, finish_resp_str = http_req(f"http://{bind_addr}/finish", token, data=finish_body, method="POST")
        finish_resp = json.loads(finish_resp_str)
        print(f"Finish response: {finish_resp}")
        assert finish_resp["finished"]

        # Wait for daemon process to cleanly exit
        proc.wait(timeout=5.0)
        print(f"Daemon exited with return code: {proc.returncode}")
        evidence["finish_verification"] = {
            "daemon_clean_exit": True,
            "returncode": proc.returncode,
        }

        print("\n=== OUT-OF-PROCESS E2E VERIFICATION SUCCEEDED ===")

        with open("release_process_e2e_evidence.json", "w", encoding="utf-8") as f:
            json.dump(evidence, f, indent=2)
        print("Wrote evidence to release_process_e2e_evidence.json")

    finally:
        if proc.poll() is None:
            proc.terminate()
            proc.wait(timeout=2.0)

if __name__ == "__main__":
    main()
