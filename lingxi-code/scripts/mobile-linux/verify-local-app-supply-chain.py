#!/usr/bin/env python3
import argparse
import base64
import hashlib
import json
import pathlib
import re
import sys


EXPECTED_DEPENDENCIES = {
    "next": "16.2.11",
    "react": "19.2.8",
    "react-dom": "19.2.8",
}
EXPECTED_NODE = "22.23.0"
EXPECTED_APK_PACKAGES = {
    "git": "2.47.3-r0",
    "nodejs": "22.23.0-r0",
}
EXPECTED_SWCS = {
    "@next/swc-linux-arm64-musl": "16.2.11",
    "@next/swc-linux-x64-musl": "16.2.11",
}
EXPECTED_WRITABLE_ROOTS = ["app", "components", "lib", "styles", "public"]
FORBIDDEN_PACKAGE_NAMES = {"corepack", "nodejs-npm", "npm", "pnpm", "yarn"}
FORBIDDEN_EXECUTABLES = {
    "/usr/bin/corepack",
    "/usr/bin/npm",
    "/usr/bin/npx",
    "/usr/bin/pnpm",
    "/usr/bin/yarn",
}
SOURCE_SUFFIXES = {
    ".css",
    ".htm",
    ".html",
    ".js",
    ".jsx",
    ".mjs",
    ".svg",
    ".ts",
    ".tsx",
    ".xml",
}
FORBIDDEN_SOURCE_PATTERNS = {
    "direct network access": re.compile(r"\b(fetch|XMLHttpRequest|WebSocket|EventSource)\s*\("),
    "dynamic code evaluation": re.compile(r"\b(eval|Function)\s*\("),
    "external script": re.compile(r"<script\b[^>]*\bsrc\s*=", re.IGNORECASE),
    "package manager invocation": re.compile(r"\b(npm|npx|corepack|yarn|pnpm)\b\s+(install|add|exec|dlx)\b"),
    "server action": re.compile(r"^[\t ]*[\"']use server[\"'];?", re.MULTILINE),
}


def fail(message: str) -> None:
    print(message, file=sys.stderr)
    raise SystemExit(1)


def load_json(path: pathlib.Path) -> dict:
    try:
        value = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as exc:
        fail(f"invalid JSON {path}: {exc}")
    if not isinstance(value, dict):
        fail(f"JSON root must be an object: {path}")
    return value


def valid_sha256(value: object) -> bool:
    return isinstance(value, str) and re.fullmatch(r"[0-9a-f]{64}", value) is not None


