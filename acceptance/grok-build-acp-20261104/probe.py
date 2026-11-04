#!/usr/bin/env python3
"""Probe Grok ACP without reading personal credentials or starting a task.

Only initialize is sent. Every process gets a temporary HOME/GROK_HOME, an
allowlisted environment, no inherited stdin, and no project as its cwd.
No raw protocol messages, credentials, or authorization URLs are printed.
"""
import argparse
import json
import os
from pathlib import Path
import selectors
import signal
import subprocess
import tempfile
import time


def probe(binary, timeout):
    with tempfile.TemporaryDirectory(prefix="lingxi-grok-acp-") as scratch:
        env = {
            key: value for key, value in os.environ.items()
            if key in {"PATH", "TMPDIR", "LANG", "LC_ALL", "HTTPS_PROXY",
                       "HTTP_PROXY", "NO_PROXY"}
        }
        env.update(HOME=scratch, GROK_HOME=scratch,
                   GROK_DISABLE_AUTOUPDATER="1", GROK_FEEDBACK_ENABLED="0",
                   RUST_LOG="off")
        process = subprocess.Popen(
            [str(binary), "--no-auto-update", "agent", "--no-leader", "stdio"],
            cwd=scratch, env=env, stdin=subprocess.PIPE,
            stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
            start_new_session=True,
        )
        report = {"isolated_home": True, "inherited_credentials": False,
                  "task_started": False, "initialize_received": False}
        try:
            request = {"jsonrpc": "2.0", "id": 1, "method": "initialize",
                       "params": {"protocolVersion": 1,
                                  "clientCapabilities": {},
                                  "clientInfo": {"name": "lingxi-compatibility-probe",
                                                 "version": "0.1.0"}}}
            process.stdin.write((json.dumps(request) + "\n").encode())
            process.stdin.flush()
            deadline = time.monotonic() + timeout
            buffer = b""
            with selectors.DefaultSelector() as selector:
                selector.register(process.stdout, selectors.EVENT_READ)
                while time.monotonic() < deadline:
                    if not selector.select(min(1, max(0, deadline - time.monotonic()))):
                        if process.poll() is not None:
                            break
                        continue
                    chunk = os.read(process.stdout.fileno(), 65536)
                    if not chunk:
                        break
                    buffer += chunk
                    if len(buffer) > 2_000_000:
                        raise RuntimeError("ACP response exceeded probe limit")
                    while b"\n" in buffer:
                        line, buffer = buffer.split(b"\n", 1)
                        response = json.loads(line)
                        if response.get("id") != 1 or "method" in response:
                            continue
                        if "error" in response:
                            report["rpc_error_code"] = response["error"].get("code")
                            return report
                        result = response.get("result", {})
                        report.update(
                            initialize_received=True,
                            protocol_version=result.get("protocolVersion"),
                            agent_version=result.get("_meta", {}).get("agentVersion"),
                            auth_methods=[{"id": method.get("id"),
                                           "name": method.get("name")}
                                          for method in result.get("authMethods", [])],
                            agent_capabilities=result.get("agentCapabilities", {}),
                        )
                        return report
                report["failure"] = "initialize unavailable before deadline"
                return report
        finally:
            # Terminate the owned process group even if the parent exited first.
            try:
                os.killpg(process.pid, signal.SIGTERM)
            except ProcessLookupError:
                pass
            try:
                process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                os.killpg(process.pid, signal.SIGKILL)
                process.wait(timeout=5)
            for stream in (process.stdin, process.stdout):
                stream.close()
            report["auth_file_created"] = (Path(scratch) / "auth.json").exists()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--timeout", type=float, default=30)
    args = parser.parse_args()
    if not args.binary.is_absolute() or not args.binary.is_file():
        parser.error("--binary must name an absolute Grok executable")
    if not 0 < args.timeout <= 60:
        parser.error("--timeout must be between 0 and 60 seconds")
    report = probe(args.binary, args.timeout)
    print(json.dumps(report, ensure_ascii=False, indent=2))
    return 0 if report["initialize_received"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
