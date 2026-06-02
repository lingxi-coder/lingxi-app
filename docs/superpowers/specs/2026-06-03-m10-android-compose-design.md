# LingXi Code — Android Compose App Design (M10 A3)

> **Status**: APPROVED v1
> **Date**: 2026-06-03
> **Program**: M10 Native Apps (predecessors: Electron A1, iOS A2, foundation F1–F3)
> **Design source**: `clients/.design-reference/project/lingxi-iphone.html` ("灵犀 · iPhone", Claude-style mobile) — the SAME surfaces the iOS shell implements, adapted to Material 3 / Compose.

---

## §1 Goal & non-goals

### Goal

A native **Jetpack Compose** application at `clients/android/` that mirrors the iPhone
design (`lingxi-iphone.html`) — the third native front-end in the M10 program, visually
and structurally consistent with the iOS shell, rendering the same conceptual
client-protocol surface. It is a **UI shell driven by the iPhone design's mock data**;
the engine/UniFFI (`.aar`) wiring is a later step (the mobile analog of the Electron↔bridge
work already done).

### Non-goals (deferred)

- **Engine / UniFFI wiring** — no `submit()`/listener integration with `engine-mobile` yet;
  the app renders mock data exactly as the iOS and Electron shells do.
- **A dedicated Android visual mockup** — none exists in the bundle; Android mirrors the
  iPhone design adapted to Material idioms (brand-consistent, not a redesign).
- **Play Store packaging / signing / release** — local dev `assembleDebug` only.
- **The `lingxi-os.html` AI-phone-OS concept** — explicitly out of scope (a different product).
- **Push notifications, widgets, background work, deep links** — beyond the parity surface.

---

## §2 Architecture

- **Build**: Gradle (Kotlin DSL) with a pinned **Gradle wrapper**; AGP + Kotlin 2.x +
  Compose BOM; Material 3; single-`Activity`, edge-to-edge. `minSdk 26`, target latest
  installed platform. Application id / package **`com.lingxi.code`** (matches the existing
  `android-aar` namespace).
- **Source layout** (mirrors the iOS tree):
  ```
  clients/android/
  ├── settings.gradle.kts, build.gradle.kts, gradle.properties, gradlew(+wrapper)
  └── app/
      ├── build.gradle.kts
      └── src/main/
          ├── AndroidManifest.xml
          └── java/com/lingxi/code/
              ├── MainActivity.kt          # single activity, setContent { RootScreen() }
              ├── RootScreen.kt            # composes drawer + conversation + settings + voice
              ├── theme/                   # DesignTokens.kt, Type.kt, Shapes.kt, Theme.kt
              ├── model/                   # Models.kt + mock data (ported from the prototype)
              ├── components/              # shared composables + icons
              ├── conversation/            # ChatScreen, MessageBubble, WorkflowBar, Composer
              ├── drawer/                  # Drawer (对话/项目/定时), project rows, cron cards
              ├── settings/                # SettingsHost + pages (providers/skills/mcp/dream/...)
              └── voice/                   # VoiceFlowOverlay
      └── src/test / src/androidTest       # unit + Compose UI tests
  ```
- **Navigation**: Navigation-Compose `NavHost`. `ModalNavigationDrawer` for the
  对话/项目/定时 drawer. Settings is a nested nav graph mirroring the iOS push-navigation
  (`TopAppBar` chevron-back + system back gesture). Voice flow is a full-screen overlay
  triggered by a long-press on the mic.
- **State**: a `ViewModel` exposing `StateFlow` for the mock conversation and a settings
  store; theme + accent provided via a `CompositionLocal` and persisted with **DataStore**
  (the Android analog of the iOS `@AppStorage`).

## §3 Theme fidelity

