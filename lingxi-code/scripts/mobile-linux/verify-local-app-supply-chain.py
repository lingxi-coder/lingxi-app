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
EXPECTED_SWCS = {
    "@next/swc-linux-arm64-musl": "16.2.11",
    "@next/swc-linux-x64-musl": "16.2.11",
}
EXPECTED_WRITABLE_ROOTS = ["app", "components", "lib", "styles", "public"]

# Package-manager policy for the *rootfs* APK closure. npm and npx are now
# first-class members of the shipped developer environment, so only the
# alternative managers stay out — keeping them would give the guest three ways
# to resolve a dependency tree and make the lockfile contract unenforceable.
# The separate node_modules-scope list in stage-local-app-runtime.py still
# forbids npm, because a vendored copy inside the app's own node_modules is a
# different thing from the interpreter's package manager.
FORBIDDEN_PACKAGE_NAMES = {"corepack", "pnpm", "yarn"}
FORBIDDEN_EXECUTABLES = {
    "/usr/bin/corepack",
    "/usr/bin/pnpm",
    "/usr/bin/yarn",
}
APK_VERSION_RE = re.compile(r"[0-9][0-9A-Za-z._]*(?:_[a-z]+[0-9]*)?-r[0-9]+")
ALPINE_RELEASE_RE = re.compile(r"[0-9]+\.[0-9]+\.[0-9]+")
ALPINE_CDN = "https://dl-cdn.alpinelinux.org/alpine"
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


def expected_apk_packages(pins: dict) -> dict:
    """The pinned primary APK set, read from the pins rather than duplicated.

    The pins file is the single source of version truth: everything else in the
    tree is checked *against* it. Holding a second copy of these versions here
    is what let the template move to Node 24.18.1 while this module still said
    22.23.0, which took the whole release gate red.
    """
    packages = pins.get("runtime_packages")
    if not isinstance(packages, dict) or not packages:
        fail("local-app runtime pins must list runtime_packages")
    for name, version in packages.items():
        if not isinstance(name, str) or not name:
            fail("runtime_packages keys must be package names")
        if not isinstance(version, str) or not APK_VERSION_RE.fullmatch(version):
            fail(f"runtime_packages must pin an exact APK version: {name}={version!r}")
        if name in FORBIDDEN_PACKAGE_NAMES:
            fail(f"runtime_packages must not install a forbidden package manager: {name}")
    return packages


def validate_alpine_pin(pins: dict) -> dict:
    """Structural checks on the Alpine pin itself.

    Nothing here asserts a specific release — advancing Alpine is a pins edit,
    not a code edit. What must hold is that the pin is internally coherent and
    points only at official repositories.
    """
    alpine = pins.get("alpine")
    if not isinstance(alpine, dict):
        fail("local-app runtime pins must carry an `alpine` record")
    version = alpine.get("version")
    if not isinstance(version, str) or not ALPINE_RELEASE_RE.fullmatch(version):
        fail("alpine.version must be an exact three-part Alpine release")
    branch = "v" + ".".join(version.split(".")[:2])
    if alpine.get("branch") != branch:
        fail(f"alpine.branch must be {branch} for Alpine {version}")
    expected_repositories = [f"{ALPINE_CDN}/{branch}/main", f"{ALPINE_CDN}/{branch}/community"]
    if alpine.get("repositories") != expected_repositories:
        fail("alpine.repositories must be the official main+community CDN URLs for the pinned branch")
    minirootfs = alpine.get("minirootfs")
    if not isinstance(minirootfs, dict) or not minirootfs:
        fail("alpine.minirootfs must pin the release tarball per architecture")
    for arch, record in minirootfs.items():
        if not isinstance(record, dict):
            fail(f"invalid minirootfs pin for {arch}")
        expected_url = (
            f"{ALPINE_CDN}/{branch}/releases/{arch}/alpine-minirootfs-{version}-{arch}.tar.gz"
        )
        if record.get("url") != expected_url:
            fail(f"minirootfs URL for {arch} must be {expected_url}")
        if not valid_sha256(record.get("sha256")):
            fail(f"minirootfs pin for {arch} needs a SHA-256")
    return alpine


