#!/usr/bin/env python3
"""Create a runner-only Cargo source override without changing the App pin."""
import hashlib
import json
import os
from pathlib import Path
import subprocess
import tomllib

workspace = Path(os.environ["GITHUB_WORKSPACE"]).resolve()
harness = Path(os.environ["RUNNER_TEMP"]).resolve() / "frame-receipt-harness"
expected = "02c0beac6b3e17632f44e7bff0b2c03cb6600b20"
actual = subprocess.check_output(["git", "-C", str(harness), "rev-parse", "HEAD"], text=True).strip()
if actual != expected:
    raise SystemExit(f"diagnostic Harness head mismatch: {actual} != {expected}")
manifest = tomllib.loads((workspace / "Cargo.toml").read_text())
deps = manifest["workspace"]["dependencies"]
repository = "https://github.com/lingxi-coder/harness-runtime.git"
selected = {spec.get("package", name) for name, spec in deps.items()
            if isinstance(spec, dict) and spec.get("git") == repository}
assert deps["harness-runtime"]["rev"] == "ff076c70babd85c1dd5f9cb04034b472d68e1797"
found = {}
for path in harness.rglob("Cargo.toml"):
    try:
        name = tomllib.loads(path.read_text()).get("package", {}).get("name")
    except (UnicodeDecodeError, tomllib.TOMLDecodeError):
        continue
    if isinstance(name, str):
        found[name] = path.parent
missing = sorted(selected - found.keys())
if missing:
    raise SystemExit(f"missing local Harness packages: {missing}")
config = workspace / ".cargo" / "config.toml"
if config.exists():
    raise SystemExit("refusing to replace existing Cargo config")
config.parent.mkdir(parents=True, exist_ok=True)
config.write_text('[patch."' + repository + '"]\n' + ''.join(
    f'"{name}" = {{ path = {json.dumps(str(found[name]))} }}\n' for name in sorted(selected)))
print(f"App manifest pin stays {deps['harness-runtime']['rev']}")
print(f"Diagnostic path override: {actual}; {len(selected)} direct packages")
# The source override requires a throwaway runner-local lock. Do not commit it.
lock = workspace / "Cargo.lock"
print(f"Original App lock SHA256: {hashlib.sha256(lock.read_bytes()).hexdigest()}")
print(f"Local patch config SHA256: {hashlib.sha256(config.read_bytes()).hexdigest()}")
subprocess.run(["cargo", "metadata", "--all-features", "--format-version=1"],
               cwd=workspace, stdout=subprocess.DEVNULL, check=True)
print(f"Runner-derived lock SHA256: {hashlib.sha256(lock.read_bytes()).hexdigest()}")
resolved = json.loads(subprocess.check_output(
    ["python3", "scripts/lib/runtime_source.py", "--json"], cwd=workspace, text=True))
if not resolved["development"] or Path(resolved["root"]).resolve() != harness:
    raise SystemExit(f"Cargo did not resolve the diagnostic Harness path: {resolved}")
subprocess.run(["git", "diff", "--exit-code", "--", "Cargo.toml"], cwd=workspace, check=True)
print("Verified runner-local path override and unchanged tracked Cargo.toml")
