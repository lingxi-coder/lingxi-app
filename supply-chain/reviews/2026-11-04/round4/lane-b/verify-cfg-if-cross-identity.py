#!/usr/bin/env python3
"""Cross-check cfg-if registry bytes against its immutable upstream tree."""
import hashlib
import json
import os
from pathlib import Path
import tarfile

here = Path(__file__).resolve().parent
archive = Path(os.environ["CFG_IF_ARCHIVE"])
expected_archive = "9330f8b2ff13f34540b44e946ef35111825727b38d33286ef986142615121801"
expected_commit = "3510ca6abea34cbbc702509a4e50ea9709925eda"
expected_tree = "b6bc463d3377bd0d3288ae4d66878803f2ac6bb5"

def oid(kind, data):
    return hashlib.sha1(kind + b" " + str(len(data)).encode() + b"\0" + data).hexdigest()

commit = json.loads((here / "cfg-if-upstream-commit.json").read_text())
tree = json.loads((here / "cfg-if-upstream-tree.json").read_text())
assert commit["sha"] == expected_commit and commit["tree"]["sha"] == expected_tree
assert tree["sha"] == expected_tree and tree["truncated"] is False
entries = {entry["path"]: entry for entry in tree["tree"]}
for folder, expected in [(p, e["sha"]) for p, e in entries.items() if e["type"] == "tree"] + [("", expected_tree)]:
    prefix = folder + "/" if folder else ""
    children = [(path[len(prefix):], e) for path, e in entries.items() if path.startswith(prefix) and "/" not in path[len(prefix):]]
    children.sort(key=lambda pair: pair[0].encode() + (b"/" if pair[1]["type"] == "tree" else b""))
    payload = b"".join(format(int(e["mode"], 8), "o").encode() + b" " + n.encode() + b"\0" + bytes.fromhex(e["sha"]) for n, e in children)
    assert oid(b"tree", payload) == expected, folder
assert hashlib.sha256(archive.read_bytes()).hexdigest() == expected_archive
inventory = []
with tarfile.open(archive) as package:
    for member in package.getmembers():
        assert member.isfile() and member.name.startswith("cfg-if-1.0.4/")
        rel = member.name[len("cfg-if-1.0.4/"):]
        data = package.extractfile(member).read()
        if rel in ("Cargo.toml", ".cargo_vcs_info.json", "Cargo.lock"):
            result = "Cargo generated/normalized"
        else:
            path = "Cargo.toml" if rel == "Cargo.toml.orig" else rel
            result = oid(b"blob", data)
            assert result == entries[path]["sha"], rel
        inventory.append({"path": rel, "sha256": hashlib.sha256(data).hexdigest(), "git_blob_or_note": result})
    assert len(inventory) == 14
print(json.dumps({"archive_sha256": expected_archive, "commit": expected_commit, "tree": expected_tree, "verified_git_tree_count": sum(x["type"] == "tree" for x in entries.values()) + 1, "files": inventory}, indent=2))