Reuse the **exact oklch→sRGB values** the iOS `DesignTokens.swift` already computed (single
brand source of truth), expressed as Compose `Color(...)`. Provide **dark + light** palettes
and the **6 accent** colors, with a live theme + accent switch. Fonts map to the system
default for UI text and `FontFamily.Monospace` for code / API-key / keyboard-key fields.
Recreate the prototype's spacing, radii, and motion with Compose equivalents
(`AnimatedVisibility`, `animate*AsState`, `updateTransition`).

## §4 Surfaces (iOS parity)

1. **Conversation** — user/assistant bubbles, thinking + streaming affordance, workflow chip
   bar with animated states, top bar (menu / title / theme toggle / new chat),
   model-selector composer (text field + send + voice button).
2. **Drawer** — workspace pills + search; 对话/项目/定时 tabs with counts; projects
   expand/collapse with active highlight; cron cards with status dot + next-run; knowledge
   base + memory shortcuts; account row → Settings.
3. **Voice flow** — long-press → immersive overlay with pulsing halo + animated waveform;
   release dismisses.
4. **Settings** (Material list + push nav): account card; **智能** — LLM providers
   (list → preset picker → edit: show/hide key, default-model radio, test-connection, set
   default, remove), web search, web fetch, voice TTS; **记忆与知识** — knowledge base,
   memory, workflows; **能力扩展** — Skills (grouped by author + detail page with triggers/
   permissions/prompt/toggle), MCP servers (stats card + edit: endpoint/transport/auth/
   per-tool permission toggles/reconnect), Dream mode (gradient orb, time window, run
   conditions, 5 stackable activities, compute budget, last-night review); **应用** —
   Appearance (theme + 6 accents + density + text-size preview), Language, Notifications,
   Input; **隐私与安全**; **关于**.

All mock data ported verbatim from the iPhone prototype; dark/light + accent are live and
single-sourced.

## §5 Data flow

Mock-driven, identical in spirit to the iOS/Electron shells: a `ViewModel` holds the mock
conversation + a `SettingsStore`; composables read `StateFlow`/state; user actions mutate
local state (send simulates a turn, test-connection/reconnect are local animations). No
network or engine calls. A clear seam (`ConversationSource` interface) is left where the
future UniFFI listener will feed real `ClientEvent`s.

## §6 Verification & testing

- **Build gate**: real `./gradlew :app:assembleDebug` (Android SDK present) +
  `compileDebugKotlin`; `./gradlew :app:lintDebug` advisory.
- **Unit tests** (`src/test`): design-token oklch→sRGB conversion sanity; mock-data
  integrity (counts/ids match the prototype).
- **Compose UI tests** (`src/androidTest`, `createAndroidComposeRule`): drawer tab switch,
  settings push-nav + back, theme/accent toggle. (UI tests need an emulator/device; the
  authoritative CI gate is `assembleDebug` + the JVM unit tests, with the instrumented tests
  documented for device runs.)
- **Adversarial review** per phase (Compose idiom correctness, design fidelity, no
  monster files, state hoisting correctness).

## §7 Execution decomposition (sequential — one Gradle module = shared state)

- **A1** Gradle scaffold (wrapper, settings/app build files, manifest, MainActivity, empty
  Theme) → `assembleDebug` green on an empty app.
- **A2** Theme/tokens (DesignTokens, Type, Shapes, Theme) + shared components + icons.
- **A3** Conversation screen + composer + workflow bar + message bubbles.
- **A4** Drawer (tabs, projects, cron, knowledge/memory, account).
- **A5** Voice-flow overlay.
- **A6** Settings — host + main grouped list + Appearance / Language / simple pages.
- **A7** Settings — providers (LLM / web search / web fetch: list → preset → edit).
- **A8** Settings — Skills + MCP + Dream mode.
- **A9** Final `assembleDebug` + unit/UI tests + `clients/android/README.md`.

Each task verifies with a Gradle compile/build; phase reviews + a commit per phase.

## §8 Isolation

All work lands under `clients/android/` in the existing `worktree-m10-native-apps` worktree.
Touches nothing under `lingxi-code/`, `clients/electron/`, or `clients/ios/`.
