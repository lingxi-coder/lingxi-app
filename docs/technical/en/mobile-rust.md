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

<a id="command-fields"></a>

## Command parameters and result checks

The request is an executable plus an argument vector. Shell operators, variable expansion and redirection are interpreted only when you explicitly invoke a shell, for example command=/bin/sh with args=[-c, script]. Keep user-selected arguments separate from shell source when the feature does not need a shell.

| Parameter / result | Contract | Caller decision |
| --- | --- | --- |
| command, args | Non-empty command; arguments are individual values, not a joined command string. | Prefer an installed guest executable and a concrete argument list. |
| cwd | Optional absolute guest path; it is not a native host path. | Use /home/lingxi for a first command or the configured guest project path. |
| env | Rust BTreeMap; FFI uses key/value records. Environment keys must be non-empty and cannot contain = or NUL. | Supply values explicitly; do not assume shell expansion occurs inside a value. |
| stdin, timeout_ms | Finite command input is optional text; a provided timeout must be positive. | Use raw stdio for binary or ongoing input, and set a budget appropriate to the operation. |
| network, resource_limits, mounts | Requested policy and filesystem exposure; success still needs the matching enforcement receipt. | Choose the necessary policy before admission; a policy error is not permission to retry with weaker isolation. |
| exit_code, timed_out, cancelled | A returned result can describe a failed or interrupted command. SDK errors are a separate result path. | Check all three before marking the operation successful; retain stderr for diagnosis. |

<a id="stream-consumption"></a>

## Consume live output without blocking execution

ProcessStreamSink has separate stdout_line(String) and stderr_chunk(Vec<u8>) callbacks. Ordinary stdout line endings are removed; stderr chunks do not promise line boundaries or complete UTF-8 characters. The Android reader also emits bounded stdout fragments for an unusually long line, so a callback is not a general-purpose binary framing boundary.

Drain stdout and stderr together. Forward output into a bounded host queue, then render on the UI executor. Returning a ProcessError from a sink stops delivery and causes the platform runner to tear down the owned command; a slow sink applies backpressure rather than producing unlimited host memory.

The portable trait provides a run-then-replay fallback. Probe streaming_output and use the selected backend implementation when real-time output is part of your feature. Receiving some output does not establish successful completion; await the command result and check its terminal fields.

### Reusable streaming helper · host supplies runtime and sink

```rust
use mobile_linux_api::{
    LinuxCommandRequest, LinuxCommandResult, MobileLinuxError,
    MobileLinuxRuntime, ProcessStreamSink,
};
use std::sync::Arc;

pub async fn stream_command(
    runtime: &dyn MobileLinuxRuntime,
    request: LinuxCommandRequest,
    sink: Arc<dyn ProcessStreamSink>,
) -> Result<LinuxCommandResult, MobileLinuxError> {
    let network = request.network;
    let limits = request.resource_limits;
    let result = runtime.run_streaming(request, sink).await?;
    result.enforcement.ensure_for(network, limits)?;
    Ok(result)
}
```

Verification method: Compile check (mobile-stream-command)

This helper assumes the host already owns and booted the runtime. Its caller still checks the command exit state and owns shutdown.

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

<a id="raw-read-loop"></a>

## Implement the raw stdio read loop

1. Open a RawStdioOpenRequest with explicit command, arguments and policy. Retain the returned session handle with the runtime owner.
2. Write complete protocol frames with write_raw_stdio. Keep framing independent of how the SDK splits later reads.
3. Read a bounded amount, for example 65,536 bytes. Append stdout to the protocol parser and drain stderr to diagnostics; neither buffer is guaranteed to contain a complete message.
4. A read with empty buffers and closed=false is not EOF. Yield or wait according to the host scheduling policy before polling again. Finish only when closed is true, handling the optional exit_code.
5. On normal completion, protocol failure or user cancellation, await close_raw_stdio. Closing owns process-tree cleanup; dropping a screen or stopping a read loop does not replace it.

FFI raw sessions reject command-request stdin and timeout_ms. Their input and lifetime are controlled through write/close calls. FFI max_bytes is 1..1048576; do not issue a zero-byte readiness probe.

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

<a id="event-cursor"></a>

## Resume monitoring with event cursors and task snapshots

| Operation | Meaning | Host handling |
| --- | --- | --- |
| read_events(None, limit) | Read currently retained events after sequence zero. The maximum batch is 512. | Use a nonzero bounded limit and store the largest sequence you processed. |
| read_events(Some(last), limit) | Returns events with sequence strictly greater than last. | Advance after processing the batch; do not add one to last before sending the cursor. |
| TaskStatusChanged / PtyClosed | Terminal state and exit detail are distinct from output frames. | Stop the activity indicator from terminal state, not from an idle stdout channel. |
| list_tasks / task_status | Current task snapshots; task_status can return None for an unknown ID. | Reconcile a recreated UI with these snapshots. Do not treat None as a successful command. |

