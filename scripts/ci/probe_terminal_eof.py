#!/usr/bin/env python3
"""Build the actual terminal broker in a small crate and exercise its PTY protocol.

This diagnostic does not build bridge-server or GStreamer. On Windows it runs
against a real PowerShell process in ConPTY; on Unix it uses /bin/sh. A clean
EOF acknowledgement reports completed cleanup actions, not a tree-reaped proof.
"""

from __future__ import annotations

import argparse
import hashlib
import io
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


def make_crate(directory: Path, lock: dict, with_rust_tests: bool = False) -> None:
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
    if with_rust_tests:
        tempfile_version = package(lock, "tempfile")["version"]
        uuid_version = package(lock, "uuid")["version"]
        manifest += f'''[dev-dependencies]
tempfile = "={tempfile_version}"
uuid = {{ version = "={uuid_version}", features = ["v4"] }}
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


def resolve_test_graph(directory: Path, original: dict, env: dict, offline: bool) -> None:
    # The copied product lock does not contain this temporary root crate.
    # Resolve it without running build scripts, verify every selected package,
    # then run build/test with --locked so the verified graph cannot drift.
    rustc = subprocess.run(["rustc", "-vV"], cwd=directory, env=env, text=True,
                           capture_output=True, check=True)
    host = next((line.removeprefix("host: ").strip() for line in rustc.stdout.splitlines()
                 if line.startswith("host: ")), "")
    if not host:
        raise RuntimeError("rustc did not report its host target")
    command = ["cargo", "metadata", "--manifest-path", str(directory / "Cargo.toml"),
               "--format-version", "1", "--filter-platform", host]
    if offline:
        command.append("--offline")
    subprocess.run(command, cwd=directory, env=env, stdout=subprocess.DEVNULL, check=True)
    verify_graph(original, directory / "Cargo.lock")


def run_rust_tests(directory: Path, env: dict, offline: bool) -> None:
    command = ["cargo", "test", "--manifest-path", str(directory / "Cargo.toml"), "--locked"]
    if offline:
        command.append("--offline")
    command.extend(["--", "--test-threads=1", "--color", "never"])
    result = subprocess.run(command, cwd=directory, env=env, text=True, capture_output=True)
    print(result.stdout, end="")
    print(result.stderr, end="", file=sys.stderr)
    if result.returncode != 0:
        raise RuntimeError(f"Rust terminal tests failed with status {result.returncode}")
    names = [
        "exit_does_not_wait_for_pty_eof_and_late_output_is_preserved",
        "output_eof_does_not_substitute_for_child_exit",
        "lost_or_ambiguous_exit_stays_unknown_and_keeps_output",
        "disconnected_parent_does_not_fabricate_exit",
        "full_output_queue_does_not_block_unknown_admission_or_leak_after_close",
        "exit_is_not_evictable_before_notification_is_queued",
        "shell_selection_fields_are_rejected_outside_create",
        "stdin_reader_distinguishes_eof_error_and_receiver_close",
        "eof_ack_requires_confirmed_clean_input_and_not_signal_shutdown",
        "draining_output_handles_a_large_response_burst",
        "cleanup_task_panic_is_reported_after_other_actions_finish",
        "unknown_blocks_broker_input_even_while_notification_is_backpressured",
        "unknown_is_not_evicted_by_sdk_exit_flag_and_rejects_operations",
    ]
    if os.name == "nt":
        names.extend([
            "unpackaged_windows_shell_search_preserves_drive_relative_paths",
            "unpackaged_windows_shell_search_resolves_current_drive_relative_path",
            "packaged_windows_fallback_resolves_original_cwd_and_user_path",
            "packaged_windows_fallback_preserves_drive_relative_semantics",
            "windows_accepts_only_an_absolute_existing_expected_shell_file",
        ])
    else:
        names.append("blocked_output_does_not_prevent_real_close_or_stdin_eof_cleanup")
    lines = result.stdout.replace("\r", "").splitlines()
    for name in names:
        expected = f"test desktop_terminal::tests::{name} ... ok"
        if lines.count(expected) != 1:
            raise RuntimeError(f"required Rust terminal test did not pass exactly once: {name}")


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
        assert self.child.stdout is not None
        self.stdout_stream = io.BufferedReader(self.child.stdout)
        self.queue: queue.Queue[object] = queue.Queue()
        self.frames: list[dict] = []
        self.output = ""
        self.stderr = bytearray()
        self.stdout_gate = threading.Event()
        self.stdout_gate.set()
        self.stdout_paused = threading.Event()
        self.stdout_thread = threading.Thread(target=self._read_stdout, daemon=True)
        self.stderr_thread = threading.Thread(target=self._read_stderr, daemon=True)
        self.stdout_thread.start()
        self.stderr_thread.start()

    def _read_stdout(self) -> None:
        try:
            while True:
                if not self.stdout_gate.is_set():
                    self.stdout_paused.set()
                    self.stdout_gate.wait()
                    self.stdout_paused.clear()
                line = self.stdout_stream.readline()
                if not line:
                    break
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
        payload = memoryview((json.dumps(request, separators=(",", ":")) + "\n").encode())
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
        if not self.child.stdin.closed:
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
        self.stdout_gate.set()
        if self.child.poll() is None:
            self.child.kill()
            self.child.wait(timeout=10)
        for stream in (self.child.stdin, self.stdout_stream, self.child.stderr):
            if stream is not None:
                stream.close()

    def pause_stdout(self) -> None:
        # Wake a reader that was already blocked in readline. The event is set
        # only after the reader reaches the gate and cannot read more bytes.
        self.stdout_gate.clear()
        self.send({"requestId": "pause-barrier", "terminalId": "missing", "kind": "resize", "cols": 80, "rows": 24})
        if not self.stdout_paused.wait(timeout=10):
            raise RuntimeError("stdout reader did not reach the pause barrier")

    def resume_stdout(self) -> None:
        self.stdout_gate.set()


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


class WindowsNativeProbe:
    """Use OS pipe and process handles; never signal a numeric PID."""

    def __init__(self, broker: Broker) -> None:
        if os.name != "nt":
            raise RuntimeError("Windows native backpressure probe requires Windows")
        import ctypes
        import msvcrt
        from ctypes import wintypes

        self.ctypes = ctypes
        self.kernel = ctypes.WinDLL("kernel32", use_last_error=True)
        self.kernel.GetNamedPipeInfo.argtypes = [wintypes.HANDLE, ctypes.POINTER(wintypes.DWORD),
                                                 ctypes.POINTER(wintypes.DWORD), ctypes.POINTER(wintypes.DWORD),
                                                 ctypes.POINTER(wintypes.DWORD)]
        self.kernel.GetNamedPipeInfo.restype = wintypes.BOOL
        self.kernel.PeekNamedPipe.argtypes = [wintypes.HANDLE, ctypes.c_void_p, wintypes.DWORD,
                                              ctypes.POINTER(wintypes.DWORD), ctypes.POINTER(wintypes.DWORD),
                                              ctypes.POINTER(wintypes.DWORD)]
        self.kernel.PeekNamedPipe.restype = wintypes.BOOL
        self.kernel.OpenProcess.argtypes = [wintypes.DWORD, wintypes.BOOL, wintypes.DWORD]
        self.kernel.OpenProcess.restype = wintypes.HANDLE
        self.kernel.WaitForSingleObject.argtypes = [wintypes.HANDLE, wintypes.DWORD]
        self.kernel.WaitForSingleObject.restype = wintypes.DWORD
        self.kernel.GetExitCodeProcess.argtypes = [wintypes.HANDLE, ctypes.POINTER(wintypes.DWORD)]
        self.kernel.GetExitCodeProcess.restype = wintypes.BOOL
        self.kernel.QueryFullProcessImageNameW.argtypes = [wintypes.HANDLE, wintypes.DWORD, wintypes.LPWSTR,
                                                           ctypes.POINTER(wintypes.DWORD)]
        self.kernel.QueryFullProcessImageNameW.restype = wintypes.BOOL
        self.kernel.CloseHandle.argtypes = [wintypes.HANDLE]
        self.kernel.CloseHandle.restype = wintypes.BOOL
        assert broker.child.stdout is not None
        self.pipe = wintypes.HANDLE(msvcrt.get_osfhandle(broker.child.stdout.fileno()))
        self.dword = wintypes.DWORD

    def _check(self, result: object, operation: str) -> None:
        if not result:
            raise RuntimeError(f"{operation} failed: {self.ctypes.WinError(self.ctypes.get_last_error())}")

    def capacity(self) -> int:
        outgoing, incoming = self.dword(), self.dword()
        self._check(self.kernel.GetNamedPipeInfo(self.pipe, None, self.ctypes.byref(outgoing),
                                                self.ctypes.byref(incoming), None), "GetNamedPipeInfo")
        capacity = max(outgoing.value, incoming.value)
        if capacity == 0:
            raise RuntimeError("pipe buffer capacity is unspecified; saturation cannot be verified")
        return capacity

    def available(self) -> int:
        available = self.dword()
        self._check(self.kernel.PeekNamedPipe(self.pipe, None, 0, None,
                                              self.ctypes.byref(available), None), "PeekNamedPipe")
        return available.value

    def open_shell(self, pid: int):
        # SYNCHRONIZE | PROCESS_QUERY_LIMITED_INFORMATION. The returned handle
        # keeps referring to the same process even if its numeric PID is reused.
        handle = self.kernel.OpenProcess(0x00100000 | 0x1000, False, pid)
        self._check(handle, "OpenProcess")
        try:
            size = self.dword(32768)
            path = self.ctypes.create_unicode_buffer(size.value)
            self._check(self.kernel.QueryFullProcessImageNameW(handle, 0, path, self.ctypes.byref(size)),
                        "QueryFullProcessImageNameW")
            if os.path.basename(path.value).lower() not in ("pwsh.exe", "powershell.exe"):
                raise RuntimeError(f"PID marker did not resolve to the PowerShell image: {path.value!r}")
            if self.wait(handle, 0):
                raise RuntimeError("shell exited before its native handle was acquired")
            return handle
        except BaseException:
            self.kernel.CloseHandle(handle)
            raise

    def wait(self, handle, timeout_ms: int) -> bool:
        result = self.kernel.WaitForSingleObject(handle, timeout_ms)
        if result == 0:  # WAIT_OBJECT_0: the same process object is signaled.
            return True
        if result == 258:  # WAIT_TIMEOUT is a failure to observe exit, not cleanup proof.
            return False
        raise RuntimeError(f"WaitForSingleObject failed: {self.ctypes.WinError(self.ctypes.get_last_error())}")

    def exit_code(self, handle) -> int:
        code = self.dword()
        self._check(self.kernel.GetExitCodeProcess(handle, self.ctypes.byref(code)), "GetExitCodeProcess")
        return code.value

    def close(self, handle) -> None:
        self._check(self.kernel.CloseHandle(handle), "CloseHandle")


def run_windows_backpressure_case(executable: Path, parent: Path, mode: str) -> None:
    cwd = parent / f"windows-blocked-{mode}"
    cwd.mkdir()
    broker = Broker(executable, cwd)
    native = None
    handle = None
    writer = None
    try:
        native = WindowsNativeProbe(broker)
        broker.ready()
        if broker.shell_kind != "powershell":
            raise RuntimeError("PowerShell is required for the native process-handle diagnostic")
        response = broker.command("pid", "input", data="Write-Output ('__LX_PID_' + $PID + '__')\r")
        if response.get("error") is not None:
            raise RuntimeError(f"shell PID request failed: {response['error']}")
        marker = re.compile(r"__LX_PID_(\d+)__")
        broker.until("PowerShell PID marker", lambda: bool(marker.search(broker.output)))
        match = marker.search(broker.output)
        assert match is not None
        handle = native.open_shell(int(match.group(1)))
        # Ensure the PID marker and the acquired handle still correspond to
        # the interactive shell: the exact nonce is assembled by PowerShell,
        # so terminal echo alone cannot satisfy this handshake.
        nonce = os.urandom(12).hex()
        identity_marker = f"__LX_HANDLE_{nonce}__"
        identity_command = f"Write-Output ('__LX_HANDLE_' + '{nonce}' + '__')\r"
        response = broker.command("identity", "input", data=identity_command)
        if response.get("error") is not None:
            raise RuntimeError(f"shell identity handshake failed: {response['error']}")
        broker.until("shell handle identity marker", lambda: identity_marker in broker.output)
        if native.wait(handle, 0):
            raise RuntimeError("shell handle exited during identity handshake")

        # The shell reports readiness, then waits on a file. Only after the
        # reader has acknowledged its pause do we release it to flood stdout.
        release = cwd / "release-flood"
        quoted = str(release).replace("'", "''")
        flood = ("Write-Output ('__LX_' + 'FLOOD_ARMED__'); "
                 f"while (-not (Test-Path -LiteralPath '{quoted}')) {{ Start-Sleep -Milliseconds 10 }}; "
                 "$chunk='X'*4096; while ($true) { [Console]::Write($chunk) }\r")
        response = broker.command("arm", "input", data=flood)
        if response.get("error") is not None:
            raise RuntimeError(f"flood input failed: {response['error']}")
        broker.until("flood readiness marker", lambda: "__LX_FLOOD_ARMED__" in broker.output)
        broker.pause_stdout()
        if not broker.stdout_paused.is_set():
            raise RuntimeError("stdout pause handshake was lost")
        capacity = native.capacity()
        release.write_text("go", encoding="utf-8")
        deadline = time.monotonic() + 15
        while True:
            if broker.child.poll() is not None:
                raise RuntimeError("broker exited before stdout saturation")
            if native.wait(handle, 0):
                raise RuntimeError("shell exited before stdout saturation")
            available = native.available()
            if available >= capacity:
                break
            if time.monotonic() >= deadline:
                raise RuntimeError(f"stdout pipe did not saturate: available={available}, capacity={capacity}")
            time.sleep(0.02)

        failures: list[BaseException] = []
        complete = threading.Event()
        def issue_requests() -> None:
            try:
                for index in range(40):
                    broker.send({"requestId": f"blocked-{index}", "terminalId": "missing", "kind": "resize", "cols": 80, "rows": 24})
                if mode == "close":
                    broker.send({"requestId": "blocked-close", "terminalId": "test", "kind": "close"})
                else:
                    assert broker.child.stdin is not None
                    broker.child.stdin.close()
            except BaseException as error:
                failures.append(error)
            finally:
                complete.set()
        writer = threading.Thread(target=issue_requests, daemon=True)
        writer.start()
        if not complete.wait(timeout=10):
            raise RuntimeError("blocked-stdout requests did not reach the broker input pipe")
        if failures:
            raise RuntimeError(f"blocked-stdout input failed: {failures[0]}")
        if not native.wait(handle, 15000):
            raise RuntimeError(f"shell process did not exit after blocked-stdout {mode}")
        code = native.exit_code(handle)
        broker.resume_stdout()
        if mode == "close":
            response = broker.response("blocked-close")
            if response.get("error") is not None:
                raise RuntimeError(f"close response reported failure: {response['error']}")
        # EOF case requires the actual EOF acknowledgement after reading is
        # restored; a process exit or a timeout alone cannot substitute for it.
        broker.finish()
        print(f"PASS windows blocked-{mode}: pipe saturated {available}/{capacity} bytes; "
              f"same shell handle signaled with exit code {code}; EOF acknowledged")
    finally:
        try:
            broker.close()
            if writer is not None:
                writer.join(timeout=2)
                if writer.is_alive():
                    raise RuntimeError("blocked-stdout writer did not finish after broker close")
        finally:
            if handle is not None and native is not None:
                try:
                    if not native.wait(handle, 10000):
                        raise RuntimeError("fixture shell remains alive after broker termination")
                finally:
                    native.close(handle)


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
    parser.add_argument("--with-rust-tests", action="store_true", help="also run the production Rust terminal unit tests")
    parser.add_argument("--rust-tests-only", action="store_true", help="run only the production Rust terminal unit tests")
    parser.add_argument("--with-windows-backpressure", action="store_true", help="also verify blocked stdout using native Windows process and pipe handles")
    args = parser.parse_args()
    if args.rust_tests_only and (args.with_node or args.with_rust_tests or args.with_windows_backpressure):
        parser.error("--rust-tests-only cannot be combined with other test modes")
    if args.with_windows_backpressure and os.name != "nt":
        parser.error("--with-windows-backpressure requires Windows")
    with_rust_tests = args.with_rust_tests or args.rust_tests_only
    source_hash, lock_hash = sha256(SOURCE), sha256(LOCK)
    original = tomllib.loads(LOCK.read_text())
    with tempfile.TemporaryDirectory(prefix="terminal-eof-probe-") as temporary:
        directory = Path(temporary).resolve()
        make_crate(directory, original, with_rust_tests)
        env = os.environ.copy()
        toolchain = tomllib.loads((ROOT / "rust-toolchain.toml").read_text())["toolchain"]["channel"]
        env.update(CARGO_TARGET_DIR=str(directory / "target"), CARGO_INCREMENTAL="0",
                   CARGO_PROFILE_DEV_DEBUG="0", CARGO_PROFILE_TEST_DEBUG="0",
                   CARGO_BUILD_JOBS="1", RUSTUP_TOOLCHAIN=toolchain)
        if with_rust_tests:
            resolve_test_graph(directory, original, env, args.offline)
            run_rust_tests(directory, env, args.offline)
            verify_graph(original, directory / "Cargo.lock")
        if not args.rust_tests_only:
            command = ["cargo", "build", "--manifest-path", str(directory / "Cargo.toml")]
            if with_rust_tests:
                command.append("--locked")
            if args.offline:
                command.append("--offline")
            subprocess.run(command, check=True, cwd=directory, env=env)
            verify_graph(original, directory / "Cargo.lock")
        if (sha256(SOURCE), sha256(LOCK)) != (source_hash, lock_hash):
            raise RuntimeError("source or product lock changed during the diagnostic build")
        print(f"source sha256={source_hash}; Cargo.lock sha256={lock_hash}; platform={sys.platform}")
        if not args.rust_tests_only:
            executable = directory / "target/debug" / ("terminal-eof-probe.exe" if os.name == "nt" else "terminal-eof-probe")
            run_case(executable, directory, "live")
            run_case(executable, directory, "exited")
            if args.with_node:
                run_node_tests(executable)
            if args.with_windows_backpressure:
                run_windows_backpressure_case(executable, directory, "close")
                run_windows_backpressure_case(executable, directory, "eof")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
