#!/usr/bin/env python3
"""Exercise emulator orchestration and specialized-test gates without Gradle or adb."""

import importlib.util
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import tarfile
import tempfile
import unittest


HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[3]
TESTS = ROOT / "apps/android/native/app/src/androidTest/java/com/lingxi/code"
spec = importlib.util.spec_from_file_location("owner_death", HERE / "test-local-app-owner-death.py")
owner_death = importlib.util.module_from_spec(spec)
spec.loader.exec_module(owner_death)


def method(source, name):
    start = re.search(r"\bfun\s+" + re.escape(name) + r"\s*\(", source)
    if start is None:
        raise AssertionError(f"missing test method {name}")
    following = re.search(r"\n\s*@Test\b", source[start.end():])
    return source[start.start():start.end() + following.start() if following else len(source)]


class AndroidCIEmulatorTests(unittest.TestCase):
    def test_owner_death_runner_requires_both_opt_in_and_recovery_marker(self):
        for phase in ("prepare", "recover"):
            args = owner_death.instrumentation_args("TestClass", "pkg.test/Runner", phase)
            self.assertEqual(args[args.index("ownerDeathAcceptance") + 1], "true")
            self.assertEqual(args[args.index("ownerDeathPhase") + 1], phase)
        with self.assertRaises(ValueError):
            owner_death.instrumentation_args("TestClass", "pkg.test/Runner", "invalid")
        self.assertFalse(owner_death.recovery_passed("OK (1 test)\n"))
        self.assertFalse(owner_death.recovery_passed("INSTRUMENTATION_RESULT: ownerDeathResult=RECOVERED\n"))
        self.assertFalse(owner_death.recovery_passed("INSTRUMENTATION_RESULT: ownerDeathResult=RECOVERED\nOK (1 test)\nFAILURES!!!\n"))
        self.assertTrue(owner_death.recovery_passed("INSTRUMENTATION_RESULT: ownerDeathResult=RECOVERED\r\nOK (1 test)\r\n"))

    def test_only_prerequisite_dependent_cases_have_explicit_gates(self):
        apps = TESTS / "apps"
        agent = (apps / "LocalAppAgentAcceptanceTest.kt").read_text(encoding="utf-8")
        flags = {
            "cleanupInterruptedSkillFixtureProvider": "localAppAgentCleanup",
            "stageCalorieAppForInteractiveAcceptance": "localAppAgentInteractive",
            "externalBrowserFileTaskUsesPublicFileApi": "localAppAgentLive",
            "externalBrowserPersistentChatStoresHistoryAndIsolatesThreads": "localAppAgentLive",
            "nativeQuestionApprovalAndTaskCancellationAreTaskScoped": "localAppAgentLive",
            "externalBrowserApprovedSkillRunsInBackgroundAndRecoversThroughNativeWakeLink": "localAppAgentSkillFixture",
            "externalGatewayRejectsInvalidRunAgentInput": "localAppAgentLive",
            "exactMcpGrantRequiresNativeApprovalAndRevokeFencesTask": "localAppAgentMcpFixture",
        }
        for name, flag in flags.items():
            with self.subTest(name=name):
                body = method(agent, name)
                self.assertRegex(body, rf'requireAcceptanceOptIn\(\s*"{flag}"')
                self.assertIn(f'com.lingxi.code.apps.LocalAppAgentAcceptanceTest#{name}', body)
                if flag in ("localAppAgentLive", "localAppAgentInteractive", "localAppAgentMcpFixture"):
                    self.assertIn('"agentProvider", "agentModel"', body)
        self.assertIn('acceptanceArgument("fixtureProviderKey")', method(agent, "externalBrowserApprovedSkillRunsInBackgroundAndRecoversThroughNativeWakeLink"))
        self.assertIn('acceptanceArgument("fixtureProviderUrl")', method(agent, "externalBrowserApprovedSkillRunsInBackgroundAndRecoversThroughNativeWakeLink"))
        self.assertIn('acceptanceArgument("fixtureNonce")', method(agent, "exactMcpGrantRequiresNativeApprovalAndRevokeFencesTask"))
        market = (apps / "UnifiedLocalAppsAcceptanceTest.kt").read_text(encoding="utf-8")
        selected = method(market, "marketInstallUpdateRemixReviewAndTakedown")
        self.assertRegex(selected, r'requireAcceptanceOptIn\(\s*"localAppMarketAcceptance"')
        self.assertIn('acceptanceArgument("localAppMarketRoot")', selected)
        self.assertIn('UnifiedLocalAppsAcceptanceTest#marketInstallUpdateRemixReviewAndTakedown", "localAppMarketRoot"', selected)
        for name in ("externalBrowserPhotoPickerReturnsBrowserFile", "externalBrowserStorageFollowsInstallationOriginAcrossRestart"):
            self.assertNotIn("requireAcceptanceOptIn", method(market, name))
        for name in ("backgroundOptInOwnsForegroundServiceAndOptOutSuspendsOnlyThisRun", "staticPythonNodeAndOwnedLifecycle"):
            self.assertIn("BuildConfig.MOBILE_LINUX_FULL", method(market, name))
        for path, name in (("UnifiedLocalAppFailureMatrixTest.kt", "preparationWriteAndDownloadFailuresKeepServingReleaseAndData"),
                           ("UnifiedLocalAppRootfsRepairTest.kt", "repairRestoresImmutableRootfsAndManifestWithoutChangingApplication")):
            self.assertIn("BuildConfig.MOBILE_LINUX_FULL", method((apps / path).read_text(encoding="utf-8"), name))
        engine = (TESTS / "EngineRoundtripTest.kt").read_text(encoding="utf-8")
        for name in ("keylessSendPromptDeliversTerminalEvent", "keylessListModelsDeliversModelList"):
            self.assertNotIn("requireAcceptanceOptIn", method(engine, name))
        for path in ("UnifiedLocalAppAuthoringTest.kt", "LocalAppDistributionChannelTest.kt"):
            self.assertNotIn("requireAcceptanceOptIn", (apps / path).read_text(encoding="utf-8"))
        owner = (apps / "UnifiedLocalAppProcessDeathTest.kt").read_text(encoding="utf-8")
        owner_test = method(owner, "hostSigkillRequiresFreshRunAndPreservesData")
        self.assertRegex(owner_test, r'requireAcceptanceOptIn\(\s*"ownerDeathAcceptance"')
        self.assertIn('UnifiedLocalAppProcessDeathTest#hostSigkillRequiresFreshRunAndPreservesData", "ownerDeathPhase"', owner_test)
        self.assertIn('putString("ownerDeathResult", "RECOVERED")', owner_test)
        runtime = (TESTS / "runtime/MobileLinuxProductAcceptanceTest.kt").read_text(encoding="utf-8")
        for name, flag, marker, test_class in (
            ("defaultUserProjectTerminalAndPersistentHome", "mobileLinuxProductAcceptance", "mobileLinuxProductResult", "MobileLinuxProductAcceptanceTest"),
            ("coldProcessRetainsUserHome", "mobileLinuxColdStartAcceptance", "mobileLinuxColdStartResult", "MobileLinuxProductColdStartAcceptanceTest"),
        ):
            self.assertRegex(method(runtime, name), rf'requireAcceptanceOptIn\(\s*"{flag}"')
            self.assertIn(f'putString("{marker}"', method(runtime, name))
            self.assertIn(f'com.lingxi.code.runtime.{test_class}#{name}', method(runtime, name))
        diagnostics = (apps / "LocalAppExitDiagnosticsTest.kt").read_text(encoding="utf-8")
        self.assertIn("com.lingxi.code.apps.LocalAppExitDiagnosticsTest#exportOwnAnr", method(diagnostics, "exportOwnAnr"))
        self.assertIn('"fixtureProviderUrl", "fixtureProviderKey", "fixtureProviderModel"',
                      method(agent, "externalBrowserApprovedSkillRunsInBackgroundAndRecoversThroughNativeWakeLink"))
        self.assertIn('"fixtureNonce", "mcpDiscovery"', method(agent, "exactMcpGrantRequiresNativeApprovalAndRevokeFencesTask"))
        self.assertIn('"fixtureCleanupProviderId"', method(agent, "cleanupInterruptedSkillFixtureProvider"))

    def make_environment(self, root):
        android = root / "apps/android/native"
        scripts = android / "scripts"
        scripts.mkdir(parents=True)
        for name in ("run-ci-emulator-tests.sh", "stage-ci-android-fixtures.sh"):
            shutil.copyfile(HERE / name, scripts / name)
        shutil.copytree(ROOT / "fixtures/local-apps/v2", root / "fixtures/local-apps/v2")
        trace = root / "trace.jsonl"
        fake_bin = root / "bin"
        fake_bin.mkdir()
        gradle = android / "gradlew"
        gradle.write_text('''#!/usr/bin/env python3
import json, os, sys
with open(os.environ["CI_TRACE"], "a") as f: f.write(json.dumps(["gradle"] + sys.argv[1:]) + "\\n")
''', encoding="utf-8")
        gradle.chmod(0o755)
        adb = fake_bin / "adb"
        adb.write_text('''#!/usr/bin/env python3
import json, os, pathlib, sys
args = sys.argv[1:]
with open(os.environ["CI_TRACE"], "a") as f: f.write(json.dumps(["adb"] + args) + "\\n")
if args == ["get-serialno"]:
    print("emulator-5554")
elif "mkdir" in args and args[-1] == "files/local-app-acceptance-fixtures" and os.environ.get("CI_EXISTING_FIXTURE") == "1":
    sys.exit(1)
elif "tar" in args:
    package = args[args.index("run-as") + 1]
    pathlib.Path(os.environ["CI_FAKE_DIR"], package + ".tar").write_bytes(sys.stdin.buffer.read())
elif "instrument" in args:
    if "com.lingxi.code.EngineRoundtripTest" in args:
        if os.environ.get("CI_MISSING_KEYLESS_MARKER") != "1":
            print("INSTRUMENTATION_RESULT: keylessTerminalResult=PASS")
            print("INSTRUMENTATION_RESULT: keylessModelListResult=PASS")
        print("OK (2 tests)")
    else:
      if os.environ.get("CI_MISSING_MARKER") != "1":
        marker = "mobileLinuxColdStartResult=VERIFIED" if "mobileLinuxColdStartAcceptance" in args else "mobileLinuxProductResult=READY"
        print("INSTRUMENTATION_RESULT: " + marker)
      print("OK (1 test)")
''', encoding="utf-8")
        adb.chmod(0o755)
        owner = scripts / "test-local-app-owner-death.py"
        owner.write_text('''import json, os, sys
with open(os.environ["CI_TRACE"], "a") as f: f.write(json.dumps(["owner"] + sys.argv[1:]) + "\\n")
if os.environ.get("CI_OWNER_FAIL") == "1": sys.exit(1)
''', encoding="utf-8")
        env = {**os.environ, "PATH": f"{fake_bin}:{os.environ['PATH']}", "RUNNER_TEMP": str(root / "runner temp"),
               "CI_TRACE": str(trace), "CI_FAKE_DIR": str(root)}
        return android, trace, env

    def test_emulator_stages_both_flavors_and_runs_real_targets(self):
        with tempfile.TemporaryDirectory(prefix="android ci emulator ") as temporary:
            root = Path(temporary)
            android, trace, env = self.make_environment(root)
            evidence = root / "rootfs evidence"
            subprocess.run(["bash", "scripts/run-ci-emulator-tests.sh", str(evidence)],
                           cwd=android, env=env, check=True, capture_output=True)
            events = [json.loads(line) for line in trace.read_text(encoding="utf-8").splitlines()]
            gradle_calls = [event for event in events if event[0] == "gradle"]
            self.assertEqual(len(gradle_calls), 2)
            for call in gradle_calls:
                self.assertIn("--max-workers=1", call)
                self.assertIn("-Pkotlin.daemon.jvmargs=-Xmx4g", call)
                self.assertIn(f"-PlingxiRootfsEvidenceDir={evidence}", call)
            self.assertTrue({":app:installPlayDebug", ":app:installPlayDebugAndroidTest",
                             ":app:installDirectDebug", ":app:installDirectDebugAndroidTest"} <= set(gradle_calls[0]))
            self.assertTrue({":app:connectedPlayDebugAndroidTest", ":app:connectedDirectDebugAndroidTest"} <= set(gradle_calls[1]))
            self.assertNotIn("-x", gradle_calls[1])
            self.assertFalse(any("testInstrumentationRunnerArguments" in arg for arg in gradle_calls[1]))
            instrument = [event for event in events if "instrument" in event]
            self.assertEqual(len(instrument), 6)
            self.assertEqual(sum("mobileLinuxProductAcceptance" in event for event in instrument), 2)
            self.assertEqual(sum("mobileLinuxColdStartAcceptance" in event for event in instrument), 2)
            self.assertEqual(sum("com.lingxi.code.EngineRoundtripTest" in event for event in instrument), 2)
            self.assertEqual(sum("force-stop" in event for event in events), 2)
            owner = [event for event in events if event[0] == "owner"]
            self.assertEqual(len(owner), 1)
            self.assertIn("emulator-5554", owner[0])
            for package in ("com.lingxi.code.debug", "com.lingxi.code.direct.debug"):
                self.assertTrue(any("com.lingxi.code.EngineRoundtripTest" in event and
                                    f"{package}.test/androidx.test.runner.AndroidJUnitRunner" in event
                                    for event in instrument))
                with tarfile.open(root / f"{package}.tar") as archive:
                    names = set(archive.getnames())
                    self.assertIn("./static/lingxi-app.json", names)
                    self.assertIn("./python/server.py", names)
                    self.assertIn("./node/package-lock.json", names)
                    self.assertIn("./agent/public-apple.jpg", names)
                self.assertTrue(any(package in event and "cp" in event and
                                    "files/local-app-acceptance-fixtures/test-photo.jpg" in event for event in events))

    def test_missing_runtime_marker_or_owner_failure_cannot_pass(self):
        for failure in ("CI_MISSING_MARKER", "CI_MISSING_KEYLESS_MARKER", "CI_OWNER_FAIL"):
            with self.subTest(failure=failure), tempfile.TemporaryDirectory(prefix="android ci failure ") as temporary:
                root = Path(temporary)
                android, trace, env = self.make_environment(root)
                env[failure] = "1"
                result = subprocess.run(["bash", "scripts/run-ci-emulator-tests.sh", str(root / "rootfs")],
                                        cwd=android, env=env, capture_output=True)
                self.assertNotEqual(result.returncode, 0)
                calls = [json.loads(line) for line in trace.read_text(encoding="utf-8").splitlines()]
                self.assertFalse(any(":app:connectedPlayDebugAndroidTest" in call for call in calls))

    def test_staging_rejects_unknown_package_and_missing_fixture_before_adb(self):
        with tempfile.TemporaryDirectory(prefix="android ci stage ") as temporary:
            root = Path(temporary)
            android, trace, env = self.make_environment(root)
            env["ANDROID_SERIAL"] = "emulator-5554"
            result = subprocess.run(["bash", "scripts/stage-ci-android-fixtures.sh", "com.example.other"], cwd=android, env=env, capture_output=True)
            self.assertNotEqual(result.returncode, 0)
            (root / "fixtures/local-apps/v2/python/server.py").unlink()
            result = subprocess.run(["bash", "scripts/stage-ci-android-fixtures.sh", "com.lingxi.code.debug"], cwd=android, env=env, capture_output=True)
            self.assertNotEqual(result.returncode, 0)
            self.assertFalse(trace.exists())

    def test_staging_does_not_overwrite_an_existing_fixture_directory(self):
        with tempfile.TemporaryDirectory(prefix="android ci existing ") as temporary:
            root = Path(temporary)
            android, trace, env = self.make_environment(root)
            env.update(ANDROID_SERIAL="emulator-5554", CI_EXISTING_FIXTURE="1")
            result = subprocess.run(["bash", "scripts/stage-ci-android-fixtures.sh", "com.lingxi.code.debug"],
                                    cwd=android, env=env, capture_output=True)
            self.assertNotEqual(result.returncode, 0)
            events = [json.loads(line) for line in trace.read_text(encoding="utf-8").splitlines()]
            self.assertFalse(any("tar" in event or "cp" in event for event in events))


if __name__ == "__main__":
    unittest.main()
