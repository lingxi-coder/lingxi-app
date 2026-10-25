# Harness Runtime

Embed the Rust agent runtime in a desktop, mobile, or custom host.

<a id="overview"></a>

## The runtime boundary

Harness Runtime is the embeddable execution layer extracted from LingXi. It composes agents, sessions, model host services, permissions, tools, plugins, and persistence. The Cargo package is harness-runtime; the Rust crate is harness_runtime.

The host owns product UI, entrypoints, credentials, platform capabilities, and application lifecycle. The independent llm-client owns communication with model providers. HarnessBuilder wraps services assembled by the host; constructing this facade does not start processes or make network requests.

These pages describe the current workspace. The source baseline records HEAD, dirty files and the product dependency revision separately. A local API is not automatically present in the pinned product build; use Rustdoc for compiler-resolved types and features.

<a id="installation"></a>

## Use one source identity

For application integration, retain the dependency revisions declared by Harness. The multi-repository path overlay below is for developing the SDKs together and requires matching contracts. Replacing its pinned LLM Client with an arbitrary newer checkout can fail in llm-runtime before your application is compiled; a Cargo package version alone does not establish source compatibility.

For the current working-tree examples, create sdk-consumer beside harness-runtime, llm-client and mobile-linux-runtime. Use the complete local manifest below. Direct Harness crates and explicit consumer patches select the three SDK working trees; inspect cargo metadata to verify those source identities before compiling the copied examples.

### Cargo.toml · working-tree consumer

```toml
[package]
name = "handbook-harness-consumer"
version = "0.1.0"
edition = "2024"
publish = false

[workspace]

[dependencies]
harness-runtime = { path = "../harness-runtime/crates/runtime", default-features = false, features = ["desktop"] }
client = { path = "../harness-runtime/crates/client", features = ["adapter"] }
tool-api = { path = "../harness-runtime/crates/tool-api" }
permission = { path = "../harness-runtime/crates/permission" }
async-trait = "0.1"
tokio = { version = "1", features = ["rt-multi-thread", "macros", "sync"] }

[patch."https://github.com/lingxi-coder/llm-client.git"]
lingxi-llm-client = { path = "../llm-client" }

[patch."https://github.com/lingxi-coder/mobile-linux-runtime.git"]
mobile-linux-api = { path = "../mobile-linux-runtime/crates/mobile-linux-api" }
mobile-linux-core = { path = "../mobile-linux-runtime/crates/mobile-linux-core" }
mobile-linux-android = { path = "../mobile-linux-runtime/crates/mobile-linux-android" }
mobile-linux-ios = { path = "../mobile-linux-runtime/crates/mobile-linux-ios" }
platform-pty = { path = "../mobile-linux-runtime/crates/platform-pty" }

[patch.crates-io]
git2 = { path = "../harness-runtime/third_party/git2-rs/git2" }
```

Verification method: Environment validation (harness-working-tree-manifest)

### Source checkout

```bash
git clone --recurse-submodules https://github.com/lingxi-coder/harness-runtime.git
cd harness-runtime
git submodule update --init --recursive
cargo check --locked -p harness-runtime
```

Verification method: Environment validation

The repository pins Rust 1.94.0 and has no root path patch selecting the sibling llm-client. These overrides belong explicitly to the isolated consumer. For a released integration, use one reviewed full Git revision for all direct Harness packages, remove working-tree SDK overrides, and review the lockfile. Never mix Git and local versions of shared Rust types.

<a id="features"></a>

## Choose a composition profile

- engine — enabled by default; exposes the shared Harness, builder, session handle, and execution components.
- desktop — includes engine and desktop assembly, tools, client adapters, platform services, Fusion, workflow, and collaboration.
- mobile — includes engine and the shared iOS/Android Rust composition; the host supplies device and platform capabilities.
- uniffi — includes mobile and native binding metadata. android-computer-use also includes mobile.
- realtime-websocket — enables realtime WebSocket support in the model runtime. fusion, workflow, and collaboration can also be selected explicitly.

The current Cargo feature is engine. Older README examples that call it core do not describe this revision; use the feature names in crates/runtime/Cargo.toml.

<a id="builder"></a>

## Assemble the public facade

### Rust · host-supplied services

```rust
use std::sync::Arc;
use harness_runtime::{
    Harness, HarnessBuilder, LifecycleService, SessionService,
};

fn assemble(
    session: Arc<dyn SessionService>,
    lifecycle: Arc<dyn LifecycleService>,
) -> Harness {
    HarnessBuilder::new(session, lifecycle).build()
}
```

Verification method: Compile check (harness-assemble)

### HarnessBuilder::new

```text
pub fn new(session: Arc<dyn SessionService>, lifecycle: Arc<dyn LifecycleService>) -> Self
```

Accept services from the same assembled runtime composition.

### HarnessBuilder::build

```text
pub fn build(self) -> Harness
```

Retain the injected services behind opaque SDK handles.

### Harness::session

```text
pub fn session(&self) -> SessionHandle
```

Return a cheap clone of the handle for the composition’s currently mounted session.

<a id="embedding-contract"></a>

## Host admission, resources and failures