def validate_apk_pins(pins: dict, release: bool, apk_dir: pathlib.Path | None) -> None:
    if pins.get("schema_version") != 2:
        fail("local-app runtime pins must use schema_version 2")
    alpine = validate_alpine_pin(pins)
    EXPECTED_APK_PACKAGES = expected_apk_packages(pins)
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
            # Pin the whole URL, not just its tail. A suffix match accepts any
            # host and any branch, which is precisely what a supply-chain pin
            # exists to prevent.
            section = artifact.get("repository")
            if section not in ("main", "community"):
                fail(f"APK artifact must record repository main|community for {abi}: {name}")
            alpine_arch = abi_record.get("alpine_arch")
            expected_url = (
                f"{ALPINE_CDN}/{alpine['branch']}/{section}/{alpine_arch}/{name}-{version}.apk"
            )
            if artifact.get("url") != expected_url:
                fail(f"APK URL for {abi}/{name} must be {expected_url}")
            if artifact.get("arch") != alpine_arch:
                fail(f"APK artifact arch must be {alpine_arch} for {abi}: {name}")
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


def expected_node(pins: dict) -> str:
    """Node version the template must pin, read from the pins.

    Derived rather than duplicated: the Node version appears in the pins, the
    template package.json, the lockfile, and the Alpine `nodejs` APK, and a
    second hardcoded copy here is what let three of them advance while the
    fourth silently stayed behind.
    """
    runtime = pins.get("next_runtime")
    if not isinstance(runtime, dict):
        fail("local-app runtime pins must carry a next_runtime record")
    version = runtime.get("node")
    if not isinstance(version, str) or not re.fullmatch(r"\d+\.\d+\.\d+", version):
        fail("next_runtime.node must be an exact three-part Node version")
    apk_version = pins.get("runtime_packages", {}).get("nodejs")
    if not isinstance(apk_version, str) or not apk_version.startswith(f"{version}-r"):
        fail(
            f"runtime_packages.nodejs ({apk_version!r}) must be the Alpine build of "
            f"next_runtime.node ({version})"
        )
    return version


