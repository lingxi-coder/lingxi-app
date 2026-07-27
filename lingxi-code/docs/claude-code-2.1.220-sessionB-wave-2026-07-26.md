# Claude Code 2.1.220 parity — Session B wave report (2026-07-26)

Branch: `gap220/integration` (lanes `gap220/ui-misc`, `gap220/mcp-stream`, `gap220/auth-eng` merged).
Base: `a13591ed6` (already contained H5 config+trust, H6, H7, H8, M3, M4, mcp-list health checks — re-verified per item before fixing).
Head at gate time: see the gate section at the bottom.
Audit source: `docs/claude-code-2.1.220-global-parity-reaudit-2026-07-26.md`; per-item evidence: session scratchpad `gap220-evidence.json` (ephemeral — key summaries copied below).

Statuses marked "gate-verified" are items whose implementation agent crashed after committing: the orchestrator recorded them NOT-FIXED, but the commits are in the lanes and the gate re-verified each at the behaviour site (oracle strings + tests present).

## 1. Report items (10)

| Item | Title (audit §) | Status | Commits |
|---|---|---|---|
| H5 | `sandbox.network.strictAllowlist` — runtime enforcement (§H5) | FIXED (config+trust half was already at base) | `45b6f00eb` |
| M5 | MCP list/get observable state + diagnostics (§M5) | FIXED — deferred sliver: M5-issue-formatting | `4f263bd58` |
| M6 | managed MCP allow/deny env-var sources (§M6) — expand predicates against the startup env snapshot | FIXED — deferred sliver: M6-flagSettings-env-tier | `934237b4b` |
| M7 | agent frontmatter `mcpServers` → live tool registry (§M7) | FIXED — deferred sliver: M7-plugin-warn-variant | `b1ad5b477` + `f38e5babb` (bridge-server `strict_mcp_config` repair) |
| M8 | `auth login` browser-failure manual fallback (§M8) | FIXED | `2d55f32c5` |
| M9 | agent names still allow `:` (§M9) — NFKC `:` reject + leading `-` on the markdown path | FIXED (gate-verified) | `cc4c92e36` |
| M11 | Windows `CLAUDE_CODE_GIT_BASH_PATH` validation + auto-detect (§M11) | FIXED (gate-verified) | `fc09d6f8b` |
| M12 | agent-view left-arrow state machine (§M12) — settings gate, kGt hints, vim NORMAL | FIXED — deferred slivers: M12-attach-detach, M12-midturn-backgrounding, M12-managed-row | `7777dfae4` |
| M13 | OAuth source / enterprise subscriber seam (§M13) | FIXED (both halves) | `8b23d69dc`, `41c66c276` |
| P1 | OTEL record-site coverage (§P1) — `code_edit_tool.decision`, pr/commit counts, CC-named log records, dispatch-gate decision | FIXED — deferred sliver: P1-user_prompt-log | `95264c04c`, `6900c2094` |

## 2. Attempted hunted gaps (this wave)

| Item | Title | Status | Commits |
|---|---|---|---|
| N-changelog-1 | skill/plugin frontmatter booleans: yes/no/on/off/1/0 coercion (2.1.218) | FIXED (gate-verified) | `b4734fa27` |
| N-changelog-2 | prompt-history persistence (`history.jsonl`, locked + deduped writes; substrate for the 2.1.218 race fix) | FIXED | `85675f6fd` |
| N-env-4 | mid-session `date_change` attachment | FIXED — landed post-crash by the orchestrator (see §2.1) | `c203d32c3` |
| N-env-5 | `CLAUDE_CODE_DISPATCH_V2S` → `anthropic-dispatch-id: v2s` header + 5xx degradation latch (2.1.219) | FIXED (gate-verified) | `6e52ad676` |
| N-protocol-1 | interrupt receipt contract (`capabilities` in system/init, `still_queued`, `cancel_queued`/`cancelled`) | FIXED | `51949c597` |
| N-protocol-2 | `fast_mode_disabled_reason` in system/init, result frames, initialize control_response | FIXED | `51949c597` (same commit) |
| N-protocol-3 | Skill tool prompt was pre-2.1.217 text; missing 2.1.218 background-skill sentence | FIXED (gate-verified) | `cbb1ee581` |
| N-protocol-6 | result-frame `modelUsage.provider` field (2.1.218 sibling of `canonicalModel`) | FIXED — landed post-crash by the orchestrator (see §2.1) | `4c65b21b5` |

