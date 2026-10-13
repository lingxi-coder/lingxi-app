"""Isolated SDK snapshots shared by reader-example and Rustdoc checks.

Only generated temporary manifests/lockfiles are changed. The consumer's local
patch closure is explicit; metadata rejects accidentally retained SDK Git pins.
"""
from __future__ import annotations

import hashlib
from contextlib import contextmanager
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import tomllib

SDKS = ("harness-runtime", "llm-client", "mobile-linux-runtime")
WEB = Path(__file__).resolve().parents[1]


@contextmanager
def cargo_target_lock(target: Path):
    """Serialize whole Cargo pipelines, including their executable doctests.

    Cargo releases some artifact locks before rustdoc test workers finish;
    another snapshot then rebuilding dependencies can invalidate those workers.
    This lock covers example and Rustdoc pipelines sharing a dependency cache.
    """
    target.mkdir(parents=True, exist_ok=True)
    with (target / ".lingxi-handbook.lock").open("a+b") as handle:
        if os.name == "nt":
            import msvcrt
            if handle.tell() == 0:
                handle.write(b"0")
                handle.flush()
            handle.seek(0)
            msvcrt.locking(handle.fileno(), msvcrt.LK_LOCK, 1)
        else:
            import fcntl
            fcntl.flock(handle.fileno(), fcntl.LOCK_EX)
        try:
            yield
        finally:
            if os.name == "nt":
                handle.seek(0)
                msvcrt.locking(handle.fileno(), msvcrt.LK_UNLCK, 1)
            else:
                fcntl.flock(handle.fileno(), fcntl.LOCK_UN)


def isolated_cargo_home(source: Path, destination: Path) -> None:
    """Keep offline package extraction/locks in a writable temporary cache.

    Existing archives and unpacked crate sources are shared as read-only
    symlinks. Indexes and Git refs/objects have writable mirrors so an explicit
    online run can add new dependencies without modifying the user's cache.
    """
    destination.mkdir(parents=True)
    for relative in ("registry/cache", "git/checkouts"):
        original = source / relative
        if not original.exists():
            continue
        for group in original.iterdir():
            if not group.is_dir():
                continue
            parent = destination / relative / group.name
            parent.mkdir(parents=True)
            for entry in group.iterdir():
                (parent / entry.name).symlink_to(entry.resolve(), target_is_directory=entry.is_dir())
    index = source / "registry" / "index"
    if index.exists():
        shutil.copytree(index, destination / "registry" / "index")
    git_db = source / "git" / "db"
    if git_db.exists():
        for repository in git_db.iterdir():
            if not repository.is_dir():
                continue
            mirror = destination / "git" / "db" / repository.name
            mirror.parent.mkdir(parents=True, exist_ok=True)
            command(["git", "clone", "--bare", "--shared", "--quiet", str(repository), str(mirror)], cwd=destination)
    original_sources = source / "registry" / "src"
    if original_sources.exists():
        for registry in original_sources.iterdir():
            if not registry.is_dir():
                continue
            parent = destination / "registry" / "src" / registry.name
            parent.mkdir(parents=True)
            for package in registry.iterdir():
                (parent / package.name).symlink_to(package.resolve(), target_is_directory=package.is_dir())


def command(args: list[str], *, cwd: Path, timeout: int = 1800) -> subprocess.CompletedProcess[str]:
    env = os.environ.copy()
    if Path(args[0]).name in {"cargo", "rustc"}:
        # Documentation checks need type/link correctness, not debugger data.
        # Scope the small profile to this child process; preserve user profiles
        # and existing Cargo artifacts rather than pruning the shared cache.
        env.update({"CARGO_PROFILE_DEV_DEBUG": "0", "CARGO_PROFILE_TEST_DEBUG": "0", "CARGO_INCREMENTAL": "0"})
    try:
        result = subprocess.run(args, cwd=cwd, text=True, capture_output=True, timeout=timeout, env=env)
    except subprocess.TimeoutExpired as error:
        raise RuntimeError(f"Timed out after {timeout}s: {' '.join(args)}") from error
    if result.returncode:
        failure_start = result.stdout.find("failures:")
        output = result.stdout[failure_start:] if failure_start >= 0 else result.stdout[-4000:]
        raise RuntimeError(f"{' '.join(args)}\n{output}{result.stderr[-12000:]}")
    return result


def snapshot_repo(source: Path, destination: Path) -> None:
    """Copy tracked + nonignored source, including initialized Git submodules."""
    paths = subprocess.check_output(
        ["git", "-C", str(source), "ls-files", "--cached", "--others", "--exclude-standard", "-z"]
    ).decode().split("\0")
    destination.mkdir(parents=True)
    for name in sorted(set(filter(None, paths))):
        path = source / name
        if not path.exists():  # an intentionally deleted working-tree file
            continue
        target = destination / name
        target.parent.mkdir(parents=True, exist_ok=True)
        if path.is_dir():  # Gitlink, not a normal tracked file
            snapshot_repo(path, target)
        else:
            shutil.copy2(path, target, follow_symlinks=True)


