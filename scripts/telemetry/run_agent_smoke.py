#!/usr/bin/env python3
"""Exercise the built CLI, real agent/tools and native Collector on loopback.

The model is a deterministic HTTP fixture, not a paid provider. No real account
credentials or settings are used. Captures are synthetic and stay in --output.
Requires Python 3.10+, the built CLI, and the pinned otelcol-contrib binary.
"""
from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import re
import signal
import socket
import subprocess
import tempfile
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.request import Request, ProxyHandler, build_opener

from verify_capture import flatten, records, validate

PRIVATE = "TELEMETRY_PRIVATE_CONTENT_CANARY_71029"
ANSWER = "TELEMETRY_AGENT_COMPLETED"
LOCAL_HTTP = build_opener(ProxyHandler({}))


def has_successful_tool_result(request: dict, tool_use_id: str = "telemetry_tool_1",
                               stdout: str = "TELEMETRY_TOOL_OK") -> bool:
    """Require actual successful output, never a command echoed in history.

    Anthropic carries tool results in user messages. Read adds numeric line
    prefixes and tools may append system reminders. Only exact output lines
    before those reminders count, never arbitrary substring matches.
    A denial can include the original command, so inspecting serialized request
    text (or just finding the tool ID) cannot prove that the tool executed.
    """
    messages = request.get("messages", [])
    if not isinstance(messages, list):
        return False
    for message in messages:
        if not isinstance(message, dict) or message.get("role") != "user":
            continue
        content = message.get("content")
        if not isinstance(content, list):
            continue
        for block in content:
            if (not isinstance(block, dict) or block.get("type") != "tool_result"
                    or block.get("tool_use_id") != tool_use_id
                    or block.get("is_error", False) is not False):
                continue
            output = block.get("content")
            if isinstance(output, str):
                texts = [output]
            elif isinstance(output, list):
                texts = [part["text"] for part in output
                         if isinstance(part, dict) and part.get("type") == "text"
                         and isinstance(part.get("text"), str)]
            else:
                continue
            for text in texts:
                output_text = text.partition("<system-reminder>")[0]
                for line in output_text.splitlines():
                    # Read's compact format is `1\ttext`; its alternate display
                    # format pads the number and uses an arrow separator.
                    line = re.sub(r"^[ \t]*[1-9][0-9]*(?:\t|→)", "", line)
                    if line.strip() == stdout:
                        return True
    return False


def port():
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        return sock.getsockname()[1]


def model_handler(requests, fail=False, fixture_file=None):
    class Model(BaseHTTPRequestHandler):
        def log_message(self, *_):
            pass

        def respond(self, value, status=200):
            payload = json.dumps(value).encode()
            self.send_response(status)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(payload)))
            self.end_headers()
            self.wfile.write(payload)

        def do_CONNECT(self):
            self.send_error(403)

        def do_GET(self):
            self.send_error(403)

        def do_POST(self):
            if self.path.split("?")[0] not in ("/v1/messages", "/v1/messages/count_tokens"):
                self.send_error(403)
                return
            body = json.loads(self.rfile.read(int(self.headers.get("Content-Length", "0"))))
            if "count_tokens" in self.path:
                self.respond({"input_tokens": 30})
                return
            requests.append(body)
            if fail:
                self.respond({"type": "error", "error": {
                    "type": "authentication_error", "message": PRIVATE}}, 401)
                return
            seen = "telemetry_tool_1" in json.dumps(body.get("messages", []))
            if seen:
                content = [{"type": "text", "text": ANSWER}]
                stop = "end_turn"
            else:
                content = [{"type": "tool_use", "id": "telemetry_tool_1", "name": "Read",
                            "input": {"file_path": str(fixture_file or "telemetry-fixture.txt")}}]
                stop = "tool_use"
            message = {"id": "msg_telemetry_fixture", "type": "message", "role": "assistant",
                       "model": body.get("model", "claude-sonnet-4-6"), "content": [],
                       "stop_reason": None, "stop_sequence": None,
                       "usage": {"input_tokens": 30, "output_tokens": 0}}
            if not body.get("stream"):
                self.respond(dict(message, content=content, stop_reason=stop,
                                  usage={"input_tokens": 30, "output_tokens": 12}))
                return
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
                        "usage": {"output_tokens": 12}}, {"type": "message_stop"}]
            payload = "".join(f"event: {e['type']}\ndata: {json.dumps(e)}\n\n" for e in events).encode()
            self.send_response(200)
            self.send_header("Content-Type", "text/event-stream")
            self.send_header("Content-Length", str(len(payload)))
            self.end_headers()
            self.wfile.write(payload)
    return Model


