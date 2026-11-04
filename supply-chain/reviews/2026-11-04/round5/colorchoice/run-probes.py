#!/usr/bin/env python3
"""Build the exact crate and test its choice mapping and concurrent access."""

import os
from pathlib import Path
import subprocess
import tarfile
import tempfile

HERE = Path(__file__).resolve().parent


def run(args, **kwargs):
    print("$", " ".join(map(str, args)), flush=True)
    return subprocess.run(args, check=True, text=True, **kwargs)


def main():
    with tempfile.TemporaryDirectory(prefix="colorchoice-review-") as td:
        temp = Path(td).resolve()
        with tarfile.open(HERE / "colorchoice-1.0.5.crate", "r:gz") as tar:
            for member in tar:
                assert member.isfile() and member.name.startswith("colorchoice-1.0.5/")
                target = temp / member.name
                assert target.resolve().is_relative_to(temp)
                target.parent.mkdir(parents=True, exist_ok=True)
                target.write_bytes(tar.extractfile(member).read())
        package = temp / "colorchoice-1.0.5"
        env = dict(os.environ, CARGO_HOME=str(temp / "cargo-home"), CARGO_TARGET_DIR=str(temp / "target"))
        run(["cargo", "test", "--locked", "--frozen", "--all-features"], cwd=package, env=env)
        source = (package / "src/lib.rs").read_text()
        for variant in ("exact", "invalid-value", "alias-value"):
            code = source
            if variant != "exact":
                original = "ColorChoice::Always => 2,"
                assert code.count(original) == 1
                code = code.replace(original, "ColorChoice::Always => 99," if variant == "invalid-value" else "ColorChoice::Always => 1,")
            src = temp / f"{variant}.rs"
            lib = temp / f"lib{variant.replace('-', '_')}.rlib"
            executable = temp / f"probe-{variant}"
            src.write_text(code)
            run(["rustc", "--edition=2021", "--crate-name", "colorchoice", "--crate-type=lib", src, "-o", lib])
            run(["rustc", "--edition=2021", HERE / "probe.rs", "--extern", f"colorchoice={lib}", "-o", executable])
            result = subprocess.run([executable], capture_output=True, text=True, timeout=30)
            assert (result.returncode == 0) == (variant == "exact"), result
            print(f"{variant}: exit {result.returncode}")
            print(result.stdout + result.stderr, end="", flush=True)
        print("PASS: exact mapping and concurrent stress; invalid and aliased mappings rejected")


if __name__ == "__main__":
    main()
