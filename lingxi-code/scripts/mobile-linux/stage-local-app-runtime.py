#!/usr/bin/env python3
import argparse
import hashlib
import importlib.util
import json
import os
import pathlib
import shutil
import stat
import tempfile

_VERIFY_SOURCE = pathlib.Path(__file__).with_name("verify-local-app-supply-chain.py")
_VERIFY_SPEC = importlib.util.spec_from_file_location("local_app_supply_chain", _VERIFY_SOURCE)
if _VERIFY_SPEC is None or _VERIFY_SPEC.loader is None:
    raise ImportError(f"cannot load {_VERIFY_SOURCE}")
_VERIFY = importlib.util.module_from_spec(_VERIFY_SPEC)
_VERIFY_SPEC.loader.exec_module(_VERIFY)

EXPECTED_DEPENDENCIES = _VERIFY.EXPECTED_DEPENDENCIES
EXPECTED_LIGHTNINGCSS_VERSION = _VERIFY.EXPECTED_LIGHTNINGCSS_VERSION
EXPECTED_OXIDE_VERSION = _VERIFY.EXPECTED_OXIDE_VERSION
EXPECTED_ROLLDOWN_VERSION = _VERIFY.EXPECTED_ROLLDOWN_VERSION
expected_native_binary_for = _VERIFY.expected_native_binary_for
expected_native_packages_for = _VERIFY.expected_native_packages_for
fail = _VERIFY.fail
load_json = _VERIFY.load_json
validate_apk_pins = _VERIFY.validate_apk_pins
validate_lock = _VERIFY.validate_lock
validate_runtime_policy = _VERIFY.validate_runtime_policy
validate_sbom = _VERIFY.validate_sbom
validate_source_policy = _VERIFY.validate_source_policy


FORBIDDEN_TOP_LEVEL_PACKAGES = {"corepack", "nodejs-npm", "npm", "pnpm", "yarn"}


