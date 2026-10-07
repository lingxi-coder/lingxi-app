#!/usr/bin/env python3
"""Smoke routing and source-policy rejection regressions for the locked product."""
import contextlib
import importlib.util
import io
import json
import os
import plistlib
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest
import xml.etree.ElementTree as ET

HOST = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(HOST / "scripts/lib"))
from runtime_source import resolve_runtime
from local_app_source import resolve_local_app
from local_app_branding import (android_package, authorization_enabled_env,
                                branding_constant, ios_bundle_id, local_app_apk_env)

ANDROID_PACKAGE = android_package()
ANDROID_PACKAGE_PATH = Path(*ANDROID_PACKAGE.split("."))
IOS_BUNDLE_ID = ios_bundle_id()
ENABLED_ENV = authorization_enabled_env()
APK_ENV = local_app_apk_env()


class SmokeRoutingTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.host = self.root / "host"
        self.runtime = self.root / "harness"
        self.sdk = self.root / "sdk"
        self.log = self.root / "calls"
        self.env = {**os.environ, ENABLED_ENV: "0",
                    "POLICY_TEST_LOG": str(self.log), "POLICY_TEST_FAIL": "",
                    "PYTHONDONTWRITEBYTECODE": "1"}
        for name in ("MOBILE_LINUX_EVIDENCE_DIR", APK_ENV):
            self.env.pop(name, None)
        self.write(self.host / "scripts/local-apps/smoke.sh",
                   (HOST / "scripts/local-apps/smoke.sh").read_text())
        for name, path in (("runtime_source", self.runtime), ("mobile_linux_source", self.sdk)):
            self.write(self.host / f"scripts/lib/{name}.py", f"print({str(path)!r})\n")
        for name in ("check-authorizations", "check-store-compliance"):
            self.gate(self.host / f"scripts/local-apps/{name}.sh", f"host-{name}")
        self.python_gate(self.host / "scripts/local-apps/verify-local-app-host.py", "host-policy")
        for name in ("check-authorizations", "check-rootfs-manifest", "check-sbom-and-licenses"):
            self.gate(self.runtime / f"scripts/local-apps/{name}.sh", name)
        self.python_gate(self.runtime / "scripts/local-apps/verify-local-app-supply-chain.py", "supply-chain")
        # The host policy fixture exposes the SDK resource-contract entrypoint.
        self.gate(self.sdk / "scripts/checks/check-resource-contracts.sh", "sdk-contracts")

    def write(self, path, source):
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(source)
        path.chmod(0o755)

    def gate(self, path, name):
        self.write(path, f'''#!/usr/bin/env bash
printf '%s\\n' "{name} $*" >> "$POLICY_TEST_LOG"
[[ "$POLICY_TEST_FAIL" != "{name}" ]] || exit 1
''')

    def python_gate(self, path, name):
        self.write(path, f'''import os, sys
with open(os.environ['POLICY_TEST_LOG'], 'a') as log:
    log.write({name!r} + ' ' + ' '.join(sys.argv[1:]) + '\\n')
sys.exit(1 if os.environ['POLICY_TEST_FAIL'] == {name!r} else 0)
''')

    def run_smoke(self):
        result = subprocess.run(["bash", str(self.host / "scripts/local-apps/smoke.sh")],
                                env=self.env, capture_output=True, text=True)
        calls = self.log.read_text().splitlines() if self.log.exists() else []
        return result, calls

    def test_disabled_checks_current_sdk_and_all_existing_gates(self):
        result, calls = self.run_smoke()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual([call.split()[0] for call in calls], [
            "host-check-authorizations", "host-check-store-compliance", "host-policy",
            "check-authorizations", "sdk-contracts", "check-sbom-and-licenses", "supply-chain"])
        self.assertIn(f"--sdk-root {self.sdk}", calls[-1])
        self.assertNotIn("--release", calls[-1])

    def test_gate_failures_stop_before_later_gates(self):
        names = ["host-check-authorizations", "host-check-store-compliance", "host-policy",
                 "check-authorizations", "sdk-contracts", "check-sbom-and-licenses", "supply-chain"]
        for name in names:
            with self.subTest(gate=name):
                self.log.unlink(missing_ok=True)
                self.env["POLICY_TEST_FAIL"] = name
                result, calls = self.run_smoke()
                self.assertEqual(result.returncode, 1, result.stderr)
                self.assertEqual([call.split()[0] for call in calls], names[:names.index(name) + 1])

    def test_missing_current_sdk_contract_fails(self):
        (self.sdk / "scripts/checks/check-resource-contracts.sh").unlink()
        result, calls = self.run_smoke()
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse(any(call.startswith("supply-chain") for call in calls))

    def test_enabled_requires_evidence(self):
        self.env[ENABLED_ENV] = "1"
        result, calls = self.run_smoke()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("enabled release requires external evidence", result.stderr)
        self.assertFalse(any(call.startswith("sdk-contracts") for call in calls))

    def test_enabled_requires_apks_after_rootfs_and_licenses(self):
        self.env.update({ENABLED_ENV: "1", "MOBILE_LINUX_EVIDENCE_DIR": str(self.root / "evidence")})
        result, calls = self.run_smoke()
        self.assertEqual(result.returncode, 1)
        self.assertIn(f"{APK_ENV} is required", result.stderr)
        self.assertEqual([call.split()[0] for call in calls[-2:]], ["check-rootfs-manifest", "check-sbom-and-licenses"])
        self.assertFalse(any(call.startswith("supply-chain") for call in calls))

    def test_enabled_preserves_release_validation_and_rejections(self):
        self.env.update({ENABLED_ENV: "1", "MOBILE_LINUX_EVIDENCE_DIR": str(self.root / "evidence"),
                         APK_ENV: str(self.root / "apks")})
        result, calls = self.run_smoke()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn(f"--release --apk-dir {self.root / 'apks'}", calls[-1])
        for name in ("check-rootfs-manifest", "check-sbom-and-licenses", "supply-chain"):
            with self.subTest(gate=name):
                self.log.unlink()
                self.env["POLICY_TEST_FAIL"] = name
                result, calls = self.run_smoke()
                self.assertEqual(result.returncode, 1, result.stderr)
                self.assertEqual(calls[-1].split()[0], name)


class LockedProfilePolicyTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.resolved = resolve_runtime()
        cls.runtime = Path(cls.resolved["root"])
        cls.local_app = Path(resolve_local_app()["root"])
        cls.dot_dir = branding_constant("DOT_DIR", cls.resolved)
        spec = importlib.util.spec_from_file_location(
            "locked_local_app_policy", cls.runtime / "scripts/local-apps/verify-local-app-supply-chain.py")
        cls.verify = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(cls.verify)

    def test_current_consolidated_source_layout(self):
        self.assertEqual(self.resolved["source"],
                         f"git+https://github.com/lingxi-coder/harness-runtime.git?rev={self.resolved['revision']}#{self.resolved['revision']}")
        for name, layout in (("core", "core"), ("platform-common", "platforms/common"),
                             ("platform-android", "platforms/android"), ("platform-ios", "platforms/ios")):
            self.assertEqual(Path(self.resolved["packages"][name]), self.runtime / f"crates/{layout}/Cargo.toml")
        self.assertNotIn("protocol", self.resolved["packages"])

    def test_all_locked_profiles_reject_source_policy_expansion(self):
        for family in ("react-dom", "canvas-2d", "three-3d", "phaser-2d", "babylon-3d"):
            with self.subTest(family=family), tempfile.TemporaryDirectory() as temporary:
                template = Path(temporary) / "template"
                shutil.copytree(self.local_app / f"crates/local-apps/templates/runtime-profiles/{family}/r4", template)
                self.verify.validate_runtime_profile_source_policy(family, template)
                policy_path = template / self.dot_dir / "source-policy.json"
                policy = json.loads(policy_path.read_text())
                mutations = {
                    "writable roots": {**policy, "agent_writable_roots": policy["agent_writable_roots"] + ["."]},
                    "host-managed paths": {**policy, "host_managed_paths": []},
                    "forbidden features": {**policy, "forbidden_features": []},
                }
                for reason, mutated in mutations.items():
                    with self.subTest(reason=reason):
                        policy_path.write_text(json.dumps(mutated))
                        errors = io.StringIO()
                        with contextlib.redirect_stderr(errors), self.assertRaises(SystemExit) as failure:
                            self.verify.validate_runtime_profile_source_policy(family, template)
                        self.assertEqual(failure.exception.code, 1)
                        self.assertIn(reason, errors.getvalue())
                policy_path.write_text(json.dumps(policy))
                (template / "lib/lingxi-bridge.js").unlink()
                errors = io.StringIO()
                with contextlib.redirect_stderr(errors), self.assertRaises(SystemExit) as failure:
                    self.verify.validate_runtime_profile_source_policy(family, template)
                self.assertEqual(failure.exception.code, 1)
                self.assertIn("missing managed helper", errors.getvalue())