def validate_lock(template: pathlib.Path, pins: dict) -> None:
    package_json = load_json(template / "package.json")
    lock = load_json(template / "package-lock.json")
    EXPECTED_NODE = expected_node(pins)
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
        # Checked against the file on disk rather than a literal, so a lockfile
        # edit that forgets to refresh the pin is caught as drift instead of
        # being frozen into a constant that has to be hand-updated in lockstep.
        "lockfile_sha256": hashlib.sha256(
            (template / "package-lock.json").read_bytes()
        ).hexdigest(),
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
        "media-src 'self' data: blob:",
        "worker-src 'none'",
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
    node_path = "/opt/lingxi/local-app-runtime/node_modules"
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
            or command.get("environment")
            != {
                "LINGXI_APP_OUTPUT": output,
                "NODE_ENV": "production",
                "NODE_PATH": node_path,
            }
            or command.get("network_policy") != "disabled"
            or command.get("memory_limit_bytes") != 800 * 1024 * 1024
            or command.get("timeout_ms") != 180000
        ):
            fail(f"fixed build command diverged: {name}")
    start = commands.get("full_start")
    if (
        not isinstance(start, dict)
        or start.get("argv")
        != [node, next_binary, "start", "--hostname", "127.0.0.1", "--port", "{loopback_port}"]
        or start.get("environment")
        != {
            "LINGXI_APP_OUTPUT": "server",
            "NODE_ENV": "production",
            "NODE_PATH": node_path,
        }
        or start.get("network_policy") != "loopback_only"
        or start.get("memory_limit_bytes") != 800 * 1024 * 1024
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
        "npm_family_present": True,
        "npm_scope": "interactive_terminal_only",
    }:
        fail("local-app package-manager policy diverged")

    launcher = policy.get("android_network_policy_launcher")
    expected_source = "clients/android/app/src/main/cpp/mobile_linux_policy_launcher.c"
    expected_overlay_source = "clients/android/app/src/main/cpp/proot_lingxi_network_policy.c"
    expected_artifact = "libmobile_linux_policy_launcher.so"
    if not isinstance(launcher, dict):
        fail("local-app runtime policy is missing the Android network-policy launcher")
    if (
        launcher.get("source") != expected_source
        or launcher.get("proot_overlay_source") != expected_overlay_source
        or launcher.get("artifact") != expected_artifact
        or launcher.get("supported_network_policies") != ["disabled", "loopback_only"]
        or launcher.get("loopback_only_ready") is not True
        or launcher.get("abis") != ["arm64-v8a", "x86_64"]
        or launcher.get("variants") != ["play", "direct"]
        or not valid_sha256(launcher.get("source_sha256"))
        or not valid_sha256(launcher.get("proot_overlay_source_sha256"))
    ):
        fail("Android network-policy launcher policy diverged")
    source = repo / expected_source
    try:
        actual_source_sha256 = hashlib.sha256(source.read_bytes()).hexdigest()
    except OSError as exc:
        fail(f"missing Android network-policy launcher source: {exc}")
    if actual_source_sha256 != launcher["source_sha256"]:
        fail("Android network-policy launcher source SHA-256 diverged")
    overlay_source = repo / expected_overlay_source
    try:
        actual_overlay_sha256 = hashlib.sha256(overlay_source.read_bytes()).hexdigest()
    except OSError as exc:
        fail(f"missing Android PRoot network-policy overlay source: {exc}")
    if actual_overlay_sha256 != launcher["proot_overlay_source_sha256"]:
        fail("Android PRoot network-policy overlay source SHA-256 diverged")

    build_script = repo / "clients/android/scripts/build-mobile-linux-native.sh"
    verify_script = repo / "clients/android/scripts/verify-mobile-linux-native.sh"
    try:
        build_text = build_script.read_text(encoding="utf-8")
        verify_text = verify_script.read_text(encoding="utf-8")
    except OSError as exc:
        fail(f"missing Android native launcher packaging script: {exc}")
    build_tokens = {
        "app/src/main/cpp/mobile_linux_policy_launcher.c",
        expected_artifact,
        'build_policy_launcher "arm64-v8a"',
        'build_policy_launcher "x86_64"',
    }
    if any(token not in build_text for token in build_tokens):
        fail("Android build script does not package the network-policy launcher for both ABIs")
    if expected_artifact not in verify_text or not all(
        abi in verify_text for abi in launcher["abis"]
    ):
        fail("Android native verifier does not require the network-policy launcher for both ABIs")

    ish_policy = policy.get("ios_ish_execution_policy")
    ish_sources = {
        "registry_source": (
            "registry_source_sha256",
            "clients/ios/Sources/LinuxRuntimeNative/LXISHExecutionPolicy.c",
        ),
        "header_source": (
            "header_source_sha256",
            "clients/ios/Sources/LinuxRuntimeNative/LXISHExecutionPolicy.h",
        ),
        "ish_patch_source": (
            "ish_patch_source_sha256",
            "clients/ios/Sources/LinuxRuntimeNative/patches/ish-socket-network-policy.patch",
        ),
    }
    if not isinstance(ish_policy, dict):
        fail("local-app runtime policy is missing the iOS iSH execution policy")
    if (
        ish_policy.get("hook_version") != 1
        or ish_policy.get("supported_network_policies") != ["disabled", "loopback_only"]
        or ish_policy.get("loopback_only_ready") is not True
        or ish_policy.get("memory_limit_bytes") != 800 * 1024 * 1024
        or ish_policy.get("watchdog_interval_ms") != 250
        or ish_policy.get("memory_accounting")
        != "guest_backed_pages_by_execution_context"
        or ish_policy.get("platform") != "iphoneos"
    ):
        fail("iOS iSH execution-policy contract diverged")
    for source_key, (digest_key, expected_path) in ish_sources.items():
        digest = ish_policy.get(digest_key)
        if ish_policy.get(source_key) != expected_path or not valid_sha256(digest):
            fail(f"iOS iSH execution-policy source declaration diverged: {source_key}")
        try:
            actual_digest = hashlib.sha256((repo / expected_path).read_bytes()).hexdigest()
        except OSError as exc:
            fail(f"missing iOS iSH execution-policy source: {exc}")
        if actual_digest != digest:
            fail(f"iOS iSH execution-policy source SHA-256 diverged: {expected_path}")

    ios_build_script = repo / "clients/ios/scripts/build-linux-runtime.sh"
    try:
        ios_build_text = ios_build_script.read_text(encoding="utf-8")
    except OSError as exc:
        fail(f"missing iOS iSH runtime build script: {exc}")
    required_ish_build_tokens = {
        "ish-socket-network-policy.patch",
        'git -C "${ISH_SOURCE}" apply --check "${ISH_NETWORK_POLICY_PATCH}"',
        'git -C "${ISH_SOURCE}" apply "${ISH_NETWORK_POLICY_PATCH}"',
        "restore_ish_policy_source",
    }
    if any(token not in ios_build_text for token in required_ish_build_tokens):
        fail("iOS build script does not apply and restore the pinned iSH policy patch")


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
