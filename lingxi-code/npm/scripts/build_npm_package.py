#!/usr/bin/env python3
"""Stage and optionally package the lingxi npm module.

Adapted from codex/codex-cli/scripts/build_npm_package.py — structure is
preserved; codex/openai-specific bits replaced with LingXi equivalents.
"""

import argparse
import json
import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

SCRIPT_DIR = Path(__file__).resolve().parent
# npm/ directory
LINGXI_NPM_ROOT = SCRIPT_DIR.parent
# lingxi-code/ directory
REPO_ROOT = LINGXI_NPM_ROOT.parent

LINGXI_NPM_NAME = "lingxi"

# Each entry describes one platform-specific optional-dependency package.
# Fields:
#   npm_name      — the package name published to npm
#   npm_tag       — "<os>-<arch>" tag (also appended to platform package version)
#   target_triple — Rust/cargo target triple; also used as the vendor sub-dir name
#   os            — value for package.json "os" field
#   cpu           — value for package.json "cpu" field
LINGXI_PLATFORM_PACKAGES: dict[str, dict[str, str]] = {
    "lingxi-linux-x64": {
        "npm_name": "lingxi-linux-x64",
        "npm_tag": "linux-x64",
        "target_triple": "x86_64-unknown-linux-musl",
        "os": "linux",
        "cpu": "x64",
    },
    "lingxi-linux-arm64": {
        "npm_name": "lingxi-linux-arm64",
        "npm_tag": "linux-arm64",
        "target_triple": "aarch64-unknown-linux-musl",
        "os": "linux",
        "cpu": "arm64",
    },
    "lingxi-darwin-x64": {
        "npm_name": "lingxi-darwin-x64",
        "npm_tag": "darwin-x64",
        "target_triple": "x86_64-apple-darwin",
        "os": "darwin",
        "cpu": "x64",
    },
    "lingxi-darwin-arm64": {
        "npm_name": "lingxi-darwin-arm64",
        "npm_tag": "darwin-arm64",
        "target_triple": "aarch64-apple-darwin",
        "os": "darwin",
        "cpu": "arm64",
    },
    "lingxi-win32-x64": {
        "npm_name": "lingxi-win32-x64",
        "npm_tag": "win32-x64",
        "target_triple": "x86_64-pc-windows-msvc",
        "os": "win32",
        "cpu": "x64",
    },
    "lingxi-win32-arm64": {
        "npm_name": "lingxi-win32-arm64",
        "npm_tag": "win32-arm64",
        "target_triple": "aarch64-pc-windows-msvc",
        "os": "win32",
        "cpu": "arm64",
    },
}

# Maps a package name to the list of packages it expands into when staged
# together (unused in practice — kept for structural parity with original).
PACKAGE_EXPANSIONS: dict[str, list[str]] = {
    "lingxi": ["lingxi", *LINGXI_PLATFORM_PACKAGES],
}

# Maps a package name to whether it carries a native binary (True) or not.
# Main "lingxi" package has no native component; platform packages do.
PACKAGE_HAS_NATIVE: dict[str, bool] = {
    "lingxi": False,
    **{name: True for name in LINGXI_PLATFORM_PACKAGES},
}

# Maps a platform package name to the target triple it corresponds to.
PACKAGE_TARGET_FILTERS: dict[str, str] = {
    package_name: package_config["target_triple"]
    for package_name, package_config in LINGXI_PLATFORM_PACKAGES.items()
}

PACKAGE_CHOICES = tuple(PACKAGE_HAS_NATIVE)


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="Build or stage the LingXi CLI npm package.")
    parser.add_argument(
        "--package",
        choices=PACKAGE_CHOICES,
        default="lingxi",
        help="Which npm package to stage (default: lingxi).",
    )
    parser.add_argument(
        "--version",
        help="Version number to write to package.json inside the staged package.",
    )
    parser.add_argument(
        "--release-version",
        help="Version to stage for npm release.",
    )
    parser.add_argument(
        "--staging-dir",
        type=Path,
        help=(
            "Directory to stage the package contents. Defaults to a new temporary directory "
            "if omitted. The directory must be empty when provided."
        ),
    )
    parser.add_argument(
        "--tmp",
        dest="staging_dir",
        type=Path,
        help=argparse.SUPPRESS,
    )
    parser.add_argument(
        "--pack-output",
        type=Path,
        help="Path where the generated npm tarball should be written.",
    )
    parser.add_argument(
        "--vendor-src",
        type=Path,
        help=(
            "Directory whose sub-directories are Rust target triples "
            "(e.g. <vendor-src>/aarch64-apple-darwin/release/lingxi-cli). "
            "Required when staging a platform package."
        ),
    )
    return parser.parse_args()


