#!/usr/bin/env python3
"""Validate the checked-in Phase 2 Local App Plugin migration.

This gate is intentionally independent of the Rust packer: it checks the
source manifest, the copied bytes, the per-family inventories/catalog, the
skill roster, and the build inventory as one cross-referenced contract. A
successful Rust build alone cannot detect a stale migration manifest or an
orphan accidentally reintroduced into a template family.
"""

from __future__ import annotations

import hashlib
import json
import re
import sys
from pathlib import Path


REPO = Path(__file__).resolve().parents[2]
CODE = REPO / "lingxi-code"
PLUGIN = CODE / "plugins" / "lingxi-local-app"
MANIFEST = REPO / "docs" / "local-apps" / "harness" / "template-migration-manifest.json"
INVENTORY = CODE / "apps" / "engine-mobile" / "builtin-plugin-inventory.txt"
PROFILE_RS = CODE / "apps" / "engine-mobile" / "src" / "local_app_runtime_profiles.rs"
PERMISSIONS_RS = CODE / "local-apps" / "src" / "permissions.rs"
PERMISSIONS_ASSET = CODE / "local-apps" / "assets" / "default-workspace-settings.local.json"
TASKS_PHASE2 = REPO / "docs" / "local-apps" / "harness" / "tasks-phase-2.json"

EXPECTED_SKILLS = {
    "accessibility",
    "babylon-3d-local-app",
    "canvas-2d-local-app",
    "create-local-app",
    "device",
    "expose-as-mcp",
    "frontend-design",
    "frontend-qa",
    "ionic-react-local-app",
    "llm-agent",
    "llm-sidequery",
    "local-app-background",
    "local-app-capture-view",
    "local-app-data",
    "local-app-debug",
    "local-app-inspect-view",
    "local-app-interact",
    "local-app-run",
    "local-app-test",
    "local-app-use",
    "mcp-flow-binding",
    "mcp-qa",
    "mcp-tool-design",
    "phaser-2d-local-app",
    "react-best-practices",
    "template-selection",
    "threejs-local-app",
}
EXPECTED_COMPONENT_FILES = {
    "agents": {
        "builder.md", "create-preparer.md", "designer.md", "mcp-designer.md", "mcp-promoter.md", "operator.md",
        "template-selector.md", "tester.md", "verifier.md",
    },
    "workflows": {
        "local-app-build.js", "local-app-mcp-authoring.js", "local-app-use-test.js",
    },
    "schemas": {
        "authoring-spec.schema.json", "design-spec.schema.json", "mcp-proposal.schema.json", "qa-report.schema.json", "workflow-agent-results.schema.json",
        "use-test-report.schema.json",
    },
}
EXPECTED_FAMILIES = {
    "react-dom": "react_dom",
    "canvas-2d": "canvas_2d",
    "three-3d": "three_3d",
    "phaser-2d": "phaser_2d",
    "babylon-3d": "babylon_3d",
}
EXPECTED_TEMPLATE_REVISIONS = {
    "react-dom": 2,
    "canvas-2d": 2,
    "three-3d": 2,
    "phaser-2d": 2,
    "babylon-3d": 1,
}
EXPECTED_SHARED_MCP_WIDGET_ASSETS = {
    "shared/mcp-widget/r2/index.html",
    "shared/mcp-widget/r2/package.json",
    "shared/mcp-widget/r2/src/main.jsx",
    "shared/mcp-widget/r2/src/widget.jsx",
    "shared/mcp-widget/r2/vite.config.mjs",
}
ORPHAN_NAMES = {
    "app/screens/detail-screen.jsx",
    "app/screens/home-screen.jsx",
    "src/stores/app-store.js",
}


def fail(message: str) -> None:
    print(f"PHASE2-PLUGIN FAIL: {message}", file=sys.stderr)
    raise SystemExit(1)


def require_file(path: Path, label: str) -> Path:
    if not path.is_file():
        fail(f"{label} missing: {path}")
    return path


def sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def canonical_json(value: object) -> bytes:
    return json.dumps(value, ensure_ascii=False, sort_keys=True, separators=(",", ":")).encode()


def read_json(path: Path, label: str) -> dict:
    require_file(path, label)
    try:
        value = json.loads(path.read_text(encoding="utf-8"))
    except json.JSONDecodeError as error:
        fail(f"{label} is not valid JSON: {error}")
    if not isinstance(value, dict):
        fail(f"{label} must be a JSON object")
    return value


