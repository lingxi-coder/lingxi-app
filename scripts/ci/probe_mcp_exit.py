#!/usr/bin/env python3
"""Run owned MCP CLI processes with bounded I/O and sanitized diagnostics.

The workload mirrors the process-level initialize/ping/tools-list/EOF test.
No configured user home, credentials, process table scan, or foreign PID is
used. A successful result covers these direct CLI processes and protocol frames;
it is not proof that unrelated descendants or the complete CI workspace passed.
"""

from __future__ import annotations

import argparse
import collections
import hashlib
import itertools
import json
import os
from pathlib import Path
import selectors
import shutil
import signal
import subprocess
import tempfile
import time


INPUT = (
    b'{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"test","version":"1"}}}\n'
    b'{"jsonrpc":"2.0","method":"notifications/initialized"}\n'
    b'{"jsonrpc":"2.0","id":2,"method":"ping"}\n'
    b'{"jsonrpc":"2.0","id":3,"method":"tools/list","params":{}}\n'
)
DEADLINE_MARKER = b"MCP runtime teardown is unverified: runtime tasks did not stop before the deadline"
UNVERIFIED_MARKERS = (
    b"MCP runtime teardown is unverified:",
    b"MCP stdio flag restoration is unverified:",
    b"LingXi MCP stdio server failed:",
)
MAX_STREAM = 4 * 1024 * 1024
MAX_THREADS = 64
MAX_SAMPLES = 16
SAMPLE_INTERVAL = 0.5
TERM_GRACE = 1.0
KILL_GRACE = 2.0
PIPE_GRACE = 2.0
THREAD_ROLES = {
    "lingxi-mcp-runt": "shutdown",
    "lingxi-cli-runt": "cli-runtime",
    "tokio-runtime-w": "tokio-worker",
    "lingxi-cli": "main",
}
WAIT_ROLES = {
    "futex_wait_queue": "futex", "futex_wait": "futex",
    "futex_wait_queue_me": "futex", "ep_poll": "epoll",
    "do_epoll_wait": "epoll", "do_poll": "poll", "do_sys_poll": "poll",
    "poll_schedule_timeout": "poll", "hrtimer_nanosleep": "timer",
    "schedule_hrtimeout_range_clock": "timer", "pipe_read": "pipe-read",
    "pipe_write": "pipe-write", "do_wait": "wait", "do_wait_intr": "wait",
    "wait_for_completion": "completion", "wait_woken": "wait",
    "0": "running-or-restricted",
}


def read_small(path: Path) -> str | None:
    try:
        with path.open("rb") as source:
            data = source.read(513)
        return data.decode("ascii").strip() if len(data) <= 512 else None
    except (OSError, UnicodeError):
        return None


def sample_owned_child(child: subprocess.Popen[bytes], elapsed: float) -> dict:
    # This process is our direct child and has not been waited/reaped. Even if
    # it exits during this read, its PID cannot be reused before we reap it.
    # Only that child's task directory is opened; no system PID scan occurs.
    task_root = Path("/proc") / str(child.pid) / "task"
    try:
        with os.scandir(task_root) as entries:
            tids = [entry.name for entry in itertools.islice(entries, MAX_THREADS + 1)]
    except OSError:
        return {"elapsed_ms": int(elapsed * 1000), "available": False}
    truncated = len(tids) > MAX_THREADS
    roles: collections.Counter[str] = collections.Counter()
    waits: collections.Counter[str] = collections.Counter()
    role_waits: dict[str, collections.Counter[str]] = {}
    for tid in tids[:MAX_THREADS]:
        if not tid.isascii() or not tid.isdigit():
            continue
        base = task_root / tid
        name = read_small(base / "comm")
        wait = read_small(base / "wchan")
        role = THREAD_ROLES.get(name, "other")
        wait_class = WAIT_ROLES.get(wait, "other" if wait else "unavailable")
        roles[role] += 1
        waits[wait_class] += 1
        role_waits.setdefault(role, collections.Counter())[wait_class] += 1
    return {"elapsed_ms": int(elapsed * 1000), "available": True,
            "thread_limit_reached": truncated, "roles": dict(roles), "waits": dict(waits),
            "role_waits": {role: dict(counts) for role, counts in role_waits.items()}}


def verify_frames(data: bytes, truncated: bool) -> tuple[bool, int]:
    if truncated or data.count(b"\n") > 16:
        return False, 0
    try:
        frames = [json.loads(line) for line in data.decode("utf-8").splitlines()]
        tools = frames[2]["result"]["tools"]
        valid = (
            len(frames) == 3
            and all(isinstance(frame, dict) and frame.get("jsonrpc") == "2.0" for frame in frames)
            and all(type(frame.get("id")) is int for frame in frames)
            and frames[0]["id"] == 1
            and frames[0]["result"]["protocolVersion"] == "2025-06-18"
            and frames[0]["result"]["serverInfo"]["name"] == "LingXi"
            and frames[1] == {"jsonrpc": "2.0", "id": 2, "result": {}}
            and frames[2]["id"] == 3
            and isinstance(tools, list) and bool(tools)
            and any(isinstance(tool, dict) and tool.get("name") == "Read" for tool in tools)
            and all(isinstance(tool, dict) and "inputSchema" in tool for tool in tools)
        )
        return valid, len(frames)
    except (UnicodeError, ValueError, IndexError, KeyError, TypeError):
        return False, 0