Use a Tokio runtime whose worker stack is at least harness_runtime::RUNTIME_THREAD_STACK_SIZE (8 MiB). Platform assembly loads configuration, constructs registries and starts owned services; it can fail before a session is available. The facade constructor alone cannot validate credentials or platform readiness.

Choose desktop::build_harness for a Rust host that owns its permission surface, mobile::build_mobile for explicit device services, or headless::run for byte-oriented noninteractive execution. Headless is enabled by desktop and reports execution, durable shutdown and output delivery separately.

- [Agent execution and recovery](agent-execution.md)

Existing sdk_surface tests check the opaque SDK contract; platform composition and real provider execution require their own checks. A build result is not a live model or device acceptance result.

<a id="choose-entrypoint"></a>

## Choose the entry point for your application

Start with the host you are building. A chat window, a batch worker and a mobile application have different input, permission and shutdown requirements, even when they use the same agent loop.

| You want to | Start with | Supply from the host |
| --- | --- | --- |
| Embed a Rust chat or desktop UI | desktop::build_harness | DesktopConfig, OutputStream and PermissionGate |
| Run a noninteractive job | headless::run | HeadlessHostServices, byte readers/writers, and host signals |
| Integrate Android or iOS | mobile::build_mobile | MobileConfig, Platform, event listener and permission sink |
| Wrap an already assembled composition | HarnessBuilder::new | SessionService and LifecycleService from the same composition |

- [Platform assembly and shutdown](harness-lifecycle.md#platform-assembly)

<a id="first-agent-workflow"></a>

## Wire your first agent workflow

1. Prepare an application workspace and model/provider configuration. Choose a composition profile and keep all direct Harness dependencies on the same source revision.
2. Create the output destination before assembling the runtime. If your UI consumes ClientEvent DTOs, wrap one shared ClientEventSink with AdapterOutputStream.
3. Provide a permission gate that can reach the user or apply an explicit noninteractive policy. Admit a prompt only after the host has a mounted session.
4. Pass RunInput and a host-owned CancellationToken to session().run. Read text and tool activity from the output stream while awaiting its terminal result.
5. Keep the returned outcome, session identity and cost snapshot. When closing the application, stop new prompts, drain active turns and then inspect shutdown().complete.

- [Copy the complete service-injection example](harness.md#builder)
- [Run, cancel and inspect a turn](harness-sessions.md)
- [Connect a client event sink](harness-client.md#output-adapter)

<a id="startup-troubleshooting"></a>

## Troubleshoot startup

| Symptom | Check first | Next action |
| --- | --- | --- |
| The builder succeeds but nothing runs | HarnessBuilder retains services; it does not submit a prompt. | Call the mounted SessionHandle::run and consume its output destination. |
| Text never reaches the interface | OutputStream or ClientEventSink ownership and delivery. | Verify the same sink is attached before startup; keep transport writes off the execution callback. |
| An operation waits on approval | The host permission request/response route. | Resolve the exact request_id, or supply the intended noninteractive policy before running. |
| Shared Rust types do not match | Duplicate Git/path identities in cargo metadata. | Use the complete consumer manifest and one reviewed source identity for companion crates. |

## Sources and documentation baseline

[Source repository](https://github.com/lingxi-coder/harness-runtime)

- [harness-runtime/crates/runtime/src/lib.rs](https://github.com/lingxi-coder/harness-runtime/blob/f975ab5804bb3af0e72beb3974e5c911ece44d53/crates/runtime/src/lib.rs) — Committed source (f975ab5804bb3af0e72beb3974e5c911ece44d53)
- [harness-runtime/crates/runtime/src/api.rs](https://github.com/lingxi-coder/harness-runtime/blob/f975ab5804bb3af0e72beb3974e5c911ece44d53/crates/runtime/src/api.rs) — Committed source (f975ab5804bb3af0e72beb3974e5c911ece44d53)
- [harness-runtime/crates/runtime/Cargo.toml](https://github.com/lingxi-coder/harness-runtime/blob/f975ab5804bb3af0e72beb3974e5c911ece44d53/crates/runtime/Cargo.toml) — Committed source (f975ab5804bb3af0e72beb3974e5c911ece44d53)
- [harness-runtime/Cargo.toml](https://github.com/lingxi-coder/harness-runtime/blob/f975ab5804bb3af0e72beb3974e5c911ece44d53/Cargo.toml) — Committed source (f975ab5804bb3af0e72beb3974e5c911ece44d53)
- [harness-runtime/rust-toolchain.toml](https://github.com/lingxi-coder/harness-runtime/blob/f975ab5804bb3af0e72beb3974e5c911ece44d53/rust-toolchain.toml) — Committed source (f975ab5804bb3af0e72beb3974e5c911ece44d53)

- harness-runtime: HEAD f975ab5804bb3af0e72beb3974e5c911ece44d53; Referenced input fingerprint sha256:92c2108dc63215bc69ea9c11ea081795da883fdcb1c9c28b060aa9221a1b95a2; Product pin e04c1837508014d68ea042d5c9fe3c8c15af5aa0

<!-- Generated from apps/web/src/data/docs. Do not edit this file directly. -->
