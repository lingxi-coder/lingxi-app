# Platform adapter responsibilities

Product entrypoints own presentation, native permissions, credentials and host
lifetimes. Harness owns the assembled Agent runtime. Shared product protocols and
binding behavior have one source even when they are compiled for multiple hosts.

| Owner | Responsibility |
| --- | --- |
| `apps/{ios,android}/native` | Native UI, OS capability implementations and one `NativeEngineFactory` per platform. Interactive, scheduled and Local App scenarios explicitly select their policy. |
| `apps/{ios,android}/ffi` | Platform-named UniFFI exports, platform construction and native callback identity. |
| `packages/mobile-ffi` | Shared Remote lifecycle, generation fences, ordered delivery, transfer cleanup, audio ownership and identical Share/SecureStorage conversion. |
| `apps/cli/host` | CLI inputs and CLI-specific background ownership; holds the assembled `DesktopRuntime` directly. |
| `apps/bridge-server` | Authenticated transport and connection-scoped turn/permission coordination. |
| `packages/host-identity` | Common product identity and endpoint eligibility for provider requests. |
| `packages/bridge-client` | Client transport, Rust-generated command/event roots, and explicit remaining nested/semantic validation. |
| Harness `platform-common` | Portable desktop clock, task spawner, watcher and LSP behavior. Platform adapters select actual OS policy and process configuration. |

## Native assembly and ownership

Android's general engine factory belongs to `runtimeintegration`; audio UI does
not own conversation, scheduled-task or Local App construction. iOS's factory
captures configuration on the main actor, constructs off-main on a dedicated
8 MiB-stack thread and applies the selected scenario's callback policy.

The media host remains app-scoped. A session engine does not acquire ownership
of every other engine or its media resources. Headless permission sinks retain
their engine weakly and decline interactive approval. Native secure storage,
PRoot/iSH execution, mount authorization and Direct/Play capability choices keep
their existing platform owners.

Harness mobile platforms store injected optional native handles in the typed
`DeviceServices` aggregate. Existing individual `Platform` accessors project those
handles; tools do not use a dynamic service registry. Minimal keeps its small
clock/spawner implementations to preserve its narrower dependency graph.
`platform-common/desktop` is opt-in on direct POSIX/Windows edges; other existing
dependency paths can also activate it, including paths from the mobile runtime.

## Shared UniFFI source

`packages/mobile-ffi` is source-owned rather than an independently exported
UniFFI component. Both platform FFI crates compile its ordinary typed modules and
retain their existing platform-named exports. Remote and callback tests live
beside that shared implementation and run under both platform crates.

UniFFI metadata includes module paths and documentation. Moving a type can change
checksums even when the public source signature is unchanged. Generate Swift and
Kotlin bindings from the matching freshly built libraries; compare generated API
names and compile native consumers. Previously generated bindings are not a
valid acceptance artifact for this refactor.

## Protocol generation

`scripts/checks/generate-client-protocol.py` resolves the same pinned or explicitly
selected development Harness source as the product. The independently locked
`tools/bridge-protocol-gen` tool parses the Rust wire declarations and writes
`packages/bridge-client/src/protocol.generated.ts`.

The generated roots cover `ClientCommand`, `ClientEvent` and selected DTOs,
variant inventories and basic event field checks. Nested DTOs, custom Serde
behavior and semantic/range validation retain explicit review responsibilities;
see the [generator scope](../../tools/bridge-protocol-gen/README.md). JSON goldens
remain independent behavioral evidence. Electron validates domain strings before
using them as permission state or privileged commands.

## Focused verification

```sh
cargo check --locked -p ios-framework -p android-aar -p cli -p bridge-server --all-targets
cargo test --locked -p host-identity -p ios-framework -p android-aar --lib
cargo test --locked -p cli --lib cli_runtime_keeps_workflow_feed_and_watcher_until_shutdown
cargo test --locked -p cli --lib cli_background_fork_uses_live_permission_and_cd_authority
cargo test --locked -p bridge-server --lib assembled_bridge_runs_injected_local_command_as_owned_turn
python3 scripts/checks/generate-client-protocol.py --check
bash scripts/checks/check-client-protocol.sh
npm --prefix packages/bridge-client run build
npm --prefix apps/electron run typecheck
```

Harness's shared platform and capability tests run in its own workspace:

```sh
cargo test --locked -p platform-common -p platform-ios -p platform-android --features platform-common/desktop --lib
cargo test --locked -p platform-posix --test fs_watch_debounce_test
cargo build --locked -p test-harness --bin mock_lsp_server
cargo test --locked -p lsp --test spawn_handshake_test
```

The LSP integration test needs the mock binary in the same target and profile.
Set `CARGO_TARGET_DIR` in the local environment when using a separate build cache;
checked-in Cargo configuration must remain portable.

Normal product builds use the immutable Git sources recorded in `Cargo.lock`.
Joint development can use a complete Cargo overlay in a separate disposable
checkout with its own lockfile. All Harness-owned package identities, generated
bindings and native libraries must resolve to that same selected source. Keep
that overlay and its lockfile outside the delivery checkout. Release integration
updates immutable upstream revisions after upstream delivery.
