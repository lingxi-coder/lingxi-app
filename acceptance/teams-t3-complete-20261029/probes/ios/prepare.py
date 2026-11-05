#!/usr/bin/env python3
"""Generate this probe only. Does not resolve packages, build or launch devices."""
import hashlib
import json
from pathlib import Path
import subprocess

root = Path(__file__).resolve().parents[4]
probe = Path(__file__).resolve().parent
output = root / "output/teams-t3-ios"
output.mkdir(parents=True, exist_ok=True)
subprocess.run(["xcodegen", "generate", "--spec", str(probe / "project.yml"), "--project", str(output)], cwd=root, check=True)
lock = root / "acceptance/teams-t0-20261029/probes/ios/Package.resolved"
workspace = output / "TeamsT3Probe.xcodeproj/project.xcworkspace/xcshareddata/swiftpm"
workspace.mkdir(parents=True, exist_ok=True)
(workspace / "Package.resolved").write_bytes(lock.read_bytes())
paths = sorted((root / "apps/ios/native/Sources/Teams").glob("*.swift"))
paths += [root / "apps/ios/native/Sources/App/CanonicalDeepLink.swift", root / "apps/ios/native/Tests/TeamBotTests.swift", root / "apps/ios/native/Tests/TeamsTests.swift"]
paths += [root / "apps/ios/native/Sources" / relative for relative in (
    "Theme/LocalizationManager.swift", "Tabs/AppTab.swift", "Tabs/TabLogic.swift",
    "Conversation/AgentAvatar.swift", "Conversation/AvatarIdentity.swift", "Conversation/PersonAvatar.swift")]
paths += [root / "apps/ios/native/Resources/Localizable.xcstrings",
          root / "acceptance/mobile-team-bot-20261104/probes/ios/TeamsProbeShell.swift"]
harness = sorted(path for path in probe.rglob("*") if path.is_file() and path.suffix in {
    ".swift", ".yml", ".py", ".md", ".plist", ".entitlements"})
manifest = {
    "kind": "Teams feature harness; preparation only; no SDK build or runtime acceptance",
    "firebaseVersion": "12.19.2",
    "firebaseRevision": "8c29ca981990a32e626c8617544fa65d22b1834f",
    "projectID": "demo-lingxi-teams-t0",
    "packageLockSource": str(lock.relative_to(root)),
    "packageLockSHA256": hashlib.sha256(lock.read_bytes()).hexdigest(),
    "actualSourceFiles": [{"path": str(path.relative_to(root)), "sha256": hashlib.sha256(path.read_bytes()).hexdigest()} for path in paths],
    "harnessFiles": [{"path": str(path.relative_to(root)), "sha256": hashlib.sha256(path.read_bytes()).hexdigest()} for path in harness],
    "ffiFrameworksLinked": False,
    "previousCacheAtOutputTeamsT0IOSPresent": (root / "output/teams-t0-ios/SourcePackages/checkouts/firebase-ios-sdk").exists(),
}
(output / "preparation-inventory.json").write_text(json.dumps(manifest, indent=2) + "\n")
