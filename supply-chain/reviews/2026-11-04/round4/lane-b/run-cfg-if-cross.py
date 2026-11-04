#!/usr/bin/env python3
"""Independently test cfg-if's empty branch, hygiene and grouped cfg gate."""
import hashlib
import itertools
import os
from pathlib import Path
import subprocess
import tarfile
import tempfile

here = Path(__file__).resolve().parent
archive = Path(os.environ["CFG_IF_ARCHIVE"])
assert hashlib.sha256(archive.read_bytes()).hexdigest() == "9330f8b2ff13f34540b44e946ef35111825727b38d33286ef986142615121801"
with tarfile.open(archive) as package:
    src = package.extractfile("cfg-if-1.0.4/src/lib.rs").read().decode()

def run(cmd):
    subprocess.run(cmd, check=True)

with tempfile.TemporaryDirectory(prefix="cfg-if-cross-") as tmp:
    tmp = Path(tmp)
    source = tmp / "lib.rs"
    lib = tmp / "libcfg_if.rlib"
    source.write_text(src)
    run(["rustc", "+1.94.0", "--edition=2018", "--crate-name=cfg_if", "--crate-type=lib", str(source), "-o", str(lib)])
    for a, b in itertools.product((False, True), repeat=2):
        flags = [x for enabled, name in ((a, "flag_a"), (b, "flag_b")) if enabled for x in ("--cfg", name)]
        run(["rustc", "+1.94.0", "--edition=2018", str(here / "cfg-if-cross-probe.rs"), "--extern", "cfg_if=" + str(lib), "-o", str(tmp / "test"), *flags])
        run([str(tmp / "test")])
    # Negative control: remove the helper grouping so a single cfg attribute
    # applies only to the first token of a branch. The false-branch error must
    # then escape into the generated caller code.
    needle = "$crate::cfg_if! { @__temp_group $( $tokens )* }"
    assert src.count(needle) == 1
    source.write_text(src.replace(needle, "$( $tokens )*"))
    run(["rustc", "+1.94.0", "--edition=2018", "--crate-name=cfg_if", "--crate-type=lib", str(source), "-o", str(lib)])
    result = subprocess.run(["rustc", "+1.94.0", "--edition=2018", str(here / "cfg-if-cross-probe.rs"), "--extern", "cfg_if=" + str(lib), "-o", str(tmp / "bad")], capture_output=True, text=True)
    assert result.returncode != 0 and "false branch escaped its gate" in result.stderr, result.stderr
    print("negative control: removing grouped cfg gate exposed false branch (expected)")
