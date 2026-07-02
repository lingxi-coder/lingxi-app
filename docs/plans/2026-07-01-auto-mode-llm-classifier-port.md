# Auto-Mode LLM Classifier — 1:1 Parity Port Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Port Claude Code's LLM-based auto-mode ("yolo") permission classifier (`claude-code/src/utils/permissions/yoloClassifier.ts`, ~1496 LOC) to Rust so the permission layer's auto-mode verdict is produced by an Opus classification call — a 1:1 behavioral copy of the pinned reference — replacing the current deterministic offline classifier.

**Architecture:** The leaf `permission` crate cannot call `llm-client`/`sidequery` directly, and the current integration point (`policy_gate.rs:312`) is synchronous. So: (1) the **pure** parts (prompt-template consts, transcript/action encoding, system-prompt assembly, `classify_result` XML parse, verdict types) live in the `permission` crate and are unit-testable offline; (2) a `ClassifierBackend` **trait** (dependency inversion) defined in `permission` abstracts the model call; (3) the sidequery-backed **impl** lives in a crate that already depends on `sidequery` (the async caller of the permission gate — orchestrator/engine), reusing the existing `SideQueryClient` seam; (4) classifier resolution **moves out of the synchronous `policy_gate`** into the async permission-check caller (mirroring CC, where `classifyYoloAction` is awaited in `permissions.ts:693` / `AgentTool/agentToolUtils.ts:410`), with the offline classifier retained as the deterministic fallback when no backend is wired.

**Tech Stack:** Rust, `sidequery` (`SideQueryClient` trait @ `sidequery/src/side_query.rs:80`, `SideQueryRequest` @ `:21`, `ProviderSideQueryClient` @ `provider_side_query.rs:75`), `serde_json`, existing `permission` verdict types.

**Reference (port verbatim from these locations — do NOT paraphrase prompt text):**
- `claude-code/src/utils/permissions/yoloClassifier.ts` — templates + flow (line anchors below).
- `claude-code/src/utils/permissions/autoModeState.ts` (40), `classifierApprovals.ts` (89), `dangerousPatterns.ts` (81).
- Callers: `claude-code/src/utils/permissions/permissions.ts:693`, `claude-code/src/tools/AgentTool/agentToolUtils.ts:410`.

**Parity baseline:** pinned reference = the 2026-03-31 snapshot in `claude-code/` (~2.1.195). "Latest" = this pinned tree (no newer source available). Confirm each ported const is byte-identical to its source.

**Worktree:** `.worktrees/parity-permission-classifier` (branch `parity/permission-classifier`; permission crate 641 tests green at baseline).

---

## Phase 0 — Preconditions (verify before coding)

### Task 0.1: Confirm the LLM seam + model selector exist
**Files:** read-only — `sidequery/src/side_query.rs`, `provider_side_query.rs`; grep for the classifier model.
- Confirm `SideQueryClient::query(&self, req: SideQueryRequest) -> Result<..>` shape and that `SideQueryRequest` carries system prompt, messages, tools, tool_choice, stop_sequences.
- Find the Rust equivalent of CC `getClassifierModel()` (grep `classifier.*model`, `sonnet`, `opus`, model-alias table in `llm-client`/`agent/model_resolution.rs`). If absent, Task 4.x adds a `classifier_model()` selector mirroring `yoloClassifier.ts` `getClassifierModel`.
- Confirm `permission/Cargo.toml` does NOT depend on `sidequery`/`llm-client` (it must stay a leaf; the trait seam is why).
**Done:** notes captured; no code.

### Task 0.2: Identify the async permission-check caller (where classifier resolution moves)
**Files:** read-only — grep callers of `policy_gate` / `evaluate_permission` in `orchestrator/`, `engine/`, `tool-api/`.
- Find the async function that today calls the synchronous permission gate and has access to the conversation `messages` + an `Arc<dyn SideQueryClient>` (or can be given one). This is the CC `permissions.ts` analogue. Record its path:line.
**Done:** the integration site is identified and recorded in this plan before Phase 4.

---

## Phase 1 — Pure prompt/type layer (permission crate, offline-testable)

