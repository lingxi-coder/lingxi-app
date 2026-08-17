#!/usr/bin/env python3
import argparse
import base64
import hashlib
import json
import pathlib
import re
import sys


EXPECTED_DEPENDENCIES = {
    "react": "19.2.8",
    "react-dom": "19.2.8",
    "vite": "8.2.1",
}
EXPECTED_SCRIPTS = {
    "build": "vite build",
    "dev": "vite",
    "preview": "vite preview",
}
EXPECTED_ROLLDOWN_BINDINGS = {
    "@rolldown/binding-linux-arm64-musl": "1.2.3",
    "@rolldown/binding-linux-x64-musl": "1.2.3",
}
EXPECTED_LIGHTNINGCSS_BINDINGS = {
    "lightningcss-linux-arm64-musl": "1.33.0",
    "lightningcss-linux-x64-musl": "1.33.0",
}
EXPECTED_NATIVE_PACKAGE_BINARIES = {
    "@rolldown/binding-linux-arm64-musl": "rolldown-binding.linux-arm64-musl.node",
    "@rolldown/binding-linux-x64-musl": "rolldown-binding.linux-x64-musl.node",
    "@rolldown/binding-linux-arm64-gnu": "rolldown-binding.linux-arm64-gnu.node",
    "@rolldown/binding-linux-x64-gnu": "rolldown-binding.linux-x64-gnu.node",
    "lightningcss-linux-arm64-musl": "lightningcss.linux-arm64-musl.node",
    "lightningcss-linux-x64-musl": "lightningcss.linux-x64-musl.node",
    "lightningcss-linux-arm64-gnu": "lightningcss.linux-arm64-gnu.node",
    "lightningcss-linux-x64-gnu": "lightningcss.linux-x64-gnu.node",
}
EXPECTED_NATIVE_PACKAGE_LIBC = {
    "@rolldown/binding-linux-arm64-musl": "musl",
    "@rolldown/binding-linux-x64-musl": "musl",
    "@rolldown/binding-linux-arm64-gnu": "glibc",
    "@rolldown/binding-linux-x64-gnu": "glibc",
    "lightningcss-linux-arm64-musl": "musl",
    "lightningcss-linux-x64-musl": "musl",
    "lightningcss-linux-arm64-gnu": "glibc",
    "lightningcss-linux-x64-gnu": "glibc",
}
EXPECTED_ROLLDOWN_VERSION = "1.2.3"
EXPECTED_LIGHTNINGCSS_VERSION = "1.33.0"
EXPECTED_WRITABLE_ROOTS = ["app", "components", "lib", "styles", "public"]
VITE_EXPECTED_WRITABLE_ROOTS = EXPECTED_WRITABLE_ROOTS + ["src"]
FORBIDDEN_ROUTE_FILES = {"route.js", "route.jsx", "route.ts", "route.tsx"}

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
    # Third-party scripts only. A same-origin RELATIVE src (`/x.js`, `./x.js`)
    # is Vite's required entry form and loads nothing the app did not ship, so
    # it is allowed; anything with a scheme (`https:`, `data:`, `javascript:`),
    # a protocol-relative `//host`, a `..` escape, a bare unrooted path, or a
    # dynamic expression still fails.
    "external script": re.compile(
        r"<script\b[^>]*?\bsrc\s*=\s*(?![\"']?\.?/(?!/))",
        re.IGNORECASE,
    ),
    "package manager invocation": re.compile(r"\b(npm|npx|corepack|yarn|pnpm)\b\s+(install|add|exec|dlx)\b"),
    "server action": re.compile(r"^[\t ]*[\"']use server[\"'];?", re.MULTILINE),
}


def expected_native_packages_for(platform: str, family: str) -> dict[str, str]:
    if family == "rolldown":
        bindings = EXPECTED_ROLLDOWN_BINDINGS
        arm64 = "@rolldown/binding-linux-arm64-musl"
    elif family == "lightningcss":
        bindings = EXPECTED_LIGHTNINGCSS_BINDINGS
        arm64 = "lightningcss-linux-arm64-musl"
    else:
        fail(f"unknown native package family: {family}")
    if platform == "ios":
        return {arm64: bindings[arm64]}
    if platform == "android":
        return dict(bindings)
    fail(f"unknown runtime platform: {platform}")


