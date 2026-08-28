#!/usr/bin/env python3
import argparse
import hashlib
import json
import pathlib
import re
import subprocess
import sys
import tempfile


EXPECTED_DEPENDENCIES = {
    # The UI kit. Ionic is what makes a generated app look native on BOTH
    # platforms from one source: `setupIonicReact({ mode })` selects the iOS or
    # the Material design language at runtime from the host's OS, which is a
    # thing the previous shadcn/Radix set could not do at all -- it is a web
    # design language, and the platform "adapter" that was supposed to bridge it
    # published fields (`stateLayer: "ripple"`) that had zero consumers.
    #
    # It must be imported from the `@ionic/react` barrel. The per-component
    # entry points under `@ionic/core/components` are the only tree-shakeable
    # path, but they dynamically import one another and rolldown rejects that
    # under the `iife` output format this build is pinned to.
    "@ionic/react": "9.0.0",
    # Native page transitions and the platform back gesture, via IonRouterOutlet.
    # It peers on react-router 6.x, which is why react-router is pinned to 6
    # rather than 7.
    "@ionic/react-router": "9.0.0",
    "@vitejs/plugin-react": "6.0.4",
    "react": "19.2.8",
    "react-dom": "19.2.8",
    "react-router": "6.30.6",
    "react-router-dom": "6.30.6",
    "vite": "8.2.1",
    "zod": "4.4.3",
    "zustand": "5.0.15",
}
EXPECTED_OVERRIDES = {"lightningcss": "1.33.0"}
EXPECTED_SCRIPTS = {
    "build": "vite build",
    "dev": "vite",
    "preview": "vite preview",
}
EXPECTED_ROLLDOWN_BINDINGS = {
    "@rolldown/binding-linux-arm64-musl": "1.2.6",
    "@rolldown/binding-linux-x64-musl": "1.2.6",
}
EXPECTED_LIGHTNINGCSS_BINDINGS = {
    "lightningcss-linux-arm64-musl": "1.33.0",
    "lightningcss-linux-x64-musl": "1.33.0",
}
EXPECTED_ROLLUP_BINDINGS = {
    "@rollup/rollup-linux-arm64-musl": "4.44.0",
    "@rollup/rollup-linux-x64-musl": "4.44.0",
}
EXPECTED_NATIVE_PACKAGE_BINARIES = {
    "@rolldown/binding-linux-arm64-musl": "rolldown-binding.linux-arm64-musl.node",
    "@rolldown/binding-linux-x64-musl": "rolldown-binding.linux-x64-musl.node",
    "@rollup/rollup-linux-arm64-musl": "rollup.linux-arm64-musl.node",
    "@rollup/rollup-linux-x64-musl": "rollup.linux-x64-musl.node",
    "lightningcss-linux-arm64-musl": "lightningcss.linux-arm64-musl.node",
    "lightningcss-linux-x64-musl": "lightningcss.linux-x64-musl.node",
}
EXPECTED_ROLLDOWN_VERSION = "1.2.6"
EXPECTED_LIGHTNINGCSS_VERSION = "1.33.0"
EXPECTED_WRITABLE_ROOTS = ["app", "components", "lib", "styles", "public"]
VITE_EXPECTED_WRITABLE_ROOTS = EXPECTED_WRITABLE_ROOTS + ["src"]
FORBIDDEN_ROUTE_FILES = {"route.js", "route.jsx", "route.ts", "route.tsx"}

