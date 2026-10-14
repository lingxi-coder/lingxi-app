#!/usr/bin/env python3
"""Compile the handbook's actual copyable Rust blocks in isolated consumers.

No production Cargo.toml/Cargo.lock is edited. Offline is the default. Source
identity is checked before Cargo tests; a green mock test proves no device or
provider behavior. The SDK's downstream-docs doctests are checked separately.
"""
from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path
import re
import sys
import tomllib

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "examples"))
from sdk_tools import SdkSnapshot, WEB, cached_revision, cargo_args, cargo_target_lock, command, docs_blocks


def consumer_source(blocks: list[dict]) -> str:
    return "\n\n".join(
        f"// Copied verbatim from {block['page']}#{block['section']}: {block['exampleId']}\n"
        f"#[allow(dead_code, unused_imports)]\nmod example_{index} {{\n{block['code']}\n}}"
        for index, block in enumerate(blocks)
    ) + "\n"


def selected_blocks(blocks: list[dict], group: str) -> list[dict]:
    # Audio composition belongs to system pages but uses the Harness SDK.
    return [block for block in blocks if block.get("verification") in ("compile", "mock")
            and block["language"] == "rust"
            and (block["group"] == group or (group == "harness" and block.get("exampleId", "").startswith("harness-")))]


def sync_fixtures(blocks: list[dict], *, check: bool) -> None:
    for group in ("harness", "llm", "mobile"):
        manifests = [block for block in blocks if block.get("exampleId") == f"{group}-working-tree-manifest"]
        if len(manifests) != 1:
            raise RuntimeError(f"Expected one authoritative {group}-working-tree-manifest")
        content = manifests[0]["code"].strip() + "\n"
        # Fail early on malformed copied config; fixtures are data, not shell text.
        tomllib.loads(content)
        destination = WEB / "examples" / group / "Cargo.toml"
        if check:
            if not destination.exists() or destination.read_text() != content:
                raise RuntimeError(f"Stale generated example fixture: {destination}; run docs:sync")
        else:
            destination.parent.mkdir(parents=True, exist_ok=True)
            destination.write_text(content)


def run_group(snapshot: SdkSnapshot, blocks: list[dict], group: str, offline: bool, target: Path) -> dict:
    examples = selected_blocks(blocks, group)
    if not examples:
        raise RuntimeError(f"No actual compile/mock handbook blocks for {group}")
    manifests = [block for block in blocks if block.get("exampleId") == f"{group}-working-tree-manifest"]
    if len(manifests) != 1:
        raise RuntimeError(f"Expected exactly one copyable {group}-working-tree-manifest; found {len(manifests)}")
    template = (WEB / "examples" / group / "Cargo.toml").read_text().rstrip()
    if manifests[0]["code"].strip() != template:
        raise RuntimeError(f"Reader manifest differs from examples/{group}/Cargo.toml")
    destination = snapshot.root / f"{group}-consumer"
    (destination / "src").mkdir(parents=True)
    manifest = destination / "Cargo.toml"
    manifest.write_text(snapshot.reader_manifest(manifests[0]["code"]) + "\n")
    (destination / "src" / "lib.rs").write_text(consumer_source(examples))
    # Preserve the SDK's reviewed third-party resolution for an offline
    # consumer. Cargo updates only this temporary lock to apply the explicit
    # local SDK closure; metadata checks every resolved SDK source identity.
    repository = {"harness": "harness-runtime", "llm": "llm-client", "mobile": "mobile-linux-runtime"}[group]
    seed = snapshot.root / repository / "Cargo.lock"
    if seed.exists():
        (destination / "Cargo.lock").write_bytes(seed.read_bytes())
    identities = snapshot.verify_metadata(manifest, offline=offline)
    output = command(cargo_args("test", manifest, offline=offline, target=target) + ["--lib"], cwd=destination).stdout
    mock_count = sum(block.get("verification") == "mock" for block in examples)
    if mock_count and re.search(r"running 0 tests", output):
        raise RuntimeError(f"Mock blocks for {group} did not define executable tests")
    return {"id": group, "state": "passed", "examples": [block["exampleId"] for block in examples],
            "identities": identities, "testOutput": output[-3000:]}


