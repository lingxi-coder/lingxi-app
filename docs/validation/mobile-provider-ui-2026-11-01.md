# Mobile provider UI alignment — 2026-11-01

## Changes

- Android and iOS provider directories display Host account configuration status and enabled/total counts. OpenAI OAuth and API key accounts share a logical provider; model permission failures do not redefine provider configuration.
- Account reads refresh independently of project catalog refresh while the settings screen is visible. Unknown/read-failed states remain distinct from unconfigured.
- Model pickers keep OpenAI subscription and API key sections adjacent, place an eligible billing channel first, and preserve exact provider-qualified model references. Host-projected references remain subject to model eligibility and user visibility settings.
- Android account editors show progress/errors inside their dialog, require confirmation before removal, and collapse permission/model details. Permission and scoped-health rows match the current account identity generation.
- iOS uses human-readable account authentication labels and asks for fee consent before enabling subscription-to-metered fallback. Existing policy is preserved.

## Earlier validation boundaries (superseded by final native acceptance below)

- Android isolated JVM suite: 22 tests passed. The actual account repository and tests execute with a transport interface stub; grouping methods are extracted verbatim from production source with DTO substitutes. This does not exercise Compose or JNI.
- iOS isolated macOS Swift suite: 23 tests passed (22 actual account repository tests and one picker grouping test). Grouping/parsing methods are extracted verbatim with DTO substitutes. This does not establish iOS simulator/device behavior.
- Changed Swift files passed syntax parsing. `git diff --check` passed.
- Full native build attempts encounter existing Agent status/generated binding mismatches: Android generated `SessionAgentSummaryDto` converter arguments and missing `reportDelivery`; iOS `SessionAgentSummaryDto` missing `reportDelivery`. These unrelated working changes and generated artifacts were retained.
- No new mobile package installation, visual/device acceptance, real OAuth authorization, or live model inference is claimed.

Logs and isolated harnesses: `/Users/luolingfeng/lingxi/.buildcache/mobile-provider-ui-20261101/`.

## Review fixes

All four reported findings were addressed:

1. Android sibling preference inheritance no longer reuses the other execution binding's catalog membership. Exact-binding catalog restrictions, picker switches and allowlists still apply.
2. iOS resolves the exact saved execution channel's picker preferences first. Logical-provider preferences apply only when no exact channel exists.
3. iOS picker configuration summaries resolve execution bindings to logical providers using Host model availability or matching account metadata; no connection-name string splitting is used.
4. Android transport/read errors have a distinct localized `read_failed` state. Secure-storage status requires explicit Host account health evidence.

Latest targeted validation: Android 27 passing tests (22 account repository, 5 visibility); iOS 27 passing tests (23 account repository, 3 visibility, 1 grouping). These use the isolated JVM/macOS harnesses described above, not installed mobile apps. Regression coverage includes subscription-only models, exact-binding catalog rejection, independent hidden channels, allowlists, grouped connections resolved through account and model projections, and read errors versus explicit storage health.

The normal native suites contain the new regressions. They remain unexecuted as complete app suites because of the previously recorded generated Agent binding mismatch. Swift syntax parsing, Android resource XML parsing, and `git diff --check` passed after the fixes. No new native package/device acceptance is claimed.

Latest logs: `/Users/luolingfeng/lingxi/.buildcache/mobile-provider-ui-20261101/review-fixes/`.

## Final native and physical acceptance — 2026-11-01

The generated Agent binding blocker recorded above is resolved. The Rust record contains the optional `report_delivery` field; stale generated converters/native artifacts did not match that record. Kotlin and Swift bindings were regenerated from fresh native metadata and promoted together with their native libraries. No manual generated converter patch is required.

- Protocol contract index and version guard cover the optional field. Event JSON and DTO round trips preserve it: 25 event tests and 13 version guard tests passed.
- Android Direct and Play ARM64 debug APKs built successfully. Their packaged JNI libraries match the fresh native artifacts. The three affected JVM suites passed 143 tests (22 account, 8 visibility, 113 reducer).
- REDMI K80 passed 10 focused instrumentation cases: native reportDelivery converter (1), native account credential/rotation/rebuild roundtrip (1), model picker (6), and actual account create/edit/disable plus process-restart restore/delete (2 phases). Picker cases ran in isolated processes. Final account phase logs supersede earlier automation-conflict failures.
- iPhone 11 passed 122 hosted tests and 4 UI tests, 126 total, zero failures. Coverage includes real native reportDelivery buffer round trips, model selection, fixture OAuth error/identity flows, and native account persistence/removal across app restart.
- The iOS StoreDebug app built with signing and passed strict signature verification. The complete ARM64 device + ARM64 simulator XCFramework also built. Its Swift bindings match the tested device-only framework. All 4,230 device archive member names and payload hashes match; archive-container hashes differ due to header timestamps.
- Native input checkpoint: 6,594 files across Product/Harness/SDK, zero drift. Other concurrent UI work was preserved; this is not a claim that every working-tree UI file was frozen.

Test harness fixes include a bounded retry for native asynchronous teardown, a missing workspaceState test-double override, a canonical main-agent label assertion across handset locales, and exact picker/dialog matchers. Production native safety fences were retained. Earlier failed logs remain available for diagnosis.

No business data was cleared. Only synthetic fixture accounts were created and removed. This acceptance does **not** include real OAuth authorization or live model inference; OAuth UI tests use controlled fixtures. Android physical instrumentation used the Direct debug app; Play was separately built. Its final update installation returned `INSTALL_FAILED_UPDATE_INCOMPATIBLE` because the existing app uses a different signature. The existing Play app and its data were retained; the tested Direct app remains installed and was launched. iPhone Store was installed and launched.

Evidence root: `/Users/luolingfeng/lingxi/.buildcache/mobile-report-delivery-20261101/`.

Final evidence: `validation-receipt.json`, `final-packages.json`, `android-apk-verification.json`, `native-source-drift.json`, `ios-device-archive-comparison.json`, `ios-test-summary-retry.json`, `ios-device-tests-retry.xcresult`, `android-account-ui-phase1-final.log`, `android-account-ui-phase2-final.log`, and the six `android-isolated-picker-*.log` files. Earlier failing runs are historical, not the final result.

## Publication scope

The published change includes provider UI/review fixes, DTO binding regressions, optional reportDelivery storage/projection, and the Harness protocol contract plus constructor adaptation. Product dependencies are pinned to the published Harness commit `c3d58501f705e8fe22896422ca92bee50eb6fe13`; clean `cargo metadata --locked` resolved that exact remote source without local path patches. The independent report-delivery producer/lifecycle and status-badge UI work remains outside this delivery. Until that producer is published, runtime constructors emit no delivery status.

The physical-package evidence above was collected from the shared development checkout with other work present; it is not an exact-binary acceptance claim for this narrower publication. Protocol tests were repeated in an isolated publication snapshot: 25 event, 9 snapshot, and 13 version guard tests passed. Publication checks and preservation receipts are under `/Users/luolingfeng/lingxi/.buildcache/mobile-delivery-20261101/`.

The isolated publication snapshot also compiled Android main, JVM-test and instrumentation-test Kotlin sources, and passed all 143 affected JVM tests. This source-only Gradle run reused generated bindings and local third-party dependencies and omitted native-asset staging/verification; earlier APK/device evidence remains separate. Changed Swift sources passed parsing.
