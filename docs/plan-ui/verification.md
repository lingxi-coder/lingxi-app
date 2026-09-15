# Native Plan document UI

Implemented on Desktop, Android and iOS: lightbulb status, bounded Markdown preview with a bottom fade, full document opening and complete Markdown copying. Existing approval decisions remain available. Ordinary reasoning and prose are not classified as plans. Tool plans stay visible outside collapsed tool groups.

## Main files

- Desktop: `PlanDocument.tsx`, `Stage.tsx`, `RuntimeCenter.tsx`, `MarkdownContent.tsx`, `App.tsx`, `bridge/planDocuments.ts` under `clients/electron/src/renderer`.
- Android: `conversation/PlanDocument.kt`, transcript/message/tool projections and permission previews under `clients/android/app/src/main/java/com/lingxi/code`.
- iOS: `Conversation/PlanDocumentView.swift`, message/tool/history projections, session details and `App/RootView.swift` under `clients/ios/Sources`.
- Shared localized labels: `clients/translations/*.json`; platform resources produced by `generate.py`.

## Verification (2026-09-12)

- Desktop: 67 plan, runtime-center and Markdown tests passed; 2 real Stage rendering tests passed.
- Electron interaction fixture: preview opens full document, complete Markdown copies, light/dark screenshots captured. Screenshot comparison passed for the tested Desktop surfaces.
- Desktop main and renderer TypeScript checks passed.
- Android: 7 isolated Kotlin/JUnit tests passed using extracted actual production functions, including Unicode escapes and surrogate pairs. Full Gradle build and Compose test execution remain blocked by concurrent Cron source/binding mismatches (`automationJson`, `createConfigured`, `updateConfigured`, `sessionId`, offload automation argument).
- iOS: 5 isolated Swift projection tests passed; modified Swift syntax checks passed. Full Xcode build remains blocked by concurrent Cron generated binding mismatches (`createConfigured`, `updateConfigured`, `updateAutomation`, `automationJson`, `sessionId`).
- Translation generation freshness and `git diff --check` passed.

Native device screenshots and complete mobile UI test runs are not claimed. No dependency or binding changes were introduced for Plan UI. Mobile build blockers must be resolved before full end-to-end acceptance.

macOS signed packaging was attempted with `npm run package:mac:flare -- --launch`. The Rust sidecar compilation failed in concurrent Cron code: `bridge-server/src/cron_host.rs` cannot resolve `cron`, `HostCronFirer` does not satisfy the expected trait, and `ClientEvent::ScheduledRunFinished` is unavailable in `server.rs`. No new packaged app was launched. Build log: `/tmp/plan-ui-package.log`.
