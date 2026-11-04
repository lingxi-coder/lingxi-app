#!/usr/bin/env python3
"""Bind a few independently inspected consumer paths to immutable inputs."""
import hashlib
import json
import pathlib
import re
import subprocess
import tarfile

HERE = pathlib.Path(__file__).resolve().parent
REPO = HERE.parents[4]
CACHE = pathlib.Path.home() / ".cargo/registry/cache/index.crates.io-1949cf8c6b5b557f"
SRC = pathlib.Path.home() / ".cargo/registry/src/index.crates.io-1949cf8c6b5b557f"
HARNESS = pathlib.Path.home() / ".cargo/git/checkouts/harness-runtime-8d2473fb8c2fadbd/ae5a65b"
REV = "ae5a65b1f4816cd26a06d462573ebc9841f14f96"
LOCK = subprocess.check_output(["git", "show", "94a53f82c49017ab3e59e1ef5866a0e6f3c49b4f:Cargo.lock"], cwd=REPO).decode()

def sha(data):
    return hashlib.sha256(data).hexdigest()

result = {"registry": {}, "harness_revision": REV, "harness_files": {}}
for name, version, paths in [
    ("futures-channel", "0.3.32", ["src/mpsc/mod.rs"]),
    ("futures-util", "0.3.32", ["src/abortable.rs", "src/task/mod.rs"]),
    ("h2", "0.4.19", ["src/share.rs"]),
]:
    pkg = name + "-" + version
    package_blocks = LOCK.split("[[package]]")
    block = next(x for x in package_blocks if re.search(r'(?m)^name = "' + re.escape(name) + r'"$', x) and re.search(r'(?m)^version = "' + re.escape(version) + r'"$', x))
    expected = re.search(r'(?m)^checksum = "([0-9a-f]{64})"$', block).group(1)
    archive = CACHE / (pkg + ".crate")
    assert sha(archive.read_bytes()) == expected
    file_hashes = {}
    with tarfile.open(archive, "r:gz") as tf:
        for p in paths:
            contents = tf.extractfile(pkg + "/" + p).read()
            assert contents == (SRC / pkg / p).read_bytes()
            file_hashes[p] = sha(contents)
    result["registry"][pkg] = {"archive_sha256": expected, "files_sha256": file_hashes}

assert subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=HARNESS, text=True).strip() == REV
for path in ["crates/core/src/host/bridge.rs", "crates/core/src/host/filesystem.rs", "crates/core/src/host/http.rs", "crates/core/src/host/fusion.rs"]:
    committed = subprocess.check_output(["git", "show", REV + ":" + path], cwd=HARNESS)
    assert committed == (HARNESS / path).read_bytes()
    result["harness_files"][path] = sha(committed)
print(json.dumps(result, indent=2, sort_keys=True))
