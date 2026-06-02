# 灵犀 Code · iPhone (SwiftUI)

A native SwiftUI recreation of the LingXi Code iPhone design prototype
(`clients/.design-reference/project/lingxi-iphone.html`). UI shell with the
prototype's mock data — no backend/engine wiring (a later milestone).

## Requirements

- macOS with Xcode 15+ (iOS 17.0 SDK or newer)
- [XcodeGen](https://github.com/yonyz/XcodeGen): `brew install xcodegen`

## Build & run

```sh
cd clients/ios
xcodegen generate          # produces LingxiCode.xcodeproj from project.yml
open LingxiCode.xcodeproj   # then ⌘R on an iPhone simulator
```

Or from the command line:

```sh
cd clients/ios
xcodegen generate
xcodebuild -project LingxiCode.xcodeproj -scheme LingxiCode \
  -sdk iphonesimulator -destination 'platform=iOS Simulator,name=iPhone 15 Pro' build
```

`LingxiCode.xcodeproj` is generated and git-ignored; regenerate after editing
`project.yml` or adding/removing source files.

## Architecture

| Folder | Contents |
|---|---|
| `Sources/App` | `@main` app entry + `RootView` (composes chat + drawer + settings + voice) |
| `Sources/Theme` | `DesignTokens` (oklch→sRGB palettes), `Theme` env + `AppState` (theme/accent persistence) |
| `Sources/Models` | Domain models, verbatim mock data, settings store |
| `Sources/Components` | `LXIcon` (SVG icon set), `Pill`, `LXToggle`, status bar, home indicator, `color-mix` helper |
| `Sources/Conversation` | `ChatView`, `Composer`, `MessageBubble`, `WorkflowBar` |
| `Sources/Drawer` | `Drawer` (workspace pills, chats/projects/crons, knowledge/memory, account) |
| `Sources/Settings` | Settings sheet host + every page (LLM/search/fetch providers, voice, skills, MCP, dream, appearance, language, etc.) |
| `Sources/Voice` | `VoiceFlowView` (long-press immersive recording) |

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