def validate_apk_pins(pins: dict, release: bool, apk_dir: pathlib.Path | None) -> None:
    if pins.get("schema_version") != 1:
        fail("local-app runtime pins must use schema_version 1")
    alpine = pins.get("alpine")
    if not isinstance(alpine, dict) or alpine.get("version") != "3.21.3" or alpine.get("branch") != "v3.21":
        fail("local-app runtime must pin Alpine 3.21.3 / v3.21")
    if pins.get("runtime_packages") != EXPECTED_APK_PACKAGES:
        fail("local-app APK package versions diverged from the product pins")
    if set(pins.get("forbidden_packages", [])) != FORBIDDEN_PACKAGE_NAMES:
        fail("forbidden package-manager package set diverged")
    if set(pins.get("forbidden_executables", [])) != FORBIDDEN_EXECUTABLES:
        fail("forbidden package-manager executable set diverged")

    by_abi = pins.get("apk_artifacts")
    if not isinstance(by_abi, dict) or set(by_abi) != {"arm64-v8a", "x86_64"}:
        fail("APK pins must cover exactly arm64-v8a and x86_64")
    for abi, abi_record in by_abi.items():
        if not isinstance(abi_record, dict):
            fail(f"invalid APK pin record for {abi}")
        artifacts = abi_record.get("artifacts")
        if not isinstance(artifacts, list):
            fail(f"APK pins missing artifacts for {abi}")
        primary_identities = set()
        all_identities = set()
        for artifact in artifacts:
            if not isinstance(artifact, dict):
                fail(f"invalid APK artifact for {abi}")
            name = artifact.get("name")
            version = artifact.get("version")
            role = artifact.get("role")
            if not isinstance(name, str) or not name or not isinstance(version, str) or not version:
                fail(f"incomplete APK identity for {abi}")
            if name in FORBIDDEN_PACKAGE_NAMES:
                fail(f"forbidden package-manager APK in closure for {abi}: {name}")
            identity = (name, version)
            if identity in all_identities:
                fail(f"duplicate APK identity for {abi}: {name}={version}")
            all_identities.add(identity)
            if role == "primary":
                if EXPECTED_APK_PACKAGES.get(name) != version:
                    fail(f"unexpected primary APK identity for {abi}: {name}={version}")
                primary_identities.add(name)
            elif role != "transitive":
                fail(f"APK artifact role must be primary or transitive for {abi}: {name}")
            expected_suffix = f"/{abi_record.get('alpine_arch')}/{name}-{version}.apk"
            if not str(artifact.get("url", "")).endswith(expected_suffix):
                fail(f"APK URL does not match its identity for {abi}: {name}")
            availability = artifact.get("availability")
            digest = artifact.get("sha256")
            if availability == "available":
                if not valid_sha256(digest):
                    fail(f"available APK must have a SHA-256 pin for {abi}: {name}")
            elif availability == "unavailable":
                if digest is not None:
                    fail(f"unavailable APK must not contain a fabricated digest for {abi}: {name}")
            else:
                fail(f"invalid APK availability for {abi}: {name}")
        if primary_identities != set(EXPECTED_APK_PACKAGES):
            fail(f"APK primary pin set incomplete for {abi}")
        if abi_record.get("closure_status") != "complete":
            blocker = abi_record.get("blocker")
            if not isinstance(blocker, str) or not blocker.strip():
                fail(f"incomplete APK closure must include a blocker for {abi}")
            if release:
                fail(f"release blocked for {abi}: {blocker}")

        if release:
            if apk_dir is None:
                fail("--apk-dir is required for release verification")
            abi_dir = apk_dir / abi
            for artifact in artifacts:
                if artifact.get("availability") != "available":
                    fail(f"release APK is unavailable for {abi}: {artifact.get('name')}")
                filename = pathlib.PurePosixPath(artifact["url"]).name
                package_path = abi_dir / filename
                if not package_path.is_file() or package_path.is_symlink():
                    fail(f"release APK is missing or unsafe: {package_path}")
                actual = hashlib.sha256(package_path.read_bytes()).hexdigest()
                if actual != artifact["sha256"]:
                    fail(f"release APK SHA-256 mismatch: {package_path}")

    if pins.get("release_ready") is not all(
        record.get("closure_status") == "complete" for record in by_abi.values()
    ):
        fail("release_ready must reflect APK closure completeness")


def validate_lock(template: pathlib.Path, pins: dict) -> None:
    package_json = load_json(template / "package.json")
    lock = load_json(template / "package-lock.json")
    if package_json.get("engines") != {"node": EXPECTED_NODE}:
        fail("template package.json must pin Node exactly")
    if package_json.get("dependencies") != EXPECTED_DEPENDENCIES:
        fail("template package.json dependencies must match the fixed runtime")
    if "scripts" in package_json:
        fail("template package.json must not expose package-manager scripts on device")

    packages = lock.get("packages")
    if not isinstance(packages, dict):
        fail("package-lock.json missing packages")
    root = packages.get("")
    if not isinstance(root, dict) or root.get("dependencies") != EXPECTED_DEPENDENCIES:
        fail("package-lock root dependencies diverged")
    if root.get("engines") != {"node": EXPECTED_NODE}:
        fail("package-lock root Node pin diverged")

    for path, package in packages.items():
        if path == "":
            continue
        if not isinstance(package, dict) or not isinstance(package.get("version"), str):
            fail(f"package-lock entry has no exact version: {path}")
        integrity = package.get("integrity")
        if not isinstance(integrity, str) or re.fullmatch(r"sha512-[A-Za-z0-9+/]+={0,2}", integrity) is None:
            fail(f"package-lock entry has no npm SHA-512 integrity: {path}")
        try:
            base64.b64decode(integrity.removeprefix("sha512-"), validate=True)
        except ValueError as exc:
            fail(f"package-lock integrity is invalid for {path}: {exc}")

    for name, version in EXPECTED_DEPENDENCIES.items():
        entry = packages.get(f"node_modules/{name}")
        if not isinstance(entry, dict) or entry.get("version") != version:
            fail(f"package-lock did not resolve {name}@{version}")
    for name, version in EXPECTED_SWCS.items():
        entry = packages.get(f"node_modules/{name}")
        if not isinstance(entry, dict) or entry.get("version") != version:
            fail(f"package-lock did not pin {name}@{version}")

    runtime = pins.get("next_runtime")
    expected_runtime = {
        "template": "lingxi-code/local-apps/templates/next-static-v1",
        "node": EXPECTED_NODE,
        "next": EXPECTED_DEPENDENCIES["next"],
        "react": EXPECTED_DEPENDENCIES["react"],
        "react_dom": EXPECTED_DEPENDENCIES["react-dom"],
        "swc": EXPECTED_SWCS,
        "lockfile": "lingxi-code/local-apps/templates/next-static-v1/package-lock.json",
        "lockfile_sha256": "2575ec5d1740fc6a652bbbb7b7e72bbd7f809aa0b48579ae6a7623eda9d63781",
    }
    if runtime != expected_runtime:
        fail("Next runtime pin manifest diverged from the template lock")
    lock_digest = hashlib.sha256((template / "package-lock.json").read_bytes()).hexdigest()
    if lock_digest != runtime["lockfile_sha256"]:
        fail("package-lock bytes diverged from the pinned SHA-256")


