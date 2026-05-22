# Staged graphify of claw-code

**Date:** 2026-05-21
**Status:** Draft — pending user review
**Target project:** `/Users/luolingfeng/Projects/LingXi-Next/claw-code`
**Final artifact location:** `/Users/luolingfeng/Projects/LingXi-Next/claw-code-graphify/graphify-out/`

## Background

`claw-code` is a multi-language workspace (Rust + Python) with ~349 source files
(~5 MB of text, but with one 127k-word `ROADMAP.md` plus a 1 MB auto-generated
`.omx/cc2/board.json`). A single `/graphify` pass on the project root exceeds
graphify's built-in safety limit (>200 files OR >2M words) and would either be
refused or produce a low-signal graph dominated by generated artifacts.

We want one merged knowledge graph covering the whole real codebase, built up in
stages so each pass stays within graphify's healthy operating range and any
single failure is recoverable without restarting the full extraction.

## Goal

Produce a single `graph.json` + `graph.html` + `GRAPH_REPORT.md` that covers
claw-code's architecture, Rust crates, Python source, ops scripts, and a
controlled subset of agent-generated artifacts — in five sequential graphify
runs that merge into the same `graphify-out/`.

Non-goals:
- Re-running graphify continuously on every commit (use `--watch` or the git
  post-commit hook separately if needed)
- Indexing images, lockfiles, port-session captures, or auto-generated boards
- Modeling cross-version drift between `~/claw-code` (1.1 GB snapshot) and the
  current LingXi copy

## Constraints

- Each stage must keep the active staging corpus under graphify's per-run limit
  (200 files / 2M words) so Step 2's "pick a subfolder" warning never triggers.
- Each stage must be idempotent — re-running on partial failure picks up cached
  files via `.graphify_semantic_cache/`.
- Node `source_file` paths must reflect the real claw-code layout, not the
  staging dir layout, so a future reader can navigate from graph to source.
- No new git repositories are created. The working directory
  (`~/Projects/LingXi-Next/`) stays non-tracked; the claw-code repo is read
  only.

## §1 — Stage breakdown

Five additive stages. Each stage's manifest is a plain-text file list (one path
per line, relative to claw-code root) consumed by `rsync --files-from=…`.

| # | Stage | Source paths (globs) | ~Files | ~Words |
|---|-------|----------------------|-------:|-------:|
| S1 | Architecture & docs | Top-level `*.md` / `*.txt` / `*.json` (not under `.omx/`); `docs/**/*.md`; `rust/*.md`; `rust/*.json` | 40 + ROADMAP slices | ~200k |
| S2 | Rust workspace | `rust/crates/**/*.rs`; `rust/**/Cargo.toml`; `rust/scripts/**` | ~100 | ~150k |
| S3 | Python source | `src/**/*.py`; `src/**/*.json` | ~100 | ~80k |
| S4 | Ops glue | `scripts/**`; `tests/**` | 7 | ~10k |
| S5 | *(optional)* Agent tooling | `.omx/cc2/render_board_md.py`, `.omx/cc2/validate_*.py`, `.omx/ultragoal/**` (excluding `.omx/cc2/board.{json,md}`) | ~5 | <2k |

### Default exclusions (apply to every stage)

- `.omx/cc2/board.json` and `.omx/cc2/board.md` — auto-generated, dominated by
  noise that would distort community detection
- `.port_sessions/` — session captures, not structural signal
- `assets/` — screenshots and logos; vision-based extraction is expensive and
  these files don't carry architectural meaning
- `rust/.claude/sessions/*.json` — stale session state
- `Cargo.lock` — lockfile, no structural value
- `.git/`, `.github/workflows/` — VCS and CI metadata, out of scope

### ROADMAP.md special handling

`ROADMAP.md` is 127k words — putting it in a single semantic-extraction chunk
would either explode the chunk's token budget or starve neighboring files of
attention. Before S1 runs, a one-shot preprocessor splits it by Markdown level-1
headings into `ROADMAP__<slug>.md` files placed alongside the original in the
staging directory. If any single H1 section exceeds 15k words, it is split
further by H2. Each slice carries YAML frontmatter:

```yaml
---
roadmap_section: <slug>
source_url: file://ROADMAP.md
---
```

Per graphify's SKILL contract (line 247 of `/Users/luolingfeng/.claude/skills/graphify/SKILL.md`), these frontmatter fields are
copied onto every node extracted from each slice, preserving the "all slices
come from one ROADMAP" relationship in the graph.

## §2 — Execution mechanism

### Staging directory

