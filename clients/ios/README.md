# 灵犀 Code · iPhone (SwiftUI)

A native SwiftUI recreation of the LingXi Code iPhone design prototype
(`clients/.design-reference/project/lingxi-iphone.html`). The conversation
surface drives the real in-process engine over UniFFI (M10-P3a) through a
`ConversationSource` seam; the rest of the UI still renders the prototype's
mock data (later milestones).

## Requirements

- macOS with Xcode 16+ (iOS 18.0 SDK or newer)
- [XcodeGen](https://github.com/yonyz/XcodeGen): `brew install xcodegen`
- Rust toolchain with the iOS std targets (`aarch64-apple-ios`,
  `aarch64-apple-ios-sim`, `x86_64-apple-ios`) — the build script adds any
  missing ones automatically.

## Engine bindings (run once before building)

The app links the in-process engine through UniFFI. The generated Swift
bindings (`Generated/`) and `Frameworks/LingxiCodeFFI.xcframework` are
**git-ignored** and reproduced from the Rust workspace by a build script — run
it before the first `xcodegen generate`, and again whenever the UniFFI surface
(`crates/apps/ios-framework`, `client-protocol`, …) changes:

```sh
cd clients/ios
scripts/build-xcframework.sh
scripts/install-sherpa-runtime.sh
```

The first script builds a host cdylib + per-arch staticlibs, generates the Swift bindings
(deduped into a single Swift module), and assembles the xcframework. No secrets
are baked in — the engine reads `ANTHROPIC_API_KEY` from the runtime
environment, never from the framework.

The Sherpa installer downloads the pinned 1.13.2 iOS artifact, verifies its
SHA-256, and stages the gitignored `sherpa-onnx.xcframework` and
`onnxruntime.xcframework` used by Local-only Voice. Re-run it only when the
shared `clients/voice/models.json` runtime version changes.

The same command also builds the device-only Linux runtime from the pinned
OpenMinis source: iSH ARM64 static archives plus an Alpine aarch64 fakefs
rootfs. Generated libraries, headers, and the rootfs archive remain untracked.
Simulator slices do not link iSH and continue to expose an unavailable runtime.

The iSH-linked iOS app is a GPLv3 combined distribution. Release builds must
ship the corresponding source and notices documented in
`docs/mobile-linux/LICENSES/NOTICE.md`; iSH's `LICENSE.IOS` contains the App
Store distribution exception.

## Conversation source (mock vs. real engine)

`Sources/Conversation/ConversationSource.swift` defines the seam `ChatView`
talks to. `ConversationSourceFactory.make()` picks the implementation at app
start:

- **`EngineConversationSource`** — the real in-process engine over UniFFI. It
  builds a `MobileEngineHandle` via the generated `buildIosEngine(...)`,
  registers a Swift `IosEventListener` whose `onEvent(_:)` maps each inbound
  `ClientEvent` (`textDelta` / `toolUse*` / `turnStarted` / `turnEnded` /
  `error` / …) onto `@MainActor`-published SwiftUI state, and submits turns with
  `handle.submit(.sendPrompt(...))`.
- **`MockConversationSource`** — the prior canned reply (used when the engine is
  not opted in, e.g. previews / no-key runs).

The engine is selected when its bindings are linked **and** it is opted in:
either `LINGXI_USE_ENGINE=1` or a non-empty `ANTHROPIC_API_KEY` in the
environment. Runtime config is read from the environment by
`EngineConfig.fromEnvironment` — `ANTHROPIC_API_KEY` (the key, **never**
hardcoded), optional `ANTHROPIC_BASE_URL`, optional `LINGXI_MODEL`. Set these in
the Xcode scheme's *Run → Arguments → Environment Variables* (or the launching
shell). The engine roots its filesystem under the app's Application Support
container.

## Build & run

```sh
cd clients/ios
scripts/build-xcframework.sh   # once — produces the gitignored Generated/ + Frameworks/
scripts/install-sherpa-runtime.sh # once — verifies and stages Sherpa + ONNX Runtime
xcodegen generate              # produces LingxiCode.xcodeproj from project.yml
open LingxiCode.xcodeproj        # then ⌘R on an iPhone simulator
```

Or from the command line:

```sh
cd clients/ios
scripts/build-xcframework.sh
scripts/install-sherpa-runtime.sh
xcodegen generate
xcodebuild -project LingxiCode.xcodeproj -scheme LingxiCode \
  -sdk iphonesimulator -destination 'platform=iOS Simulator,name=iPhone 15 Pro' build
```

`LingxiCode.xcodeproj` is generated and git-ignored; regenerate after editing
`project.yml` or adding/removing source files. The `Generated/` Swift bindings
are added to the app target's sources. `Frameworks/LingxiCodeFFI.xcframework`
and the Sherpa/ONNX Runtime static XCFrameworks are linked but not embedded;
all are referenced from `project.yml`.

## Architecture

| Folder | Contents |
|---|---|
| `Sources/App` | `@main` app entry + `RootView` (composes chat + drawer + settings + voice) |
| `Sources/Theme` | `DesignTokens` (oklch→sRGB palettes), `Theme` env + `AppState` (theme/accent persistence) |
| `Sources/Models` | Domain models, verbatim mock data, settings store |
| `Sources/Components` | `LXIcon` (SVG icon set), `Pill`, `LXToggle`, status bar, home indicator, `color-mix` helper |
| `Sources/Conversation` | `ChatView`, `Composer`, `MessageBubble`, and the `ConversationSource` seam (`MockConversationSource` + `EngineConversationSource` over UniFFI) |
| `Sources/Bridge` | `EngineModule` — UniFFI linkage smoke (force-links the engine static archive) |
| `Sources/Drawer` | `Drawer` (workspace pills, chats/projects/crons, knowledge/memory, account) |
| `Sources/Settings` | Settings sheet host + every page (LLM/search/fetch providers, voice, skills, MCP, dream, appearance, language, etc.) |
| `Sources/Voice` | Unified Voice preferences/capabilities, Flow, Sherpa STT/TTS bridge, model store, and playback |
| `Sources/Theme/LocalizationManager.swift` | Persisted app-language override (follow-system default) + the `Bundle` swizzle that makes `String(localized:)`/`Text` re-resolve without a relaunch |

### Localization

UI copy lives in [`clients/translations/`](../translations/README.md), not in
this target's own `.strings`/`.xcstrings` files by hand — `Resources/Localizable.xcstrings`
is generated from `clients/translations/*.json` by `clients/translations/generate.py`
and gets overwritten on every run. To add or change a string, edit the JSON
source and regenerate; see that README for the extraction conventions
(placeholder formats, what NOT to extract, how a plain `ViewModel`/non-View
class reaches localized text). Unit tests that assert exact localized copy
depend on the `-AppleLanguages (zh-Hans)` launch argument pinned on the
`LingxiCode`/`LingxiCodeStore`/`LingxiCodeFull` schemes' Test action in
`project.yml` — without it, results follow whatever language the host Mac
happens to be running.

### Design tokens

CSS `oklch(L C H)` values have no SwiftUI equivalent, so each token was
converted offline to sRGB (D65) via the OKLab→linear-sRGB matrix + sRGB gamma
and stored as `Color(.sRGB, red:green:blue:opacity:)`, with the original oklch
string kept in a trailing comment. Dark and light palettes are both provided;
the theme toggle + 6 accent colors are single-sourced in `DesignTokens.swift`
and injected through the `\.theme` environment (the SwiftUI analog of the
prototype's `Theme` React context).

Fonts map to the system font (SF Pro) for UI and `.monospaced` for code/keys,
keeping the prototype's weights and sizes.
