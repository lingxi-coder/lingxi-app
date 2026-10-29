import importlib.util
import json
import os
from pathlib import Path, PurePosixPath
import subprocess
import sys
import tempfile
import textwrap
import unittest
from unittest import mock
import uuid
from types import SimpleNamespace


SCRIPTS = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(SCRIPTS))
RUNNER_SPEC = importlib.util.spec_from_file_location(
    "owner_death_retained_runner", SCRIPTS / "test-local-app-owner-death.py")
runner = importlib.util.module_from_spec(RUNNER_SPEC)
RUNNER_SPEC.loader.exec_module(runner)


PACKAGE = "com.lingxi.code.direct.debug"
APP_DATA = "/data/user/0/" + PACKAGE
TOKEN = str(uuid.uuid4())
SOURCE_TOKEN = str(uuid.uuid4())
CONFIG = (PurePosixPath(APP_DATA) / "files/mobile-linux/android-proot/persistent/lingxi"
          / (runner.CONFIG_PREFIX + TOKEN))
SOURCE = CONFIG / ".apps" / ("owner-death-" + SOURCE_TOKEN)
DATA_PATH = CONFIG / "local-apps/v2/apps/test-app/data"
CI_CAPABILITY = "a" * 64
RUN_NONCE = "b" * 64


def receipt(*, recovered=False):
    value = {
        "ownerDeathPhase": "recovered" if recovered else "prepared",
        "ownerDeathRunNonce": RUN_NONCE,
        "recoveryComplete": recovered,
        "hostProcessName": PACKAGE,
        "recoveryProcessName": PACKAGE,
        "configCreated": True,
        "ownerMarkerWritten": True,
        "configHome": str(CONFIG),
        "configToken": TOKEN,
        "source": str(SOURCE),
        "markerPath": str(SOURCE / "build-in-progress"),
        "dataPath": str(DATA_PATH),
        "data": "owner-death-retain-test-data",
        "appId": "test-app",
    }
    if recovered:
        value.update({
            "appRemoved": True,
            "backendPortsClosed": True,
            "candidateBuildStopped": True,
            "recoveryHostPid": 4102,
            "recoveryHostStartTicks": "99901",
            "recoverySelfKillRequested": True,
        })
    else:
        value.update({
            "hostPid": 4101,
            "hostStartTicks": "99900",
            "prepareSelfKillRequested": True,
        })
    return value


