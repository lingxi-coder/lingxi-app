#!/usr/bin/env python3
"""Exercise Desktop host telemetry against a built bridge and native Collector.

Uses the actual Desktop SDK and bridge client with a deterministic local model.
No Electron window, paid model, account credential, or business settings are used.
The second real turn proves live disable after a race-free checkpoint handshake.
"""
from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
from queue import Empty, Queue
import shutil
import signal
import subprocess
import tempfile
import threading
import time
from http.server import ThreadingHTTPServer

from run_agent_smoke import (
    ANSWER, PRIVATE, has_successful_tool_result, model_handler, port, receiver_handler, wait_health,
)
from verify_capture import validate


def stop_process(process: subprocess.Popen | None, timeout: float = 10):
    if process is None or process.poll() is not None:
        return
    process.send_signal(signal.SIGTERM)
    try:
        process.wait(timeout=timeout)
    except subprocess.TimeoutExpired:
        process.kill()
        process.wait(timeout=5)


def wait_lockfile(directory: Path, process: subprocess.Popen) -> Path:
    deadline = time.monotonic() + 45
    while time.monotonic() < deadline:
        if process.poll() is not None:
            raise RuntimeError("Bridge exited before publishing a lockfile; see bridge.log")
        candidates = sorted(directory.glob("*.lock"))
        for path in candidates:
            try:
                value = json.loads(path.read_text())
                if isinstance(value, dict) and value.get("authToken"):
                    return path
            except (OSError, json.JSONDecodeError):
                pass
        time.sleep(0.1)
    raise RuntimeError("Bridge lockfile readiness timed out; see bridge.log")


def collect_stdout(process: subprocess.Popen, destination: Path, messages: Queue):
    assert process.stdout is not None
    with destination.open("w") as output:
        for line in process.stdout:
            output.write(line)
            output.flush()
            try:
                value = json.loads(line)
            except json.JSONDecodeError:
                continue
            if isinstance(value, dict):
                messages.put(value)