def receiver_handler(target, receipts):
    class Receiver(BaseHTTPRequestHandler):
        def log_message(self, *_):
            pass

        def do_POST(self):
            # The JS OTLP exporter streams chunked protobuf; Rust sends a fixed
            # length. Preserve both transports instead of accepting empty data.
            self.connection.settimeout(5)
            try:
                if self.headers.get("Transfer-Encoding", "").lower() == "chunked":
                    data = bytearray()
                    while True:
                        length = int(self.rfile.readline(128).split(b";", 1)[0].strip(), 16)
                        if length < 0 or len(data) + length > 4 * 1024 * 1024:
                            raise ValueError("OTLP fixture body exceeds limit")
                        if length == 0:
                            for _ in range(32):
                                if self.rfile.readline(8192) in (b"\r\n", b"\n"):
                                    break
                            else:
                                raise ValueError("Invalid chunk trailers")
                            break
                        chunk = self.rfile.read(length)
                        if len(chunk) != length or self.rfile.read(2) != b"\r\n":
                            raise ValueError("Incomplete chunk")
                        data.extend(chunk)
                    body = bytes(data)
                else:
                    length = int(self.headers.get("Content-Length", "0"))
                    if not 0 <= length <= 4 * 1024 * 1024:
                        raise ValueError("OTLP fixture body exceeds limit")
                    body = self.rfile.read(length)
                    if len(body) != length:
                        raise ValueError("Incomplete OTLP body")
            except (ValueError, OSError):
                self.send_error(400)
                return
            receipts.append((self.path, body))
            request = Request(target + self.path, data=body,
                              headers={"Content-Type": self.headers.get("Content-Type", "application/json")})
            try:
                with LOCAL_HTTP.open(request, timeout=5) as response:
                    result = response.read()
                    content_type = response.headers.get("Content-Type", "application/json")
                self.send_response(200)
                self.send_header("Content-Type", content_type)
                self.send_header("Content-Length", str(len(result)))
                self.end_headers()
                self.wfile.write(result)
            except Exception:
                self.send_error(503)
    return Receiver


def wait_health(address, process):
    deadline = time.monotonic() + 15
    while time.monotonic() < deadline:
        if process.poll() is not None:
            raise RuntimeError("Collector exited before readiness; see collector.log")
        try:
            with LOCAL_HTTP.open(address, timeout=0.25) as response:
                if response.status == 200:
                    return
        except OSError:
            time.sleep(0.1)
    raise RuntimeError("Collector readiness timed out")


