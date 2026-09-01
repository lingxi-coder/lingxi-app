# Multi-Agent LLM Seam Wiring Plan (P8 spike)

Branch: `multi-agent-impl`. Investigation only — no production code changed.
Sanity gate `cargo check -p multi-agent`: PASS (warnings only, no errors).

## Goal

Make the three `// TODO(multi-agent)` seams real, all of which need the SAME
capability — *drive a file-editing agent multi-turn loop pinned to a cwd and
produce a git diff of what changed*:

1. `orchestrator::TodoLlmCandidateRunner` (impl `CandidateRunner`)
2. `revision::TodoLlmReviser` (impl `Reviser`)
3. `verification::NoopFixer` (impl `VerificationFixer`)

## Key finding: the right driver is `platform_api::SubagentSpawner`, not `agent::run_subagent`

`run_subagent`/`run_subagent_loop` (agent/src/runner.rs) is a LOW-level future
the pool hands to the runtime. Driving it directly would mean reconstructing the
entire `SubagentContext` (28 fields: api_client, agent_definition, tool registry
resolution, permission policy, hook_executor, transcript_subdir, budget, env
renderer, …) plus a `StateMachinePool` slot and an event-pump loop — exactly the
plumbing `agent::handle::PoolSubagentSpawner` already encapsulates.

`PoolSubagentSpawner` already exposes the precise surface we need via the
cross-crate `platform_api::subagent_spawn::SubagentSpawner` trait:

```rust
async fn spawn(&self, request: SubagentSpawnRequest, inherit: SubagentInheritance)
    -> Result<SubagentResult, SubagentSpawnError>;
```

- `SubagentSpawnRequest.cwd: Option<String>` → the runner threads it onto every
  dispatched tool's `SubagentInvocationContext.cwd` → `ToolUseContext.cwd`, so a
  file-editing agent's Edit/Write/Bash run inside the candidate worktree
  (context.rs `cwd` field doc; runner.rs:1031-1033). **This is the cwd-pin we
  need** — no new infra.