def run_fixture(bridge: Path, collector: Path, output: Path, node: str, loader: Path,
                config_home: Path) -> dict:
    repo = Path(__file__).resolve().parents[2]
    script = repo / "apps/electron/scripts/telemetry-bridge-smoke.ts"
    for path in (bridge, collector, loader, script):
        if not path.is_file():
            raise RuntimeError(f"Required executable/module does not exist: {path}")
    output.mkdir(parents=True, exist_ok=False)
    for name in ("cwd", "tmp", "capture", "bridge"):
        (output / name).mkdir()
    fixture_file = output / "cwd/telemetry-fixture.txt"
    fixture_file.write_text("TELEMETRY_TOOL_OK\n")
    fixture_file.chmod(0o400)
    # This scope grants only the deterministic fixture read; the Node driver
    # also rejects every unexpected permission request.
    (config_home / "settings.json").write_text(json.dumps({
        "permissions": {"defaultMode": "default", "allow": ["Read(./telemetry-fixture.txt)"]},
    }) + "\n")
    (config_home / "global.json").write_text('{"mcpServers":{}}\n')
    ports = set()
    while len(ports) < 3:
        ports.add(port())
    grpc, http, health = ports
    config = (repo / "deploy/telemetry/collector.yaml").read_text()
    config = config.replace(":4317", f":{grpc}").replace(":4318", f":{http}").replace(":13133", f":{health}")
    config_path = output / "collector.yaml"
    config_path.write_text(config)

    model_requests, receipts = [], []
    model = ThreadingHTTPServer(("127.0.0.1", 0), model_handler(model_requests, fixture_file=fixture_file))
    receiver = ThreadingHTTPServer(("127.0.0.1", 0), receiver_handler(f"http://127.0.0.1:{http}", receipts))
    threading.Thread(target=model.serve_forever, daemon=True).start()
    threading.Thread(target=receiver.serve_forever, daemon=True).start()
    model_url = f"http://127.0.0.1:{model.server_port}"
    endpoint = f"http://127.0.0.1:{receiver.server_port}"
    # HOME is inherited unchanged. Explicit application configuration owners and
    # an isolated cwd prevent this fixture from editing the user's settings.
    env = {key: os.environ[key] for key in ("PATH", "HOME", "USER", "LANG") if key in os.environ}
    env.update({
        "TMPDIR": str(output / "tmp"), "LINGXI_CONFIG_DIR": str(config_home),
        "CLAUDE_CONFIG_DIR": str(config_home), "ANTHROPIC_BASE_URL": model_url,
        "LINGXI_API_BASE_URL": model_url, "DISABLE_AUTOUPDATER": "1",
        "DISABLE_ERROR_REPORTING": "1", "ENABLE_TOOL_SEARCH": "false",
        "HTTP_PROXY": model_url, "HTTPS_PROXY": model_url, "ALL_PROXY": model_url,
        "NO_PROXY": "127.0.0.1,localhost", "LINGXI_ENABLE_TELEMETRY": "true",
        "OTEL_EXPORTER_OTLP_ENDPOINT": endpoint, "OTEL_EXPORTER_OTLP_PROTOCOL": "http/protobuf",
        "OTEL_METRICS_EXPORTER": "otlp", "OTEL_LOGS_EXPORTER": "otlp",
        "OTEL_TRACES_EXPORTER": "otlp", "OTEL_TRACES_SAMPLER": "always_on",
        "OTEL_METRIC_EXPORT_INTERVAL": "200", "OTEL_EXPORTER_OTLP_TIMEOUT": "1000",
        "OTEL_BSP_SCHEDULE_DELAY": "200", "OTEL_BLRP_SCHEDULE_DELAY": "200",
        "OTEL_LOG_USER_PROMPTS": "true", "OTEL_LOG_TOOL_DETAILS": "true",
        "OTEL_RESOURCE_ATTRIBUTES": f"private={PRIVATE}",
        "LINGXI_TELEMETRY_FIXTURE_FILE": str(fixture_file),
        "RUST_MIN_STACK": "16777216",
    })
    collector_env = dict(env, LINGXI_OTEL_BIND_ADDRESS="127.0.0.1",
                         LINGXI_OTEL_OUTPUT_DIR=str(output / "capture"))
    collector_process = bridge_process = node_process = None
    disabled_receipts = disabled_model_requests = None
    completed = None
    start = time.monotonic()
    try:
        with (output / "collector.log").open("wb") as collector_log, \
                (output / "bridge.log").open("wb") as bridge_log, \
                (output / "desktop-stderr.txt").open("w") as node_errors:
            collector_process = subprocess.Popen([str(collector), "--config", str(config_path)],
                env=collector_env, stdout=collector_log, stderr=subprocess.STDOUT)
            wait_health(f"http://127.0.0.1:{health}", collector_process)
            bridge_process = subprocess.Popen([
                str(bridge), "--cwd", str(output / "cwd"), "--model", "claude-sonnet-4-6",
                "--api-key-stdin", "--bridge-dir", str(output / "bridge"),
                "--session-config-home", str(config_home),
                "--session-global-config-path", str(config_home / "global.json"),
                "--permission-timeout-secs", "15",
            ], cwd=output / "cwd", env=dict(env, OTEL_SERVICE_NAME="lingxi-desktop-bridge"),
                stdin=subprocess.PIPE, stdout=bridge_log, stderr=subprocess.STDOUT)
            assert bridge_process.stdin is not None
            bridge_process.stdin.write(b"fixture-only-key\n")
            bridge_process.stdin.close()
            lockfile = wait_lockfile(output / "bridge", bridge_process)
            node_process = subprocess.Popen([
                node, "--import", str(loader), str(script), str(lockfile), PRIVATE,
                "--disable-and-repeat", "--checkpoint-stdin",
            ], cwd=repo / "apps/electron", env=env, stdin=subprocess.PIPE,
                stdout=subprocess.PIPE, stderr=node_errors, text=True, bufsize=1)
            messages: Queue = Queue()
            reader = threading.Thread(target=collect_stdout, args=(
                node_process, output / "desktop-stdout.jsonl", messages), daemon=True)
            reader.start()
            deadline = time.monotonic() + 150
            while time.monotonic() < deadline:
                try:
                    message = messages.get(timeout=0.1)
                except Empty:
                    if node_process.poll() is not None and not reader.is_alive():
                        break
                    continue
                if message.get("checkpoint") == "disabled":
                    if disabled_receipts is not None:
                        raise RuntimeError("Duplicate disable checkpoint")
                    disabled_receipts = len(receipts)
                    disabled_model_requests = len(model_requests)
                    if disabled_receipts == 0 or disabled_model_requests < 2:
                        raise RuntimeError("Enabled turn did not export and execute the model/tool cycle")
                    assert node_process.stdin is not None
                    node_process.stdin.write("continue\n")
                    node_process.stdin.flush()
                    node_process.stdin.close()
                if message.get("success") is True:
                    completed = message
                    break
            if completed is None:
                raise RuntimeError("Desktop driver did not finish; inspect desktop-stderr.txt and bridge.log")
            node_process.wait(timeout=10)
            reader.join(timeout=2)
            if node_process.returncode != 0:
                raise RuntimeError("Desktop driver returned an error")
            if not completed.get("reportingDisabled") or len(completed.get("roots", [])) != 2:
                raise RuntimeError("Missing two-turn/live-disable result")
            if not completed["roots"][0] or completed["roots"][1] is not None:
                raise RuntimeError("Disabled Desktop turn created a trace context")
            if disabled_model_requests is None or len(model_requests) <= disabled_model_requests:
                raise RuntimeError("No real model request occurred after disabling reporting")
            if not any(has_successful_tool_result(request) for request in model_requests):
                raise RuntimeError("Successful tool result did not reach the model")
            # Allow another interval for any erroneously active periodic exporter
            # before closing the bridge. Shutdown must also honor the revocation.
            time.sleep(1)
            stop_process(bridge_process)
            time.sleep(0.5)
    finally:
        stop_process(node_process)
        stop_process(bridge_process)
        stop_process(collector_process)
        model.shutdown()
        receiver.shutdown()
        model.server_close()
        receiver.server_close()

    if disabled_receipts is None or len(receipts) != disabled_receipts:
        raise RuntimeError("New OTLP request received after live-disable checkpoint")
    for _, raw in receipts:
        if PRIVATE.encode() in raw or b"fixture-only-key" in raw:
            raise RuntimeError("Private marker/credential reached OTLP before Collector filtering")
    capture = validate(output / "capture", "lingxi.desktop.request", "lingxi.orchestrator.turn",
                       "lingxi.llm.request", "lingxi.tool.execute", forbidden=[PRIVATE, "fixture-only-key"])
    summary = {"ok": True, "case": "desktop-enabled-and-live-disable",
               "driver": "DesktopTelemetry + BridgeClient against built bridge",
               "model_requests": len(model_requests), "model_requests_before_disable": disabled_model_requests,
               "otlp_requests": len(receipts), "otlp_requests_after_disable": len(receipts) - disabled_receipts,
               "expected_answer": ANSWER, "elapsed_seconds": round(time.monotonic() - start, 3),
               "capture": capture}
    (output / "result.json").write_text(json.dumps(summary, indent=2) + "\n")
    return summary


def run(bridge: Path, collector: Path, output: Path, node: str, loader: Path) -> dict:
    # The existing credential layer recognizes /private/tmp configuration roots
    # as ephemeral. Keep NativePreferred behavior and leave HOME unchanged.
    with tempfile.TemporaryDirectory(prefix="lingxi-telemetry-desktop-", dir="/private/tmp") as directory:
        return run_fixture(bridge, collector, output, node, loader, Path(directory))


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--bridge", required=True, type=Path)
    parser.add_argument("--collector", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--node", default=shutil.which("node") or "node")
    parser.add_argument("--tsx-loader", type=Path, default=Path(__file__).resolve().parents[2]
                        / "packages/bridge-client/node_modules/tsx/dist/loader.mjs")
    args = parser.parse_args()
    print(json.dumps(run(args.bridge.resolve(), args.collector.resolve(), args.output.resolve(),
                         args.node, args.tsx_loader.resolve()), indent=2))
