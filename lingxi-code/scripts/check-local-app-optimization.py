#!/usr/bin/env python3
"""Check the Local App Plugin optimization ledger.

This script reports a source-size proxy only. The loader-faithful byte check
and exact 5/6/7 agent-call assertions live in the engine-mobile/workflow Rust
tests, where ``MobileDiskSkillLoader::resolve_and_load`` and the real workflow
runtime are available. Keeping this distinction explicit prevents source-file
lengths from being mistaken for injected prompt bytes.
"""

from __future__ import annotations

import re
import io
import subprocess
import tarfile
import tempfile
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
PLUGIN = ROOT / "lingxi-code" / "plugins" / "lingxi-local-app"
BUILD = PLUGIN / "workflows" / "local-app-build.js"
BASELINE_REV = "21771b43"


def _split_frontmatter(source: str) -> tuple[str, str]:
    parts = source.split("---", 2)
    return (parts[1], parts[2]) if len(parts) == 3 else ("", source)


def _declared_skills(agent_source: str) -> list[str]:
    frontmatter, _ = _split_frontmatter(agent_source)
    skills_block = frontmatter.split("skills:", 1)[-1]
    return re.findall(r"^  - ([^\n]+)$", skills_block, re.MULTILINE)


def _loader_like_source_proxy(agent: str, plugin_root: Path) -> tuple[int, list[str]]:
    """Approximate loaded content without counting frontmatter/openai.yaml.

    The matching loader test is authoritative; this helper intentionally says
    *proxy* because Python does not execute the Rust loader.
    """
    agent_source = (plugin_root / "agents" / f"{agent}.md").read_text(encoding="utf-8")
    _, agent_body = _split_frontmatter(agent_source)
    skills = [skill.strip() for skill in _declared_skills(agent_source)]
    total = len(agent_body.encode())
    for skill in skills:
        skill_root = plugin_root / "skills" / skill
        if not skill_root.is_dir():
            raise SystemExit(f"{agent}: skill {skill!r} is not materialized")
        _, skill_body = _split_frontmatter((skill_root / "SKILL.md").read_text(encoding="utf-8"))
        total += len(skill_body.encode())
        for reference in sorted(skill_root.joinpath("references").rglob("*.md")):
            content = reference.read_text(encoding="utf-8")
            relative = reference.relative_to(skill_root).as_posix()
            total += len(f"\n\n## Bundled resource: {relative}\n\n{content}".encode())
    return total, skills


def _baseline_plugin_root() -> tempfile.TemporaryDirectory[str]:
    temp = tempfile.TemporaryDirectory(prefix="lingxi-local-app-baseline-")
    archive = subprocess.run(
        ["git", "archive", BASELINE_REV, "lingxi-code/plugins/lingxi-local-app/agents", "lingxi-code/plugins/lingxi-local-app/skills"],
        check=True,
        cwd=ROOT,
        stdout=subprocess.PIPE,
    ).stdout
    with tarfile.open(fileobj=io.BytesIO(archive)) as tar:
        tar.extractall(temp.name)
    return temp


def check_workflow_contract() -> str:
    source = BUILD.read_text(encoding="utf-8")
    required = [
        "create-preparer",
        "LocalAppContract",
        "LocalAppStageCreate",
        "LocalAppQaBegin",
        "LocalAppQaReadEvidence",
        "LocalAppQaFinalize",
        "verification_strategy=",
        "operator_result",
        "qa_review",
        "qa_finalize",
        "single bounded evidence resample",
        "quality === 'thorough'",
    ]
    missing = [needle for needle in required if needle not in source]
    if missing:
        raise SystemExit(f"workflow contract missing: {', '.join(missing)}")
    if "tester-finalize" in source:
        raise SystemExit("tester finalization must happen in the same agent pass")
    return "workflow/tests/plugin_workflow_scripts.rs::first_pass_agent_call_counts_are_exercised_for_each_quality_level"


def main() -> None:
    baseline_temp = _baseline_plugin_root()
    try:
        baseline_root = Path(baseline_temp.name) / "lingxi-code" / "plugins" / "lingxi-local-app"
        measurements = {}
        for agent in ("builder", "designer"):
            baseline, baseline_skills = _loader_like_source_proxy(agent, baseline_root)
            after, skills = _loader_like_source_proxy(agent, PLUGIN)
            limit = int(baseline * 0.60)
            if after > limit:
                raise SystemExit(f"{agent}: source proxy {after} exceeds 40% reduction limit {limit}")
            measurements[agent] = {
                "baseline_revision": BASELINE_REV,
                "baseline_source_proxy": baseline,
                "after_source_proxy": after,
                "reduction_percent": round((1 - after / baseline) * 100, 2),
                "baseline_skills": baseline_skills,
                "skills": skills,
            }
    finally:
        baseline_temp.cleanup()
    print({"guide_bytes_source_proxy": measurements, "first_pass_agent_calls_test": check_workflow_contract()})


if __name__ == "__main__":
    main()