class MobileStorePolicyFixture(unittest.TestCase):
    """Exercise the complete gate on real declarations plus hostile fixtures."""
    ANDROID = "{http://schemas.android.com/apk/res/android}"
    MAIN = "apps/android/native/app/src/main/AndroidManifest.xml"
    DIRECT = "apps/android/native/app/src/direct/AndroidManifest.xml"

    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.host = self.root / "host"
        self.runtime = self.root / "harness"
        self.env = {**os.environ, "PYTHONDONTWRITEBYTECODE": "1"}
        for path in ("apps/android/native", "apps/ios/native", "apps/android/ffi", "apps/ios/ffi"):
            (self.host / path).mkdir(parents=True, exist_ok=True)
        for path in ("crates/platforms/android", "crates/platforms/ios"):
            (self.runtime / path).mkdir(parents=True)
        self.write("scripts/local-apps/check-store-compliance.sh",
                   (HOST / "scripts/local-apps/check-store-compliance.sh").read_text())
        # Run the real Host gates: their trusted identity source must remain
        # independent of the mutable --repo-root supplied by this fixture.
        for name in ("check-android-special-use.py", "check-ios-background-policy.py"):
            self.write(f"scripts/local-apps/{name}",
                       f"import runpy\nrunpy.run_path({str(HOST / 'scripts/local-apps' / name)!r}, run_name='__main__')\n")
        for path in ("apps/android/native/app/build.gradle.kts", "apps/ios/native/project.yml"):
            self.write(path, (HOST / path).read_text())
        self.write("scripts/lib/runtime_source.py", f"print({str(self.runtime)!r})\n")
        for path in (self.MAIN, self.DIRECT):
            self.write(path, (HOST / path).read_text())
        self.write("apps/ios/native/Info.plist", (HOST / "apps/ios/native/Info.plist").read_text())
        for source in ("Cron/CronModels.swift", "Cron/CronSystemAdapters.swift", "LocalApps/LocalAppsStore.swift",
                       "App/AppNotificationDelegate.swift", "App/ConversationBackgroundActivity.swift",
                       "Voice/VoiceAudioSessionCoordinator.swift", "App/RootView.swift"):
            path = f"apps/ios/native/Sources/{source}"
            self.write(path, (HOST / path).read_text())
        # The source check limits constant provenance; runtime semantics were
        # reviewed in the real implementation, not simulated by this fixture.
        for path in (
            Path("main/java") / ANDROID_PACKAGE_PATH / "conversation/ConversationBackgroundExecution.kt",
            Path("direct/java") / ANDROID_PACKAGE_PATH / "computeruse/ComputerUseSessionService.kt",
        ):
            self.write(f"apps/android/native/app/src/{path}", "val type = ServiceInfo.FOREGROUND_SERVICE_TYPE_SPECIAL_USE\n")

    def write(self, path, text):
        destination = self.host / path
        destination.parent.mkdir(parents=True, exist_ok=True)
        destination.write_text(text)

    def run_policy(self, mode="play", library=None):
        command = ["bash", str(self.host / "scripts/local-apps/check-store-compliance.sh"), mode]
        if library:
            command.append(str(library))
        return subprocess.run(command, capture_output=True, text=True, env=self.env)

    def assert_rejected(self, mode="play", reason="specialUse"):
        result = self.run_policy(mode)
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertIn(reason, result.stderr)
        self.assertNotIn("Traceback (most recent call last)", result.stderr)

    def mutate_main(self, mutate):
        path = self.host / self.MAIN
        tree = ET.parse(path)
        mutate(tree.getroot())
        tree.write(path, encoding="unicode")