class SdkSnapshot:
    def __init__(self, workspace: Path):
        self.workspace = workspace.resolve()
        self.temp = tempfile.TemporaryDirectory(prefix="lingxi-handbook-")
        self.root = Path(self.temp.name).resolve()
        for name in SDKS:
            if not (self.workspace / name / "Cargo.toml").exists():
                raise RuntimeError(f"Missing SDK checkout: {self.workspace / name}")
            snapshot_repo(self.workspace / name, self.root / name)
        toolchains = {name: tomllib.loads((self.root / name / "rust-toolchain.toml").read_text())["toolchain"]["channel"] for name in SDKS}
        if len(set(toolchains.values())) != 1:
            raise RuntimeError(f"SDK toolchain channels disagree: {toolchains}")
        shutil.copy2(self.root / "harness-runtime" / "rust-toolchain.toml", self.root / "rust-toolchain.toml")
        self.toolchain = next(iter(toolchains.values()))
        hasher = hashlib.sha256()
        for name in SDKS:
            for path in sorted((self.root / name).rglob("*")):
                if path.is_file():
                    hasher.update(str(path.relative_to(self.root)).encode() + b"\0")
                    hasher.update(path.read_bytes())
        self.source_fingerprint = hasher.hexdigest()
        self.previous_cargo_home = os.environ.get("CARGO_HOME")
        cached_home = Path(self.previous_cargo_home or Path.home() / ".cargo")
        isolated_cargo_home(cached_home, self.root / "cargo-home")
        os.environ["CARGO_HOME"] = str(self.root / "cargo-home")
        self.expected: dict[str, Path] = {}
        for name in SDKS:
            for manifest in (self.root / name).rglob("Cargo.toml"):
                package = tomllib.loads(manifest.read_text()).get("package", {})
                if "name" in package:
                    # SDK public/workspace and vendored packages are all kept local.
                    self.expected[package["name"]] = manifest.resolve()
        self.overlay(self.root / "harness-runtime" / "Cargo.toml")

    def close(self) -> None:
        if self.previous_cargo_home is None:
            os.environ.pop("CARGO_HOME", None)
        else:
            os.environ["CARGO_HOME"] = self.previous_cargo_home
        self.temp.cleanup()

    def compiler_evidence(self) -> dict[str, str]:
        compiler = command(["rustc", "-vV"], cwd=self.root).stdout
        host = next((line.removeprefix("host: ") for line in compiler.splitlines() if line.startswith("host: ")), "unknown")
        return {"toolchain": self.toolchain, "compiler": compiler.splitlines()[0], "hostTarget": host}

    def overlay(self, manifest: Path) -> None:
        text = manifest.read_text()
        additions = {
            "https://github.com/lingxi-coder/llm-client.git": {
                "lingxi-llm-client": self.root / "llm-client",
            },
            "https://github.com/lingxi-coder/mobile-linux-runtime.git": {
                name: self.root / "mobile-linux-runtime" / "crates" / name
                for name in ("mobile-linux-api", "mobile-linux-core", "mobile-linux-android", "mobile-linux-ios", "platform-pty")
            },
        }
        for url, packages in additions.items():
            section = f'[patch."{url}"]'
            if section in text:
                raise RuntimeError(f"Snapshot patch unexpectedly already defined in {manifest}: {url}")
            text += "\n" + section + "\n"
            text += "".join(f'{name} = {{ path = "{path}" }}\n' for name, path in packages.items())
        manifest.write_text(text)

    def reader_manifest(self, text: str) -> str:
        """Rebind only checkout-relative paths; preserve the copied reader config."""
        for name in SDKS:
            text = text.replace(f'path = "../{name}', f'path = "{self.root / name}')
        return text

    def verify_metadata(self, manifest: Path, *, offline: bool = True, feature_args: list[str] | None = None) -> list[dict[str, str]]:
        args = ["cargo", "metadata", "--format-version", "1", "--manifest-path", str(manifest)]
        args += feature_args or []
        if offline:
            args.append("--offline")
        metadata = json.loads(command(args, cwd=manifest.parent).stdout)
        identities = []
        resolved = {node["id"] for node in metadata["resolve"]["nodes"]}
        for package in metadata["packages"]:
            expected = self.expected.get(package["name"])
            if expected is None or package["id"] not in resolved:
                continue
            actual = Path(package["manifest_path"]).resolve()
            if actual != expected or package["source"] is not None:
                raise RuntimeError(
                    f"Wrong SDK identity for {package['name']}: {actual}, {package['source']}; expected {expected}"
                )
            identities.append({"package": package["name"], "manifest": str(actual.relative_to(self.root)), "source": "working-tree"})
        if not identities:
            raise RuntimeError(f"No SDK package was resolved by {manifest}")
        return sorted(identities, key=lambda entry: entry["package"])


def docs_blocks(web: Path = WEB) -> list[dict]:
    expression = """import {docPages} from './src/data/docs/index.ts';
console.log(JSON.stringify(docPages.flatMap(page => page.sections.flatMap(section =>
  (section.code ?? []).map(block => ({...block, page: page.id, group: page.group, section: section.id}))))));"""
    return json.loads(command(["node", "--import", "tsx", "--input-type=module", "-e", expression], cwd=web).stdout)


def cargo_args(verb: str, manifest: Path, *, offline: bool, target: Path) -> list[str]:
    args = ["cargo", verb, "--manifest-path", str(manifest), "--target-dir", str(target)]
    if offline:
        args.append("--offline")
    return args


def cached_revision(repo: str, revision: str) -> Path:
    """Locate an exact reviewed Git snapshot already present in Cargo's cache."""
    home = Path(os.environ.get("CARGO_HOME", Path.home() / ".cargo"))
    for checkout in sorted((home / "git" / "checkouts").glob(f"{repo}-*/*")):
        result = subprocess.run(["git", "-C", str(checkout), "rev-parse", "HEAD"], text=True, capture_output=True)
        if result.returncode == 0 and result.stdout.strip() == revision:
            return checkout
    raise RuntimeError(f"Exact committed source is unavailable offline: {repo}@{revision}")
