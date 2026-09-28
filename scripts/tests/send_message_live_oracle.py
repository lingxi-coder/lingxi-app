#!/usr/bin/env python3
"""Capture SendMessage's four wire schemas from a local Claude Code binary.

Uses a fresh HOME/config, a fake API key, and a loopback mock; all proxy
variables target that mock, which refuses external connections. No real model
requests are made. --bare cannot be used: it removes SendMessage entirely.
Raw full request captures remain under --output and are not repository fixtures.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import subprocess
import threading
from http.server import ThreadingHTTPServer
from pathlib import Path

from compact_live_oracle import dump, isolated_env, make_handler


def capture(binary: Path, output: Path) -> dict:
    fixture = {
        "version": "2.1.263",
        "binary_sha256": hashlib.sha256(binary.read_bytes()).hexdigest(),
        "source": "Real binary requests to a loopback mock API, with isolated HOME/config and a fake API key. ENABLE_TOOL_SEARCH=false.",
        "cases": [],
    }
    for cross, teams in [(False, False), (False, True), (True, False), (True, True)]:
        root = output / f"{int(cross)}-{int(teams)}"
        root.mkdir(parents=True, exist_ok=False)
        for folder in ("config", "tmp", "work"):
            (root / folder).mkdir()
        requests = []
        server = ThreadingHTTPServer(("127.0.0.1", 0), make_handler(root, "success", requests))
        threading.Thread(target=server.serve_forever, daemon=True).start()
        env = isolated_env(root, f"http://127.0.0.1:{server.server_port}")
        env.update({
            "HOME": str(root),
            "CLAUDE_CODE_HARBOR_KITE": str(int(cross)),
            "CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS": str(int(teams)),
            "ENABLE_TOOL_SEARCH": "false",
        })
        try:
            result = subprocess.run([
                str(binary), "-p", "schema capture", "--output-format", "json",
                "--model", "claude-sonnet-4-6", "--setting-sources", "",
                "--strict-mcp-config", "--mcp-config", '{"mcpServers":{}}',
                "--tools", "default", "--system-prompt", "Offline schema capture; reply briefly.",
            ], cwd=root / "work", env=env, capture_output=True, text=True, timeout=35, check=True)
            (root / "stdout.txt").write_text(result.stdout)
            (root / "stderr.txt").write_text(result.stderr)
            assert len(requests) == 1, "Expected exactly one mock API request"
            tool = next(tool for tool in requests[0]["body"]["tools"] if tool.get("name") == "SendMessage")
            fixture["cases"].append({
                "cross_session": cross, "teams": teams,
                "request_sha256": hashlib.sha256((root / "request-01.json").read_bytes()).hexdigest(),
                "input_schema_json": json.dumps(tool["input_schema"], ensure_ascii=False, separators=(",", ":")),
                "prompt": tool["description"],
            })
        finally:
            server.shutdown()
            server.server_close()
    return fixture


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("binary", type=Path)
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args()
    fixture = capture(args.binary.resolve(), args.output.resolve())
    dump(args.output / "send_message_2_1_263_wire.json", fixture)
    print(f"Captured {len(fixture['cases'])} schema/prompt pairs under {args.output}")