> Port the pure, deterministic pieces first. Each is verbatim from CC and unit-testable with NO LLM.

### Task 1.1: Verdict + result types
**Files:** Modify `permission/src/classifier.rs`; Test: same file `#[cfg(test)]`.
- Add `pub struct YoloClassifierResult { pub should_block: bool, pub reason: String, pub model: String, pub unavailable: bool }` mirroring CC `YoloClassifierResult` (yoloClassifier.ts return shape, see `classifyYoloAction` doc @ ~999-1011: on API error → `should_block:true, unavailable:true`).
- Keep existing `AutoModeClassifierVerdict` (offline fallback) — do not delete.
- **Step 1 (test):** assert the struct's `Default`/construction + serde round-trip if serialized.
- **Step 2:** run `cargo test -p permission classifier::` → fail. **Step 3:** implement. **Step 4:** pass. **Step 5:** commit `feat(permission): YoloClassifierResult type (classifier parity P1.1)`.

### Task 1.2: Prompt-template consts (VERBATIM port)
**Files:** Create `permission/src/classifier_prompt.rs`; register `mod classifier_prompt;` in `permission/src/lib.rs`.
- Port, byte-for-byte, from `yoloClassifier.ts`: `BASE_PROMPT`, `EXTERNAL_PERMISSIONS_TEMPLATE`, and the tagged default rule bodies (`user_allow_rules_to_replace` / `user_deny_rules_to_replace` / `user_environment_to_replace`) into Rust `pub const`/`&str`. Copy the exact text from the reference file — do not rewrite or "improve" it; the LLM's behavior depends on byte-identical wording.
- Port `getDefaultExternalAutoModeRules` (@100) and `buildDefaultExternalSystemPrompt` (@125): the tag-extraction + template-substitution logic → Rust functions.
- **Test:** `build_default_external_system_prompt()` output length + that it contains each `<...>` section, and `default_external_auto_mode_rules()` returns the expected allow/deny/environment bullet vectors (assert counts + a couple known bullets).
- TDD steps 1-4; **commit** `feat(permission): auto-mode prompt templates + rule extraction (P1.2)`.

### Task 1.3: `classify_result` tool schema + XML parse
**Files:** Modify `permission/src/classifier_prompt.rs` (or new `classifier_parse.rs`); Test: same file.
- Port `YOLO_CLASSIFIER_TOOL_NAME` (@260 = `"classify_result"`) and its input schema.
- Port the XML/verdict parsing used by `classifyYoloActionXml` (@711) — extract the block/allow decision + reason from the model's `classify_result` output.
- **Test:** feed representative model outputs (a block verdict, an allow verdict, a malformed one) → assert `YoloClassifierResult`. Malformed → treat as `unavailable` (matches CC error contract).
- TDD 1-4; **commit** `feat(permission): classify_result schema + verdict parse (P1.3)`.

### Task 1.4: Transcript + action encoding
**Files:** Create `permission/src/classifier_transcript.rs`; Test: same file.
- Port `buildTranscriptEntries` (@302), `buildTranscriptForClassifier` (@434), `formatActionForClassifier` (@1487), and the `toCompact` / `toAutoClassifierInput` action-encoding contract (empty string `''` = "no classifier-relevant input" → short-circuit `should_block:false`, see `classifyYoloAction` @~1020-1027).
- **Test:** given a synthetic message list + a tool action, assert the produced transcript entries + compact action string match expected; assert the empty-action short-circuit.
- TDD 1-4; **commit** `feat(permission): classifier transcript + action encoding (P1.4)`.

### Task 1.5: System-prompt assembly (`buildYoloSystemPrompt`)
**Files:** Modify `permission/src/classifier_prompt.rs`; Test: same file.
- Port `buildYoloSystemPrompt` (@484): assemble BASE_PROMPT + permissions template + user rule overrides (from `ToolPermissionContext` / settings) into the final system prompt string. Thread the Rust permission-context equivalent (the `Bash(prompt:)` rules extraction referenced in `classifyYoloAction` docs).
- **Test:** default (no overrides) equals `build_default_external_system_prompt()`; with a user allow/deny override, the section is replaced.
- TDD 1-4; **commit** `feat(permission): buildYoloSystemPrompt assembly (P1.5)`.

