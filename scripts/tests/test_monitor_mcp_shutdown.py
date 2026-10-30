import importlib.util
import os
import signal
import subprocess
import sys
import tempfile
import time
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
SCRIPT = ROOT / "scripts/ci/monitor_mcp_shutdown.py"
spec = importlib.util.spec_from_file_location("monitor_mcp_shutdown", SCRIPT)
assert spec is not None and spec.loader is not None
monitor = importlib.util.module_from_spec(spec)
spec.loader.exec_module(monitor)


def workflow_body():
    source = (ROOT / ".github/workflows/ci.yml").read_text()
    section = source.split("      - name: Test all retained product packages\n", 1)[1]
    block = section.split("        run: |\n", 1)[1].split("\n\n  lint:", 1)[0]
    return "\n".join(line[10:] for line in block.splitlines()) + "\n"


def fake_process(root, pid, start, executable, tasks, parent=0):
    directory = root / str(pid)
    directory.mkdir()
    # Kernel stat field 22 is starttime, indexed 19 after the (comm) field.
    (directory / "stat").write_text(f"{pid} (sensitive-name) S {parent} " + "0 " * 17 + f"{start} 0\n")
    (directory / "exe").symlink_to(executable)
    for tid, (children, name, channel) in tasks.items():
        task = directory / "task" / str(tid)
        task.mkdir(parents=True)
        (task / "children").write_text(" ".join(map(str, children)))
        (task / "comm").write_text(name)
        (task / "wchan").write_text(channel)