Adjudicated during the wave (not counted as attempted):

- **N-protocol-4** (`set_cwd` control request incl. `needs_trust`/`trust_root`): **already closed at baseline** — commit `7b3e6be1b` (contained in base `a13591ed6`) implemented it in `permission/src/set_cwd.rs` + the `run.rs` dispatch arm. Re-verified this session against the 2.1.220 binary: success `{status:"ok",cwd,changed,transcript_relocated}` and `{status:"needs_trust",directory,trust_root?}` match key-for-key (`trust_root` omitted when redundant, exactly like the oracle's conditional); the 4 `set_cwd` regression tests pass in this worktree. No change needed.

### 2.1 N-env-4 / N-protocol-6 — landed post-crash by the orchestrator

The agent that owned N-env-4 + N-protocol-6 stalled mid-stream (API error) AFTER finishing the edits but before verifying/committing. The orchestrator session took over its in-progress task: confirmed every `ModelUsageRow` literal site in the workspace was covered, ran `cargo check -q --workspace --tests` (clean; the two remaining warnings — missing-docs on `StdinChannels` and the `#[test]`-less `init_frame_matches_2_1_201_p_mode_shape` — pre-exist on base `a13591ed6` from `cea12a147`), ran the targeted suites (orchestrator `date_change` 2, cli `canonical_model` 2, cost 84, tui `screen_view` 15, command-core 376 — all green), then split-committed the WIP per item (`conversation.rs` divided by hunk via `git apply --cached`): `4c65b21b5` (N-protocol-6), `c203d32c3` (N-env-4). Merged into integration at `db5a4bf6b`.

## 3. Hunted-gap backlog — NOT attempted this wave

Evidence summaries copied verbatim from `gap220-evidence.json` (the scratchpad is ephemeral).

### N-env-2 (Medium, est L) — Ultracode ultra-effort enter/exit reminder chain + `CLAUDE_CODE_JUNIPER_SUNDIAL` cadence override missing

> Oracle 2.1.220: j2y (@237710871) is a transcript state machine: when EK(model,effort,workflowsOn)===true (`EK(e,t,r){return r===!0&&LA()&&Goe(e,t)==="xhigh"}`, LA()=workflows available+enabled — a real external feature) it emits `ultra_effort_enter` reminderType:"full" ('Ultracode is on: … Use the Workflow tool on every substantive task…'), re-emits sparse ('Ultracode is still on…') every bop() non-meta user turns, and `ultra_effort_exit` ('Ultracode is off…') on leave. bop() (@237702588): env CLAUDE_CODE_JUNIPER_SUNDIAL > statsig `tengu_juniper_sundial` > gate > ULTRA_EFFORT_CONFIG.TURNS_BETWEEN_MAINTENANCE=10. Chain exists in 2.1.217; env var added 2.1.218. Related `workflow_keyword_request` attachment + `workflowKeywordTriggerEnabled` setting. Port: rg for ultra_effort|Ultracode|JUNIPER_SUNDIAL|TURNS_BETWEEN_MAINTENANCE|workflow_keyword|workflowKeywordTriggerEnabled → zero code hits; effort.rs:106 hardcodes `dynamic_workflows_enabled() -> false` although tools/workflow is live (per report M3/M4).

Port files: `commands/core/src/effort.rs`, `orchestrator/src/conversation.rs`, `tools/workflow/src/lib.rs`.

### N-env-3 (Medium, est XL) — Memory-sync push mass-delete hold + `CLAUDE_CODE_DISABLE_MEMORY_MASS_DELETE_HOLD` escape hatch absent (whole push/pull sync engine unported)

> Oracle 2.1.220 (@235604090): `function Ity(e){if(Z.CLAUDE_CODE_DISABLE_MEMORY_MASS_DELETE_HOLD)return Number.POSITIVE_INFINITY;return Math.max(kty,Math.floor(e*xty))}` with kty=50, xty=0.1 — a memory push that would delete >= max(50, 10% of store) is HELD (data-loss safety, default ON); env var (new 2.1.218) disables the hold. Sits in the user+team multistore sync engine: push_written/push_deleted/conflicts log, delete modes corroborate|immediate|never via CLAUDE_CODE_MEMORY_PUSH_DELETE_MODE/`tengu_mem_push_delete_mode` (already in 2.1.217). Port: memory/src/team_memory.rs is only a local mtime poll watcher over ~/.lingxi/team-mem; rg push_deleted|push_written|mass.?delete|corroborate|MASS_DELETE_HOLD|MEMORY_PUSH_DELETE over all Rust → zero hits; no remote push/pull/delete path exists, so neither the hold nor its escape hatch has a consumer. Subsystem-scope product call needed first (team-memory adjacency to the frozen team/swarm features); if not ported, record an explicit Divergence(reason).

Port files: `memory/src/team_memory.rs`, `memory/src/lib.rs`.

### N-changelog-3 (Medium, est L) — `/deep-research` built-in workflow absent (with 2.1.218 manual-invocation-only semantics) — port ships no built-in workflow library

> Oracle: 2.1.218 changelog "Changed /deep-research to start only when invoked manually". 2.1.220 binary bundles the full workflow: `"deep-research"`, phases Scope/Search/Fetch/Verify/Synthesize, generated script (`VOTES_PER_CLAIM=3`, `MAX_FETCH=15`), invoked via `Workflow({name:'deep-research', args:'<question>'})`, gated only by kill-switch `tengu_sorrel_avocet` (enabled by default); 2.1.220 adds prompt guard "Do not use workflows or deep-research unless the user requested it" (0 hits in 217/218, 2 in 220). Port: only hit is a test fixture string (apps/cli/src/stream_json.rs:1648); tools/workflow/src/lib.rs:174-175 states "LingXi ships no built-in workflow library, so a `name` that isn't a saved file is an error" — `Workflow({name:'deep-research'})` errors.

Port files: `tools/workflow/src/lib.rs`, `commands/core/src/bundled/mod.rs`. Coordinate with H1 owners for the guard sentence.

### N-changelog-4 (Low, est M) — Copy-on-select (OSC 52) absent, so the 2.1.219 GNU-screen DCS-passthrough fix has no behaviour site

> Oracle: 2.1.219 changelog "Fixed copy-on-select inside GNU screen printing base64 into the terminal instead of copying the selection". 2.1.219 binary contains the OSC 52 clipboard write `]52;c;`, `tmux;` DCS-passthrough wrapping, and "copy on select" strings; feature predates the window (13 hits already in 2.1.217) — 219's delta is wrapping the OSC 52 emission in a DCS passthrough when $TERM/screen is detected. Port: zero hits for `]52;`/osc52/copy_on_select across tui and tui-core in main and w75; the only clipboard surfaces are /copy and clipboard-image paste — mouse-selection copy never emits OSC 52, so neither the feature nor the screen passthrough exists.

Port files: `tui/src/app.rs`, `tui-core/src/terminal_setup.rs`.

### N-changelog-5 (Low, est M) — Screen-reader input announcements stuck at 2.1.201: no deleted-text announcements (2.1.218) and full-line re-announce instead of typed-char echo (2.1.219)

> Oracle: 2.1.218 "Added screen-reader announcements of deleted text for word and line deletions in --ax-screen-reader mode" + "Fixed VoiceOver reading 'new line' instead of echoing the typed space"; 2.1.219 "Fixed screen-reader mode rewriting the entire input line on every keystroke instead of echoing only the typed character". Binary corroboration: 217→218 adds one `announce` and two `aria-live` sites. Port: both modules self-document as 2.1.201 ports (apps/cli/src/ax_screen_reader.rs:3, tui/src/screen_reader.rs:9); announcement granularity is whole-line diffs (`diff_lines`, screen_reader.rs:402) — exactly the pre-219 behavior — and the file has zero deletion-announcement logic. Verify exact wording against a live 2.1.220 run before locking strings.

Port files: `tui/src/screen_reader.rs`, `tui/src/composer.rs`, `apps/cli/src/ax_screen_reader.rs`.

### N-protocol-5 (Medium, est XL) — Agent observer pairing absent — frontmatter `observer`/`observerMessage` and 2.1.218 `observeSubagents` fan-out

> Oracle: agent definition zod schema has observer (string, 217+), observerMessage (217+), and observeSubagents (boolean, 0 hits in 2.1.217 → 12 in 218+). Runtime: '[agentObserver] Agent X declares observer Y…', pairing carries fanoutToSubagents:e.observeSubagents!==!1 and fanoutDepth with a depth cap, and spawned subagents inherit the observer unless observeSubagents:false ('not fanning out to observer agent (no chaining)'). Port: rg observer/observerMessage/observeSubagents across all .rs = 0 hits; agent/src/catalog.rs frontmatter parser has zero 'observ' matches, so the fields are silently dropped and no observer agent is ever armed. Schedule after H7 (depth 3) since fan-out interacts with spawn depth.

Port files: `agent/src/catalog.rs`, `traits/src/subagent_spawn.rs`.

### N-protocol-7 (Low, est S) — Lean-model (Opus 5) Bash/Agent tool description content not ported

> Oracle black-box: 2.1.220 with claude-opus-5 sends a Bash description WITHOUT the 'IMPORTANT: Avoid using this tool to run `cat`…' bullet and WITH new bullet '- Command output is displayed to you, not reliably to the user.'; the Agent tool drops its 'Reach for this when the task matches…' lead-in. Rerun of the SAME binary with claude-opus-4-8 shows neither change → model-conditional lean-tool-prompt content, unreachable pre-219. Port: tools/shell/src/prompt.rs:656-685 simple_prompt_concise still contains the avoid-list bullet and lacks the new line ('displayed to you' = 0 hits port-wide). **Depends on the sibling session's H1/M1/H2 capability-registry work** — refresh the SHORT prompt CONTENT when that lands.

Port files: `tools/shell/src/prompt.rs`, `tools/task/src`, `tool-api/src/model_prompt_gate.rs` (sibling-owned).

### N-protocol-8 (Low, est M) — Memory-backend push mass-delete hold (2.1.218, default-on) absent — rides an unported memory sync/push pipeline

> Oracle: 2.1.218 adds CLAUDE_CODE_DISABLE_MEMORY_MASS_DELETE_HOLD (0 hits in 2.1.217, 3 in 218+); binary: pushes deleting more than max(50, 10% of entries) are held by default, alongside the pre-existing push-delete modes (corroborate|immediate|never). Port: rg mass_delete/push_delete/corroborate/memory_stream across .rs = 0 hits; memory/src has no backend push module at all, so the entire push-delete pipeline the hold guards is absent. First make a product call whether backend memory sync is in scope (first-party account endpoints); if no, record it as an explicit boundary. Overlaps N-env-3 — triage the two together.

Port files: `memory/src/lib.rs`.

## 4. Adjudicated refutations / already-closed

| Item | Adjudication |
|---|---|
| H4 (`/code-review` 仍为 inline prompt) | **REFUTED** — in 2.1.220 `/code-review` IS the built-in skill invocation ("runs the built-in skill at its lightest effort level"); `CODE_REVIEW_WORKFLOW_NAME` has no consumer in the binary. Audit item was stale. |
| P2 (micro-compact 无真实 idle-gap 判定) | **REFUTED** at adjudication — evidence did not hold up oracle-side. Do not re-open without fresh binary evidence. |
| H3 (`context: fork` 没有真实执行链) | **Already closed** before this wave (context:fork + resume gate landed in the previous parity wave). |
| P3 (`MEMORY.md` near-cap advisory 未接入 write hook) | **Already closed** before this wave. |
| H6 (`DirectoryAdded` hook + `register_repo_root`) | **Partial** — the event, payload, and firing site were already closed at base `a13591ed6`; adjudicated as partially complete, remainder stays on the backlog. |

## 5. Out of scope for Session B (sibling session)

- **M10** (对外 parity 版本仍标记为 2.1.217 — `CLAUDE_CODE_VERSION` bump) and **H1** (lean main system prompt consumer) remain with the sibling session's prompt/model wave (H1/H2/M1/M2/M10). This gate deliberately did NOT bump `CLAUDE_CODE_VERSION`.
- Sibling-owned files were never touched by this wave: `orchestrator/src/prompt/*`, `tool-api/src/model_prompt_gate.rs`, `traits/src/model_capabilities.rs`.
- ⚠️ **Duplicate implementations to reconcile at the integration→main merge**: the sibling session's `d0ddcfb32` on main re-implemented **H5 runtime enforcement + M11** (Session B items per the split) from the same oracle sites (`wSu`/`srt` @229871903, `MQ` @226607421). Same four files on both sides: `sandbox-runtime/src/{config,matcher}.rs`, `sandbox-runtime-runner/src/convert.rs`, `tools/shell/src/bash.rs`. Whoever merges second keeps ONE implementation and the UNION of tests (this wave: `45b6f00eb` incl. the http_proxy CONNECT-403 enforcement-layer test + runner structural-key tests, `fc09d6f8b`; main: the cross-separator Windows-basename fix).

## 6. Deferred slivers carried out of this wave

M12-attach-detach, M12-midturn-backgrounding, M12-managed-row, P1-user_prompt-log, M5-issue-formatting, M6-flagSettings-env-tier, M7-plugin-warn-variant — documented in the owning lane summaries; nothing in the merge blocks them.

## 7. Integration + gate

- Merges: `gap220/ui-misc` → `6e3170341`, `gap220/mcp-stream` → `c609e5edd`, `gap220/auth-eng` → `5b9d6f4ea`; one textual conflict (`mcp/src/json_config.rs`, M5×M7 — resolved preserving both) and one semantic cross-lane breakage (M7's `strict_mcp_config` field missing from bridge-server initializers) repaired in `f38e5babb`. Second `gap220/ui-misc` merge (N-protocol-6 + N-env-4 remainder) → `db5a4bf6b` (clean; `stream_json.rs` auto-merged against the mcp-stream lane).
- Gate run 1 (head `db5a4bf6b`): `cargo build --workspace --tests` FAILED — E0063, M7's engine-desktop test helper missed mcp-stream's new `McpServerConfig.config_error` field (the second M5×M7 cross-lane seam; only `--tests` builds see it). Repaired in `51853980a`; the accompanying `cargo check -q --workspace --tests` sweep found no further sites (remaining warnings all pre-exist on base in files this wave never touched).
- Gate run 2 (head `51853980a`): `cargo build --workspace --tests` exit 0; `cargo test --workspace` (doctests included) — **502 suites, 12694 passed, 0 failed, 10 ignored**. No test-count drop vs the ~12.6k baseline (count is UP ~80 from this wave's new tests), so no missing-test-binary red flag.
- `CLAUDE_CODE_VERSION` was NOT bumped by this wave (stays `2.1.217`; the bump is the sibling session's M10, after both waves merge).
