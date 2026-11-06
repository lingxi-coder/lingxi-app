# Bounded backend cleanup

Scope: this task's new backend execution/contracts/eligibility/cleanup files, market gateway changes, shared Chat source and inventory script. No concurrent Bot creation files or unrelated maintenance code.

Behavior lock: 15 contract/gateway/eligibility unit tests, seven Chat source tests, 82 broad Emulator cases, then 21 execution regression cases after independent review corrections; all passed without skips.

Plan, before cleanup edits:

1. Inspect removed execution exports and stale compatibility branches; retain only rejection vectors and explicit offline deletion selectors.
2. Remove the repeated read-action classification in `shared-bots.mjs`, keeping current ACL rechecks and mutation receipt behavior identical.
3. Inspect error messages/comments and new boundaries for obsolete protocol statements. Do not rewrite established source-cache or transaction abstractions without evidence.
4. Re-run unit/source/retired-symbol checks, then the final Emulator suite. No new dependencies or deleted behavior tests.

Independent review fixes are behavioral work with dedicated regression cases, not cleanup: retryable start receipts, host discovery pagination, history upper bounds, staged publication and concurrent install deduplication.

Result: replaced duplicate read-action lists with one module-local set; no extra abstraction/dependency. The live retired-symbol scan passes with two exact non-execution exclusions. Unit/source checks pass; final full Emulator suite after cleanup passes 85 backend plus eight SDK/UI tests, zero skipped (`/tmp/teams-root-all-final.log`). The seven shared-source tests also pass. Package/device/production checks remain separate open gates.
