# Staged graphify of claw-code — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a 5-stage progressive graphify pipeline that produces a single merged knowledge graph of `~/Projects/LingXi-Next/claw-code`.

**Architecture:** A manifest-driven orchestrator rsyncs claw-code files into a staging directory in additive stages; each stage invokes `/graphify` (first time) or `/graphify --update` (subsequent), with an audit gate between stages. A one-shot preprocessor splits the 127k-word `ROADMAP.md` into per-section files before stage 1. A final `--cluster-only` pass relabels communities on the merged graph.

**Tech Stack:** Python 3.11+ (stdlib only — no third-party deps), `graphify` (pip-installed), `rsync`, `bash`, `pytest` for tests.

**Spec reference:** `docs/superpowers/specs/2026-05-21-claw-code-staged-graphify-design.md`

**Note on commits:** The spec explicitly forbids new git repositories. Where this plan would normally say "commit", substitute "append an entry to `staging-log.md`". The orchestrator directory is intentionally outside any tracked tree.

---

## File structure

All new files live under `~/Projects/LingXi-Next/claw-code-graphify-orchestrator/` (created in Task 2). Staging output lives at `~/Projects/LingXi-Next/claw-code-graphify/` (auto-created by rsync in Task 9).

```
claw-code-graphify-orchestrator/
├── README.md                       # 5-line pointer to the spec + plan
├── staging-log.md                  # append-only journal (one entry per task/stage)
├── generate_manifests.py           # builds stage1.manifest … stage5.manifest
├── split_roadmap.py                # ROADMAP.md → ROADMAP__<slug>.md files
├── audit_gate.py                   # post-stage go/no-go checker
├── run_stage.sh                    # ties rsync + /graphify + audit + log
├── manifests/                      # generated, gitignored conceptually
│   ├── stage1.manifest
│   ├── …
│   └── stage5.manifest
└── tests/
    ├── conftest.py                 # pytest fixtures for fake claw-code trees
    ├── test_generate_manifests.py
    ├── test_split_roadmap.py
    └── test_audit_gate.py
```

Each Python module has one responsibility and is independently testable. The shell orchestrator (`run_stage.sh`) is a thin wrapper that calls them in sequence.

---

## Task 1: Probe environment and locate graphify's semantic cache

**Files:**
- Create: `~/Projects/LingXi-Next/claw-code-graphify-orchestrator/` (directory)
- Create: `~/Projects/LingXi-Next/claw-code-graphify-orchestrator/staging-log.md`

This resolves the spec §4 ambiguity about whether the cache is project-local or global. We need to know before Task 9.

- [ ] **Step 1: Create the orchestrator directory and seed the log**

```bash
mkdir -p ~/Projects/LingXi-Next/claw-code-graphify-orchestrator
cd ~/Projects/LingXi-Next/claw-code-graphify-orchestrator
cat > staging-log.md <<'EOF'
# claw-code staged graphify — execution log

Append-only. One section per completed task or stage.

EOF
```

- [ ] **Step 2: Verify graphify is importable (install if missing)**

```bash
python3 -c "import graphify; print(graphify.__file__)" \
  || pip install graphifyy -q --break-system-packages
python3 -c "import graphify; print('OK:', graphify.__file__)"
```

Expected: prints the path to graphify's `__init__.py`. If install was needed, the second line confirms it now imports.

- [ ] **Step 3: Build a 1-file probe corpus and locate the cache**

```bash
mkdir -p /tmp/graphify-probe
cat > /tmp/graphify-probe/hello.md <<'EOF'
# Hello
This is a probe document about an AuthModule that depends on a Database.
EOF

# Find any pre-existing cache locations
find ~/ -type d -name '.graphify_semantic_cache' 2>/dev/null | head -5 > /tmp/probe-before.txt
find ~/.cache -type d -name 'graphify*' 2>/dev/null >> /tmp/probe-before.txt
echo '--- before ---' && cat /tmp/probe-before.txt
```

- [ ] **Step 4: Run a minimal extraction and re-scan**

Run inside Claude Code:

```
/graphify /tmp/graphify-probe
```

After it completes, scan again:

```bash
find ~/ -type d -name '.graphify_semantic_cache' 2>/dev/null | head -5 > /tmp/probe-after.txt
find ~/.cache -type d -name 'graphify*' 2>/dev/null >> /tmp/probe-after.txt
echo '--- after ---' && cat /tmp/probe-after.txt
diff /tmp/probe-before.txt /tmp/probe-after.txt || true
```

Expected: a new `.graphify_semantic_cache/` directory appears either inside `/tmp/graphify-probe/graphify-out/` (project-local) or under `~/.cache/graphify/` (global).

- [ ] **Step 5: Log the finding and clean up**

Append to `staging-log.md`:

```bash
cd ~/Projects/LingXi-Next/claw-code-graphify-orchestrator
cat >> staging-log.md <<EOF

## Task 1 — environment probe ($(date '+%Y-%m-%d %H:%M'))

- graphify installed at: $(python3 -c 'import graphify; print(graphify.__file__)')
- semantic cache location: <PASTE the path from probe-after diff>
- cache scope: <local | global>

EOF

rm -rf /tmp/graphify-probe /tmp/probe-before.txt /tmp/probe-after.txt
```

---

## Task 2: Scaffold orchestrator directory and pytest setup

**Files:**
- Create: `claw-code-graphify-orchestrator/README.md`
- Create: `claw-code-graphify-orchestrator/manifests/` (empty)
- Create: `claw-code-graphify-orchestrator/tests/__init__.py`
- Create: `claw-code-graphify-orchestrator/tests/conftest.py`

- [ ] **Step 1: Make subdirectories and seed README**

```bash
cd ~/Projects/LingXi-Next/claw-code-graphify-orchestrator
mkdir -p manifests tests
cat > README.md <<'EOF'
# claw-code staged graphify orchestrator

Implementation of the staged graphify pipeline described in
`../docs/superpowers/specs/2026-05-21-claw-code-staged-graphify-design.md`
following the plan at
`../docs/superpowers/plans/2026-05-21-claw-code-staged-graphify.md`.

Run stages with `./run_stage.sh <1|2|3|4|5>`.
Final graph appears in `../claw-code-graphify/graphify-out/`.
EOF
touch tests/__init__.py
```

- [ ] **Step 2: Write the conftest with a fake claw-code tree fixture**