# Package-manager policy for the *rootfs* APK closure. npm remains available
# for terminal users, while the host-owned local-app installer uses the pinned
# pnpm tarball. Only managers that are not part of the supported toolchain stay
# out of the rootfs closure.
FORBIDDEN_PACKAGE_NAMES = {"corepack", "yarn"}
FORBIDDEN_EXECUTABLES = {
    "/usr/bin/corepack",
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

RUNTIME_PROFILE_COMMON_DEPENDENCIES = {
    "@ionic/react": "9.0.0",
    "@ionic/react-router": "9.0.0",
    "@vitejs/plugin-react": "6.0.4",
    "react": "19.2.8",
    "react-dom": "19.2.8",
    "react-router": "6.30.6",
    "react-router-dom": "6.30.6",
    "vite": "8.2.1",
    "zod": "4.4.3",
    "zustand": "5.0.15",
}
RUNTIME_PROFILE_LOCK_PACKAGES = {
    "rolldown": "1.2.6",
    "@rolldown/binding-linux-arm64-musl": "1.2.6",
    "@rolldown/binding-linux-x64-musl": "1.2.6",
    "@rollup/rollup-linux-arm64-musl": "4.44.0",
    "@rollup/rollup-linux-x64-musl": "4.44.0",
    "lightningcss": "1.33.0",
    "lightningcss-linux-arm64-musl": "1.33.0",
    "lightningcss-linux-x64-musl": "1.33.0",
}
RUNTIME_PROFILE_LOCK_SHA256 = {
    "react-dom": "ff805143f51e7a9cc31a935495b64f3515a54a6ebb8d4b7374f1d5e33869aabb",
    "canvas-2d": "ff805143f51e7a9cc31a935495b64f3515a54a6ebb8d4b7374f1d5e33869aabb",
    "three-3d": "dee30efc799fdf0b859a21b9ba482e931ce117d83253f750f47974aeb623aed6",
    "phaser-2d": "4673a2fa573ed431b7e48d58fb143bfda8a9ef3e379d63ebd05395d5c4935b95",
    "babylon-3d": "a2b282f45cb5cfde7cae1fce39c06b0dd943a25ba037b911704727d959de632c",
}
RUNTIME_PROFILES = {
    "react-dom": {
        "extra_dependencies": {},
        "host_managed_helpers": [],
    },
    "canvas-2d": {
        "extra_dependencies": {},
        "host_managed_helpers": ["lib/frame-loop.js"],
    },
    "three-3d": {
        "extra_dependencies": {"three": "0.185.1"},
        "host_managed_helpers": ["lib/frame-loop.js"],
    },
    "phaser-2d": {
        "extra_dependencies": {"phaser": "4.2.1"},
        "host_managed_helpers": ["lib/frame-loop.js", "lib/phaser-runtime.js"],
    },
    "babylon-3d": {
        "extra_dependencies": {
            "@babylonjs/core": "9.22.1",
            "@babylonjs/havok": "1.3.14",
            "@babylonjs/loaders": "9.22.1",
        },
        "host_managed_helpers": ["lib/frame-loop.js", "lib/babylon-runtime.js"],
    },
}
RUNTIME_PROFILE_HOST_MANAGED_BASE = [
    ".lingxi",
    ".gitignore",
    "LINGXI.md",
    "index.html",
    "jsconfig.json",
    "vite.config.mjs",
    "package.json",
    "pnpm-lock.yaml",
    "pnpm-workspace.yaml",
    "lib/device-context.js",
    "lib/lingxi-bridge.js",
    "lib/platform-adapter.js",
    "lib/lingxi-provider.jsx",
]
RUNTIME_PROFILE_HOST_MANAGED_SUFFIX = [
    "styles/foundation.css",
    "node_modules",
]


def expected_native_packages_for(platform: str, family: str) -> dict[str, str]:
    if family == "rolldown":
        bindings = EXPECTED_ROLLDOWN_BINDINGS
        arm64 = "@rolldown/binding-linux-arm64-musl"
    elif family == "rollup":
        bindings = EXPECTED_ROLLUP_BINDINGS
        arm64 = "@rollup/rollup-linux-arm64-musl"
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


def load_yaml_mapping(path: pathlib.Path) -> dict:
    """Parse the flat two-level settings mapping pnpm-workspace.yaml uses.

    PyYAML is not a dependency of this gate and adding one to a supply-chain
    verifier to read seven settings is a poor trade. The grammar accepted here
    is exactly what the template file uses: `key: value` at column 0, and
    `key:` followed by two-space-indented `name: value` pairs. Anything else
    fails loudly rather than being skipped, so a file that grows a construct
    this cannot represent cannot pass by being misread.
    """
    mapping: dict = {}
    current: dict | None = None
    for number, raw in enumerate(path.read_text(encoding="utf-8").splitlines(), start=1):
        if not raw.strip() or raw.lstrip().startswith("#"):
            continue
        if raw.startswith("  "):
            if current is None:
                fail(f"{path}:{number}: indented entry outside a mapping")
            nested = raw[2:]
            # A third level would be flattened into the second if it were
            # accepted here, which is exactly the silent misreading this parser
            # must not do: the caller would compare a mapping that never
            # existed in the file.
            if nested.startswith(" "):
                fail(f"{path}:{number}: unsupported nesting depth")
            key, separator, value = nested.partition(":")
            if not separator or not key.strip() or not value.strip():
                fail(f"{path}:{number}: unsupported nested syntax")
            current[key.strip()] = value.strip()
            continue
        # Any other leading whitespace -- a single space, or a tab, which YAML
        # forbids for indentation -- would otherwise be stripped and read as a
        # TOP-LEVEL key, turning a nested entry into a sibling of its parent.
        if raw[:1].isspace():
            fail(f"{path}:{number}: unsupported indentation")
        key, separator, value = raw.partition(":")
        if not separator:
            fail(f"{path}:{number}: unsupported syntax")
        if value.strip():
            mapping[key.strip()] = value.strip()
            current = None
        else:
            current = {}
            mapping[key.strip()] = current
    return mapping


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


def validate_sbom(repo: pathlib.Path, template: pathlib.Path) -> None:
    sbom = load_json(repo / "docs" / "mobile-linux" / "sbom" / "local-app-runtime.spdx.json")
    if sbom.get("spdxVersion") != "SPDX-2.3":
        fail("local-app runtime SBOM must use SPDX 2.3")
    packages = sbom.get("packages")
    if not isinstance(packages, list):
        fail("local-app runtime SBOM missing packages")
    if len(packages) != 1 or not isinstance(packages[0], dict):
        fail("local-app runtime SBOM must contain one pnpm lockfile package")
    lock_digest = hashlib.sha256((template / "pnpm-lock.yaml").read_bytes()).hexdigest()
    package = packages[0]
    if (
        package.get("name") != "lingxi-local-app-template"
        or package.get("versionInfo") != "pnpm-lock.yaml"
        or package.get("sourceInfo") != f"pnpm lockfile sha256: {lock_digest}"
        or {"algorithm": "SHA256", "checksumValue": lock_digest}
        not in package.get("checksums", [])
    ):
        fail("local-app runtime SBOM does not match pnpm-lock.yaml")


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
        "source": "embedded:runtime-profiles/react-dom/r1/pnpm-lock.yaml",
        "materialize_into": f"{build_root}/node_modules",
        "guest_mount": "forbidden",
        "selection_policy": "exact_lock_only",
        "install_command": "pnpm install --frozen-lockfile --ignore-scripts --no-runtime --prefer-offline",
    }:
        fail("local-app dependency snapshot policy diverged")
    build_mount = policy.get("build_mount")
    if build_mount != {
        "kind": "LocalAppBuild",
        "count": 1,
        "host_path_policy": "workspace_or_staging_or_store_root",
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

    # The bundled dependency seed only reaches the engine if the launch config
    # actually carries its path. `LocalAppsRuntimeDistribution.runtimeRoot` was
    # computed and then discarded -- BOTH call sites passed
    # `localAppsRuntimeRoot: nil`, so `configured_runtime_root()` reported the
    # seed as unconfigured and every device installed dependencies over the
    # network with a complete tree sitting in its own bundle. A Rust test
    # cannot see Swift, so assert the Swift spelling here.
    for relative in (
        "clients/ios/Sources/Conversation/ConversationSource.swift",
        "clients/ios/Sources/Cron/CronFFIBridge.swift",
    ):
        source_path = repo / relative
        try:
            source_text = source_path.read_text(encoding="utf-8")
        except OSError as exc:
            fail(f"missing iOS engine launch source: {exc}")
        if "localAppsRuntimeRoot: LocalAppsRuntimeDistribution.runtimeRoot" not in source_text:
            fail(f"{relative} must pass the bundled local-app runtime root to the engine")
        if "localAppsRuntimeRoot: nil" in source_text:
            fail(f"{relative} still discards the bundled local-app runtime root")


def validate_create_skill(repo: pathlib.Path) -> None:
    skill_path = repo / "skills" / "create-local-app" / "SKILL.md"
    try:
        text = skill_path.read_text(encoding="utf-8")
    except OSError as exc:
        fail(f"missing create-local-app skill: {exc}")
    if not text.startswith("---\nname: create-local-app\ndescription: "):
        fail("create-local-app skill frontmatter is invalid")
    required_tokens = {
        "LocalAppCreate",
        "LocalAppRuntimeProfiles",
        "LocalAppConfirmRuntimeProfile",
        "runtime_profile_receipt",
        "LocalAppManifest",
        "LocalAppBuild",
        "LocalAppRuntime",
        "LocalAppLogs",
        "LocalAppInspectUi",
        "LocalAppActOnUi",
        "LocalAppQueryData",
        "LocalAppMutateData",
        "LocalAppCheckpointRestore",
        "window.lingxi.v2",
        "Do not call `LocalAppList` or `LocalAppGet`",
        "call `AskUserQuestion`",
        "Never ask unresolved questions in ordinary assistant text",
        "Every collection requires `id`, `name`, and `fields`",
        "Never declare host-owned record metadata",
        "vite build --outDir dist --emptyOutDir",
        "build/store/dist/",
        "recommended strategy",
        "task-local workflow",
        "rescore",
        "revised confirmed specification",
        "For a `dom` surface",
        "`fast`, `balanced`, or `thorough`",
        "for a `canvas` surface, offer",
        "Never advertise or pass",
        "`fast` for a canvas surface",
        "expected_writable_collections",
        "host reads the materialized manifest and overwrites it",
        "Do not supply `args.runtime_profile` as an authority",
        # Runtime profile, not a model-authored surface/renderer, is the
        # immutable scaffold choice.
        '"runtime_profile_receipt":"<receipt id>"',
        "Profile family CANNOT be changed afterwards",
        "`canvas` when the whole interface is one drawn surface",
        "LocalAppConfirmDependencyChange",
        "LocalAppUpdateDependencies",
        # The display name is the model's to write. Without this the engine
        # falls back to the brief's first 24 characters, which is what the
        # deferred create flow exists to stop.
        "`name` is yours to write",
        "streamLlmChat",
        "onLlmStreamFrame",
        "getClipboardText",
        "setClipboardText",
        "shareContent",
        "synthesizeSpeech",
        "readFile",
        "writeFile",
        "getDeviceStatus",
        "triggerHaptics",
        "openDeepLink",
        "listCalendarEvents",
        "searchContacts",
        "getMedia",
        "background_schedule",
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
    # Read the workflows the way the BINARY assembles them: `builtins.rs`
    # concatenates a shape file with the shared driver, so a token that moved
    # into the driver is still present in the shipped script even though it left
    # the shape file. Checking the shape file alone would have reported the
    # shared strategy vocabulary as missing.
    workflow_dir = repo / "lingxi-code" / "tools" / "workflow" / "src"
    try:
        workflow_core = (workflow_dir / "local_app_workflow_core.js").read_text(encoding="utf-8")
        workflow_dom = (workflow_dir / "local_app_build_workflow.js").read_text(encoding="utf-8")
        workflow_canvas = (workflow_dir / "local_app_canvas_workflow.js").read_text(encoding="utf-8")
    except OSError as exc:
        fail(f"missing local-app build workflow: {exc}")
    workflow = workflow_dom + workflow_core
    canvas_workflow = workflow_canvas + workflow_core
    workflow_tokens = {
        "LocalAppBuild",
        "--outDir dist --emptyOutDir",
        "build/store/dist/",
        "strategy?: 'fast'|'balanced'|'thorough'",
        "'balanced'",
        "maxRepairRounds",
        "Selected workflow strategy",
        "Complexity score",
        "agent_calls",
        "verification_mode",
        "args.expected_writable_collections",
        "Host-materialized expected_writable_collections",
        "Return not_applicable only when the host-materialized expected_writable_collections is empty",
        "streamLlmChat",
        "getClipboardText",
        "setClipboardText",
        "shareContent",
        "synthesizeSpeech",
        "readFile",
        "writeFile",
        "getDeviceStatus",
        "triggerHaptics",
        "openDeepLink",
        "listCalendarEvents",
        "searchContacts",
        "getMedia",
        "background_schedule",
    }
    missing_workflow = sorted(token for token in workflow_tokens if token not in workflow)
    if missing_workflow:
        fail(f"local-app-build workflow is missing Vite CLI contract tokens: {missing_workflow}")
    # The drawn-surface sibling shares the driver, so it inherits the tokens
    # above; what it must additionally carry is the evidence contract that
    # replaces `data_roundtrip` for an app that declares no collection.
    canvas_tokens = {
        "app/screens/game-screen.jsx",
        "createFrameLoop in lib/frame-loop.js",
        "Keep per-frame simulation state in a ref",
        "There is no not_applicable: this app draws its whole interface",
        "only a difference proves the loop is running",
        "cannot be driven by the host automation path",
        "NEVER from @ionic/core/components",
        "locked `three@0.185.1` runtime",
    }
    missing_canvas = sorted(
        token for token in workflow_tokens | canvas_tokens if token not in canvas_workflow
    )
    if missing_canvas:
        fail(f"local-canvas-build workflow is missing contract tokens: {missing_canvas}")
    if "allowedStrategies: ['balanced', 'thorough']" not in canvas_workflow:
        fail("local-canvas-build must allow exactly balanced and thorough strategies")

    handoff_path = repo / "docs" / "local-apps" / "HANDOFF.md"
    try:
        handoff = handoff_path.read_text(encoding="utf-8")
    except OSError as exc:
        fail(f"missing local-app handoff: {exc}")
    # Keep the handoff tied to the scaffold that the host actually seeds. The
    # old Template v2 paragraph described an unavailable Tailwind/shadcn stack
    # and omitted the provider/bridge helpers that generated source must use.
    handoff_tokens = {
        "DOM workflow shape",
        "Canvas workflow shape",
        "Shared workflow core",
        "vite-react-static-v1",
        "vite-react-canvas-v1",
        "@ionic/react",
        "LingXiBridgeProvider",
        "IonReactHashRouter",
        "lib/lingxi-provider.jsx",
        "queryCollection",
        "requestLlmChat",
        "expected_writable_collections",
    }
    missing_handoff = sorted(token for token in handoff_tokens if token not in handoff)
    if missing_handoff:
        fail(f"local-app handoff is missing scaffold/workflow contract tokens: {missing_handoff}")
    if "Template v2 bundles the JSX Vite/Tailwind foundation" in handoff:
        fail("local-app handoff still describes the retired Template v2 scaffold")
    # The host-chrome contract, carried VERBATIM by both shapes: the run surface
    # has no title bar and no back button, and the host paints one control over
    # the bottom-leading corner. Without these two sentences a generated app can
    # ship with zero bars and no way out, and nothing downstream notices — the
    # build succeeds and the DOM snapshot of a bar-less page is perfectly valid.
    # Only the two invariant clauses are pinned here; the keep-clear dimensions
    # are derived from the clients' tap targets and are pinned once, in
    # `builtins.rs`, so a legitimate re-derivation touches one gate and not two.
    host_chrome_tokens = {
        "The host draws NO chrome around a running app",
        "The host floats ONE control over the running page in the BOTTOM-LEADING corner",
    }
    for label, script in (
        ("local-app-build", workflow),
        ("local-canvas-build", canvas_workflow),
    ):
        missing_chrome = sorted(token for token in host_chrome_tokens if token not in script)
        if missing_chrome:
            fail(f"{label} workflow is missing host-chrome contract tokens: {missing_chrome}")


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


def runtime_profile_template_root(repo: pathlib.Path) -> pathlib.Path:
    return repo / "lingxi-code" / "local-apps" / "templates" / "runtime-profiles"


def expected_runtime_profile_dependencies(profile_name: str) -> dict[str, str]:
    profile = RUNTIME_PROFILES.get(profile_name)
    if profile is None:
        fail(f"unknown runtime profile: {profile_name}")
    return dict(RUNTIME_PROFILE_COMMON_DEPENDENCIES | profile["extra_dependencies"])


def expected_runtime_profile_host_managed_paths(profile_name: str) -> list[str]:
    profile = RUNTIME_PROFILES.get(profile_name)
    if profile is None:
        fail(f"unknown runtime profile: {profile_name}")
    return [
        *RUNTIME_PROFILE_HOST_MANAGED_BASE,
        *profile["host_managed_helpers"],
        *RUNTIME_PROFILE_HOST_MANAGED_SUFFIX,
    ]


def lock_contains_package(lock_text: str, name: str, version: str) -> bool:
    return re.search(
        rf"^\s*['\"]?{re.escape(name)}@{re.escape(version)}['\"]?:\s*$",
        lock_text,
        re.MULTILINE,
    ) is not None


def validate_runtime_profile_lock(
    profile_name: str,
    template: pathlib.Path,
    pins: dict,
) -> None:
    package_json = load_json(template / "package.json")
    lock_path = template / "pnpm-lock.yaml"
    lock_text = lock_path.read_text(encoding="utf-8")
    expected_dependencies = expected_runtime_profile_dependencies(profile_name)
    if package_json.get("engines") != {"node": expected_node(pins)}:
        fail(f"{profile_name} package.json must pin Node exactly")
    if package_json.get("dependencies") != expected_dependencies:
        fail(f"{profile_name} package.json dependencies diverged from the runtime-profile contract")
    if package_json.get("scripts") != EXPECTED_SCRIPTS:
        fail(f"{profile_name} package.json must expose the standard Vite scripts only")
    if "overrides" in package_json:
        fail(f"{profile_name} package.json must not carry pnpm overrides")
    if hashlib.sha256(lock_path.read_bytes()).hexdigest() != RUNTIME_PROFILE_LOCK_SHA256[profile_name]:
        fail(f"{profile_name} pnpm-lock.yaml bytes diverged from the reviewed runtime-profile lock")

    workspace_settings = load_yaml_mapping(template / "pnpm-workspace.yaml")
    expected_workspace_settings = {
        "lockfile": "pnpm-lock.yaml",
        "nodeLinker": "hoisted",
        "packageImportMethod": "clone-or-copy",
        "verifyStoreIntegrity": "true",
        "strictStorePkgContentCheck": "true",
        "ignoreScripts": "true",
        "preferFrozenLockfile": "true",
        "overrides": {"lightningcss": "1.33.0"},
    }
    if workspace_settings != expected_workspace_settings:
        fail(f"{profile_name} pnpm-workspace.yaml diverged from the fixed runtime-profile settings")
    if not re.search(r"^lockfileVersion:\s*['\"]?9\.0['\"]?\s*$", lock_text, re.MULTILINE):
        fail(f"{profile_name} pnpm-lock.yaml must use lockfileVersion 9")
    if "importers:" not in lock_text or "packages:" not in lock_text or "snapshots:" not in lock_text:
        fail(f"{profile_name} pnpm-lock.yaml is missing importers/packages/snapshots")
    for name, version in expected_dependencies.items():
        pattern = rf"(?ms)^\s+['\"]?{re.escape(name)}['\"]?:\s*\n\s+specifier:\s*{re.escape(version)}\b"
        if not re.search(pattern, lock_text):
            fail(f"{profile_name} pnpm-lock importer did not pin {name}@{version}")
    for name, version in RUNTIME_PROFILE_LOCK_PACKAGES.items():
        if not lock_contains_package(lock_text, name, version):
            fail(f"{profile_name} pnpm-lock did not pin {name}@{version}")


def validate_base_seed_profile_relationships(repo: pathlib.Path, pins: dict) -> None:
    base_template = repo / pins["local_app_runtime"]["template"]
    if base_template != runtime_profile_template_root(repo) / "react-dom" / "r1":
        fail("bundled local-app dependency seed must point to runtime-profiles/react-dom/r1")
    base_lock_sha = hashlib.sha256((base_template / "pnpm-lock.yaml").read_bytes()).hexdigest()
    if pins["local_app_runtime"]["lockfile_sha256"] != base_lock_sha:
        fail("bundled local-app dependency seed lock SHA diverged from the base runtime profile")

    canvas_lock_sha = hashlib.sha256(
        (runtime_profile_template_root(repo) / "canvas-2d" / "r1" / "pnpm-lock.yaml").read_bytes()
    ).hexdigest()
    if canvas_lock_sha != base_lock_sha:
        fail("react_dom and canvas_2d must share the engine-free bundled seed lock")
    for profile_name in ("three-3d", "phaser-2d", "babylon-3d"):
        profile_lock_sha = hashlib.sha256(
            (runtime_profile_template_root(repo) / profile_name / "r1" / "pnpm-lock.yaml").read_bytes()
        ).hexdigest()
        if profile_lock_sha == base_lock_sha:
            fail(f"{profile_name} must not share the engine-free bundled seed lock")

    package_json = load_json(base_template / "package.json")
    dependencies = package_json.get("dependencies", {})
    forbidden_engine_packages = {"three", "phaser", "@babylonjs/core", "@babylonjs/loaders", "@babylonjs/havok"}
    present = sorted(package for package in forbidden_engine_packages if package in dependencies)
    if present:
        fail(f"bundled local-app dependency seed must remain engine-free, found {present}")
    lock_text = (base_template / "pnpm-lock.yaml").read_text(encoding="utf-8")
    forbidden_lock_entries = []
    for package in forbidden_engine_packages:
        if re.search(rf"^\s*['\"]?{re.escape(package)}@", lock_text, re.MULTILINE):
            forbidden_lock_entries.append(package)
    if forbidden_lock_entries:
        fail(
            "bundled local-app dependency seed lock must remain engine-free, found "
            f"{sorted(forbidden_lock_entries)}"
        )
    expected_runtime = {
        "template": "lingxi-code/local-apps/templates/runtime-profiles/react-dom/r1",
        "node": expected_node(pins),
        "react": EXPECTED_DEPENDENCIES["react"],
        "react_dom": EXPECTED_DEPENDENCIES["react-dom"],
        "vite": EXPECTED_DEPENDENCIES["vite"],
        "rolldown": EXPECTED_ROLLDOWN_VERSION,
        "rolldown_bindings": EXPECTED_ROLLDOWN_BINDINGS,
        "rollup": "4.44.0",
        "rollup_bindings": EXPECTED_ROLLUP_BINDINGS,
        "lightningcss": EXPECTED_LIGHTNINGCSS_VERSION,
        "lightningcss_bindings": EXPECTED_LIGHTNINGCSS_BINDINGS,
        "lockfile": "lingxi-code/local-apps/templates/runtime-profiles/react-dom/r1/pnpm-lock.yaml",
        "lockfile_sha256": base_lock_sha,
    }
    if pins.get("local_app_runtime") != expected_runtime:
        fail("bundled local-app dependency seed pins diverged from the engine-free base profile")


def scan_runtime_profile_sources(template: pathlib.Path) -> None:
    writable_roots = set(VITE_EXPECTED_WRITABLE_ROOTS)
    allowed_top_level = {
        *writable_roots,
        ".gitignore",
        ".lingxi",
        "index.html",
        "jsconfig.json",
        "vite.config.mjs",
        "package.json",
        "pnpm-lock.yaml",
        "pnpm-workspace.yaml",
        "node_modules",
        "dist",
    }
    for path in template.rglob("*"):
        relative = path.relative_to(template)
        if relative.parts[0] in {"node_modules", "dist"}:
            continue
        if path.is_symlink():
            fail(f"symbolic links are forbidden in runtime profile sources: {relative}")
        if relative.parts[0] not in allowed_top_level:
            fail(f"runtime profile path is outside the fixed workspace roots: {relative}")
        if not path.is_file() or path.suffix not in SOURCE_SUFFIXES:
            continue
        if relative.name in FORBIDDEN_ROUTE_FILES:
            fail(f"API routes are forbidden: {relative}")
        text = path.read_text(encoding="utf-8")
        for label, pattern in FORBIDDEN_SOURCE_PATTERNS.items():
            if pattern.search(text):
                fail(f"forbidden {label} in {relative}")


def validate_runtime_profile_source_policy(profile_name: str, template: pathlib.Path) -> None:
    source_policy = load_json(template / ".lingxi" / "source-policy.json")
    if source_policy.get("schema_version") != 1:
        fail(f"{profile_name} source policy schema_version must remain 1")
    if source_policy.get("agent_writable_roots") != VITE_EXPECTED_WRITABLE_ROOTS:
        fail(f"{profile_name} source policy writable roots diverged")
    expected_host_managed = expected_runtime_profile_host_managed_paths(profile_name)
    if source_policy.get("host_managed_paths") != expected_host_managed:
        fail(f"{profile_name} source policy host-managed paths diverged")
    if set(source_policy.get("forbidden_features", [])) != {
        "arbitrary_javascript_bridge_actions",
        "direct_network_calls",
        "eval",
        "external_scripts",
        "package_install",
        "server_actions",
        "symbolic_links",
    }:
        fail(f"{profile_name} source policy forbidden features diverged")
    for helper in expected_host_managed:
        if "/" in helper and helper not in {"node_modules"} and not (template / helper).is_file():
            fail(f"{profile_name} source policy references a missing managed helper: {helper}")
    lingxi_dir = template / ".lingxi"
    lingering = sorted(
        path.relative_to(lingxi_dir).as_posix()
        for path in lingxi_dir.rglob("*")
        if path.is_file() or path.is_symlink()
    )
    if lingering != ["source-policy.json"]:
        fail(f"{profile_name} runtime profile must keep only .lingxi/source-policy.json, found {lingering}")
    config = (template / "vite.config.mjs").read_text(encoding="utf-8")
    compressed_size_values = re.findall(
        r"^[\t ]*reportCompressedSize\s*:\s*(true|false)\s*,?[\t ]*(?://.*)?$",
        config,
        flags=re.MULTILINE,
    )
    if compressed_size_values != ["false"]:
        fail(f"{profile_name} fixed Vite build must disable compressed-size reporting")
    out_dir_values = re.findall(
        r'^[\t ]*outDir\s*:\s*["\']([^"\']+)["\']\s*,?[\t ]*(?://.*)?$',
        config,
        flags=re.MULTILINE,
    )
    if out_dir_values != ["dist"]:
        fail(f"{profile_name} fixed Vite build must use the official dist output directory")
    scan_runtime_profile_sources(template)


def validate_runtime_profile_sbom(
    repo: pathlib.Path,
    profile_name: str,
    template: pathlib.Path,
) -> None:
    generator = repo / "lingxi-code" / "scripts" / "mobile-linux" / "generate-local-app-sbom.py"
    with tempfile.TemporaryDirectory(prefix=f"local-app-sbom-{profile_name}-") as temp_root:
        output = pathlib.Path(temp_root) / "runtime.spdx.json"
        subprocess.run(
            [sys.executable, str(generator), "--lock", str(template / "pnpm-lock.yaml"), "--output", str(output)],
            check=True,
        )
        sbom = load_json(output)
        if sbom.get("spdxVersion") != "SPDX-2.3":
            fail(f"{profile_name} generated SBOM must remain SPDX-2.3")
        packages = sbom.get("packages")
        if not isinstance(packages, list) or len(packages) != 1:
            fail(f"{profile_name} generated SBOM must describe exactly one lockfile package")
        lock_digest = hashlib.sha256((template / "pnpm-lock.yaml").read_bytes()).hexdigest()
        package = packages[0]
        if package.get("checksums") != [{"algorithm": "SHA256", "checksumValue": lock_digest}]:
            fail(f"{profile_name} generated SBOM checksum must match pnpm-lock.yaml")
        if not str(sbom.get("documentNamespace", "")).endswith(lock_digest):
            fail(f"{profile_name} generated SBOM namespace must end with the lock digest")


def validate_runtime_profiles(
    repo: pathlib.Path,
    pins: dict,
    selected_profile: str | None,
) -> None:
    root = runtime_profile_template_root(repo)
    profile_names = [selected_profile] if selected_profile else sorted(RUNTIME_PROFILES)
    for profile_name in profile_names:
        if profile_name not in RUNTIME_PROFILES:
            fail(f"unknown runtime profile: {profile_name}")
        template = root / profile_name / "r1"
        if not template.is_dir():
            fail(f"runtime profile template is missing: {template}")
        validate_runtime_profile_lock(profile_name, template, pins)
        validate_runtime_profile_source_policy(profile_name, template)
        validate_runtime_profile_sbom(repo, profile_name, template)


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--repo-root", required=True)
    parser.add_argument("--release", action="store_true")
    parser.add_argument("--apk-dir")
    parser.add_argument("--profile", choices=sorted(RUNTIME_PROFILES), help="validate one runtime profile only")
    parser.add_argument("--template", help="override a single runtime profile r1 directory")
    args = parser.parse_args()

    repo = pathlib.Path(args.repo_root).resolve()
    pins = load_json(repo / "docs" / "mobile-linux" / "local-app-runtime-pins.json")
    validate_apk_pins(
        pins,
        release=args.release,
        apk_dir=pathlib.Path(args.apk_dir).resolve() if args.apk_dir else None,
    )
    if args.template:
        if not args.profile:
            fail("--template requires --profile so the expected runtime profile contract is known")
        template = pathlib.Path(args.template).resolve()
        validate_runtime_profile_lock(args.profile, template, pins)
        validate_runtime_profile_source_policy(args.profile, template)
        validate_runtime_profile_sbom(repo, args.profile, template)
    else:
        template = repo / pins.get("local_app_runtime", {}).get("template", "")
        validate_runtime_profile_lock("react-dom", template, pins)
        validate_runtime_profile_source_policy("react-dom", template)
        validate_base_seed_profile_relationships(repo, pins)
        validate_sbom(repo, template)
        validate_runtime_profiles(repo, pins, args.profile)
    validate_runtime_policy(repo)
    validate_create_skill(repo)
    validate_product_model_name_absence(repo)
    print("local-app runtime profile supply-chain pins verified")


if __name__ == "__main__":
    main()
