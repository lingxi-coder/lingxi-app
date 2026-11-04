# Mobile full-screen image preview — 2026-11-01

Android and iOS now share the Desktop presentation: a dark canvas, centered image, floating save/close controls and a bottom zoom capsule. Transcript images, composer attachments, session resources and workspace images use the native viewer. Pinch, drag, double-tap, zoom buttons and fit-to-screen are supported. Workspace preview close identifiers and Android export filenames are preserved.

Saving uses the system document picker, without requesting photo-library permissions. The export path retains original encoded bytes. iOS declares concrete writable image types so the exporter preserves the format/extension. Picker presentation and cancellation were exercised; a successful write to a user-selected provider was not exercised.

## Validation

- Android: 7 geometry tests passed; Direct ARM64 debug app and instrumentation APKs built. Native-support, rootfs evidence and final APK verification gates passed without changes to their policies.
- Android physical device: 3 Compose UI tests passed individually: message image open/zoom/double-tap/drag/fit/close, system Back, and system save-picker cancellation. A new 48MP rendering/accessibility regression was compiled; its physical-device rerun was interrupted by APK installation failure. The OEM blocks a subsequent background test Activity launch, so each method was run after explicitly foregrounding the debug test Activity. Temporary stay-awake settings were restored. Application data was retained.
- iOS: StoreDebug simulator build-for-testing passed. On an isolated iPhone 17 Pro / iOS 26.3 simulator, 3 native scroll-view tests and 2 UI tests passed, covering centering, fit, zoom bounds, native viewport resizing, zoom/double-tap/drag/close and exporter cancellation.
- The 8 new labels match Android, iOS and shared catalogs in all 5 supported languages. The 4 existing Desktop localization tests passed. `git diff --check` passed.

Both applications retain their existing portrait-only orientation declarations. No iPhone physical-device build/install or signed iOS release acceptance is claimed.

## Evidence and build isolation

The first canonical Android build was blocked by existing Cargo.lock/local dependency patch disagreement. Android verification used a source snapshot under `/Users/luolingfeng/lingxi/.buildcache/image-preview-mobile/android-checkout`; only that snapshot's lockfile was resolved. All affected Android source/resource/test files match the canonical checkout byte-for-byte, recorded in `android-image-source-sha256.json`. Current generated bindings and native artifacts were reused with the normal verification gates enabled. The canonical dependency files were not changed by this task.

Evidence is under `/Users/luolingfeng/lingxi/.buildcache/image-preview-mobile/`: `android-device-final.log`, per-method device logs, the source hash receipt, and `ios-image-tests-verified.xcresult`. Earlier failed attempts are retained separately. Screenshots are under `/Users/luolingfeng/lingxi/artifacts/image-preview-mobile-20261101/` (`android-portrait.png`, `ios-portrait.png`, `ios-exporter.png`).

## Review corrections

Desktop saves through a validated native IPC operation and the system save dialog. The generic browser download block remains enabled. The Electron regression verifies original-byte output, cancellation and failure feedback with downloads blocked. Sender authorization, payload bounds, media signatures and revalidation after the dialog have focused tests.

Android display decoding is capped at 4,194,304 pixels and 4096 pixels per edge; source dimensions still determine zoom percentages, and export retains original bytes. Fit-to-window has a translated action label and percentage state for TalkBack.
