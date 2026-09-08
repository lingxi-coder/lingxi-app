#!/usr/bin/env python3
"""Offline smoke for the actual CLI pane worker and its private parent protocol.

Usage: python3 scripts/teammate_worker_smoke.py /tmp/lingxi-teammate-target/debug/lingxi-cli
Uses only a fake loopback model, synthetic credentials, isolated HOME/config/cwd,
and an authenticated Unix socket. A real PTY supplies the worker terminal.
No environment credentials/settings are inherited. Proxy variables refuse any
non-loopback HTTP destination. Output is retained in an optional --output folder.
This tests the worker transport; it does not exercise tmux/iTerm2 pane creation.
--test-harness runs the exact ignored CLI test with test-only fixture storage;
that result is a real worker/engine test, not a packaged production CLI test.
"""
from __future__ import annotations

import argparse
import fcntl
import json
import os
from pathlib import Path
import pty
import queue
import re
import secrets
import struct
import signal
import socket
import subprocess
import tempfile
import termios
import threading
import time
import uuid
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

MARKER = "PANE_WORKER_OFFLINE_SMOKE"
TEXT = "PANE_WORKER_MODEL_OUTPUT_OK"
WAKE = "PANE_WORKER_WAKE_OUTPUT_OK"


def dump(path, value):
    path.write_text(json.dumps(value, ensure_ascii=False, indent=2) + "\n")


def environment(root, base):
    return {
        "PATH": os.defpath,
        "HOME": str(root / "home"),
        "TMPDIR": str(root / "tmp"),
        "TERM": "xterm-256color",
        "LANG": "en_US.UTF-8",
        "LINGXI_CONFIG_DIR": str(root / "config"),
        "CLAUDE_CONFIG_DIR": str(root / "config"),
        "LINGXI_API_KEY": "fake-local-pane-smoke-only",
        "ANTHROPIC_API_KEY": "sk-ant-api03-fake-local-pane-smoke-only",
        "LINGXI_API_BASE_URL": base,
        "ANTHROPIC_BASE_URL": base,
        "LINGXI_EXPERIMENTAL_AGENT_TEAMS": "1",
        "CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS": "1",
        "LINGXI_DISABLE_NONESSENTIAL_TRAFFIC": "1",
        "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC": "1",
        "DISABLE_AUTOUPDATER": "1",
        "DISABLE_TELEMETRY": "1",
        "DISABLE_ERROR_REPORTING": "1",
        "HTTP_PROXY": base, "HTTPS_PROXY": base, "ALL_PROXY": base,
        "http_proxy": base, "https_proxy": base, "all_proxy": base,
        "NO_PROXY": "127.0.0.1,localhost", "no_proxy": "127.0.0.1,localhost",
    }


def handler(root, requests, rejected):
    class API(BaseHTTPRequestHandler):
        def log_message(self, *_args):
            pass

        def do_CONNECT(self):
            rejected.append("CONNECT")
            self.send_error(403, "External network disabled")

        def do_GET(self):
            rejected.append(self.path)
            self.send_error(403, "Only the local model mock is available")

        def do_POST(self):
            if self.path.split("?")[0] not in ("/v1/messages", "/v1/messages/count_tokens"):
                rejected.append(self.path)
                self.send_error(403, "Only the local model mock is available")
                return
            body = json.loads(self.rfile.read(int(self.headers.get("Content-Length", "0"))))
            requests.append(body)
            dump(root / "requests.json", requests)
            if "count_tokens" in self.path:
                return self.respond({"input_tokens": 100})
            messages = json.dumps(body.get("messages", []), ensure_ascii=False)
            smoke = MARKER in messages
            sent = "toolu_pane_smoke_message" in messages
            wake = "WAKE_THE_PANE_WORKER" in messages
            content = ([{"type": "tool_use", "id": "toolu_pane_smoke_message", "name": "SendMessage",
                         "input": {"to": "main", "message": "SMOKE_RPC_FROM_CHILD", "summary": "Offline smoke child message"}}]
                       if smoke and not sent else
                       [{"type": "text", "text": WAKE if wake else TEXT}])
            stop = "tool_use" if content[0]["type"] == "tool_use" else "end_turn"
            message = {"id": "msg_" + uuid.uuid4().hex, "type": "message", "role": "assistant",
                       "model": body.get("model", "claude-sonnet-4-6"), "content": [],
                       "stop_reason": None, "stop_sequence": None,
                       "usage": {"input_tokens": 100, "output_tokens": 0,
                                 "cache_creation_input_tokens": 0, "cache_read_input_tokens": 0}}
            if not body.get("stream"):
                return self.respond(dict(message, content=content, stop_reason=stop))
            block = content[0]
            events = [{"type": "message_start", "message": message}]
            if block["type"] == "tool_use":
                events += [{"type": "content_block_start", "index": 0,
                            "content_block": dict(block, input={})},
                           {"type": "content_block_delta", "index": 0,
                            "delta": {"type": "input_json_delta", "partial_json": json.dumps(block["input"])}}]
            else:
                events += [{"type": "content_block_start", "index": 0,
                            "content_block": {"type": "text", "text": ""}},
                           {"type": "content_block_delta", "index": 0,
                            "delta": {"type": "text_delta", "text": block["text"]}}]
            events += [{"type": "content_block_stop", "index": 0},
                       {"type": "message_delta", "delta": {"stop_reason": stop, "stop_sequence": None},
                        "usage": {"output_tokens": 20}}, {"type": "message_stop"}]
            payload = "".join(f"event: {event['type']}\ndata: {json.dumps(event)}\n\n" for event in events).encode()
            self.send_response(200)
            self.send_header("Content-Type", "text/event-stream")
            self.send_header("Content-Length", str(len(payload)))
            self.end_headers()
            self.wfile.write(payload)

        def respond(self, body):
            payload = json.dumps(body).encode()
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(payload)))
            self.end_headers()
            self.wfile.write(payload)
    return API


