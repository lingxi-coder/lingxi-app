# Provider multi-account source delivery, 2026-11-01

Providers and accounts belong to the Host, independently of project sessions. API keys and OAuth grants have separate secure slots, identity generations and credential revisions. Stable user priority is preserved while unavailable accounts are skipped. Model-specific permissions, endpoint/model cooldowns, credential rotation and account-pinned resources remain scoped to the selected account.

This is a current-format protocol replacement. Obsolete singleton provider credential commands and client adapters are removed together across Desktop, Android, iOS and CLI. Provider settings use the checked Host-wide API, including managed policy and concurrent-update checks.

Desktop settings show configuration status and enabled/total account counts. Account dialogs explicitly center despite global CSS resets. OpenAI API and ChatGPT OAuth model choices share one provider group, with distinct billing/connection sections and eligible connections first. Selection retains the exact execution binding; an unverified entitlement is not a denial, and OAuth never silently becomes metered API usage.

## Current evidence

- Desktop Node and renderer type checks passed. The final delivery-focused Node suite passed 112 tests.
- The protocol client suite passed 347 tests with 5 skips in an isolated checkout using the pinned Harness snapshots.
- Model catalog logic passed 25 tests; three real Electron picker scenarios passed, including OAuth-only selection without redirecting to settings. The account renderer scenario also passed with production global styles.
- The native Desktop bridge and private Team executables compiled. The normal Flare packaging wrapper produced a signed app; deep/strict signature and static package checks passed.
- Original-profile Desktop OAuth A survived restarts. The signed app showed real OpenAI and DeepSeek account configuration. In the original `yoyo` workspace, selecting ChatGPT subscription GPT-6 Astra was confirmed by the model control and the checked subscription row; the API row stayed unchecked. No new inference message was sent for that selection check.
- Dependency delivery pins use immutable SDK and Harness Git commits. Lockfile verification runs outside local Cargo path overrides; this is dependency-resolution evidence, not another native package build.

## Open acceptance items

- Full packaged runtime smoke has not passed. The latest run timed out waiting for the Apps library; an earlier run intermittently timed out on an authenticated request after restart. Their causes are not established as fixed.
- A second real OAuth identity, live multi-account failover/recovery, and per-account model denial still require final end-to-end acceptance.
- Android, iOS and CLI changes are source integration only for this delivery. Their latest final native/package/device acceptance is incomplete. In particular, the iOS account authorization UI/browser-flow regression remains open; prior platform test/build results must not be treated as acceptance of the final source.

Do not describe this source merge as a fully validated four-platform release.
