# External LLM client migration

Status: implementation and validation complete.

## Accepted decisions

- Replace the in-tree protocol client with `lingxi-llm-client` from
  `https://github.com/lingxi-coder/llm-client`, pinned by full Git revision.
- Preserve current product capabilities across desktop, CLI, iOS and Android.
  No old-version API/configuration compatibility layer is required.
- Keep host policy (history, OAuth refresh, permissions, tools, retry, Fusion
  admission and durable accounting) in the new `llm-runtime` crate.
- Upgrade Rust and CI to 1.94.0.
- Add shared `providerRegion`: `international` (default) or `china_mainland`.
  Save and apply through existing idle-time restart/reconnect flows. Never
  silently reroute an unavailable selection to another provider/account.
- Complete missing general-purpose communication interfaces upstream before
  pinning the final remote revision. Do not copy provider codecs into the host.

## Working locations

The upstream integration branch is `codex/lingxi-runtime-integration`, based on
`1f1073ff521dc6cc7565edbec47f7fb9e82ad035`. Its isolated worktree is
`/private/tmp/lingxi-llm-client-migration`, attached to the existing independent
repository. The original upstream checkout remains untouched.

The initial upstream changes were published on that integration branch as
`ec8876c26e5bb8f76b507d3f2b57e115dc776c43`. The follow-up
[shared execution migration](2026-09-24-shared-llm-execution.md) advances the pin
to `9b0323f10f76c5834acafedaa04470c4e96346f2`. Cargo.lock records the same
HTTPS Git source and full revision. There is no temporary path dependency.

## Implementation checkpoints

- [x] Inspect both repositories and verify remote baseline.
- [x] Rename local crate and downstream imports to `llm-runtime`.
- [x] Upgrade primary Rust toolchain and 1.82 CI pins to 1.94.
- [x] Upstream prepared single-dispatch API; convenience calls share its path.
- [x] Upstream response usage extraction before semantic decode, and stream
      batch observations that return without waiting after usage-only frames.
- [x] Regression coverage for single dispatch, usage before decode errors,
      error frames and cancellation after usage-only data.
- [x] Finish upstream request controls, native content replay, exact counting,
      pricing snapshots and WebSocket support with focused regressions.
- [x] Replace local codecs/catalogs with upstream-backed host adapters.
- [x] Finish shared region wiring, save validation and UI on all platforms.
- [x] Verify retry/settlement, exact model identity and reasoning token mapping.
- [x] Run upstream and downstream checks, platform builds and package checks.
- [x] Publish upstream revision, pin Git dependency, remove temporary paths,
      confirm final dependency tree and document validation boundaries.

Existing uncommitted Electron notification/UI changes predate this task and
must remain intact.

## Validation results

- Upstream: 616 tests passed, including 21 doctests; strict Clippy passed for
  all targets and all features.
- Runtime/provider configuration: 1,286 tests passed against the pinned Git
  source. This includes exact model-row selection, physical-attempt settlement,
  independent usage extraction, late native-content replay ordering, and Gemini
  signed text/tool/provider-ID round trips without duplicate calls.
  The final replay-metadata envelope adjustment also passed its focused Gemini
  regression and the subsequent production package build.
- Full workspace/all-target compilation passed. Orchestrator streaming-loop
  regressions passed (10 tests), including late native-content transcript order.
- Dependency architecture check: 88 workspace crates, no violations.
- Electron: application TypeScript checks and 54 provider/settings tests passed.
  The isolated region save/reconnect fixture and terminal interaction fixture
  passed. The region panel was visually inspected from a painted screenshot.
- Android: DirectDebug Kotlin compilation passed; Android arm64 Rust target
  checking passed.
- iOS: Apple Silicon simulator UI build passed with the existing XCFramework;
  the migrated Rust engine was separately checked for the arm64 iOS simulator.
- macOS: the final `npm run package:mac:flare -- --launch` completed successfully
  with Flare signing, static verification, and packaged-app smoke checks for
  renderer/sidecar, terminal, Git, Keychain persistence across restart,
  authenticated localhost model requests, permissions and scheduled tasks.
  The verified application was launched from
  `apps/electron/dist/LingXi-Code-0.1.0-mac-arm64/LingXi Code.app`.

The wire client remains responsible for its own protocol tests. Host projection
regressions now assert its intentional contracts: native blocks survive replay,
unsupported numeric reasoning budgets are rejected rather than approximated,
truncated streams remain errors, and malformed tool arguments are never executed.
The initial migration used the upstream low-level codec interface. The follow-up
[shared execution migration](2026-09-24-shared-llm-execution.md) replaces that
production path with upstream request preparation, execution and stream handling.

No real-provider billed calls or physical mobile-device acceptance tests have
been performed. Mobile UI compilation and Rust target checking are separate
validation steps, not a newly packaged mobile release.
