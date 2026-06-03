# 灵犀 Code · iPhone — in-process engine over UniFFI (M10 A2)

This document covers the engine integration end to end: how to reproduce the
gitignored bindings, regenerate the Xcode project, run the keyless simulator
e2e test, and do a REAL run that streams live model output. It is the in-process
analog of the verified Electron↔bridge real-conversation path — **one** protocol
(`client-protocol`), **one** transport (UniFFI), **one** listener (SwiftUI).

```
SwiftUI (ConversationSource / XCTest)
   │  buildIosEngine(...) + handle.submit(.sendPrompt(...))
   ▼
UniFFI  ── generated Swift bindings (Generated/*.swift) + LingxiCodeFFI.xcframework
   │
   ▼
in-process engine  (MobileEngineHandle, handle-owned tokio runtime)
   │  every ClientEvent
   ▼
IosEventListener.onEvent(_:)   ← Swift listener (EngineListener / CollectingListener)
```

## 0. Prerequisites

- macOS + Xcode 15+ (iOS 17 SDK or newer)
- `brew install xcodegen`
- Rust toolchain with the iOS std targets (`aarch64-apple-ios`,
  `aarch64-apple-ios-sim`, `x86_64-apple-ios`). The build script installs any
  missing ones for the workspace's pinned toolchain.

## 1. Reproduce the engine bindings (gitignored)

The generated Swift bindings (`Generated/`) and the FFI static-library
xcframework (`Frameworks/LingxiCodeFFI.xcframework`) are **git-ignored** and
reproduced from the Rust workspace. Run this before the first `xcodegen
generate`, and again whenever the UniFFI surface
(`lingxi-code/apps/ios-framework`, `client-protocol`, …) changes:

```sh
cd clients/ios
scripts/build-xcframework.sh
```

It builds a host cdylib + per-arch staticlibs, generates the Swift bindings
(deduped into a single Swift module), forces the `IosEventListener`
callback-vtable registration in `buildIosEngine` (see note below), and assembles
the xcframework. **No secrets are baked in** — the engine reads
`ANTHROPIC_API_KEY` from the runtime environment, never from the framework.

> **Callback-vtable init note.** UniFFI 0.28 registers a `callback_interface`'s
> foreign vtable inside its namespace's `private` lazy `initializationResult`,
> forced only by that namespace's `uniffiEnsureInitialized()`. The generated
> synchronous `buildIosEngine` does **not** force it, so without a patch the
> first engine event would panic in Rust with *"Foreign pointer not set"*. The
> build script deterministically injects `uniffiEnsureInitialized()` as
> `buildIosEngine`'s first statement (a bindings fix only — no engine-semantics
> change). This is what lets the listener receive events at all.

## 2. Regenerate the Xcode project

```sh
cd clients/ios
xcodegen generate     # produces LingxiCode.xcodeproj from project.yml (gitignored)
```

`project.yml` defines two targets:

- **`LingxiCode`** (app) — compiles `Sources/` + the generated `Generated/`
  bindings and links `LingxiCodeFFI.xcframework`.
- **`LingxiCodeTests`** (`bundle.unit-test`) — the engine round-trip suite in
  `Tests/`, hosted in the app target (so it shares the compiled bindings) and
  also linking the FFI xcframework so the per-namespace `engine_mobileFFI` clang
  module is importable. The `LingxiCode` scheme runs this target under `test`.

## 3. Keyless simulator e2e test (no API key required)

`Tests/EngineRoundtripTests.swift` builds the engine via `buildIosEngine(...)`,
registers a `CollectingListener` (an `IosEventListener`), submits
`.sendPrompt("Reply with exactly: hello from lingxi")`, and asserts the streamed
`ClientEvent` path with an `XCTestExpectation`.

It is designed to pass **KEYLESS**. With no `ANTHROPIC_API_KEY`:

- `buildIosEngine(...)` succeeds — the engine host is constructed and the foreign
  listener is registered (the handshake). No network, no key needed.
- `submit(.sendPrompt(...))` returns Ok — the turn is spawned on the engine's
  owned runtime; a turn failure is **not** thrown here, it streams to the
  listener.
- the keyless turn then surfaces a **terminal `ClientEvent.error`** to the
  listener. On the simulator `cfg(target_os = "ios")` is **true**, so the engine
  builds the real `IosPlatform` whose `http` handle is the shared
  `reqwest` + `rustls` client (`platform_common::http::ReqwestHttp`). The turn
  therefore makes a **real HTTPS request** to the Anthropic-compatible endpoint,
  and the terminal error is a real transport outcome — a `401`
  (`non-success HTTP status 401: …`) when the request reaches the host, or a
  `connection failed: …` when the simulator has no route to it. **Either is a
  real-client result**; what it is *not* is the old `platform-posix-minimal` SSE
  stub (`posix-minimal: … SSE stub …`).

