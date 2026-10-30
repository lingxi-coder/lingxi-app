from contextlib import redirect_stdout
import importlib.util
import io
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile
import time
import types
import unittest
from unittest import mock


SCRIPT = Path(__file__).resolve().parents[1] / "ci" / "probe_terminal_eof.py"
spec = importlib.util.spec_from_file_location("probe_terminal_eof", SCRIPT)
assert spec is not None and spec.loader is not None
probe = importlib.util.module_from_spec(spec)
spec.loader.exec_module(probe)


class ProbeTests(unittest.TestCase):
    @unittest.skipIf(os.name == "nt", "the native Windows test runs the actual broker")
    def test_pause_handshake_stops_a_real_reader_and_resumes_it(self):
        with tempfile.TemporaryDirectory(prefix="terminal-probe-reader-") as temp:
            root = Path(temp)
            executable = root / "fixture"
            executable.write_text(
                "#!/usr/bin/env python3\n"
                "import json, sys\n"
                "for line in sys.stdin:\n"
                " r=json.loads(line)\n"
                " print(json.dumps({'kind':'response','requestId':r['requestId']}), flush=True)\n"
                "print(json.dumps({'kind':'shutdown','reason':'eof'}), flush=True)\n",
                encoding="utf-8",
            )
            executable.chmod(0o755)
            cwd = root / "session"
            cwd.mkdir()
            broker = probe.Broker(executable, cwd)
            try:
                broker.command("first", "resize", cols=80, rows=24)
                broker.pause_stdout()
                self.assertTrue(broker.stdout_paused.is_set())
                before = broker.queue.qsize()
                for index in range(10):
                    broker.send({"kind": "resize", "terminalId": "test", "requestId": f"paused-{index}", "cols": 80, "rows": 24})
                time.sleep(0.05)
                self.assertEqual(broker.queue.qsize(), before, "the reader must not drain stdout while paused")
                broker.resume_stdout()
                broker.response("paused-9")
                broker.finish()
            finally:
                broker.close()

    def test_rust_test_mode_rejects_zero_tests(self):
        completed = types.SimpleNamespace(returncode=0, stdout="test result: ok. 0 passed; 0 failed\n", stderr="")
        with mock.patch.object(probe.subprocess, "run", return_value=completed), redirect_stdout(io.StringIO()):
            with self.assertRaisesRegex(RuntimeError, "did not pass exactly once"):
                probe.run_rust_tests(Path("/tmp"), {}, False)

    def test_graph_check_rejects_a_package_absent_from_product_lock(self):
        with tempfile.TemporaryDirectory(prefix="terminal-probe-lock-") as temp:
            lock = Path(temp) / "Cargo.lock"
            lock.write_text('version = 4\n[[package]]\nname = "unexpected"\nversion = "1.0.0"\n', encoding="utf-8")
            with self.assertRaisesRegex(ValueError, "not present in the product lock"):
                probe.verify_graph({"package": []}, lock)

    def test_native_wait_timeout_is_not_treated_as_exit(self):
        native = object.__new__(probe.WindowsNativeProbe)
        native.kernel = types.SimpleNamespace(WaitForSingleObject=lambda _handle, timeout: 0 if timeout == 1 else 258)
        self.assertTrue(native.wait(object(), 1))
        self.assertFalse(native.wait(object(), 0))

    def test_windows_diagnostic_rejects_unsaturated_pipe_and_unsignaled_process(self):
        instances = []

        class FakeBroker:
            def __init__(self, _executable, _cwd):
                self.output = ""
                self.shell_kind = "powershell"
                self.stdout_paused = probe.threading.Event()
                self.child = types.SimpleNamespace(poll=lambda: None, stdin=types.SimpleNamespace(close=lambda: None))
                self.closed = False
                instances.append(self)
            def ready(self): pass
            def command(self, request_id, _kind, **fields):
                if request_id == "pid": self.output += "__LX_PID_123__"
                if request_id == "identity":
                    nonce = re.search(r"'([0-9a-f]{24})'", fields["data"]).group(1)
                    self.output += f"__LX_HANDLE_{nonce}__"
                if request_id == "arm": self.output += "__LX_FLOOD_ARMED__"
                return {}
            def until(self, _label, condition): self.assert_ready(condition)
            def assert_ready(self, condition):
                if not condition(): raise AssertionError("missing fake marker")
            def pause_stdout(self): self.stdout_paused.set()
            def send(self, _request): pass
            def close(self): self.closed = True

        class FakeNative:
            saturated = False
            def __init__(self, _broker): self.handle = object(); self.closed = False
            def open_shell(self, _pid): return self.handle
            def capacity(self): return 4096
            def available(self): return 4096 if self.saturated else 0
            def wait(self, handle, timeout):
                if handle is not self.handle: raise AssertionError("wrong process handle")
                return timeout == 10000  # only fixture cleanup completes
            def close(self, handle):
                if handle is not self.handle: raise AssertionError("wrong process handle")
                self.closed = True

        with tempfile.TemporaryDirectory(prefix="terminal-probe-negative-") as temp:
            with mock.patch.object(probe, "Broker", FakeBroker), mock.patch.object(probe, "WindowsNativeProbe", FakeNative):
                with mock.patch.object(probe.time, "monotonic", side_effect=[0, 16]):
                    with self.assertRaisesRegex(RuntimeError, "did not saturate"):
                        probe.run_windows_backpressure_case(Path("fake"), Path(temp), "close")
                self.assertTrue(instances[-1].closed)
                FakeNative.saturated = True
                with self.assertRaisesRegex(RuntimeError, "did not exit"):
                    probe.run_windows_backpressure_case(Path("fake"), Path(temp), "eof")
                self.assertTrue(instances[-1].closed)

    @unittest.skipIf(os.name == "nt", "this checks non-Windows option rejection")
    def test_windows_mode_fails_on_a_non_windows_host(self):
        result = subprocess.run([sys.executable, str(SCRIPT), "--with-windows-backpressure"], capture_output=True, text=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("requires Windows", result.stderr)


if __name__ == "__main__":
    unittest.main()
