# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

A platform-agnostic Rust engine for an AI coding assistant with **1:1 behavioral parity to claude-code** (2026-03-31 TypeScript reference), plus native clients that all drive that one engine. Parity is the organizing constraint: most design questions are settled by "what does the reference do", not by preference. `docs/ARCHITECTURE.md#claude-code-parity-guarantees-locked-in-v030` lists the guarantees that are locked.

## Repo navigation

Only two directories are the product, and only these are tracked by git:

- `lingxi-code/` — the Rust workspace (~90 crates)
- `clients/` — Electron desktop, iOS, Android, web, the shared TS SDK, i18n

Also tracked: `docs/`, `skills/`, `third_party/`, `.github/`.

Everything else at the repo root (`claude-code/`, `codex/`, `opencode/`, `claw-code*/`, `backups/`, `output/`, `codegraph-out/`, `liter-llm/`) is **untracked local working material**. Scope repo-wide sweeps to `lingxi-code/` and `clients/` or they will wander into unrelated trees.

## Build and test

### Engine (from `lingxi-code/`)

```bash
cargo build --workspace --release
cargo test --workspace --all-features --no-fail-fast   # what CI runs
cargo test -p orchestrator --lib                        # one crate
cargo test -p orchestrator --test resume_test some_name # one test
./scripts/check-all.sh                                  # every checked-in gate
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
```

`check-all.sh` **discovers** gates by walking `scripts/` for `check-*.sh` / `*-gate.sh` — a new gate is picked up without editing a list. Run it from `lingxi-code/`, not from inside `scripts/`.

The core crate's package name is `core` but its Rust ident is `lingxi_core` (so it does not shadow sysroot `libcore`): `cargo test -p core`, `use lingxi_core::`.

### Clients

```bash
./clients/setup.sh          # idempotent bootstrap for a fresh clone
```

`clients/shared` (`@lingxi/bridge-client`) **must be built before electron** — electron resolves it via `file:../shared` and consumes the compiled `dist/`.

```bash
cd clients/shared  && npm install && npm run build
cd clients/electron && npm install && npm run typecheck && npm test
node --import ../shared/node_modules/tsx/dist/loader.mjs --test test/<one>.test.ts
```

Mobile builds are optional in `setup.sh` and skip cleanly without their toolchains: Android needs `cargo-ndk` + NDK (`clients/android/scripts/build-jni.sh`); iOS needs `xcodebuild` + `xcodegen` (`clients/ios/scripts/build-xcframework.sh`, then `xcodegen generate`).

## Architecture

### Composition roots decide what ships

Library crates make no shipping choices — they expose capabilities (`tools/*`, `skill-api`, `command-api`) and abstractions (`tool-api`, `platform_api::Platform`). Two composition roots under `apps/` assemble a product by naming a different subset of capability crates as Cargo dependencies:

- `apps/engine-desktop` — all 14 desktop tool crates (40 tools), core + desktop commands
- `apps/engine-mobile` — the cross-platform subset + mobile tools (camera/voice/share)

There is **no `#[cfg(target_os)]` in any library crate**. That is confined to `platforms/*` and `apps/*`, and `scripts/check-deps.sh` (the §8.1 dependency-graph gate) enforces it: tools never depend on sibling tools or platforms, apps are leaves, `*-api` crates stay impl-free.

### Engine ↔ client seam

The clients never see engine types. The path is:

```
orchestrator → client-adapter (lowering) → client-protocol DTOs → bridge-server / UniFFI → client
```

- `client-protocol` — the wire DTOs. `CLIENT_PROTOCOL_VERSION` is governed by a **structural** guard (`tests/version_guard_test.rs`): a removed, renamed, or retyped variant/field is a MAJOR bump; a new variant or new optional field is additive and needs none.
- `client-adapter` — lowers engine values to DTOs. Note the **live vs replay asymmetry**: a live `ToolUseResult` carries the tool's full structured `data`, while a replayed `ContentBlock::ToolResult` holds only model-facing text. Anything structural a client reads off `result_json` must be recovered explicitly on the resume path.
- Desktop reaches the engine over a loopback `bridge-server`; mobile links it in-process via UniFFI.

### Cross-cutting invariants

- No `tokio::spawn` outside the runtime trait — background work goes through `RuntimeSpawner`.
- No `tokio::fs` / `std::fs` in engine crates — all I/O through the `FileSystem` trait.
- `Secret<T>`'s Debug is always `<redacted>`.
- `ProcessRunner::run` accepts only a `SandboxedCommand` — the sandbox is type-enforced.

Key flows (turn loop, tool dispatch, compaction, subagent spawn) are sketched in `docs/ARCHITECTURE.md#key-flows`.

## macOS Electron packaging

From `AGENTS.md`, which governs this and should be read in full before packaging:

- Always package through `npm run package:mac:flare` (or `scripts/package-macos-flare.sh`) from `clients/electron`. Do **not** bypass the wrapper with `npm run package:mac`.
- Team: Flare App, Inc., `AZ4AX7J833` — the same as iOS. The wrapper rejects a different Team ID and selects the valid `Apple Development` identity by certificate hash.
- Development packaging uses the isolated `development` credential channel and needs **Mac** App Development profiles for `com.lingxi.code.development` and `com.lingxi.code.credential-broker.development`. An iOS Xcode Managed Profile is not a substitute.
- Missing profiles are created through the checked-in provisioning bootstrap target with Xcode Automatic Signing (`-allowProvisioningUpdates -allowProvisioningDeviceRegistration`) — never by automating the Apple Developer website.
- `npm run package:mac:flare -- --check` validates signing assets without a real build; add `-- --launch` to launch after verification.
- Never commit Apple IDs, app-specific passwords, private keys, App Store Connect API keys, or local profile files.

**`npm run dev` cannot test credential persistence or open a session** — the Credential Broker deliberately rejects unpackaged Electron processes and fails closed (no plaintext or login-keychain fallback). Anything touching real credentials or sessions needs a signed package.

## Things that will cost you time

- **`cargo check` is blind to test modules.** Use `cargo build --tests --keep-going` to surface every error at once, and `--all-features` — a feature-gated subsystem (mobile UniFFI) is invisible without it.
- **Generated artifacts are not sources.** `clients/ios/Generated/` and the Android JNI bindings are gitignored build output — change a wire DTO without regenerating and the clients cannot compile. `clients/ios/Resources/Localizable.xcstrings` and Android `strings.xml` are generated from `clients/translations/*.json` by `generate.py`; edit the JSON.
- **`client-protocol --features uniffi` has a metadata budget.** CI runs `cargo test -p client-protocol --features uniffi --test uniffi_metadata_budget_test`; blowing it breaks both mobile builds while ordinary gates stay green.
- **A green test run is not a green commit.** Uncommitted changes can make the working tree compile while the committed tree does not; and a falling test *count* at zero failures means a binary aborted early, not that everything passed.