def shared_widget_inventory_records(template_root: Path) -> list[dict]:
    records = []
    for relative in sorted(EXPECTED_SHARED_MCP_WIDGET_ASSETS):
        path = template_root / relative
        require_file(path, f"shared MCP widget asset {relative}")
        records.append(
            {
                "path": f"app/mcp-widget/{Path(relative).relative_to('shared/mcp-widget/r2').as_posix()}",
                "bytes": path.stat().st_size,
                "sha256": sha256(path),
            }
        )
    return records


def shared_overlay_inventory_sha256(
    template_root: Path, family: str, family_key: str, family_records: list[dict]
) -> str:
    combined_files = sorted(
        [
            {"path": item["path"], "bytes": item["bytes"], "sha256": item["sha256"]}
            for item in family_records
        ]
        + shared_widget_inventory_records(template_root),
        key=lambda item: item["path"],
    )
    combined = {
        "schemaVersion": 2,
        "family": family_key,
        "sourceFamily": family,
        "revision": 2,
        "baseRevision": 1,
        "sharedOverlay": "shared/mcp-widget/r2",
        "files": combined_files,
        "totalBytes": sum(item["bytes"] for item in combined_files),
    }
    return hashlib.sha256(canonical_json(combined)).hexdigest()


def check_manifest_and_skills() -> tuple[dict, list[dict]]:
    manifest = read_json(PLUGIN / ".lingxi-plugin" / "plugin.json", "Plugin manifest")
    required = {
        "name": "lingxi-local-app",
        "displayName": "LingXi Local App",
        "version": "1.0.0",
        "defaultEnabled": True,
        "skills": "./skills/",
        "agents": "./agents/",
        "workflows": "./workflows/",
    }
    for key, expected in required.items():
        if manifest.get(key) != expected:
            fail(f"Plugin manifest {key!r} must be {expected!r}, got {manifest.get(key)!r}")
    if manifest.get("author", {}).get("name") != "LingXi":
        fail("Plugin manifest author.name must preserve the LingXi brand")

    skill_root = PLUGIN / "skills"
    if not skill_root.is_dir():
        fail("Plugin must contain exactly 27 skills; skill root is missing")
    skill_dirs = {path.name for path in skill_root.iterdir() if path.is_dir()}
    if skill_dirs != EXPECTED_SKILLS:
        fail(
            "Plugin must contain exactly 27 skills; "
            f"missing={sorted(EXPECTED_SKILLS - skill_dirs)}, extra={sorted(skill_dirs - EXPECTED_SKILLS)}"
        )

    root_skills = REPO / "skills"
    for name in sorted({
        "accessibility", "babylon-3d-local-app", "canvas-2d-local-app", "create-local-app",
        "frontend-design", "frontend-qa", "ionic-react-local-app", "phaser-2d-local-app",
        "react-best-practices", "threejs-local-app",
    }):
        source = root_skills / name
        destination = skill_root / name
        if not source.is_dir():
            fail(f"root Local App skill source missing: {source}")
        source_files = {path.relative_to(source) for path in source.rglob("*") if path.is_file()}
        destination_files = {path.relative_to(destination) for path in destination.rglob("*") if path.is_file()}
        if source_files != destination_files:
            fail(f"skill migration file set differs for {name}: source/destination are not equal")
        for relative in source_files:
            source_file = source / relative
            destination_file = destination / relative
            if source_file.read_bytes() != destination_file.read_bytes():
                fail(f"skill migration bytes differ: {source_file} vs {destination_file}")
        if not (destination / "SKILL.md").is_file() or not (destination / "agents" / "openai.yaml").is_file():
            fail(f"{name} must include SKILL.md and inert agents/openai.yaml metadata")

    for name in sorted(EXPECTED_SKILLS):
        skill_file = skill_root / name / "SKILL.md"
        lines = skill_file.read_text(encoding="utf-8").splitlines()
        if len(lines) < 3 or lines[0].strip() != "---" or "---" not in lines[1:]:
            fail(f"{skill_file} does not have parseable YAML frontmatter")
        closing = lines[1:].index("---") + 1
        frontmatter = "\n".join(lines[1:closing])
        if not re.search(r"(?m)^name:\s*\S", frontmatter) or not re.search(r"(?m)^description:\s*\S", frontmatter):
            fail(f"{skill_file} frontmatter must contain non-empty name and description")

    for directory, expected in EXPECTED_COMPONENT_FILES.items():
        entries = {path.name for path in (PLUGIN / directory).iterdir() if path.is_file()}
        if entries != expected:
            fail(
                f"Plugin {directory} roster differs: "
                f"missing={sorted(expected - entries)}, extra={sorted(entries - expected)}"
            )
    return manifest, []


