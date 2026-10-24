#!/usr/bin/env python3
"""Stage only the reviewed cross-platform fixture allowlists into the test bundle."""
import hashlib
import json
from pathlib import Path
import shutil
import sys
import tempfile

source = Path(__file__).resolve().parents[4] / "fixtures/local-apps/v2"
output = Path(sys.argv[1]).resolve()
if output.name != "UnifiedAppFixtures" or output == source or source.is_relative_to(output):
    raise SystemExit("output must be an independent UnifiedAppFixtures directory")
output.parent.mkdir(parents=True, exist_ok=True)
temporary = Path(tempfile.mkdtemp(prefix=".local-app-fixtures-", dir=output.parent))
try:
    inventory = []
    for name in ("static", "python", "node", "agent"):
        folder = source / name
        manifest = json.loads((folder / "lingxi-app.json").read_text())
        for relative in ["lingxi-app.json"] + manifest["package"]["files"]:
            original = folder / relative
            if original.is_symlink() or not original.is_file() or not original.resolve().is_relative_to(folder):
                raise ValueError("invalid fixture allowlist path")
            target = temporary / name / relative
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(original, target)
            target.chmod(0o700 if original.stat().st_mode & 0o111 else 0o600)
            inventory.append({"path": f"{name}/{relative}", "sha256": hashlib.sha256(target.read_bytes()).hexdigest()})
    (temporary / "inventory.json").write_text(json.dumps(inventory, indent=2))
    if output.exists():
        shutil.rmtree(output)
    temporary.rename(output)
finally:
    if temporary.exists():
        shutil.rmtree(temporary)
print(f"staged {len(inventory)} source-only local-app fixture files")
