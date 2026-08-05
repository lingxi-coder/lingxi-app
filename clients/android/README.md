# 灵犀 (LingXi) — Android client

A Jetpack Compose port of the LingXi mobile experience (`lingxi-iphone.html` /
the iOS client), built as a Material 3 single-Activity app. This is the **UI
shell** milestone (M10 A3): every surface is rendered against verbatim mock
data with a clean `ConversationSource` seam for future UniFFI/engine wiring — no
network or engine calls happen yet.

Application id: `com.lingxi.code` (debug variant: `com.lingxi.code.debug`).

## Surfaces

Mirrors the iPhone design 1:1, adapted to idiomatic Compose:

- **Conversation** — top bar (drawer / theme toggle / new chat), workflow chip
  bar, message list with streaming "thinking" dots + new-chat empty state, and
  a pill composer with a model picker and press-and-hold mic.
- **Drawer** (`ModalNavigationDrawer`) — 对话 / 项目 / 定时 tabs, workspace
  pills, search, knowledge/memory shortcuts, and the account row that opens
  settings.
- **Voice flow** — immersive full-screen overlay driven by a press-and-hold
  gesture (松开发送).
- **Settings** — a full-surface overlay hosting a Navigation-Compose `NavHost`
  push-nav stack: account, LLM providers / web search / web fetch / voice,
  Skills, MCP, Dream, knowledge / memory / workflows, appearance, language,
  notifications, input, and privacy. System back pops one page or closes at the
  root.

Theme + accent persist through a DataStore-backed `AppearanceStore`; the dark +
light palettes and the 6 brand accents reuse the exact oklch→sRGB color values
shared with the iOS client (see `theme/DesignTokens.kt`).

## Requirements

| Tool | Version |
|---|---|
| JDK | 17 |
| Gradle | 8.14 (via the checked-in wrapper — use `./gradlew`) |
| Android Gradle Plugin | 8.13.2 |
| Kotlin | 2.2.20 |
| compileSdk / targetSdk | 35 |
| minSdk | 26 |

The Android SDK location is read from `local.properties`
(`sdk.dir=$ANDROID_HOME`). That file is environment-specific and is **not**
checked in; create it if it is missing:

```bash
echo "sdk.dir=$ANDROID_HOME" > local.properties
```

## Build

All commands run from `clients/android/`.

```bash
# Assemble the debug APK (output: app/build/outputs/apk/debug/app-debug.apk)
./gradlew :app:assembleDebug
```

## Run

### Android Studio
1. Open the `clients/android/` folder in Android Studio (Giraffe or newer).
2. Let Gradle sync, then select the `app` run configuration.
3. Choose a device/emulator (API 26+) and press Run.

### Command line (emulator or device)
```bash
# Start an emulator (or plug in a device with USB debugging), then:
./gradlew :app:installDebug
adb shell am start -n com.lingxi.code.debug/com.lingxi.code.MainActivity
```

## Tests

### JVM unit tests — the CI gate (device-free)

The headless CI gate is **`assembleDebug` + `testDebugUnitTest`**. The unit
tests run on the plain JVM (no emulator) and cover the data/color invariants:

```bash
./gradlew :app:assembleDebug && ./gradlew :app:testDebugUnitTest
```

What they assert:
- **`DesignTokensTest`** — the dark/light palettes match the canonical
  oklch→sRGB values copied from the iOS `DesignTokens.swift`, alpha is carried
  on the border/ambient tokens, status colors are shared across appearances,
  and `palette()` / `withAccent()` behave. The epsilon tolerates Compose's
  8-bit (1/255) sRGB channel quantization.
- **`AccentsTest`** — the 6 accent swatches, their canonical oklch id strings,
  order, colors, the default id, and the lookup fallback.
- **`MockDataTest`** — counts + ids for workspaces / chats / projects /
  sessions / crons / models / default messages, the flattened `allSessions`
  lookup, and `session()` fallback — pinned against the prototype.
- **`SettingsMockTest`** — provider / skill / MCP counts, enabled/default
  splits, preset catalog sizes, `ConnStatus` labels, `NotifConfig.enabledCount`,
  and the `newProvider` add-flow.

HTML report: `app/build/reports/tests/testDebugUnitTest/index.html`.

### Instrumented Compose UI tests — require an emulator

The UI tests in `app/src/androidTest/` use `createAndroidComposeRule` against the
real `MainActivity`, so they need a **connected device or running emulator** and
are therefore **not** part of the headless CI gate.

```bash
# With an emulator/device connected:
./gradlew :app:connectedDebugAndroidTest
```

`AppFlowUiTest` covers:
- **Drawer tab switch** — open the drawer, switch 对话 → 项目 → 定时 → 对话 and
  assert the mock content for each tab.
- **Settings push-nav + back** — open settings from the account row, push to 外观
  (Appearance), then system-back to the settings root.
- **Theme + accent toggle** — flip the 浅色/深色 theme radio and pick a non-default
  accent swatch on the Appearance page.

Icon-only affordances are tagged via `components/UiTags.kt`; everything else is
found by its visible label.

## Module layout

```
app/src/main/java/com/lingxi/code/
  MainActivity.kt        single Activity host (edge-to-edge, theme wiring)
  RootScreen.kt          ModalNavigationDrawer + conversation + voice overlay
  components/            shared UI atoms (icons, toggle, pill, UiTags)
  conversation/          ChatScreen, composer, message bubbles, ConversationSource seam
  drawer/                drawer content + hoisted DrawerUiState
  model/                 domain models + verbatim mock data (Models, SettingsModels)
  settings/              settings NavHost, pages, and the SettingsStore
  theme/                 DesignTokens (oklch→sRGB), Theme, AppearanceStore
  voice/                 immersive voice-flow overlay + hold gesture
app/src/test/            JVM unit tests (CI gate)
app/src/androidTest/     instrumented Compose UI tests (need an emulator)
```

## Localization

UI copy comes from [`clients/translations/`](../translations/README.md), not
from hand-edited `res/values*/strings.xml` — those are generated by
`clients/translations/generate.py` from the canonical `*.json` locale files
and get overwritten on every run. To add or change a string, edit the JSON
source and regenerate; see that README for extraction conventions
(placeholder formats, what NOT to extract, how a plain `ViewModel`/callback
with no `Context` reaches localized text via the
`ConversationStrings`/`DefaultConversationStrings`-style resolver seam).
`theme/AppLanguageStore.kt` owns the persisted app-language override and the
`attachBaseContext` locale wrap.
