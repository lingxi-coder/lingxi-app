"""Exercise the CI probe's real subprocess protocol and failure paths."""

from pathlib import Path
import os
import platform
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
    @unittest.skipUnless(sys.platform == "darwin" and os.environ.get("LINGXI_GSTREAMER_SDK_MACOS"),
                         "requires the configured macOS GStreamer SDK")
    def test_macos_sdk_dylib_needs_and_resolves_the_linked_rpath(self) -> None:
        sdk_lib = Path(os.environ["LINGXI_GSTREAMER_SDK_MACOS"]) / "lib"
        library = sdk_lib / "libgstvideo-1.0.0.dylib"
        self.assertTrue(library.is_file(), f"SDK library missing: {library}")
        subprocess.run(["/usr/bin/lipo", "-verify_arch", platform.machine(), str(library)], check=True)
        install_name = subprocess.run(["/usr/bin/otool", "-D", str(library)],
                                      check=True, text=True, capture_output=True).stdout
        if "@rpath/libgstvideo-1.0.0.dylib" not in install_name:
            self.skipTest("SDK libgstvideo uses a different install-name convention")
        linker_argument = f"-Wl,-rpath,{sdk_lib}"
        if os.environ.get("GITHUB_ACTIONS") == "true":
            self.assertIn(f"-C link-arg={linker_argument}", os.environ.get("RUSTFLAGS", ""))
        with tempfile.TemporaryDirectory(prefix="local-app-macos-rpath-") as directory:
            root = Path(directory).resolve()
            source = root / "main.c"
            source.write_text(
                '#include <stdio.h>\n'
                'extern const char *gst_video_format_to_string(int);\n'
                'int main(void) { char line[1024]; '
                'if (!gst_video_format_to_string(1) || !fgets(line, sizeof(line), stdin)) return 1; '
                'puts("{\\"id\\":\\"ci-local-app-list\\",\\"ok\\":true,\\"value\\":[]}"); return 0; }\n',
                encoding="utf-8",
            )
            missing = root / "without-rpath"
            linked = root / "with-rpath"
            for binary, flags in ((missing, []), (linked, [linker_argument])):
                subprocess.run(["/usr/bin/clang", str(source), str(library), *flags, "-o", str(binary)],
                               check=True, capture_output=True, text=True)
            # The negative binary must still fail when the caller supplies an
            # otherwise sufficient DYLD override: the probe removes it.
            with mock.patch.dict(os.environ, {"DYLD_LIBRARY_PATH": str(sdk_lib)}):
                with self.assertRaisesRegex(RuntimeError, "exited .*no LC_RPATH's found"):
                    probe([str(missing)], root / "missing", root)
                probe([str(linked)], root / "linked", root)

    def test_macos_does_not_inherit_dyld_search_overrides(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            with mock.patch.object(service.sys, "platform", "darwin"), mock.patch.dict(os.environ, {
                "DYLD_LIBRARY_PATH": "/external/libraries",
                "DYLD_FRAMEWORK_PATH": "/external/frameworks",
                "DYLD_FALLBACK_LIBRARY_PATH": "/external/fallback",
                "DYLD_FALLBACK_FRAMEWORK_PATH": "/external/fallback-frameworks",
                "DYLD_INSERT_LIBRARIES": "/external/injected.dylib",
            }):
                env = service.isolated_environment(Path(directory))
            self.assertFalse(any(name.startswith("DYLD_") for name in env))
            self.assertEqual(env["HOME"], str(Path(directory) / "home"))

    def test_macos_reports_rpath_and_fails_on_load_command_inspection_error(self) -> None:
        libraries = subprocess.CompletedProcess([], 0, "binary:\n\t@rpath/libgstvideo.dylib\n", "")
        load_commands = subprocess.CompletedProcess([], 0, "cmd LC_RPATH\n  cmdsize 72\n  path /sdk/lib (offset 12)\n", "")
        with mock.patch.object(service.sys, "platform", "darwin"), \
                mock.patch.object(service.subprocess, "run", side_effect=[libraries, load_commands]), \
                mock.patch("builtins.print") as output:
            service.show_native_libraries(Path("/example/bridge-server"))
        output.assert_any_call("Executable LC_RPATH entries: ['/sdk/lib']", flush=True)
        failed = subprocess.CompletedProcess([], 1, "", "invalid Mach-O")
        with mock.patch.object(service.sys, "platform", "darwin"), \
                mock.patch.object(service.subprocess, "run", side_effect=[libraries, failed]):
            with self.assertRaisesRegex(RuntimeError, "load-command inspection failed.*invalid Mach-O"):
                service.show_native_libraries(Path("/example/bridge-server"))

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
