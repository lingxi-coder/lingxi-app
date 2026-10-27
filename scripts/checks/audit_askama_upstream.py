#!/usr/bin/env python3
"""Audit the registry baselines of the pinned upstream Askama Git packages.

Cargo-audit skips Git sources. These registry identities retain advisory
coverage for their 0.16.1 baselines; cargo-vet separately reviews the Git delta.
"""

import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import tarfile
import tempfile
import urllib.request


VERSION = "0.16.1"
REPOSITORY = "https://github.com/askama-rs/askama.git"
REVISION = "3863808cf594054f7644979ed92caaafc878f378"
BASELINE_REVISION = "9ae4d5bdf4352b35e34106e38944c90d7a7ede3e"
SOURCE = f"git+{REPOSITORY}?rev={REVISION}#{REVISION}"
REGISTRY_SOURCE = "registry+https://github.com/rust-lang/crates.io-index"
ARCHIVES = {
    "askama_derive": "071ee5ebf2138e3ad180e0aacf6940c2cab5e6d8333741d9925c7bee2b153f39",
    "askama_parser": "2c5ae75772275d268b03ab8bdccdd12117b6169ee23256942b34e46c9f476583",
}
MAX_ARCHIVE_BYTES = 2 * 1024 * 1024


def verify_git_packages(metadata):
    """Require exactly the reviewed Git identities from one Cargo checkout."""
    roots = set()
    for name in ARCHIVES:
        matches = [package for package in metadata["packages"] if package["name"] == name]
        if len(matches) != 1:
            raise ValueError(f"expected exactly one {name} package in Cargo metadata")
        package = matches[0]
        if package["version"] != VERSION or package["source"] != SOURCE:
            raise ValueError(f"{name} is not the pinned upstream Git identity")
        manifest = Path(package["manifest_path"]).resolve()
        if manifest.name != "Cargo.toml" or manifest.parent.name != name:
            raise ValueError(f"{name} has an unexpected Git source layout")
        roots.add(manifest.parent.parent)
    if len(roots) != 1:
        raise ValueError("Askama Git packages resolved from different checkouts")
    root = roots.pop()
    # These three files are symlinks in the upstream Git tree. Windows Git
    # may otherwise check them out as plain files containing only link text.
    for source, target in (
        ("askama_derive/src/ascii_str.rs", "askama/src/ascii_str.rs"),
        ("askama_derive/src/html.rs", "askama/src/html.rs"),
        ("askama_parser/src/ascii_str.rs", "askama/src/ascii_str.rs"),
    ):
        link = root / source
        original = root / target
        if not link.is_file() or not original.is_file() or link.read_bytes() != original.read_bytes():
            raise ValueError(f"Askama checkout did not resolve the upstream source link: {source}")


def registry_archive(name, directory):
    """Read the official crate archive from Cargo's cache or crates.io."""
    cargo_home = Path(os.environ.get("CARGO_HOME", str(Path.home() / ".cargo")))
    filename = f"{name}-{VERSION}.crate"
    cached = sorted((cargo_home / "registry/cache").glob(f"*/{filename}"))
    if cached:
        return cached[0]
    url = f"https://static.crates.io/crates/{name}/{filename}"
    request = urllib.request.Request(url, headers={"User-Agent": "upstream-advisory-audit/1.0"})
    with urllib.request.urlopen(request, timeout=60) as response:
        data = response.read(MAX_ARCHIVE_BYTES + 1)
    if len(data) > MAX_ARCHIVE_BYTES:
        raise ValueError(f"{name} registry archive exceeds the expected size limit")
    archive = directory / filename
    archive.write_bytes(data)
    return archive


def verify_archive(name, archive):
    if archive.stat().st_size > MAX_ARCHIVE_BYTES:
        raise ValueError(f"{name} registry archive exceeds the expected size limit")
    if hashlib.sha256(archive.read_bytes()).hexdigest() != ARCHIVES[name]:
        raise ValueError(f"{name} registry archive checksum differs from the pinned baseline")
    with tarfile.open(archive, "r:gz") as source:
        member = source.getmember(f"{name}-{VERSION}/.cargo_vcs_info.json")
        if not member.isfile():
            raise ValueError(f"{name} registry provenance is not an ordinary file")
        provenance = json.load(source.extractfile(member))
    if provenance["git"]["sha1"] != BASELINE_REVISION or provenance["path_in_vcs"] != name:
        raise ValueError(f"{name} registry archive has an unexpected Git baseline")


def verify_checkout(workspace):
    metadata = subprocess.run(
        ["cargo", "metadata", "--locked", "--all-features", "--format-version=1"],
        cwd=workspace, check=True, text=True, capture_output=True,
    )
    verify_git_packages(json.loads(metadata.stdout))


def audit(workspace):
    verify_checkout(workspace)
    with tempfile.TemporaryDirectory(prefix="askama-upstream-audit-") as temporary:
        directory = Path(temporary)
        for name in ARCHIVES:
            verify_archive(name, registry_archive(name, directory))
        # Do not add the old crates' transitive dependencies to the actual
        # workspace graph: the complete workspace audit checks current deps.
        lock = directory / "Cargo.lock"
        lock_text = "version = 4\n"
        for name, checksum in ARCHIVES.items():
            lock_text += (f'\n[[package]]\nname = "{name}"\nversion = "{VERSION}"\n'
                          f'source = "{REGISTRY_SOURCE}"\nchecksum = "{checksum}"\n')
        lock.write_text(lock_text, encoding="utf-8")
        # Do not inherit a user's ignore list for this supplementary audit.
        config = directory / ".cargo/audit.toml"
        config.parent.mkdir()
        config.write_text("[advisories]\nignore = []\n", encoding="utf-8")
        print("Auditing the two crates.io Askama 0.16.1 baselines; the Git delta requires separate review", flush=True)
        subprocess.run(["cargo", "audit", "--deny", "warnings", "--file", str(lock)], cwd=directory, check=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--workspace", type=Path, default=Path.cwd())
    parser.add_argument("--checkout-only", action="store_true", help="verify pinned Git source and checkout links without auditing")
    args = parser.parse_args()
    try:
        if args.checkout_only:
            verify_checkout(args.workspace.resolve())
            print("Verified the pinned Askama Git checkout and source links")
        else:
            audit(args.workspace.resolve())
    except subprocess.CalledProcessError as error:
        if error.stderr:
            print(error.stderr, file=sys.stderr)
        print(f"Askama upstream audit: command failed with exit code {error.returncode}", file=sys.stderr)
        return error.returncode if 0 < error.returncode < 126 else 1
    except (OSError, ValueError, KeyError, json.JSONDecodeError, tarfile.TarError) as error:
        parser.exit(1, f"Askama upstream audit: {error}\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