class RetainedOwnerDeathRunnerTests(unittest.TestCase):
    def test_ephemeral_ci_gate_requires_actions_emulator_and_qemu(self):
        runner.require_ephemeral_ci_emulator(
            ci="true", github_actions="true", runner_environment="github-hosted",
            serial="emulator-5554", qemu_property="1\n")
        invalid = (
            {"ci": "false", "github_actions": "true", "runner_environment": "github-hosted", "serial": "emulator-5554", "qemu_property": "1"},
            {"ci": "true", "github_actions": "false", "runner_environment": "github-hosted", "serial": "emulator-5554", "qemu_property": "1"},
            {"ci": "true", "github_actions": "true", "runner_environment": "self-hosted", "serial": "emulator-5554", "qemu_property": "1"},
            {"ci": "true", "github_actions": "true", "runner_environment": "github-hosted", "serial": "R58M123", "qemu_property": "0"},
            {"ci": "true", "github_actions": "true", "runner_environment": "github-hosted", "serial": "emulator-5554", "qemu_property": "0"},
        )
        for case in invalid:
            with self.subTest(case=case), self.assertRaises(RuntimeError):
                runner.require_ephemeral_ci_emulator(**case)

    def test_instrumentation_requires_a_runner_issued_one_time_capability(self):
        for phase in ("prepare", "recover"):
            args = runner.instrumentation_args(phase, CI_CAPABILITY, RUN_NONCE)
            self.assertEqual(args[args.index("ownerDeathCiCapability") + 1], CI_CAPABILITY)
            self.assertEqual(args[args.index("ownerDeathRunNonce") + 1], RUN_NONCE)
            self.assertIn(phase, args)
        with self.assertRaises(ValueError):
            runner.instrumentation_args("cleanup", CI_CAPABILITY, RUN_NONCE)
        with self.assertRaises(ValueError):
            runner.instrumentation_args("prepare", "ownerDeathCiEphemeral=true", RUN_NONCE)
        with self.assertRaises(ValueError):
            runner.instrumentation_args("prepare", CI_CAPABILITY, "stale")
        source = (SCRIPTS / "test-local-app-owner-death.py").read_text()
        self.assertNotIn('"kill"', source)
        self.assertNotIn('"rm"', source)
        owner_test = (SCRIPTS.parent / "app/src/androidTest/java/com/lingxi/code/apps/UnifiedLocalAppProcessDeathTest.kt").read_text()
        self.assertIn("consumeCiCapability(context, ciCapability)", owner_test)
        self.assertLess(owner_test.index("consumeCiCapability(context, ciCapability)"),
                        owner_test.index("newOwnerDeathConfigHome(context)"))
        self.assertIn("AtomicFile(file)", owner_test)

    def test_proc_stat_parser_handles_parentheses_and_start_ticks(self):
        fields = ["S"] + ["0"] * 18 + ["98765"]
        self.assertEqual(runner.parse_proc_stat("101 (runner (nested)) " + " ".join(fields)), ("S", "98765"))
        with self.assertRaises(RuntimeError):
            runner.parse_proc_stat("bad-stat")

    def test_identity_observer_never_signals_and_treats_reuse_as_original_exit(self):
        commands = []

        def command(*args, check=True):
            commands.append(args)
            if args[-1] == "/proc/101/stat":
                fields = ["S"] + ["0"] * 18 + ["98765"]
                return SimpleNamespace(stdout="101 (app host) " + " ".join(fields), stderr="", returncode=0)
            if args[-1] == "/proc/102/stat":
                fields = ["S"] + ["0"] * 18 + ["55555"]
                return SimpleNamespace(stdout="102 (reused pid) " + " ".join(fields), stderr="", returncode=0)
            return SimpleNamespace(stdout="", stderr="", returncode=1)

        self.assertTrue(runner.process_is_running(command, PACKAGE, 101, "98765"))
        self.assertFalse(runner.process_is_running(command, PACKAGE, 102, "98765"))
        self.assertTrue(all(call[-2:] == ("cat", call[-1]) for call in commands))
        self.assertFalse(any("kill" in call or "rm" in call for call in commands))

    def test_wait_observes_exit_without_sending_a_signal(self):
        calls = {"count": 0}

        def command(*args, check=True):
            calls["count"] += 1
            if calls["count"] == 1:
                fields = ["S"] + ["0"] * 18 + ["98765"]
                return SimpleNamespace(stdout="101 (runner) " + " ".join(fields), stderr="", returncode=0)
            return SimpleNamespace(stdout="", stderr="cat: No such file or directory", returncode=1)

        runner.wait_for_process_exit(command, PACKAGE, 101, "98765", timeout=1.0)
        self.assertEqual(calls["count"], 2)

    def test_receipt_binds_isolated_root_and_separate_recovery_process(self):
        prepared = runner.validate_receipt(receipt(), APP_DATA, recovered=False, run_nonce=RUN_NONCE)
        recovered = runner.validate_receipt(receipt(recovered=True), APP_DATA, recovered=True, run_nonce=RUN_NONCE)
        self.assertEqual(prepared["configHome"], str(CONFIG))
        self.assertEqual(prepared["source"], str(SOURCE))
        self.assertEqual(prepared["dataPath"], str(DATA_PATH))
        self.assertNotEqual((prepared["hostPid"], prepared["hostStartTicks"]),
                            (recovered["hostPid"], recovered["hostStartTicks"]))

    def test_receipt_negative_controls_reject_unowned_data_and_missing_self_kill(self):
        cases = []
        bad = receipt(); bad["configHome"] = "/data/user/0/other/config"
        cases.append((bad, False))
        bad = receipt(); bad["source"] = str(CONFIG / "outside" / SOURCE.name)
        cases.append((bad, False))
        bad = receipt(); bad["dataPath"] = "/data/user/0/unrelated/data"
        cases.append((bad, False))
        bad = receipt(); bad["prepareSelfKillRequested"] = False
        cases.append((bad, False))
        bad = receipt(recovered=True); bad["backendPortsClosed"] = False
        cases.append((bad, True))
        bad = receipt(recovered=True); bad["recoveryProcessName"] = PACKAGE + ".test"
        cases.append((bad, True))
        for value, is_recovered in cases:
            with self.subTest(value=value), self.assertRaises(RuntimeError):
                runner.validate_receipt(value, APP_DATA, recovered=is_recovered, run_nonce=RUN_NONCE)
        with self.assertRaisesRegex(RuntimeError, "this runner invocation"):
            runner.validate_receipt(receipt(), APP_DATA, recovered=False, run_nonce="c" * 64)

    def test_preflight_rejects_stale_atomicfile_sidecars(self):
        for stale_path in (runner.RECEIPT + ".bak", runner.RECEIPT + ".new",
                           runner.SETUP_RECEIPT + ".bak", runner.SETUP_RECEIPT + ".new"):
            checked = []

            def command(*args, check=True):
                checked.append(args[-1])
                return SimpleNamespace(stdout="", stderr="", returncode=0 if args[-1] == stale_path else 1)

            with self.subTest(stale_path=stale_path), self.assertRaisesRegex(RuntimeError, "AtomicFile sidecar"):
                runner.require_no_existing_owner_death_state(command)
            self.assertIn(stale_path, checked)

    def test_post_exit_receipt_retry_accepts_only_completed_prepare_checkpoint(self):
        responses = [
            SimpleNamespace(stdout="", stderr="cat: No such file or directory", returncode=1),
            SimpleNamespace(stdout=json.dumps(receipt()), stderr="", returncode=0),
        ]
        calls = []

        def command(*args, check=True, timeout=20):
            calls.append((args, timeout))
            return responses.pop(0)

        self.assertEqual(runner.read_completed_prepare_receipt(
            command, RUN_NONCE, attempts=2, retry_delay=0), receipt())
        self.assertEqual(len(calls), 2)
        self.assertTrue(all(timeout == 5 for _, timeout in calls))

        wrong_run = receipt()
        wrong_run["ownerDeathRunNonce"] = "c" * 64
        for stale in (receipt(recovered=True), wrong_run, {"ownerDeathPhase": "prepared"}, []):
            with self.subTest(stale=stale), self.assertRaisesRegex(RuntimeError, "unavailable or invalid"):
                runner.read_completed_prepare_receipt(
                    lambda *args, **kwargs: SimpleNamespace(
                        stdout=json.dumps(stale), stderr="", returncode=0),
                    RUN_NONCE, attempts=2, retry_delay=0)

    def test_prepare_deadline_rereads_after_exit_during_last_sleep(self):
        now = [0.0]
        polls = iter((None, 0))
        calls = []

        def command(*args, **kwargs):
            calls.append(args)
            if len(calls) == 2:
                return SimpleNamespace(stdout="", stderr="missing", returncode=1)
            body = {"ownerDeathPhase": "starting"} if len(calls) == 1 else receipt()
            return SimpleNamespace(stdout=json.dumps(body), stderr="", returncode=0)

        with mock.patch.object(runner.time, "monotonic", side_effect=lambda: now[0]), \
                mock.patch.object(runner.time, "sleep", side_effect=lambda _: now.__setitem__(0, 2.0)):
            prepared = runner.wait_for_prepare_checkpoint(
                command, SimpleNamespace(poll=lambda: next(polls)), RUN_NONCE, timeout=1)
        self.assertEqual(prepared, receipt())
        self.assertEqual(len(calls), 3)

    def test_prepare_deadline_accepts_committed_receipt_while_host_exits(self):
        now = [0.0]
        calls = []

        def command(*args, **kwargs):
            calls.append(args)
            body = {"ownerDeathPhase": "starting"} if len(calls) == 1 else receipt()
            return SimpleNamespace(stdout=json.dumps(body), stderr="", returncode=0)

        with mock.patch.object(runner.time, "monotonic", side_effect=lambda: now[0]), \
                mock.patch.object(runner.time, "sleep", side_effect=lambda _: now.__setitem__(0, 2.0)):
            prepared = runner.wait_for_prepare_checkpoint(
                command, SimpleNamespace(poll=lambda: None), RUN_NONCE, timeout=1)
        self.assertEqual(prepared, receipt())
        self.assertEqual(len(calls), 2)

    def test_prepare_deadline_still_fails_if_host_is_running(self):
        now = [0.0]
        calls = []

        def command(*args, **kwargs):
            calls.append(args)
            return SimpleNamespace(stdout=json.dumps({"ownerDeathPhase": "starting"}), stderr="", returncode=0)

        with mock.patch.object(runner.time, "monotonic", side_effect=lambda: now[0]), \
                mock.patch.object(runner.time, "sleep", side_effect=lambda _: now.__setitem__(0, 2.0)):
            with self.assertRaisesRegex(RuntimeError, "checkpoint before timeout"):
                runner.wait_for_prepare_checkpoint(
                    command, SimpleNamespace(poll=lambda: None), RUN_NONCE, timeout=1)
        self.assertEqual(len(calls), 2)

    def test_full_runner_simulation_observes_both_self_exits_without_signals_or_deletion(self):
        with tempfile.TemporaryDirectory(prefix="owner death runner ") as temporary:
            root = Path(temporary)
            fake_bin = root / "bin"
            fake_bin.mkdir()
            state_path = root / "device-state.json"
            event_path = root / "adb-events.jsonl"
            initial = {
                "appData": APP_DATA,
                "config": str(CONFIG),
                "source": str(SOURCE),
                "token": TOKEN,
                "capability": None,
                "marker": str(SOURCE / "build-in-progress"),
                "dataPath": str(DATA_PATH),
                "data": "owner-death-retain-test-data",
                "receipt": None,
                "run_nonce": None,
                "hostAlive": False,
                "buildAlive": False,
                "recoveryAlive": False,
                "phase": None,
                "partial_receipt_reads": 0,
                "partial_receipt_served": 0,
                "atomic_gap_receipt_reads": 0,
                "atomic_gap_served": 0,
                "receipt_read_started": False,
                "prepare_instrumentation_finished": False,
                "post_exit_prepare_receipt_reads": 0,
            }
            state_path.write_text(json.dumps(initial))
            fake_adb = fake_bin / "adb"
            fake_adb.write_text("#!/usr/bin/env python3\n" + textwrap.dedent(r'''
                import json, os, re, sys, time
                from pathlib import Path
                args = sys.argv[1:]
                state_path = Path(os.environ["FAKE_DEVICE_STATE"])
                event_path = Path(os.environ["FAKE_DEVICE_EVENTS"])
                with event_path.open("a") as stream:
                    stream.write(json.dumps(args) + "\n")
                state = json.loads(state_path.read_text())
                def save(): state_path.write_text(json.dumps(state))
                def output(value="", code=0, newline=True):
                    if value:
                        if newline: print(value)
                        else: sys.stdout.write(value)
                    raise SystemExit(code)
                def stat_line(pid, ticks):
                    fields = ["S"] + ["0"] * 18 + [str(ticks)]
                    return str(pid) + " (lingxi app) " + " ".join(fields)
                device = args[2:]
                if device == ["shell", "getprop", "ro.kernel.qemu"]: output("1")
                if device == ["shell", "run-as", "com.lingxi.code.direct.debug", "pwd"]: output(state["appData"])
                if device[:5] == ["shell", "run-as", "com.lingxi.code.direct.debug", "readlink", "-f"]:
                    output(device[5])
                if device[:3] == ["shell", "pidof", "com.lingxi.code.direct.debug"]: output("", 1)
                if device[:3] == ["shell", "run-as", "com.lingxi.code.direct.debug"] and device[3:5] == ["test", "-e"]:
                    if device[5] == "files/local-app-owner-death-ci-capability" and state["capability"] is not None:
                        output("")
                    output("", 1)
                if device[:5] == ["shell", "run-as", "com.lingxi.code.direct.debug", "sh", "-c"]:
                    match = re.fullmatch(r"printf '%s' '([0-9a-f]{64})' > 'files/local-app-owner-death-ci-capability'", device[5])
                    if not match: output("invalid fake capability write", 2)
                    state["capability"] = match.group(1)
                    save()
                    output()
                if device[:3] == ["shell", "run-as", "com.lingxi.code.direct.debug"] and device[3] == "cat":
                    path = device[4]
                    if path == "files/local-app-owner-death-acceptance.json":
                        if state["receipt"] is None: output("missing", 1)
                        if state["partial_receipt_reads"]:
                            state["partial_receipt_reads"] -= 1
                            state["partial_receipt_served"] += 1
                            save()
                            output('{"ownerDeathPhase":', newline=False)
                        if state["atomic_gap_receipt_reads"]:
                            state["atomic_gap_receipt_reads"] -= 1
                            state["receipt_read_started"] = True
                            save()
                            deadline = time.monotonic() + 10
                            while time.monotonic() < deadline:
                                state = json.loads(state_path.read_text())
                                if state["prepare_instrumentation_finished"]: break
                                time.sleep(0.01)
                            if not state["prepare_instrumentation_finished"]:
                                output("fake prepare did not exit during receipt read", 2)
                            time.sleep(0.3)
                            state = json.loads(state_path.read_text())
                            state["atomic_gap_served"] += 1
                            save()
                            output("cat: No such file or directory", 1)
                        if state["phase"] == "prepare" and state["atomic_gap_served"]:
                            state["post_exit_prepare_receipt_reads"] += 1
                            save()
                        output(json.dumps(state["receipt"]))
                    if path == "files/local-app-owner-death-ci-capability":
                        if state["capability"] is None: output("missing", 1)
                        output(state["capability"], newline=False)
                    if path == state["config"] + "/" + ".owner-death-acceptance-owner": output(state["token"])
                    if path == state["dataPath"] + "/acceptance.bin": output(state["data"], newline=False)
                    if path == state["marker"]: output(json.dumps({"pid": 4201, "startTicks": "901", "starts": 1}))
                    if path == "/proc/4101/stat" and state["hostAlive"]: output(stat_line(4101, 900))
                    if path == "/proc/4102/stat" and state["recoveryAlive"]: output(stat_line(4102, 902))
                    if path == "/proc/4201/stat" and state["buildAlive"]: output(stat_line(4201, 901))
                    output("cat: No such file or directory", 1)
                if device[0:3] == ["shell", "am", "instrument"]:
                    phase = device[device.index("ownerDeathPhase") + 1]
                    capability = device[device.index("ownerDeathCiCapability") + 1]
                    run_nonce = device[device.index("ownerDeathRunNonce") + 1]
                    if capability != state["capability"]: output("missing or stale CI capability", 2)
                    state["capability"] = None
                    state["phase"] = phase
                    state["run_nonce"] = run_nonce
                    receipt = {
                        "ownerDeathPhase": "prepared" if phase == "prepare" else "recovered",
                        "ownerDeathRunNonce": run_nonce,
                        "configCreated": True, "ownerMarkerWritten": True,
                        "configHome": state["config"], "configToken": state["token"],
                        "source": state["source"], "markerPath": state["marker"],
                        "dataPath": state["dataPath"], "data": state["data"], "appId": "test-app",
                        "hostProcessName": "com.lingxi.code.direct.debug",
                        "recoveryProcessName": "com.lingxi.code.direct.debug",
                        "hostPid": 4101, "hostStartTicks": "900",
                        "prepareSelfKillRequested": True, "buildPid": 4201,
                        "buildStartTicks": "901", "buildStarts": 1,
                        "currentRevision": 1, "candidateRevision": 2,
                        "recoveryComplete": phase == "recover", "appRemoved": phase == "recover",
                        "backendPortsClosed": phase == "recover", "candidateBuildStopped": phase == "recover",
                        "recoveryHostPid": 4102, "recoveryHostStartTicks": "902",
                        "recoverySelfKillRequested": phase == "recover",
                    }
                    state["receipt"] = receipt
                    state["recoveryAlive"] = False
                    if phase == "prepare":
                        state["hostAlive"] = True
                        state["buildAlive"] = True
                        state["partial_receipt_reads"] = 1
                        state["atomic_gap_receipt_reads"] = 1
                    else:
                        state["hostAlive"] = False
                        state["buildAlive"] = False
                    save()
                    if phase == "prepare":
                        deadline = time.monotonic() + 10
                        while time.monotonic() < deadline:
                            state = json.loads(state_path.read_text())
                            if state["receipt_read_started"]: break
                            time.sleep(0.01)
                        if not state["receipt_read_started"]:
                            output("fake receipt read did not start", 2)
                        state["hostAlive"] = False
                        state["buildAlive"] = False
                        state["prepare_instrumentation_finished"] = True
                        save()
                    output("expected self-SIGKILL", 1)
                output("unsupported fake adb command", 2)
            ''').lstrip(), encoding="utf-8")
            fake_adb.chmod(0o755)
            environment = dict(os.environ)
            environment.update({
                "PATH": str(fake_bin) + os.pathsep + environment["PATH"],
                "CI": "true", "GITHUB_ACTIONS": "true", "RUNNER_ENVIRONMENT": "github-hosted",
                "FAKE_DEVICE_STATE": str(state_path), "FAKE_DEVICE_EVENTS": str(event_path),
            })
            output_dir = root / "runner-output"
            result = subprocess.run([
                sys.executable, str(SCRIPTS / "test-local-app-owner-death.py"),
                "--serial", "emulator-5554", "--ci-ephemeral-emulator",
                "--output-dir", str(output_dir),
            ], env=environment, text=True, capture_output=True, timeout=30)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            self.assertIn("PASS: self-SIGKILL", result.stdout)
            events = [json.loads(line) for line in event_path.read_text().splitlines()]
            self.assertFalse(any("kill" in event or "rm" in event or "force-stop" in event for event in events))
            retained = json.loads((output_dir / "owner-death-retained-config.json").read_text())
            self.assertEqual(retained["configHome"], str(CONFIG))
            self.assertTrue(state_path.exists())
            final_state = json.loads(state_path.read_text())
            self.assertIsNotNone(final_state["receipt"])
            self.assertEqual(final_state["partial_receipt_served"], 1)
            self.assertEqual(final_state["atomic_gap_served"], 1)
            self.assertGreaterEqual(final_state["post_exit_prepare_receipt_reads"], 1)
            self.assertIsNone(final_state["capability"])


if __name__ == "__main__":
    unittest.main()
