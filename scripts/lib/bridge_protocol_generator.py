"""Run the syntax-based bridge DTO generator against a resolved Harness root."""
import os
from pathlib import Path
import subprocess
import tempfile
import tomllib

ROOT = Path(__file__).resolve().parents[2]
MANIFEST = ROOT / "tools/bridge-protocol-gen/Cargo.toml"
OUTPUT = ROOT / "packages/bridge-client/src/protocol.generated.ts"


def run_cargo(command, *, arguments=(), capture_output=False):
    # Cargo discovers .cargo/config from its current directory. The product's
    # development Harness patch must not leak into this independent tool's lock.
    # Keep the product toolchain even though Cargo runs from an isolated folder.
    environment = os.environ.copy()
    toolchain = tomllib.loads((ROOT / "rust-toolchain.toml").read_text())["toolchain"]["channel"]
    environment.setdefault("RUSTUP_TOOLCHAIN", toolchain)
    cargo = ["cargo", command, "--quiet", "--locked", "--manifest-path", str(MANIFEST)]
    if arguments:
        cargo.extend(["--", *arguments])
    with tempfile.TemporaryDirectory(prefix="lingxi-protocol-gen-") as cwd:
        return subprocess.run(
            cargo, cwd=cwd, env=environment, check=True,
            stdout=subprocess.PIPE if capture_output else None,
        )


def generated_bytes(runtime_root):
    return run_cargo("run", arguments=[str(runtime_root)], capture_output=True).stdout


def run_tests():
    run_cargo("test")


def check_or_write(runtime_root, *, check):
    generated = generated_bytes(runtime_root)
    if check:
        if not OUTPUT.is_file() or OUTPUT.read_bytes().replace(b"\r\n", b"\n") != generated:
            raise SystemExit(
                "CLIENT-PROTOCOL FAIL: generated TypeScript is stale; "
                "run python3 scripts/checks/generate-client-protocol.py"
            )
        print("CLIENT-PROTOCOL OK: generated TypeScript matches the resolved runtime")
    else:
        OUTPUT.write_bytes(generated)
        print(f"Generated {OUTPUT.relative_to(ROOT)} from {runtime_root}")