def sha256(path: pathlib.Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        while chunk := handle.read(1024 * 1024):
            digest.update(chunk)
    return digest.hexdigest()


def validate_symlinks(root: pathlib.Path) -> None:
    for path in root.rglob("*"):
        if not path.is_symlink():
            continue
        try:
            path.resolve(strict=True).relative_to(root.resolve())
        except (OSError, ValueError) as exc:
            fail(f"node_modules symlink escapes or is broken: {path}: {exc}")

def validate_node_modules(
    root: pathlib.Path,
    platform: str,
) -> None:
    if not root.is_dir() or root.is_symlink():
        fail(f"node_modules input is missing or unsafe: {root}")
    validate_symlinks(root)
    present = {path.name for path in root.iterdir() if path.is_dir()}
    forbidden = present & FORBIDDEN_TOP_LEVEL_PACKAGES
    if forbidden:
        fail(f"forbidden package managers in node_modules: {sorted(forbidden)}")
    for name, version in EXPECTED_DEPENDENCIES.items():
        package = load_json(root / name / "package.json")
        if package.get("version") != version:
            fail(f"runtime node_modules did not resolve {name}@{version}")
    if (root / "next").exists():
        fail("runtime node_modules must not retain the Next.js package")
    if (root / "@next").exists():
        fail("runtime node_modules must not retain @next SWC packages")
    vite_binary = root / "vite" / "bin" / "vite.js"
    if not vite_binary.is_file() or vite_binary.is_symlink():
        fail("runtime node_modules is missing the fixed Vite CLI")
    rolldown = load_json(root / "rolldown" / "package.json")
    if rolldown.get("version") != EXPECTED_ROLLDOWN_VERSION:
        fail(f"runtime node_modules did not resolve rolldown@{EXPECTED_ROLLDOWN_VERSION}")
    lightningcss = load_json(root / "lightningcss" / "package.json")
    if lightningcss.get("version") != EXPECTED_LIGHTNINGCSS_VERSION:
        fail(f"runtime node_modules did not resolve lightningcss@{EXPECTED_LIGHTNINGCSS_VERSION}")
    oxide = load_json(root / "@tailwindcss" / "oxide" / "package.json")
    if oxide.get("version") != EXPECTED_OXIDE_VERSION:
        fail(f"runtime node_modules did not resolve @tailwindcss/oxide@{EXPECTED_OXIDE_VERSION}")
    allowed_rolldown_bindings = expected_native_packages_for(platform, "rolldown")
    allowed_rolldown_dirs = {name.removeprefix("@rolldown/") for name in allowed_rolldown_bindings}
    rolldown_roots = (
        [path for path in (root / "@rolldown").iterdir()]
        if (root / "@rolldown").is_dir()
        else []
    )
    for path in rolldown_roots:
        if path.name.startswith("binding-") and path.name not in allowed_rolldown_dirs:
            fail(f"runtime node_modules resolved an unexpected Rolldown binding: @rolldown/{path.name}")
    for name, version in allowed_rolldown_bindings.items():
        package_root = root / pathlib.PurePosixPath(name)
        package = load_json(package_root / "package.json")
        if package.get("version") != version:
            fail(f"runtime node_modules did not resolve {name}@{version}")
        binary = package_root / expected_native_binary_for(name)
        native_bindings = list(package_root.glob("*.node"))
        if (
            len(native_bindings) != 1
            or native_bindings[0].is_symlink()
            or native_bindings[0].name != binary.name
            or not binary.is_file()
        ):
            fail(f"runtime node_modules must contain one real native binding for {name}")
    allowed_lightningcss_bindings = expected_native_packages_for(platform, "lightningcss")
    lightningcss_roots = [path for path in root.iterdir() if path.is_dir()]
    for path in lightningcss_roots:
        if (
            path.name.startswith("lightningcss-")
            and path.name not in allowed_lightningcss_bindings
        ):
            fail(f"runtime node_modules resolved an unexpected Lightning CSS binding: {path.name}")
    for name, version in allowed_lightningcss_bindings.items():
        package_root = root / name
        package = load_json(package_root / "package.json")
        if package.get("version") != version:
            fail(f"runtime node_modules did not resolve {name}@{version}")
        binary = package_root / expected_native_binary_for(name)
        native_bindings = list(package_root.glob("*.node"))
        if (
            len(native_bindings) != 1
            or native_bindings[0].is_symlink()
            or native_bindings[0].name != binary.name
            or not binary.is_file()
        ):
            fail(f"runtime node_modules must contain one real native binding for {name}")
    allowed_oxide_bindings = expected_native_packages_for(platform, "oxide")
    oxide_roots = (
        [path for path in (root / "@tailwindcss").iterdir()]
        if (root / "@tailwindcss").is_dir()
        else []
    )
    allowed_oxide_dirs = {name.removeprefix("@tailwindcss/") for name in allowed_oxide_bindings}
    for path in oxide_roots:
        if path.name.startswith("oxide-") and path.name not in allowed_oxide_dirs:
            fail(f"runtime node_modules resolved an unexpected Tailwind Oxide binding: @tailwindcss/{path.name}")
    for name, version in allowed_oxide_bindings.items():
        package_root = root / pathlib.PurePosixPath(name)
        package = load_json(package_root / "package.json")
        if package.get("version") != version:
            fail(f"runtime node_modules did not resolve {name}@{version}")
        binary = package_root / expected_native_binary_for(name)
        native_bindings = list(package_root.glob("*.node"))
        if (
            len(native_bindings) != 1
            or native_bindings[0].is_symlink()
            or native_bindings[0].name != binary.name
            or not binary.is_file()
        ):
            fail(f"runtime node_modules must contain one real native binding for {name}")
    bin_dir = root / ".bin"
    for name in ("corepack", "npm", "npx", "pnpm", "yarn"):
        path = bin_dir / name
        if path.exists() or path.is_symlink():
            fail(f"forbidden package-manager executable in node_modules: {path}")


def make_read_only(root: pathlib.Path) -> None:
    for current_root, directories, files in os.walk(root, topdown=False, followlinks=False):
        current = pathlib.Path(current_root)
        for filename in files:
            path = current / filename
            if path.is_symlink():
                continue
            mode = stat.S_IMODE(path.stat().st_mode)
            path.chmod(0o555 if mode & 0o111 else 0o444)
        for dirname in directories:
            path = current / dirname
            if not path.is_symlink():
                path.chmod(0o555)
        current.chmod(0o555)
    # Keep the staging root writable by its owner so it can be atomically
    # renamed or replaced. Bundled descendants remain immutable host inputs;
    # builds copy them into a disposable project rather than mounting them.
    root.chmod(0o755)


def inventory(root: pathlib.Path) -> list[dict]:
    entries = []
    for path in sorted(root.rglob("*")):
        relative = path.relative_to(root).as_posix()
        if path.is_symlink():
            target = os.readlink(path)
            entries.append(
                {
                    "path": relative,
                    "kind": "symlink",
                    "sha256": hashlib.sha256(target.encode("utf-8")).hexdigest(),
                    "size_bytes": len(target.encode("utf-8")),
                }
            )
        elif path.is_file():
            entries.append(
                {
                    "path": relative,
                    "kind": "file",
                    "sha256": sha256(path),
                    "size_bytes": path.stat().st_size,
                }
            )
    return entries


def remove_tree(path: pathlib.Path) -> None:
    def make_writable_and_retry(function, value, _error) -> None:
        os.chmod(pathlib.Path(value).parent, 0o700)
        os.chmod(value, 0o700)
        function(value)

    shutil.rmtree(path, onerror=make_writable_and_retry)


def assert_safe_output(repo: pathlib.Path, output: pathlib.Path) -> None:
    allowed_roots = [
        repo / "clients" / "android" / "app" / "build",
        repo / "clients" / "ios" / "build",
    ]
    resolved = output.resolve()
    if not any(resolved.is_relative_to(root.resolve()) for root in allowed_roots):
        fail(f"runtime staging output must remain under a client build directory: {output}")


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--repo-root", required=True)
    parser.add_argument("--node-modules", required=True)
    parser.add_argument("--output", required=True)
    parser.add_argument("--platform", choices=("android", "ios"), required=True)
    parser.add_argument("--variant", required=True)
    args = parser.parse_args()

    repo = pathlib.Path(args.repo_root).resolve()
    node_modules = pathlib.Path(args.node_modules).resolve()
    output = pathlib.Path(args.output)
    assert_safe_output(repo, output)

    pins = load_json(repo / "docs" / "mobile-linux" / "local-app-runtime-pins.json")
    template = repo / pins["local_app_runtime"]["template"]
    validate_apk_pins(pins, release=False, apk_dir=None)
    validate_lock(template, pins)
    validate_source_policy(template, pins)
    validate_sbom(repo, template)
    validate_runtime_policy(repo)
    allowed_rolldown_bindings = expected_native_packages_for(args.platform, "rolldown")
    allowed_lightningcss_bindings = expected_native_packages_for(args.platform, "lightningcss")
    allowed_oxide_bindings = expected_native_packages_for(args.platform, "oxide")
    validate_node_modules(node_modules, args.platform)

    output.parent.mkdir(parents=True, exist_ok=True)
    temporary = pathlib.Path(tempfile.mkdtemp(prefix=f".{output.name}.", dir=output.parent))
    try:
        # Android assets do not preserve Unix symlinks when AAPT packages and
        # AssetManager extracts them. Dereference only after validating every
        # source link stays inside node_modules; iOS bundles can preserve the
        # original, already-validated link topology.
        preserve_symlinks = args.platform == "ios"
        shutil.copytree(
            node_modules,
            temporary / "node_modules",
            symlinks=preserve_symlinks,
        )
        shutil.copytree(
            template,
            temporary / "template",
            symlinks=preserve_symlinks,
        )
        shutil.copy2(
            repo / "docs" / "mobile-linux" / "local-app-runtime-policy.json",
            temporary / "runtime-policy.json",
        )
        shutil.copy2(
            repo / "docs" / "mobile-linux" / "local-app-runtime-pins.json",
            temporary / "runtime-pins.json",
        )
        shutil.copy2(
            repo / "docs" / "mobile-linux" / "sbom" / "local-app-runtime.spdx.json",
            temporary / "runtime.spdx.json",
        )
        manifest = {
            "schema_version": 1,
            "platform": args.platform,
            "variant": args.variant,
            "read_only": True,
            "pnpm_lock_sha256": sha256(template / "pnpm-lock.yaml"),
            "resolved_rolldown_bindings": sorted(allowed_rolldown_bindings),
            "resolved_lightningcss_bindings": sorted(allowed_lightningcss_bindings),
            "resolved_oxide_bindings": sorted(allowed_oxide_bindings),
            "files": inventory(temporary),
        }
        (temporary / "runtime-manifest.json").write_text(
            json.dumps(manifest, indent=2, sort_keys=True) + "\n",
            encoding="utf-8",
        )
        make_read_only(temporary)
        if output.exists():
            remove_tree(output)
        os.replace(temporary, output)
    finally:
        if temporary.exists():
            remove_tree(temporary)

    print(f"staged verified read-only local-app runtime seed: {output}")


if __name__ == "__main__":
    main()
