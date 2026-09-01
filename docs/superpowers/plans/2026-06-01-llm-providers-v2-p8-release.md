# LLM Providers v2 — P8: Polish & Release — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:subagent-driven-development. Steps use `- [ ]`.

**Goal:** Ship v2 — bump all crates to `v0.12.0`, write the docs/CHANGELOG/README, run a final holistic review, and tag `v0.12.0`.

**Spec:** `2026-06-01-llm-providers-v2-design.md` §7 P8. Branch `llm-providers-v2` (P1–P7 done, tags `llm-v2-p1`…`llm-v2-p7`).

**Parity gate:** docs + version-only changes; no behavior change. Full suite + parity stay green. Do NOT modify `traits/`.

---

## Task A: Version bump 0.11.0 → 0.12.0

**Files:** the 68 `lingxi-code/**/Cargo.toml` with `version = "0.11.0"`; 3 TUI version-string snapshots (if not normalized).

- [ ] **Step 1 — bump package versions.** From `lingxi-code/`, find package-version lines (line-start `version = "0.11.0"`, which excludes inline dependency specs) and bump to `0.12.0`:
```bash
rg -l '^version = "0.11.0"' . -g 'Cargo.toml'   # expect ~68
# for each, replace the line `version = "0.11.0"` -> `version = "0.12.0"`
```
Use a precise replacement (only the line-anchored package `version`, NOT dependency `version = "0.11.0"` specs). Verify the count of changed files (~68) and that no dependency version spec was altered (`rg 'version = "0.11.0"' -g Cargo.toml` afterward should be empty or only intended).

- [ ] **Step 2 — rebuild + regenerate version snapshots.** `cargo build --workspace`. Then `cargo test -p tui` — if version-string snapshots fail (3 candidates: `render_placeholder`, `settings_status_tab`, `status_renders_real_snapshot_rows`), regenerate them and CONFIRM the diff is version-string-only (`0.11.0`→`0.12.0` or equivalent). If the snapshots normalize the version (e.g. show `vln`), they won't change — no action.

- [ ] **Step 3 — gates + commit.**
```bash
cargo build --workspace
git add -A lingxi-code
git commit -m "chore(llm-v2 P8): bump workspace to v0.12.0

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task B: Docs — CHANGELOG, README, LLM_PROVIDERS (controller-authored)

The controller authors these (full v2 narrative). Content requirements:
- **`CHANGELOG.md`** — a `## [0.12.0] — LLM Providers v2` entry above `[0.11.0]`, covering: vision (image input), reasoning params (effort/budget + decode), Azure OpenAI, Vertex (GCP token), Bedrock (SigV4, non-streaming + synthetic stream), router (aliases/fallback/retry). Note the bounded deferrals (TUI paste-to-image, real Bedrock streaming, `/model` handle wiring, Bedrock pricing).
- **`README.md`** — update the intro + "Subsystem status (vX)" heading to v0.12.0; extend/append the LLM Providers row(s) with the v2 capabilities.
- **`docs/LLM_PROVIDERS.md`** — comprehensive v2 section: new provider kinds (`azureOpenAi`/`vertex`/`bedrock`), env keys (`AZURE_OPENAI_KEY`, `GOOGLE_APPLICATION_CREDENTIALS`/ADC, `AWS_ACCESS_KEY_ID`/`AWS_SECRET_ACCESS_KEY`/`AWS_SESSION_TOKEN`), reasoning config (`reasoningEffort`/`thinkingBudget`), the `routing` block (aliases/fallback/retry), vision usage, and the documented limitations.
- Commit: `docs(llm-v2 P8): v0.12.0 CHANGELOG + README + LLM_PROVIDERS for v2`.

---

## Task C: Final holistic review + release gate + tag

- [ ] **Step 1 — full gate.** From `lingxi-code/`:
  - `cargo build --workspace` → Finished.
  - `cargo test -p providers -p orchestrator -p core -p cost -p test-harness` → all pass (parity byte-locks green).
  - `cargo clippy -p providers -p orchestrator -p core --no-deps --all-targets -- -D warnings` → clean.
  - `bash scripts/check-deps.sh` → OK.
- [ ] **Step 2 — final holistic review (opus subagent):** review the whole v2 diff (`llm-v2-p1^..HEAD` i.e. the v2 commits) for cohesion, the bounded deviations, and any cross-phase gap. Verdict READY / NOT READY.
- [ ] **Step 3 — tag.** `git tag -a v0.12.0 -m "LLM Providers v2: vision, reasoning, Azure/Vertex/Bedrock, router"` and `git tag -a llm-v2-p8 -m "LLM Providers v2 P8: release"`.

---

## Self-Review

**Spec coverage (§7 P8):** version bump (A) ✓; docs/CHANGELOG/README (B) ✓; dep-gate allowances (handled in P5/P6) ✓; final holistic review (C2) ✓; tag (C3) ✓.
**Placeholder scan:** mechanical bump + doc content requirements + gate commands — concrete.
**Consistency:** v0.12.0 across Cargo.toml + docs; tags `llm-v2-p8` + `v0.12.0`.