A sibling directory at `~/Projects/LingXi-Next/claw-code-graphify/` holds the
progressive corpus. The directory is built additively: each stage `rsync`s its
manifest's files in (preserving relative paths), then graphify is invoked
against the staging root.

### Command template

```bash
# Stage 1 — fresh run (creates graphify-out/)
rsync -a --files-from=stage1.manifest ./claw-code/ ./claw-code-graphify/
/graphify ~/Projects/LingXi-Next/claw-code-graphify --mode deep

# Stages 2–5 — incremental
rsync -a --files-from=stage<N>.manifest ./claw-code/ ./claw-code-graphify/
/graphify ~/Projects/LingXi-Next/claw-code-graphify --update
```

### Flag rationale

- `--mode deep` on **S1 only**. Documents need aggressive INFERRED edges to
  surface cross-doc concept overlaps. Code stages already get structural edges
  from AST extraction; running deep there mostly adds AMBIGUOUS noise.
- No `--obsidian`, `--svg`, `--graphml`, `--neo4j`, or `--mcp` flags by default.
  HTML + JSON + report are sufficient. If the user later wants alternate
  outputs, regenerate once at the end with `--cluster-only` plus the desired
  flag.
- `--files-from=stage<N>.manifest` is the **only** mechanism for selecting per-
  stage files. The manifest is the auditable source of truth for what got
  included; the staging directory is downstream of the manifest, not the input
  to it.

### Final reclustering

After S4 (and optionally S5) completes, run once more:

```bash
/graphify ~/Projects/LingXi-Next/claw-code-graphify --cluster-only
```

This re-runs Louvain community detection on the merged graph so the final
community boundaries reflect the whole project, not the boundaries that existed
when each stage was added in isolation.

## §3 — Per-stage audit gate

After each `/graphify` invocation completes, check three numbers before
proceeding to the next stage:

| Metric | Source | Threshold |
|--------|--------|-----------|
| New nodes + edges added | Delta in `graph.json` vs. previous snapshot | > 2 × number of files in the stage manifest |
| Failed extraction chunks | `WARNING: chunk N failed` lines printed by graphify Step 3B2 | ≤ 20% of total chunks (graphify hard-stops at 50%) |
| AMBIGUOUS edge share | `GRAPH_REPORT.md` confidence breakdown | < 30% — higher means the source was unusually vague |

Also inspect the report's **God Nodes** and **Surprising Connections** sections.
If God Nodes for that stage are dominated by table-of-contents-style documents
(README, ROADMAP itself), this is a signal that real internal structure was not
captured — pause and check whether the manifest is missing signal files before
moving on.

## §4 — Failure recovery

- **≥ 50% chunks fail in one stage:** graphify auto-aborts (SKILL line 256).
  Identify the offending file class (usually a single oversized text file) and
  apply the §1 ROADMAP-style slicing rule to it. Re-run `--update` on the same
  stage.
- **A stage is interrupted (Ctrl+C, OOM, network):** re-run the same `--update`
  command. `.graphify_semantic_cache/` holds per-file results; only files
  without cache entries get re-extracted.
- **Stage manifest contains a wrong file:** delete the staging directory and
  restart from S1. If graphify's semantic cache lives outside the staging dir
  (e.g. under `~/.cache/graphify/`), already-extracted files short-circuit and
  the dominant cost is rebuild + re-cluster. If the cache lives inside
  `graphify-out/`, deletion forces full re-extraction; cost is then bounded by
  the original first-run budget. The implementation plan will probe and record
  which case applies before relying on the cheaper path.
- **Need to undo a stage selectively:** not supported. Stage-level rollback
  would require maintaining a per-stage graph snapshot history, which is
  out of scope. The pragmatic workaround is: rebuild from S1 with the corrected
  manifests.

## §5 — Final outputs

In `~/Projects/LingXi-Next/claw-code-graphify/graphify-out/`:

- `graph.html` — interactive browser-rendered graph (skipped if the merged
  graph exceeds 5000 nodes; in that case the `GRAPH_REPORT.md` still has the
  community summary)
- `graph.json` — raw node/edge data, GraphRAG-compatible
- `GRAPH_REPORT.md` — community labels, God Nodes, Surprising Connections,
  Suggested Questions, token-cost audit
- `cost.json` — cumulative input/output tokens across all stages

Adjacent to the staging directory:

- `stage1.manifest` … `stage5.manifest` — exact file lists used per stage
- `staging-log.md` — append-only journal: timestamp, stage number, command run,
  audit-gate result (pass/warn/fail), and a 1-line summary of new God Nodes

## Open questions

None remaining for the design. Concrete implementation details (manifest
generation script, ROADMAP splitter, audit-gate checker) will be specified in
the implementation plan produced by the next skill.