class ProcSnapshotTests(unittest.TestCase):
    def test_discovers_only_test_descendants_and_redacts_names(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            fake_process(root, 100, 1, "/sensitive/bash", {100: ([105, 110], "bash", "secret")})
            fake_process(root, 105, 2, "/sensitive/monitor", {105: ([], "monitor", "secret")}, 100)
            fake_process(root, 110, 3, "/sensitive/cargo", {110: ([120, 140], "cargo", "secret")}, 100)
            fake_process(root, 120, 4, "/sensitive/mcp_serve-123abc", {121: ([130], "private-test-name", "secret")}, 110)
            fake_process(root, 130, 5, "/sensitive/lingxi-cli", {
                130: ([], "lingxi-cli", "0"),
                131: ([], "tokio-runtime-w", "futex_wait_queue"),
                132: ([], "lingxi-mcp-runt", "futex_wait_queue"),
                133: ([], "private-thread-name", "private-wait-channel"),
            }, 120)
            fake_process(root, 140, 6, "/sensitive/other", {140: ([150], "other", "secret")}, 110)
            fake_process(root, 150, 7, "/sensitive/lingxi-cli", {150: ([], "lingxi-mcp-runt", "secret")}, 140)
            scan = monitor.Monitor(root, 100, 105)
            scan.sample(20.0)
            scan.sample(21.1)
            report = scan.report()
            self.assertIn("test_tid=121 cli_pid=130 elapsed_ms=1100", report)
            self.assertIn("tid=132 role=shutdown wait=futex", report)
            self.assertIn("tid=133 role=other wait=other", report)
            self.assertNotIn("cli_pid=150", report)
            self.assertNotIn("sensitive", report)
            self.assertNotIn("private", report)

    def test_stale_or_missing_identity_and_missing_proc_are_not_success_claims(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            fake_process(root, 3, 42, "/private/lingxi-cli", {3: ([], "lingxi-mcp-runt", "secret")})
            self.assertIsNone(monitor.snapshot(root, 3, 41))
            missing = monitor.Monitor(root, 999, 1000)
            missing.sample(1.0)
            self.assertIn("unavailable", missing.report())
            self.assertNotIn("secret", missing.report())

    def test_stale_children_listing_cannot_include_a_reparented_cli(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            fake_process(root, 100, 1, "/bash", {100: ([110], "bash", "0")})
            fake_process(root, 110, 2, "/cargo", {110: ([120], "cargo", "0")}, 100)
            fake_process(root, 120, 3, "/mcp_serve-123", {121: ([130], "test", "0")}, 110)
            fake_process(root, 130, 4, "/lingxi-cli", {130: ([], "lingxi-mcp-runt", "0")}, 999)
            self.assertEqual(monitor.discover(root, 100, 105), [])

    def test_sampling_is_bounded(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            fake_process(root, 100, 1, "/bash", {100: ([110], "bash", "0")})
            fake_process(root, 110, 2, "/cargo", {110: ([115], "cargo", "0")}, 100)
            fake_process(root, 115, 3, "/mcp_serve-123", {111: ([120], "test", "0")}, 110)
            fake_process(root, 120, 4, "/lingxi-cli", {
                tid: ([], "lingxi-mcp-runt" if tid == 219 else "unknown", "0")
                for tid in range(120, 220)
            }, 115)
            scan = monitor.Monitor(root, 100, 999)
            for instant in range(20):
                scan.sample(float(instant))
            samples = next(iter(scan.records.values()))["samples"]
            self.assertEqual(len(samples), monitor.MAX_SNAPSHOTS)
            self.assertTrue(all(len(threads) <= monitor.MAX_THREADS for _, threads in samples))


class WorkflowProcessTests(unittest.TestCase):
    @staticmethod
    def is_running(pid):
        try:
            os.kill(pid, 0)
        except ProcessLookupError:
            return False
        stat = monitor.read_small(Path("/proc") / str(pid) / "stat", 4096)
        # A zombie is no longer executing; its adoptive parent owns reaping.
        return stat is None or stat.rsplit(")", 1)[1].split()[0] != "Z"

    def run_workflow(self, code, broken_monitor=False, missing_temp=False, signalled=False):
        with tempfile.TemporaryDirectory() as temp:
            directory = Path(temp)
            cargo = directory / "cargo"
            cargo.write_text(
                "#!/bin/bash\nIFS= read -r value || true\n"
                "printf 'cargo-out:%s\\n' \"$value\"\n"
                "printf 'cargo-err:%s\\n' \"$value\" >&2\n"
                "if [[ \"$TEST_CARGO_SIGNAL\" == 1 ]]; then kill -TERM \"$$\"; fi\n"
                "exit \"$TEST_CARGO_CODE\"\n"
            )
            cargo.chmod(0o755)
            if broken_monitor:
                fake_python = directory / "python3"
                fake_python.write_text("#!/bin/bash\nexit 44\n")
                fake_python.chmod(0o755)
            environment = os.environ.copy()
            environment.update({
                "PATH": str(directory) + os.pathsep + environment.get("PATH", ""),
                "RUNNER_TEMP": str(directory / "missing") if missing_temp else str(directory),
                "TEST_CARGO_CODE": str(code),
                "TEST_CARGO_SIGNAL": "1" if signalled else "0",
            })
            return subprocess.run(
                ["/bin/bash", "-e", "-c", workflow_body()], cwd=ROOT, env=environment,
                input="sentinel input\n", text=True, capture_output=True, timeout=5,
            )

    def test_preserves_stdin_stdout_stderr_and_exit_status(self):
        for code in (0, 1, 17, 101):
            with self.subTest(code=code):
                result = self.run_workflow(code)
                self.assertEqual(result.returncode, code)
                self.assertEqual(result.stdout, "cargo-out:sentinel input\n")
                self.assertTrue(result.stderr.startswith("cargo-err:sentinel input\n"))
                self.assertEqual("diagnostics" in result.stderr.lower(), code != 0)
                if code:
                    self.assertIn("thread names and wait channels classified", result.stderr)

    def test_monitor_failure_and_unavailable_temp_do_not_change_cargo_status(self):
        for kwargs in ({"broken_monitor": True}, {"missing_temp": True}):
            for code in (0, 17):
                with self.subTest(code=code, options=kwargs):
                    result = self.run_workflow(code, **kwargs)
                    self.assertEqual(result.returncode, code)
                    self.assertEqual(result.stdout, "cargo-out:sentinel input\n")
                    if code:
                        self.assertIn("diagnostics unavailable", result.stderr)
                    else:
                        self.assertEqual(result.stderr, "cargo-err:sentinel input\n")

    def test_cargo_terminated_by_signal_keeps_bash_status(self):
        result = self.run_workflow(0, signalled=True)
        self.assertEqual(result.returncode, 128 + signal.SIGTERM)
        self.assertEqual(result.stdout, "cargo-out:sentinel input\n")
        self.assertTrue(result.stderr.startswith("cargo-err:sentinel input\n"))
        self.assertIn("diagnostics", result.stderr.lower())

    def test_unwritable_completion_marker_does_not_delay_cargo_or_leave_monitor(self):
        with tempfile.TemporaryDirectory() as temp:
            directory = Path(temp)
            cargo = directory / "cargo"
            cargo.write_text(
                "#!/bin/bash\n"
                "for d in \"$RUNNER_TEMP\"/lingxi-mcp-monitor.*; do\n"
                "  while [[ ! -s \"$d/ready\" ]]; do sleep 0.01; done\n"
                "  cp \"$d/ready\" \"$TEST_WORKER_PID_FILE\"\n"
                "  mkdir \"$d/done\"\n"
                "done\nexit 17\n"
            )
            cargo.chmod(0o755)
            guard_file = directory / "guard-pid"
            worker_file = directory / "worker-pid"
            body = workflow_body().replace(
                'monitor_pid=$!',
                'monitor_pid=$!\nprintf "%s" "$monitor_pid" > "$TEST_GUARD_PID_FILE"',
                1,
            )
            environment = os.environ.copy()
            environment.update({
                "PATH": str(directory) + os.pathsep + environment.get("PATH", ""),
                "RUNNER_TEMP": str(directory),
                "TEST_GUARD_PID_FILE": str(guard_file),
                "TEST_WORKER_PID_FILE": str(worker_file),
            })
            shell = subprocess.Popen(
                ["/bin/bash", "-e", "-c", body], cwd=ROOT, env=environment,
                stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE,
                start_new_session=True,
            )
            try:
                _, stderr = shell.communicate(timeout=5)
                self.assertEqual(shell.returncode, 17)
                self.assertIn(b"diagnostics unavailable", stderr)
                pids = (int(guard_file.read_text()), int(worker_file.read_text()))
                deadline = time.monotonic() + 2
                while time.monotonic() < deadline and any(self.is_running(pid) for pid in pids):
                    time.sleep(0.02)
                self.assertFalse(any(self.is_running(pid) for pid in pids))
            finally:
                try:
                    os.killpg(shell.pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass
                shell.wait(timeout=3)

    def assert_signal_cleanup(self, target, signum):
        with tempfile.TemporaryDirectory() as temp:
            directory = Path(temp)
            cargo = directory / "cargo"
            cargo.write_text("#!/bin/bash\nprintf '%s' \"$$\" > \"$TEST_CHILD_PID_FILE\"\nexec sleep 30\n")
            cargo.chmod(0o755)
            child_file = directory / "child-pid"
            monitor_file = directory / "monitor-pid"
            body = workflow_body().replace(
                'monitor_pid=$!',
                'monitor_pid=$!\nprintf "%s" "$monitor_pid" > "$TEST_MONITOR_PID_FILE"',
                1,
            )
            environment = os.environ.copy()
            environment.update({
                "PATH": str(directory) + os.pathsep + environment.get("PATH", ""),
                "RUNNER_TEMP": str(directory),
                "TEST_CHILD_PID_FILE": str(child_file),
                "TEST_MONITOR_PID_FILE": str(monitor_file),
            })
            child = subprocess.Popen(
                ["/bin/bash", "-e", "-c", body], cwd=ROOT, env=environment,
                stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                start_new_session=True,
            )
            try:
                deadline = time.monotonic() + 3
                def ready(file):
                    try:
                        return file.read_text().isdigit()
                    except FileNotFoundError:
                        return False

                def monitor_ready():
                    return any(ready(file) for file in directory.glob("lingxi-mcp-monitor.*/ready"))

                while time.monotonic() < deadline and not (ready(child_file) and ready(monitor_file) and monitor_ready()):
                    time.sleep(0.01)
                self.assertTrue(ready(child_file) and ready(monitor_file) and monitor_ready())
                cargo_pid = int(child_file.read_text())
                monitor_pid = int(monitor_file.read_text())
                worker_pid = int(next(directory.glob("lingxi-mcp-monitor.*/ready")).read_text())
                if target == "group":
                    os.killpg(child.pid, signum)
                else:
                    os.kill(child.pid, signum)
                self.assertEqual(child.wait(timeout=3), -signum)
                deadline = time.monotonic() + 2
                while time.monotonic() < deadline and self.is_running(monitor_pid):
                    time.sleep(0.02)
                self.assertFalse(self.is_running(monitor_pid), "monitor did not stop after its shell")
                self.assertFalse(self.is_running(worker_pid), "sampler did not stop after its shell")
                if target == "parent":
                    # This is also the original shell-only signal behavior; the
                    # observer must not introduce a signal to the cargo child.
                    self.assertTrue(self.is_running(cargo_pid))
                else:
                    self.assertFalse(self.is_running(cargo_pid))
            finally:
                try:
                    os.killpg(child.pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass
                child.wait(timeout=3)

    def test_process_group_sigterm_stops_the_monitor(self):
        self.assert_signal_cleanup("group", signal.SIGTERM)

    def test_shell_only_sigterm_stops_monitor_without_signalling_cargo(self):
        self.assert_signal_cleanup("parent", signal.SIGTERM)

    def test_process_group_sigint_stops_the_monitor(self):
        self.assert_signal_cleanup("group", signal.SIGINT)

    def test_stopped_sampler_is_killed_and_reaped_after_cargo(self):
        with tempfile.TemporaryDirectory() as temp:
            directory = Path(temp)
            program = (
                "import importlib.util,os,signal,sys;"
                f"spec=importlib.util.spec_from_file_location('monitor',{str(SCRIPT)!r});"
                "m=importlib.util.module_from_spec(spec);spec.loader.exec_module(m);"
                "m.Monitor.sample=lambda self,now:os.kill(os.getpid(),signal.SIGSTOP);"
                "sys.argv=['monitor',sys.argv[1],sys.argv[2]];sys.exit(m.main())"
            )
            guard = subprocess.Popen(
                [sys.executable, "-c", program, str(os.getpid()), str(directory)],
                stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                start_new_session=True,
            )
            worker_pid = None
            try:
                ready = directory / "ready"
                deadline = time.monotonic() + 3
                while time.monotonic() < deadline:
                    if ready.exists() and ready.read_text().isdigit():
                        worker_pid = int(ready.read_text())
                        break
                    time.sleep(0.01)
                self.assertIsNotNone(worker_pid)
                started = time.monotonic()
                (directory / "done").write_text("")
                self.assertNotEqual(guard.wait(timeout=3), 0)
                self.assertLess(time.monotonic() - started, 2)
                self.assertFalse(self.is_running(worker_pid))
            finally:
                try:
                    os.killpg(guard.pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass
                guard.wait(timeout=3)


if __name__ == "__main__":
    unittest.main()
