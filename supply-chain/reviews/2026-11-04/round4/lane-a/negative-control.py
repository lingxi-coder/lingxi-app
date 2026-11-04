#!/usr/bin/env python3
"""Prove the representative test rejects a broken first-match exclusion."""
import os
from pathlib import Path
import subprocess
import tempfile

HERE = Path(__file__).resolve().parent
SOURCE = Path(os.environ.get(
    "CFG_IF_SOURCE_DIR",
    "/Users/luolingfeng/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cfg-if-1.0.4",
))
RUSTC = os.environ.get("CFG_IF_RUSTC", "/Users/luolingfeng/.cargo/bin/rustc")
original = (SOURCE / "src/lib.rs").read_text()
needle = "not(any( $( $($no)+ ),* ))"
assert original.count(needle) == 1
with tempfile.TemporaryDirectory(prefix="cfg-if-control-") as temp:
    root = Path(temp)
    changed = root / "lib.rs"
    changed.write_text(original.replace(needle, "all()"))
    lib = root / "libcfg_if.rlib"
    subprocess.run([RUSTC, "+1.94.0", "--edition=2018", "--crate-name=cfg_if",
                    "--crate-type=lib", str(changed), "-o", str(lib)], check=True)
    cmd = [RUSTC, "+1.94.0", "--edition=2018", str(HERE / "cfg-if-probe.rs"),
           "--extern", "cfg_if=" + str(lib), "--cfg", "flag_a", "--cfg", "flag_b",
           "-o", str(root / "probe")]
    result = subprocess.run(cmd, text=True, capture_output=True)
    if result.returncode == 0 or "defined multiple times" not in result.stderr:
        raise RuntimeError("first-match negative control was not rejected as expected")
    print("Changed previous-branch exclusion to unconditional all().")
    print("Probe rejected the changed macro when both first branches matched.")
    print("compiler exit:", result.returncode)
    print("duplicate-definition diagnostic:", "defined multiple times" in result.stderr)