def run_case(cli, collector, output, case):
    root = output / case
    root.mkdir(parents=True, exist_ok=False)
    for name in ("config", "cwd", "tmp", "capture"):
        (root / name).mkdir()
    fixture_file = (root / "cwd" / "telemetry-fixture.txt").resolve()
    fixture_file.write_text("TELEMETRY_TOOL_OK\n")
    # Exercise the existing explicit ephemeral credential-store path, without
    # accessing the user's signed credential broker or changing its policy.
    fixture_home = tempfile.TemporaryDirectory(prefix="lingxi-telemetry-", dir="/private/tmp")
    config_home = Path(fixture_home.name) / "config"
    config_home.mkdir()
    (config_home / "settings.json").write_text(json.dumps({
        "permissions": {"defaultMode": "default", "allow": ["Read"]},
    }) + "\n")
    grpc, http, health = port(), port(), port()
    config = Path(__file__).resolve().parents[2] / "deploy/telemetry/collector.yaml"
    yaml = config.read_text().replace(":4317", f":{grpc}").replace(":4318", f":{http}").replace(":13133", f":{health}")
    config_path = root / "collector.yaml"
    config_path.write_text(yaml)
    collector_env = dict(os.environ, LINGXI_OTEL_BIND_ADDRESS="127.0.0.1",
                         LINGXI_OTEL_OUTPUT_DIR=str(root / "capture"))
    model_requests, receipts = [], []
    model = ThreadingHTTPServer(("127.0.0.1", 0), model_handler(
        model_requests, case == "provider-error", fixture_file=fixture_file))
    receiver = ThreadingHTTPServer(("127.0.0.1", 0), receiver_handler(f"http://127.0.0.1:{http}", receipts))
    threading.Thread(target=model.serve_forever, daemon=True).start()
    threading.Thread(target=receiver.serve_forever, daemon=True).start()
    with (root / "collector.log").open("wb") as collector_log:
        proc = subprocess.Popen([str(collector), "--config", str(config_path)], env=collector_env,
                                stdout=collector_log, stderr=subprocess.STDOUT)
        try:
            wait_health(f"http://127.0.0.1:{health}", proc)
            model_url = f"http://127.0.0.1:{model.server_port}"
            endpoint = f"http://127.0.0.1:{port() if case == 'collector-down' else receiver.server_port}"
            env = {key: os.environ[key] for key in ("PATH", "HOME", "USER", "LANG") if key in os.environ}
            env.update({"RUST_BACKTRACE": "1", "RUST_MIN_STACK": "16777216", "TMPDIR": str(root / "tmp"), "LINGXI_CONFIG_DIR": str(config_home),
                        "CLAUDE_CONFIG_DIR": str(config_home), "ANTHROPIC_API_KEY": "fixture-only-key",
                        "ANTHROPIC_BASE_URL": model_url,
                        "LINGXI_API_BASE_URL": model_url, "DISABLE_AUTOUPDATER": "1",
                        "DISABLE_ERROR_REPORTING": "1", "ENABLE_TOOL_SEARCH": "false",
                        "HTTP_PROXY": model_url, "HTTPS_PROXY": model_url, "ALL_PROXY": model_url,
                        "NO_PROXY": "127.0.0.1,localhost", "LINGXI_ENABLE_TELEMETRY": "true",
                        "OTEL_SERVICE_NAME": "lingxi-cli-smoke", "OTEL_EXPORTER_OTLP_ENDPOINT": endpoint,
                        "OTEL_EXPORTER_OTLP_PROTOCOL": "http/json", "OTEL_METRICS_EXPORTER": "otlp",
                        "OTEL_LOGS_EXPORTER": "otlp", "OTEL_TRACES_EXPORTER": "otlp",
                        "OTEL_TRACES_SAMPLER": "always_on", "OTEL_METRIC_EXPORT_INTERVAL": "200",
                        "OTEL_EXPORTER_OTLP_TIMEOUT": "1000", "OTEL_BSP_SCHEDULE_DELAY": "200",
                        "OTEL_BLRP_SCHEDULE_DELAY": "200", "OTEL_LOG_USER_PROMPTS": "true",
                        "OTEL_LOG_TOOL_DETAILS": "true", "OTEL_RESOURCE_ATTRIBUTES": f"private={PRIVATE}"})
            if case == "disabled":
                env["LINGXI_ENABLE_TELEMETRY"] = "false"
            elif case == "do-not-track":
                env["DO_NOT_TRACK"] = "1"
            elif case == "disable-telemetry":
                env["DISABLE_TELEMETRY"] = "1"
            start = time.monotonic()
            result = subprocess.run([str(cli), "-p", PRIVATE, "--output-format", "json",
                                     "--model", "claude-sonnet-4-6", "--setting-sources", "user",
                                     "--strict-mcp-config", "--mcp-config", '{"mcpServers":{}}',
                                     "--tools", "Read", "--allowedTools", "Read", "--permission-mode", "dontAsk",
                                     "--system-prompt", "Local deterministic telemetry fixture."],
                                    cwd=root / "cwd", env=env, capture_output=True, timeout=60)
            elapsed = time.monotonic() - start
            (root / "stdout.txt").write_bytes(result.stdout)
            (root / "stderr.txt").write_bytes(result.stderr)
            # Retain only the synthetic tool results when diagnosing a fixture
            # failure; model prompts and request bodies are not persisted.
            tool_results = [block for request in model_requests
                            for message in request.get("messages", []) if message.get("role") == "user"
                            for block in (message.get("content") if isinstance(message.get("content"), list) else [])
                            if isinstance(block, dict) and block.get("type") == "tool_result"]
            (root / "tool-results.json").write_text(json.dumps(tool_results, indent=2) + "\n")
            if case == "provider-error":
                assert result.returncode != 0, "provider failure must reach the CLI result"
            else:
                assert result.returncode == 0 and ANSWER.encode() in result.stdout, "agent did not finish; inspect stdout/stderr"
                assert len(model_requests) >= 2, "tool result must be followed by another model request"
                assert has_successful_tool_result(model_requests[-1]), "successful tool result did not reach model"
            time.sleep(0.5)
        finally:
            proc.send_signal(signal.SIGTERM)
            try:
                proc.wait(timeout=10)
            except subprocess.TimeoutExpired:
                proc.kill()
                proc.wait()
            model.shutdown()
            receiver.shutdown()
            model.server_close()
            receiver.server_close()
            fixture_home.cleanup()
    for path, raw in receipts:
        assert PRIVATE.encode() not in raw, f"private content reached OTLP {path} before Collector filtering"
        assert b"fixture-only-key" not in raw, "credential reached OTLP"
    summary = {"case": case, "exit_code": result.returncode, "model_requests": len(model_requests),
               "otlp_requests": len(receipts), "elapsed_seconds": round(elapsed, 3)}
    if case in ("disabled", "do-not-track", "disable-telemetry", "collector-down"):
        assert not receipts, "disabled telemetry reached the receiver"
    elif case == "enabled":
        summary["capture"] = validate(root / "capture", "lingxi.headless.request", "lingxi.orchestrator.turn",
                                      "lingxi.llm.request", "lingxi.tool.execute", forbidden=[PRIVATE, "fixture-only-key"])
    else:
        assert receipts, "error case emitted no telemetry"
        spans = flatten(records(root / "capture", "traces"), "resourceSpans", "scopeSpans", "spans", None)
        assert any(span.get("name") == "lingxi.llm.request" and
                   span.get("status", {}).get("code") in (2, "STATUS_CODE_ERROR") for span in spans), \
            "provider failure must be recorded on the actual model request span"
    (root / "result.json").write_text(json.dumps(summary, indent=2) + "\n")
    return summary


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--cli", required=True, type=Path)
    parser.add_argument("--collector", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    cases = ["enabled", "disabled", "do-not-track", "disable-telemetry", "provider-error", "collector-down"]
    parser.add_argument("--cases", nargs="+", choices=cases, default=cases)
    args = parser.parse_args()
    summaries = [run_case(args.cli.resolve(), args.collector.resolve(), args.output.resolve(), case) for case in args.cases]
    print(json.dumps(summaries, indent=2))
