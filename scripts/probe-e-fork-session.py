"""Probe E: can codex-core + app-server act as a real Session Host?

Binary question (A/B): drive `codex app-server --stdio` end to end -
initialize -> thread/start -> thread/queue/add -> thread/queue/start -> wait
for a REAL file side effect (unique file + nonce) in the workspace.

Run in your own terminal with the FORK binary (codex-deepseek), e.g.:
    python scripts\\probe-e-fork-session.py
Default codex: D:\\experience_codex\\codex-main\\codex-rs\\target\\debug\\codex.exe
Default CODEX_HOME: D:\\experience_codex\\codex-main\\.codex-exp-home
"""

import json
import os
import re
import subprocess
import sys
import threading
import time
import uuid
from pathlib import Path

CODEX = sys.argv[1] if len(sys.argv) > 1 else (
    r"D:\experience_codex\codex-main\codex-rs\target\debug\codex.exe"
)
CODEX_HOME = sys.argv[2] if len(sys.argv) > 2 else (
    r"D:\experience_codex\codex-main\.codex-exp-home"
)
WORKSPACE_ROOT = Path(sys.argv[3]) if len(sys.argv) > 3 else (
    Path(r"D:\experience_codex\experience-main") / "probe-e-workspace"
)


def load_deepseek_key():
    cfg = Path.home() / ".codex" / "config.toml"
    if not cfg.exists():
        return None
    text = cfg.read_text(encoding="utf-8", errors="ignore")
    match = re.search(
        r"\[model_providers\.deepseek\][^\[]*?experimental_bearer_token\s*=\s*\"([^\"]+)\"",
        text,
        re.S,
    )
    return match.group(1) if match else None


class Server:
    def __init__(self):
        env = dict(os.environ)
        env["CODEX_HOME"] = CODEX_HOME
        key = load_deepseek_key()
        if key:
            env["DEEPSEEK_API_KEY"] = key
        self.proc = subprocess.Popen(
            [CODEX, "app-server", "--stdio"],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            env=env,
            text=True,
            encoding="utf-8",
            errors="replace",
        )
        self.lines = []
        self.lock = threading.Lock()
        self.event = threading.Event()
        threading.Thread(target=self._reader, daemon=True).start()
        threading.Thread(target=self._stderr, daemon=True).start()

    def _reader(self):
        for line in self.proc.stdout:
            with self.lock:
                self.lines.append(line.rstrip("\r\n"))
            self.event.set()

    def _stderr(self):
        for line in self.proc.stderr:
            with self.lock:
                self.lines.append("[stderr] " + line.rstrip("\r\n"))

    def send(self, payload):
        self.proc.stdin.write(json.dumps(payload, ensure_ascii=False) + "\n")
        self.proc.stdin.flush()

    def wait_for(self, predicate, timeout):
        deadline = time.time() + timeout
        while time.time() < deadline:
            self.event.wait(0.25)
            self.event.clear()
            with self.lock:
                for line in list(self.lines):
                    try:
                        msg = json.loads(line)
                    except Exception:
                        continue
                    if predicate(msg):
                        return msg
        return None


def find_thread_id(value):
    if isinstance(value, dict):
        for key, item in value.items():
            if key in ("threadId", "thread_id", "id") and isinstance(item, str):
                return item
            found = find_thread_id(item)
            if found:
                return found
    elif isinstance(value, list):
        for item in value:
            found = find_thread_id(item)
            if found:
                return found
    return None


def main():
    workspace = WORKSPACE_ROOT
    workspace.mkdir(parents=True, exist_ok=True)
    nonce = uuid.uuid4().hex[:12]
    filename = f"experience-probe-e-{nonce}.txt"
    target = workspace / filename
    print(f"codex      : {CODEX}")
    print(f"CODEX_HOME : {CODEX_HOME}")
    print(f"workspace  : {workspace}")
    print(f"target     : {target}")
    print(f"nonce      : {nonce}")
    print()

    server = Server()

    server.send({
        "method": "initialize",
        "id": 1,
        "params": {
            "clientInfo": {"name": "experience-probe-e", "version": "0.0.0"},
            "capabilities": {"experimentalApi": True},
        },
    })
    init = server.wait_for(lambda m: m.get("id") == 1, 30)
    if init is None:
        print("FAIL: no initialize reply"); return 1
    print("initialize: OK")
    server.send({"method": "initialized"})

    server.send({
        "method": "thread/start",
        "id": 2,
        "params": {
            "cwd": str(workspace),
            "threadSource": "experience-probe-e",
        },
    })
    started = server.wait_for(lambda m: m.get("id") == 2, 30)
    if started is None:
        print("FAIL: thread/start silent (no reply) -> host absent?"); return 1
    if "error" in started:
        print("thread/start error:", json.dumps(started["error"])); return 1
    thread_id = find_thread_id(started.get("result", {}))
    print(f"thread/start: OK threadId={thread_id}")
    if not thread_id:
        print("note: threadId not found in reply; dumping result")
        print(json.dumps(started.get("result", {}))[:2000]); return 1

    task = (
        f"Create a file named {filename} in the directory {workspace} "
        f"whose content is exactly {nonce}, then stop. Do not modify anything else."
    )
    server.send({
        "method": "thread/queue/add",
        "id": 3,
        "params": {
            "threadId": thread_id,
            "clientUserMessageId": "probe-e-msg-1",
            "input": [{"type": "text", "text": task}],
        },
    })
    queued = server.wait_for(lambda m: m.get("id") == 3, 30)
    if queued is None or "error" in queued:
        print("FAIL: thread/queue/add", json.dumps(queued or {})); return 1
    print("thread/queue/add: OK")

    server.send({
        "method": "thread/queue/start",
        "id": 4,
        "params": {"threadId": thread_id},
    })
    print("thread/queue/start: sent; waiting for real file side effect ...")

    deadline = time.time() + 300
    while time.time() < deadline:
        if target.exists() and target.read_text(encoding="utf-8", errors="ignore").strip() == nonce:
            print(f"PROBE-E: PASS - file created and verified: {target}")
            return 0
        time.sleep(2)

    print("PROBE-E: FAIL - file not created within 300s")
    print("--- transcript tail ---")
    with server.lock:
        tail = server.lines[-60:]
    print("\n".join(tail))
    return 1


if __name__ == "__main__":
    sys.exit(main())
