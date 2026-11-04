#!/usr/bin/env python3
"""Validate captured official source against the App lock and Git trees."""
import hashlib
import json
import tarfile
from pathlib import Path

here = Path(__file__).resolve().parent
expected_archive = "c08d65885ee38876c4f86fa503fb49d7b507c2b62552df7c70b2fce627e06381"
expected_commit = "dd4adddf8b716bb51eb8ccb8c73e47bff1e358aa"
expected_root = "ab61d531224b65dbca816ae817c4e424035f27fb"
expected_subtree = "e8dd1ea835919c7962d2ff5a51fec90f237571d2"


def git_object(kind, data):
    return hashlib.sha1(kind + b" " + str(len(data)).encode() + b"\0" + data).hexdigest()


commit = json.loads((here / "upstream-commit.json").read_text())
tree = json.loads((here / "upstream-tree.json").read_text())
assert commit["sha"] == expected_commit and commit["tree"]["sha"] == expected_root
assert tree["sha"] == expected_root and tree["truncated"] is False
entries = {e["path"]: e for e in tree["tree"]}
assert entries["opaque-debug"]["sha"] == expected_subtree

# Recompute every tree object (including root) from GitHub's recursive listing.
for folder, expected in [(p, e["sha"]) for p, e in entries.items() if e["type"] == "tree"] + [("", expected_root)]:
    prefix = folder + "/" if folder else ""
    children = []
    for path, item in entries.items():
        if path.startswith(prefix):
            name = path[len(prefix):]
            if "/" not in name:
                children.append((name, item))
    children.sort(key=lambda pair: pair[0].encode() + (b"/" if pair[1]["type"] == "tree" else b""))
    payload = b"".join(
        format(int(item["mode"], 8), "o").encode() + b" " + name.encode() + b"\0" + bytes.fromhex(item["sha"])
        for name, item in children
    )
    assert git_object(b"tree", payload) == expected, folder

idx = [json.loads(line) for line in (here / "crates-index.jsonl").read_text().splitlines()]
record = next(x for x in idx if x["vers"] == "0.3.1")
assert record["name"] == "opaque-debug" and record["cksum"] == expected_archive
assert record["deps"] == [] and record["features"] == {} and record["yanked"] is False
archive = here / "opaque-debug-0.3.1.crate"
assert hashlib.sha256(archive.read_bytes()).hexdigest() == expected_archive

seen = set()
inventory = []
with tarfile.open(archive) as package:
    for member in package.getmembers():
        assert member.isfile() and member.name.startswith("opaque-debug-0.3.1/")
        rel = member.name[len("opaque-debug-0.3.1/"):]
        assert rel and all(part not in ("", ".", "..") for part in rel.split("/")) and rel not in seen
        seen.add(rel)
        data = package.extractfile(member).read()
        if rel == ".cargo_vcs_info.json":
            vcs = json.loads(data)
            assert vcs["git"]["sha1"] == expected_commit and vcs["path_in_vcs"] == "opaque-debug"
            upstream = "Cargo-generated"
        elif rel == "Cargo.toml":
            upstream = "Cargo-normalized"
        else:
            path = "opaque-debug/" + ("Cargo.toml" if rel == "Cargo.toml.orig" else rel)
            assert git_object(b"blob", data) == entries[path]["sha"], rel
            upstream = entries[path]["sha"]
            if rel == "src/lib.rs":
                assert data == (here / "upstream-lib.rs").read_bytes()
        inventory.append({"path": rel, "sha256": hashlib.sha256(data).hexdigest(), "upstream_git_blob": upstream})

assert seen == {".cargo_vcs_info.json", "CHANGELOG.md", "Cargo.toml", "Cargo.toml.orig", "LICENSE-APACHE", "LICENSE-MIT", "README.md", "src/lib.rs", "tests/mod.rs"}
print(json.dumps({"archive_sha256": expected_archive, "commit": expected_commit, "upstream_root_tree": expected_root, "upstream_package_tree": expected_subtree, "verified_git_tree_count": sum(x["type"] == "tree" for x in entries.values()) + 1, "files": inventory}, indent=2))