The test asserts both that an `.error` event arrived (proving the full
SwiftUI→UniFFI→engine→listener path in-process) **and**, negatively, that the
error message does not carry the posix-minimal stub markers (`posix-minimal` /
`SSE stub` / `HTTP stub`) — proving a real `reqwest` call was made. This stays
hermetic: it passes for a `401` *or* a connection error, without a secret and
without requiring network success.

Run it (no key in the environment):

```sh
cd clients/ios
scripts/build-xcframework.sh        # once, if not already reproduced
xcodegen generate                   # once, if the project is not generated

# pick an available iPhone simulator
D=$(xcrun simctl list devices available | grep -oE 'iPhone[^(]*' \
      | grep -v 'BAZEL\|_2[0-9]' | head -1 | xargs)
xcodebuild -scheme LingxiCode -sdk iphonesimulator \
  -destination "platform=iOS Simulator,name=$(echo "$D" | sed 's/ *$//')" \
  test CODE_SIGNING_ALLOWED=NO
```

Expected: `** TEST SUCCEEDED **`. The test log prints the real engine event
stream it received over the UniFFI listener — a **real** transport error from
the `reqwest`+`rustls` client, e.g.:

```
[P4] keyless ClientEvents received via UniFFI listener: turnStarted(turnId: nil) | error(kind: ...ErrorKindDto.transport, message: "streaming transport error: non-success HTTP status 401: ...")
```

or, when the simulator cannot reach the host:

```
[P4] keyless ClientEvents received via UniFFI listener: turnStarted(turnId: nil) | error(kind: ...ErrorKindDto.transport, message: "streaming transport error: connection failed: ...")
```

## 4. REAL run (with a key → live `textDelta`)

To exercise the same path with a working model call, provide a real key **in the
environment** (never hardcoded, never committed). The engine reads
`ANTHROPIC_API_KEY` (and optional `ANTHROPIC_BASE_URL` / `LINGXI_MODEL`) at
runtime via `EngineConfig.fromEnvironment`.

### Run the app

In Xcode: open `LingxiCode.xcodeproj`, edit the **LingxiCode** scheme →
*Run → Arguments → Environment Variables*, add `ANTHROPIC_API_KEY = <your key>`
(optionally `LINGXI_USE_ENGINE = 1` to force the engine source), then ⌘R on an
iPhone simulator and send a prompt. The conversation streams real
`ClientEvent.textDelta` into the assistant bubble.

From the command line (key stays in your shell, not in any file):

```sh
cd clients/ios
export ANTHROPIC_API_KEY=...        # your key — environment only
xcodegen generate
open LingxiCode.xcodeproj           # ⌘R; the engine source is selected because a key is present
```

### Run the test against a live endpoint

The keyless test asserts the `.error` path, so it is unaffected by a present
key (it does not require success). To observe a live `textDelta` stream from the
test path instead, set the key in the environment before running `xcodebuild
test`:

```sh
cd clients/ios
export ANTHROPIC_API_KEY=...        # environment only — NEVER committed/printed
D=$(xcrun simctl list devices available | grep -oE 'iPhone[^(]*' \
      | grep -v 'BAZEL\|_2[0-9]' | head -1 | xargs)
xcodebuild -scheme LingxiCode -sdk iphonesimulator \
  -destination "platform=iOS Simulator,name=$(echo "$D" | sed 's/ *$//')" \
  test CODE_SIGNING_ALLOWED=NO
```

With a valid key reaching a real Anthropic-compatible endpoint, the same
listener receives `turnStarted` → one or more `textDelta` → `turnEnded` (the
`[P4] …` log line then shows `textDelta(...)` entries). The simulator's
`IosPlatform` now uses the real `reqwest`+`rustls` HTTP/SSE client
(`platform_common::http::ReqwestHttp`, shared with desktop and Android), so the
streamed `textDelta` is a genuine network response — no stub stands in the path.
The keyless terminal-error assertion (a real `401` / connection error, never the
`posix-minimal … SSE stub`) is the portable, secret-free proof that the entire
in-process transport — including the real client — is wired.

## Files

| Path | Role |
|---|---|
| `Sources/Conversation/ConversationSource.swift` | the `ConversationSource` seam; `EngineConversationSource` builds the handle and maps `ClientEvent`s onto SwiftUI state |
| `Sources/Bridge/EngineModule.swift` | UniFFI link smoke (force-links the static archive) |
| `Tests/EngineRoundtripTests.swift` | the keyless simulator e2e (this document's §3) |
| `scripts/build-xcframework.sh` | reproduces `Generated/` + `Frameworks/LingxiCodeFFI.xcframework` |
| `project.yml` | XcodeGen spec: app target, `LingxiCodeTests` unit-test target, and the test scheme |
```