Create `tests/conftest.py`:

```python
import pytest
from pathlib import Path


@pytest.fixture
def fake_claw_code(tmp_path: Path) -> Path:
    """Minimal claw-code-shaped tree for manifest-generator tests."""
    root = tmp_path / "claw-code"
    layout = {
        "README.md": "# claw-code",
        "ROADMAP.md": "# Roadmap\n## Q1\nThings.\n## Q2\nMore.",
        "prd.json": "{}",
        "progress.txt": "in progress",
        "Cargo.lock": "should be excluded",
        "docs/g001.md": "# spec",
        "docs/MODEL_COMPATIBILITY.md": "# compat",
        "rust/README.md": "# rust workspace",
        "rust/Cargo.toml": "[workspace]",
        "rust/crates/api/Cargo.toml": "[package]\nname = 'api'",
        "rust/crates/api/src/lib.rs": "pub fn hi() {}",
        "rust/crates/runtime/src/lib.rs": "pub fn run() {}",
        "rust/.claude/sessions/session-1.json": "should be excluded",
        "src/cli/main.py": "def main(): pass",
        "src/utils/strings.py": "def fmt(): pass",
        "src/schemas/user.json": "{}",
        "scripts/build.sh": "#!/bin/sh\necho build",
        "tests/integration.py": "def test_it(): pass",
        "assets/logo.png": "",  # should be excluded
        ".omx/cc2/board.json": "huge generated",  # excluded
        ".omx/cc2/board.md": "huge generated",  # excluded
        ".omx/cc2/render_board_md.py": "def render(): pass",  # S5 keep
        ".omx/ultragoal/spec.md": "# ultragoal",  # S5 keep
        ".port_sessions/sess1.log": "captured",  # excluded
    }
    for rel, content in layout.items():
        p = root / rel
        p.parent.mkdir(parents=True, exist_ok=True)
        p.write_text(content)
    return root
```

- [ ] **Step 3: Verify pytest collects the empty test tree**

```bash
cd ~/Projects/LingXi-Next/claw-code-graphify-orchestrator
python3 -m pytest tests/ -v
```

Expected: `no tests ran` — confirms pytest is wired up and the conftest imports cleanly.

- [ ] **Step 4: Log scaffold completion**

```bash
cat >> staging-log.md <<EOF

## Task 2 — scaffold ($(date '+%Y-%m-%d %H:%M'))

- Created orchestrator/, manifests/, tests/
- pytest collection passes with empty test suite
EOF
```

---

## Task 3: Manifest generator — inclusion rules (TDD)

**Files:**
- Create: `claw-code-graphify-orchestrator/tests/test_generate_manifests.py`
- Create: `claw-code-graphify-orchestrator/generate_manifests.py`

- [ ] **Step 1: Write a failing test for stage 1 inclusion**

Create `tests/test_generate_manifests.py`:

```python
from pathlib import Path
from generate_manifests import build_manifest


def test_stage1_includes_root_markdown_and_docs(fake_claw_code: Path):
    files = build_manifest(stage=1, root=fake_claw_code)
    assert "README.md" in files
    assert "ROADMAP.md" in files
    assert "prd.json" in files
    assert "progress.txt" in files
    assert "docs/g001.md" in files
    assert "rust/README.md" in files
    assert "rust/Cargo.toml" not in files  # belongs to stage 2
    assert "src/cli/main.py" not in files
```

- [ ] **Step 2: Run the test — confirm it fails because the module does not exist**

```bash
cd ~/Projects/LingXi-Next/claw-code-graphify-orchestrator
python3 -m pytest tests/test_generate_manifests.py::test_stage1_includes_root_markdown_and_docs -v
```

Expected: `ModuleNotFoundError: No module named 'generate_manifests'`.

- [ ] **Step 3: Implement the minimal generator with stage 1 only**

Create `generate_manifests.py`:

```python
"""Build per-stage file manifests for claw-code staged graphify.

Each manifest is a newline-separated list of paths relative to the claw-code
root, consumed by `rsync --files-from=`.
"""
from __future__ import annotations
import sys
from pathlib import Path

# Per-stage inclusion globs (paths relative to claw-code root).
# Order within a list does not matter; deduplication happens later.
STAGE_INCLUDES: dict[int, list[str]] = {
    1: [
        "*.md", "*.txt", "*.json",
        "docs/**/*.md",
        "rust/*.md", "rust/*.json",
    ],
    2: [
        "rust/crates/**/*.rs",
        "rust/**/Cargo.toml",
        "rust/scripts/**/*",
    ],
    3: [
        "src/**/*.py",
        "src/**/*.json",
    ],
    4: [
        "scripts/**/*",
        "tests/**/*",
    ],
    5: [
        ".omx/cc2/render_board_md.py",
        ".omx/cc2/validate_*.py",
        ".omx/ultragoal/**/*",
    ],
}


def build_manifest(stage: int, root: Path) -> list[str]:
    """Return sorted relative paths included in `stage` under `root`."""
    if stage not in STAGE_INCLUDES:
        raise ValueError(f"unknown stage {stage}")
    matched: set[str] = set()
    for pattern in STAGE_INCLUDES[stage]:
        for match in root.glob(pattern):
            if match.is_file():
                matched.add(str(match.relative_to(root)))
    return sorted(matched)


def main(root: str, out_dir: str) -> None:
    root_path = Path(root)
    out_path = Path(out_dir)
    out_path.mkdir(parents=True, exist_ok=True)
    for stage in STAGE_INCLUDES:
        files = build_manifest(stage, root_path)
        manifest = out_path / f"stage{stage}.manifest"
        manifest.write_text("\n".join(files) + ("\n" if files else ""))
        print(f"stage{stage}: {len(files)} files -> {manifest}")


if __name__ == "__main__":
    main(sys.argv[1], sys.argv[2])
```

- [ ] **Step 4: Run the test — confirm it passes**

```bash
python3 -m pytest tests/test_generate_manifests.py::test_stage1_includes_root_markdown_and_docs -v
```

Expected: 1 passed.

- [ ] **Step 5: Add tests for stages 2–5**

Append to `tests/test_generate_manifests.py`:

