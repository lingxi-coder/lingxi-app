"""Exercise the store-policy scan against the shared mobile FFI sources."""
from pathlib import Path
import re
import shutil
import subprocess
import tempfile
import unittest


ROOT = Path(__file__).resolve().parents[2]
SCRIPT_FILES = (
    "scripts/mobile-linux/check-store-compliance.sh",
    "scripts/mobile-linux/check-android-special-use.py",
    "scripts/mobile-linux/check-ios-background-policy.py",
    "scripts/lib/product_identity.py",
)
POLICY_FILES = (
    "apps/android/native/app/build.gradle.kts",
    "apps/android/native/app/src/main/AndroidManifest.xml",
    "apps/android/native/app/src/direct/AndroidManifest.xml",
    "apps/ios/native/project.yml",
    "apps/ios/native/Info.plist",
    "apps/ios/native/Sources/Cron/CronModels.swift",
    "apps/ios/native/Sources/Cron/CronSystemAdapters.swift",
    "apps/ios/native/Sources/App/AppNotificationDelegate.swift",
    "apps/ios/native/Sources/App/ConversationBackgroundActivity.swift",
    "apps/ios/native/Sources/Apps/LocalAppAgentBackground.swift",
    "apps/ios/native/Sources/Voice/VoiceAudioSessionCoordinator.swift",
    "apps/ios/native/Sources/App/RootView.swift",
)


class StoreComplianceTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="store-compliance-")
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name).resolve()
        for relative in SCRIPT_FILES + POLICY_FILES:
            destination = self.root / relative
            destination.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(ROOT / relative, destination)

        self.shared = self.root / "packages/mobile-ffi"
        shutil.copytree(ROOT / "packages/mobile-ffi", self.shared)
        for relative in (
            "apps/android/ffi", "apps/ios/ffi",
            "runtime/crates/platforms/android", "runtime/crates/platforms/ios",
        ):
            (self.root / relative).mkdir(parents=True)

        # The scanner's real source resolver needs the Cargo-locked checkout.
        # Keep this fixture independent of Cargo/network while running the real
        # source scan and policy helpers against the copied product contracts.
        manifest = (self.root / "apps/android/native/app/src/direct/AndroidManifest.xml").read_text()
        brand = re.search(r'android:name="\.computeruse\.([A-Za-z0-9_]+)AccessibilityService"', manifest)
        self.assertIsNotNone(brand)
        branding = self.root / "runtime/crates/branding/src/lib.rs"
        branding.parent.mkdir(parents=True)
        branding.write_text(f'pub const PRODUCT_NAME: &str = "{brand.group(1)}";\n')
        (self.root / "scripts/lib/runtime_source.py").write_text(
            "from pathlib import Path\n"
            "def resolve_runtime():\n"
            "    root = Path(__file__).resolve().parents[2] / 'runtime'\n"
            "    return {'packages': {'branding': str(root / 'crates/branding/Cargo.toml')}}\n"
            "if __name__ == '__main__':\n"
            "    print(Path(__file__).resolve().parents[2] / 'runtime')\n"
        )

    def scan(self, mode):
        return subprocess.run(
            ["bash", str(self.root / "scripts/mobile-linux/check-store-compliance.sh"), mode],
            cwd=self.root,
            capture_output=True,
            text=True,
        )

    def test_shared_sources_are_scanned_by_both_release_modes(self):
        cases = (
            ("// REQUEST_INSTALL_PACKAGES\n", "forbidden Android self-update"),
            ("ServiceInfo.FOREGROUND_SERVICE_TYPE_SPECIAL_USE\n", "forbidden Android specialUse policy"),
            ('const POLICY: &str = "UIBackgroundModes";\n', "forbidden iOS background policy"),
        )
        planted = self.shared / "src/unreviewed.rs"
        for mode in ("play", "direct"):
            with self.subTest(mode=mode, case="clean"):
                result = self.scan(mode)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertIn(f"{mode} store-compliance scan passed", result.stdout)
            for source, error in cases:
                with self.subTest(mode=mode, case=error):
                    planted.write_text(source)
                    try:
                        result = self.scan(mode)
                        self.assertNotEqual(result.returncode, 0)
                        self.assertIn(error, result.stderr)
                        self.assertIn(str(planted), result.stderr)
                    finally:
                        planted.unlink()

    def test_missing_shared_sources_fail_closed(self):
        self.shared.rename(self.root / "packages/mobile-ffi-moved")
        for mode in ("play", "direct"):
            with self.subTest(mode=mode):
                result = self.scan(mode)
                self.assertNotEqual(result.returncode, 0)
                self.assertIn("required compliance source missing", result.stderr)
                self.assertIn(str(self.shared), result.stderr)


if __name__ == "__main__":
    unittest.main()
