#!/usr/bin/env python3
"""Check exact crate contents against the App lock, registry and Git objects."""

import argparse
import hashlib
import io
import json
from pathlib import Path, PurePosixPath
import tarfile
import tomllib

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[4]
NAME = "colorchoice"
VERSION = "1.0.5"
DIGEST = "1d07550c9036bf2ae0c684c4297d503f838287c83c53686d05370d0e139ae570"
COMMIT = "0fe6f0ff6d52e9f91d4071199bd0b24bd46f3d35"


def sha256(data):
    return hashlib.sha256(data).hexdigest()


def git_hash(kind, data):
    return hashlib.sha1(f"{kind} {len(data)}\0".encode() + data).hexdigest()


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--cache", type=Path, help="optional extracted Cargo source directory")
    args = parser.parse_args()
    archive = (HERE / f"{NAME}-{VERSION}.crate").read_bytes()
    assert sha256(archive) == DIGEST
    lock_bytes = (ROOT / "Cargo.lock").read_bytes()
    packages = tomllib.loads(lock_bytes.decode())["package"]
    package = next(p for p in packages if p["name"] == NAME and p["version"] == VERSION)
    assert package["source"] == "registry+https://github.com/rust-lang/crates.io-index"
    assert package["checksum"] == DIGEST
    index = next(json.loads(line) for line in (HERE / "crates-index.jsonl").read_text().splitlines() if json.loads(line)["vers"] == VERSION)
    assert index["name"] == NAME and index["cksum"] == DIGEST
    assert index["deps"] == [] and index["features"] == {} and index["rust_version"] == "1.66.0"

    commit = json.loads((HERE / "upstream-commit.json").read_text())
    tree = json.loads((HERE / "upstream-tree.json").read_text())
    assert commit["sha"] == COMMIT and tree["sha"] == COMMIT and not tree["truncated"]
    commit_object = (HERE / "upstream-commit-object.txt").read_bytes()
    assert git_hash("commit", commit_object) == COMMIT
    assert commit_object.split(b"\n", 1)[0] == f"tree {commit['tree']['sha']}".encode()
    by_path = {e["path"]: e for e in tree["tree"]}
    assert len(by_path) == len(tree["tree"])
    directories = {"": []}
    for path, entry in by_path.items():
        pp = PurePosixPath(path)
        parent = str(pp.parent) if str(pp.parent) != "." else ""
        directories.setdefault(parent, []).append((pp.name, entry))
        if entry["type"] == "tree":
            directories.setdefault(path, [])
    for directory in sorted(directories, key=lambda s: (s.count("/"), len(s)), reverse=True):
        items = sorted(directories[directory], key=lambda i: i[0].encode() + (b"/" if i[1]["type"] == "tree" else b""))
        body = b"".join(e["mode"].lstrip("0").encode() + b" " + name.encode() + b"\0" + bytes.fromhex(e["sha"]) for name, e in items)
        expected = by_path[directory]["sha"] if directory else commit["tree"]["sha"]
        assert git_hash("tree", body) == expected, directory

    upstream_workspace = (HERE / "upstream-workspace-Cargo.toml").read_bytes()
    assert git_hash("blob", upstream_workspace) == by_path["Cargo.toml"]["sha"]
    workspace = tomllib.loads(upstream_workspace.decode())["workspace"]
    files = {}
    contents = {}
    prefix = f"{NAME}-{VERSION}/"
    with tarfile.open(fileobj=io.BytesIO(archive), mode="r:gz") as tar:
        for member in tar:
            assert member.isfile() and member.name.startswith(prefix)
            relative = member.name[len(prefix):]
            assert relative and not PurePosixPath(relative).is_absolute() and ".." not in PurePosixPath(relative).parts
            assert relative not in files
            data = tar.extractfile(member).read()
            contents[relative] = data
            files[relative] = {"size": len(data), "mode": oct(member.mode), "sha256": sha256(data), "git_blob": git_hash("blob", data)}
            if args.cache:
                assert (args.cache / relative).read_bytes() == data, relative
            if relative not in (".cargo_vcs_info.json", "Cargo.lock", "Cargo.toml"):
                upstream = "crates/colorchoice/" + ("Cargo.toml" if relative == "Cargo.toml.orig" else relative)
                assert by_path[upstream]["type"] == "blob" and by_path[upstream]["sha"] == git_hash("blob", data), relative
    assert set(files) == {".cargo_vcs_info.json", "Cargo.lock", "Cargo.toml", "Cargo.toml.orig", "LICENSE-APACHE", "LICENSE-MIT", "README.md", "src/lib.rs"}
    vcs = json.loads(contents[".cargo_vcs_info.json"])
    assert vcs["git"]["sha1"] == COMMIT and vcs["path_in_vcs"] == "crates/colorchoice"
    normalized = tomllib.loads(contents["Cargo.toml"].decode())
    original = tomllib.loads(contents["Cargo.toml.orig"].decode())
    for key in ("repository", "license", "edition", "rust-version", "include"):
        assert original["package"][key] == {"workspace": True}
        assert normalized["package"][key] == workspace["package"][key]
    assert normalized["lints"] == workspace["lints"]
    assert normalized["package"]["build"] is False and normalized["package"]["autolib"] is False
    assert tomllib.loads(contents["Cargo.lock"].decode())["package"] == [{"name": NAME, "version": VERSION}]
    if args.cache:
        cached = {p.relative_to(args.cache).as_posix() for p in args.cache.rglob("*") if p.is_file()}
        assert cached == set(files) | {".cargo-ok"}
    print(json.dumps({"archive_sha256": DIGEST, "lock_sha256": sha256(lock_bytes), "index_yanked_at_capture": index["yanked"], "upstream_commit": COMMIT, "commit_object_verified": True, "upstream_root_tree": commit["tree"]["sha"], "tree_objects_checked": len(directories), "upstream_blob_files_checked": 6, "normalized_inherited_values_match_upstream_workspace": True, "cache_checked": bool(args.cache), "archive_files": files}, indent=2, sort_keys=True))


if __name__ == "__main__":
    main()
