#!/usr/bin/env python3
"""Recheck the shipped crate against the lock, captured index and Git tree."""

import argparse
import hashlib
import json
from pathlib import Path, PurePosixPath
import tarfile
import tomllib

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[4]
NAME = "is_terminal_polyfill"
VERSION = "1.70.2"
EXPECTED_SHA256 = "a6cb138bb79a146c1bd460005623e142ef0181e3d0219cb493e02f7d08a35695"
COMMIT = "9ed4c24ea4e9202cb38a6564368252ed35197a08"


def sha256(data):
    return hashlib.sha256(data).hexdigest()


def git_hash(kind, data):
    return hashlib.sha1(f"{kind} {len(data)}\0".encode() + data).hexdigest()


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--cache", type=Path, help="optional extracted Cargo source directory")
    args = parser.parse_args()
    archive = (HERE / f"{NAME}-{VERSION}.crate").read_bytes()
    assert sha256(archive) == EXPECTED_SHA256
    lock = tomllib.loads((ROOT / "Cargo.lock").read_text())
    package = next(p for p in lock["package"] if p["name"] == NAME and p["version"] == VERSION)
    assert package["source"] == "registry+https://github.com/rust-lang/crates.io-index"
    assert package["checksum"] == EXPECTED_SHA256
    entries = [json.loads(line) for line in (HERE / "crates-index.jsonl").read_text().splitlines()]
    index = next(e for e in entries if e["vers"] == VERSION)
    assert index["name"] == NAME and index["cksum"] == EXPECTED_SHA256
    assert index["deps"] == [] and index["features"] == {"default": []}

    commit = json.loads((HERE / "upstream-commit.json").read_text())
    tree = json.loads((HERE / "upstream-tree.json").read_text())
    assert commit["sha"] == COMMIT and tree["sha"] == COMMIT and not tree["truncated"]
    commit_object = (HERE / "upstream-commit-object.txt").read_bytes()
    assert git_hash("commit", commit_object) == COMMIT
    assert commit_object.split(b"\n", 1)[0] == f"tree {commit['tree']['sha']}".encode()
    by_path = {e["path"]: e for e in tree["tree"]}
    assert len(by_path) == len(tree["tree"])
    # Reconstruct every Git tree from the recursive API response; a tree entry
    # sorts as its name plus '/', and its raw child SHA is part of the object.
    dirs = {"": []}
    for path, entry in by_path.items():
        pp = PurePosixPath(path)
        parent = str(pp.parent) if str(pp.parent) != "." else ""
        dirs.setdefault(parent, []).append((pp.name, entry))
        if entry["type"] == "tree":
            dirs.setdefault(path, [])
    for directory in sorted(dirs, key=lambda s: (s.count("/"), len(s)), reverse=True):
        items = sorted(dirs[directory], key=lambda item: item[0].encode() + (b"/" if item[1]["type"] == "tree" else b""))
        body = b"".join(e["mode"].lstrip("0").encode() + b" " + name.encode() + b"\0" + bytes.fromhex(e["sha"]) for name, e in items)
        expected = by_path[directory]["sha"] if directory else commit["tree"]["sha"]
        assert git_hash("tree", body) == expected, directory

    files = {}
    prefix = f"{NAME}-{VERSION}/"
    with tarfile.open(fileobj=__import__("io").BytesIO(archive), mode="r:gz") as tar:
        for member in tar:
            assert member.isfile() and member.name.startswith(prefix)
            relative = member.name[len(prefix):]
            assert relative and not PurePosixPath(relative).is_absolute() and ".." not in PurePosixPath(relative).parts
            assert relative not in files
            data = tar.extractfile(member).read()
            if relative == ".cargo_vcs_info.json":
                vcs = json.loads(data)
                assert vcs["git"]["sha1"] == COMMIT and vcs["path_in_vcs"] == ""
            files[relative] = {"size": len(data), "mode": oct(member.mode), "sha256": sha256(data), "git_blob": git_hash("blob", data)}
            if args.cache:
                assert (args.cache / relative).read_bytes() == data, relative
            if relative not in (".cargo_vcs_info.json", "Cargo.toml"):
                upstream = "Cargo.toml" if relative == "Cargo.toml.orig" else relative
                assert by_path[upstream]["type"] == "blob" and by_path[upstream]["sha"] == git_hash("blob", data), relative
    assert set(files) == {".cargo_vcs_info.json", "Cargo.lock", "Cargo.toml", "Cargo.toml.orig", "LICENSE-APACHE", "LICENSE-MIT", "README.md", "src/lib.rs"}
    if args.cache:
        cached = {p.relative_to(args.cache).as_posix() for p in args.cache.rglob("*") if p.is_file()}
        assert cached == set(files) | {".cargo-ok"}
    print(json.dumps({"archive_sha256": EXPECTED_SHA256, "lock_sha256": sha256((ROOT / "Cargo.lock").read_bytes()), "index_yanked_at_capture": index["yanked"], "upstream_commit": COMMIT, "commit_object_verified": True, "upstream_root_tree": commit["tree"]["sha"], "tree_objects_checked": len(dirs), "cache_checked": bool(args.cache), "upstream_blob_files_checked": 6, "archive_files": files}, indent=2, sort_keys=True))


if __name__ == "__main__":
    main()
