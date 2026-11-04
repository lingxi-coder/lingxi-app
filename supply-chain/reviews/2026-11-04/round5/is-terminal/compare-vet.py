#!/usr/bin/env python3
"""Verify the captured strict vet delta and the exact-only audit insertion."""

import hashlib
import json
from pathlib import Path
import subprocess
import tomllib

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[4]
sha = lambda b: hashlib.sha256(b).hexdigest()
context = json.loads((HERE / "before-context.json").read_text())
before = json.loads((HERE / "before-vet.json").read_text())
after = json.loads((HERE / "after-vet.json").read_text())
assert sha((HERE / "before-vet.json").read_bytes()) == context["output_sha256"]
assert before["conclusion"] == after["conclusion"] == "fail (vetting)"


def failures(report):
    rows = {(r["name"], r["version"], tuple(r["missing_criteria"])) for r in report["failures"]}
    assert len(rows) == len(report["failures"])
    return rows


prior, current = failures(before), failures(after)
only = ("is_terminal_polyfill", "1.70.2", ("safe-to-deploy",))
assert len(prior) == 819 and len(current) == 818
assert prior - current == {only} and not current - prior

for relpath, digest in context["input_sha256"].items():
    if relpath != "supply-chain/audits.toml":
        assert sha((ROOT / relpath).read_bytes()) == digest, relpath
base_bytes = subprocess.check_output(["git", "show", f"{context['head']}:supply-chain/audits.toml"], cwd=ROOT)
assert sha(base_bytes) == context["input_sha256"]["supply-chain/audits.toml"]
base = tomllib.loads(base_bytes.decode())
audit_bytes = (ROOT / "supply-chain/audits.toml").read_bytes()
candidate = tomllib.loads(audit_bytes.decode())
new = candidate["audits"].pop("is_terminal_polyfill")
assert candidate == base and len(new) == 1
record = new[0]
assert record["who"] == "Codex AI-assisted" and record["version"] == "1.70.2" and record["criteria"] == "safe-to-deploy"
for file in ("review.md", "root-cross-review.md"):
    assert sha((HERE / file).read_bytes()) in record["notes"]
print(json.dumps({"before_failures": len(prior), "after_failures": len(current), "removed": [{"name": only[0], "version": only[1], "missing_criteria": list(only[2])}], "added": [], "before_output_sha256": sha((HERE / "before-vet.json").read_bytes()), "after_output_sha256": sha((HERE / "after-vet.json").read_bytes()), "after_audits_sha256": sha(audit_bytes), "other_lock_and_store_inputs_unchanged": True, "only_audits_toml_change": "is_terminal_polyfill 1.70.2 exact full record", "overall_conclusion": after["conclusion"]}, indent=2))