Streaming callbacks from RuntimeHandle.run_command_streaming use a separate per-call run-N identifier and sequence 0. Do not overwrite a backend read_events cursor with callback metadata. The runtime event buffer is not a durable transcript store.

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
        cwd: Some("/home/lingxi".into()),
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

- [mobile-linux-runtime/crates/mobile-linux-api/src/mobile_linux.rs](https://github.com/lingxi-coder/mobile-linux-runtime/blob/d96c0b9b40a7d0efbf1e14e333e06b6d641ac6d9/crates/mobile-linux-api/src/mobile_linux.rs) — Committed source (d96c0b9b40a7d0efbf1e14e333e06b6d641ac6d9)
- [mobile-linux-runtime/crates/mobile-linux-api/src/execution.rs](https://github.com/lingxi-coder/mobile-linux-runtime/blob/d96c0b9b40a7d0efbf1e14e333e06b6d641ac6d9/crates/mobile-linux-api/src/execution.rs) — Committed source (d96c0b9b40a7d0efbf1e14e333e06b6d641ac6d9)
- [mobile-linux-runtime/crates/mobile-linux-ffi/src/runtime.rs](https://github.com/lingxi-coder/mobile-linux-runtime/blob/d96c0b9b40a7d0efbf1e14e333e06b6d641ac6d9/crates/mobile-linux-ffi/src/runtime.rs) — Committed source (d96c0b9b40a7d0efbf1e14e333e06b6d641ac6d9)
- [mobile-linux-runtime/crates/mobile-linux-core/src/store/install.rs](https://github.com/lingxi-coder/mobile-linux-runtime/blob/d96c0b9b40a7d0efbf1e14e333e06b6d641ac6d9/crates/mobile-linux-core/src/store/install.rs) — Committed source (d96c0b9b40a7d0efbf1e14e333e06b6d641ac6d9)
- [mobile-linux-runtime/crates/mobile-linux-core/src/store/lifecycle.rs](https://github.com/lingxi-coder/mobile-linux-runtime/blob/d96c0b9b40a7d0efbf1e14e333e06b6d641ac6d9/crates/mobile-linux-core/src/store/lifecycle.rs) — Committed source (d96c0b9b40a7d0efbf1e14e333e06b6d641ac6d9)
- [mobile-linux-runtime/crates/mobile-linux-android/src/runtime/implementation.rs](https://github.com/lingxi-coder/mobile-linux-runtime/blob/d96c0b9b40a7d0efbf1e14e333e06b6d641ac6d9/crates/mobile-linux-android/src/runtime/implementation.rs) — Committed source (d96c0b9b40a7d0efbf1e14e333e06b6d641ac6d9)
- [mobile-linux-runtime/crates/mobile-linux-ios/src/runtime/implementation.rs](https://github.com/lingxi-coder/mobile-linux-runtime/blob/d96c0b9b40a7d0efbf1e14e333e06b6d641ac6d9/crates/mobile-linux-ios/src/runtime/implementation.rs) — Committed source (d96c0b9b40a7d0efbf1e14e333e06b6d641ac6d9)
- [mobile-linux-runtime/crates/mobile-linux-ios/src/runtime/tests.rs](https://github.com/lingxi-coder/mobile-linux-runtime/blob/d96c0b9b40a7d0efbf1e14e333e06b6d641ac6d9/crates/mobile-linux-ios/src/runtime/tests.rs) — Committed source (d96c0b9b40a7d0efbf1e14e333e06b6d641ac6d9)
- [mobile-linux-runtime/crates/mobile-linux-android/src/runtime/process.rs](https://github.com/lingxi-coder/mobile-linux-runtime/blob/d96c0b9b40a7d0efbf1e14e333e06b6d641ac6d9/crates/mobile-linux-android/src/runtime/process.rs) — Committed source (d96c0b9b40a7d0efbf1e14e333e06b6d641ac6d9)
- [mobile-linux-runtime/crates/mobile-linux-android/src/runtime/streaming.rs](https://github.com/lingxi-coder/mobile-linux-runtime/blob/d96c0b9b40a7d0efbf1e14e333e06b6d641ac6d9/crates/mobile-linux-android/src/runtime/streaming.rs) — Committed source (d96c0b9b40a7d0efbf1e14e333e06b6d641ac6d9)
- [mobile-linux-runtime/crates/mobile-linux-ios/src/runtime/validation.rs](https://github.com/lingxi-coder/mobile-linux-runtime/blob/d96c0b9b40a7d0efbf1e14e333e06b6d641ac6d9/crates/mobile-linux-ios/src/runtime/validation.rs) — Committed source (d96c0b9b40a7d0efbf1e14e333e06b6d641ac6d9)

- mobile-linux-runtime: HEAD d96c0b9b40a7d0efbf1e14e333e06b6d641ac6d9; Referenced input fingerprint sha256:c85076a3035f8a036929b8e4a66cfae06a1e6eac952ef5315ba1cba3470d2139; Product pin d96c0b9b40a7d0efbf1e14e333e06b6d641ac6d9

<!-- Generated from apps/web/src/data/docs. Do not edit this file directly. -->