def main() -> int:
    args = parse_args()

    package = args.package
    version = args.version
    release_version = args.release_version
    if release_version:
        if version and version != release_version:
            raise RuntimeError("--version and --release-version must match when both are provided.")
        version = release_version

    if not version:
        raise RuntimeError("Must specify --version or --release-version.")

    staging_dir, created_temp = prepare_staging_dir(args.staging_dir)

    try:
        stage_sources(staging_dir, version, package)

        vendor_src = args.vendor_src.resolve() if args.vendor_src else None
        has_native = PACKAGE_HAS_NATIVE.get(package, False)
        target_triple = PACKAGE_TARGET_FILTERS.get(package)

        if has_native:
            if vendor_src is None:
                raise RuntimeError(
                    f"Package '{package}' requires a native binary. "
                    "Provide --vendor-src pointing to the directory whose "
                    "sub-directories are Rust target triples."
                )
            copy_native_binary(
                vendor_src=vendor_src,
                staging_dir=staging_dir,
                target_triple=target_triple,
            )

        if release_version:
            staging_dir_str = str(staging_dir)
            if package == "lingxi":
                print(
                    f"Staged version {version} for release in {staging_dir_str}\n\n"
                    "Verify the CLI:\n"
                    f"    node {staging_dir_str}/bin/lingxi.js --version\n"
                    f"    node {staging_dir_str}/bin/lingxi.js --help\n\n"
                )
            elif package in LINGXI_PLATFORM_PACKAGES:
                print(
                    f"Staged version {version} for release in {staging_dir_str}\n\n"
                    "Verify native payload contents:\n"
                    f"    ls {staging_dir_str}/vendor\n\n"
                )
        else:
            print(f"Staged package in {staging_dir}")

        if args.pack_output is not None:
            output_path = run_npm_pack(staging_dir, args.pack_output)
            print(f"npm pack output written to {output_path}")
    finally:
        if created_temp:
            # Preserve the staging directory for further inspection.
            pass

    return 0


def prepare_staging_dir(staging_dir: Path | None) -> tuple[Path, bool]:
    if staging_dir is not None:
        staging_dir = staging_dir.resolve()
        staging_dir.mkdir(parents=True, exist_ok=True)
        if any(staging_dir.iterdir()):
            raise RuntimeError(f"Staging directory {staging_dir} is not empty.")
        return staging_dir, False

    temp_dir = Path(tempfile.mkdtemp(prefix="lingxi-npm-stage-"))
    return temp_dir, True


def stage_sources(staging_dir: Path, version: str, package: str) -> None:
    """Populate *staging_dir* with the source files for *package* at *version*."""
    package_json: dict
    package_json_path: Path | None = None

    if package == "lingxi":
        # Copy the JS launcher.
        bin_dir = staging_dir / "bin"
        bin_dir.mkdir(parents=True, exist_ok=True)
        shutil.copy2(LINGXI_NPM_ROOT / "bin" / "lingxi.js", bin_dir / "lingxi.js")

        # Use the existing package.json as the template; version + optDeps are
        # rewritten below.
        package_json_path = LINGXI_NPM_ROOT / "package.json"

    elif package in LINGXI_PLATFORM_PACKAGES:
        # Platform packages carry only a native binary (staged by
        # copy_native_binary) plus a minimal package.json.
        platform_pkg = LINGXI_PLATFORM_PACKAGES[package]

        with open(LINGXI_NPM_ROOT / "package.json", "r", encoding="utf-8") as fh:
            main_package_json = json.load(fh)

        package_json = {
            "name": platform_pkg["npm_name"],
            "version": version,
            "license": main_package_json.get("license", "MIT OR Apache-2.0"),
            "os": [platform_pkg["os"]],
            "cpu": [platform_pkg["cpu"]],
            "files": ["vendor"],
        }

        # Write the platform package.json directly (no further mutation needed).
        with open(staging_dir / "package.json", "w", encoding="utf-8") as out:
            json.dump(package_json, out, indent=2)
            out.write("\n")
        return

    else:
        raise RuntimeError(f"Unknown package '{package}'.")

    # --- main "lingxi" package manifest post-processing ---
    if package_json_path is not None:
        with open(package_json_path, "r", encoding="utf-8") as fh:
            package_json = json.load(fh)
        package_json["version"] = version

    if package == "lingxi":
        # Rewrite all 6 optionalDependencies versions to the release version.
        package_json["optionalDependencies"] = {
            name: version
            for name in LINGXI_PLATFORM_PACKAGES
        }

    with open(staging_dir / "package.json", "w", encoding="utf-8") as out:
        json.dump(package_json, out, indent=2)
        out.write("\n")