def child_environment(root: Path, loader_dir: str | None) -> dict[str, str]:
    # Deliberately construct an environment rather than inheriting host values.
    env = {
        "PATH": "/usr/bin:/bin", "HOME": str(root),
        "XDG_CONFIG_HOME": str(root / ".config"),
        "XDG_CACHE_HOME": str(root / ".cache"),
        "XDG_DATA_HOME": str(root / ".local/share"),
        "TMPDIR": str(root / "tmp"), "LINGXI_CONFIG_DIR": str(root / "config"),
        "ANTHROPIC_API_KEY": "sk-ant-test-mcp-serve", "DISABLE_AUTOUPDATER": "1",
        "RUST_MIN_STACK": "16777216", "LANG": "C.UTF-8",
        "GST_PLUGIN_PATH": "", "GST_PLUGIN_PATH_1_0": "",
        "GST_PLUGIN_SYSTEM_PATH": "", "GST_PLUGIN_SYSTEM_PATH_1_0": "",
        "GST_REGISTRY": str(root / "gst-registry.bin"),
        "GST_REGISTRY_1_0": str(root / "gst-registry.bin"),
    }
    if loader_dir is not None:
        env["LD_LIBRARY_PATH"] = loader_dir
    return env


def terminate_owned(child: subprocess.Popen[bytes]) -> bool:
    """Best-effort emergency cleanup; only signal the unreaped direct child."""
    if child.poll() is not None:
        return True
    try:
        child.terminate()
    except ProcessLookupError:
        pass
    try:
        child.wait(timeout=TERM_GRACE)
        return True
    except subprocess.TimeoutExpired:
        pass
    try:
        child.kill()
    except ProcessLookupError:
        pass
    try:
        child.wait(timeout=KILL_GRACE)
        return True
    except subprocess.TimeoutExpired:
        return False


def interrupted(_signal: int, _frame: object) -> None:
    raise SystemExit(143)