- `SubagentSpawnRequest.subagent_type` resolves a real `AgentDefinition`
  (general-purpose gives All-tools incl. Edit/Write/Bash) with a model override
  via `request.model`. (Model-family override is `"sonnet"|"opus"|"haiku"`; the
  multi-agent crate's per-candidate provider/model is richer — see "Model
  routing gap" below.)
- `SubagentResult::Completed { content, usage: SubagentUsage{ input_tokens,
  output_tokens, cache_creation_input_tokens, cache_read_input_tokens,
  total_tokens }, .. }` → maps cleanly to `llm_client::TokenUsage`.
- `spawn` internally allocates a pool slot, pumps `SubagentEvent` to terminal,
  and deallocates — so the adapter needs no event loop.

The diff is NOT in the spawn result (the agent returns text). The adapter
extracts it by running `git` in `ctx.cwd` after the spawn completes — design doc
§artifact table line 382 ("self-report = candidate agent 最后一轮输出，或 host 从
git diff + logs 生成"). The `git` CLI pattern is already in this crate:
`finalizer::GitPatchApplier::run_git` (finalizer.rs:313-347).

## (a) Build a cwd-pinned file-editing agent

The adapter holds an injected `Arc<dyn platform_api::SubagentSpawner>` +
`SubagentInheritance { tool_invoker, budget }` (the same Arcs the main session
holds — recursion-lock + budget inheritance). Per candidate:

```rust
let req = SubagentSpawnRequest {
    subagent_type: "general-purpose".into(),     // All-tools (Edit/Write/Bash)
    prompt: implementer_or_reviser_prompt,        // (b) below
    cwd: Some(ctx.cwd.display().to_string()),     // PIN to the worktree
    model: model_family_for(&ctx.resolved),       // see Model routing gap
    description: Some("dual-llm candidate".into()),
    ..SubagentSpawnRequest::default_for_spawn()    // all other fields None/empty
};
let result = spawner.spawn(req, inherit.clone()).await?;
```

The agent's writes are confined to `cwd` by the runner threading
`SubagentInvocationContext.cwd`; the multi-agent crate ADDITIONALLY enforces
isolation by extracting the diff only from that cwd and rejecting an empty diff
(orchestrator.rs:378).

## (b) Feed the implementer / reviser / fixer prompt

- Candidate runner: prompt = `prompts::IMPLEMENTER` + `ctx.task_brief`
  (multi-agent/src/prompts.rs already embeds the four phase prompts as `&str`
  constants via include_str!). Seeded as `request.prompt` (the first user
  message; the agent definition body stays the system prompt).
- Reviser: prompt = `prompts::REVISER` + `ctx.task_brief` + `ctx.review_feedback`
  + `ctx.pre_revision_patch` ("revise YOUR branch per this review"). cwd =
  author's own worktree.
- Fixer: prompt composed from `ctx.failure_log` + "make the verification command
  pass". cwd = `ctx.workspace` (winner worktree / final workspace, single-writer).

## (c) Observe to completion + map usage → TokenUsage

`spawn` already pumps to terminal. The adapter matches `SubagentResult`:

- `Completed { usage, .. }` → `TokenUsage { input: usage.input_tokens, output:
  usage.output_tokens, cache_read: usage.cache_read_input_tokens, cache_write:
  usage.cache_creation_input_tokens, reasoning_output: 0 }`. VERIFIED field
  names: `llm_client::TokenUsage` (types.rs:54) = `{ input, output, cache_write,
  cache_read, reasoning_output }` (all `u64`); `SubagentUsage` =
  `{ total_tokens, input_tokens, output_tokens, cache_creation_input_tokens,
  cache_read_input_tokens }`. `content` text → the `self_report`
  (CandidateRunner) / `note` (Reviser).
- `Failed { reason }` → `MultiAgentError::CandidateFailed { reason, .. }` /
  recoverable error for the reviser (revision.rs keeps the pre-revision patch).
- `Killed` → `MultiAgentError::Cancelled`.

## (d) Extract the unified diff (git diff in the cwd)

Candidate worktrees are created from a baseline; the agent's edits are
uncommitted working-tree changes. To capture BOTH modified-tracked and new files
as one unified diff:

```text
git -C <cwd> add -A          # stage everything the agent wrote (incl. new files)
git -C <cwd> diff --cached    # unified diff vs the worktree's HEAD baseline
```

(Use `git diff --cached` after `add -A` so untracked/new files appear in the
patch; a bare `git diff` omits untracked files.) This produces the `patch.diff`
artifact the orchestrator persists and the finalizer later `git apply`s to the
main workspace. Reuse the `GitPatchApplier::run_git` spawn/stdin/exit pattern
(finalizer.rs) — a tiny `run_git(cwd, &["add","-A"], None)` then
`run_git(cwd, &["diff","--cached"], None)` reading stdout.

`CandidateRunContext.cwd` is the worktree path; the worktree provisioner
(worktrees.rs) already created it via `WorktreeManager::create_worktree`, so it
has a committed HEAD baseline to diff against.

## (e) Honor the CancellationToken

`CandidateRunContext.cancel` / `RevisionContext.cancel` is a
`tokio_util::sync::CancellationToken`. The `SubagentSpawner` trait surface does
NOT take a token. Two compatible mechanisms, both already in place:

1. **Outer timeout/cancel race (primary).** `orchestrator::run_one` and
   `revision::revise_author` already wrap the seam call in
   `tokio::time::timeout(...)` and fire `ctx.cancel.cancel()` on timeout. The
   adapter additionally races the spawn against `ctx.cancel.cancelled()`:
   `tokio::select! { r = spawner.spawn(..) => r, _ = ctx.cancel.cancelled() => Err(Cancelled) }`.
   Dropping the `spawn` future drops the pool slot's receiver; the runner's
   in-flight stream future is cancel-safe (runner.rs drops the API future on the
   termination arm).
2. **In-loop UserExit (best-effort, runtime-only).** The runner's loop races
   `lingxi_core::Event::UserExit/UserInterrupt` on its event channel → `Killed`. A
   future enhancement could bridge `cancel` → a `UserExit` on the slot's event
   channel, but `SubagentSpawner::spawn` does not expose the channel, so the
   select-drop in (1) is the achievable cancellation for this seam.

## (3) WHERE the real adapter lives — new module in `apps/engine-desktop`

**Decision: a new module `apps/engine-desktop/src/multi_agent_runtime.rs`**, NOT
a new crate, NOT inside the `multi-agent` crate.

Justification:
- The `multi-agent` crate must keep its injected traits (`CandidateRunner`,
  `Reviser`, `VerificationFixer`) and must NOT depend on `agent` — adding `agent`
  as a dep of `multi-agent` would (a) violate the stated crate-dep boundary
  ("multi-agent deps today: traits, llm-client, sidequery, protocol, cost — NOT
  agent") and (b) drag the whole pool/runner/hook surface into a crate that is
  meant to stay thin and trait-driven (same discipline as review.rs taking
  `Arc<dyn SideQueryClient>`, finalizer taking `Arc<dyn PatchApplier>`).
- The adapter only needs the CROSS-CRATE `platform_api::SubagentSpawner` (not `agent`
  internals). `engine-desktop` is where `subagent_spawner_arc:
  Arc<PoolSubagentSpawner>` is already assembled with its real api_client, tool
  registry, permission policy, hook executor, env renderer
  (lib.rs:2421-2491). The adapter wraps that same `Arc<dyn SubagentSpawner>` +
  the session's `SubagentInheritance` (tool_invoker + budget_enforcer already
  built at lib.rs:2499) — zero new plumbing.
- A separate `multi-agent-runtime` crate is unnecessary: the adapter is ~3 small
  structs implementing 3 traits over one injected `Arc<dyn SubagentSpawner>` +
  the git-diff helper. It has no consumers other than the engine-desktop
  composition root that also constructs `DualLlm`. Keeping it a module avoids a
  new workspace member + Cargo wiring for no isolation benefit. (If a second host
  — engine-mobile — later needs it, promote the module to a crate then; mobile
  passes spawner=real too, so the trait-only adapter would move cleanly.)

The adapter file defines:
```rust
pub struct SpawnerCandidateRunner { spawner: Arc<dyn SubagentSpawner>, inherit: SubagentInheritance }
pub struct SpawnerReviser        { spawner: Arc<dyn SubagentSpawner>, inherit: SubagentInheritance }
pub struct SpawnerVerificationFixer { spawner: Arc<dyn SubagentSpawner>, inherit: SubagentInheritance }
async fn worktree_diff(cwd: &Path) -> Result<String, ..>;  // git add -A && git diff --cached
fn map_usage(u: &SubagentUsage) -> TokenUsage;
fn model_family_for(resolved: &ResolvedCandidate) -> Option<String>;
```
`DualLlm::new(.., runner: Arc::new(SpawnerCandidateRunner{..}), ..)` is then
constructed at the composition root GATED behind `decide_dispatch(..) ==
Dispatch::DualLlm` (execution.rs) — off by default. The multi-agent pipeline is
NOT wired into engine-desktop today (only a comment at lib.rs:1122), so this
addition cannot regress the baseline turn loop.

## Model routing gap (call out, do not fake)

The multi-agent crate resolves a RICH per-candidate route
(`ResolvedCandidate` = provider id + wire model, via `providers::ModelResolver`),
but `SubagentSpawner::spawn` only accepts a `model: Option<String>` family alias
(`sonnet|opus|haiku`) resolved against the spawner's single `default_model`. So a
candidate pinned to e.g. `profile-b/model-b` (a different provider) cannot be
expressed through the spawner seam as built today.

Options (decide at impl time, not in this spike):
- **MVP:** run both candidates through the one wired `subagent_api`
  (`ProviderApiAdapter`), mapping each candidate's tier to a family alias. This
  gives dual-CANDIDATE (two parallel agent runs, two diffs, cross-review,
  arbitration) but NOT dual-PROVIDER. Honest + shippable; matches MVP scope
  (design doc §最小可行版本 — "多于两个 provider" is explicitly out of MVP).
- **Full:** give the adapter its own per-candidate `Arc<dyn SubagentApiClient>`
  built from `ResolvedCandidate` (the multi-agent crate already has the provider
  routing; engine-desktop builds `ProviderApiAdapter` per provider). This needs
  either a spawner variant that accepts an explicit api_client per spawn, or the
  adapter to drive a per-candidate `PoolSubagentSpawner` configured with that
  candidate's api_client. Larger; defer past MVP.

## (4) Unit-testable with a mock spawner vs runtime-only

**Unit-testable (deterministic, with a mock `Arc<dyn SubagentSpawner>`):**
- The adapter calls `spawn` with the correct `cwd`, `subagent_type`, prompt, and
  inherited Arcs (a recording mock asserts the request fields + `Arc::ptr_eq` on
  inheritance — the existing `traits` test-mock pattern).
- `SubagentResult` → outcome mapping: `Completed` → CandidateOutcome/RevisedOutcome,
  `Failed` → recoverable error, `Killed` → Cancelled.
- `map_usage` field-by-field.
- `worktree_diff` against a REAL temp git repo (init, commit baseline, write a
  file, assert the produced unified diff contains the new file) — this is a real
  `git` integration test, not a mock, and is fully deterministic (the same style
  as finalizer's GitPatchApplier git-CLI tests).
- Empty-diff / cancellation-race paths (mock spawner that returns empty content
  with no file writes → empty diff → CandidateFailed; a cancel fired before
  spawn → Cancelled).

**Runtime-only (live provider — call out, do not fake green):**
- That a real model actually edits files in the worktree (the end-to-end "agent
  produces a meaningful diff"). The mock spawner can SIMULATE writes (write a
  file into `ctx.cwd` then return Completed, exactly as
  `MockCandidateRunner::Succeed` does today), so the DIFF-EXTRACTION and
  orchestration are tested, but the QUALITY/CONTENT of a live edit is not.
- Real token usage numbers / real provider streaming behavior.
- The full dual-LLM pipeline against two live providers (model routing gap).

## TDD order (per seam)

1. Add `SpawnerCandidateRunner` with a failing test: mock spawner that writes a
   file into `ctx.cwd` + returns `Completed`; assert `run()` returns a
   non-empty `patch_diff` containing that file and mapped usage. Watch it fail
   (adapter unimplemented) → implement `spawn` call + `worktree_diff` → pass.
2. Same for `SpawnerReviser` (prompt carries review_feedback; cwd = own worktree)
   and `SpawnerVerificationFixer` (cwd = workspace; Failed → loop marks failed).
3. Compose in engine-desktop behind the gated `decide_dispatch` (off by default);
   `cargo check -p multi-agent && -p engine-desktop` + `cargo test` for touched
   crates as the gate (NOT clippy -D warnings — pre-existing dep lints).
