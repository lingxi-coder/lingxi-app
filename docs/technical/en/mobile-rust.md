# Rust API

Shared contracts for commands, task lifecycle, PTY, raw stdio, mounts and rootfs verification.

<a id="crates"></a>

## Rust integration

Select mobile-linux-api and mobile-linux-core with mobile-linux-android or mobile-linux-ios from the same Git revision. The SDK-owned platform-pty crate supplies shared PTY support. Embed native-support artifacts alongside your existing Rust library to keep one runtime in the process.

### AndroidProotRuntimeConfig

```text
pub struct AndroidProotRuntimeConfig
```

Caller-supplied managed root, app sandbox root, ABI, rootfs version/hash, native library directory and optional isolated build profile.

### IosIshRuntime::new

```text
pub fn new(mut config: IosIshRuntimeConfig) -> Result<Self, MobileLinuxError>
```

Constructs a session without mutating filesystem or native state and validates the process-wide kernel identity.

<a id="working-tree-consumer"></a>

## Consumer workspace and source identity

Create a standalone consumer directory beside the mobile-linux-runtime checkout and use this complete Cargo.toml. The empty workspace keeps the consumer out of a parent product workspace; the direct path dependency binds it to the current SDK working tree. Save the Rust lifecycle function from this page in src/lib.rs and run cargo check from the consumer root. Replace the SDK path only when deliberately choosing another source snapshot.

This example depends only on the portable mobile-linux-api contract. Constructing a platform runtime additionally requires its Android or iOS backend and matching native-support artifacts from one revision. Compilation of this consumer does not boot Linux, install rootfs assets, exercise UniFFI or prove physical-device execution.

### Cargo.toml · complete working-tree consumer

```toml
[package]
name = "handbook-mobile-consumer"
version = "0.1.0"
edition = "2024"
publish = false

[workspace]

[dependencies]
mobile-linux-api = { path = "../mobile-linux-runtime/crates/mobile-linux-api" }

```

Verification method: Environment validation (mobile-working-tree-manifest)

<a id="commands"></a>

## Command requests and results

LinuxCommandRequest describes the executable, arguments, guest cwd, environment, optional text stdin and timeout, explicit network policy, resource limits and mounts. LinuxCommandResult returns collected text, exit code, timeout/cancellation flags and an enforcement receipt.

### mobile_linux.rs · request declaration excerpt

```rust
pub struct LinuxCommandRequest {
    pub command: String,
    pub args: Vec<String>,
    pub cwd: Option<String>,
    pub env: BTreeMap<String, String>,
    pub stdin: Option<String>,
    pub timeout_ms: Option<u64>,
    pub network: NetworkPolicy,
    #[serde(default)]
    pub resource_limits: ResourceLimits,
    pub mounts: Vec<MountSpec>,
}
```

Verification method: API excerpt

### MobileLinuxRuntime::run

```text
async fn run(&self, request: LinuxCommandRequest) -> Result<LinuxCommandResult, MobileLinuxError>
```

Run a command to completion inside the mobile backend.

### MobileLinuxRuntime::run_streaming

```text
async fn run_streaming(&self, request: LinuxCommandRequest, sink: Arc<dyn ProcessStreamSink>) -> Result<LinuxCommandResult, MobileLinuxError>
```

Deliver stdout lines and stderr byte chunks through the sink and return the final result.

<a id="task-lifecycle"></a>

## Runtime and background tasks

### MobileLinuxRuntime::probe_capability

```text
async fn probe_capability(&self) -> MobileLinuxCapability
```

Probe availability and support for streaming, background processes, PTY, mounts and integrity without changing state.

### MobileLinuxRuntime::boot

```text
async fn boot(&self) -> Result<RootfsStatus, MobileLinuxError>
```

Ensure that the backend is ready to accept requests.

### MobileLinuxRuntime::shutdown

```text
async fn shutdown(&self) -> Result<(), MobileLinuxError>
```

Stop the session and release transient resources; iOS native kernel lifetime remains process-wide.

### MobileLinuxRuntime::spawn_background

```text
async fn spawn_background(&self, request: LinuxCommandRequest) -> Result<LinuxProcessHandle, MobileLinuxError>
```

Start a managed background command and retain its opaque handle.