def run_probe(cli: Path, workers: int, timeout: float, loader_dir: str | None) -> dict:
    temp = Path(tempfile.mkdtemp(prefix="lingxi-owned-mcp-"))
    selector = selectors.DefaultSelector()
    records = []
    all_reaped = True
    started = time.monotonic()
    try:
        for index in range(workers):
            home = temp / f"case-{index}"
            for part in ("config", "project", "tmp"):
                (home / part).mkdir(parents=True, exist_ok=True)
            process = subprocess.Popen(
                [str(cli), "mcp", "serve"], cwd=home / "project",
                env=child_environment(home, loader_dir),
                stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                close_fds=True,
            )
            record = {"process": process, "started": time.monotonic(),
                      "deadline": time.monotonic() + timeout, "next_sample": 0.0,
                      "samples": collections.deque(maxlen=MAX_SAMPLES),
                      "shutdown_observed": False,
                      "out": bytearray(), "err": bytearray(),
                      "truncated": set(), "streams": 2, "exit_seen": None,
                      "stop_stage": None, "stop_deadline": None,
                      "timed_out": False, "unreaped": False, "pipe_lingered": False}
            records.append(record)
            for name, stream in (("out", process.stdout), ("err", process.stderr)):
                assert stream is not None
                os.set_blocking(stream.fileno(), False)
                selector.register(stream, selectors.EVENT_READ, (record, name))

        # Start all owned processes before supplying their identical workload.
        for record in records:
            process = record["process"]
            assert process.stdin is not None
            try:
                process.stdin.write(INPUT)
                process.stdin.close()  # The exact EOF boundary being tested.
            except BrokenPipeError:
                process.stdin.close()

        while True:
            now = time.monotonic()
            for record in records:
                child = record["process"]
                if child.returncode is None and not record["unreaped"]:
                    # Sampling precedes poll, so the owned PID cannot have been
                    # reaped and reused while /proc is being inspected.
                    if now >= record["next_sample"]:
                        sample = sample_owned_child(child, now - record["started"])
                        record["samples"].append(sample)
                        record["shutdown_observed"] |= bool(sample.get("roles", {}).get("shutdown", 0))
                        record["next_sample"] = now + SAMPLE_INTERVAL
                    status = child.poll()
                    if status is not None:
                        record["exit_seen"] = now
                    elif record["stop_stage"] is None and now >= record["deadline"]:
                        record["timed_out"] = True
                        try:
                            child.terminate()
                        except ProcessLookupError:
                            pass
                        record["stop_stage"] = "term"
                        record["stop_deadline"] = now + TERM_GRACE
                    elif record["stop_stage"] == "term" and now >= record["stop_deadline"]:
                        try:
                            child.kill()
                        except ProcessLookupError:
                            pass
                        record["stop_stage"] = "kill"
                        record["stop_deadline"] = now + KILL_GRACE
                    elif record["stop_stage"] == "kill" and now >= record["stop_deadline"]:
                        record["unreaped"] = True
                        all_reaped = False
                if record["streams"] and (record["unreaped"] or (
                    record["exit_seen"] is not None and now - record["exit_seen"] >= PIPE_GRACE
                )):
                    # An inherited pipe may remain open after our direct child
                    # exits. Do not infer descendant cleanup from that exit.
                    record["pipe_lingered"] = True
                    for key in list(selector.get_map().values()):
                        if key.data[0] is record:
                            selector.unregister(key.fileobj)
                            key.fileobj.close()
                            record["streams"] -= 1
            if all((r["process"].returncode is not None or r["unreaped"]) and r["streams"] == 0 for r in records):
                break
            if all(r["unreaped"] for r in records):
                break
            for key, _ in selector.select(0.05):
                record, name = key.data
                try:
                    chunk = os.read(key.fd, 65536)
                except BlockingIOError:
                    continue
                if not chunk:
                    selector.unregister(key.fileobj)
                    key.fileobj.close()
                    record["streams"] -= 1
                else:
                    remaining = MAX_STREAM - len(record[name])
                    record[name].extend(chunk[:max(remaining, 0)])
                    if len(chunk) > remaining:
                        record["truncated"].add(name)

        cases = []
        for record in records:
            process = record["process"]
            stdout = bytes(record["out"])
            stderr = bytes(record["err"])
            frame_ok, frame_count = verify_frames(stdout, "out" in record["truncated"])
            reaped = process.returncode is not None and not record["unreaped"]
            unverified = any(marker in stderr for marker in UNVERIFIED_MARKERS)
            case_ok = (reaped and process.returncode == 0 and frame_ok
                       and not record["timed_out"] and not record["truncated"]
                       and not record["pipe_lingered"] and not unverified)
            samples = list(record["samples"])
            cases.append({
                "pid": process.pid, "exit_code": process.returncode,
                "exit_elapsed_ms": (int((record["exit_seen"] - record["started"]) * 1000)
                                    if record["exit_seen"] is not None else None),
                "direct_child_reaped": reaped, "deadline_reached": record["timed_out"],
                "signal_stage": record["stop_stage"], "pipe_open_after_exit": record["pipe_lingered"],
                "protocol_ok": frame_ok, "frame_count": frame_count,
                "stdout_bytes_captured": len(stdout), "stderr_bytes_captured": len(stderr),
                "streams_truncated": sorted(record["truncated"]),
                "runtime_deadline_marker_observed": DEADLINE_MARKER in stderr,
                "unverified_marker_observed": unverified,
                "stderr_sha256": hashlib.sha256(stderr).hexdigest(),
                "shutdown_thread_observed": record["shutdown_observed"],
                "thread_samples": samples, "ok": case_ok,
            })
        return {"workers": workers, "timeout_seconds": timeout,
                "elapsed_ms": int((time.monotonic() - started) * 1000),
                "runtime_deadline_count": sum(c["runtime_deadline_marker_observed"] for c in cases),
                "all_direct_children_reaped": all(c["direct_child_reaped"] for c in cases),
                "temporary_state_preserved": not all_reaped,
                "cases": cases, "ok": bool(cases) and all(c["ok"] for c in cases)}
    finally:
        selector.close()
        for record in records:
            child = record["process"]
            if child.returncode is None and not record["unreaped"]:
                all_reaped = terminate_owned(child) and all_reaped
            for stream in (child.stdin, child.stdout, child.stderr):
                if stream is not None and not stream.closed:
                    stream.close()
        # Preserve temporary state when direct-child reaping is unconfirmed.
        if all_reaped:
            shutil.rmtree(temp)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--cli", type=Path, required=True)
    parser.add_argument("--workers", type=int, default=1)
    parser.add_argument("--timeout", type=float, default=45)
    parser.add_argument("--loader-dir", type=Path)
    parser.add_argument("--report", type=Path, required=True)
    args = parser.parse_args()
    cli = args.cli.resolve(strict=True)
    if not cli.is_file() or not os.access(cli, os.X_OK):
        parser.error("--cli must be an executable regular file")
    if not 1 <= args.workers <= 8 or not 1 <= args.timeout <= 120:
        parser.error("workers must be 1..8 and timeout must be 1..120 seconds")
    loader = None
    if args.loader_dir is not None:
        loader_path = args.loader_dir.resolve(strict=True)
        if not loader_path.is_dir():
            parser.error("--loader-dir must be a directory")
        loader = str(loader_path)
    # Permit normal CI cancellation to execute the owned-child cleanup path.
    signal.signal(signal.SIGTERM, interrupted)
    result = run_probe(cli, args.workers, args.timeout, loader)
    digest = hashlib.sha256()
    with cli.open("rb") as binary:
        for chunk in iter(lambda: binary.read(1024 * 1024), b""):
            digest.update(chunk)
    result["binary_sha256"] = digest.hexdigest()
    result["scope"] = "direct owned CLI processes; single-package Cargo feature graph"
    args.report.parent.mkdir(parents=True, exist_ok=True)
    args.report.write_text(json.dumps(result, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    print(json.dumps(result, sort_keys=True))
    return 0 if result["ok"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