---

## Phase 2 — `ClassifierBackend` trait seam (dependency inversion)

### Task 2.1: Define the backend trait in the permission crate
**Files:** Create `permission/src/classifier_backend.rs`; register in `lib.rs`; Test: same file with a mock.
- Define:
  ```rust
  #[async_trait::async_trait]
  pub trait ClassifierBackend: Send + Sync {
      /// Run the assembled system prompt + transcript + action through the model,
      /// returning the raw model output for XML parse. Errors → caller maps to unavailable.
      async fn classify(&self, req: ClassifierRequest) -> Result<String, ClassifierBackendError>;
  }
  ```
  `ClassifierRequest` carries: system_prompt, transcript messages, action content block, model id, tool schema, abort token.
- Add `add async-trait` to `permission/Cargo.toml` (already a workspace dep elsewhere — confirm). Permission crate still does NOT depend on sidequery/llm-client.
- **Test:** a mock `ClassifierBackend` returning a canned block verdict → end-to-end pure path: `assemble prompt → mock.classify → parse → YoloClassifierResult{should_block:true}`.
- TDD 1-4; **commit** `feat(permission): ClassifierBackend trait + pure classify pipeline (P2.1)`.

### Task 2.2: The pure orchestration function
**Files:** Modify `permission/src/classifier.rs`; Test: same file.
- Add `pub async fn classify_yolo_action(backend: &dyn ClassifierBackend, ctx: ClassifierCallCtx) -> YoloClassifierResult` mirroring CC `classifyYoloAction` (@1012): empty-action short-circuit → assemble system prompt (P1.5) → transcript (P1.4) → `backend.classify()` → parse (P1.3) → result; on `Err` → `{should_block:true, unavailable:true}` (CC error contract).
- **Test:** with mock backends (allow / block / error) assert the three outcomes incl. `unavailable`.
- TDD 1-4; **commit** `feat(permission): classify_yolo_action orchestration (P2.2)`.

---

## Phase 3 — sidequery-backed backend impl

### Task 3.1: Implement `ClassifierBackend` over `SideQueryClient`
**Files:** Create the impl in the crate that owns sidequery wiring (per Task 0.2 — likely `sidequery/src/classifier_backend_impl.rs` or `orchestrator/`); Test: same file with a mock `SideQueryClient`.
- Implement `ClassifierBackend::classify` by building a `SideQueryRequest` (system prompt, transcript messages, action block with `cache_control` per CC @~1101-1108, the `classify_result` tool + `tool_choice`, `getClassifierModel()` model) and calling `SideQueryClient::query`; return the model's tool-output text.
- Port `getCacheControl({querySource:'auto_mode'})` (@~1099) behavior (or the minimal cache_control block).
- **Test:** mock `SideQueryClient` asserts the request carries the right system prompt/model/tool + returns a canned block → impl returns that text.
- TDD 1-4; **commit** `feat(sidequery): SideQuery-backed ClassifierBackend (P3.1)`.

### Task 3.2: Classifier model selector
**Files:** wherever model aliases live (`agent/model_resolution.rs` or `llm-client`); Test alongside.
- Port `getClassifierModel()` — the model id CC uses for classification (Opus-class). **Test:** returns the expected alias.
- **commit** `feat: classifier model selector (getClassifierModel parity) (P3.2)`.

---

## Phase 4 — Async integration (move resolution out of sync policy_gate)

### Task 4.1: Async classifier resolution at the caller
**Files:** Modify the async permission-check caller (Task 0.2 site); Modify `policy_gate.rs:298-344`; Test: caller's crate.
- In `policy_gate` (sync), keep the offline `classify_tool_call` ONLY as the fallback; expose the "this reason is classifier-eligible" signal (`reason_allows_classifier`) so the async caller knows to invoke the LLM classifier.
- In the async caller: when the gate yields a classifier-eligible Ask AND a `ClassifierBackend` is wired → `classify_yolo_action(backend, ctx).await`; map `should_block` → `ClassifierRejected`, `!should_block` → `ClassifierApproved` (reuse existing `PermissionDecisionReason` variants + the danger-rule strip/restore already in `policy_gate`). `unavailable` → fall back to the normal prompt gate (mirror CC "couldn't respond" path).
- **Test:** caller-level test with a mock backend: eligible Ask + block-backend → rejected; + allow-backend → approved; + error-backend → prompt fallback; no backend → offline fallback (existing behavior preserved).
- TDD 1-4; **commit** `feat: wire LLM classifier into async permission check (P4.1)`.