### MobileLinuxRuntime::kill

```text
async fn kill(&self, handle: &LinuxProcessHandle) -> Result<(), MobileLinuxError>
```

Success guarantees that the complete guest process group/tree has been reaped and no task descendant remains runnable.

<a id="pty"></a>

## PTY sessions

PTY input and output are byte buffers. Set terminal size in cells with PtySize { cols, rows }; drain PtyOutput and PtyClosed events through read_events.

### MobileLinuxRuntime::open_pty

```text
async fn open_pty(&self, request: PtyOpenRequest) -> Result<PtySessionHandle, MobileLinuxError>
```

Open an interactive terminal process.

### MobileLinuxRuntime::write_pty

```text
async fn write_pty(&self, handle: &PtySessionHandle, input: Vec<u8>) -> Result<(), MobileLinuxError>
```

Write terminal input bytes.

### MobileLinuxRuntime::resize_pty

```text
async fn resize_pty(&self, handle: &PtySessionHandle, size: PtySize) -> Result<(), MobileLinuxError>
```

Resize the terminal.

### MobileLinuxRuntime::close_pty

```text
async fn close_pty(&self, handle: &PtySessionHandle) -> Result<(), MobileLinuxError>
```

Close the PTY and reap its complete guest process tree before returning success.

<a id="raw-stdio"></a>

## Raw stdio sessions

Use RawStdioOpenRequest for a long-lived process with independent binary stdin/stdout/stderr. This surface preserves exact bytes for protocols such as LSP. Unsupported implementations fail closed instead of substituting a PTY.

### mobile_linux.rs · raw stdio request excerpt

```rust
pub struct RawStdioOpenRequest {
    pub command: String,
    pub args: Vec<String>,
    pub cwd: Option<String>,
    pub env: BTreeMap<String, String>,
    pub network: NetworkPolicy,
    #[serde(default)]
    pub resource_limits: ResourceLimits,
    pub mounts: Vec<MountSpec>,
}
```

Verification method: API excerpt

### MobileLinuxRuntime::open_raw_stdio

```text
async fn open_raw_stdio(&self, request: RawStdioOpenRequest) -> Result<RawStdioSessionHandle, MobileLinuxError>
```

Open a binary stdio process.

### MobileLinuxRuntime::write_raw_stdio

```text
async fn write_raw_stdio(&self, handle: &RawStdioSessionHandle, input: Vec<u8>) -> Result<(), MobileLinuxError>
```

Write raw stdin bytes.

### MobileLinuxRuntime::read_raw_stdio

```text
async fn read_raw_stdio(&self, handle: &RawStdioSessionHandle, max_bytes: usize) -> Result<RawStdioReadResult, MobileLinuxError>
```

Poll bounded stdout/stderr bytes and inspect closed and exit_code.

### MobileLinuxRuntime::close_raw_stdio

```text
async fn close_raw_stdio(&self, handle: &RawStdioSessionHandle) -> Result<(), MobileLinuxError>
```

Close the session and synchronously reap its complete guest process tree.

The FFI form takes MobileLinuxCommandRequestFfi, rejects any provided stdin or timeout_ms, and requires read max_bytes in 1..1048576. Control writes and lifetime through the returned session ID.

<a id="events"></a>

## Events and task snapshots

### MobileLinuxRuntime::read_events

```text
async fn read_events(&self, after_sequence: Option<u64>, limit: usize) -> Result<Vec<MobileLinuxEvent>, MobileLinuxError>
```

Read events strictly after a sequence cursor. MAX_MOBILE_LINUX_EVENT_BATCH is 512.

### MobileLinuxRuntime::list_tasks

```text
async fn list_tasks(&self) -> Result<Vec<MobileLinuxTaskSnapshot>, MobileLinuxError>
```

Enumerate known tasks and sessions with their status and completion details.

### MobileLinuxRuntime::task_status

```text
async fn task_status(&self, task_id: &str) -> Result<Option<MobileLinuxTaskSnapshot>, MobileLinuxError>
```

Read one task by ID; None means no matching task.

<a id="mounts"></a>

## Mounts and path translation

### MobileLinuxRuntime::configure_mounts