def validate_source_policy(template: pathlib.Path) -> None:
    policy = load_json(template / ".lingxi" / "source-policy.json")
    if policy.get("agent_writable_roots") != EXPECTED_WRITABLE_ROOTS:
        fail("agent writable roots must match the fixed source policy")
    allowed_top_level = set(EXPECTED_WRITABLE_ROOTS) | {
        ".lingxi",
        "next.config.mjs",
        "package-lock.json",
        "package.json",
    }
    for path in template.rglob("*"):
        if path.is_symlink():
            fail(f"symbolic links are forbidden in the app template: {path}")
        relative = path.relative_to(template)
        if relative.parts[0] not in allowed_top_level:
            fail(f"path is outside the fixed app workspace roots: {relative}")
        if not path.is_file() or path.suffix not in SOURCE_SUFFIXES:
            continue
        if relative.parts[0] not in EXPECTED_WRITABLE_ROOTS and relative.name not in {"next.config.mjs"}:
            continue
        if relative.name in {"route.js", "route.jsx", "route.ts", "route.tsx"}:
            fail(f"API routes are forbidden: {relative}")
        text = path.read_text(encoding="utf-8")
        for label, pattern in FORBIDDEN_SOURCE_PATTERNS.items():
            if pattern.search(text):
                fail(f"forbidden {label} in {relative}")

    next_config = (template / "next.config.mjs").read_text(encoding="utf-8")
    required_csp = {
        "default-src 'self'",
        "script-src 'self' 'unsafe-inline'",
        "connect-src 'self'",
        "object-src 'none'",
        "base-uri 'none'",
        "frame-ancestors 'none'",
        "form-action 'self'",
    }
    missing_csp = sorted(directive for directive in required_csp if directive not in next_config)
    if missing_csp:
        fail(f"Next server CSP is incomplete: {missing_csp}")
    if 'outputMode === "server"' not in next_config or "async headers()" not in next_config:
        fail("Next security headers must be server-mode-only")


def validate_sbom(repo: pathlib.Path, template: pathlib.Path) -> None:
    sbom = load_json(repo / "docs" / "mobile-linux" / "sbom" / "local-app-runtime.spdx.json")
    if sbom.get("spdxVersion") != "SPDX-2.3":
        fail("local-app runtime SBOM must use SPDX 2.3")
    packages = sbom.get("packages")
    if not isinstance(packages, list):
        fail("local-app runtime SBOM missing packages")
    lock = load_json(template / "package-lock.json")
    lock_entries = {path: package for path, package in lock["packages"].items() if path}
    if len(packages) != len(lock_entries):
        fail(
            "local-app runtime SBOM package count diverged from package-lock "
            f"(sbom={len(packages)}, lock={len(lock_entries)})"
        )
    sbom_by_path = {}
    for package in packages:
        if not isinstance(package, dict):
            fail("local-app runtime SBOM packages must be objects")
        source_info = package.get("sourceInfo")
        prefix = "npm package-lock path: "
        if not isinstance(source_info, str) or not source_info.startswith(prefix):
            fail("local-app runtime SBOM package is missing its lock path")
        path = source_info.removeprefix(prefix)
        if path in sbom_by_path:
            fail(f"duplicate package-lock path in SBOM: {path}")
        sbom_by_path[path] = package
    if set(sbom_by_path) != set(lock_entries):
        fail("local-app runtime SBOM paths diverged from package-lock")

    for path, lock_entry in lock_entries.items():
        sbom_entry = sbom_by_path[path]
        name = path.rsplit("node_modules/", 1)[1]
        if sbom_entry.get("name") != name or sbom_entry.get("versionInfo") != lock_entry.get("version"):
            fail(f"SBOM identity does not match package-lock: {path}")
        expected_hex = base64.b64decode(lock_entry["integrity"].split("-", 1)[1]).hex()
        checksums = sbom_entry.get("checksums", [])
        if {"algorithm": "SHA512", "checksumValue": expected_hex} not in checksums:
            fail(f"SBOM checksum does not match package-lock integrity: {path}")


