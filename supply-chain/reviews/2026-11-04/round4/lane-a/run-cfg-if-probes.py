#!/usr/bin/env python3
"""Compile exact cfg-if sources and exercise representative cfg combinations."""
import hashlib
import itertools
import os
from pathlib import Path
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parent
SOURCE = Path(os.environ.get(
    "CFG_IF_SOURCE_DIR",
    "/Users/luolingfeng/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cfg-if-1.0.4",
))
RUSTC = os.environ.get("CFG_IF_RUSTC", "/Users/luolingfeng/.cargo/bin/rustc")
EXPECTED = {
    "src/lib.rs": "c09723e0890d15810374009e96b20bf0eb2f65f383006516f34db36240835c85",
    "tests/xcrate.rs": "bcec148e69db81b1a618bdd6f96a25d9a0442e6ecc692fe28f1206d9bffc006a",
}


def run(command):
    print("+", " ".join(map(str, command)), flush=True)
    subprocess.run(command, check=True)


for name, checksum in EXPECTED.items():
    assert hashlib.sha256((SOURCE / name).read_bytes()).hexdigest() == checksum, name

run([RUSTC, "+1.94.0", "--version"])
with tempfile.TemporaryDirectory(prefix="cfg-if-audit-") as tmp:
    tmp = Path(tmp)
    lib = tmp / "libcfg_if.rlib"
    run([RUSTC, "+1.94.0", "--edition=2018", "--crate-name=cfg_if",
         "--crate-type=lib", SOURCE / "src/lib.rs", "-o", lib])
    unit = tmp / "unit"
    run([RUSTC, "+1.94.0", "--edition=2018", "--test", SOURCE / "src/lib.rs", "-o", unit])
    run([unit])
    integration = tmp / "xcrate"
    run([RUSTC, "+1.94.0", "--edition=2018", "--test", SOURCE / "tests/xcrate.rs",
         "--extern", "cfg_if=" + str(lib), "-o", integration])
    run([integration])
    for flags in itertools.product((False, True), repeat=3):
        binary = tmp / "probe"
        cfg = [part for flag, active in zip(("flag_a", "flag_b", "flag_c"), flags)
               if active for part in ("--cfg", flag)]
        run([RUSTC, "+1.94.0", "--edition=2018", ROOT / "cfg-if-probe.rs",
             "--extern", "cfg_if=" + str(lib), "-o", binary, *cfg])
        run([binary])
