# Auto-Mode Permission Classifier — Parity Audit & Design Conclusion (2026-07-01)

- **Status:** DECISION REQUIRED (product/architecture). No implementation pending.
- **Worktree:** `.worktrees/parity-permission-classifier` (WIP snapshot; permission crate 641 tests green; main tree undisturbed).
- **Scope audited:** the auto-mode ("yolo") permission classifier. Bash-AST-security parity was NOT audited (separate slice — see "Next options").

## Audit finding

The Rust auto-mode classifier and Claude Code's classifier differ at the **mechanism** level, and the divergence is **deliberate and documented** — not incomplete work.

| Aspect | Claude Code (`src/utils/permissions/yoloClassifier.ts`, ~1496 LOC) | Rust (`permission/src/classifier.rs`, ~441 LOC) |
|---|---|---|
| Decision engine | LLM-based: `classifyYoloAction` builds a system prompt + transcript and calls **Opus** via a `classify_result` tool | **Deterministic, offline** rule table: `classify_tool_call` dispatches per tool (read-only allow, shell hard/soft-deny, file-mutation, web, agent) |
| Rule source | Prompt template + `getDefaultExternalAutoModeRules()` consumed by the model | Hard-coded Rust rule fns + `critique_rules()` structural linter |
| Determinism | Non-deterministic (model), network-dependent | Deterministic, offline, no network from the permission layer |
| Verdict shape | allow / block via model XML | `AutoModeClassifierVerdict::{Allow, Deny{hard}, Pass}` |
| Author intent | — | Module doc: *"deterministic and offline: applies the shipped auto-mode policy categories to the tool call shape instead of starting an LLM request from the permission layer."* |

The Rust classifier is enabled (`is_classifier_permissions_enabled() == true`), wired into `policy_gate`, and produces `ClassifierApproved`/`ClassifierRejected` decisions with the danger-rule strip/restore path — i.e. the *integration* is complete; only the *decision mechanism* differs.

## Conclusion

Making the classifier literally "1:1" with Claude Code means **porting the entire LLM-classifier pipeline** (system-prompt construction, transcript building, an Opus round-trip, `classify_result` XML parsing, `CLAUDE_CODE_DUMP_AUTO_MODE` req/res dumps, error-transcript files). That is:

1. A **large new feature**, not a "finish the WIP" edit.
2. A **behavioral regression risk**: it makes permission decisions non-deterministic and network-dependent from the permission layer — the opposite of the current documented design.
3. A **product/architecture decision**, which the engine owner must make deliberately. It should not be implemented unilaterally.

Therefore: **no implementation is proposed for the classifier here.** The offline classifier is a defensible, tested (641 green) design; unless the owner explicitly wants the LLM pipeline, this is a *deliberate LingXi divergence*, not a parity bug.

## Next options (owner's call)

1. **Bash-AST-security parity** — `permission/src/bash_ast_security.rs` (4159 LOC) vs CC's bash-security reference. Far more likely to contain genuine, closeable rule-coverage gaps (mechanical parity work). Recommended next audit if parity work continues.
2. **Adopt the LLM classifier** — if desired, this doc's finding becomes the input to a *new* brainstorming + writing-plans cycle for that feature (system prompt, transcript, Opus call, XML parse, dump files). Non-trivial.
3. **Accept offline classifier** — record as a deliberate divergence in `docs/gap-report-2026-07-01-cc2.1.195-reconciled.md` and pick a different slice.

## Worktree disposition

The isolated worktree remains in place (`.worktrees/parity-permission-classifier`). Tear down with `git worktree remove` + delete branch `parity/permission-classifier` if the classifier slice is not pursued.