```python
def test_stage2_includes_rust_workspace(fake_claw_code: Path):
    files = build_manifest(stage=2, root=fake_claw_code)
    assert "rust/crates/api/src/lib.rs" in files
    assert "rust/crates/runtime/src/lib.rs" in files
    assert "rust/Cargo.toml" in files
    assert "rust/crates/api/Cargo.toml" in files
    assert "src/cli/main.py" not in files


def test_stage3_includes_python_source(fake_claw_code: Path):
    files = build_manifest(stage=3, root=fake_claw_code)
    assert "src/cli/main.py" in files
    assert "src/utils/strings.py" in files
    assert "src/schemas/user.json" in files
    assert "rust/crates/api/src/lib.rs" not in files


def test_stage4_includes_scripts_and_tests(fake_claw_code: Path):
    files = build_manifest(stage=4, root=fake_claw_code)
    assert "scripts/build.sh" in files
    assert "tests/integration.py" in files


def test_stage5_includes_omx_tooling(fake_claw_code: Path):
    files = build_manifest(stage=5, root=fake_claw_code)
    assert ".omx/cc2/render_board_md.py" in files
    assert ".omx/ultragoal/spec.md" in files
```

- [ ] **Step 6: Run all tests — confirm they pass**

```bash
python3 -m pytest tests/test_generate_manifests.py -v
```

Expected: 5 passed.

- [ ] **Step 7: Log inclusion-rules milestone**

```bash
cat >> staging-log.md <<EOF

## Task 3 — manifest generator (inclusion) ($(date '+%Y-%m-%d %H:%M'))

- 5 tests passing
- All stage inclusion patterns implemented
EOF
```

---

## Task 4: Manifest generator — exclusion rules (TDD)

**Files:**
- Modify: `claw-code-graphify-orchestrator/generate_manifests.py`
- Modify: `claw-code-graphify-orchestrator/tests/test_generate_manifests.py`

- [ ] **Step 1: Write failing tests for exclusion rules**

Append to `tests/test_generate_manifests.py`:

```python
def test_omx_board_artifacts_are_excluded(fake_claw_code: Path):
    """`.omx/cc2/board.{json,md}` are auto-generated noise."""
    all_files = []
    for stage in (1, 2, 3, 4, 5):
        all_files.extend(build_manifest(stage, fake_claw_code))
    assert ".omx/cc2/board.json" not in all_files
    assert ".omx/cc2/board.md" not in all_files


def test_assets_and_sessions_are_excluded(fake_claw_code: Path):
    all_files = []
    for stage in (1, 2, 3, 4, 5):
        all_files.extend(build_manifest(stage, fake_claw_code))
    assert not any(f.startswith("assets/") for f in all_files)
    assert not any(f.startswith(".port_sessions/") for f in all_files)
    assert not any(f.startswith("rust/.claude/sessions/") for f in all_files)


def test_cargo_lock_is_excluded(fake_claw_code: Path):
    all_files = []
    for stage in (1, 2, 3, 4, 5):
        all_files.extend(build_manifest(stage, fake_claw_code))
    assert "Cargo.lock" not in all_files
    assert "rust/Cargo.lock" not in all_files
```

- [ ] **Step 2: Run the new tests — confirm exclusion tests fail**

```bash
python3 -m pytest tests/test_generate_manifests.py -v
```

Expected: existing tests still pass; new exclusion tests fail because (a) `.omx/cc2/board.json` matches `*.json` from stage 1's root glob and (b) `Cargo.lock` matches stage 1's `*.json`/`*.txt`/`*.md` no — actually `Cargo.lock` doesn't match those, so that test may pass by accident. Read the failure output carefully before moving on.

- [ ] **Step 3: Add exclusion filter to the generator**

Modify `generate_manifests.py`. Add at top of file (after `STAGE_INCLUDES`):

```python
# Excludes apply globally to every stage. Listed as glob patterns.
GLOBAL_EXCLUDES: list[str] = [
    ".omx/cc2/board.json",
    ".omx/cc2/board.md",
    ".port_sessions/**",
    "assets/**",
    "rust/.claude/sessions/**",
    "Cargo.lock",
    "rust/Cargo.lock",
    ".git/**",
    ".github/**",
]


def _is_excluded(rel_path: str, root: Path) -> bool:
    from fnmatch import fnmatch
    for pattern in GLOBAL_EXCLUDES:
        # fnmatch handles ** poorly, so check both literal-prefix and fnmatch.
        if "**" in pattern:
            prefix = pattern.split("**")[0]
            if rel_path.startswith(prefix):
                return True
        elif fnmatch(rel_path, pattern):
            return True
    return False
```

Replace the body of `build_manifest` with:

```python
def build_manifest(stage: int, root: Path) -> list[str]:
    if stage not in STAGE_INCLUDES:
        raise ValueError(f"unknown stage {stage}")
    matched: set[str] = set()
    for pattern in STAGE_INCLUDES[stage]:
        for match in root.glob(pattern):
            if not match.is_file():
                continue
            rel = str(match.relative_to(root))
            if _is_excluded(rel, root):
                continue
            matched.add(rel)
    return sorted(matched)
```

- [ ] **Step 4: Run all tests — confirm everything passes**

```bash
python3 -m pytest tests/test_generate_manifests.py -v
```

Expected: 8 passed.

- [ ] **Step 5: Log exclusion-rules milestone**

```bash
cat >> staging-log.md <<EOF

## Task 4 — manifest generator (exclusion) ($(date '+%Y-%m-%d %H:%M'))

- 8 tests passing
- Global excludes: .omx board artifacts, assets/, .port_sessions/, sessions json, lockfiles, .git/.github
EOF
```

---

## Task 5: Generate the real claw-code manifests

**Files:**
- Create: `claw-code-graphify-orchestrator/manifests/stage1.manifest` … `stage5.manifest`

- [ ] **Step 1: Run the generator against the real claw-code**

```bash
cd ~/Projects/LingXi-Next/claw-code-graphify-orchestrator
python3 generate_manifests.py \
    ~/Projects/LingXi-Next/claw-code \
    ./manifests
```

Expected output (counts should be in these rough ranges, per spec §1):

```
stage1: 30–45 files -> manifests/stage1.manifest
stage2: 90–120 files -> manifests/stage2.manifest
stage3: 90–110 files -> manifests/stage3.manifest
stage4: 5–10 files -> manifests/stage4.manifest
stage5: 3–8 files -> manifests/stage5.manifest
```

- [ ] **Step 2: Spot-check each manifest**