def run_downstream(snapshot: SdkSnapshot, offline: bool, target: Path) -> dict:
    manifest = snapshot.root / "llm-client" / "tests" / "downstream-docs" / "Cargo.toml"
    identities = snapshot.verify_metadata(manifest, offline=offline)
    args = cargo_args("test", manifest, offline=offline, target=target)
    result = command(args, cwd=manifest.parent, timeout=2400)
    return {"id": "downstream-docs", "state": "passed", "identities": identities, "testOutput": result.stdout[-3000:]}


def run_bridge(snapshot: SdkSnapshot, blocks: list[dict]) -> dict:
    examples = [block for block in blocks if block.get("verification") == "compile" and block["language"] == "typescript" and block["group"] == "bridge"]
    if not examples:
        raise RuntimeError("No actual compile-marked TypeScript Bridge handbook blocks")
    package = snapshot.workspace / "lingxi-app" / "packages" / "bridge-client"
    entrypoint = package / "src" / "index.ts"
    if not entrypoint.is_file():
        raise RuntimeError(f"Bridge public source entrypoint missing: {entrypoint}")
    destination = snapshot.root / "bridge-consumer"
    destination.mkdir()
    files = []
    for index, block in enumerate(examples):
        source = destination / f"example-{index}.ts"
        source.write_text(block["code"] + "\n")
        files.append(str(source))
    config = destination / "tsconfig.json"
    config.write_text(json.dumps({"compilerOptions": {
        "target": "ES2022", "module": "ESNext", "moduleResolution": "Bundler",
        "strict": True, "skipLibCheck": True, "noEmit": True, "esModuleInterop": True,
        "types": ["node"], "typeRoots": [str(WEB / "node_modules" / "@types"), str(package / "node_modules" / "@types")],
        "paths": {"@lingxi/bridge-client": [str(entrypoint)]},
    }, "files": files}, indent=2) + "\n")
    tsc = WEB / "node_modules" / "typescript" / "bin" / "tsc"
    result = command(["node", str(tsc), "-p", str(config)], cwd=destination)
    hasher = hashlib.sha256()
    for source in sorted((package / "src").rglob("*.ts")):
        hasher.update(str(source.relative_to(package)).encode() + b"\0")
        hasher.update(source.read_bytes())
    return {"id": "bridge", "state": "passed", "examples": [block["exampleId"] for block in examples],
            "identities": [{"package": "@lingxi/bridge-client", "entrypoint": "lingxi-app/packages/bridge-client/src/index.ts", "source": "working-tree", "sourceFingerprint": hasher.hexdigest()}],
            "testOutput": result.stdout}


