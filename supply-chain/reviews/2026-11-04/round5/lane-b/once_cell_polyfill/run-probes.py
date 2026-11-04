#!/usr/bin/env python3
"""Compile the exact package and run host probes with two negative controls."""
import hashlib
import os
from pathlib import Path
import shlex
import subprocess
import sys
import tarfile
import tempfile

HERE = Path(__file__).resolve().parent
ARCHIVE = HERE / "once_cell_polyfill-1.70.2.crate"
EXPECTED = "384b8ab6d37215f3c5301a95a4accb5d64aa607f1fcb26a11b5303878451b4fe"


def run(command, env, *, expected_failure=None):
    print("+", shlex.join(str(x) for x in command), flush=True)
    result = subprocess.run(command, env=env, text=True, capture_output=True)
    print(result.stdout + result.stderr, end="", flush=True)
    if expected_failure is None:
        if result.returncode:
            raise RuntimeError(f"command failed: {result.returncode}")
    elif result.returncode == 0 or expected_failure not in result.stderr:
        raise RuntimeError(f"negative control did not fail at {expected_failure}")
    else:
        print(f"negative control caught: {expected_failure}")


def main():
    if hashlib.sha256(ARCHIVE.read_bytes()).hexdigest() != EXPECTED:
        raise RuntimeError("source archive hash mismatch")
    with tempfile.TemporaryDirectory(prefix="lingxi-once-cell-probes-") as temporary:
        tmp = Path(temporary)
        with tarfile.open(ARCHIVE, "r:gz") as archive:
            archive.extractall(tmp, filter="data")
        package = tmp / "once_cell_polyfill-1.70.2"
        source = package / "src/sync/mod.rs"
        original = source.read_text()
        env = dict(os.environ, CARGO_HOME=str(tmp / "cargo-home"), CARGO_INCREMENTAL="0",
                   CARGO_PROFILE_DEV_DEBUG="0", CARGO_PROFILE_TEST_DEBUG="0",
                   CARGO_BUILD_JOBS="2", RUST_BACKTRACE="0")
        target = tmp / "target"
        def cargo_command(action, *args):
            return ["cargo", "+1.94.0", action, "--manifest-path", str(package / "Cargo.toml"),
                    "--target-dir", str(target), "--locked", "--frozen", "--all-features", *args]
        run(cargo_command("test"), env)
        run(cargo_command("check", "--target", "x86_64-pc-windows-gnu"), env)
        run(cargo_command("build"), env)
        def compile_and_run(expected_failure=None):
            binary = tmp / "probe"
            run(["rustc", "+1.94.0", "--edition=2021", "-C", "debuginfo=0", str(HERE / "probe.rs"),
                 "--extern", f"once_cell_polyfill={target / 'debug/libonce_cell_polyfill.rlib'}",
                 "-o", str(binary)], env)
            run([str(binary)], env, expected_failure=expected_failure)
        compile_and_run()
        for expression, mutant, marker in (
            ("self.0.get()", "None", "GET_VALUE"),
            ("self.0.set(value)", "Err(value)", "INITIAL_SET"),
        ):
            if original.count(expression) != 1:
                raise RuntimeError(f"unexpected source for control: {expression}")
            source.write_text(original.replace(expression, mutant))
            run(cargo_command("build"), env)
            compile_and_run(marker)
        print("exact package host probes, Windows target check, and both controls passed")


if __name__ == "__main__":
    try:
        main()
    except (OSError, RuntimeError, subprocess.SubprocessError) as error:
        print(f"once_cell_polyfill probes failed: {error}", file=sys.stderr)
        sys.exit(1)