def check_migration(manifest: dict) -> None:
    migration = read_json(MANIFEST, "template migration manifest")
    entries = migration.get("files")
    if not isinstance(entries, list) or len(entries) != 112:
        fail(f"template migration manifest must lock exactly 112 files, got {len(entries) if isinstance(entries, list) else entries!r}")
    if migration.get("callSites") != 112 or migration.get("entries") != 112:
        fail(f"template migration callSites/entries must both be 112, got {migration.get('callSites')!r}/{migration.get('entries')!r}")
    if migration.get("missingOnDisk") or migration.get("referencedButUntracked"):
        fail("template migration manifest contains missingOnDisk or referencedButUntracked entries")

    expected_destinations: set[str] = set()
    family_entries: dict[str, list[dict]] = {family: [] for family in EXPECTED_FAMILIES}
    for entry in entries:
        family = entry.get("family")
        relative = entry.get("path")
        if family not in EXPECTED_FAMILIES or not isinstance(relative, str) or not relative:
            fail(f"invalid template migration entry: {entry!r}")
        if Path(relative).is_absolute() or ".." in Path(relative).parts:
            fail(f"unsafe template migration path: {relative!r}")
        expected_source = (
            CODE / "local-apps" / "templates" / "runtime-profiles" / family / "r1" / relative
        )
        source = REPO / entry.get("source", "")
        if source != expected_source:
            fail(
                f"template migration source must be the legacy production asset for "
                f"{family}/{relative}, got {source}"
            )
        destination = PLUGIN / "assets" / "templates" / family / "r1" / relative
        expected_destinations.add(destination.relative_to(PLUGIN / "assets" / "templates").as_posix())
        family_entries[family].append(entry)
        require_file(source, f"template source {family}/{relative}")
        require_file(destination, f"template destination {family}/{relative}")
        source_bytes = source.read_bytes()
        destination_bytes = destination.read_bytes()
        if source_bytes != destination_bytes:
            fail(f"template migration bytes differ: {source} vs {destination}")
        if len(source_bytes) != entry.get("bytes") or sha256(source) != entry.get("sha256"):
            fail(f"template migration manifest digest is stale for {family}/{relative}")
    if {family: len(items) for family, items in family_entries.items()} != migration.get("perFamily"):
        fail(f"template migration perFamily counts do not match entries: {migration.get('perFamily')!r}")
    if migration.get("totalBytes") != sum(item["bytes"] for item in entries):
        fail("template migration totalBytes does not match its 112 entries")

    profile_text = PROFILE_RS.read_text(encoding="utf-8")
    call_sites = re.findall(
        r'profile_file!\(\s*"([^"]+)"\s*,\s*"([^"]+)"\s*\)',
        profile_text,
        re.DOTALL,
    )
    declared = [(item["family"], item["path"]) for item in entries]
    if call_sites[: len(declared)] != declared:
        fail(
            "template migration entries must remain the exact prefix order of the "
            "112 baseline profile_file! call sites"
        )

    template_root = PLUGIN / "assets" / "templates"
    actual_assets = {
        path.relative_to(template_root).as_posix()
        for path in template_root.rglob("*")
        if path.is_file() and path.name not in {"inventory.json", "catalog.json"}
    }
    if not expected_destinations.issubset(actual_assets):
        fail(
            "Plugin template asset set is missing a baseline r1 asset: "
            f"missing={sorted(expected_destinations - actual_assets)[:4]}"
        )
    unexpected_r1_assets = {
        path
        for path in actual_assets
        if "/r1/" in path and path not in expected_destinations
    }
    if unexpected_r1_assets:
        fail(f"unexpected extra r1 assets: {sorted(unexpected_r1_assets)[:4]}")
    unexpected_family_r2_assets = {
        path
        for path in actual_assets
        if any(path.startswith(f"{family}/r2/") for family in EXPECTED_FAMILIES)
    }
    if unexpected_family_r2_assets:
        fail(
            "r2 template assets must be synthesized from the shared overlay, "
            f"not copied per family: {sorted(unexpected_family_r2_assets)[:4]}"
        )
    shared_assets = {path for path in actual_assets if path.startswith("shared/")}
    if shared_assets != EXPECTED_SHARED_MCP_WIDGET_ASSETS:
        fail(
            "shared MCP widget asset set differs: "
            f"missing={sorted(EXPECTED_SHARED_MCP_WIDGET_ASSETS - shared_assets)}, "
            f"extra={sorted(shared_assets - EXPECTED_SHARED_MCP_WIDGET_ASSETS)}"
        )

    orphaned = migration.get("orphanedTrackedFiles")
    expected_orphans = {
        (
            CODE
            / "local-apps"
            / "templates"
            / "runtime-profiles"
            / family
            / "r1"
            / relative
        ).relative_to(REPO).as_posix()
        for family in EXPECTED_FAMILIES
        if family != "react-dom"
        for relative in ORPHAN_NAMES
    }
    if not isinstance(orphaned, list) or set(orphaned) != expected_orphans:
        fail(f"migration manifest must retain the exact 12 tracked orphan record, got {orphaned!r}")
    for orphan in orphaned:
        orphan_path = REPO / orphan
        if orphan_path.exists():
            fail(f"tracked orphan was reintroduced into source templates: {orphan_path}")
        parts = Path(orphan).parts
        try:
            family_index = parts.index("runtime-profiles") + 1
            revision_index = parts.index("r1")
            family = parts[family_index]
            relative = "/".join(parts[revision_index + 1:])
        except ValueError:
            family, relative = "", ""
        if family in EXPECTED_FAMILIES and relative and (template_root / family / "r1" / relative).exists():
            fail(f"tracked orphan was copied into Plugin inventory: {family}/{relative}")

    for family, family_key in EXPECTED_FAMILIES.items():
        inventory_path = template_root / family / "r1" / "inventory.json"
        inventory = read_json(inventory_path, f"{family} inventory")
        family_files = family_entries[family]
        # Preserve the migration manifest's stable call-site order. The
        # inventory is consumed by the packer as an ordered declaration, so a
        # path-set-only comparison would allow an accidental reordering to
        # drift from the checked-in golden.
        expected_records = [
            {"path": item["path"], "bytes": item["bytes"], "sha256": item["sha256"]}
            for item in family_files
        ]
        if (
            inventory.get("schemaVersion") != 1
            or inventory.get("family") != family_key
            or inventory.get("sourceFamily") != family
            or inventory.get("revision") != 1
        ):
            fail(f"{inventory_path} has wrong schema/family/sourceFamily/revision")
        if inventory.get("files") != expected_records:
            fail(f"{inventory_path} does not equal the migration manifest records")
        if inventory.get("totalBytes") != sum(item["bytes"] for item in expected_records):
            fail(f"{inventory_path} totalBytes is stale")

    catalog = read_json(template_root / "catalog.json", "runtime profile catalog")
    toolchain_match = re.search(
        r'const RUNTIME_PROFILE_TOOLCHAIN_KEY: &str = "([^"]+)";', profile_text
    )
    if toolchain_match is None:
        fail("cannot resolve the production runtime profile toolchain key")
    if catalog.get("schemaVersion") != 2 or catalog.get("toolchainKey") != toolchain_match.group(1):
        fail("runtime profile catalog schema/toolchain differs from production")
    templates = catalog.get("templates")
    if not isinstance(templates, list) or len(templates) != 5:
        fail(f"runtime profile catalog must contain exactly 5 templates, got {templates!r}")
    seen_families = set()
    for template in templates:
        family = next((raw for raw, key in EXPECTED_FAMILIES.items() if key == template.get("family")), None)
        if family is None or template.get("revision") != EXPECTED_TEMPLATE_REVISIONS[family]:
            fail(f"catalog contains unknown family or revision: {template!r}")
        if template.get("templateId") != f"{family}-r{EXPECTED_TEMPLATE_REVISIONS[family]}":
            fail(f"catalog templateId is not canonical for {family}")
        expected_surface = "dom" if family == "react-dom" else "canvas"
        if template.get("surface") != expected_surface:
            fail(f"catalog surface is stale for {family}")
        if not re.fullmatch(r"[0-9a-f]{64}", str(template.get("contractSha256", ""))):
            fail(f"catalog contractSha256 is not a canonical SHA-256 for {family}")
        seen_families.add(family)
        if template.get("mcpDefaultEnabled") is not False:
            fail(f"catalog mcpDefaultEnabled must be false for {family}")
        suggestions = template.get("mcpSuggestions")
        if not isinstance(suggestions, list) or not suggestions or not all(isinstance(item, str) and item for item in suggestions):
            fail(f"catalog mcpSuggestions must be a non-empty string list for {family}")
        inventory_value = read_json(template_root / family / "r1" / "inventory.json", f"{family} inventory")
        if EXPECTED_TEMPLATE_REVISIONS[family] == 1:
            expected_digest = hashlib.sha256(canonical_json(inventory_value)).hexdigest()
        else:
            expected_digest = shared_overlay_inventory_sha256(
                template_root,
                family,
                EXPECTED_FAMILIES[family],
                inventory_value["files"],
            )
        if template.get("inventorySha256") != expected_digest:
            fail(f"catalog inventorySha256 is stale for {family}")
        if template.get("available") is not (family != "babylon-3d"):
            fail(f"catalog availability must keep Babylon fail-closed: {template!r}")
        if family == "babylon-3d" and "real-device" not in template.get("availabilityReason", ""):
            fail("Babylon catalog entry must explain the unavailable real-device validation")
    if seen_families != set(EXPECTED_FAMILIES):
        fail(f"catalog families differ from runtime profile families: {seen_families!r}")


