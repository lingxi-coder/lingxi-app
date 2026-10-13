# Local App Market integration

No UI or legacy Harness Apps FFI dependency. Call from the main actor:

```swift
let market = try LocalAppMarket(baseURL: URL(string: "https://YOUR_MARKET/api/v1")!)
await market.initialize() // restores records and retries pending uninstall cleanup
await market.search("")
await market.loadNextPage()
let detail = try await market.detail("appId")
market.install(appId: detail.id, versionId: detail.currentVersion!.id)
// Observe operations[id] until .idle; installed is updated after commit.
let view = try await market.open(detail.id) // embed returned WKWebView in UI
market.runtime.onExternalNavigation = { url in /* UI decides handling */ }
await market.checkUpdates()
try await market.update(detail.id) // launches cancellable operation
market.cancel(detail.id) // accepted before commit only
market.suspend(detail.id)
market.resume(detail.id)
await market.close(detail.id)
try await market.uninstall(detail.id)
```

Use one long-lived instance per process/root. `apps`, `installed`, `updates`, `operations`,
`error`, `runtimeErrors`, `isLoading` and `hasNextPage` are observable. Runtime errors
include occupied stable port; the UI can surface them without guessing availability.
App errors preserve server code/message. Instantiate once and call initialize before UI
opens installed apps; query responses are epoch-fenced. No Firebase headers are sent.

Installation UUID owns a named persistent WKWebsiteDataStore and stable loopback port.
Update closes the old view, preserves UUID/port/browser storage, and atomically publishes
validated content. Reopen explicitly after update. Before uninstall, remove the embedded WebView and release consumer references to the
returned raw WKWebView. WebKit rejects named-store deletion while a consumer retains
a view/configuration; that error remains recoverable rather than being suppressed.
Uninstall tombstones survive failures;
initialize retries deletion before listing installed apps. Reinstall creates a fresh UUID.

Self-contained packages work offline. HTTPS/WSS requests obey normal CORS; no native
bridge, worker or device-permission capability is supplied. Network lifecycle belongs
to the container: UI must close when removing a view, and suspend/resume with scenes.
Origin uses HTTP on 127.0.0.1 with a narrow ATS exception. Remote origins may need to
allow that origin through their own CORS policy; the container does not override it.

## Verified delivery

Focused native XCTest run passed: **9 tests, 0 failures**, using a temporary iOS app
host containing exactly this module, the checked-in LocalAppMarketTests, and the
pinned ZIPFoundation 0.9.20 local source. It ran on iPhone 17 Pro Max / iOS 26.3
simulator. Evidence: `/private/tmp/lingxi-market-testhost/test11.log` and
`/private/tmp/lingxi-market-testhost/build/Logs/Test/Test-MarketTestHost-2026.10.12_23-59-19--0700.xcresult`.

The suite exercises anonymous platform-filtered search/paging, duplicate filtering,
stale query fencing, API service errors; incremental download size limits, SHA256,
HTML staging and old-version preservation on failure; ZIP CRC, encrypted archives,
traversal and collisions; atomic registry reload/removal retry; static HTTP HEAD,
Range, path rejection and occupied-port error; actual WebKit relative module loading,
localStorage/IndexedDB isolation and persistence across reopen, stable-port reuse,
and deletion of both named browser stores. WebKit cleanup explicitly awaits removing
all website data before removing the named store. Listener shutdown awaits socket
cancellation before reopening the stable port.

Both production StoreDebug and FullDebug simulator builds were attempted. They are
blocked by existing generated Harness binding mismatches in EngineConversationSource
(missing prompt-suggestion command/event members) and a non-exhaustive switch in
IOSAudioServiceCallback; no market-module compile errors occurred. Evidence:
`/private/tmp/market-ios-store-build.log`, `/private/tmp/market-ios-full-build.log`.

This is focused simulator evidence, not device or full application acceptance.
HTTPS/WSS remote connectivity and complete update/uninstall/reinstall user flows
still require device/full-app acceptance. The production market backend must be
configured before live catalog acceptance; API errors are exposed to consumers.