```bash
echo '=== stage1 sample ==='
head -10 manifests/stage1.manifest
echo '=== stage2 sample ==='
head -10 manifests/stage2.manifest
echo '=== checking exclusions ==='
grep -l 'board.json\|board.md\|Cargo.lock\|port_sessions' manifests/*.manifest && echo 'FAIL: found excluded file' || echo 'PASS: no excluded files'
```

Expected: `PASS: no excluded files`. The `head` outputs show plausible file paths.

- [ ] **Step 3: Log manifest counts**

```bash
cat >> staging-log.md <<EOF

## Task 5 — real manifests generated ($(date '+%Y-%m-%d %H:%M'))

$(for s in 1 2 3 4 5; do echo "- stage$s: $(wc -l < manifests/stage$s.manifest | tr -d ' ') files"; done)
EOF
```

---

## Task 6: ROADMAP splitter (TDD)

**Files:**
- Create: `claw-code-graphify-orchestrator/tests/test_split_roadmap.py`
- Create: `claw-code-graphify-orchestrator/split_roadmap.py`

- [ ] **Step 1: Write failing test for H1 splitting**

Create `tests/test_split_roadmap.py`:

```python
from pathlib import Path
from split_roadmap import split


def _read(p: Path) -> str:
    return p.read_text()


def test_h1_split_produces_one_file_per_section(tmp_path: Path):
    src = tmp_path / "ROADMAP.md"
    src.write_text(
        "# Vision\nThe long term plan.\n\n"
        "# Quarter 1\nShip MVP.\n\n"
        "# Quarter 2\nIterate.\n"
    )
    outputs = split(src, out_dir=tmp_path, max_words=15000)
    assert len(outputs) == 3
    slugs = sorted(p.stem.replace("ROADMAP__", "") for p in outputs)
    assert slugs == ["quarter-1", "quarter-2", "vision"]


def test_split_files_include_frontmatter(tmp_path: Path):
    src = tmp_path / "ROADMAP.md"
    src.write_text("# Vision\nbody\n")
    outputs = split(src, out_dir=tmp_path, max_words=15000)
    text = _read(outputs[0])
    assert text.startswith("---\n")
    assert "roadmap_section: vision" in text
    assert "source_url: file://ROADMAP.md" in text
    assert "---\n# Vision" in text


def test_oversized_section_is_h2_split(tmp_path: Path):
    """If a single H1 section exceeds max_words, fall back to H2 chunking."""
    big_body = " ".join(["word"] * 20)  # 20-word body per H2
    src = tmp_path / "ROADMAP.md"
    src.write_text(
        "# Huge\n"
        f"## Part A\n{big_body}\n"
        f"## Part B\n{big_body}\n"
        f"## Part C\n{big_body}\n"
    )
    outputs = split(src, out_dir=tmp_path, max_words=30)  # forces H2 split
    # Three H2 sub-sections become three files.
    slugs = sorted(p.stem.replace("ROADMAP__", "") for p in outputs)
    assert slugs == ["huge--part-a", "huge--part-b", "huge--part-c"]
```

- [ ] **Step 2: Run tests — confirm failure**

```bash
python3 -m pytest tests/test_split_roadmap.py -v
```

Expected: `ModuleNotFoundError: No module named 'split_roadmap'`.

- [ ] **Step 3: Implement the splitter**

Create `split_roadmap.py`:

```python
"""Split ROADMAP.md by H1 (and H2 fallback) into per-section files.

Each output gets YAML frontmatter so graphify's semantic extractor preserves
the "all slices come from one ROADMAP" relationship via node metadata copy
(see graphify SKILL.md line 247).
"""
from __future__ import annotations
import re
import sys
from pathlib import Path

H1_RE = re.compile(r"^# +(.+?)\s*$", re.MULTILINE)
H2_RE = re.compile(r"^## +(.+?)\s*$", re.MULTILINE)


def _slugify(title: str) -> str:
    s = title.lower().strip()
    s = re.sub(r"[^a-z0-9]+", "-", s)
    return s.strip("-") or "untitled"


def _word_count(text: str) -> int:
    return len(text.split())


def _split_at(text: str, header_re: re.Pattern[str]) -> list[tuple[str, str]]:
    """Return [(title, body_including_header), ...] split at each header match.

    Content before the first header is dropped (frontmatter or stray prose).
    """
    matches = list(header_re.finditer(text))
    if not matches:
        return []
    sections: list[tuple[str, str]] = []
    for i, m in enumerate(matches):
        title = m.group(1).strip()
        start = m.start()
        end = matches[i + 1].start() if i + 1 < len(matches) else len(text)
        sections.append((title, text[start:end]))
    return sections


def _write_section(
    out_dir: Path,
    slug: str,
    body: str,
    source_name: str,
) -> Path:
    out = out_dir / f"ROADMAP__{slug}.md"
    content = (
        "---\n"
        f"roadmap_section: {slug}\n"
        f"source_url: file://{source_name}\n"
        "---\n"
        + body
    )
    out.write_text(content)
    return out


def split(src: Path, out_dir: Path, max_words: int = 15000) -> list[Path]:
    """Split `src` into ROADMAP__<slug>.md files under `out_dir`.

    Strategy: split by H1. Any H1 section larger than max_words is further
    split into per-H2-subsection files with slug "h1slug--h2slug".

    Returns the list of files written.
    """
    text = src.read_text()
    out_dir.mkdir(parents=True, exist_ok=True)
    h1_sections = _split_at(text, H1_RE)
    if not h1_sections:
        # No H1 — write the whole file as one slice keyed by filename stem.
        return [_write_section(out_dir, _slugify(src.stem), text, src.name)]

    written: list[Path] = []
    for h1_title, h1_body in h1_sections:
        h1_slug = _slugify(h1_title)
        if _word_count(h1_body) <= max_words:
            written.append(_write_section(out_dir, h1_slug, h1_body, src.name))
            continue
        # Fall back to H2 split inside this oversized H1.
        h2_sections = _split_at(h1_body, H2_RE)
        if not h2_sections:
            # No H2 to split on — write as one big slice anyway.
            written.append(_write_section(out_dir, h1_slug, h1_body, src.name))
            continue
        for h2_title, h2_body in h2_sections:
            slug = f"{h1_slug}--{_slugify(h2_title)}"
            written.append(_write_section(out_dir, slug, h2_body, src.name))
    return written


def main(src: str, out_dir: str) -> None:
    files = split(Path(src), Path(out_dir))
    print(f"wrote {len(files)} sections to {out_dir}")
    for f in files:
        print(f"  {f.name}")


if __name__ == "__main__":
    main(sys.argv[1], sys.argv[2])
```