class AndroidSpecialUsePolicyTests(MobileStorePolicyFixture):
    def test_real_conversation_and_direct_declarations_pass(self):
        for mode in ("play", "direct"):
            with self.subTest(mode=mode):
                result = self.run_policy(mode)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertIn(f"{mode} store-compliance scan passed", result.stdout)

    def test_formatting_and_namespace_prefix_do_not_change_policy(self):
        for qualified in (False, True):
            with self.subTest(qualified=qualified):
                self.write(self.MAIN, (HOST / self.MAIN).read_text())
                def mutate(root):
                    if qualified:
                        root.set("package", ANDROID_PACKAGE)
                        root.find("application/service").set(
                            self.ANDROID + "name", ANDROID_PACKAGE + ".conversation.ConversationTurnService")
                self.mutate_main(mutate)  # ElementTree emits ns0 attributes.
                self.assertEqual(self.run_policy().returncode, 0)

    def test_exported_missing_export_or_wrong_lifecycle_rejected(self):
        original = (HOST / self.MAIN).read_text()
        for attribute, value in (("exported", "true"), ("exported", None), ("stopWithTask", "true")):
            with self.subTest(attribute=attribute, value=value):
                self.write(self.MAIN, original)
                def mutate(root):
                    node = root.find("application/service")
                    if value is None:
                        node.attrib.pop(self.ANDROID + attribute)
                    else:
                        node.set(self.ANDROID + attribute, value)
                self.mutate_main(mutate)
                self.assert_rejected()

    def test_unknown_service_types_and_subtypes_rejected(self):
        original = (HOST / self.MAIN).read_text()
        for attribute, value in (("name", ".UnknownService"), ("foregroundServiceType", "specialUse|dataSync"),
                                 ("foregroundServiceType", "specialUseDeceptive"), ("foregroundServiceType", "dataSync")):
            with self.subTest(attribute=attribute, value=value):
                self.write(self.MAIN, original)
                self.mutate_main(lambda root: root.find("application/service").set(self.ANDROID + attribute, value))
                self.assert_rejected()
        for value in ("", "Background work", "User-started Android Computer Use control session"):
            with self.subTest(subtype=value):
                self.write(self.MAIN, original)
                self.mutate_main(lambda root: root.find("application/service/property").set(self.ANDROID + "value", value))
                self.assert_rejected()
        self.write(self.MAIN, original)
        self.mutate_main(lambda root: root.find("application/service").remove(root.find("application/service/property")))
        self.assert_rejected()

    def test_missing_duplicate_and_unknown_permissions_rejected(self):
        original = (HOST / self.MAIN).read_text()
        for mutation in ("missing", "duplicate", "unknown", "sdk-23", "missing-base", "capped"):
            with self.subTest(mutation=mutation):
                self.write(self.MAIN, original)
                def mutate(root):
                    permission = next(node for node in root.findall("uses-permission")
                                      if node.get(self.ANDROID + "name", "").endswith("FOREGROUND_SERVICE_SPECIAL_USE"))
                    if mutation == "missing":
                        root.remove(permission)
                    elif mutation == "duplicate":
                        ET.SubElement(root, permission.tag, permission.attrib.copy())
                    elif mutation == "unknown":
                        permission.set(self.ANDROID + "name", "android.permission.FOREGROUND_SERVICE_SPECIAL_USE_UNKNOWN")
                    elif mutation == "sdk-23":
                        permission.tag = "uses-permission-sdk-23"
                    elif mutation == "capped":
                        permission.set(self.ANDROID + "maxSdkVersion", "33")
                    else:
                        base = next(node for node in root.findall("uses-permission")
                                    if node.get(self.ANDROID + "name") == "android.permission.FOREGROUND_SERVICE")
                        root.remove(base)
                self.mutate_main(mutate)
                self.assert_rejected()

    def test_duplicate_service_intent_filter_and_merger_directive_rejected(self):
        original = (HOST / self.MAIN).read_text()
        for mutation in ("duplicate", "intent", "merger", "package"):
            with self.subTest(mutation=mutation):
                self.write(self.MAIN, original)
                def mutate(root):
                    service = root.find("application/service")
                    if mutation == "duplicate":
                        root.find("application").append(ET.fromstring(ET.tostring(service)))
                    elif mutation == "intent":
                        ET.SubElement(service, "intent-filter")
                    elif mutation == "merger":
                        service.set("{http://schemas.android.com/tools}node", "replace")
                    else:
                        root.set("package", "com.deceptive")
                self.mutate_main(mutate)
                self.assert_rejected()
        # Changing the fixture's Gradle config along with its manifest cannot
        # authorize a new package, qualified service or constant location.
        unreviewed = "com.deceptive"
        config = "apps/android/native/app/build.gradle.kts"
        self.write(config, (HOST / config).read_text().replace(ANDROID_PACKAGE, unreviewed))
        self.write(self.MAIN, original.replace(ANDROID_PACKAGE, unreviewed))
        self.mutate_main(lambda root: root.set("package", unreviewed))
        self.assert_rejected(reason="unreviewed service package")
        self.write(self.MAIN, original)
        self.mutate_main(lambda root: root.find("application/service").set(
            self.ANDROID + "name", unreviewed + ".conversation.ConversationTurnService"))
        self.assert_rejected()

    def test_overlay_cannot_export_or_replace_conversation(self):
        for name in (".conversation.ConversationTurnService", ANDROID_PACKAGE + ".conversation.ConversationTurnService"):
            with self.subTest(name=name):
                self.write("apps/android/native/app/src/play/AndroidManifest.xml", f'''<manifest xmlns:android="http://schemas.android.com/apk/res/android">
<application><service android:name="{name}" android:exported="true" /></application></manifest>''')
                self.assert_rejected()

    def test_permission_only_or_unknown_service_in_other_manifest_rejected(self):
        for declaration in (
            '<uses-permission android:name="android.permission.FOREGROUND_SERVICE_SPECIAL_USE" />',
            '<application><service android:name=".Deceptive" android:exported="false" android:foregroundServiceType="specialUse" /></application>',
        ):
            with self.subTest(declaration=declaration):
                self.write("apps/android/native/app/src/play/AndroidManifest.xml",
                           '<manifest xmlns:android="http://schemas.android.com/apk/res/android">' + declaration + '</manifest>')
                self.assert_rejected()

    def test_unknown_source_references_rejected_even_in_reviewed_file(self):
        paths = ("apps/android/native/app/src/main/java/Unknown.kt",
                 Path("apps/android/native/app/src/main/java") / ANDROID_PACKAGE_PATH / "conversation/ConversationBackgroundExecution.kt",
                 "apps/android/ffi/unknown.rs")
        for path in paths:
            with self.subTest(path=path):
                self.write(path, "val type = ServiceInfo.FOREGROUND_SERVICE_TYPE_SPECIAL_USE\nval other = \"FOREGROUND_SERVICE_SPECIAL_USE\"\n")
                self.assert_rejected()
                (self.host / path).unlink()

    def test_malformed_or_missing_manifest_fails_closed(self):
        self.write(self.MAIN, "<manifest")
        self.assert_rejected()
        (self.host / self.MAIN).unlink()
        self.assert_rejected(reason="missing reviewed Android manifest")

    def test_device_control_declaration_is_rejected_outside_direct(self):
        self.write("apps/android/native/app/src/play/AndroidManifest.xml", (HOST / self.DIRECT).read_text())
        self.assert_rejected(reason="Android privilege escalation")

    def test_direct_requires_binding_and_exact_control_types(self):
        original = (HOST / self.DIRECT).read_text()
        for old, new in (("BIND_ACCESSIBILITY_SERVICE", "BIND_OTHER_SERVICE"),
                         ("mediaProjection|microphone|specialUse", "specialUse"),
                         ("User-started Android Computer Use control session", "Background work"),
                         (branding_constant("PRODUCT_NAME") + "AccessibilityService", "OtherAccessibilityService")):
            with self.subTest(old=old):
                self.write(self.DIRECT, original.replace(old, new))
                self.assert_rejected("direct")

    def test_unrelated_privilege_patterns_remain_rejected(self):
        for mode in ("play", "direct"):
            for token in ("Shizuku", "SYSTEM_ALERT_WINDOW", "ACTION_MANAGE_OVERLAY_PERMISSION",
                          "REQUEST_INSTALL_PACKAGES", "PackageInstaller", "installPackage(", "DexClassLoader",
                          "PathClassLoader", "InMemoryDexClassLoader", "pm install", "QUERY_ALL_PACKAGES",
                          "REQUEST_IGNORE_BATTERY_OPTIMIZATIONS", "ACTION_REQUEST_IGNORE_BATTERY_OPTIMIZATIONS",
                          "FOREGROUND_SERVICE_MEDIA_PLAYBACK", "ServiceInfo.FOREGROUND_SERVICE_TYPE_MEDIA_PLAYBACK",
                          'foregroundServiceType="mediaPlayback"'):
                with self.subTest(mode=mode, token=token):
                    self.write("apps/android/native/app/src/main/java/Forbidden.kt", token)
                    self.assert_rejected(mode, reason="forbidden Android")
        for token in ("AccessibilityService", "FOREGROUND_SERVICE_MEDIA_PROJECTION"):
            self.write("apps/android/native/app/src/main/java/Forbidden.kt", token)
            self.assert_rejected(reason="Android privilege escalation")
        (self.host / "apps/android/native/app/src/main/java/Forbidden.kt").unlink()
        self.write("apps/ios/native/Forbidden.swift", 'let declaration = "UIBackgroundModes"')
        self.assert_rejected(reason="unreviewed iOS background declaration")

    def test_audio_variable_names_are_allowed_but_media_fgs_xml_is_not(self):
        self.write("apps/android/native/app/src/main/java/Audio.kt", "val mediaPlayback = ConcurrentHashMap()\n")
        self.assertEqual(self.run_policy().returncode, 0)
        self.write("apps/android/native/app/src/play/AndroidManifest.xml", '''<manifest xmlns:a="http://schemas.android.com/apk/res/android">
<application><service a:name=".FakeMedia" a:foregroundServiceType="media&#80;layback" /></application></manifest>''')
        self.assert_rejected(reason="forbidden mediaPlayback foreground service")

    def test_scan_errors_fail_closed(self):
        script = self.host / "scripts/local-apps/check-store-compliance.sh"
        script.write_text(script.read_text().replace("REQUEST_INSTALL_PACKAGES|PackageInstaller", "(|PackageInstaller"))
        self.assert_rejected(reason="compliance scan failed")

    def test_native_library_flavor_gate_remains_enforced(self):
        library = self.root / "lib.so"
        library.write_text("android_use fixture\n")
        self.assertEqual(self.run_policy("play", library).returncode, 1)
        self.assertEqual(self.run_policy("direct", library).returncode, 0)
        library.write_text("no control symbols\n")
        self.assertEqual(self.run_policy("play", library).returncode, 0)
        self.assertEqual(self.run_policy("direct", library).returncode, 1)



