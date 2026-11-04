#!/usr/bin/env python3
"""Independently bind the reviewed archive to Cargo and the full upstream Git tree."""
import hashlib
import json
import pathlib
import re
import subprocess
import tarfile

HERE = pathlib.Path(__file__).resolve().parent
REPO = HERE.parents[4]
ARCHIVE = HERE / "futures-task-0.3.32.crate"
COMMIT = "d9bba94c239daa1175a5bb2958f37a5c72db3f6a"
SHA256 = "037711b3d59c33004d3856fbdc83b99d4ff37a24768fa1be9ce3538a1cde4393"
BASES = ["e67108615b748d0e1bd330e52e19de8a64f7f3d8"]


def git_object(kind, data):
    return hashlib.sha1((kind + " " + str(len(data)) + "\0").encode() + data).hexdigest()


archive_bytes = ARCHIVE.read_bytes()
assert hashlib.sha256(archive_bytes).hexdigest() == SHA256
with tarfile.open(ARCHIVE, "r:gz") as archive:
    members = archive.getmembers()
    assert len(members) == 14 and all(m.isfile() for m in members)
    assert len({m.name for m in members}) == len(members)
    prefix = "futures-task-0.3.32/"
    assert all(m.name.startswith(prefix) and ".." not in pathlib.PurePosixPath(m.name).parts for m in members)
    files = {m.name[len(prefix):]: archive.extractfile(m).read() for m in members}
expected = {
    ".cargo_vcs_info.json", "Cargo.lock", "Cargo.toml", "Cargo.toml.orig", "LICENSE-APACHE",
    "LICENSE-MIT", "README.md", "src/arc_wake.rs", "src/future_obj.rs", "src/lib.rs",
    "src/noop_waker.rs", "src/spawn.rs", "src/waker.rs", "src/waker_ref.rs",
}
assert set(files) == expected
source_paths = sorted(pathlib.Path.home().glob(".cargo/registry/src/*/futures-task-0.3.32"))
assert source_paths
for source_path in source_paths:
    cached_files = {str(p.relative_to(source_path)) for p in source_path.rglob("*") if p.is_file()}
    assert cached_files - {".cargo-ok"} == expected
    assert all((source_path / name).read_bytes() == contents for name, contents in files.items())
assert json.loads(files[".cargo_vcs_info.json"]) == {
    "git": {"sha1": COMMIT}, "path_in_vcs": "futures-task"
}

index = [json.loads(line) for line in (HERE / "crates-index.jsonl").read_text().splitlines()]
record = next(item for item in index if item.get("name") == "futures-task" and item.get("vers") == "0.3.32")
assert record["cksum"] == SHA256 and record["yanked"] is False
cache_paths = sorted(pathlib.Path.home().glob(".cargo/registry/cache/*/futures-task-0.3.32.crate"))
assert cache_paths and all(hashlib.sha256(p.read_bytes()).hexdigest() == SHA256 for p in cache_paths)
index_paths = sorted(pathlib.Path.home().glob(".cargo/registry/index/*/.cache/fu/tu/futures-task"))
assert index_paths
for path in index_paths:
    records = [json.loads(part) for part in path.read_bytes().split(b"\0") if part.startswith(b'{"name":')]
    cached = next(r for r in records if r.get("vers") == "0.3.32")
    assert cached["cksum"] == SHA256 and cached["name"] == record["name"]

lock_hashes = {}
for rev in BASES:
    raw = subprocess.check_output(["git", "show", rev + ":Cargo.lock"], cwd=REPO)
    match = re.search(rb'(?m)^name = "futures-task"\nversion = "0\.3\.32"\nsource = "registry\+https://github\.com/rust-lang/crates\.io-index"\nchecksum = "([0-9a-f]{64})"', raw)
    assert match and match.group(1).decode() == SHA256
    lock_hashes[rev] = hashlib.sha256(raw).hexdigest()
assert len(set(lock_hashes.values())) == 1

commit_raw = (HERE / "upstream-commit.raw").read_bytes()
assert git_object("commit", commit_raw) == COMMIT
upstream = json.loads((HERE / "upstream-commit.json").read_text())
assert upstream["sha"] == COMMIT
root_sha = re.match(rb"tree ([0-9a-f]{40})\n", commit_raw).group(1).decode()
assert upstream["tree"]["sha"] == root_sha
tree_data = json.loads((HERE / "upstream-tree.json").read_text())
assert tree_data["sha"] == COMMIT and tree_data["truncated"] is False
entries = {e["path"]: e for e in tree_data["tree"]}
assert len(entries) == len(tree_data["tree"])
children = {"": []}
for name, entry in entries.items():
    parent, _, basename = name.rpartition("/")
    children.setdefault(parent, []).append((basename, entry))
    if entry["type"] == "tree":
        children.setdefault(name, [])
    assert len(bytes.fromhex(entry["sha"])) == 20
tree_hashes = {}
for directory in sorted(children, key=lambda x: x.count("/") + bool(x), reverse=True):
    payload = bytearray()
    for basename, item in sorted(children[directory], key=lambda x: x[0].encode() + (b"/" if x[1]["type"] == "tree" else b"")):
        if item["type"] == "tree":
            full = (directory + "/" if directory else "") + basename
            assert tree_hashes[full] == item["sha"]
        mode = item["mode"].lstrip("0").encode()
        payload.extend(mode + b" " + basename.encode() + b"\0" + bytes.fromhex(item["sha"]))
    tree_hashes[directory] = git_object("tree", bytes(payload))
assert tree_hashes[""] == root_sha

mappings = {p: "futures-task/" + p for p in files if p.startswith("src/") or p == "README.md"}
mappings["Cargo.toml.orig"] = "futures-task/Cargo.toml"
for name in ["LICENSE-APACHE", "LICENSE-MIT"]:
    mappings[name] = name
    assert entries["futures-task/" + name]["mode"] == "120000"
    assert entries["futures-task/" + name]["sha"] == git_object("blob", ("../" + name).encode())
for archive_name, tree_name in mappings.items():
    item = entries[tree_name]
    assert item["type"] == "blob" and item["mode"] == "100644"
    assert item["size"] == len(files[archive_name]) and item["sha"] == git_object("blob", files[archive_name])

result = {
    "archive_sha256": SHA256,
    "archive_files_sha256": {p: hashlib.sha256(files[p]).hexdigest() for p in sorted(files)},
    "cache_archive_count": len(cache_paths),
    "cache_extracted_source_count": len(source_paths),
    "cached_index_count": len(index_paths),
    "index_checksum": record["cksum"],
    "index_yanked": record["yanked"],
    "git_lock_sha256": lock_hashes,
    "upstream_commit_git_sha1": COMMIT,
    "upstream_root_git_tree_sha1": root_sha,
    "upstream_total_entries": len(entries),
    "upstream_all_directory_count": len(tree_hashes),
    "upstream_matched_archive_file_count": len(mappings),
    "upstream_archive_mappings": mappings,
    "archive_cargo_generated_files": [".cargo_vcs_info.json", "Cargo.lock", "Cargo.toml"],
}
print(json.dumps(result, indent=2, sort_keys=True))