def check_build_inventory_and_permissions() -> None:
    inventory_lines = [line.strip() for line in require_file(INVENTORY, "build inventory").read_text().splitlines() if line.strip() and not line.lstrip().startswith("#")]
    if len(inventory_lines) != len(set(inventory_lines)):
        fail("build inventory contains duplicate paths")
    if inventory_lines != sorted(inventory_lines):
        fail("build inventory paths must stay sorted")
    actual = {path.relative_to(PLUGIN).as_posix() for path in PLUGIN.rglob("*") if path.is_file()}
    if set(inventory_lines) != actual:
        fail(
            "build inventory must equal the actual Plugin directory: "
            f"missing={sorted(actual - set(inventory_lines))[:4]}, extra={sorted(set(inventory_lines) - actual)[:4]}"
        )
    if len(inventory_lines) != 213:
        fail(f"build inventory expected 213 exact files for the current package, got {len(inventory_lines)}")
    source = require_file(PERMISSIONS_ASSET, "permission settings asset")
    if "default-workspace-settings.local.json" not in PERMISSIONS_RS.read_text(encoding="utf-8"):
        fail("permissions.rs must include the migrated production settings asset")
    try:
        permission_payload = json.loads(source.read_text(encoding="utf-8"))
    except json.JSONDecodeError as error:
        fail(f"permission settings asset is not valid JSON: {error}")
    if not isinstance(permission_payload.get("permissions"), dict):
        fail("permission settings asset must retain the production permissions object")
    if (PLUGIN / "assets" / "templates" / "vite-react-static-v1").exists():
        fail("permission settings must not be copied into Plugin template assets")
    profile_text = PROFILE_RS.read_text(encoding="utf-8")
    if len(re.findall(r'profile_file!\(\s*"', profile_text)) < 112:
        fail("local_app_runtime_profiles.rs must retain at least the 112 baseline profile_file! call sites")
    if "plugins/lingxi-local-app/assets/templates/" not in profile_text or "runtime-profiles" in profile_text:
        fail("runtime profile production includes must point only at Plugin template assets")