def expected_native_binary_for(package_name: str) -> str:
    binary = EXPECTED_NATIVE_PACKAGE_BINARIES.get(package_name)
    if binary is None:
        fail(f"missing expected binary metadata for native package: {package_name}")
    return binary


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
    runtime = pins.get("local_app_runtime")
    if not isinstance(runtime, dict):
        fail("local-app runtime pins must carry a local_app_runtime record")
    version = runtime.get("node")
    if not isinstance(version, str) or not re.fullmatch(r"\d+\.\d+\.\d+", version):
        fail("local_app_runtime.node must be an exact three-part Node version")
    apk_version = pins.get("runtime_packages", {}).get("nodejs")
    if not isinstance(apk_version, str) or not apk_version.startswith(f"{version}-r"):
        fail(
            f"runtime_packages.nodejs ({apk_version!r}) must be the Alpine build of "
            f"local_app_runtime.node ({version})"
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
    if package_json.get("scripts") != EXPECTED_SCRIPTS:
        fail("template package.json must expose the standard Vite scripts only")

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
    if "node_modules/next" in packages or any(
        path.startswith("node_modules/@next/") for path in packages
    ):
        fail("package-lock must not retain Next.js or SWC packages")
    rolldown = packages.get("node_modules/rolldown")
    if not isinstance(rolldown, dict) or rolldown.get("version") != EXPECTED_ROLLDOWN_VERSION:
        fail(f"package-lock did not pin rolldown@{EXPECTED_ROLLDOWN_VERSION}")
    for name, version in EXPECTED_ROLLDOWN_BINDINGS.items():
        entry = packages.get(f"node_modules/{name}")
        if not isinstance(entry, dict) or entry.get("version") != version:
            fail(f"package-lock did not pin {name}@{version}")
        if entry.get("libc") != [EXPECTED_NATIVE_PACKAGE_LIBC[name]]:
            fail(f"package-lock did not pin {name} libc={EXPECTED_NATIVE_PACKAGE_LIBC[name]}")
    for name, version in {
        "@rolldown/binding-linux-arm64-gnu": EXPECTED_ROLLDOWN_VERSION,
        "@rolldown/binding-linux-x64-gnu": EXPECTED_ROLLDOWN_VERSION,
    }.items():
        entry = packages.get(f"node_modules/{name}")
        if not isinstance(entry, dict) or entry.get("version") != version:
            fail(f"package-lock did not pin {name}@{version}")
        if entry.get("libc") != [EXPECTED_NATIVE_PACKAGE_LIBC[name]]:
            fail(f"package-lock did not pin {name} libc={EXPECTED_NATIVE_PACKAGE_LIBC[name]}")
    lightningcss = packages.get("node_modules/lightningcss")
    if (
        not isinstance(lightningcss, dict)
        or lightningcss.get("version") != EXPECTED_LIGHTNINGCSS_VERSION
    ):
        fail(f"package-lock did not pin lightningcss@{EXPECTED_LIGHTNINGCSS_VERSION}")
    for name, version in EXPECTED_LIGHTNINGCSS_BINDINGS.items():
        entry = packages.get(f"node_modules/{name}")
        if not isinstance(entry, dict) or entry.get("version") != version:
            fail(f"package-lock did not pin {name}@{version}")
        if entry.get("libc") != [EXPECTED_NATIVE_PACKAGE_LIBC[name]]:
            fail(f"package-lock did not pin {name} libc={EXPECTED_NATIVE_PACKAGE_LIBC[name]}")
    for name, version in {
        "lightningcss-linux-arm64-gnu": EXPECTED_LIGHTNINGCSS_VERSION,
        "lightningcss-linux-x64-gnu": EXPECTED_LIGHTNINGCSS_VERSION,
    }.items():
        entry = packages.get(f"node_modules/{name}")
        if not isinstance(entry, dict) or entry.get("version") != version:
            fail(f"package-lock did not pin {name}@{version}")
        if entry.get("libc") != [EXPECTED_NATIVE_PACKAGE_LIBC[name]]:
            fail(f"package-lock did not pin {name} libc={EXPECTED_NATIVE_PACKAGE_LIBC[name]}")

    runtime = pins.get("local_app_runtime")
    expected_runtime = {
        "template": "lingxi-code/local-apps/templates/vite-react-static-v1",
        "node": EXPECTED_NODE,
        "react": EXPECTED_DEPENDENCIES["react"],
        "react_dom": EXPECTED_DEPENDENCIES["react-dom"],
        "vite": EXPECTED_DEPENDENCIES["vite"],
        "rolldown": EXPECTED_ROLLDOWN_VERSION,
        "rolldown_bindings": EXPECTED_ROLLDOWN_BINDINGS,
        "lightningcss": EXPECTED_LIGHTNINGCSS_VERSION,
        "lightningcss_bindings": EXPECTED_LIGHTNINGCSS_BINDINGS,
        "lockfile": "lingxi-code/local-apps/templates/vite-react-static-v1/package-lock.json",
        # Checked against the file on disk rather than a literal, so a lockfile
        # edit that forgets to refresh the pin is caught as drift instead of
        # being frozen into a constant that has to be hand-updated in lockstep.
        "lockfile_sha256": hashlib.sha256(
            (template / "package-lock.json").read_bytes()
        ).hexdigest(),
    }
    if runtime != expected_runtime:
        fail("local-app runtime pin manifest diverged from the template lock")
    lock_digest = hashlib.sha256((template / "package-lock.json").read_bytes()).hexdigest()
    if lock_digest != runtime["lockfile_sha256"]:
        fail("package-lock bytes diverged from the pinned SHA-256")


def validate_workspace_sources(
    template: pathlib.Path,
    writable_roots: list[str],
    top_level_files: set[str],
    description: str,
) -> None:
    """The one source-policy walk both app templates go through.

    Parametrized rather than copied: the Vite fallback used to carry a
    near-duplicate of this function that had silently dropped the API-route ban
    and the top-level-config escape hatch, so `index.html` and `vite.config.mjs`
    were never pattern-scanned at all. A single walk means the next rule added
    here lands on both templates by construction.

    `top_level_files` are the host-managed files that live outside every
    writable root and must still be scanned — the Next config, and the Vite
    entry HTML plus its config.
    """
    policy = load_json(template / ".lingxi" / "source-policy.json")
    if policy.get("agent_writable_roots") != writable_roots:
        fail(f"{description} agent writable roots must match the fixed source policy")
    allowed_top_level = (
        set(writable_roots)
        | top_level_files
        | {".lingxi", "package-lock.json", "package.json"}
    )
    for path in template.rglob("*"):
        if path.is_symlink():
            fail(f"symbolic links are forbidden in the {description}: {path}")
        relative = path.relative_to(template)
        if relative.parts[0] not in allowed_top_level:
            fail(f"path is outside the fixed app workspace roots: {relative}")
        if not path.is_file() or path.suffix not in SOURCE_SUFFIXES:
            continue
        if (
            relative.parts[0] not in writable_roots
            and relative.as_posix() not in top_level_files
        ):
            continue
        if relative.name in FORBIDDEN_ROUTE_FILES:
            fail(f"API routes are forbidden: {relative}")
        text = path.read_text(encoding="utf-8")
        for label, pattern in FORBIDDEN_SOURCE_PATTERNS.items():
            if pattern.search(text):
                fail(f"forbidden {label} in {relative}")


def validate_source_policy(template: pathlib.Path, pins: dict) -> None:
    validate_workspace_sources(
        template,
        writable_roots=VITE_EXPECTED_WRITABLE_ROOTS,
        top_level_files={"index.html", "vite.config.mjs"},
        description="app template",
    )
    source_policy = load_json(template / ".lingxi" / "source-policy.json")
    if source_policy.get("host_managed_paths") != [
        ".lingxi",
        "LINGXI.md",
        "index.html",
        "vite.config.mjs",
        "package.json",
        "package-lock.json",
        "lib/device-context.js",
        "lib/lingxi-bridge.js",
        "lib/platform-adapter.js",
        "node_modules",
    ]:
        fail("app template host-managed paths diverged from build enforcement")
    if "package_install" not in source_policy.get("forbidden_features", []):
        fail("app template must forbid package installation during generation")
    config = (template / "vite.config.mjs").read_text(encoding="utf-8")
    out_dir_values = re.findall(
        r'^[\t ]*outDir\s*:\s*["\']([^"\']+)["\']\s*,?[\t ]*(?://.*)?$',
        config,
        flags=re.MULTILINE,
    )
    if out_dir_values != ["dist"]:
        fail("fixed Vite build must use the official dist output directory")
    compressed_size_values = re.findall(
        r"^[\t ]*reportCompressedSize\s*:\s*(true|false)\s*,?[\t ]*(?://.*)?$",
        config,
        flags=re.MULTILINE,
    )
    if compressed_size_values != ["false"]:
        fail("fixed Vite build must disable compressed-size reporting")
    package_json = load_json(template / "package.json")
    if package_json.get("scripts") != EXPECTED_SCRIPTS:
        fail("canonical Vite template must expose the standard dev/build/preview scripts")
    manifest = load_json(template / ".lingxi" / "app.manifest.json")
    if manifest != {
        "schema_version": 1,
        "template_id": "vite-react-static-v1",
        "template_version": 1,
        "runtime_compatibility": ["store-static"],
        "dependencies": {
            "node": expected_node(pins),
            "react": EXPECTED_DEPENDENCIES["react"],
            "react-dom": EXPECTED_DEPENDENCIES["react-dom"],
            "vite": EXPECTED_DEPENDENCIES["vite"],
        },
        "capabilities": {"collections": [], "network_domains": []},
    }:
        fail("canonical Vite app manifest diverged")
    design_spec = load_json(template / ".lingxi" / "design-spec.json")
    if design_spec != {
        "schema_version": 1,
        "template_id": "vite-react-static-v1",
        "template_version": 1,
        "answers": {},
        "legacy_fields": {},
    }:
        fail("canonical Vite design spec stub diverged")


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
    build_root = "/var/lingxi/local-app-build/{app_id}/{channel}/project"
    guest_build_state_root = f"{build_root}/.lingxi-build-state"
    guest_home_root = f"{guest_build_state_root}/home"
    guest_temp_root = f"{guest_build_state_root}/tmp"
    guest_xdg_cache_root = f"{guest_build_state_root}/xdg-cache"
    guest_xdg_config_root = f"{guest_build_state_root}/xdg-config"
    guest_xdg_data_root = f"{guest_build_state_root}/xdg-data"
    guest_npm_cache_root = f"{guest_build_state_root}/npm-cache"
    vite_binary = f"{build_root}/node_modules/vite/bin/vite.js"
    if policy.get("schema_version") != 1:
        fail("local-app runtime policy must use schema_version 1")
    if policy.get("node_executable") != node or policy.get("vite_executable") != vite_binary:
        fail("local-app runtime command paths diverged")
    if "next_executable" in policy:
        fail("local-app runtime policy must not retain a Next executable path")
    if "node_modules_mount" in policy:
        fail("local-app runtime policy must not guest-mount shared node_modules")
    if "scaffold" in policy:
        fail("local-app runtime policy must not pin create-vite scaffolding policy")
    dependency_snapshot = policy.get("dependency_snapshot")
    if dependency_snapshot != {
        "source": "bundled:local-app-runtime/node_modules",
        "materialize_into": f"{build_root}/node_modules",
        "guest_mount": "forbidden",
        "selection_policy": "verified_runtime_only",
    }:
        fail("local-app dependency snapshot policy diverged")
    build_mount = policy.get("build_mount")
    if build_mount != {
        "kind": "LocalAppBuild",
        "count": 1,
        "host_path_policy": "staging_or_store_root",
        "guest_path": build_root,
        "writable": True,
    }:
        fail("local-app build mount policy diverged")
    commands = policy.get("commands")
    old_space_argument = "--max-old-space-size={build_node_old_space_size_mib}"
    if not isinstance(commands, dict):
        fail("local-app runtime policy missing commands")
    if set(commands) != {"vite_static_build"}:
        fail("local-app runtime policy must expose only the Vite static build command")
    vite_build = commands.get("vite_static_build")
    if (
        not isinstance(vite_build, dict)
        or vite_build.get("argv")
        != [node, old_space_argument, vite_binary, "build", "--outDir", "dist", "--emptyOutDir"]
        or vite_build.get("cwd") != build_root
        or vite_build.get("output_dir") != "dist"
        or vite_build.get("environment")
        != {
            "NODE_ENV": "production",
            "HOME": guest_home_root,
            "TMPDIR": guest_temp_root,
            "TMP": guest_temp_root,
            "TEMP": guest_temp_root,
            "XDG_CACHE_HOME": guest_xdg_cache_root,
            "XDG_CONFIG_HOME": guest_xdg_config_root,
            "XDG_DATA_HOME": guest_xdg_data_root,
            "NPM_CONFIG_CACHE": guest_npm_cache_root,
            "npm_config_cache": guest_npm_cache_root,
        }
        or vite_build.get("network_policy") != "disabled"
        or vite_build.get("memory_limit_policy") != "physical_memory_tier"
        or "memory_limit_bytes" in vite_build
        or vite_build.get("timeout_ms") != 30 * 60 * 1000
    ):
        fail("fixed Vite build command diverged")
    limits = policy.get("limits")
    expected_build_memory_tiers = [
        {
            "physical_memory_max_exclusive_bytes": 6 * 1024**3,
            "process_tree_memory_bytes": 2048 * 1024**2,
            "node_max_old_space_size_mib": 1536,
        },
        {
            "physical_memory_max_exclusive_bytes": 8 * 1024**3,
            "process_tree_memory_bytes": 3072 * 1024**2,
            "node_max_old_space_size_mib": 2304,
        },
        {
            "physical_memory_max_exclusive_bytes": None,
            "process_tree_memory_bytes": 4096 * 1024**2,
            "node_max_old_space_size_mib": 3072,
        },
    ]
    if (
        not isinstance(limits, dict)
        or limits.get("build_concurrency") != 1
        or limits.get("build_node_old_space_percent") != 75
        or limits.get("build_memory_tiers") != expected_build_memory_tiers
        or limits.get("runtime_process_tree_memory_bytes") != 800 * 1024 * 1024
        or "node_process_tree_memory_bytes" in limits
    ):
        fail("local-app build tiers or runtime memory limit diverged")
    if "package_manager_policy" in policy:
        fail("local-app runtime policy must not pin CLI scaffolding/package-manager policy")

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
        or ish_policy.get("runtime_memory_limit_bytes") != 800 * 1024 * 1024
        or "memory_limit_bytes" in ish_policy
        or ish_policy.get("watchdog_interval_ms") != 250
        or ish_policy.get("memory_accounting")
        != "guest_backed_pages_by_execution_context"
        or ish_policy.get("local_app_build_mount_layout")
        != "single_root_materialized_snapshot"
        or ish_policy.get("nested_bind_mount_resolution") != "longest_guest_prefix"
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

    ish_patch_path = repo / ish_sources["ish_patch_source"][1]
    try:
        ish_patch_text = ish_patch_path.read_text(encoding="utf-8")
    except OSError as exc:
        fail(f"missing iOS iSH execution-policy patch: {exc}")
    nested_bind_tokens = {
        "best_guest_match",
        "best_guest_path_len",
        "g_bind_mounts[best_guest_match].host_path",
    }
    if any(token not in ish_patch_text for token in nested_bind_tokens):
        fail("iOS iSH patch does not make nested bind mounts prefer the longest guest prefix")

    ios_build_script = repo / "clients/ios/scripts/build-linux-runtime.sh"
    try:
        ios_build_text = ios_build_script.read_text(encoding="utf-8")
    except OSError as exc:
        fail(f"missing iOS iSH runtime build script: {exc}")
    required_ish_build_tokens = {
        "ish-socket-network-policy.patch",
        'git -C "${ISH_SOURCE}" apply --unidiff-zero --check "${ISH_NETWORK_POLICY_PATCH}"',
        'git -C "${ISH_SOURCE}" apply --unidiff-zero "${ISH_NETWORK_POLICY_PATCH}"',
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
        "mcp__local_apps__create",
        "mcp__local_apps__update_manifest",
        "mcp__local_apps__build",
        "mcp__local_apps__manage_runtime",
        "mcp__local_apps__read_logs",
        "mcp__local_apps__inspect_ui",
        "mcp__local_apps__act_on_ui",
        "mcp__local_apps__query_data",
        "mcp__local_apps__mutate_data",
        "mcp__local_apps__restore_checkpoint",
        "window.lingxi.v1",
        "Do not call `mcp__local_apps__list` or `mcp__local_apps__get`",
        "call `AskUserQuestion`",
        "Never ask unresolved questions in ordinary assistant text",
        "Every collection requires `id`, `name`, and `fields`",
        "Never declare host-owned record metadata",
        "vite build --outDir dist --emptyOutDir",
        "build/store/dist/",
    }
    missing = sorted(token for token in required_tokens if token not in text)
    if missing:
        fail(f"create-local-app skill is missing host contract tokens: {missing}")
    local_apps_host_path = (
        repo / "lingxi-code" / "apps" / "engine-mobile" / "src" / "local_apps_host.rs"
    )
    try:
        local_apps_host = local_apps_host_path.read_text(encoding="utf-8")
    except OSError as exc:
        fail(f"missing local-apps host: {exc}")
    if "already bound to local app `{id}`" not in local_apps_host:
        fail("app-scoped LINGXI.md does not make its current app id authoritative")
    workflow_path = repo / "lingxi-code" / "tools" / "workflow" / "src" / "local_app_build_workflow.js"
    try:
        workflow = workflow_path.read_text(encoding="utf-8")
    except OSError as exc:
        fail(f"missing local-app-build workflow: {exc}")
    workflow_tokens = {
        "mcp__local_apps__build",
        "--outDir dist --emptyOutDir",
        "build/store/dist/",
    }
    missing_workflow = sorted(token for token in workflow_tokens if token not in workflow)
    if missing_workflow:
        fail(f"local-app-build workflow is missing Vite CLI contract tokens: {missing_workflow}")


def validate_product_model_name_absence(repo: pathlib.Path) -> None:
    """Keep the task-only model name out of product routing and generation."""
    forbidden = "gpt-5.6" + "luna"
    roots = [
        repo / "clients",
        repo / "lingxi-code" / "apps",
        repo / "lingxi-code" / "llm-client",
        repo / "lingxi-code" / "tools",
        repo / "skills",
    ]
    for root in roots:
        if not root.exists():
            continue
        for path in root.rglob("*"):
            if not path.is_file() or any(
                part in {".git", "target", "build", "node_modules"} for part in path.parts
            ):
                continue
            try:
                if forbidden in path.read_text(encoding="utf-8"):
                    fail(f"task-only model name leaked into product source: {path}")
            except (OSError, UnicodeDecodeError):
                continue


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--repo-root", required=True)
    parser.add_argument("--release", action="store_true")
    parser.add_argument("--apk-dir")
    parser.add_argument("--template")
    parser.add_argument("--vite-template")
    args = parser.parse_args()

    repo = pathlib.Path(args.repo_root).resolve()
    pins = load_json(repo / "docs" / "mobile-linux" / "local-app-runtime-pins.json")
    template = (
        pathlib.Path(args.template).resolve()
        if args.template
        else repo / pins.get("local_app_runtime", {}).get("template", "")
    )
    vite_template = (
        pathlib.Path(args.vite_template).resolve()
        if args.vite_template
        else template
    )
    validate_apk_pins(
        pins,
        release=args.release,
        apk_dir=pathlib.Path(args.apk_dir).resolve() if args.apk_dir else None,
    )
    validate_lock(template, pins)
    validate_source_policy(template, pins)
    if vite_template != template:
        validate_source_policy(vite_template, pins)
    validate_sbom(repo, template)
    validate_runtime_policy(repo)
    validate_create_skill(repo)
    validate_product_model_name_absence(repo)
    print("local-app supply-chain pins verified")


if __name__ == "__main__":
    main()
