#!/usr/bin/env python3
"""Build the actual terminal broker in a small crate and exercise its PTY protocol.

This diagnostic does not build bridge-server or GStreamer. On Windows it runs
against a real PowerShell process in ConPTY; on Unix it uses /bin/sh. A clean
EOF acknowledgement reports completed cleanup actions, not a tree-reaped proof.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import queue
import re
import shutil
import subprocess
import sys
import tempfile
import threading
import time
import tomllib


ROOT = Path(__file__).resolve().parents[2]
SOURCE = ROOT / "apps/bridge-server/src/desktop_terminal.rs"
LOCK = ROOT / "Cargo.lock"


def sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def package(lock: dict, name: str) -> dict:
    matches = [entry for entry in lock["package"] if entry["name"] == name]
    if len(matches) != 1:
        raise ValueError(f"expected exactly one locked {name}")
    return matches[0]


def make_crate(directory: Path, lock: dict) -> None:
    pty = package(lock, "platform-pty")
    source = pty.get("source", "")
    match = re.fullmatch(
        r"git\+(https://github\.com/lingxi-coder/mobile-linux-runtime\.git)\?rev=([0-9a-f]{40})#\2",
        source,
    )
    if not match:
        raise ValueError("platform-pty must use the full Cargo-locked SDK revision")
    versions = {name: package(lock, name)["version"] for name in ("anyhow", "serde", "serde_json", "tokio")}
    manifest = f'''[package]
name = "terminal-eof-probe"
version = "0.1.0"
edition = "2021"
[dependencies]
anyhow = "={versions["anyhow"]}"
serde = {{ version = "={versions["serde"]}", features = ["derive"] }}
serde_json = "={versions["serde_json"]}"
tokio = {{ version = "={versions["tokio"]}", features = ["full"] }}
platform-pty = {{ git = "{match[1]}", rev = "{match[2]}" }}
'''
    (directory / "Cargo.toml").write_text(manifest)
    shutil.copyfile(LOCK, directory / "Cargo.lock")
    src = directory / "src"
    src.mkdir()
    # A Rust path attribute compiles the current production file itself.
    module_path = json.dumps(SOURCE.as_posix(), ensure_ascii=False)
    (src / "main.rs").write_text(
        f'#[path = {module_path}]\nmod desktop_terminal;\n'
        '#[tokio::main(flavor = "multi_thread", worker_threads = 2)]\n'
        'async fn main() -> anyhow::Result<()> { desktop_terminal::run().await }\n'
    )


def verify_graph(original: dict, probe_lock: Path) -> None:
    current = tomllib.loads(probe_lock.read_text())
    def identity(entry: dict) -> tuple:
        return entry["name"], entry["version"], entry.get("source"), entry.get("checksum")
    allowed = {identity(entry) for entry in original["package"]}
    added = [identity(entry) for entry in current["package"]
             if entry["name"] != "terminal-eof-probe" and identity(entry) not in allowed]
    if added:
        raise ValueError(f"diagnostic resolved dependencies not present in the product lock: {added}")


class Broker:
    def __init__(self, executable: Path, cwd: Path) -> None:
        allowed = {"PATH", "HOME", "USER", "LOGNAME", "SHELL", "TMPDIR", "TMP", "TEMP", "LANG",
                   "LC_ALL", "LC_CTYPE", "TZ", "SYSTEMROOT", "WINDIR", "COMSPEC", "PATHEXT",
                   "USERPROFILE", "APPDATA", "LOCALAPPDATA", "HOMEDRIVE", "HOMEPATH"}
        env = {key: value for key, value in os.environ.items() if key.upper() in allowed}
        home, appdata, localdata, temporary = (cwd / name for name in ("home", "appdata", "localdata", "tmp"))
        for path in (home, appdata, localdata, temporary):
            path.mkdir()
        env.update(HOME=str(home), USERPROFILE=str(home), APPDATA=str(appdata), LOCALAPPDATA=str(localdata),
                   TMPDIR=str(temporary), TMP=str(temporary), TEMP=str(temporary))
        if os.name == "nt":
            drive, tail = os.path.splitdrive(str(home))
            if not drive:
                raise ValueError("Windows probe home must have a drive")
            env.update(HOMEDRIVE=drive, HOMEPATH=tail or "\\")
            search_path = next((value for key, value in env.items() if key.upper() == "PATH"), None)
            shell = shutil.which("pwsh.exe", path=search_path) or shutil.which("powershell.exe", path=search_path)
            self.shell_kind = "powershell" if shell else "cmd"
        else:
            env["SHELL"] = "/bin/sh"
            self.shell_kind = "posix"
        self.cwd = cwd
        self.child = subprocess.Popen([str(executable), "--desktop-terminal"], stdin=subprocess.PIPE,
                                      stdout=subprocess.PIPE, stderr=subprocess.PIPE, env=env, bufsize=0)
        self.queue: queue.Queue[object] = queue.Queue()
        self.frames: list[dict] = []
        self.output = ""
        self.stderr = bytearray()
        self.stdout_thread = threading.Thread(target=self._read_stdout, daemon=True)
        self.stderr_thread = threading.Thread(target=self._read_stderr, daemon=True)
        self.stdout_thread.start()
        self.stderr_thread.start()

    def _read_stdout(self) -> None:
        try:
            assert self.child.stdout is not None
            for line in self.child.stdout:
                if not line.endswith(b"\n"):
                    raise ValueError("incomplete broker frame")
                frame = json.loads(line)
                if not isinstance(frame, dict):
                    raise ValueError("broker frame is not an object")
                self.queue.put(frame)
        except Exception as error:
            self.queue.put(error)
        finally:
            self.queue.put(None)

    def _read_stderr(self) -> None:
        assert self.child.stderr is not None
        while chunk := self.child.stderr.read(4096):
            self.stderr.extend(chunk)
            del self.stderr[:-16384]

    def send(self, request: dict) -> None:
        assert self.child.stdin is not None
        payload = memoryview((json.dumps(request) + "\n").encode())
        while payload:
            count = self.child.stdin.write(payload)
            if count is None or count <= 0:
                raise RuntimeError("broker input closed")
            payload = payload[count:]

    def until(self, label: str, condition, timeout: float = 30) -> None:
        deadline = time.monotonic() + timeout
        while not condition():
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                raise TimeoutError(f"timed out waiting for {label}")
            try:
                frame = self.queue.get(timeout=min(remaining, 0.1))
            except queue.Empty:
                continue
            if frame is None:
                raise RuntimeError(f"broker output closed before {label}")
            if isinstance(frame, Exception):
                raise frame
            assert isinstance(frame, dict)
            self.frames.append(frame)
            if frame.get("kind") == "output" and frame.get("terminalId") == "test":
                self.output = (self.output + str(frame.get("data", "")))[-16384:]

    def response(self, request_id: str) -> dict:
        self.until(f"response {request_id}", lambda: any(
            frame.get("kind") == "response" and frame.get("requestId") == request_id for frame in self.frames
        ))
        return next(frame for frame in self.frames
                    if frame.get("kind") == "response" and frame.get("requestId") == request_id)

    def command(self, request_id: str, kind: str, **fields) -> dict:
        self.send({"requestId": request_id, "terminalId": "test", "kind": kind, **fields})
        return self.response(request_id)

    def ready(self) -> None:
        create = {"cwd": str(self.cwd), "cols": 80, "rows": 24}
        response = self.command("create", "create", **create)
        if response.get("error") is not None:
            raise RuntimeError(f"terminal create failed: {response['error']}")
        if self.shell_kind == "powershell":
            command = "Write-Output ('__LX_READY_' + '51__')\r"
        elif self.shell_kind == "cmd":
            command = 'set "LXPROBE=51"\recho __LX_READY_%LXPROBE%__\r'
        else:
            command = "printf '__LX_READY_%s__\\n' 51\n"
        response = self.command("ready", "input", data=command)
        if response.get("error") is not None:
            raise RuntimeError(f"terminal input failed: {response['error']}")
        # The complete marker is constructed by the shell, not present in the
        # echoed command. This establishes that the real shell consumed input.
        self.until("shell readiness marker", lambda: "__LX_READY_51__" in self.output)

    def finish(self) -> None:
        assert self.child.stdin is not None
        self.child.stdin.close()
        self.until("EOF shutdown", lambda: any(
            frame.get("kind") == "shutdown" and frame.get("reason") == "eof" for frame in self.frames
        ))
        code = self.child.wait(timeout=30)
        self.stdout_thread.join(timeout=2)
        self.stderr_thread.join(timeout=2)
        if code != 0 or self.stdout_thread.is_alive():
            raise RuntimeError(f"broker did not close cleanly after EOF: exit={code}")
        while not self.queue.empty():
            frame = self.queue.get_nowait()
            if isinstance(frame, Exception):
                raise frame
            if isinstance(frame, dict):
                self.frames.append(frame)
        if sum(frame.get("kind") == "shutdown" and frame.get("reason") == "eof" for frame in self.frames) != 1:
            raise RuntimeError("expected exactly one real EOF shutdown frame")

    def close(self) -> None:
        if self.child.poll() is None:
            self.child.kill()
            self.child.wait(timeout=10)
        for stream in (self.child.stdin, self.child.stdout, self.child.stderr):
            if stream is not None:
                stream.close()


def run_case(executable: Path, parent: Path, mode: str) -> None:
    cwd = parent / mode
    cwd.mkdir()
    broker = Broker(executable, cwd)
    try:
        broker.ready()
        if mode == "exited":
            if broker.shell_kind == "powershell":
                command, tail = "Write-Output ('__LX_TAIL_' + '73__'); exit 0\r", "__LX_TAIL_73__"
            elif broker.shell_kind == "cmd":
                command, tail = "echo __LX_TAIL_%LXPROBE%__\rexit 0\r", "__LX_TAIL_51__"
            else:
                command, tail = "printf '__LX_TAIL_%s__\\n' 73; exit 0\n", "__LX_TAIL_73__"
            response = broker.command("exit", "input", data=command)
            if response.get("error") is not None:
                raise RuntimeError(f"exit input was rejected: {response['error']}")
            broker.until("real shell exit", lambda: any(
                frame.get("kind") == "exit" and frame.get("terminalId") == "test" for frame in broker.frames
            ))
            exit_frame = next(frame for frame in broker.frames if frame.get("kind") == "exit" and frame.get("terminalId") == "test")
            if exit_frame.get("exitCode") != 0:
                raise RuntimeError(f"unexpected shell exit status: {exit_frame}")
            broker.until("shell output tail", lambda: tail in broker.output)
            response = broker.command("dead", "input", data="\n" if broker.shell_kind == "posix" else "\r")
            if response.get("error") not in ("terminal has exited", "unknown terminal"):
                raise RuntimeError(f"broker accepted input after shell exit: {response}")
        else:
            response = broker.command("live", "input", data="\n" if broker.shell_kind == "posix" else "\r")
            if response.get("error") is not None or any(frame.get("kind") == "exit" for frame in broker.frames):
                raise RuntimeError(f"broker did not accept input before EOF: {response}")
        broker.finish()
        print(f"PASS {mode}: actual PTY broker EOF protocol")
    except Exception as error:
        raise RuntimeError(f"{mode}: {error}; broker output={broker.output!r}; "
                           f"stderr={broker.stderr.decode(errors='replace')!r}") from error
    finally:
        broker.close()


def run_node_tests(executable: Path) -> None:
    env = os.environ.copy()
    env["LINGXI_TERMINAL_TEST_BIN"] = str(executable)
    names = (
        "raw Rust broker acknowledges real stdin EOF after terminal cleanup",
        "raw Rust broker rejects input probe after shell exit",
    )
    command = [
        "node", "--import", "../../packages/bridge-client/node_modules/tsx/dist/loader.mjs",
        "--test", "--test-reporter=tap",
        "--test-name-pattern=^raw Rust broker (acknowledges real stdin EOF after terminal cleanup|rejects input probe after shell exit)$",
        "test/terminal-manager.integration.test.ts",
    ]
    result = subprocess.run(command, cwd=ROOT / "apps/electron", env=env, text=True, capture_output=True)
    print(result.stdout, end="")
    print(result.stderr, end="", file=sys.stderr)
    if result.returncode != 0:
        raise RuntimeError(f"Node broker tests failed with status {result.returncode}")
    lines = result.stdout.replace("\r", "").splitlines()
    for name in names:
        if sum(bool(re.fullmatch(r"ok [0-9]+ - " + re.escape(name), line)) for line in lines) != 1:
            raise RuntimeError(f"required Node broker test did not pass exactly once: {name}")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--offline", action="store_true", help="require cached Cargo dependencies")
    parser.add_argument("--with-node", action="store_true", help="also run the actual Node raw-broker tests")
    args = parser.parse_args()
    source_hash, lock_hash = sha256(SOURCE), sha256(LOCK)
    original = tomllib.loads(LOCK.read_text())
    with tempfile.TemporaryDirectory(prefix="terminal-eof-probe-") as temporary:
        directory = Path(temporary).resolve()
        make_crate(directory, original)
        env = os.environ.copy()
        toolchain = tomllib.loads((ROOT / "rust-toolchain.toml").read_text())["toolchain"]["channel"]
        env.update(CARGO_TARGET_DIR=str(directory / "target"), CARGO_INCREMENTAL="0",
                   CARGO_PROFILE_DEV_DEBUG="0", CARGO_BUILD_JOBS="1", RUSTUP_TOOLCHAIN=toolchain)
        command = ["cargo", "build", "--manifest-path", str(directory / "Cargo.toml")]
        if args.offline:
            command.append("--offline")
        subprocess.run(command, check=True, cwd=directory, env=env)
        verify_graph(original, directory / "Cargo.lock")
        if (sha256(SOURCE), sha256(LOCK)) != (source_hash, lock_hash):
            raise RuntimeError("source or product lock changed during the diagnostic build")
        executable = directory / "target/debug" / ("terminal-eof-probe.exe" if os.name == "nt" else "terminal-eof-probe")
        print(f"source sha256={source_hash}; Cargo.lock sha256={lock_hash}; platform={sys.platform}")
        run_case(executable, directory, "live")
        run_case(executable, directory, "exited")
        if args.with_node:
            run_node_tests(executable)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