def copy_native_binary(
    vendor_src: Path,
    staging_dir: Path,
    target_triple: str | None,
) -> None:
    """Copy the compiled LingXi binary for *target_triple* into the staging dir.

    Source layout (produced by cargo build --release):
        <vendor_src>/<triple>/release/lingxi-cli[.exe]

    Destination layout (consumed by bin/lingxi.js):
        <staging_dir>/vendor/<triple>/bin/lingxi[.exe]
    """
    vendor_src = vendor_src.resolve()
    if not vendor_src.exists():
        raise RuntimeError(f"Vendor source directory not found: {vendor_src}")

    if target_triple is None:
        raise RuntimeError("target_triple must be specified for copy_native_binary.")

    is_windows = target_triple.endswith("-windows-msvc")
    src_name = "lingxi-cli.exe" if is_windows else "lingxi-cli"
    dst_name = "lingxi.exe" if is_windows else "lingxi"

    src_path = vendor_src / target_triple / "release" / src_name
    if not src_path.exists():
        raise RuntimeError(
            f"Binary not found: {src_path}\n"
            "Run 'cargo build --release --target <triple>' first, or "
            "point --vendor-src at the directory containing <triple>/release/."
        )

    dst_dir = staging_dir / "vendor" / target_triple / "bin"
    dst_dir.mkdir(parents=True, exist_ok=True)
    dst_path = dst_dir / dst_name
    shutil.copy2(src_path, dst_path)
    # Ensure the binary is executable on non-Windows platforms.
    if not is_windows:
        dst_path.chmod(dst_path.stat().st_mode | 0o111)

    broker_src = vendor_src / target_triple / "credential-broker"
    if broker_src.exists():
        broker_dst = staging_dir / "vendor" / target_triple / "credential-broker"
        shutil.copytree(broker_src, broker_dst, dirs_exist_ok=True)


def run_command(cmd: list[str], cwd: Path | None = None) -> None:
    print("+", " ".join(cmd), flush=True)
    subprocess.run(cmd, cwd=cwd, check=True)


def run_npm_pack(staging_dir: Path, output_path: Path) -> Path:
    output_path = output_path.resolve()
    output_path.parent.mkdir(parents=True, exist_ok=True)

    with tempfile.TemporaryDirectory(prefix="lingxi-npm-pack-") as pack_dir_str:
        pack_dir = Path(pack_dir_str)
        npm_cache_dir = pack_dir / "npm-cache"
        npm_logs_dir = pack_dir / "npm-logs"
        npm_cache_dir.mkdir()
        npm_logs_dir.mkdir()
        env = os.environ.copy()
        env["NPM_CONFIG_CACHE"] = str(npm_cache_dir)
        env["NPM_CONFIG_LOGS_DIR"] = str(npm_logs_dir)
        stdout = subprocess.check_output(
            ["npm", "pack", "--json", "--pack-destination", str(pack_dir)],
            cwd=staging_dir,
            env=env,
            text=True,
        )
        try:
            pack_output = json.loads(stdout)
        except json.JSONDecodeError as exc:
            raise RuntimeError("Failed to parse npm pack output.") from exc

        if not pack_output:
            raise RuntimeError("npm pack did not produce an output tarball.")

        tarball_name = pack_output[0].get("filename") or pack_output[0].get("name")
        if not tarball_name:
            raise RuntimeError("Unable to determine npm pack output filename.")

        tarball_path = pack_dir / tarball_name
        if not tarball_path.exists():
            raise RuntimeError(f"Expected npm pack output not found: {tarball_path}")

        shutil.move(str(tarball_path), output_path)

    return output_path


if __name__ == "__main__":
    import sys

    sys.exit(main())
