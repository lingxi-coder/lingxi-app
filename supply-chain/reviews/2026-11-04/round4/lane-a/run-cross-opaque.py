#!/usr/bin/env python3
"""Compile directly from the checksum-matched opaque-debug archive."""
import hashlib
import os
from pathlib import Path
import subprocess
import tarfile
import tempfile

HERE = Path(__file__).resolve().parent
ARCHIVE = Path(os.environ.get(
    "OPAQUE_DEBUG_ARCHIVE",
    "/Users/luolingfeng/.cargo/registry/cache/index.crates.io-1949cf8c6b5b557f/opaque-debug-0.3.1.crate",
))
RUSTC = os.environ.get("OPAQUE_DEBUG_RUSTC", "/Users/luolingfeng/.cargo/bin/rustc")
assert hashlib.sha256(ARCHIVE.read_bytes()).hexdigest() == "c08d65885ee38876c4f86fa503fb49d7b507c2b62552df7c70b2fce627e06381"
with tempfile.TemporaryDirectory(prefix="opaque-cross-") as temp:
    root = Path(temp)
    with tarfile.open(ARCHIVE, "r:gz") as tar:
        source = tar.extractfile("opaque-debug-0.3.1/src/lib.rs").read()
    assert hashlib.sha256(source).hexdigest() == "14c4ed5ab5488b22b7f3c0f4d2aeb7eb7ac96113950ac5fc805d23454effa79e"
    (root / "lib.rs").write_bytes(source)
    lib = root / "libopaque_debug.rlib"
    subprocess.run([RUSTC, "+1.94.0", "--version"], check=True)
    subprocess.run([RUSTC, "+1.94.0", "--edition=2018", "--crate-name=opaque_debug",
                    "--crate-type=lib", str(root / "lib.rs"), "-o", str(lib)], check=True)
    binary = root / "probe"
    subprocess.run([RUSTC, "+1.94.0", "--edition=2018", str(HERE / "cross-opaque-probe.rs"),
                    "--extern", "opaque_debug=" + str(lib), "-o", str(binary)], check=True)
    subprocess.run([str(binary)], check=True)