def run_pins(snapshot: SdkSnapshot, blocks: list[dict], offline: bool, target: Path) -> dict:
    pin_blocks = [block for block in blocks if block.get("exampleId", "").endswith("-pinned-manifest")]
    if not pin_blocks:
        raise RuntimeError("No copyable fixed-revision installation manifest is documented")
    verified = []
    for index, block in enumerate(pin_blocks):
        data = tomllib.loads(block["code"])
        dependencies = data.get("dependencies", {})
        pinned = [(name, value) for name, value in dependencies.items() if isinstance(value, dict) and "git" in value]
        if not pinned or any(not re.fullmatch("[0-9a-f]{40}", value.get("rev", "")) for _, value in pinned):
            raise RuntimeError(f"Full reviewed revisions required by {block['exampleId']}")
        for _, dependency in pinned:
            cached_revision(Path(dependency["git"]).stem, dependency["rev"])
        destination = snapshot.root / f"pinned-consumer-{index}"
        (destination / "src").mkdir(parents=True)
        (destination / "src" / "lib.rs").write_text("// Installation source/feature check; no provider or device calls.\n")
        manifest = destination / "Cargo.toml"
        manifest.write_text(block["code"] + "\n")
        metadata = json.loads(command(["cargo", "metadata", "--format-version", "1", "--manifest-path", str(manifest)] + (["--offline"] if offline else []), cwd=destination).stdout)
        for name, dependency in pinned:
            packages = [package for package in metadata["packages"] if package["name"] == dependency.get("package", name)]
            if len(packages) != 1 or not (packages[0].get("source") or "").endswith("#" + dependency["rev"]):
                raise RuntimeError(f"Pinned installation resolved unexpected source for {name}")
        command(cargo_args("check", manifest, offline=offline, target=target), cwd=destination)
        verified.append({"example": block["exampleId"], "dependencies": [{"package": name, "revision": dependency["rev"]} for name, dependency in pinned]})
    return {"id": "pins", "state": "passed", "manifests": verified}


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--workspace", type=Path, default=WEB.parents[2])
    parser.add_argument("--online", action="store_true", help="allow Cargo to fetch dependencies; offline is default")
    parser.add_argument("--only", help="comma-separated harness,llm,mobile,bridge,downstream,pins")
    parser.add_argument("--report", type=Path, help="optional JSON evidence output")
    parser.add_argument("--target-dir", type=Path, help="cache directory; defaults to existing Harness target")
    parser.add_argument("--inventory", action="store_true", help="print exact snippet inventory without Cargo")
    parser.add_argument("--sync-fixtures", action="store_true", help="generate copyable manifest fixtures from handbook data, without Cargo")
    parser.add_argument("--check-fixtures", action="store_true", help="check generated manifest fixtures, without Cargo")
    args = parser.parse_args()
    blocks = docs_blocks()
    evidence_blocks = [block for block in blocks if block.get("verification") in ("compile", "mock") or block.get("exampleId", "").endswith("-manifest")]
    snippet_fingerprint = hashlib.sha256(json.dumps(sorted(evidence_blocks, key=lambda block: block["exampleId"]), sort_keys=True, ensure_ascii=False, separators=(",", ":")).encode()).hexdigest()
    if args.sync_fixtures or args.check_fixtures:
        if args.sync_fixtures and args.check_fixtures:
            parser.error("Choose --sync-fixtures or --check-fixtures")
        sync_fixtures(blocks, check=args.check_fixtures)
        print("Example manifest fixtures are current.")
        return 0
    if args.inventory:
        print(json.dumps([{key: block.get(key) for key in ("page", "section", "exampleId", "verification", "language")} for block in blocks], indent=2))
        return 0
    ids = [block.get("exampleId") for block in blocks if block.get("verification") in ("compile", "mock")]
    if any(not value for value in ids) or len(ids) != len(set(ids)):
        raise RuntimeError("Compile/mock examples require unique stable exampleId values")
    for block in blocks:
        if block.get("verification") not in ("compile", "mock"):
            continue
        supported_rust = block["language"] == "rust" and (block["group"] in {"harness", "llm", "mobile"} or block.get("exampleId", "").startswith("harness-"))
        supported_bridge = block["language"] == "typescript" and block["group"] == "bridge" and block["verification"] == "compile"
        if not (supported_rust or supported_bridge):
            raise RuntimeError(f"No automatic checker for {block['exampleId']}; add one before marking it compile/mock")
    selected = args.only.split(",") if args.only else ["harness", "llm", "mobile", "bridge", "downstream", "pins"]
    if set(selected) - {"harness", "llm", "mobile", "bridge", "downstream", "pins"}:
        raise RuntimeError("Unknown --only group")
    target = args.target_dir or args.workspace / "harness-runtime" / "target"
    lock = cargo_target_lock(target)
    lock.__enter__()
    snapshot = SdkSnapshot(args.workspace)
    compiler_evidence = snapshot.compiler_evidence()
    results = []
    try:
        for group in selected:
            print(f"Checking {group}: copied handbook code, isolated SDK sources", flush=True)
            try:
                if group == "bridge":
                    result = run_bridge(snapshot, blocks)
                elif group == "downstream":
                    result = run_downstream(snapshot, not args.online, target)
                elif group == "pins":
                    result = run_pins(snapshot, blocks, not args.online, target)
                else:
                    result = run_group(snapshot, blocks, group, not args.online, target)
                results.append(result)
                print(f"PASS {group}", flush=True)
            except (RuntimeError, OSError) as error:
                results.append({"id": group, "state": "failed", "reason": str(error)})
                print(f"FAIL {group}: {error}", file=sys.stderr, flush=True)
    finally:
        snapshot.close()
        lock.__exit__(None, None, None)
    if args.report:
        args.report.parent.mkdir(parents=True, exist_ok=True)
        args.report.write_text(json.dumps({"sourceFingerprint": snapshot.source_fingerprint, "snippetFingerprint": snippet_fingerprint,
                                         "compiler": compiler_evidence,
                                         "checks": results, "boundary": "host compile and mock only; no live provider or device acceptance"}, indent=2) + "\n")
    return int(any(result["state"] != "passed" for result in results))


if __name__ == "__main__":
    sys.exit(main())
