#!/usr/bin/env python3
"""Generate only Team R strings and immutable source provenance; never patch sources."""
import hashlib
import json
import re
from pathlib import Path
import xml.etree.ElementTree as ET

PROBE = Path(__file__).resolve().parent
ROOT = PROBE.parents[3]
OUTPUT = ROOT / "output/teams-t3-android"
TEAMS = ROOT / "apps/android/native/app/src/main/java/com/lingxi/code/teams"
STRINGS = ROOT / "apps/android/native/app/src/main/res/values-en/strings.xml"
INVITATION_SUPPORT = [ROOT / "apps/android/native/app/src/main/java/com/lingxi/code" / relative for relative in (
    "navigation/DeepLink.kt", "navigation/DeepLinkInbox.kt", "push/PushRoute.kt")]
TAB_SUPPORT = [ROOT / "apps/android/native/app/src/main/java/com/lingxi/code" / relative for relative in (
    "tabs/AppTab.kt", "tabs/TabLogic.kt", "theme/AvatarIdentity.kt", "theme/AgentAvatar.kt", "theme/PersonAvatar.kt")]
SHARED_PROBE = ROOT / "acceptance/mobile-team-bot-20261104/probes/android"

def main():
    sources = sorted(TEAMS.glob("*.kt"))
    assert sources, "Actual Teams sources missing"
    assert 'internal const val TEAMS_API_ROOT = "https://lingxios.net"' in (TEAMS / "TeamHttp.kt").read_text()
    # API roots are immutable constructor dependencies; only this demo executable injects loopback.
    for name in ("TeamsViewModel.kt", "TeamMessagingModel.kt", "TeamsWorkspace.kt"):
        source = (TEAMS / name).read_text()
        assert "apiRoot: String = TEAMS_API_ROOT" in source, name
    wanted = set()
    for path in sources:
        wanted.update(re.findall(r"R\.string\.(\w+)", path.read_text()))
    resources = ET.Element("resources")
    names = set()
    for node in ET.parse(STRINGS).getroot():
        if node.tag == "string" and node.attrib.get("name", "") in wanted:
            resources.append(node); names.add(node.attrib["name"])
    assert wanted <= names, f"Missing production Team strings: {wanted - names}"
    path = OUTPUT / "resources/values/strings.xml"
    path.parent.mkdir(parents=True, exist_ok=True)
    ET.indent(resources)
    path.write_bytes(ET.tostring(resources, encoding="utf-8", xml_declaration=True))
    harness = sorted(path for path in PROBE.rglob("*") if path.is_file() and (path.suffix in {".kt", ".kts", ".py", ".md", ".properties"} or path.name == "AndroidManifest.xml"))
    support = sorted(SHARED_PROBE.rglob("*.kt")) + TAB_SUPPORT
    fixtures = [ROOT / "apps/android/native/app/src/test/java/com/lingxi/code/tabs/SharedVectors.kt",
                ROOT / "packages/bridge-client/fixtures/mobile-team-bot-vectors.json"]
    avatars = sorted((ROOT / "apps/android/native/app/src/main/res").glob("drawable*/agent_avatar_*"))
    tracked = sources + INVITATION_SUPPORT + support + fixtures + avatars + [STRINGS, ROOT / "apps/android/native/gradle/libs.versions.toml"] + harness
    provenance = {
        "kind": "Android Teams Compose/Firebase feature harness, not the full signed product",
        "source_mode": "Gradle compiles all actual Teams Kotlin sources directly; no Kotlin source copies or patches",
        "unit_test_support": [str(path.relative_to(ROOT)) for path in INVITATION_SUPPORT],
        "tab_support": [str(path.relative_to(ROOT)) for path in support],
        "resources": {"source": str(STRINGS.relative_to(ROOT)), "names": sorted(names), "generated": str(path.relative_to(ROOT))},
        "production_endpoint_check": "All three constructor roots default to https://lingxios.net",
        "sha256": {str(file.relative_to(ROOT)): hashlib.sha256(file.read_bytes()).hexdigest() for file in tracked},
    }
    (OUTPUT / "provenance.json").write_text(json.dumps(provenance, indent=2) + "\n")
    print(json.dumps({"actual_team_sources": len(sources), "team_strings": len(names), "output": str(OUTPUT)}))

if __name__ == "__main__":
    main()
