"""Exercise the CI probe's real subprocess protocol and failure paths."""

from pathlib import Path
import os
import subprocess
import sys
import tempfile
import time
import unittest
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "ci"))
import probe_local_app_service as service  # noqa: E402
from probe_local_app_service import probe  # noqa: E402


CHILD = r'''
import json, os, pathlib, subprocess, sys, time
mode = sys.argv[1]
assert sys.argv[2] == '--local-app-service-json'
assert sys.argv[3] == '--config-home'
assert pathlib.Path(sys.argv[4]).is_absolute()
assert pathlib.Path(sys.argv[4]).is_dir()
assert os.environ.get('OPENAI_API_KEY') is None
assert os.environ.get('AWS_SECRET_ACCESS_KEY') is None
assert pathlib.Path(os.environ['HOME']).parent == pathlib.Path(sys.argv[4]).parent
request = json.loads(sys.stdin.readline())
assert request['request'] == {'op': 'list'}
if mode == 'exit':
    print('injected loader failure', file=sys.stderr)
    sys.exit(17)
if mode == 'malformed':
    print('not json')
elif mode == 'wrong':
    print(json.dumps({'id': request['id'], 'ok': True, 'value': [{'unexpected': True}]}))
elif mode == 'wrongid':
    print(json.dumps({'id': 'other-request', 'ok': True, 'value': []}))
elif mode == 'rejected':
    print(json.dumps({'id': request['id'], 'ok': False, 'error': 'rejected'}))
elif mode in ('descendant', 'exited-parent'):
    config = pathlib.Path(sys.argv[4])
    started, escaped = config / 'descendant-started', config / 'descendant-escaped'
    delay = 5 if mode == 'descendant' else 4
    code = 'import pathlib,time; pathlib.Path(%r).write_text("started"); time.sleep(%d); pathlib.Path(%r).write_text("escaped")' % (str(started), delay, str(escaped))
    subprocess.Popen([sys.executable, '-c', code])
    while not started.exists(): time.sleep(0.01)
    if mode == 'descendant':
        time.sleep(10)
    else:
        print(json.dumps({'id': request['id'], 'ok': True, 'value': []}))
elif mode == 'timeout':
    time.sleep(5)
else:
    print(json.dumps({'id': request['id'], 'ok': True, 'value': []}))
'''


class LocalAppServiceProbeTests(unittest.TestCase):
    def run_probe(self, mode: str, timeout: float = 10.0) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory).resolve()
            with mock.patch.dict(os.environ, {
                "OPENAI_API_KEY": "should-not-be-forwarded",
                "AWS_SECRET_ACCESS_KEY": "should-not-be-forwarded",
            }):
                probe([sys.executable, "-c", CHILD, mode], root / "isolated", root, timeout)

    def test_correlated_empty_list_and_isolated_credentials(self) -> None:
        self.run_probe("success")

    def test_early_exit_reports_stderr_and_status(self) -> None:
        with self.assertRaisesRegex(RuntimeError, "exited 17.*injected loader failure"):
            self.run_probe("exit")

    def test_malformed_and_nonempty_responses_fail(self) -> None:
        with self.assertRaisesRegex(RuntimeError, "invalid JSON"):
            self.run_probe("malformed")
        for mode in ("wrong", "wrongid", "rejected"):
            with self.subTest(mode=mode):
                with self.assertRaisesRegex(RuntimeError, "unexpected list response"):
                    self.run_probe(mode)

    def test_stuck_service_is_not_treated_as_healthy(self) -> None:
        with self.assertRaisesRegex(RuntimeError, "timed out"):
            self.run_probe("timeout", timeout=0.2)

    def test_timeout_stops_a_child_that_inherited_output(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory).resolve()
            isolated = root / "isolated"
            with self.assertRaisesRegex(RuntimeError, "timed out"):
                probe([sys.executable, "-c", CHILD, "descendant"], isolated, root, timeout=3.0)
            self.assertTrue((isolated / "config" / "descendant-started").exists())
            time.sleep(5.3)
            self.assertFalse((isolated / "config" / "descendant-escaped").exists())

    def test_timeout_reports_tree_termination_failure(self) -> None:
        if os.name == "posix":
            failure = mock.patch.object(service.os, "killpg", side_effect=OSError("injected"))
        else:
            failed_taskkill = subprocess.CompletedProcess([], 1, b"", b"injected")
            failure = mock.patch.object(service.subprocess, "run", return_value=failed_taskkill)
        with failure:
            with self.assertRaisesRegex(RuntimeError, "termination failed:.*injected"):
                self.run_probe("timeout", timeout=0.2)

    def test_exited_service_does_not_wait_for_inherited_output_handles(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory).resolve()
            isolated = root / "isolated"
            started = time.monotonic()
            probe([sys.executable, "-c", CHILD, "exited-parent"], isolated, root, timeout=2.0)
            self.assertLess(time.monotonic() - started, 3.0)
            # The fixture's child exits naturally. The probe only promises
            # bounded capture when its direct service has already exited.
            marker = isolated / "config" / "descendant-escaped"
            deadline = time.monotonic() + 10.0
            while not marker.exists() and time.monotonic() < deadline:
                time.sleep(0.05)
            self.assertTrue(marker.exists())


if __name__ == "__main__":
    unittest.main()
