#!/usr/bin/env python3
"""Compile the exact package, compare all adapters on pipes and a PTY."""

import os
from pathlib import Path
import pty
import select
import subprocess
import tarfile
import tempfile

HERE = Path(__file__).resolve().parent
ARCHIVE = HERE / "is_terminal_polyfill-1.70.2.crate"


def run(args, **kw):
    print("$", " ".join(map(str, args)), flush=True)
    return subprocess.run(args, check=True, text=True, **kw)


def main():
    with tempfile.TemporaryDirectory(prefix="is-terminal-review-") as td:
        temp = Path(td).resolve()
        # The exact archive is checked separately by verify-identity.py.
        with tarfile.open(ARCHIVE, "r:gz") as tar:
            for member in tar:
                assert member.isfile() and member.name.startswith("is_terminal_polyfill-1.70.2/")
                target = temp / member.name
                assert target.resolve().is_relative_to(temp)
                target.parent.mkdir(parents=True, exist_ok=True)
                target.write_bytes(tar.extractfile(member).read())
        package = temp / "is_terminal_polyfill-1.70.2"
        env = dict(os.environ, CARGO_HOME=str(temp / "cargo-home"), CARGO_TARGET_DIR=str(temp / "target"))
        run(["cargo", "test", "--locked", "--frozen", "--all-features"], cwd=package, env=env)
        source = (package / "src/lib.rs").read_text()
        for variant in ("exact", "negative-invert", "negative-false"):
            code = source
            if variant != "exact":
                original = "std::io::IsTerminal::is_terminal(self)"
                assert code.count(original) == 1
                replacement = f"!{original}" if variant == "negative-invert" else "false"
                code = code.replace(original, replacement)
            source_path = temp / f"{variant}.rs"
            library = temp / f"lib{variant.replace('-', '_')}.rlib"
            executable = temp / f"probe-{variant}"
            source_path.write_text(code)
            run(["rustc", "--edition=2021", "--crate-name", "is_terminal_polyfill", "--crate-type=lib", source_path, "-o", library])
            run(["rustc", "--edition=2021", HERE / "probe.rs", "--extern", f"is_terminal_polyfill={library}", "-o", executable])
            print(f"{variant}: pipe test", flush=True)
            result = subprocess.run([executable, "pipe", temp / f"{variant}-pipe"], input="", capture_output=True, text=True)
            assert (result.returncode == 0) == (variant != "negative-invert"), result
            print(result.stdout + result.stderr, end="")
            print(f"{variant}: pty test", flush=True)
            master, slave = pty.openpty()
            try:
                proc = subprocess.run([executable, "tty", temp / f"{variant}-tty"], stdin=slave, stdout=slave, stderr=slave, timeout=15)
                chunks = []
                # On macOS, read before closing the final slave fd; otherwise
                # buffered output can disappear after the child exits.
                while select.select([master], [], [], 0.25)[0]:
                    try:
                        chunk = os.read(master, 4096)
                    except OSError:
                        break
                    if not chunk:
                        break
                    chunks.append(chunk)
                assert (proc.returncode == 0) == (variant == "exact"), proc
                output = b"".join(chunks).decode().replace("\r\n", "\n")
                if variant == "exact":
                    for name in ("stdin", "stdin-lock", "stdout", "stdout-lock", "stderr", "stderr-lock"):
                        assert f"{name}: polyfill=true std=true" in output, output
                if variant == "negative-false":
                    assert "stdin differs from std" in output, output
                print(output, end="")
            finally:
                if slave != -1:
                    os.close(slave)
                os.close(master)
        print("PASS: exact package matches std on regular file, pipes, and PTY; inversion and false-on-PTY controls rejected")


if __name__ == "__main__":
    main()
