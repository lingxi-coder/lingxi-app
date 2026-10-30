# Mobile Market browser installation

Android and iOS Apps toolbars open the system default browser instead of a native market catalog. Native entries select `platform=android` and `platform=ios`; Desktop selects its actual OS. The website validates and preserves this filter across navigation.

`lingxi://market/<id>` carries a market identity only. Android now routes it to a separate installation review instead of treating it as a local installation ID. iOS uses an installation review with native release consent. Existing installations open their local details without link-triggered updates or duplicate records.

Each client requests a distribution for its actual platform, architecture, runtime version, distribution channel and supported capabilities. Browser filters cannot select the install target. Extra client checks reject a response for another platform, environment or channel before download. Android now advertises `store`/`full` rather than packaging flavor names `play`/`direct`, and serializes installation operations. Source verification, local release permissions and platform-local application data remain in place.

## Validation

- Market: typecheck, production build, 64 unit/API tests and 6 Chromium browser tests passed. Four Firebase-emulator tests remain skipped.
- Android: the real production platform-routing source and tests compiled with Kotlin 2.4.21 and each flavor's generated BuildConfig; 3 tests passed for Play and 3 for Direct.
- iOS: the real production MarketAPI and models compiled with Swift; the source-only URLProtocol harness verified the iOS browser URL, anonymous POST target and 7 platform/environment/channel response cases. Changed Swift files passed parsing. XCTest regression cases were added.
- Desktop: Node TypeScript check passed for the platform-specific browser URL change.

Full Android builds are blocked by the absent native-support manifest and outdated generated bindings (including missing Local App service APIs). The attempted iOS Store simulator build is blocked by missing `GStreamer.xcframework`. These are source-level checks, not completed native app builds, signed package delivery, real-device tests, or browser-to-device installation acceptance.

Evidence: `/Users/luolingfeng/lingxi/artifacts/market-browser-install-20261030/`.