- [ ] **Step 4: Run tests — confirm all pass**

```bash
python3 -m pytest tests/test_split_roadmap.py -v
```

Expected: 3 passed.

- [ ] **Step 5: Smoke-test against the real ROADMAP**

```bash
python3 split_roadmap.py \
    ~/Projects/LingXi-Next/claw-code/ROADMAP.md \
    /tmp/roadmap-split-probe

ls /tmp/roadmap-split-probe | head -20
wc -w /tmp/roadmap-split-probe/*.md | tail -5
```

Expected: 15–40 files named `ROADMAP__*.md`, none significantly over 15k words.

- [ ] **Step 6: Clean up the smoke test and log**

```bash
rm -rf /tmp/roadmap-split-probe
cat >> staging-log.md <<EOF

## Task 6 — ROADMAP splitter ($(date '+%Y-%m-%d %H:%M'))

- 3 tests passing
- Smoke run against real ROADMAP.md produced plausible chunk sizes
EOF
```

---

## Task 7: Audit gate (TDD)

**Files:**
- Create: `claw-code-graphify-orchestrator/tests/test_audit_gate.py`
- Create: `claw-code-graphify-orchestrator/audit_gate.py`

The audit gate evaluates a completed stage by reading the post-stage `graph.json` (and optionally a pre-stage snapshot for delta computation) plus the post-stage `GRAPH_REPORT.md`. It exits 0 (pass), 1 (warn — proceed with caution), or 2 (fail — stop).

- [ ] **Step 1: Write failing tests for the gate logic**

Create `tests/test_audit_gate.py`:

```python
import json
from pathlib import Path
from audit_gate import evaluate, GateResult


def _write_graph(p: Path, node_count: int, edge_count: int, ambig_share: float):
    edges = []
    n_ambig = int(edge_count * ambig_share)
    n_extracted = edge_count - n_ambig
    for i in range(n_extracted):
        edges.append({"source": "a", "target": "b", "confidence": "EXTRACTED"})
    for i in range(n_ambig):
        edges.append({"source": "a", "target": "b", "confidence": "AMBIGUOUS"})
    p.write_text(json.dumps({
        "nodes": [{"id": str(i)} for i in range(node_count)],
        "links": edges,
    }))


def test_gate_passes_when_growth_and_quality_are_healthy(tmp_path: Path):
    pre = tmp_path / "pre.json"
    post = tmp_path / "post.json"
    _write_graph(pre, node_count=10, edge_count=20, ambig_share=0.1)
    _write_graph(post, node_count=60, edge_count=120, ambig_share=0.1)
    result = evaluate(
        pre_graph=pre, post_graph=post,
        stage_file_count=20,
        failed_chunks=0, total_chunks=5,
    )
    assert result.status == "pass"


def test_gate_fails_when_growth_is_too_small(tmp_path: Path):
    pre = tmp_path / "pre.json"
    post = tmp_path / "post.json"
    _write_graph(pre, node_count=10, edge_count=20, ambig_share=0.1)
    _write_graph(post, node_count=15, edge_count=22, ambig_share=0.1)
    # Stage claims 20 files; threshold is files * 2 = 40 new nodes+edges.
    result = evaluate(
        pre_graph=pre, post_graph=post,
        stage_file_count=20,
        failed_chunks=0, total_chunks=5,
    )
    assert result.status == "fail"
    assert "growth" in result.reason.lower()


def test_gate_fails_on_chunk_failure_rate(tmp_path: Path):
    pre = tmp_path / "pre.json"
    post = tmp_path / "post.json"
    _write_graph(pre, node_count=10, edge_count=20, ambig_share=0.1)
    _write_graph(post, node_count=60, edge_count=120, ambig_share=0.1)
    result = evaluate(
        pre_graph=pre, post_graph=post,
        stage_file_count=20,
        failed_chunks=2, total_chunks=5,  # 40% failure > 20% threshold
    )
    assert result.status == "fail"
    assert "chunk" in result.reason.lower()


def test_gate_warns_on_high_ambiguous_share(tmp_path: Path):
    pre = tmp_path / "pre.json"
    post = tmp_path / "post.json"
    _write_graph(pre, node_count=10, edge_count=20, ambig_share=0.1)
    _write_graph(post, node_count=60, edge_count=120, ambig_share=0.4)
    result = evaluate(
        pre_graph=pre, post_graph=post,
        stage_file_count=20,
        failed_chunks=0, total_chunks=5,
    )
    assert result.status == "warn"
    assert "ambiguous" in result.reason.lower()
```

- [ ] **Step 2: Run tests — confirm failure**

```bash
python3 -m pytest tests/test_audit_gate.py -v
```

Expected: `ModuleNotFoundError: No module named 'audit_gate'`.

- [ ] **Step 3: Implement the audit gate**

Create `audit_gate.py`:

```python
"""Post-stage go/no-go evaluation for the staged graphify pipeline.

Thresholds (from spec §3):
- Growth: new nodes + new edges must exceed stage_file_count * 2 (FAIL otherwise)
- Failed chunks: ≤ 20% of total chunks (FAIL otherwise; graphify hard-stops at 50%)
- AMBIGUOUS share: < 30% of post-stage edges (WARN otherwise; informational)
"""
from __future__ import annotations
import json
import sys
from dataclasses import dataclass
from pathlib import Path


GROWTH_MULTIPLIER = 2
MAX_FAILED_CHUNK_RATIO = 0.20
MAX_AMBIGUOUS_SHARE = 0.30


@dataclass
class GateResult:
    status: str          # "pass" | "warn" | "fail"
    reason: str
    metrics: dict


def _load_graph(p: Path) -> dict:
    return json.loads(p.read_text())


def _count_ambiguous(graph: dict) -> int:
    edges = graph.get("links") or graph.get("edges") or []
    return sum(1 for e in edges if e.get("confidence") == "AMBIGUOUS")


def evaluate(
    pre_graph: Path | None,
    post_graph: Path,
    stage_file_count: int,
    failed_chunks: int,
    total_chunks: int,
) -> GateResult:
    post = _load_graph(post_graph)
    post_nodes = len(post.get("nodes", []))
    post_edges = len(post.get("links") or post.get("edges") or [])

    if pre_graph and pre_graph.exists():
        pre = _load_graph(pre_graph)
        pre_nodes = len(pre.get("nodes", []))
        pre_edges = len(pre.get("links") or pre.get("edges") or [])
    else:
        pre_nodes = pre_edges = 0

    delta = (post_nodes - pre_nodes) + (post_edges - pre_edges)
    min_growth = stage_file_count * GROWTH_MULTIPLIER

    chunk_fail_ratio = (failed_chunks / total_chunks) if total_chunks else 0.0
    ambig_share = (_count_ambiguous(post) / post_edges) if post_edges else 0.0

    metrics = {
        "delta_nodes_plus_edges": delta,
        "min_growth_required": min_growth,
        "chunk_failure_ratio": round(chunk_fail_ratio, 3),
        "ambiguous_share": round(ambig_share, 3),
    }

    if delta < min_growth:
        return GateResult("fail",
            f"growth too small: {delta} < required {min_growth}",
            metrics)
    if chunk_fail_ratio > MAX_FAILED_CHUNK_RATIO:
        return GateResult("fail",
            f"chunk failure rate {chunk_fail_ratio:.0%} > {MAX_FAILED_CHUNK_RATIO:.0%}",
            metrics)
    if ambig_share > MAX_AMBIGUOUS_SHARE:
        return GateResult("warn",
            f"ambiguous edge share {ambig_share:.0%} > {MAX_AMBIGUOUS_SHARE:.0%}",
            metrics)
    return GateResult("pass", "all thresholds satisfied", metrics)


def main() -> int:
    import argparse
    ap = argparse.ArgumentParser()
    ap.add_argument("--pre-graph", type=Path, default=None)
    ap.add_argument("--post-graph", type=Path, required=True)
    ap.add_argument("--stage-file-count", type=int, required=True)
    ap.add_argument("--failed-chunks", type=int, required=True)
    ap.add_argument("--total-chunks", type=int, required=True)
    args = ap.parse_args()
    result = evaluate(
        args.pre_graph, args.post_graph,
        args.stage_file_count, args.failed_chunks, args.total_chunks,
    )
    print(f"[{result.status.upper()}] {result.reason}")
    print(f"  metrics: {result.metrics}")
    return {"pass": 0, "warn": 1, "fail": 2}[result.status]


if __name__ == "__main__":
    sys.exit(main())
```

- [ ] **Step 4: Run tests — confirm all pass**

```bash
python3 -m pytest tests/test_audit_gate.py -v
```

Expected: 4 passed.

- [ ] **Step 5: Log audit gate completion**

```bash
cat >> staging-log.md <<EOF

## Task 7 — audit gate ($(date '+%Y-%m-%d %H:%M'))

- 4 tests passing
- Thresholds: growth ≥ files*2 (fail), failed chunks ≤ 20% (fail), AMBIGUOUS share ≤ 30% (warn)
EOF
```

---

## Task 8: Stage orchestrator shell script

**Files:**
- Create: `claw-code-graphify-orchestrator/run_stage.sh`

`run_stage.sh` glues rsync + graphify + audit-gate + log into one command per stage. graphify itself is a Claude Code slash command, so the script cannot invoke `/graphify` directly — instead it prepares everything and prints the exact slash command for the operator (human or executing agent) to paste. After the slash command finishes, the operator re-runs the script with `--post` to evaluate the gate.

- [ ] **Step 1: Write the orchestrator script**

Create `run_stage.sh`:

```bash
#!/usr/bin/env bash
# run_stage.sh — drive one stage of the staged-graphify pipeline.
#
# Usage:
#   ./run_stage.sh <stage> --pre    # rsync files + print the /graphify command
#   ./run_stage.sh <stage> --post   # run audit gate + log result
#
# The two-phase split is because /graphify is a Claude Code slash command that
# cannot be invoked from a non-interactive shell. The operator runs --pre,
# pastes the printed slash command in their Claude Code session, then runs
# --post once the command returns.

set -euo pipefail

CLAW=~/Projects/LingXi-Next/claw-code
STAGING=~/Projects/LingXi-Next/claw-code-graphify
ORCH=~/Projects/LingXi-Next/claw-code-graphify-orchestrator
SNAPSHOT_DIR="${ORCH}/.snapshots"

stage="${1:?usage: run_stage.sh <stage> --pre|--post}"
phase="${2:?usage: run_stage.sh <stage> --pre|--post}"
manifest="${ORCH}/manifests/stage${stage}.manifest"

[[ -f "$manifest" ]] || { echo "missing $manifest — run generate_manifests.py first"; exit 1; }

mkdir -p "$SNAPSHOT_DIR"

case "$phase" in
  --pre)
    # Snapshot the existing graph (if any) so the audit gate can compute delta.
    if [[ -f "${STAGING}/graphify-out/graph.json" ]]; then
      cp "${STAGING}/graphify-out/graph.json" "${SNAPSHOT_DIR}/pre-stage${stage}.json"
    fi
    # Special: stage 1 splits ROADMAP.md into staging-local slices first.
    if [[ "$stage" == "1" ]]; then
      mkdir -p "$STAGING"
      python3 "${ORCH}/split_roadmap.py" "${CLAW}/ROADMAP.md" "${STAGING}"
      # Remove the original ROADMAP.md from the manifest so it isn't double-included.
      grep -v '^ROADMAP\.md$' "$manifest" > "${manifest}.filtered"
      manifest="${manifest}.filtered"
    fi
    # Rsync this stage's files into staging.
    rsync -a --files-from="$manifest" "${CLAW}/" "${STAGING}/"
    # Print the slash command for the operator.
    echo
    echo "==> Files rsynced for stage ${stage}. Now paste into Claude Code:"
    if [[ "$stage" == "1" ]]; then
      echo
      echo "    /graphify ${STAGING} --mode deep"
      echo
    else
      echo
      echo "    /graphify ${STAGING} --update"
      echo
    fi
    echo "==> After graphify completes, run: ./run_stage.sh ${stage} --post"
    ;;

  --post)
    post_graph="${STAGING}/graphify-out/graph.json"
    pre_graph="${SNAPSHOT_DIR}/pre-stage${stage}.json"
    file_count=$(wc -l < "$manifest" | tr -d ' ')
    # The operator must supply failed/total chunks from the graphify run output.
    # If they pass them as env vars, use them; otherwise assume 0 failures.
    failed="${FAILED_CHUNKS:-0}"
    total="${TOTAL_CHUNKS:-1}"
    python3 "${ORCH}/audit_gate.py" \
        --pre-graph "$pre_graph" \
        --post-graph "$post_graph" \
        --stage-file-count "$file_count" \
        --failed-chunks "$failed" \
        --total-chunks "$total"
    status=$?
    {
      echo
      echo "## Stage ${stage} — $(date '+%Y-%m-%d %H:%M')"
      echo
      echo "- manifest files: ${file_count}"
      echo "- graphify chunks: ${failed} failed / ${total} total"
      case $status in
        0) echo "- audit: PASS" ;;
        1) echo "- audit: WARN (review before continuing)" ;;
        2) echo "- audit: FAIL (do not proceed; investigate)" ;;
      esac
    } >> "${ORCH}/staging-log.md"
    exit $status
    ;;

  *)
    echo "unknown phase: $phase (use --pre or --post)" >&2
    exit 1
    ;;
esac
```