### Task 4.2: Compose the backend at the engine root
**Files:** `apps/engine-desktop/src/lib.rs` (+ `engine-mobile` if applicable); Test: composition smoke.
- Construct the sidequery-backed `ClassifierBackend` from the live `SideQueryClient` and inject it into the permission-check caller. `None` (tests/headless) → offline fallback stays.
- **Test:** engine builds; a smoke test that auto-mode with a stub backend yields a classifier decision.
- **commit** `feat(engine): compose auto-mode classifier backend (P4.2)`.

---

## Phase 5 — Fidelity extras + error/telemetry parity

### Task 5.1: Two-stage XML classifier option
- Port `isTwoStageClassifierEnabled()` / `getTwoStageMode()` gating (@~1112) + `classifyYoloActionXml` (@711) two-stage path. If LingXi has no GrowthBook, gate behind an env/config flag defaulting OFF (single-stage), documented. TDD + commit.

### Task 5.2: Dump + error-transcript files
- Port `maybeDumpAutoMode` (env `CLAUDE_CODE_DUMP_AUTO_MODE`/LingXi equivalent), `getAutoModeClassifierErrorDumpPath` (@186), `getAutoModeClassifierTranscript` (@200). Default OFF. TDD + commit.

### Task 5.3: `classifierApprovals.ts` + `dangerousPatterns.ts` + `autoModeState.ts`
- Port these three small files (approval bookkeeping, dangerous-pattern list, auto-mode state) to their Rust equivalents; reconcile with existing `dangerous_patterns.rs` (83 LOC) — extend to match CC's list byte-for-byte. TDD + commit each.

---

## Phase 6 — End-to-end parity verification

### Task 6.1: Golden parity fixtures
**Files:** `permission/tests/auto_mode_classifier_parity.rs`.
- Build fixtures: for a set of (messages, action) inputs, assert the assembled **system prompt + transcript + request** are byte-identical to what CC would send (derive expected from the reference by running the TS if possible, else hand-verified snapshots). Lock with `insta` or literal asserts.
- Assert verdict mapping for canned model outputs.
- **commit** `test(permission): auto-mode classifier parity fixtures (P6.1)`.

### Task 6.2: Full suite green + report update
- `cargo test -p permission` + the caller crate + `engine-desktop` green.
- Update `docs/gap-report-2026-07-01-cc2.1.195-reconciled.md`: classifier row → ✅ DONE (LLM parity) with the caveat that determinism/offline behavior is now replaced by the CC LLM path.
- **commit** `docs: mark auto-mode classifier at LLM parity`.

---

## Risks / decisions to confirm during execution
1. **Async boundary:** if the permission-check caller is NOT already async or lacks a `SideQueryClient`, Task 4 grows — may require threading the backend through the tool-use context. Task 0.2 must nail this before Phase 4.
2. **Prompt fidelity is load-bearing:** the classifier's decisions depend on byte-identical prompt text. Phase 1.2/1.5 must copy verbatim from the reference; any drift changes behavior. Add a test asserting the ported const length matches the source.
3. **Non-determinism:** this replaces a deterministic offline classifier with a network LLM call — auto-mode tests that asserted deterministic verdicts must move behind the mock backend; the live path is inherently model-dependent. Flag any locked fixtures that assumed offline determinism.
4. **Cost/latency:** every auto-mode Ask now triggers an Opus call. Confirm this matches intended product behavior (it's what CC does).
5. **`getClassifierModel` availability:** requires an Opus-class model alias resolvable in LingXi's provider layer.

## Execution note
This is a large port (~1500 LOC + cross-crate async architecture). Recommend executing Phase 1 fully (pure, offline, high-confidence) before committing to Phases 3-4 (the architectural change), so the risky async wiring is attempted only after the pure layer is proven.
