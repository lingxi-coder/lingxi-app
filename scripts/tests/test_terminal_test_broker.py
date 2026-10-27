import contextlib
import importlib.util
import io
import json
import os
import select
import shutil
import signal
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch


ROOT = Path(__file__).resolve().parents[2]
SCRIPT = ROOT / "scripts/ci/prepare_terminal_test_broker.py"
sys.path.insert(0, str(SCRIPT.parent))
spec = importlib.util.spec_from_file_location("prepare_terminal_test_broker", SCRIPT)
assert spec is not None and spec.loader is not None
broker = importlib.util.module_from_spec(spec)
spec.loader.exec_module(broker)
import build_gstreamer_linux as builder


class TerminalTestBroker(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="terminal broker 'test' ")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name).resolve()
        self.workspace = self.root / "workspace"
        self.binary = self.workspace / "target/release/bridge-server"
        self.binary.parent.mkdir(parents=True)
        self.runner = self.root / "runner"
        self.runner.mkdir()
        lock = broker.load_lock()
        identity = broker.build_identity(lock)
        self.output = self.runner / "gstreamer-sdk/linux-source" / f"gstreamer-{lock['version']}-{identity}"
        self.prefix = self.output / "prefix"
        self.library_dir = self.prefix / broker.LIBDIR
        self.library_dir.mkdir(parents=True)
        builder.source_sdk_marker(self.output).write_text(identity + "\n")
        for name in broker.RUNTIME_LIBRARIES:
            (self.library_dir / name).write_bytes(f"fixture {name}".encode())
        self.config = self.runner / "config"
        self.config.mkdir()
        self.wrapper = self.config / "terminal-wrapper"
        self.evidence = self.runner / "evidence"
        self.evidence.mkdir()
        self.binary.write_text(
            f"#!{sys.executable}\n"
            "import json, os, signal, sys\n"
            "signal.signal(signal.SIGTERM, lambda *_: sys.exit(73))\n"
            "print(json.dumps({'pid': os.getpid(), 'args': sys.argv[1:], 'env': dict(os.environ)}), flush=True)\n"
            "signal.pause()\n"
        )
        self.binary.chmod(0o755)

    def prepare(self, **kwargs):
        values = dict(workspace=self.workspace, runner_temp=self.runner, sdk_prefix=self.prefix,
                      bridge=self.binary, wrapper=self.wrapper, evidence=self.evidence)
        values.update(kwargs)
        return broker.prepare(**values)

    def test_exec_preserves_arguments_pid_and_signal_with_filtered_environment(self):
        manifest = self.prepare()
        args = ["--desktop-terminal", "", "two words", "dollar $ and 'quotes'", "line\nbreak", "终端"]
        env = {
            "PATH": "/usr/bin:/bin", "HOME": str(self.root), "TERM": "xterm-256color",
            "COLORTERM": "truecolor", "LD_LIBRARY_PATH": "/untrusted/path",
            "LD_PRELOAD": "/nonexistent/lingxi-test-preload.so", "LD_AUDIT": "/nonexistent/audit.so",
            "LD_DEBUG": "unused", "GLIBC_TUNABLES": "evil=value", "GCONV_PATH": "/untrusted",
            "PYTHONPATH": "/untrusted", "UNTRUSTED_TEST_SECRET": "sentinel",
        }
        child = subprocess.Popen([str(self.wrapper), *args], env=env, stdout=subprocess.PIPE,
                                 stderr=subprocess.PIPE, text=True)
        try:
            assert child.stdout is not None
            self.assertTrue(select.select([child.stdout], [], [], 5)[0], "broker did not report readiness")
            report = json.loads(child.stdout.readline())
            self.assertEqual(report["pid"], child.pid)
            self.assertEqual(report["args"], args)
            self.assertEqual(report["env"]["LD_LIBRARY_PATH"], str(self.library_dir))
            for key in ("LD_PRELOAD", "LD_AUDIT", "LD_DEBUG", "GLIBC_TUNABLES", "GCONV_PATH",
                        "PYTHONPATH", "UNTRUSTED_TEST_SECRET"):
                self.assertNotIn(key, report["env"])
            self.assertEqual(report["env"]["HOME"], str(self.root))
            self.assertEqual(report["env"]["TERM"], "xterm-256color")
            self.assertNotIn("TMPDIR", report["env"])
            child.send_signal(signal.SIGTERM)
            _, stderr = child.communicate(timeout=5)
            self.assertEqual(child.returncode, 73, stderr)
        finally:
            if child.poll() is None:
                child.kill()
                child.communicate(timeout=5)
        records = manifest["files"]
        self.assertEqual(sum(record["kind"] == "sdk-library" for record in records), len(broker.RUNTIME_LIBRARIES))
        self.assertTrue(any(record["kind"] == "bridge-server" for record in records))
        self.assertTrue(any(record["kind"] == "launcher" for record in records))
        self.assertTrue(all(len(record["sha256"]) == 64 for record in records))
        broker.verify_manifest(self.evidence / "terminal-broker-provenance.json")

    def test_rejects_untrusted_sdk_identity_and_outside_inputs(self):
        outside = self.root / "other-prefix"
        outside.mkdir()
        with self.assertRaises(broker.PreparationError):
            self.prepare(sdk_prefix=outside)
        marker = builder.source_sdk_marker(self.output)
        identity = marker.read_text()
        marker.write_text("incorrect\n")
        with self.assertRaises(broker.PreparationError):
            self.prepare()
        marker.write_text(identity)
        with self.assertRaises(broker.PreparationError):
            self.prepare(wrapper=self.root / "outside-wrapper")
        outside_binary = self.root / "outside-binary"
        outside_binary.write_text("untrusted")
        outside_binary.chmod(0o755)
        with self.assertRaises(broker.PreparationError):
            self.prepare(bridge=outside_binary)
        self.assertFalse(self.wrapper.exists())

    def test_cli_consumes_only_the_builder_export_and_its_validated_marker(self):
        exported = builder.source_sdk_environment(self.prefix)
        self.assertTrue(exported.items() <= builder._environment(self.prefix).items())
        self.assertEqual(builder.source_sdk_prefix(exported), str(self.prefix))
        source_root = self.runner / "gstreamer-sdk/linux-source"
        with patch.object(builder, "_validate_prefix") as validate:
            self.assertEqual(builder._build_sdk(source_root, builder.load_lock()), self.prefix)
            validate.assert_called_once_with(self.prefix, builder.load_lock()["version"], False)

        argv = [str(SCRIPT), "--workspace", str(self.workspace), "--runner-temp", str(self.runner),
                "--bridge", str(self.binary), "--wrapper", str(self.wrapper), "--evidence", str(self.evidence)]
        outside = self.root / "other-prefix"
        outside.mkdir()
        empty_prefix = dict.fromkeys(exported, "")
        for environment in ({}, empty_prefix, builder.source_sdk_environment(outside)):
            with self.subTest(environment=environment), patch.object(sys, "argv", argv), \
                    patch.object(sys, "platform", "linux"), patch.dict(os.environ, environment, clear=True), \
                    contextlib.redirect_stderr(io.StringIO()), self.assertRaises(SystemExit) as error:
                broker.main()
            self.assertEqual(error.exception.code, 1)
            self.assertFalse(self.wrapper.exists())

        with patch.object(sys, "argv", argv), patch.object(sys, "platform", "linux"), \
                patch.dict(os.environ, exported, clear=True), patch.object(broker, "probe_linkage") as probe, \
                contextlib.redirect_stdout(io.StringIO()):
            broker.main()
        probe.assert_called_once_with(self.binary, self.library_dir, self.evidence)
        broker.verify_manifest(self.evidence / "terminal-broker-provenance.json")

    def test_rejects_loader_metacharacters_in_an_otherwise_valid_sdk_path(self):
        for name in ("runner:untrusted", "runner;untrusted", "runner-$ORIGIN", "runner\nnewline"):
            with self.subTest(name=name):
                alternate_runner = self.root / name
                shutil.copytree(self.runner / "gstreamer-sdk", alternate_runner / "gstreamer-sdk")
                alternate_prefix = alternate_runner / self.prefix.relative_to(self.runner)
                config = alternate_runner / "config"
                evidence = alternate_runner / "evidence"
                config.mkdir()
                evidence.mkdir()
                with self.assertRaises(broker.PreparationError):
                    self.prepare(runner_temp=alternate_runner, sdk_prefix=alternate_prefix,
                                 wrapper=config / "wrapper", evidence=evidence)

    def test_rejects_missing_or_escaping_libraries_and_existing_wrapper(self):
        library = self.library_dir / broker.RUNTIME_LIBRARIES[0]
        library.unlink()
        with self.assertRaises(broker.PreparationError):
            self.prepare()
        outside = self.root / "external.so"
        outside.write_text("untrusted")
        library.symlink_to(outside)
        with self.assertRaises(broker.PreparationError):
            self.prepare()
        library.unlink()
        library.write_text("restored")
        self.wrapper.write_text("preserve existing contents")
        with self.assertRaises(FileExistsError):
            self.prepare()
        self.assertEqual(self.wrapper.read_text(), "preserve existing contents")

    def test_detects_library_content_and_symlink_changes(self):
        versioned = self.library_dir / "libfixture.so.0.1"
        versioned.write_text("version one")
        other = self.library_dir / "libfixture.so.0.2"
        other.write_text("version one")
        alias = self.library_dir / "libfixture.so.0"
        alias.symlink_to(versioned.name)
        self.prepare()
        manifest = self.evidence / "terminal-broker-provenance.json"
        versioned.write_text("version two")
        with self.assertRaises(broker.PreparationError):
            broker.verify_manifest(manifest)
        versioned.write_text("version one")
        alias.unlink()
        alias.symlink_to(other.name)
        with self.assertRaises(broker.PreparationError):
            broker.verify_manifest(manifest)
        alias.unlink()
        alias.symlink_to(versioned.name)
        (self.library_dir / "libinjected.so.0").write_text("untrusted")
        with self.assertRaises(broker.PreparationError):
            broker.verify_manifest(manifest)

    def test_linkage_probe_rejects_missing_symbols_and_non_sdk_gstreamer(self):
        system = SimpleNamespace(returncode=1, stdout="libgstreamer-1.0.so.0 => not found\n", stderr="")
        sdk_line = f"libgstreamer-1.0.so.0 => {self.library_dir / 'libgstreamer-1.0.so.0'} (0x1234)\n"
        good = SimpleNamespace(returncode=0, stdout=sdk_line, stderr="")
        unresolved = SimpleNamespace(returncode=0, stdout=sdk_line, stderr="undefined symbol: missing_fixture\n")
        outside = self.root / "outside.so"
        outside.write_text("outside")
        outside_result = SimpleNamespace(returncode=0,
            stdout=f"libgstreamer-1.0.so.0 => {outside} (0x1234)\n", stderr="")
        absent = SimpleNamespace(returncode=0, stdout="libc.so.6 => /lib/libc.so.6 (0x1234)\n", stderr="")
        for candidate, succeeds in ((good, True), (unresolved, False), (outside_result, False), (absent, False)):
            with self.subTest(candidate=candidate), patch.object(broker.shutil, "which", return_value="/usr/bin/ldd"), \
                    patch.object(broker.subprocess, "run", side_effect=[system, candidate]) as run:
                if succeeds:
                    broker.probe_linkage(self.binary, self.library_dir, self.evidence)
                    self.assertEqual(run.call_args_list[0].kwargs["env"], {"PATH": "/usr/bin:/bin", "LC_ALL": "C"})
                    self.assertEqual(run.call_args_list[1].kwargs["env"]["LD_LIBRARY_PATH"], str(self.library_dir))
                else:
                    with self.assertRaises(broker.PreparationError):
                        broker.probe_linkage(self.binary, self.library_dir, self.evidence)

    @unittest.skipUnless(sys.platform.startswith("linux") and shutil.which("cc") and shutil.which("ldd"),
                         "requires Linux with cc and ldd")
    def test_linux_loader_really_resolves_the_pinned_sdk(self):
        library_source = self.root / "fixture-library.c"
        binary_source = self.root / "fixture-binary.c"
        library_source.write_text("int lingxi_fixture_symbol(void) { return 37; }\n")
        binary_source.write_text(
            "#include <stdio.h>\n"
            "extern int lingxi_fixture_symbol(void);\n"
            "int main(void) { printf(\"%d\\n\", lingxi_fixture_symbol()); return 0; }\n"
        )
        soname = self.library_dir / "libgstreamer-1.0.so.0"
        subprocess.run(["cc", "-shared", "-fPIC", "-Wl,-soname,libgstreamer-1.0.so.0",
                        "-o", str(soname), str(library_source)], check=True, capture_output=True)
        subprocess.run(["cc", "-o", str(self.binary), str(binary_source),
                        f"-L{self.library_dir}", "-l:libgstreamer-1.0.so.0"], check=True, capture_output=True)
        manifest = self.prepare()
        broker.probe_linkage(self.binary, self.library_dir, self.evidence)
        system_probe = (self.evidence / "terminal-broker-ldd-sanitized-system.txt").read_text()
        self.assertTrue("not found" in system_probe or "undefined symbol:" in system_probe)
        pinned_probe = (self.evidence / "terminal-broker-ldd-pinned-sdk.txt").read_text()
        self.assertIn(str(self.library_dir), pinned_probe)
        clean_env = {"PATH": "/usr/bin:/bin"}
        failed = subprocess.run([str(self.binary)], env=clean_env, text=True, capture_output=True)
        self.assertNotEqual(failed.returncode, 0)
        loaded = subprocess.run([str(self.wrapper)], env=clean_env, text=True, capture_output=True, check=True)
        self.assertEqual(loaded.stdout, "37\n")
        self.assertEqual(manifest["sdk_library_dir"], str(self.library_dir))


if __name__ == "__main__":
    unittest.main()