class IOSBackgroundPolicyTests(MobileStorePolicyFixture):
    INFO = "apps/ios/native/Info.plist"

    def mutate_info(self, mutate, fmt=plistlib.FMT_XML):
        path = self.host / self.INFO
        info = plistlib.loads(path.read_bytes())
        mutate(info)
        path.write_bytes(plistlib.dumps(info, fmt=fmt))

    def test_processing_and_foreground_tts_pass_xml_and_binary(self):
        for fmt in (plistlib.FMT_XML, plistlib.FMT_BINARY):
            with self.subTest(format=fmt):
                self.mutate_info(lambda info: None, fmt)
                self.write("apps/ios/native/Sources/Voice/ForegroundTTS.swift", '''
let session = AVAudioSession.sharedInstance()
try session.setCategory(.playback, mode: .spokenAudio)
try session.setActive(true)
// Background audio would require UIBackgroundModes=audio.
let endpoint = "silence detected"
''')
                result = self.run_policy()
                self.assertEqual(result.returncode, 0, result.stderr)

    def test_planted_background_audio_is_rejected_xml_and_binary(self):
        original = (HOST / self.INFO).read_bytes()
        for fmt in (plistlib.FMT_XML, plistlib.FMT_BINARY):
            for modes in (["audio"], ["processing", "audio"]):
                with self.subTest(format=fmt, modes=modes):
                    (self.host / self.INFO).write_bytes(original)
                    self.mutate_info(lambda info: info.update(UIBackgroundModes=modes), fmt)
                    self.assert_rejected(reason="forbidden iOS audio background mode")

    def test_unknown_modes_and_bad_identifiers_fail_closed(self):
        original = (HOST / self.INFO).read_bytes()
        for field, values in (
            ("UIBackgroundModes", ["processing", "location"]), ("UIBackgroundModes", ["processing", "processing"]),
            ("UIBackgroundModes", "processing"), ("UIBackgroundModes", [True]), ("UIBackgroundModes", []),
            ("BGTaskSchedulerPermittedIdentifiers", []), ("BGTaskSchedulerPermittedIdentifiers", ["*"]),
            ("BGTaskSchedulerPermittedIdentifiers", [IOS_BUNDLE_ID + ".unknown"]),
            ("BGTaskSchedulerPermittedIdentifiers", IOS_BUNDLE_ID + ".cron.reconcile"),
        ):
            with self.subTest(field=field, values=values):
                (self.host / self.INFO).write_bytes(original)
                self.mutate_info(lambda info: info.update({field: values}))
                self.assert_rejected(reason="iOS background policy")
        (self.host / self.INFO).write_bytes(original)
        self.mutate_info(lambda info: info["BGTaskSchedulerPermittedIdentifiers"].append(info["BGTaskSchedulerPermittedIdentifiers"][0]))
        self.assert_rejected(reason="duplicate BGTaskSchedulerPermittedIdentifiers")
        # A coordinated change to fixture config, plist and every source owner
        # must still be rejected against the trusted Host's base bundle ID.
        unreviewed = "com.deceptive"
        config = "apps/ios/native/project.yml"
        self.write(config, (HOST / config).read_text().replace(IOS_BUNDLE_ID, unreviewed))
        (self.host / self.INFO).write_bytes(original)
        self.mutate_info(lambda info: info.update(BGTaskSchedulerPermittedIdentifiers=[
            value.replace(IOS_BUNDLE_ID, unreviewed) for value in info["BGTaskSchedulerPermittedIdentifiers"]]))
        for path in (self.host / "apps/ios/native/Sources").rglob("*.swift"):
            path.write_text(path.read_text().replace(IOS_BUNDLE_ID, unreviewed))
        self.assert_rejected(reason="unreviewed processing modes / permitted identifiers")
        # Restore only the plist to prove the source anchors independently
        # enforce the exact reviewed IDs (including escaped regex dots).
        (self.host / self.INFO).write_bytes(original)
        self.assert_rejected(reason="missing reviewed processing identifier")

    def test_missing_unregistered_or_commented_task_source_rejected(self):
        path = self.host / "apps/ios/native/Sources/Cron/CronModels.swift"
        original = path.read_text()
        path.write_text(original.replace(IOS_BUNDLE_ID + ".cron.reconcile", IOS_BUNDLE_ID + ".cron.unknown"))
        self.assert_rejected(reason="missing reviewed processing identifier")
        path.write_text("/* " + original + " */")
        self.assert_rejected(reason="missing reviewed processing identifier")
        path.unlink()
        self.assert_rejected(reason="iOS background policy")

    def test_registration_expiration_completion_and_audio_cleanup_required(self):
        for relative, token in (
            ("App/AppNotificationDelegate.swift", "LocalAppBackgroundTaskBridge.shared.registerAtLaunch()"),
            ("Cron/CronSystemAdapters.swift", "processingTask.expirationHandler"),
            ("LocalApps/LocalAppsStore.swift", "processing.setTaskCompleted"),
            ("App/ConversationBackgroundActivity.swift", "BGContinuedProcessingTaskRequest("),
            ("Voice/VoiceAudioSessionCoordinator.swift", "sessionDriver.deactivate()"),
            ("App/RootView.swift", "VoiceAudioSessionCoordinator.shared.suspendForBackground()"),
        ):
            with self.subTest(source=relative):
                path = self.host / f"apps/ios/native/Sources/{relative}"
                original = path.read_text()
                self.assertIn(token, original)
                path.write_text(original.replace(token, "removedContract"))
                self.assert_rejected(reason="iOS background policy")
                path.write_text(original)

    def test_unreviewed_plist_cannot_declare_processing_or_audio(self):
        for modes in (["processing"], ["audio"]):
            with self.subTest(modes=modes):
                path = self.host / "apps/ios/native/Extra.plist"
                path.write_bytes(plistlib.dumps({"UIBackgroundModes": modes}))
                self.assert_rejected(reason="iOS background policy")

    def test_misplaced_duplicate_and_malformed_plist_rejected(self):
        original = (HOST / self.INFO).read_text()
        self.mutate_info(lambda info: info.update(Nested={"UIBackgroundModes": ["audio"]}))
        self.assert_rejected(reason="misplaced background declaration")
        self.write(self.INFO, original.replace('</dict>', '<key>UIBackgroundModes</key><array><string>audio</string></array></dict>'))
        self.assert_rejected(reason="duplicate plist key")
        self.write(self.INFO, "<plist><dict>")
        self.assert_rejected(reason="iOS background policy")
        (self.host / self.INFO).unlink()
        self.assert_rejected(reason="missing reviewed iOS Info.plist")

    def test_silent_or_muted_idle_audio_loops_rejected_without_audio_mode(self):
        for source in (
            'let player = AVAudioPlayer(contentsOf: URL(fileURLWithPath: "silence.wav"))\nplayer.numberOfLoops = -1\nplayer.play()',
            'player.numberOfLoops = -1\nplayer.volume = 0.0\nplayer.play()',
            'player.scheduleBuffer(buffer, options: .loops)\nplayer.volume = 0\n',
        ):
            for mode in ("play", "direct"):
                with self.subTest(mode=mode, source=source):
                    self.write("apps/ios/native/Sources/Voice/IdleKeepAlive.swift", source)
                    self.assert_rejected(mode, reason="forbidden fake idle background audio loop")

    def test_normal_audio_and_comments_are_not_idle_audio_abuse(self):
        self.write("apps/ios/native/Sources/Voice/Audio.swift", '''
player.numberOfLoops = 0
player.volume = 0.5
// player.numberOfLoops = -1; player.volume = 0
/* nested /* ignored */ numberOfLoops = -1 */
try session.setCategory(.playback)
''')
        result = self.run_policy()
        self.assertEqual(result.returncode, 0, result.stderr)


if __name__ == "__main__":
    unittest.main()
