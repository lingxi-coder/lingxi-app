# Three-repository structure migration — validation record

LingXi keeps product entrypoints and clients in its root workspace. Harness owns Agent, session, permission, tools, and platform composition. Mobile Linux SDK owns neutral execution interfaces, rootfs lifecycle, PTY, Android/iOS guest backends, and native support. Android/iOS host adapters retain native app capabilities while delegating Linux execution to the SDK.

Pinned sources:
- Mobile Linux SDK: `acdf47a4c01ce1c3ccab4e8aa3f8e51fcffac771` (draft PR #3).
- Harness: `5d2aa7178624c9b277c6173ea1f98386e706129c` (draft PR #3).
- llm-client: `9b0323f10f76c5834acafedaa04470c4e96346f2`.

Validation evidence:
- SDK: CI and real x86 guest runs passed at `acdf47a`; 56 Python tests, including stale iSH cache and missing-symbol negatives, passed. A rebuilt iOS native-support artifact passed Swift import, digest and policy-symbol checks; the prior unpatched artifact failed the new symbol gate.
- Harness: 21 CI jobs passed at `5d2aa7`, including Linux seccomp, Android/iOS compile and feature checks. Six job categories retain the same failures as the pre-structure migration commit: full lint, unit tests, parity fixtures, supply chain advisories, brand/skill gates, and Windows desktop.
- LingXi: 76 Harness and 7 SDK packages have one full-SHA source each. Root Cargo workspace/all-target check passed at the final pins with three pre-existing missing-docs warnings in test fixtures; `cargo fmt --all -- --check` passed. Final-pin Android direct/play native support, Kotlin/Gradle unit tests and APK helper checks passed. `com.lingxi.code.direct.debug` installed and started on the connected SM-S9310 (Android 36); `com.lingxi.code.debug` installed and started on an isolated Android 37 emulator with 16 KiB pages. The standalone clean-SDK sample consumed the final-pin full AAR and passed ten real-rootfs checks on the phone and on the emulator with R8 minification. LingXi's Maven staging was then restored to native-support-only and reverified. The earlier pin also executed PRoot/mksh on the phone and PRoot/Toybox on an emulator. Device instrumentation compilation is blocked by stale voice adapter references in three pre-structure test files; these errors were present in the preparation baseline.
- iOS: final-pin native support, rootfs supply chain and all three Rust/XCFramework slices passed. The FullDebug app linked and signed successfully with the native policy hooks, and `com.lingxi.code.full` installed on the iPhone 11 without changing the Store app. The phone was locked when launch was attempted, so final-pin guest execution remains unverified. The final-pin simulator ran 207 focused tests: 205 passed and the same two assertions failed as on the pre-split source.
- macOS: `npm run package:mac:flare -- --check` passed for Team AZ4AX7J833. Full signed package and `verify:package` remain pending.
- Product `scripts/check-all.sh`: five of six gates pass; brand baseline remains red. The pre-structure primary checkout already had five new and two stale brand entries. The structural staging tree exposes additional intentional product paths and currently reports 19 new and 10 stale entries; none were auto-accepted into the baseline.

Rollback: the SDK and Harness changes are isolated in draft PR branches. Reverting the eventual LingXi structural switch commit restores the prior `lingxi-code/` workspace and dependency layout. No user data directory or serialized format is migrated. The existing primary LingXi working tree and its concurrent translation/resource changes have not been overwritten.
