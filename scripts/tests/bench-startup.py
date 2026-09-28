#!/usr/bin/env python3
"""Measure LingXi CLI startup surfaces without requiring external services."""

from __future__ import annotations

import argparse
import json
import os
import pty
import select
import signal
import subprocess
import sys
import time
from dataclasses import asdict, dataclass
from pathlib import Path


@dataclass
class CaseResult:
    name: str
    argv: list[str]
    exit_code: int | None
    elapsed_ms: float
    first_byte_ms: float | None = None
    welcome_ms: float | None = None
    fallback_notices: int = 0
    timed_out: bool = False


def run_process(name: str, argv: list[str], cwd: Path, timeout: float) -> CaseResult:
    started = time.perf_counter()
    proc = subprocess.run(
        argv,
        cwd=cwd,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        timeout=timeout,
        check=False,
        text=True,
    )
    elapsed = (time.perf_counter() - started) * 1000.0
    output = proc.stdout + proc.stderr
    return CaseResult(
        name=name,
        argv=argv,
        exit_code=proc.returncode,
        elapsed_ms=elapsed,
        first_byte_ms=elapsed if output else None,
        fallback_notices=output.count("default model") + output.count("provider is not connected"),
    )


def run_tty_case(name: str, argv: list[str], cwd: Path, timeout: float) -> CaseResult:
    master_fd, slave_fd = pty.openpty()
    started = time.perf_counter()
    first_byte: float | None = None
    welcome: float | None = None
    output = bytearray()
    proc = subprocess.Popen(
        argv,
        cwd=cwd,
        stdin=slave_fd,
        stdout=slave_fd,
        stderr=slave_fd,
        start_new_session=True,
    )
    os.close(slave_fd)
    timed_out = False
    try:
        deadline = started + timeout
        while time.perf_counter() < deadline:
            readable, _, _ = select.select([master_fd], [], [], 0.05)
            if not readable:
                if proc.poll() is not None:
                    break
                continue
            try:
                chunk = os.read(master_fd, 8192)
            except OSError:
                break
            if not chunk:
                break
            now = time.perf_counter()
            if first_byte is None:
                first_byte = now
            output.extend(chunk)
            text = output.decode(errors="ignore")
            if welcome is None and "Welcome" in text:
                welcome = now
                break
        else:
            timed_out = True
    finally:
        if proc.poll() is None:
            try:
                os.killpg(proc.pid, signal.SIGTERM)
            except ProcessLookupError:
                pass
            try:
                proc.wait(timeout=1.0)
            except subprocess.TimeoutExpired:
                try:
                    os.killpg(proc.pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass
                proc.wait(timeout=1.0)
        os.close(master_fd)
    elapsed = (time.perf_counter() - started) * 1000.0
    text = output.decode(errors="ignore")
    return CaseResult(
        name=name,
        argv=argv,
        exit_code=proc.returncode,
        elapsed_ms=elapsed,
        first_byte_ms=(first_byte - started) * 1000.0 if first_byte is not None else None,
        welcome_ms=(welcome - started) * 1000.0 if welcome is not None else None,
        fallback_notices=text.count("provider is not connected"),
        timed_out=timed_out,
    )


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--bin", default="target/debug/lingxi-cli", help="lingxi-cli binary")
    parser.add_argument("--cwd", default=".", help="working directory for benchmarks")
    parser.add_argument("--timeout", type=float, default=8.0)
    parser.add_argument("--json", action="store_true", help="emit JSON only")
    args = parser.parse_args()

    cwd = Path(args.cwd).resolve()
    binary = str(Path(args.bin).resolve())
    cases = [
        run_process("version", [binary, "--version"], cwd, args.timeout),
        run_process("help", [binary, "--help"], cwd, args.timeout),
        run_tty_case("tui", [binary], cwd, args.timeout),
        run_tty_case("tui_bare", [binary, "--bare"], cwd, args.timeout),
        run_tty_case("tui_safe_mode", [binary, "--safe-mode"], cwd, args.timeout),
    ]
    payload = [asdict(case) for case in cases]
    if args.json:
        print(json.dumps(payload, indent=2, sort_keys=True))
        return 0
    for case in cases:
        print(
            f"{case.name}: elapsed={case.elapsed_ms:.1f}ms "
            f"first_byte={fmt(case.first_byte_ms)} "
            f"welcome={fmt(case.welcome_ms)} "
            f"fallback_notices={case.fallback_notices} "
            f"exit={case.exit_code}"
        )
    return 0


def fmt(value: float | None) -> str:
    return "n/a" if value is None else f"{value:.1f}ms"


if __name__ == "__main__":
    sys.exit(main())