- [ ] **Step 2: Make it executable and smoke-test the help output**

```bash
chmod +x ~/Projects/LingXi-Next/claw-code-graphify-orchestrator/run_stage.sh
~/Projects/LingXi-Next/claw-code-graphify-orchestrator/run_stage.sh 1 --pre 2>&1 | head -5 || true
```

Expected: either the script runs to completion (if `manifests/stage1.manifest` exists from Task 5) and prints the `/graphify ... --mode deep` instruction, OR it errors with "missing manifest". Either result confirms the script parses.

- [ ] **Step 3: Log orchestrator completion**

```bash
cat >> staging-log.md <<EOF

## Task 8 — orchestrator shell script ($(date '+%Y-%m-%d %H:%M'))

- run_stage.sh handles --pre (rsync + print slash command) and --post (audit gate)
- Stage 1 includes ROADMAP splitter step
EOF
```

---

## Task 9: Execute Stage 1 (architecture & docs)

This task and Tasks 10–13 execute the actual pipeline. No new code is written; the operator drives `run_stage.sh` and pastes graphify slash commands into Claude Code.

- [ ] **Step 1: Run the pre-phase**

```bash
cd ~/Projects/LingXi-Next/claw-code-graphify-orchestrator
./run_stage.sh 1 --pre
```

Expected output:
- A short list of ROADMAP slice files written to `~/Projects/LingXi-Next/claw-code-graphify/`
- An rsync run with no errors
- A printed line `/graphify ~/Projects/LingXi-Next/claw-code-graphify --mode deep`

- [ ] **Step 2: Paste the slash command into Claude Code**

In your active Claude Code session, paste exactly what the script printed:

```
/graphify ~/Projects/LingXi-Next/claw-code-graphify --mode deep
```

The graphify pipeline will run its 9 steps. Note in particular:
- Step 2 prints a corpus summary; verify total file count is in the 30–65 range
- Step 3B dispatches semantic subagents and prints `WARNING: chunk N failed` if any
- Step 4 prints `Graph: X nodes, Y edges, Z communities`
- Step 5 (community labels) — accept the auto-generated names for now
- Step 9 prints the final token cost

Record the `failed chunks` and `total chunks` counts from Step 3B's output.

- [ ] **Step 3: Run the post-phase audit gate**

```bash
FAILED_CHUNKS=<value from step 2> \
TOTAL_CHUNKS=<value from step 2> \
  ./run_stage.sh 1 --post
```

Expected: `[PASS]` printed. If WARN or FAIL, stop and read the staging-log.md entry plus the freshly written GRAPH_REPORT.md.

- [ ] **Step 4: Sanity-check the graph**

```bash
GRAPH=~/Projects/LingXi-Next/claw-code-graphify/graphify-out
echo '--- node count ---'
python3 -c "import json; print(len(json.loads(open('$GRAPH/graph.json').read())['nodes']))"
echo '--- top god nodes ---'
grep -A 10 'God Nodes' "$GRAPH/GRAPH_REPORT.md" | head -15
```

Expected: node count ≥ 80 (~2× the file count); God Nodes include 1-2 ROADMAP slices and 1-2 architecture docs.

- [ ] **Step 5: Move to stage 2 only after PASS**

If WARN: read the §3 troubleshooting in the spec and decide whether to accept-and-continue or fix-and-rerun. If FAIL: do not proceed.

---

## Task 10: Execute Stage 2 (Rust workspace)

- [ ] **Step 1: Run the pre-phase**

```bash
cd ~/Projects/LingXi-Next/claw-code-graphify-orchestrator
./run_stage.sh 2 --pre
```

Expected: rsync brings in ~100 Rust files; script prints `/graphify ~/Projects/LingXi-Next/claw-code-graphify --update`.

- [ ] **Step 2: Paste the slash command into Claude Code**

```
/graphify ~/Projects/LingXi-Next/claw-code-graphify --update
```

graphify's `--update` mode (SKILL.md `## For --update`) detects new files vs. its manifest, re-extracts only those, and merges into the existing graph. Step 4 should print a `Merged: …` line. AST extraction will run automatically on the .rs files.

Record `failed chunks` and `total chunks` (may be 0/0 if all-code update skipped Part B per SKILL line 145).

- [ ] **Step 3: Run the post-phase audit gate**

```bash
FAILED_CHUNKS=<value> TOTAL_CHUNKS=<value> ./run_stage.sh 2 --post
```

Expected: `[PASS]`. The pre-graph is the stage-1 snapshot saved by `--pre`; the audit measures the stage-2 delta.

- [ ] **Step 4: Spot-check Rust crate coverage**

```bash
python3 -c "
import json
g = json.loads(open('$HOME/Projects/LingXi-Next/claw-code-graphify/graphify-out/graph.json').read())
rust = [n for n in g['nodes'] if (n.get('source_file') or '').startswith('rust/')]
crates = sorted({n['source_file'].split('/')[2] for n in rust if n['source_file'].startswith('rust/crates/')})
print('rust nodes:', len(rust))
print('crates seen:', crates)
"
```

Expected: at least 7 of 9 crates (api, commands, runtime, plugins, etc.) appear.

---

## Task 11: Execute Stage 3 (Python source)