def run(binary, root, timeout, test_harness=False):
    for folder in ("home", "config", "tmp", "cwd"):
        (root / folder).mkdir(parents=True, exist_ok=True)
    private = Path(tempfile.mkdtemp(prefix="lx-smoke-", dir="/tmp"))
    private.chmod(0o700)
    requests, rejected, frames = [], [], []
    server = ThreadingHTTPServer(("127.0.0.1", 0), handler(root, requests, rejected))
    base = f"http://127.0.0.1:{server.server_port}"
    listener = socket.socket(socket.AF_UNIX)
    listener.bind(str(private / "ipc"))
    listener.listen(1)
    listener.settimeout(timeout)
    parent = str(uuid.uuid4())
    token = secrets.token_hex(32)
    manifest_path = private / "launch.json"
    manifest = {"socket_path": str(private / "ipc"), "token": token,
                "agent_id": str(uuid.uuid4()), "name": "smoke", "team_name": "session-smoke",
                "parent_session_id": parent,
                "request": {"subagent_type": "general-purpose", "prompt": MARKER,
                            "name": "smoke", "description": "Offline pane protocol smoke",
                            "model": "claude-sonnet-4-6", "mode": "default", "cwd": str(root / "cwd"),
                            "origin_session_id": parent, "depth": 1}}
    manifest_path.write_text(json.dumps(manifest))
    manifest_path.chmod(0o600)
    master, slave = pty.openpty()
    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 24, 100, 0, 0))
    def attach_terminal():
        os.setsid()
        fcntl.ioctl(0, termios.TIOCSCTTY, 0)
    env = environment(root, base)
    if test_harness:
        env["LINGXI_PANE_SMOKE_MANIFEST"] = str(manifest_path)
        env["RUST_MIN_STACK"] = "33554432"
        command = [str(binary), "--exact", "teammate_worker::unix::tests::offline_worker_engine_smoke",
                   "--ignored", "--nocapture"]
    else:
        command = [str(binary), "--bare", "--teammate-launch-file", str(manifest_path)]
    process = subprocess.Popen(command,
                               cwd=root / "cwd", env=env, stdin=slave,
                               stdout=slave, stderr=slave, preexec_fn=attach_terminal)
    os.close(slave)
    terminal = bytearray()
    permission_answers = []
    def drain_terminal():
        try:
            while data := os.read(master, 65536):
                terminal.extend(data)
                (root / "terminal.log").write_bytes(terminal)
                # Only approve the synthetic SendMessage operation emitted by
                # this mock. An unexpected tool prompt remains unanswered.
                count = len(re.findall(rb"needs your permission to use SendMessage\r?\n\[y/N\]", terminal))
                while len(permission_answers) < count:
                    os.write(master, b"y\n")
                    permission_answers.append("SendMessage")
        except OSError:
            pass
    terminal_thread = threading.Thread(target=drain_terminal, daemon=True)
    terminal_thread.start()
    threading.Thread(target=server.serve_forever, daemon=True).start()
    connection = None
    report = {"binary": str(binary), "passed": False,
              "execution": "worker-engine-test-harness" if test_harness else "production-cli"}
    try:
        connection, _ = listener.accept()
        incoming = queue.Queue()
        def read_frames():
            try:
                with connection.makefile("rb") as stream:
                    for line in stream:
                        incoming.put(json.loads(line))
            except Exception as error:
                incoming.put({"type": "reader_error", "error": str(error)})
            finally:
                incoming.put({"type": "eof"})
        threading.Thread(target=read_frames, daemon=True).start()
        def send(frame):
            connection.sendall((json.dumps(frame) + "\n").encode())
        deadline = time.monotonic() + timeout
        hello = ready = rpc = idle = woke = False
        output = ""
        while time.monotonic() < deadline:
            try:
                frame = incoming.get(timeout=0.2)
            except queue.Empty:
                if process.poll() is not None:
                    raise AssertionError(f"worker exited early: {process.returncode}")
                continue
            frames.append({k: v for k, v in frame.items() if k != "token"})
            dump(root / "frames.json", frames)
            kind = frame["type"]
            if kind == "hello":
                assert frame["token"] == token, "Hello token mismatch"
                hello = True
            elif kind == "ready":
                assert hello, "Ready before authenticated Hello"
                ready = True
            elif kind == "send_message":
                assert frame["input"]["to"] == "main"
                assert frame["input"]["message"] == "SMOKE_RPC_FROM_CHILD"
                rpc = True
                send({"type": "send_message_result", "id": frame["id"],
                      "result": {"success": True, "message": "SMOKE_PARENT_ACK"}, "is_error": False})
            elif kind == "output":
                output += frame["text"]
                # Status frames are 100ms snapshots. A fast wake can finish
                # between polls, so the unchanged idle snapshot is deduplicated.
                # Require the new wake response and its actual completion event;
                # the first idle state and parent message remain mandatory.
                if idle and WAKE in output:
                    wake_tail = output[output.index(WAKE):]
                    completed = wake_tail.find("\ncompleted: ")
                    if completed >= 0 and WAKE in wake_tail[completed:]:
                        woke = True
                        break
            elif kind == "state":
                assert frame["status"] not in ("failed", "killed"), frame
                if frame["status"] == "idle" and ready and rpc and TEXT in output and not idle:
                    idle = True
                    send({"type": "message", "text": "WAKE_THE_PANE_WORKER"})
            elif kind in ("eof", "reader_error"):
                raise AssertionError(frame)
        assert hello and ready and rpc and idle and woke, {
            "hello": hello, "ready": ready, "rpc": rpc, "idle": idle, "woke": woke,
            "model_calls": len(requests), "output_bytes": len(output)}
        assert any(MARKER in json.dumps(body) for body in requests), "No task model request"
        assert any("SMOKE_PARENT_ACK" in json.dumps(body) for body in requests), "RPC result not replayed to model"
        assert any("WAKE_THE_PANE_WORKER" in json.dumps(body.get("messages", [])) for body in requests), "Parent wake did not reach a new model request"
        assert not manifest_path.exists(), "Worker did not consume private manifest"
        send({"type": "shutdown"})
        process.wait(timeout=15)
        assert process.returncode == 0, f"Shutdown exit: {process.returncode}"
        report.update(passed=True, hello=True, ready=True, message_rpc=True, idle=True,
                      wake=True, wake_completed_event=True, shutdown_exit=process.returncode, manifest_consumed=True,
                      model_requests=len(requests), output_bytes=len(output))
    except Exception as error:
        report["error"] = str(error)
    finally:
        if process.poll() is None and connection is not None:
            try:
                connection.sendall(b'{"type":"shutdown"}\n')
            except OSError:
                pass
        if process.poll() is None:
            try:
                process.wait(timeout=1)
            except subprocess.TimeoutExpired:
                pass
        if process.poll() is None:
            os.killpg(process.pid, signal.SIGTERM)
            try:
                process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                os.killpg(process.pid, signal.SIGKILL)
                process.wait()
        if connection:
            connection.close()
        listener.close()
        server.shutdown()
        server.server_close()
        terminal_thread.join(timeout=1)
        os.close(master)
        import shutil
        shutil.rmtree(private)
        (root / "terminal.log").write_bytes(terminal)
        dump(root / "frames.json", frames)
        dump(root / "requests.json", requests)
        report["synthetic_terminal_permissions_approved"] = permission_answers
        report["external_http_attempts_rejected"] = rejected
        dump(root / "report.json", report)
    return report


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("binary", type=Path)
    parser.add_argument("--output", type=Path)
    parser.add_argument("--timeout", type=float, default=90)
    parser.add_argument("--test-harness", action="store_true",
                        help="run the ignored worker-engine test with explicit fixture storage")
    args = parser.parse_args()
    binary = args.binary.resolve()
    if not binary.is_file():
        parser.error(f"binary does not exist: {binary}")
    root = args.output.resolve() if args.output else Path(tempfile.mkdtemp(prefix="lingxi-worker-smoke-"))
    root.mkdir(parents=True, exist_ok=True)
    report = run(binary, root, args.timeout, args.test_harness)
    print(json.dumps(dict(report, evidence=str(root)), ensure_ascii=False))
    return 0 if report["passed"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