```text
async fn configure_mounts(&self, mounts: Vec<MountSpec>) -> Result<(), MobileLinuxError>
```

Replace the active bind-mount set after platform policy validation.

### MobileLinuxRuntime::current_mounts

```text
fn current_mounts(&self) -> Vec<MountSpec>
```

Return the current guest-to-host mount table.

### map_guest_path_to_host

```text
pub fn map_guest_path_to_host(path: &str, mounts: &[MountSpec]) -> Option<std::path::PathBuf>
```

Use the longest matching mount prefix. Pure lexical mapping does not provide filesystem fencing or read-only enforcement.

### map_host_path_to_guest

```text
pub fn map_host_path_to_guest(path: &std::path::Path, mounts: &[MountSpec]) -> Option<String>
```

Translate an absolute clean host path to its guest coordinate without filesystem access.

<a id="rootfs-store"></a>

## RootfsStore

mobile-linux-core owns inventory verification, safe staging and atomic activation. RootfsManifest identifies platform/ABI, archive identity, immutable files, writable paths, executable allowlists and package/SBOM data.

### RootfsStore::stage_prepared_rootfs

```text
pub fn stage_prepared_rootfs(&self, prepared_root: &Path, manifest: &RootfsManifest) -> Result<PathBuf, RootfsStoreError>
```

Verify a prepared directory against the manifest before publishing staging.

### RootfsStore::activate_staged_rootfs

```text
pub fn activate_staged_rootfs(&self, manifest: &RootfsManifest) -> Result<RootfsStatus, RootfsStoreError>
```

Recover interrupted activation, verify staging and activate the image atomically.

### RootfsStore::authorize_manifest_file

```text
pub fn authorize_manifest_file(&self, guest_path: &str, manifest: &RootfsManifest) -> Result<PathBuf, RootfsStoreError>
```

Validate an allowlisted immutable executable; writable guest paths and absent allowlist entries are rejected.

<a id="complete-command"></a>

## Complete command lifecycle example

This reusable function accepts the already configured platform trait, boots it, executes a bounded command, validates the requested network receipt and always attempts shutdown after execution. It returns nonzero exit codes as command results; callers must inspect exit_code, timed_out and cancelled before declaring success. Construction and rootfs distribution remain host inputs.

### Rust consumer function

```rust
use mobile_linux_api::{
    LinuxCommandRequest, LinuxCommandResult, MobileLinuxError,
    MobileLinuxRuntime, NetworkPolicy, ResourceLimits,
};
use std::collections::BTreeMap;

pub async fn execute_once(
    runtime: &dyn MobileLinuxRuntime,
) -> Result<LinuxCommandResult, MobileLinuxError> {
    runtime.boot().await?;
    let network = NetworkPolicy::Disabled;
    let limits = ResourceLimits::default();
    let execution = runtime.run(LinuxCommandRequest {
        command: "/bin/sh".into(),
        args: vec!["-c".into(), "printf 'ready\n'".into()],
        cwd: Some("/root".into()),
        env: BTreeMap::new(),
        stdin: None,
        timeout_ms: Some(10_000),
        network,
        resource_limits: limits,
        mounts: Vec::new(),
    }).await;
    let cleanup = runtime.shutdown().await;
    let result = execution?;
    cleanup?;
    result.enforcement.ensure_for(network, limits)?;
    Ok(result)
}
```

Verification method: Compile check (mobile-command-lifecycle)

<a id="io-ownership"></a>

## IO selection, cancellation and event cursors

Use run for collected text, run_streaming for incremental stdout lines and stderr byte chunks, PTY for a terminal, and raw stdio for byte-framed protocols. PTY terminal processing changes bytes, so LSP or a binary protocol needs raw stdio. A zero-length raw read is rejected by the FFI rather than used as a readiness probe.

Keep sequence cursors only for read_events, advancing to the largest returned sequence. The UniFFI streaming sink synthesizes per-call events with sequence 0 and a run-N task identifier; those callback sequences are not the backend event-log cursor. A task snapshot reconciles current state after a UI reconnect; output is not durable conversation history.

Managed background, PTY and raw handles own guest processes. Explicit kill/close returns only after the owned process tree is reaped. Foreground drop, timeout and shutdown paths have dedicated iOS transport tests, but a future cancellation alone is not an app-level promise of suspended or resumable mobile execution.