- [ ] **Step 1: Run the pre-phase**

```bash
cd ~/Projects/LingXi-Next/claw-code-graphify-orchestrator
./run_stage.sh 3 --pre
```

Expected: rsync brings in ~100 Python+JSON files under `src/`.

- [ ] **Step 2: Paste the slash command into Claude Code**

```
/graphify ~/Projects/LingXi-Next/claw-code-graphify --update
```

`src/` has 30 feature subdirectories — expect graphify to surface multiple communities at this stage. Record `failed chunks` and `total chunks` from Step 3B.

- [ ] **Step 3: Run the post-phase audit gate**

```bash
FAILED_CHUNKS=<value> TOTAL_CHUNKS=<value> ./run_stage.sh 3 --post
```

Expected: `[PASS]`.

- [ ] **Step 4: Spot-check src/ subsystem coverage**

```bash
python3 -c "
import json
g = json.loads(open('$HOME/Projects/LingXi-Next/claw-code-graphify/graphify-out/graph.json').read())
py = [n for n in g['nodes'] if (n.get('source_file') or '').startswith('src/')]
subs = sorted({n['source_file'].split('/')[1] for n in py})
print('python nodes:', len(py))
print('subsystems seen:', subs)
"
```

Expected: at least 20 of the 30 src/ subdirectories represented.

---

## Task 12: Execute Stage 4 (ops glue)

- [ ] **Step 1: Run the pre-phase**

```bash
cd ~/Projects/LingXi-Next/claw-code-graphify-orchestrator
./run_stage.sh 4 --pre
```

Expected: rsync brings in 5–10 shell/python script files from `scripts/` and `tests/`.

- [ ] **Step 2: Paste the slash command into Claude Code**

```
/graphify ~/Projects/LingXi-Next/claw-code-graphify --update
```

This stage is small; runtime should be under a minute.

- [ ] **Step 3: Run the post-phase audit gate**

```bash
FAILED_CHUNKS=<value> TOTAL_CHUNKS=<value> ./run_stage.sh 4 --post
```

Expected: `[PASS]`. Stage growth threshold is lower because the stage is small (file_count × 2 = ~15 min growth).

---

## Task 13: Execute Stage 5 (optional agent tooling)

**Decision point:** Stage 5 is optional per spec §1. Skip if the user only wants structural understanding; include if they want to see how the agent tooling (`.omx/cc2/render_board_md.py` and friends) plugs into the rest of the codebase.

- [ ] **Step 1: Confirm whether to run this stage**

If yes, proceed. If no, jump to Task 14.

- [ ] **Step 2: Run the pre-phase**

```bash
cd ~/Projects/LingXi-Next/claw-code-graphify-orchestrator
./run_stage.sh 5 --pre
```

Expected: rsync brings in 3–8 files from `.omx/cc2/` (scripts only, not the board) and `.omx/ultragoal/`.

- [ ] **Step 3: Paste the slash command into Claude Code**

```
/graphify ~/Projects/LingXi-Next/claw-code-graphify --update
```

- [ ] **Step 4: Run the post-phase audit gate**

```bash
FAILED_CHUNKS=<value> TOTAL_CHUNKS=<value> ./run_stage.sh 5 --post
```

Expected: `[PASS]` or `[WARN]`. AMBIGUOUS share may be elevated here because the corpus is small and atypical; this is acceptable.

---

## Task 14: Final re-clustering and verification

After all chosen stages are merged, re-cluster the whole graph and produce the final outputs.

- [ ] **Step 1: Paste the final clustering slash command into Claude Code**

```
/graphify ~/Projects/LingXi-Next/claw-code-graphify --cluster-only
```

This re-runs Louvain (SKILL.md `## For --cluster-only`) on the merged graph and rewrites GRAPH_REPORT.md and graph.html so community labels reflect the entire project, not the last-merged stage.

- [ ] **Step 2: Eyeball the final report**

```bash
GRAPH=~/Projects/LingXi-Next/claw-code-graphify/graphify-out
echo '=== Communities ==='
grep -E '^## (Communities|Community )' "$GRAPH/GRAPH_REPORT.md" | head -30
echo
echo '=== God Nodes ==='
sed -n '/^## God Nodes/,/^## /p' "$GRAPH/GRAPH_REPORT.md" | head -25
echo
echo '=== Surprising Connections ==='
sed -n '/^## Surprising Connections/,/^## /p' "$GRAPH/GRAPH_REPORT.md" | head -25
echo
echo '=== Total cost ==='
cat "$GRAPH/cost.json" | python3 -m json.tool | head -10
```

Expected: 5–15 distinct communities with human-readable names; God Nodes span Rust crates and Python subsystems (not just docs); Surprising Connections include at least one cross-language edge (e.g., a Rust crate semantically similar to a Python module).

- [ ] **Step 3: Open the HTML graph and skim**

```bash
open ~/Projects/LingXi-Next/claw-code-graphify/graphify-out/graph.html
```

Confirm visually: nodes are colored by community, layout shows distinct clusters for Rust crates vs. Python subsystems vs. docs, no obvious orphan-node islands containing high-signal content.

- [ ] **Step 4: Write the final staging-log entry**

```bash
cat >> ~/Projects/LingXi-Next/claw-code-graphify-orchestrator/staging-log.md <<EOF

## Final — pipeline complete ($(date '+%Y-%m-%d %H:%M'))

Outputs in ~/Projects/LingXi-Next/claw-code-graphify/graphify-out/:
- graph.html ($(du -h ~/Projects/LingXi-Next/claw-code-graphify/graphify-out/graph.html 2>/dev/null | cut -f1))
- graph.json ($(du -h ~/Projects/LingXi-Next/claw-code-graphify/graphify-out/graph.json 2>/dev/null | cut -f1))
- GRAPH_REPORT.md ($(du -h ~/Projects/LingXi-Next/claw-code-graphify/graphify-out/GRAPH_REPORT.md 2>/dev/null | cut -f1))

Total token cost:
$(cat ~/Projects/LingXi-Next/claw-code-graphify/graphify-out/cost.json | python3 -m json.tool)
EOF
```

- [ ] **Step 5: (Optional) Try a query against the merged graph**

Pick the most interesting Suggested Question from GRAPH_REPORT.md and run:

```
/graphify query "<that question>"
```

If the answer makes sense and cites source files, the merged graph is healthy.
