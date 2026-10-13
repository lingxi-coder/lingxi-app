# Local App Market integration (no UI)

Create exactly one `LocalAppMarket` in the application service graph with an anonymous HTTPS API root ending `/api/v1`, and a `CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate)`. Call store methods on Main. No token, old Harness Apps bindings or bridge is used.

```kotlin
val market = LocalAppMarket.create(applicationContext, "https://your-market-host/api/v1", appScope)
market.store.initialize().join()
market.store.query("") // state: StateFlow<MarketState>, limit 24 / platform android
market.store.query("calculator")
market.store.loadNext()
val detail = market.store.detail(appId)
val versions = market.store.versions(appId)
market.store.install(appId, versionId).join()
val webView = market.runtime.open(appId) { externalUrl -> /* UI decides navigation */ }
// UI may attach this handle; do not call loadUrl, replace clients, add bridges or alter settings.
market.runtime.suspend(appId)
market.runtime.resume(appId)
market.runtime.close(appId)
market.store.checkUpdates().join()
market.store.update(appId, newVersionId)
market.store.cancel(appId) // download/extraction only; commit cannot be cancelled
market.store.uninstall(appId)
```

`state` contains typed catalog/pagination, installed records, operations/progress/errors, update statuses and capability error. Detail/version requests throw `MarketException` or transport exceptions so the UI can display failures. Query generations discard stale responses. No background updates are scheduled.

The committed registry under app-private `files/local-app-market` is the installed source of truth. Immutable version directories and staging prevent failed/cancelled updates from damaging the old version. Platform metadata is checked before installation. HTML and ZIP run from independent installation profiles and stable HTTPS origins, preserving localStorage/IndexedDB across updates. Modern WebView MULTI_PROFILE and DELETE_BROWSING_DATA support is required; unsupported devices return explicit errors. Uninstall waits for browsing-data deletion, retains profile tombstones and removes loaded profiles on next cold initialization before opening any profile. Pending uninstall cannot reopen; failures can be retried. Do not construct a second repository/runtime for the same root.

Asset serving includes MIME, CSP, GET/HEAD and single byte Range. ZIP extraction validates CRC, root index, size/count limits, path traversal, conflicts, symlinks, encryption and compression support. Network CSP permits HTTPS/WSS; standard CORS applies. Device permissions are denied. Service workers are disabled by CSP. External top-level navigation is delegated to UI; no native capability bridge exists.

Validation: focused JVM tests live in `app/src/test/java/com/lingxi/code/market`; instrumented response/capability tests live in `app/src/androidTest/java/com/lingxi/code/market`. Native persistence, module-script execution, HTTPS/WSS and profile data isolation require runtime device acceptance beyond compilation/JVM checks. Full app builds may independently fail on stale generated bindings; do not modify UI files to hide those failures.

## Validation recorded 2026-10-12 (America/Los_Angeles)

- Isolated Android library compiled all four production market files against compileSdk 37 / minSdk 26, AGP 9.2.1, Kotlin 2.4.21, WebKit 1.17.1, Coroutines 1.11.0 and Commons Compress 1.27.1. Its native instrumented tests also compiled successfully.
- JUnit `MarketTest`: **14 tests, zero failures, zero errors**. Tests cover catalog query/platform/sort/cursors/update statuses, stale replies/pagination dedupe/error state, HTML/ZIP/index/path conflict/traversal/count/symlink safeguards, actual download size/hash, CRC corruption and same-app concurrency, update identity/failure retention, atomic commit failure, pending uninstall blocking/recovery and orphan cleanup.
- Full app `:app:compilePlayDebugKotlin` and `:app:compileDirectDebugKotlin` were attempted with native-ELF verification tasks excluded. Both failed on existing generated-binding mismatch: `ChatViewModel.kt` references missing `PromptSuggestionState`, `PromptSuggestion`, `UpdatePromptSuggestionContext`, `PromptSuggestionFeedback`; `AndroidNativeAudioServiceAdapter.kt` has a non-exhaustive `AudioTarget.LocalApp` branch. Market source compilation emitted no errors. No unrelated UI/audio/generated bindings were modified.
- No ADB device was attached. The three instrumented tests are **compiled, not executed**. They cover CSP/MIME/Range/HEAD response semantics, provider feature support and real module-script/localStorage isolation, reopen, update preservation and reinstall reset. HTTPS/WSS, IndexedDB, actual process restart persistence and provider-specific runtime behavior still require device acceptance.
- Production backend service availability remains independent of this native module; configured API failures propagate to callers/state.

### Isolated reproduction when unrelated app compilation is blocked

Use a temporary Android library project with a `local.properties` copy pointing at the installed SDK and source sets referring only to `app/src/main/java/com/lingxi/code/market`, `app/src/test/java/com/lingxi/code/market` and `app/src/androidTest/java/com/lingxi/code/market`. Use the versions above; test dependencies are JUnit 4.13.2, org.json 20240303, coroutines-test 1.11.0, androidx.test.ext:junit 1.3.0 and androidx.test:runner 1.7.0. Set `android.useAndroidX=true`, `android.builtInKotlin=false`, `android.newDsl=false`, JVM target 17 and `testOptions.unitTests.isReturnDefaultValues=true`. Run the repository Gradle wrapper with `-p <temporary-project> testDebugUnitTest compileDebugAndroidTestKotlin`. This excludes product UI and stale FFI while compiling the real native market runtime against exact Android/WebKit APIs. The validation harness used `/tmp/android-market-harness`; its JUnit XML records 14 passing tests. Repeat full variant compilation once the binding mismatch is resolved by its owner.