def check_phase2_task_evidence() -> None:
    descriptor = read_json(TASKS_PHASE2, "Phase 2 task descriptor")
    tasks = descriptor.get("tasks")
    if not isinstance(tasks, list) or {task.get("id") for task in tasks} != {"P2.0", "P2.1", "P2.2", "P2.3"}:
        fail("Phase 2 task descriptor must contain exactly P2.0 through P2.3")
    for task in tasks:
        owned_sources: set[Path] = set()
        for owned in task.get("owns", []):
            path = REPO / owned
            if path.is_file() and path.suffix == ".rs":
                owned_sources.add(path)
            elif path.is_dir():
                owned_sources.update(path.rglob("*.rs"))
        rust_sources = "\n".join(
            path.read_text(encoding="utf-8", errors="ignore")
            for path in sorted(owned_sources)
        )
        for test_name in task.get("addedTests", []):
            if not re.search(rf"\bfn\s+{re.escape(test_name)}\b", rust_sources):
                fail(
                    f"Phase 2 task {task.get('id')} claims nonexistent addedTests symbol "
                    f"{test_name!r}"
                )


def main() -> int:
    check_manifest_and_skills()
    migration = read_json(MANIFEST, "template migration manifest")
    check_migration(migration)
    check_build_inventory_and_permissions()
    check_phase2_task_evidence()
    print("PHASE2-PLUGIN OK: 27 skills, 9 agents, 3 workflows, 6 schemas, 112 base profile assets, 5 shared MCP widget assets, 12 excluded orphans, and exact build inventory")
    return 0


if __name__ == "__main__":
    sys.exit(main())