def validate_runtime_policy(repo: pathlib.Path) -> None:
    policy = load_json(repo / "docs" / "mobile-linux" / "local-app-runtime-policy.json")
    node = "/usr/bin/node"
    next_binary = "/opt/lingxi/local-app-runtime/node_modules/next/dist/bin/next"
    if policy.get("schema_version") != 1:
        fail("local-app runtime policy must use schema_version 1")
    if policy.get("node_executable") != node or policy.get("next_executable") != next_binary:
        fail("local-app runtime command paths diverged")
    mount = policy.get("node_modules_mount")
    if not isinstance(mount, dict) or mount.get("target") != "/opt/lingxi/local-app-runtime/node_modules" or mount.get("read_only") is not True:
        fail("node_modules must use the fixed read-only mount")
    commands = policy.get("commands")
    expected_build = [node, next_binary, "build"]
    if not isinstance(commands, dict):
        fail("local-app runtime policy missing commands")
    for name, output in (("store_build", "export"), ("full_build", "server")):
        command = commands.get(name)
        if (
            not isinstance(command, dict)
            or command.get("argv") != expected_build
            or command.get("environment") != {"LINGXI_APP_OUTPUT": output, "NODE_ENV": "production"}
            or command.get("timeout_ms") != 180000
        ):
            fail(f"fixed build command diverged: {name}")
    start = commands.get("full_start")
    if (
        not isinstance(start, dict)
        or start.get("argv")
        != [node, next_binary, "start", "--hostname", "127.0.0.1", "--port", "{loopback_port}"]
        or start.get("environment") != {"LINGXI_APP_OUTPUT": "server", "NODE_ENV": "production"}
        or start.get("cold_start_timeout_ms") != 120000
        or start.get("warm_start_timeout_ms") != 30000
    ):
        fail("fixed production start command diverged")
    limits = policy.get("limits")
    if not isinstance(limits, dict) or limits.get("build_concurrency") != 1 or limits.get("node_process_tree_memory_bytes") != 800 * 1024 * 1024:
        fail("local-app build concurrency or memory limit diverged")
    package_policy = policy.get("package_manager_policy")
    if package_policy != {
        "interactive_terminal_apk": True,
        "generation_jobs": False,
        "mcp": False,
        "npm_family_present": False,
    }:
        fail("local-app package-manager policy diverged")


def validate_create_skill(repo: pathlib.Path) -> None:
    skill_path = repo / "skills" / "create-local-app" / "SKILL.md"
    try:
        text = skill_path.read_text(encoding="utf-8")
    except OSError as exc:
        fail(f"missing create-local-app skill: {exc}")
    if not text.startswith("---\nname: create-local-app\ndescription: "):
        fail("create-local-app skill frontmatter is invalid")
    required_tokens = {
        "mcp__local_apps__list",
        "mcp__local_apps__create",
        "mcp__local_apps__propose_design",
        "mcp__local_apps__query_data",
        "mcp__local_apps__mutate_data",
        "mcp__local_apps__inspect_ui",
        "mcp__local_apps__act_on_ui",
        "mcp__local_apps__restore_checkpoint",
        "next-static-v1",
        "window.lingxi.v1",
    }
    missing = sorted(token for token in required_tokens if token not in text)
    if missing:
        fail(f"create-local-app skill is missing host contract tokens: {missing}")


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--repo-root", required=True)
    parser.add_argument("--release", action="store_true")
    parser.add_argument("--apk-dir")
    parser.add_argument("--template")
    args = parser.parse_args()

    repo = pathlib.Path(args.repo_root).resolve()
    pins = load_json(repo / "docs" / "mobile-linux" / "local-app-runtime-pins.json")
    template = (
        pathlib.Path(args.template).resolve()
        if args.template
        else repo / pins.get("next_runtime", {}).get("template", "")
    )
    validate_apk_pins(
        pins,
        release=args.release,
        apk_dir=pathlib.Path(args.apk_dir).resolve() if args.apk_dir else None,
    )
    validate_lock(template, pins)
    validate_source_policy(template)
    validate_sbom(repo, template)
    validate_runtime_policy(repo)
    validate_create_skill(repo)
    print("local-app supply-chain pins verified")


if __name__ == "__main__":
    main()