| Surface | Output / ownership | Finish |
| --- | --- | --- |
| run / run_streaming | One awaited operation; final result carries enforcement. | Completion, timeout, or owner cancellation. |
| spawn_background | Opaque LinuxProcessHandle plus task snapshots. | kill(handle), then inspect terminal task state. |
| open_pty | Terminal bytes and PtyOutput/PtyClosed events. | close_pty(handle). |
| open_raw_stdio | Separate binary streams and closed/exit_code reads. | close_raw_stdio(handle). |

## Sources and documentation baseline

[Source repository](https://github.com/lingxi-coder/mobile-linux-runtime/tree/main/crates/mobile-linux-api/src/mobile_linux.rs)

- [mobile-linux-runtime/crates/mobile-linux-api/src/mobile_linux.rs](https://github.com/lingxi-coder/mobile-linux-runtime/blob/eb82dcd73080bc35d0236abc15209a3bbee9f52c/crates/mobile-linux-api/src/mobile_linux.rs) — Committed source (eb82dcd73080bc35d0236abc15209a3bbee9f52c)
- [mobile-linux-runtime/crates/mobile-linux-api/src/execution.rs](https://github.com/lingxi-coder/mobile-linux-runtime/blob/eb82dcd73080bc35d0236abc15209a3bbee9f52c/crates/mobile-linux-api/src/execution.rs) — Committed source (eb82dcd73080bc35d0236abc15209a3bbee9f52c)
- [mobile-linux-runtime/crates/mobile-linux-ffi/src/runtime.rs](https://github.com/lingxi-coder/mobile-linux-runtime/blob/eb82dcd73080bc35d0236abc15209a3bbee9f52c/crates/mobile-linux-ffi/src/runtime.rs) — Committed source (eb82dcd73080bc35d0236abc15209a3bbee9f52c)
- [mobile-linux-runtime/crates/mobile-linux-core/src/store/install.rs](https://github.com/lingxi-coder/mobile-linux-runtime/blob/eb82dcd73080bc35d0236abc15209a3bbee9f52c/crates/mobile-linux-core/src/store/install.rs) — Committed source (eb82dcd73080bc35d0236abc15209a3bbee9f52c)
- [mobile-linux-runtime/crates/mobile-linux-core/src/store/lifecycle.rs](https://github.com/lingxi-coder/mobile-linux-runtime/blob/eb82dcd73080bc35d0236abc15209a3bbee9f52c/crates/mobile-linux-core/src/store/lifecycle.rs) — Committed source (eb82dcd73080bc35d0236abc15209a3bbee9f52c)
- [mobile-linux-runtime/crates/mobile-linux-android/src/runtime/implementation.rs](https://github.com/lingxi-coder/mobile-linux-runtime/blob/eb82dcd73080bc35d0236abc15209a3bbee9f52c/crates/mobile-linux-android/src/runtime/implementation.rs) — Committed source (eb82dcd73080bc35d0236abc15209a3bbee9f52c)
- [mobile-linux-runtime/crates/mobile-linux-ios/src/runtime/implementation.rs](https://github.com/lingxi-coder/mobile-linux-runtime/blob/eb82dcd73080bc35d0236abc15209a3bbee9f52c/crates/mobile-linux-ios/src/runtime/implementation.rs) — Committed source (eb82dcd73080bc35d0236abc15209a3bbee9f52c)
- [mobile-linux-runtime/crates/mobile-linux-ios/src/runtime/tests.rs](https://github.com/lingxi-coder/mobile-linux-runtime/blob/eb82dcd73080bc35d0236abc15209a3bbee9f52c/crates/mobile-linux-ios/src/runtime/tests.rs) — Committed source (eb82dcd73080bc35d0236abc15209a3bbee9f52c)

- mobile-linux-runtime: HEAD eb82dcd73080bc35d0236abc15209a3bbee9f52c; Referenced input fingerprint sha256:78374cc6ec701b4b4e082679fb8033d488b6379c3345bd5d03cd383177c3daf4; Product pin b65f8856d773d27ebf73b0cade907884d3080704

<!-- Generated from apps/web/src/data/docs. Do not edit this file directly. -->
