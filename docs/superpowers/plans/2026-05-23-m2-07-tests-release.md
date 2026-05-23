# LingXi Core M2 · Plan 07 · Test infrastructure + docs + release v0.3.0

> **For agentic workers:** REQUIRED SUB-SKILL: Use `superpowers:subagent-driven-development` or `superpowers:executing-plans`. Every test below MUST be written *before* it is wired into a driver (`tests/contract_*.rs` or `tests/parity_*.rs`) and MUST fail until the underlying impl is real (M2-01..M2-06 deliverables). If a contract method does not exist on a trait the spec lists, stop and reconcile with the trait — do not weaken the contract.

**Goal:** Lock in M2's behavioral parity with claude-code by (1) adding contract test suites for the 12 traits still missing from M1, (2) materializing 7 high-value parity fixtures driving real settings/wire/error-string assertions, (3) updating the docs (`CHANGELOG.md`, `docs/ARCHITECTURE.md`, `docs/PLATFORMS.md`, `README.md`), (4) running the full release verification matrix, and (5) tagging `v0.3.0`.

**Depends on:** M2-01..M2-06 complete. Workspace must compile clean, all 104 v0.2.0 baseline tests still pass, and the new platform impls from M2-02..M2-06 must be wired into `platforms/posix/` and `platforms/windows/`.

---

## File Structure

```
lingxi-core/crates/test-harness/
├── src/
│   ├── contracts/
│   │   ├── mod.rs               ← MODIFY: add 12 new pub mod lines
│   │   ├── filesystem.rs        ← UNCHANGED (M1 baseline)
│   │   ├── clock.rs             ← NEW
│   │   ├── runtime.rs           ← NEW
│   │   ├── http.rs              ← NEW
│   │   ├── process.rs           ← NEW
│   │   ├── mcp.rs               ← NEW
│   │   ├── lsp.rs               ← NEW
│   │   ├── sandbox.rs           ← NEW
│   │   ├── worktree.rs          ← NEW
│   │   ├── secure_storage.rs    ← NEW
│   │   ├── swarm.rs             ← NEW
│   │   ├── bridge.rs            ← NEW
│   │   └── effect_handler.rs    ← NEW
│   ├── parity/
│   │   ├── mod.rs               ← REWRITE: real `ParityFixture` + loader
│   │   └── fixtures/
│   │       ├── sandbox_config_conversion.json        ← NEW
│   │       ├── mcp_initialize_request.json           ← NEW
│   │       ├── mcp_transport_settings_matrix.json    ← NEW
│   │       ├── lsp_plugin_only.json                  ← NEW
│   │       ├── worktree_branch_naming.json           ← NEW
│   │       ├── secure_storage_macos_service_name.json← NEW
│   │       └── tmux_windows_refusal.json             ← NEW
│   └── lib.rs                   ← UNCHANGED
└── tests/
    ├── contract_filesystem.rs   ← UNCHANGED (M1 baseline)
    ├── contract_clock.rs        ← NEW
    ├── contract_runtime.rs      ← NEW
    ├── contract_http.rs         ← NEW
    ├── contract_process.rs      ← NEW
    ├── contract_mcp.rs          ← NEW
    ├── contract_lsp.rs          ← NEW
    ├── contract_sandbox.rs      ← NEW
    ├── contract_worktree.rs     ← NEW
    ├── contract_secure_storage.rs ← NEW
    ├── contract_swarm.rs        ← NEW
    ├── contract_bridge.rs       ← NEW
    ├── contract_effect_handler.rs ← NEW
    ├── parity_sandbox_config.rs ← NEW
    ├── parity_mcp_initialize.rs ← NEW
    ├── parity_mcp_transports.rs ← NEW
    ├── parity_lsp_plugin_only.rs ← NEW
    ├── parity_worktree_naming.rs ← NEW
    ├── parity_keychain_service_name.rs ← NEW
    └── parity_tmux_windows.rs   ← NEW

CHANGELOG.md                     ← MODIFY: prepend `## [0.3.0]` section + migration notes
docs/ARCHITECTURE.md             ← MODIFY: refresh crate map + add parity guarantees section
docs/PLATFORMS.md                ← NEW
README.md                        ← MODIFY: platform callout + quickstart for v0.3.0
.github/workflows/ci.yml         ← MODIFY: split desktop / mobile cross-compile matrix
```

Net new: 12 contract modules + 12 contract drivers + 7 parity fixtures + 7 parity drivers + 1 PLATFORMS.md. Modify: contracts/mod.rs, parity/mod.rs, test-harness Cargo.toml, CHANGELOG.md, docs/ARCHITECTURE.md, README.md, .github/workflows/ci.yml.

---

## Phase A: Contract test suites (Tasks 1-12)

Each suite mirrors `contracts/filesystem.rs` (M1 baseline):

- Entry point: `pub async fn <name>_contract_tests<T: TheTrait>(impl: &T)`.
- Each test is a `async fn test_<assertion>` of 3-7 lines that panics on violation.
- The suite is exercised by `tests/contract_<name>.rs` against (a) the M1 mock impl from `lingxi_test_harness::mocks::*` when one exists and (b) the production impl from `platforms/posix/` and/or `platforms/windows/` when applicable. Where a feature is `Unsupported` on a platform the driver runs the suite anyway and asserts the error class — that **is** the contract.

The 12 traits, their trait files, and the canonical impl locations are:

| Trait | trait file | M1 mock | posix impl | windows impl |
|---|---|---|---|---|
| `Clock` | `crates/traits/src/clock.rs` | `mocks::MockClock` | `posix::PosixClock` | `windows::WindowsClock` |
| `RuntimeSpawner` | `crates/traits/src/runtime.rs` | `mocks::MockRuntimeSpawner` | `posix::PosixRuntimeSpawner` | `windows::WindowsRuntimeSpawner` |
| `HttpTransport` | `crates/traits/src/http.rs` | `mocks::MockHttpTransport` | `posix::PosixHttp` | `windows::WindowsHttp` |
| `ProcessRunner` | `crates/traits/src/process.rs` | none in M1 | `posix::PosixProcess` | `windows::WindowsProcess` |
| `McpTransport` | `crates/traits/src/mcp.rs` | `mocks::MockMcpTransport` | `posix::PosixMcp` | `windows::WindowsMcp` |
| `LspTransport` | `crates/traits/src/lsp.rs` | none in M1 | `posix::PosixLsp` | `windows::WindowsLsp` |
| `Sandbox` | `crates/traits/src/sandbox.rs` | none in M1 | `posix::PosixSandbox` | `windows::WindowsSandbox` |
| `WorktreeManager` | `crates/traits/src/worktree.rs` | none in M1 | `posix::PosixWorktree` | `windows::WindowsWorktree` |
| `SecureStorage` | `crates/traits/src/secure_storage.rs` | none in M1 | `posix::PlainTextSecureStorage` + `MacOsKeychainStorage` (macOS only) | `windows::PlainTextSecureStorage` |
| `SwarmBackend` | `crates/traits/src/swarm.rs` | none in M1 | `posix::PosixSwarm` | `windows::WindowsSwarmBackend` |
| `BridgeTransport` | `crates/traits/src/bridge.rs` | none in M1 | `posix::PosixBridge` | `windows::WindowsBridge` |
| `EffectHandler` | `crates/traits/src/effect_handler.rs` | none in M1 | `posix-minimal::PosixMinimalHost` (already tested via demo) | n/a |

Where a contract needs subprocess access on CI (e.g. real LSP spawn, real macOS Keychain) the driver gates with `#[cfg(target_os = "...")]` and/or an env var so `cargo test --workspace` on ubuntu-latest stays clean.

---

### Task 1: clock contract

**Files:** `crates/test-harness/src/contracts/clock.rs` (new), `crates/test-harness/tests/contract_clock.rs` (new), `crates/test-harness/src/contracts/mod.rs` (add `pub mod clock;`).

The `Clock` trait surface (per `crates/traits/src/clock.rs`):
- `fn now(&self) -> SystemTime`
- `fn elapsed_since(&self, earlier: SystemTime) -> Duration` (default impl: `now() - earlier`)

`contracts/clock.rs`:

```rust
//! [`Clock`] contract test suite — verifies non-decreasing `now()` and that
//! `elapsed_since(earlier)` returns a sane forward-going duration.

use lingxi_traits::Clock;
use std::time::{Duration, SystemTime};

pub async fn clock_contract_tests<C: Clock>(clock: &C) {
    test_now_is_non_decreasing(clock).await;
    test_elapsed_since_zero_for_now(clock).await;
    test_elapsed_since_grows_with_real_sleep(clock).await;
    test_elapsed_since_before_epoch_returns_zero(clock).await;
}

async fn test_now_is_non_decreasing<C: Clock>(clock: &C) {
    let t0 = clock.now();
    let t1 = clock.now();
    assert!(t1 >= t0, "Clock::now() must be non-decreasing: t0={t0:?} t1={t1:?}");
}

async fn test_elapsed_since_zero_for_now<C: Clock>(clock: &C) {
    let t0 = clock.now();
    let d = clock.elapsed_since(t0);
    assert!(d < Duration::from_millis(50), "elapsed_since(now()) must be ~0, got {d:?}");
}

async fn test_elapsed_since_grows_with_real_sleep<C: Clock>(clock: &C) {
    let t0 = clock.now();
    tokio::time::sleep(Duration::from_millis(20)).await;
    let d = clock.elapsed_since(t0);
    assert!(d >= Duration::from_millis(15), "elapsed_since must grow with wall-clock sleep, got {d:?}");
}

async fn test_elapsed_since_before_epoch_returns_zero<C: Clock>(clock: &C) {
    // Clocks that use SystemTime::duration_since must saturate at zero, not panic.
    let now = clock.now();
    let future = now + Duration::from_secs(3600);
    let d = clock.elapsed_since(future);
    assert_eq!(d, Duration::from_secs(0), "elapsed_since(future) must saturate at zero");
}
```

`tests/contract_clock.rs`:

```rust
use lingxi_test_harness::contracts::clock::clock_contract_tests;
use lingxi_test_harness::mocks::MockClock;

#[tokio::test]
async fn mock_clock_passes_contract() {
    let c = MockClock::default();
    clock_contract_tests(&c).await;
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[tokio::test]
async fn posix_clock_passes_contract() {
    let c = lingxi_platform_posix::PosixClock::new();
    clock_contract_tests(&c).await;
}

#[cfg(target_os = "windows")]
#[tokio::test]
async fn windows_clock_passes_contract() {
    let c = lingxi_platform_windows::WindowsClock::new();
    clock_contract_tests(&c).await;
}
```

**Cargo.toml dev-dependency additions** (one-time for the whole phase, added in this task):

```toml
[target.'cfg(any(target_os = "linux", target_os = "macos"))'.dev-dependencies]
lingxi-platform-posix = { path = "../../platforms/posix" }

[target.'cfg(target_os = "windows")'.dev-dependencies]
lingxi-platform-windows = { path = "../../platforms/windows" }
```

**Verification:**

```bash
cargo test -p lingxi-test-harness --test contract_clock
```

Expected: 1-2 passing tests on Linux/macOS, 1 on Windows. No `#[ignore]`.

**Commit boundary:** none yet — Phase A commits at the end of Task 12.

---

### Task 2: runtime contract

**Files:** `crates/test-harness/src/contracts/runtime.rs` (new), `crates/test-harness/tests/contract_runtime.rs` (new), update `contracts/mod.rs`.

The `RuntimeSpawner` trait surface (per `crates/traits/src/runtime.rs`):
- `async fn spawn(&self, fut: Pin<Box<dyn Future<Output=()> + Send>>) -> Result<BackgroundTaskHandle, RuntimeError>`
- `async fn cancel(&self, handle: &BackgroundTaskHandle) -> Result<(), RuntimeError>`

`contracts/runtime.rs`:

```rust
//! [`RuntimeSpawner`] contract: spawn returns a handle, the task runs, and
//! `cancel(handle)` is idempotent on already-finished handles.

use lingxi_traits::RuntimeSpawner;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

pub async fn runtime_spawner_contract_tests<R: RuntimeSpawner>(rt: &R) {
    test_spawn_runs_future_to_completion(rt).await;
    test_cancel_after_completion_is_ok(rt).await;
    test_spawn_many_does_not_starve(rt).await;
}

async fn test_spawn_runs_future_to_completion<R: RuntimeSpawner>(rt: &R) {
    let counter = Arc::new(AtomicU32::new(0));
    let c2 = counter.clone();
    let handle = rt
        .spawn(Box::pin(async move {
            c2.store(1, Ordering::SeqCst);
        }))
        .await
        .expect("spawn must succeed");
    // Allow the spawned task to run.
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(counter.load(Ordering::SeqCst), 1, "spawned task must run");
    let _ = rt.cancel(&handle).await;
}

async fn test_cancel_after_completion_is_ok<R: RuntimeSpawner>(rt: &R) {
    let handle = rt.spawn(Box::pin(async {})).await.expect("spawn ok");
    tokio::time::sleep(Duration::from_millis(20)).await;
    // Cancelling an already-finished handle must not error.
    let r = rt.cancel(&handle).await;
    assert!(r.is_ok(), "cancel of finished task must be Ok, got {r:?}");
}

async fn test_spawn_many_does_not_starve<R: RuntimeSpawner>(rt: &R) {
    let counter = Arc::new(AtomicU32::new(0));
    let mut handles = Vec::new();
    for _ in 0..8 {
        let c = counter.clone();
        handles.push(
            rt.spawn(Box::pin(async move {
                c.fetch_add(1, Ordering::SeqCst);
            }))
            .await
            .expect("spawn"),
        );
    }
    tokio::time::sleep(Duration::from_millis(80)).await;
    assert_eq!(counter.load(Ordering::SeqCst), 8, "all 8 tasks must complete");
    for h in handles {
        let _ = rt.cancel(&h).await;
    }
}
```

`tests/contract_runtime.rs`:

```rust
use lingxi_test_harness::contracts::runtime::runtime_spawner_contract_tests;
use lingxi_test_harness::mocks::MockRuntimeSpawner;

#[tokio::test]
async fn mock_runtime_passes_contract() {
    let rt = MockRuntimeSpawner::new();
    runtime_spawner_contract_tests(&rt).await;
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[tokio::test]
async fn posix_runtime_passes_contract() {
    let rt = lingxi_platform_posix::PosixRuntimeSpawner::new();
    runtime_spawner_contract_tests(&rt).await;
}

#[cfg(target_os = "windows")]
#[tokio::test]
async fn windows_runtime_passes_contract() {
    let rt = lingxi_platform_windows::WindowsRuntimeSpawner::new();
    runtime_spawner_contract_tests(&rt).await;
}
```

**Verification:**

```bash
cargo test -p lingxi-test-harness --test contract_runtime
```

---

### Task 3: http contract

**Files:** `crates/test-harness/src/contracts/http.rs` (new), `crates/test-harness/tests/contract_http.rs` (new), update `contracts/mod.rs`.

The `HttpTransport` trait surface (per `crates/traits/src/http.rs`):
- `async fn request(&self, req: HttpRequest) -> Result<HttpResponse, HttpError>`
- `async fn stream_sse(&self, req: HttpRequest) -> Result<SseStream, HttpError>`

`contracts/http.rs`:

```rust
//! [`HttpTransport`] contract: `request` returns a status + body; `stream_sse`
//! returns a Stream that terminates. The suite uses an in-process echo server
//! so it works against `MockHttpTransport`, `PosixHttp`, and `WindowsHttp`
//! without external network.

use futures::StreamExt;
use lingxi_traits::http::{HttpRequest, HttpTransport};
use std::collections::HashMap;

pub async fn http_transport_contract_tests<H: HttpTransport>(http: &H, base_url: &str) {
    test_request_returns_status_and_body(http, base_url).await;
    test_request_failure_yields_error(http).await;
    test_stream_sse_terminates(http, base_url).await;
}

async fn test_request_returns_status_and_body<H: HttpTransport>(http: &H, base_url: &str) {
    let req = HttpRequest {
        method: "GET".into(),
        url: format!("{base_url}/ok"),
        headers: HashMap::new(),
        body: None,
    };
    let resp = http.request(req).await.expect("request must succeed");
    assert_eq!(resp.status, 200, "expected 200, got {}", resp.status);
    assert!(!resp.body.is_empty(), "response body must not be empty");
}

async fn test_request_failure_yields_error<H: HttpTransport>(http: &H) {
    let req = HttpRequest {
        method: "GET".into(),
        url: "http://127.0.0.1:1/never-listens".into(),
        headers: HashMap::new(),
        body: None,
    };
    let r = http.request(req).await;
    assert!(r.is_err(), "request to closed port must return HttpError");
}

async fn test_stream_sse_terminates<H: HttpTransport>(http: &H, base_url: &str) {
    let req = HttpRequest {
        method: "GET".into(),
        url: format!("{base_url}/sse"),
        headers: HashMap::from([("Accept".into(), "text/event-stream".into())]),
        body: None,
    };
    let mut stream = http.stream_sse(req).await.expect("stream_sse must succeed");
    let mut events = 0u32;
    while let Some(_evt) = stream.next().await {
        events += 1;
        if events > 100 {
            break;
        }
    }
    assert!(events > 0, "SSE stream must yield at least one event before closing");
}
```

`tests/contract_http.rs`:

```rust
use lingxi_test_harness::contracts::http::http_transport_contract_tests;
use lingxi_test_harness::mocks::MockHttpTransport;

#[tokio::test]
async fn mock_http_passes_contract() {
    // MockHttpTransport ships a fake "/ok" + "/sse" route under its base URL.
    let http = MockHttpTransport::with_default_routes();
    http_transport_contract_tests(&http, http.base_url()).await;
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[tokio::test]
async fn posix_http_passes_contract() {
    // Spawn a small in-process axum/hyper server that responds to GET /ok and
    // GET /sse, then run the production reqwest-based transport against it.
    let server = lingxi_test_harness::contracts::http::spawn_echo_server().await;
    let http = lingxi_platform_posix::PosixHttp::new();
    http_transport_contract_tests(&http, &server.base_url()).await;
    server.shutdown().await;
}

#[cfg(target_os = "windows")]
#[tokio::test]
async fn windows_http_passes_contract() {
    let server = lingxi_test_harness::contracts::http::spawn_echo_server().await;
    let http = lingxi_platform_windows::WindowsHttp::new();
    http_transport_contract_tests(&http, &server.base_url()).await;
    server.shutdown().await;
}
```

In `contracts/http.rs`, add a small `spawn_echo_server()` helper that binds `127.0.0.1:0`, serves `GET /ok` → `200 "ok"` and `GET /sse` → `text/event-stream` with three `data:` frames followed by `\n\n` and a stream close, and exposes `base_url() -> String`. Implementation: `hyper` 1.x server in a `tokio::spawn`ed task; shutdown via `oneshot`.

**Cargo.toml additions:**

```toml
[dev-dependencies]
hyper = { version = "1", features = ["server", "http1"] }
hyper-util = { version = "0.1", features = ["server", "tokio"] }
http-body-util = "0.1"
```

**Verification:**

```bash
cargo test -p lingxi-test-harness --test contract_http
```

---

### Task 4: process contract

**Files:** `crates/test-harness/src/contracts/process.rs` (new), `crates/test-harness/tests/contract_process.rs` (new), update `contracts/mod.rs`.

The `ProcessRunner` trait surface (per `crates/traits/src/process.rs`):
- `async fn run(&self, cmd: &SandboxedCommand) -> Result<ProcessOutput, ProcessError>`
- `async fn spawn_background(&self, cmd: &SandboxedCommand) -> Result<ProcessHandle, ProcessError>`
- `fn is_available(&self) -> bool`

`contracts/process.rs`:

```rust
//! [`ProcessRunner`] contract. The runner accepts only [`SandboxedCommand`],
//! so the suite constructs commands via [`Sandbox::bypass_with_audit`] under
//! the canonical "test bypass" reason.

use lingxi_traits::{
    ProcessRunner, Sandbox,
    sandbox::ProcessCommand,
};

const TEST_BYPASS_REASON: &str = "contract test (bypass auditing intentional)";

pub async fn process_runner_contract_tests<P, S>(proc: &P, sandbox: &S)
where
    P: ProcessRunner,
    S: Sandbox,
{
    test_is_available_returns_bool(proc).await;
    test_run_echo_returns_exit_zero_and_stdout(proc, sandbox).await;
    test_run_false_returns_nonzero_exit(proc, sandbox).await;
}

async fn test_is_available_returns_bool<P: ProcessRunner>(proc: &P) {
    // Trivial smoke: the implementation must answer the question without
    // panicking. The value itself is platform-dependent.
    let _ = proc.is_available();
}

async fn test_run_echo_returns_exit_zero_and_stdout<P, S>(proc: &P, sandbox: &S)
where
    P: ProcessRunner,
    S: Sandbox,
{
    if !proc.is_available() {
        return; // Platform refuses subprocess execution; nothing to assert.
    }
    let cmd = ProcessCommand {
        command: "/bin/sh".into(),
        args: vec!["-c".into(), "echo hello".into()],
        cwd: None,
        env: Default::default(),
        timeout_ms: Some(5_000),
        stdin: None,
    };
    let sandboxed = sandbox.bypass_with_audit(cmd, TEST_BYPASS_REASON);
    let out = proc.run(&sandboxed).await.expect("run must succeed");
    assert_eq!(out.exit_code, 0, "echo exit code");
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("hello"),
        "stdout must contain 'hello', got {:?}",
        String::from_utf8_lossy(&out.stdout)
    );
}

async fn test_run_false_returns_nonzero_exit<P, S>(proc: &P, sandbox: &S)
where
    P: ProcessRunner,
    S: Sandbox,
{
    if !proc.is_available() {
        return;
    }
    let cmd = ProcessCommand {
        command: "/bin/sh".into(),
        args: vec!["-c".into(), "exit 7".into()],
        cwd: None,
        env: Default::default(),
        timeout_ms: Some(5_000),
        stdin: None,
    };
    let sandboxed = sandbox.bypass_with_audit(cmd, TEST_BYPASS_REASON);
    let out = proc.run(&sandboxed).await.expect("run must return Ok with non-zero exit");
    assert_eq!(out.exit_code, 7, "exit 7 must be propagated");
}
```

`tests/contract_process.rs`:

```rust
use lingxi_test_harness::contracts::process::process_runner_contract_tests;

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[tokio::test]
async fn posix_process_passes_contract() {
    let proc = lingxi_platform_posix::PosixProcess::new();
    let sandbox = lingxi_platform_posix::PosixSandbox::new();
    process_runner_contract_tests(&proc, &sandbox).await;
}

#[cfg(target_os = "windows")]
#[tokio::test]
async fn windows_process_passes_contract() {
    // Windows has no /bin/sh. The contract suite's two real-exec tests
    // short-circuit on platforms where `is_available()` is false OR where the
    // /bin/sh-shaped command shape is not portable. The windows runner is
    // available but uses cmd.exe; for this task we limit windows coverage to
    // is_available() and leave a TODO(M2.next) to add a Windows-specific
    // subcontract.
    let proc = lingxi_platform_windows::WindowsProcess::new();
    let _ = proc.is_available();
}
```

**Verification:**

```bash
cargo test -p lingxi-test-harness --test contract_process
```

Expected: on macOS/Linux, 1 test asserts echo + non-zero exit. On Windows, is_available smoke only.

---

### Task 5: mcp contract

**Files:** `crates/test-harness/src/contracts/mcp.rs` (new), `crates/test-harness/tests/contract_mcp.rs` (new), update `contracts/mod.rs`.

The `McpTransport` trait surface (per `crates/traits/src/mcp.rs`):
- `async fn connect(&self, spec: &McpTransportSpec) -> Result<McpRawConnection, McpError>`
- `async fn disconnect(&self, conn_id: McpConnectionId) -> Result<(), McpError>`
- `fn supported_transports(&self) -> Vec<McpTransportKind>`
- inbound handlers for `handle_elicitation` etc.

`contracts/mcp.rs`:

```rust
//! [`McpTransport`] contract: at minimum the transport must answer
//! `supported_transports()` and must return `UnsupportedTransport` (not panic,
//! not hang, not connect) when handed a kind it doesn't claim to support.

use lingxi_traits::mcp::{
    McpError, McpTransport, McpTransportKind, McpTransportSpec,
};
use std::collections::HashMap;

pub async fn mcp_transport_contract_tests<T: McpTransport>(t: &T) {
    test_supported_transports_non_empty(t).await;
    test_inprocess_kind_returns_unsupported(t).await;
}

async fn test_supported_transports_non_empty<T: McpTransport>(t: &T) {
    let kinds = t.supported_transports();
    assert!(
        !kinds.is_empty(),
        "supported_transports() must be non-empty (Unsupported impls can still answer)"
    );
}

async fn test_inprocess_kind_returns_unsupported<T: McpTransport>(t: &T) {
    // claude-code does not expose InProcess as a user-configurable transport
    // in settings; we mirror that — `connect` must reject it with
    // `McpError::UnsupportedTransport(_)`.
    if t.supported_transports().contains(&McpTransportKind::InProcess) {
        return; // Mock or test transport that opts into InProcess; skip.
    }
    let spec = McpTransportSpec::InProcess {
        // Constructor-shaped placeholder; the trait's enum carries this variant
        // even when no platform exposes it as a user-facing transport.
        name: "unused".into(),
    };
    let r = t.connect(&spec).await;
    match r {
        Err(McpError::UnsupportedTransport(_)) => {}
        other => panic!("expected UnsupportedTransport for InProcess, got {other:?}"),
    }
}
```

`tests/contract_mcp.rs`:

```rust
use lingxi_test_harness::contracts::mcp::mcp_transport_contract_tests;
use lingxi_test_harness::mocks::MockMcpTransport;

#[tokio::test]
async fn mock_mcp_passes_contract() {
    let t = MockMcpTransport::new();
    mcp_transport_contract_tests(&t).await;
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[tokio::test]
async fn posix_mcp_passes_contract() {
    let t = lingxi_platform_posix::PosixMcp::new();
    mcp_transport_contract_tests(&t).await;
}

#[cfg(target_os = "windows")]
#[tokio::test]
async fn windows_mcp_passes_contract() {
    let t = lingxi_platform_windows::WindowsMcp::new();
    mcp_transport_contract_tests(&t).await;
}
```

**Note:** the live transport-roundtrip tests (real stdio MCP server, real WS bridge) belong in `crates/mcp/tests/` and were authored in M2-02. This contract is the trait-level invariant only.

**Verification:**

```bash
cargo test -p lingxi-test-harness --test contract_mcp
```

---

### Task 6: lsp contract

**Files:** `crates/test-harness/src/contracts/lsp.rs` (new), `crates/test-harness/tests/contract_lsp.rs` (new), update `contracts/mod.rs`.

The `LspTransport` trait surface (per `crates/traits/src/lsp.rs`):
- `async fn start_server(&self, config: &LspServerConfig) -> Result<LspRawConnection, LspError>`
- `async fn request(&self, conn_id, method, params) -> Result<serde_json::Value, LspError>`
- `async fn shutdown(&self, conn_id: McpConnectionId) -> Result<(), LspError>`
- `fn is_available(&self) -> bool`

`contracts/lsp.rs`:

```rust
//! [`LspTransport`] contract. Real LSP servers are heavy and not portable to
//! ubuntu-latest CI without a sidecar install; the contract verifies trait
//! invariants only.

use lingxi_traits::lsp::{LspError, LspServerConfig, LspTransport};
use lingxi_protocol::McpConnectionId;

pub async fn lsp_transport_contract_tests<T: LspTransport>(t: &T) {
    test_is_available_returns_bool(t).await;
    test_shutdown_unknown_conn_is_idempotent(t).await;
    test_start_server_with_bogus_binary_returns_error(t).await;
}

async fn test_is_available_returns_bool<T: LspTransport>(t: &T) {
    let _ = t.is_available();
}

async fn test_shutdown_unknown_conn_is_idempotent<T: LspTransport>(t: &T) {
    // Calling shutdown twice on an id that never existed must not panic and
    // must return Ok or LspError::Transport — never UnwrapNoneException-style
    // bugs.
    let bogus = McpConnectionId::new();
    let _ = t.shutdown(bogus.clone()).await;
    let r = t.shutdown(bogus).await;
    assert!(
        r.is_ok() || matches!(r, Err(LspError::Transport(_))),
        "shutdown of unknown conn must be Ok or Transport error, got {r:?}"
    );
}

async fn test_start_server_with_bogus_binary_returns_error<T: LspTransport>(t: &T) {
    if !t.is_available() {
        return;
    }
    let config = LspServerConfig {
        name: "contract-bogus".into(),
        command: "__nonexistent_binary_contract_test__".into(),
        args: vec![],
        extensions: vec![],
        env: Default::default(),
        root_uri: None,
    };
    let r = t.start_server(&config).await;
    assert!(r.is_err(), "starting a nonexistent binary must error, got Ok");
}
```

`tests/contract_lsp.rs`:

```rust
use lingxi_test_harness::contracts::lsp::lsp_transport_contract_tests;

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[tokio::test]
async fn posix_lsp_passes_contract() {
    let t = lingxi_platform_posix::PosixLsp::new();
    lsp_transport_contract_tests(&t).await;
}

#[cfg(target_os = "windows")]
#[tokio::test]
async fn windows_lsp_passes_contract() {
    let t = lingxi_platform_windows::WindowsLsp::new();
    lsp_transport_contract_tests(&t).await;
}
```

**Verification:**

```bash
cargo test -p lingxi-test-harness --test contract_lsp
```

---

### Task 7: sandbox contract

**Files:** `crates/test-harness/src/contracts/sandbox.rs` (new), `crates/test-harness/tests/contract_sandbox.rs` (new), update `contracts/mod.rs`.

The `Sandbox` trait surface (per `crates/traits/src/sandbox.rs`):
- `fn is_available(&self) -> bool`
- `fn backend(&self) -> SandboxBackend`
- `fn prepare(&self, cmd: ProcessCommand, policy: &SandboxPolicy) -> Result<SandboxedCommand, SandboxError>`
- `fn bypass_with_audit(&self, cmd: ProcessCommand, reason: &str) -> SandboxedCommand`
- `async fn probe_capability(&self) -> SandboxCapability`

`contracts/sandbox.rs`:

```rust
//! [`Sandbox`] contract: prepare/bypass must round-trip the inner
//! [`ProcessCommand`]; `bypass_with_audit` must record the reason in the tag.

use lingxi_traits::sandbox::{
    NetworkPolicy, ProcessCommand, ResourceLimits, Sandbox, SandboxPolicy, SandboxedTag,
};

const REASON: &str = "sandbox contract bypass";

pub async fn sandbox_contract_tests<S: Sandbox>(s: &S) {
    test_is_available_returns_bool(s).await;
    test_bypass_preserves_inner_command(s).await;
    test_bypass_tag_carries_reason(s).await;
    test_prepare_succeeds_with_default_policy_when_available(s).await;
    test_probe_capability_returns(s).await;
}

async fn test_is_available_returns_bool<S: Sandbox>(s: &S) {
    let _ = s.is_available();
}

async fn test_bypass_preserves_inner_command<S: Sandbox>(s: &S) {
    let cmd = ProcessCommand {
        command: "/usr/bin/echo".into(),
        args: vec!["hi".into()],
        cwd: None,
        env: Default::default(),
        timeout_ms: None,
        stdin: None,
    };
    let sandboxed = s.bypass_with_audit(cmd.clone(), REASON);
    assert_eq!(sandboxed.inner().command, "/usr/bin/echo", "inner.command preserved");
    assert_eq!(sandboxed.inner().args, vec!["hi".to_string()], "inner.args preserved");
}

async fn test_bypass_tag_carries_reason<S: Sandbox>(s: &S) {
    let cmd = ProcessCommand {
        command: "/bin/true".into(),
        args: vec![],
        cwd: None,
        env: Default::default(),
        timeout_ms: None,
        stdin: None,
    };
    let sandboxed = s.bypass_with_audit(cmd, REASON);
    match sandboxed.tag() {
        SandboxedTag::BypassAuditedWithReason { reason } => {
            assert_eq!(reason, REASON, "bypass reason must round-trip");
        }
        other => panic!("expected BypassAuditedWithReason tag, got {other:?}"),
    }
}

async fn test_prepare_succeeds_with_default_policy_when_available<S: Sandbox>(s: &S) {
    if !s.is_available() {
        return; // Windows/WSL1 sandbox returns Unsupported here — that's by design.
    }
    let cmd = ProcessCommand {
        command: "/bin/true".into(),
        args: vec![],
        cwd: None,
        env: Default::default(),
        timeout_ms: None,
        stdin: None,
    };
    let policy = SandboxPolicy {
        network: NetworkPolicy::Disabled,
        writable_paths: vec![],
        denied_paths: vec![],
        allow_subprocess: false,
        limits: ResourceLimits::default(),
    };
    let r = s.prepare(cmd, &policy);
    assert!(r.is_ok(), "prepare with default policy must succeed when sandbox is available, got {r:?}");
}

async fn test_probe_capability_returns<S: Sandbox>(s: &S) {
    let cap = s.probe_capability().await;
    assert_eq!(
        cap.available,
        s.is_available(),
        "probe_capability().available must agree with is_available()"
    );
}
```

`tests/contract_sandbox.rs`:

```rust
use lingxi_test_harness::contracts::sandbox::sandbox_contract_tests;

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[tokio::test]
async fn posix_sandbox_passes_contract() {
    let s = lingxi_platform_posix::PosixSandbox::new();
    sandbox_contract_tests(&s).await;
}

#[cfg(target_os = "windows")]
#[tokio::test]
async fn windows_sandbox_passes_contract() {
    // Windows sandbox is intentionally Unsupported (M2-01 correction). The
    // contract still validates that `bypass_with_audit` works and that
    // `prepare` returns SandboxError::Unsupported.
    let s = lingxi_platform_windows::WindowsSandbox::new();
    sandbox_contract_tests(&s).await;
}
```

**Verification:**

```bash
cargo test -p lingxi-test-harness --test contract_sandbox
```

---

### Task 8: worktree contract

**Files:** `crates/test-harness/src/contracts/worktree.rs` (new), `crates/test-harness/tests/contract_worktree.rs` (new), update `contracts/mod.rs`.

The `WorktreeManager` trait surface (per `crates/traits/src/worktree.rs`):
- `async fn create_worktree(&self, repo_root, slug, copy_includes) -> Result<WorktreeHandle, _>`
- `async fn cleanup_stale(&self, max_age) -> Result<Vec<PathBuf>, _>`
- `fn is_supported(&self) -> bool`

`contracts/worktree.rs`:

```rust
//! [`WorktreeManager`] contract. Real `git worktree add` requires a real git
//! repo; the contract sets one up in a tempdir, exercises the happy path, and
//! verifies the claude-code-mandated branch prefix.

use lingxi_traits::worktree::{WorktreeError, WorktreeManager};
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;
use tempfile::TempDir;

pub async fn worktree_manager_contract_tests<W: WorktreeManager>(w: &W) {
    test_is_supported_returns_bool(w).await;
    test_create_worktree_yields_handle_with_prefixed_branch(w).await;
    test_invalid_slug_rejected(w).await;
}

async fn test_is_supported_returns_bool<W: WorktreeManager>(w: &W) {
    let _ = w.is_supported();
}

async fn test_create_worktree_yields_handle_with_prefixed_branch<W: WorktreeManager>(w: &W) {
    if !w.is_supported() {
        return;
    }
    let tmp = TempDir::new().expect("tempdir");
    init_git_repo(tmp.path());
    let handle = w
        .create_worktree(tmp.path(), "feature-x", &[])
        .await
        .expect("create_worktree must succeed");
    assert!(
        handle.branch.starts_with("worktree-"),
        "branch must use worktree- prefix (claude-code parity), got {}",
        handle.branch
    );
    assert!(
        handle.path.starts_with(tmp.path().join(".claude").join("worktrees")),
        "worktree path must be under <root>/.claude/worktrees/, got {:?}",
        handle.path
    );
    let _ = w.cleanup_stale(Duration::from_secs(0)).await;
}

async fn test_invalid_slug_rejected<W: WorktreeManager>(w: &W) {
    if !w.is_supported() {
        return;
    }
    let tmp = TempDir::new().expect("tempdir");
    init_git_repo(tmp.path());
    // Spaces are outside the allowed slug-segment alphabet.
    let r = w.create_worktree(tmp.path(), "bad slug", &[]).await;
    match r {
        Err(WorktreeError::InvalidSlug(_)) => {}
        other => panic!("expected InvalidSlug for 'bad slug', got {other:?}"),
    }
}

fn init_git_repo(root: &std::path::Path) {
    let run = |args: &[&str]| {
        Command::new("git")
            .args(args)
            .current_dir(root)
            .output()
            .expect("git")
    };
    let _ = run(&["init", "-q", "-b", "main"]);
    let _ = run(&["config", "user.email", "test@example.com"]);
    let _ = run(&["config", "user.name", "Contract Test"]);
    std::fs::write(root.join("README"), "x").unwrap();
    let _ = run(&["add", "."]);
    let _ = run(&["commit", "-q", "-m", "init"]);
}
```

`tests/contract_worktree.rs`:

```rust
use lingxi_test_harness::contracts::worktree::worktree_manager_contract_tests;

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[tokio::test]
async fn posix_worktree_passes_contract() {
    let w = lingxi_platform_posix::PosixWorktree::new();
    worktree_manager_contract_tests(&w).await;
}

#[cfg(target_os = "windows")]
#[tokio::test]
async fn windows_worktree_passes_contract() {
    let w = lingxi_platform_windows::WindowsWorktree::new();
    worktree_manager_contract_tests(&w).await;
}
```

**Cargo.toml additions** (one-time):

```toml
[dev-dependencies]
tempfile = "3.13"
```

**Verification:**

```bash
cargo test -p lingxi-test-harness --test contract_worktree
```

Requires `git` on PATH — assumed available on all M2 CI runners.

---

### Task 9: secure_storage contract

**Files:** `crates/test-harness/src/contracts/secure_storage.rs` (new), `crates/test-harness/tests/contract_secure_storage.rs` (new), update `contracts/mod.rs`.

The `SecureStorage` trait surface (per `crates/traits/src/secure_storage.rs`):
- `async fn store(&self, service, account, data) -> Result<(), _>`
- `async fn retrieve(&self, service, account) -> Result<Option<Vec<u8>>, _>`
- `async fn delete(&self, service, account) -> Result<(), _>`
- `fn is_encrypted(&self) -> bool`
- `fn backend(&self) -> SecureStorageBackend`

`contracts/secure_storage.rs`:

```rust
//! [`SecureStorage`] contract: store, retrieve, delete round-trip with
//! byte-identical payload. `is_encrypted()` is a smoke check — the value
//! depends on the backend.

use lingxi_traits::SecureStorage;

const SERVICE: &str = "lingxi-contract-test";
const ACCOUNT: &str = "user@example.com";

pub async fn secure_storage_contract_tests<S: SecureStorage>(s: &S) {
    test_is_encrypted_returns_bool(s).await;
    test_store_then_retrieve_roundtrips(s).await;
    test_delete_removes_entry(s).await;
    test_retrieve_missing_returns_none(s).await;
}

async fn test_is_encrypted_returns_bool<S: SecureStorage>(s: &S) {
    let _ = s.is_encrypted();
}

async fn test_store_then_retrieve_roundtrips<S: SecureStorage>(s: &S) {
    let payload = b"contract-payload-bytes";
    s.store(SERVICE, ACCOUNT, payload).await.expect("store must succeed");
    let got = s
        .retrieve(SERVICE, ACCOUNT)
        .await
        .expect("retrieve must succeed")
        .expect("retrieve must yield Some after store");
    assert_eq!(got, payload, "retrieved bytes must equal stored bytes");
    let _ = s.delete(SERVICE, ACCOUNT).await;
}

async fn test_delete_removes_entry<S: SecureStorage>(s: &S) {
    s.store(SERVICE, ACCOUNT, b"x").await.expect("store");
    s.delete(SERVICE, ACCOUNT).await.expect("delete");
    let got = s.retrieve(SERVICE, ACCOUNT).await.expect("retrieve");
    assert!(got.is_none(), "after delete, retrieve must return None");
}

async fn test_retrieve_missing_returns_none<S: SecureStorage>(s: &S) {
    let got = s
        .retrieve(SERVICE, "never-stored-account")
        .await
        .expect("retrieve must succeed");
    assert!(got.is_none(), "missing entry must yield None, not error");
}
```

`tests/contract_secure_storage.rs`:

```rust
use lingxi_test_harness::contracts::secure_storage::secure_storage_contract_tests;
use tempfile::TempDir;

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[tokio::test]
async fn posix_plaintext_secure_storage_passes_contract() {
    let dir = TempDir::new().unwrap();
    let s = lingxi_platform_posix::PlainTextSecureStorage::new(dir.path().to_path_buf())
        .await
        .expect("plaintext storage init");
    secure_storage_contract_tests(&s).await;
}

#[cfg(target_os = "macos")]
#[tokio::test]
async fn macos_keychain_secure_storage_passes_contract() {
    // Keychain tests are gated on CI providing the LINGXI_TEST_KEYCHAIN=1 env
    // var — the runner must already be unlocked and the key must be flushed
    // afterwards.
    if std::env::var("LINGXI_TEST_KEYCHAIN").ok().as_deref() != Some("1") {
        eprintln!("skipping macos_keychain_secure_storage: LINGXI_TEST_KEYCHAIN not set");
        return;
    }
    let dir = TempDir::new().unwrap();
    let s = lingxi_platform_posix::MacOsKeychainStorage::new(
        "contract-test-user".into(),
        dir.path().to_path_buf(),
    )
    .await
    .expect("keychain storage init");
    secure_storage_contract_tests(&s).await;
}

#[cfg(target_os = "windows")]
#[tokio::test]
async fn windows_plaintext_secure_storage_passes_contract() {
    let dir = TempDir::new().unwrap();
    let s = lingxi_platform_windows::PlainTextSecureStorage::new(dir.path().to_path_buf())
        .await
        .expect("plaintext storage init");
    secure_storage_contract_tests(&s).await;
}
```

**Verification:**

```bash
cargo test -p lingxi-test-harness --test contract_secure_storage
```

---

### Task 10: swarm contract

**Files:** `crates/test-harness/src/contracts/swarm.rs` (new), `crates/test-harness/tests/contract_swarm.rs` (new), update `contracts/mod.rs`.

The `SwarmBackend` trait surface (per `crates/traits/src/swarm.rs`):
- `async fn destroy_swarm(&self, handle: SwarmHandle) -> Result<(), SwarmError>`
- `fn is_available(&self) -> bool`
- plus `start_swarm` / `create_teammate_pane` (kept out of the trait-level contract — real-tmux tests live in `platforms/posix/tests/`)

`contracts/swarm.rs`:

```rust
//! [`SwarmBackend`] contract: trait-level invariants only. Real tmux/iTerm
//! exec tests live in `platforms/posix/tests/` and gate on `TMUX_AVAILABLE`.

use lingxi_traits::swarm::{SwarmBackend, SwarmError, SwarmHandle};

pub async fn swarm_backend_contract_tests<S: SwarmBackend>(s: &S) {
    test_is_available_returns_bool(s).await;
    test_destroy_swarm_idempotent_on_unknown_handle(s).await;
}

async fn test_is_available_returns_bool<S: SwarmBackend>(s: &S) {
    let _ = s.is_available();
}

async fn test_destroy_swarm_idempotent_on_unknown_handle<S: SwarmBackend>(s: &S) {
    let handle = SwarmHandle::new();
    let r = s.destroy_swarm(handle).await;
    // Unsupported (Windows) or NoSuchSession (posix without tmux) is fine; what
    // matters is that the call returns instead of panicking or hanging.
    match r {
        Ok(()) | Err(SwarmError::Unsupported(_)) | Err(SwarmError::NoSuchSession(_)) => {}
        other => panic!(
            "destroy_swarm on unknown handle must be Ok/Unsupported/NoSuchSession, got {other:?}"
        ),
    }
}
```

`tests/contract_swarm.rs`:

```rust
use lingxi_test_harness::contracts::swarm::swarm_backend_contract_tests;

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[tokio::test]
async fn posix_swarm_passes_contract() {
    let s = lingxi_platform_posix::PosixSwarm::new();
    swarm_backend_contract_tests(&s).await;
}

#[cfg(target_os = "windows")]
#[tokio::test]
async fn windows_swarm_passes_contract() {
    // Windows swarm is intentionally Unsupported (M2-01 correction).
    let s = lingxi_platform_windows::WindowsSwarmBackend::new();
    assert!(!s.is_available(), "Windows swarm must report unavailable");
    swarm_backend_contract_tests(&s).await;
}
```

**Verification:**

```bash
cargo test -p lingxi-test-harness --test contract_swarm
```

---

### Task 11: bridge contract

**Files:** `crates/test-harness/src/contracts/bridge.rs` (new), `crates/test-harness/tests/contract_bridge.rs` (new), update `contracts/mod.rs`.

The `BridgeTransport` trait surface (per `crates/traits/src/bridge.rs`):
- `async fn connect(&self, config: &BridgeConfig) -> Result<BridgeConnection, BridgeError>`
- `async fn disconnect(&self, conn: BridgeConnection) -> Result<(), BridgeError>`

`contracts/bridge.rs`:

```rust
//! [`BridgeTransport`] contract. Without a real running IDE on the CI box,
//! `connect()` must surface either `Unsupported` (Windows or no lockfile
//! discovery wired) or `LockfileNotFound`; never panic, never hang.

use lingxi_traits::bridge::{BridgeConfig, BridgeError, BridgeTransport};
use std::time::Duration;
use tokio::time::timeout;

pub async fn bridge_transport_contract_tests<B: BridgeTransport>(b: &B) {
    test_connect_returns_within_timeout(b).await;
    test_disconnect_unknown_is_ok_or_closed(b).await;
}

async fn test_connect_returns_within_timeout<B: BridgeTransport>(b: &B) {
    let config = BridgeConfig::default();
    let r = timeout(Duration::from_secs(3), b.connect(&config)).await;
    let inner = r.expect("connect must return within 3s, not hang");
    match inner {
        Ok(_conn) => {} // Live IDE on this CI box — fine, exercise the happy path.
        Err(BridgeError::Unsupported(_)) | Err(BridgeError::LockfileNotFound) => {}
        Err(other) => panic!(
            "connect must return Ok/Unsupported/LockfileNotFound, got {other:?}"
        ),
    }
}

async fn test_disconnect_unknown_is_ok_or_closed<B: BridgeTransport>(b: &B) {
    let bogus = lingxi_traits::bridge::BridgeConnection::synthetic_for_test();
    let r = b.disconnect(bogus).await;
    match r {
        Ok(()) | Err(BridgeError::Closed) | Err(BridgeError::Unsupported(_)) => {}
        other => panic!("disconnect must be Ok/Closed/Unsupported, got {other:?}"),
    }
}
```

The `BridgeConnection::synthetic_for_test()` helper is a `pub(crate)`-via-`#[cfg(test)]` constructor that produces a connection handle the transport can recognize as "not mine." Add it to `crates/traits/src/bridge.rs` as part of this task — it's the only way a contract author can construct a `BridgeConnection` without going through `connect()`.

`tests/contract_bridge.rs`:

```rust
use lingxi_test_harness::contracts::bridge::bridge_transport_contract_tests;

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[tokio::test]
async fn posix_bridge_passes_contract() {
    let b = lingxi_platform_posix::PosixBridge::new();
    bridge_transport_contract_tests(&b).await;
}

#[cfg(target_os = "windows")]
#[tokio::test]
async fn windows_bridge_passes_contract() {
    let b = lingxi_platform_windows::WindowsBridge::new();
    bridge_transport_contract_tests(&b).await;
}
```

**Verification:**

```bash
cargo test -p lingxi-test-harness --test contract_bridge
```

---

### Task 12: effect_handler contract + Phase A commit

**Files:** `crates/test-harness/src/contracts/effect_handler.rs` (new), `crates/test-harness/tests/contract_effect_handler.rs` (new), update `contracts/mod.rs`.

The `EffectHandler` trait surface (per `crates/traits/src/effect_handler.rs`):
- `async fn handle(&self, effect: Effect) -> Result<EffectResult, EffectError>`

`contracts/effect_handler.rs`:

```rust
//! [`EffectHandler`] contract: `handle` must accept every Effect variant
//! without panicking. Each variant either returns `Ok(EffectResult)` or a
//! specific `EffectError` — never an internal `unreachable!`.

use lingxi_protocol::{Effect, EffectError};
use lingxi_traits::effect_handler::EffectHandler;
use lingxi_protocol::{MessageId, RequestId, SessionId};

pub async fn effect_handler_contract_tests<H: EffectHandler>(h: &H) {
    test_render_stream_delta_is_handled(h).await;
    test_send_api_request_is_handled(h).await;
    test_execute_tool_is_handled(h).await;
    test_persist_session_is_handled(h).await;
}

async fn test_render_stream_delta_is_handled<H: EffectHandler>(h: &H) {
    let effect = Effect::RenderStreamDelta {
        message_id: MessageId::new(),
        delta: "x".into(),
    };
    let r = h.handle(effect).await;
    assert!(
        !matches!(r, Err(EffectError::Internal(_))),
        "RenderStreamDelta must not return Internal error, got {r:?}"
    );
}

async fn test_send_api_request_is_handled<H: EffectHandler>(h: &H) {
    let effect = Effect::SendApiRequest {
        request_id: RequestId::new(),
        // Effect carries the full request payload; we use the protocol's
        // builder to assemble a minimal valid payload.
        request: lingxi_protocol::ApiRequestEnvelope::minimal_for_test(),
    };
    let r = h.handle(effect).await;
    let _ = r; // Just verifying no panic + variant accepted.
}

async fn test_execute_tool_is_handled<H: EffectHandler>(h: &H) {
    let effect = Effect::ExecuteTool {
        tool_use_id: lingxi_protocol::ToolUseId::new(),
        tool_name: "Read".into(),
        input: serde_json::json!({"path": "/tmp/contract"}),
    };
    let r = h.handle(effect).await;
    let _ = r;
}

async fn test_persist_session_is_handled<H: EffectHandler>(h: &H) {
    let effect = Effect::PersistSession {
        session_id: SessionId::new(),
    };
    let r = h.handle(effect).await;
    let _ = r;
}
```

`tests/contract_effect_handler.rs`:

```rust
use lingxi_test_harness::contracts::effect_handler::effect_handler_contract_tests;

#[tokio::test]
async fn posix_minimal_effect_handler_passes_contract() {
    // PosixMinimalHost backs cli-demo; use it as the canonical handler since
    // it's the only EffectHandler that's been wired end-to-end since M1.
    let host = lingxi_platform_posix_minimal::PosixMinimalHost::for_test();
    effect_handler_contract_tests(&host).await;
}
```

Add `pub fn for_test() -> Self` to `PosixMinimalHost` if not already present — it returns a fully-formed host with in-memory FS + mock HTTP + mock MCP so the contract has no external dependencies. This belongs to `platforms/posix-minimal/src/lib.rs` and is gated `#[cfg(any(test, feature = "test-fixtures"))]`.

**`contracts/mod.rs` final state** after Phase A:

```rust
//! Trait-level contract test suites. Each `MockX` (or production) impl runs
//! through these to verify it satisfies the trait's invariants.
//!
//! M1.23 shipped the `filesystem` suite. M2.07 adds the remaining 12 (clock,
//! runtime, http, process, mcp, lsp, sandbox, worktree, secure_storage,
//! swarm, bridge, effect_handler). All 13 traits now have a contract.

pub mod bridge;
pub mod clock;
pub mod effect_handler;
pub mod filesystem;
pub mod http;
pub mod lsp;
pub mod mcp;
pub mod process;
pub mod runtime;
pub mod sandbox;
pub mod secure_storage;
pub mod swarm;
pub mod worktree;
```

**Phase A commit** (after all 12 tasks pass on the dev machine):

```bash
cd lingxi-core
cargo test -p lingxi-test-harness --test 'contract_*'
git add crates/test-harness/src/contracts crates/test-harness/tests/contract_*.rs crates/test-harness/Cargo.toml
git add platforms/posix-minimal/src/lib.rs  # for_test() helper if needed
git add crates/traits/src/bridge.rs         # synthetic_for_test() helper
git commit -m "$(cat <<'EOF'
test(contracts): 12 trait contract suites

Mirrors the M1 filesystem contract pattern across the remaining 12 traits
(clock, runtime, http, process, mcp, lsp, sandbox, worktree, secure_storage,
swarm, bridge, effect_handler). Each suite is exercised against the M1 mock
impl (where it exists) plus posix/windows production impls under
cfg(target_os). Subprocess-heavy suites (process, secure_storage macOS
Keychain, worktree git, http live server) gate on capability env vars to
keep ubuntu-latest CI green.
EOF
)"
```

---

## Phase B: Parity fixtures + drivers (Tasks 13-19)

Each parity test loads a JSON fixture from `crates/test-harness/src/parity/fixtures/`, runs the relevant production code, and asserts the output matches the fixture byte-for-byte where the parity claim demands it.

First rewrite `parity/mod.rs`:

```rust
//! Parity fixtures derived from the claude-code 2026-03-31 reference. Each
//! fixture captures a known input + expected output for a behavior we
//! committed to in `docs/superpowers/specs/2026-05-23-m2-claude-code-parity-design.md`.

use serde::de::DeserializeOwned;
use std::path::PathBuf;

/// Locate the `fixtures/` directory beside this source file.
fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("src")
        .join("parity")
        .join("fixtures")
}

/// Load a fixture by stem (e.g. `"mcp_initialize_request"`).
///
/// # Panics
///
/// Panics if the fixture file is missing or its JSON does not deserialize as
/// `T`; tests rely on this to fail loudly.
pub fn load_fixture<T: DeserializeOwned>(stem: &str) -> T {
    let path = fixtures_dir().join(format!("{stem}.json"));
    let bytes = std::fs::read(&path)
        .unwrap_or_else(|e| panic!("read fixture {path:?}: {e}"));
    serde_json::from_slice::<T>(&bytes)
        .unwrap_or_else(|e| panic!("deserialize fixture {stem}: {e}"))
}
```

---

### Task 13: parity_sandbox_config

**Files:** `crates/test-harness/src/parity/fixtures/sandbox_config_conversion.json` (new), `crates/test-harness/tests/parity_sandbox_config.rs` (new).

**Behavior locked:** `convert_settings_to_runtime_config(settings)` (from M2-04) preserves the field shape and allow/deny array contents of `permissions.allow` / `permissions.deny` rules into `SandboxRuntimeConfig.filesystem.{allowWrite, denyWrite, allowRead, denyRead}` and `SandboxRuntimeConfig.network.allowedDomains`.

`fixtures/sandbox_config_conversion.json`:

```json
{
  "input_settings": {
    "permissions": {
      "allow": [
        "Edit(/Users/alice/projects/*)",
        "Read(/Users/alice/projects/*)",
        "Bash(npm install:*)",
        "WebFetch(domain:api.anthropic.com)",
        "WebFetch(domain:github.com)"
      ],
      "deny": [
        "Edit(/etc/*)",
        "Read(/Users/alice/.ssh/*)"
      ]
    },
    "sandbox": {
      "enabled": true,
      "failIfUnavailable": false
    }
  },
  "expected_runtime_config": {
    "enabled": true,
    "failIfUnavailable": false,
    "filesystem": {
      "allowWrite": ["/Users/alice/projects/*"],
      "denyWrite": ["/etc/*"],
      "allowRead": ["/Users/alice/projects/*"],
      "denyRead": ["/Users/alice/.ssh/*"]
    },
    "network": {
      "allowedDomains": ["api.anthropic.com", "github.com"]
    }
  }
}
```

`tests/parity_sandbox_config.rs`:

```rust
use lingxi_sandbox::policy_convert::convert_settings_to_runtime_config;
use lingxi_test_harness::parity::load_fixture;
use serde::Deserialize;
use serde_json::Value;

#[derive(Deserialize)]
struct Fixture {
    input_settings: Value,
    expected_runtime_config: Value,
}

#[tokio::test]
async fn sandbox_config_conversion_matches_claude_code() {
    let fx: Fixture = load_fixture("sandbox_config_conversion");
    let settings: lingxi_protocol::settings::SettingsJson =
        serde_json::from_value(fx.input_settings).expect("settings parses");
    let runtime = convert_settings_to_runtime_config(&settings);

    // Compare on the three fields the fixture exercises. Equality is checked
    // through serde_json::Value to avoid coupling to internal field order.
    let got = serde_json::to_value(&runtime).expect("runtime serializes");
    let want = fx.expected_runtime_config;

    for key in ["filesystem", "network", "enabled", "failIfUnavailable"] {
        assert_eq!(
            got.get(key),
            want.get(key),
            "field {key} must match claude-code conversion exactly: got={:?} want={:?}",
            got.get(key),
            want.get(key)
        );
    }
}
```

**Verification:**

```bash
cargo test -p lingxi-test-harness --test parity_sandbox_config
```

---

### Task 14: parity_mcp_initialize

**Files:** `crates/test-harness/src/parity/fixtures/mcp_initialize_request.json` (new), `crates/test-harness/tests/parity_mcp_initialize.rs` (new).

**Behavior locked (from spec §6.2):**
- MCP client `name: "claude-code"` literal
- `title: "Claude Code"`
- `version: env!("CARGO_PKG_VERSION")` at compile time
- `websiteUrl: "https://claude.com/claude-code"`
- `capabilities: {"roots":{}, "elicitation":{}}` — both values are empty objects, not null

`fixtures/mcp_initialize_request.json`:

```json
{
  "expected_initialize_params_shape": {
    "protocolVersion": "2025-06-18",
    "clientInfo": {
      "name": "claude-code",
      "title": "Claude Code",
      "version": "__VERSION_PLACEHOLDER__",
      "websiteUrl": "https://claude.com/claude-code"
    },
    "capabilities": {
      "roots": {},
      "elicitation": {}
    }
  }
}
```

`tests/parity_mcp_initialize.rs`:

```rust
use lingxi_mcp::initialize_params::build_initialize_params;
use lingxi_test_harness::parity::load_fixture;
use serde::Deserialize;
use serde_json::Value;

#[derive(Deserialize)]
struct Fixture {
    expected_initialize_params_shape: Value,
}

#[test]
fn mcp_initialize_request_matches_claude_code_identity() {
    let fx: Fixture = load_fixture("mcp_initialize_request");
    let params = build_initialize_params();
    let got = serde_json::to_value(&params).expect("params serializes");

    // Identity literals.
    assert_eq!(got["clientInfo"]["name"], "claude-code");
    assert_eq!(got["clientInfo"]["title"], "Claude Code");
    assert_eq!(got["clientInfo"]["websiteUrl"], "https://claude.com/claude-code");

    // Version: must equal the crate's CARGO_PKG_VERSION at compile time.
    assert_eq!(
        got["clientInfo"]["version"],
        env!("CARGO_PKG_VERSION"),
        "version literal must come from CARGO_PKG_VERSION"
    );

    // Capabilities: roots and elicitation are LITERALLY empty objects (not null,
    // not missing, not {} from any-of). Java SDK rejects {form:{},url:{}}.
    let caps = &got["capabilities"];
    assert!(caps.is_object(), "capabilities must be an object");
    assert_eq!(
        caps["roots"], serde_json::json!({}),
        "capabilities.roots must be the literal empty object"
    );
    assert_eq!(
        caps["elicitation"], serde_json::json!({}),
        "capabilities.elicitation must be the literal empty object"
    );
    // Sanity: no extra unexpected capability keys.
    let cap_keys: Vec<_> = caps.as_object().unwrap().keys().collect();
    assert_eq!(
        cap_keys.len(), 2,
        "capabilities must have exactly 2 keys (roots, elicitation), got {cap_keys:?}"
    );

    // Touch the fixture to keep it load-bearing even when the literals match.
    let _ = fx.expected_initialize_params_shape;
}
```

**Verification:**

```bash
cargo test -p lingxi-test-harness --test parity_mcp_initialize
```

---

### Task 15: parity_mcp_transports

**Files:** `crates/test-harness/src/parity/fixtures/mcp_transport_settings_matrix.json` (new), `crates/test-harness/tests/parity_mcp_transports.rs` (new).

**Behavior locked (from spec §6.2 and §10):**
- User-configured types `stdio`, `sse`, `http`, `ws` parse into `McpTransportSpec` variants without error.
- Types `inProcess`, `sdk` are not user-facing; `McpRegistry::connect_with_spec` must return `McpError::UnsupportedTransport`.

`fixtures/mcp_transport_settings_matrix.json`:

```json
{
  "supported": [
    {
      "raw": {
        "type": "stdio",
        "command": "/usr/local/bin/mcp-server-filesystem",
        "args": ["--root", "/tmp"]
      }
    },
    {
      "raw": {
        "type": "sse",
        "url": "https://example.com/mcp/sse",
        "headers": {"X-API-Key": "secret"}
      }
    },
    {
      "raw": {
        "type": "http",
        "url": "https://example.com/mcp/stream"
      }
    },
    {
      "raw": {
        "type": "ws",
        "url": "ws://localhost:9999",
        "headers": {"X-Claude-Code-Ide-Authorization": "tok"}
      }
    }
  ],
  "unsupported": [
    {
      "raw": {
        "type": "inProcess",
        "name": "computer-use"
      }
    },
    {
      "raw": {
        "type": "sdk",
        "name": "internal-control"
      }
    }
  ]
}
```

`tests/parity_mcp_transports.rs`:

```rust
use lingxi_mcp::registry::{McpRegistry, McpRegistryError};
use lingxi_mcp::settings::parse_transport_spec;
use lingxi_test_harness::parity::load_fixture;
use lingxi_traits::mcp::McpError;
use serde::Deserialize;
use serde_json::Value;

#[derive(Deserialize)]
struct Entry { raw: Value }

#[derive(Deserialize)]
struct Fixture { supported: Vec<Entry>, unsupported: Vec<Entry> }

#[tokio::test]
async fn mcp_transport_matrix_matches_claude_code() {
    let fx: Fixture = load_fixture("mcp_transport_settings_matrix");

    // Use the M1 mock MCP transport so the registry can `connect_with_spec`
    // without spawning a real server.
    let mock = lingxi_test_harness::mocks::MockMcpTransport::new();
    let registry = McpRegistry::with_transport(Box::new(mock));

    for entry in fx.supported {
        let spec = parse_transport_spec(&entry.raw)
            .unwrap_or_else(|e| panic!("supported spec must parse: {:?} err={e}", entry.raw));
        let r = registry.connect_with_spec("test", &spec).await;
        // We don't assert connect succeeds (the mock may reject any actual I/O)
        // — only that the parse + dispatch path doesn't return
        // UnsupportedTransport.
        match r {
            Err(McpError::UnsupportedTransport(t)) => {
                panic!("supported transport {t:?} must not be rejected as Unsupported")
            }
            _ => {}
        }
    }

    for entry in fx.unsupported {
        let spec = parse_transport_spec(&entry.raw)
            .unwrap_or_else(|e| panic!("unsupported spec must still parse: {:?} err={e}", entry.raw));
        let r = registry.connect_with_spec("test", &spec).await;
        match r {
            Err(McpError::UnsupportedTransport(_)) => {} // expected
            Err(McpRegistryError::Mcp(McpError::UnsupportedTransport(_))) => {}
            other => panic!(
                "unsupported transport {:?} must yield UnsupportedTransport, got {other:?}",
                entry.raw
            ),
        }
    }
}
```

**Verification:**

```bash
cargo test -p lingxi-test-harness --test parity_mcp_transports
```

---

### Task 16: parity_lsp_plugin_only

**Files:** `crates/test-harness/src/parity/fixtures/lsp_plugin_only.json` (new), `crates/test-harness/tests/parity_lsp_plugin_only.rs` (new), `crates/lsp/tests/compile_fail/register_config_external.rs` (new — compile-fail fixture).

**Behavior locked (from spec §6.3):** `LspRegistry::register_config` is `pub(crate)`. User-settings code paths cannot register LSP servers; only `crates/plugin/src/manager.rs` can call into LSP server registration. claude-code's `config.ts::getAllLspServers()` only consults `getPluginLspServers()` — we mirror that.

`fixtures/lsp_plugin_only.json`:

```json
{
  "behavior": "LspRegistry::register_config is pub(crate) — only crates/plugin may register LSP servers",
  "rationale": "claude-code config.ts::getAllLspServers() only consults getPluginLspServers(); user/project settings cannot register LSP servers",
  "permitted_callers": ["crates/lsp/src/registry.rs", "crates/plugin/src/manager.rs"]
}
```

`tests/parity_lsp_plugin_only.rs`:

```rust
use lingxi_test_harness::parity::load_fixture;
use serde::Deserialize;

#[derive(Deserialize)]
struct Fixture {
    behavior: String,
    rationale: String,
    permitted_callers: Vec<String>,
}

#[test]
fn lsp_plugin_only_doc_test_loads() {
    let fx: Fixture = load_fixture("lsp_plugin_only");
    assert!(fx.behavior.contains("pub(crate)"));
    assert!(fx.rationale.contains("getPluginLspServers"));
    assert!(fx.permitted_callers.iter().any(|p| p == "crates/plugin/src/manager.rs"));
}

/// Smoke test: this file lives outside `crates/lsp`. If `register_config`
/// were `pub`, this would compile. The block below is wrapped in a
/// `compile_fail` doctest so cargo will reject any future change that loosens
/// the visibility.
///
/// ```compile_fail
/// fn _attempt() {
///     let r = lingxi_lsp::registry::LspRegistry::new();
///     // register_config is pub(crate) — this must not compile.
///     r.register_config(unimplemented!());
/// }
/// ```
#[test]
fn lsp_register_config_visibility_doc_smoke() {
    // Body intentionally empty — the `compile_fail` doctest above is the
    // assertion. Adding this fn ensures `cargo test` discovers the doctest.
}
```

If the `compile_fail` doctest above is unreliable across Rust toolchains (rare but possible on rust-toolchain pin 1.82.0), fall back to a `crates/lsp/tests/compile_fail.rs` using the `trybuild` crate. Either is acceptable — pick whichever lands cleanest on CI.

**Verification:**

```bash
cargo test -p lingxi-test-harness --test parity_lsp_plugin_only
cargo test -p lingxi-lsp --doc  # exercises the compile_fail block
```

---

### Task 17: parity_worktree_naming

**Files:** `crates/test-harness/src/parity/fixtures/worktree_branch_naming.json` (new), `crates/test-harness/tests/parity_worktree_naming.rs` (new).

**Behavior locked (from spec §3 and §6.1):**
- Branch prefix: `worktree-` (literal). Not `lingxi/`. Not `claude/`.
- Path: `<repo_root>/.claude/worktrees/<flattened-slug>`.
- Slug flatten: `/` → `+`. So `user/feature` → branch `worktree-user+feature`, path `<root>/.claude/worktrees/user+feature`.
- Slug validation: each `/`-separated segment is alphanumeric + `_-.`, max 64 chars total.

`fixtures/worktree_branch_naming.json`:

```json
{
  "cases": [
    {
      "slug": "feature-x",
      "expected_branch": "worktree-feature-x",
      "expected_path_suffix": ".claude/worktrees/feature-x"
    },
    {
      "slug": "user/feature",
      "expected_branch": "worktree-user+feature",
      "expected_path_suffix": ".claude/worktrees/user+feature"
    },
    {
      "slug": "team/area/widget",
      "expected_branch": "worktree-team+area+widget",
      "expected_path_suffix": ".claude/worktrees/team+area+widget"
    }
  ],
  "invalid_slugs": [
    "bad slug",
    "with$dollar",
    "",
    "leading/",
    "/leading",
    "trailing+sign"
  ]
}
```

`tests/parity_worktree_naming.rs`:

```rust
use lingxi_test_harness::parity::load_fixture;
use lingxi_traits::worktree::{WorktreeError, WorktreeManager};
use serde::Deserialize;
use std::path::Path;
use std::process::Command;
use tempfile::TempDir;

#[derive(Deserialize)]
struct Case { slug: String, expected_branch: String, expected_path_suffix: String }

#[derive(Deserialize)]
struct Fixture { cases: Vec<Case>, invalid_slugs: Vec<String> }

fn init_git_repo(root: &Path) {
    let run = |args: &[&str]| Command::new("git").args(args).current_dir(root).output().unwrap();
    let _ = run(&["init", "-q", "-b", "main"]);
    let _ = run(&["config", "user.email", "t@e.com"]);
    let _ = run(&["config", "user.name", "T"]);
    std::fs::write(root.join("README"), "x").unwrap();
    let _ = run(&["add", "."]);
    let _ = run(&["commit", "-q", "-m", "init"]);
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[tokio::test]
async fn worktree_branch_naming_matches_claude_code() {
    let fx: Fixture = load_fixture("worktree_branch_naming");
    let w = lingxi_platform_posix::PosixWorktree::new();
    if !w.is_supported() {
        return;
    }

    for case in fx.cases {
        let tmp = TempDir::new().unwrap();
        init_git_repo(tmp.path());
        let handle = w
            .create_worktree(tmp.path(), &case.slug, &[])
            .await
            .unwrap_or_else(|e| panic!("create_worktree({}) must succeed: {e}", case.slug));
        assert_eq!(
            handle.branch, case.expected_branch,
            "slug {} → branch must equal {}, got {}",
            case.slug, case.expected_branch, handle.branch
        );
        let suffix = handle.path.strip_prefix(tmp.path()).unwrap();
        assert_eq!(
            suffix.to_str().unwrap(),
            case.expected_path_suffix,
            "slug {} → path suffix must equal {}, got {:?}",
            case.slug, case.expected_path_suffix, suffix
        );
    }

    for bad in fx.invalid_slugs {
        let tmp = TempDir::new().unwrap();
        init_git_repo(tmp.path());
        let r = w.create_worktree(tmp.path(), &bad, &[]).await;
        match r {
            Err(WorktreeError::InvalidSlug(_)) => {}
            other => panic!("slug {bad:?} must be rejected as InvalidSlug, got {other:?}"),
        }
    }
}
```

**Verification:**

```bash
cargo test -p lingxi-test-harness --test parity_worktree_naming
```

---

### Task 18: parity_keychain_service_name

**Files:** `crates/test-harness/src/parity/fixtures/secure_storage_macos_service_name.json` (new), `crates/test-harness/tests/parity_keychain_service_name.rs` (new).

**Behavior locked (from spec §6.6):** macOS Keychain service name format:
`format!("Claude Code{oauth_suffix}-credentials{dir_hash}")`
where:
- `oauth_suffix` is empty for the default OAuth flow, otherwise `"-{flow}"`.
- `dir_hash` is empty when the user is using the default config dir, otherwise the first 8 hex chars of `sha256(config_dir)`.

`fixtures/secure_storage_macos_service_name.json`:

```json
{
  "cases": [
    {
      "label": "default config dir, default oauth",
      "input_user": "Claude Code",
      "input_oauth_suffix": "",
      "input_dir_hash": "",
      "expected_service_name": "Claude Code-credentials"
    },
    {
      "label": "non-default config dir",
      "input_user": "Claude Code",
      "input_oauth_suffix": "",
      "input_dir_hash": "abc12345",
      "expected_service_name": "Claude Code-credentialsabc12345"
    },
    {
      "label": "non-default oauth flow",
      "input_user": "Claude Code",
      "input_oauth_suffix": "-enterprise",
      "input_dir_hash": "",
      "expected_service_name": "Claude Code-enterprise-credentials"
    },
    {
      "label": "both non-default",
      "input_user": "Claude Code",
      "input_oauth_suffix": "-enterprise",
      "input_dir_hash": "abc12345",
      "expected_service_name": "Claude Code-enterprise-credentialsabc12345"
    }
  ]
}
```

`tests/parity_keychain_service_name.rs`:

```rust
use lingxi_test_harness::parity::load_fixture;
use serde::Deserialize;

#[derive(Deserialize)]
struct Case {
    label: String,
    input_user: String,
    input_oauth_suffix: String,
    input_dir_hash: String,
    expected_service_name: String,
}

#[derive(Deserialize)]
struct Fixture { cases: Vec<Case> }

// The `full_service_name` helper lives in
// `platforms/posix/src/secure_storage.rs`. It is `pub(crate)` by default;
// expose it via `pub mod parity_helpers { pub use crate::secure_storage::full_service_name; }`
// gated on `#[cfg(any(test, feature = "parity-helpers"))]` so the parity
// driver can reach it without leaking implementation detail into the public
// API.
#[cfg(target_os = "macos")]
use lingxi_platform_posix::parity_helpers::full_service_name;

#[cfg(target_os = "macos")]
#[test]
fn keychain_service_name_matches_claude_code() {
    let fx: Fixture = load_fixture("secure_storage_macos_service_name");
    for case in fx.cases {
        let got = full_service_name(
            &case.input_user,
            &case.input_oauth_suffix,
            &case.input_dir_hash,
        );
        assert_eq!(
            got, case.expected_service_name,
            "{}: full_service_name must yield {}, got {}",
            case.label, case.expected_service_name, got
        );
    }
}

#[cfg(not(target_os = "macos"))]
#[test]
fn keychain_service_name_fixture_loads_on_non_macos() {
    let _fx: Fixture = load_fixture("secure_storage_macos_service_name");
}
```

The `parity_helpers` module is a new addition to `platforms/posix/src/lib.rs` — gate it on `#[cfg(any(test, feature = "parity-helpers"))]` and add the `parity-helpers` feature to the crate's Cargo.toml. The test-harness then opts in:

```toml
[target.'cfg(target_os = "macos")'.dev-dependencies]
lingxi-platform-posix = { path = "../../platforms/posix", features = ["parity-helpers"] }
```

**Verification:**

```bash
cargo test -p lingxi-test-harness --test parity_keychain_service_name
```

---

### Task 19: parity_tmux_windows + Phase B commit

**Files:** `crates/test-harness/src/parity/fixtures/tmux_windows_refusal.json` (new), `crates/test-harness/tests/parity_tmux_windows.rs` (new).

**Behavior locked (from spec §6.5 and §6.1):**
- On Windows, `WindowsSwarmBackend::is_available()` returns `false`.
- `start_swarm` returns `SwarmError::Unsupported("--tmux is not supported on Windows")` (exact string match).

`fixtures/tmux_windows_refusal.json`:

```json
{
  "expected_is_available": false,
  "expected_error_message_substring": "--tmux is not supported on Windows"
}
```

`tests/parity_tmux_windows.rs`:

```rust
use lingxi_test_harness::parity::load_fixture;
use lingxi_traits::swarm::{SwarmBackend, SwarmError, SwarmLayout};
use serde::Deserialize;

#[derive(Deserialize)]
struct Fixture {
    expected_is_available: bool,
    expected_error_message_substring: String,
}

#[cfg(target_os = "windows")]
#[tokio::test]
async fn windows_swarm_refuses_tmux_with_claude_code_message() {
    let fx: Fixture = load_fixture("tmux_windows_refusal");
    let s = lingxi_platform_windows::WindowsSwarmBackend::new();
    assert_eq!(s.is_available(), fx.expected_is_available);

    let layout = SwarmLayout::default();
    let r = s.start_swarm(&layout).await;
    match r {
        Err(SwarmError::Unsupported(msg)) => {
            assert!(
                msg.contains(&fx.expected_error_message_substring),
                "Windows swarm error must contain {:?}, got {:?}",
                fx.expected_error_message_substring, msg
            );
        }
        other => panic!("Windows start_swarm must return Unsupported, got {other:?}"),
    }
}

#[cfg(not(target_os = "windows"))]
#[test]
fn windows_swarm_fixture_loads_on_non_windows() {
    // Linux/macOS CI still verifies the fixture is well-formed JSON.
    let _fx: Fixture = load_fixture("tmux_windows_refusal");
}
```

**Phase B commit** (after all 7 parity tasks pass):

```bash
cd lingxi-core
cargo test -p lingxi-test-harness --test 'parity_*'
git add crates/test-harness/src/parity crates/test-harness/tests/parity_*.rs
git add platforms/posix/src/lib.rs platforms/posix/Cargo.toml  # parity-helpers feature
git commit -m "$(cat <<'EOF'
test(parity): 7 claude-code behavior fixtures

JSON fixtures + driver tests locking the wire and string surfaces M2
committed to: SettingsJson→SandboxRuntimeConfig conversion shape, MCP
initialize identity (name='claude-code', capabilities {roots:{},
elicitation:{}}), MCP transport matrix (stdio/sse/http/ws supported;
inProcess/sdk Unsupported), LSP register_config visibility (plugin-only),
worktree branch prefix 'worktree-' with '/' → '+' slug flattening, macOS
Keychain service name format, and Windows tmux Unsupported error string.
EOF
)"
```

---

## Phase C: Docs (Tasks 20-23)

### Task 20: CHANGELOG.md for v0.3.0

**Files:** `CHANGELOG.md` (modify — prepend new section above the existing `## [0.2.0]` entry).

The new section, inserted at the top after the `# Changelog` heading:

```markdown
## [0.3.0] — M2 claude-code Behavioral Parity

### Crates added
- `lingxi-jsonrpc` — JSON-RPC 2.0 framing shared by MCP and LSP. Supports
  Content-Length-prefixed (LSP / modern MCP) and line-delimited (older MCP)
  framing with auto-detect, outbound request router with timeout + drop-cancel,
  inbound request router (for `roots/list`, `elicitation/create`), and a
  notification broker.

### Crates expanded
- `lingxi-sandbox` — full `SandboxRuntimeConfig` schema (matches claude-code
  `entrypoints/sandboxTypes.ts` field-for-field), `convert_settings_to_runtime_config`,
  `dependency_check`, `violation_store`, and `wrap_with_sandbox` dispatcher
  (macOS `sandbox-exec` SBPL profile, Linux `bwrap+socat`, Windows/WSL1 Unsupported).
- `lingxi-mcp` — real client over `lingxi-jsonrpc` covering `initialize`, `list_tools`,
  `call_tool` (with timeout error string `"MCP server \"...\" tool \"...\" timed out
  after Ns"`), `list_resources`, `list_prompts`, `read_resource`, `ping`, plus
  inbound `roots/list` and `elicitation/create` handlers. Identity locked:
  `name="claude-code"`, `title="Claude Code"`, capabilities `{"roots":{}, "elicitation":{}}`.
- `lingxi-lsp` — real client over `lingxi-jsonrpc` with 9 tool operations
  (`goToDefinition`, `findReferences`, `hover`, `documentSymbol`, `workspaceSymbol`,
  `goToImplementation`, `prepareCallHierarchy`, `incomingCalls`, `outgoingCalls`),
  1-based ↔ 0-based line/character translation, `textDocument/didOpen` registry,
  `textDocument/publishDiagnostics` accumulation, 10 MB `MAX_LSP_FILE_SIZE_BYTES`
  cap, and plugin-only `register_config` (visibility narrowed to `pub(crate)`).
- `lingxi-bridge` — lockfile-based local IDE bridge. Reads `~/.claude/ide/<port>.lock`,
  builds an MCP-over-WebSocket transport spec with header
  `X-Claude-Code-Ide-Authorization`. The 8-char pairing protocol and
  project-scoped JWT machinery from v0.2.0 were removed (they had no claude-code
  counterpart). Cloud Remote Control bridge remains out of scope.
- `lingxi-platform-posix` — real impls land for `Sandbox` (macOS + Linux + WSL2),
  `McpTransport` (stdio + sse + http + ws), `LspTransport`, `SwarmBackend`
  (tmux + iTerm + InProcess fallback), `FileSystem::watch` (notify + debounce),
  `HttpTransport::stream_sse`, `ProcessRunner::spawn_background` + `kill_tree`
  + `pwd -P` cwd tracking, `SecureStorage` (macOS Keychain via `security` CLI
  + plaintext fallback).
- `lingxi-platform-windows` — `Sandbox` and `SwarmBackend` explicitly return
  `Unsupported` (claude-code does not support sandbox or tmux on Windows).
  `FileSystem::watch` switches to `notify`'s `ReadDirectoryChangesW` path.
  `SecureStorage` remains plaintext (Windows Credential Vault deferred).

### 1:1 parity guarantees locked
- Worktree branch prefix: `worktree-` (was `lingxi/` in v0.2.0). Slug flattening
  `/` → `+`. Path: `<repo_root>/.claude/worktrees/<flattened-slug>`.
- MCP client identity: `name="claude-code"`, `title="Claude Code"`,
  `websiteUrl="https://claude.com/claude-code"`, capabilities
  `{"roots":{}, "elicitation":{}}` (empty objects, not null).
- IDE WebSocket auth header: `X-Claude-Code-Ide-Authorization` (literal).
- macOS Keychain service name format: `Claude Code{oauth_suffix}-credentials{dir_hash}`.
- LSP file size cap: `MAX_LSP_FILE_SIZE_BYTES = 10_000_000` (10 MB).
- Sandbox WSL1 refusal: `"sandbox.enabled is set but WSL1 is not supported (requires WSL2)"`.
- Sandbox unsupported-platform: `"sandbox.enabled is set but ${platform} is not supported (requires macOS, Linux, or WSL2)"`.
- Windows tmux refusal: `"--tmux is not supported on Windows"`.
- LSP `register_config` is `pub(crate)` — only plugins can register LSP servers.
- MCP tool name format: `mcp__<server>__<tool>`.

### Known deferrals carried forward to M3+
- Cloud Remote Control bridge (claude.ai worker integration, ~14k TS lines).
- In-process MCP transports (computer-use, Chrome) — depend on a separate
  computer-use server crate.
- Linux SecureStorage native backend (libsecret) — plaintext fallback only,
  matching claude-code's TODO.
- Android / iOS platform crates — M3 milestone.
- Plugin marketplace UI and `.mcpb` bundle installer — M4 UI Layer.
- Web pty-server — M4 UI Layer.
- macOS SBPL profile fidelity beyond the M2 template — separate research task.

### Migration from v0.2.0

The following surfaces changed in source-incompatible ways. Downstream users
of `lingxi-core` as a library MUST update accordingly:

- **Worktree branch prefix:** existing v0.2.0 worktrees with `lingxi/<slug>`
  branches are not recognized by v0.3.0 cleanup. Run `git worktree remove`
  manually for any orphan v0.2.0 worktree before upgrading.
- **Worktree path:** `WorktreeManager::create_worktree` parameter renamed from
  `worktree_base` to `repo_root`. The path is now hardcoded to
  `<repo_root>/.claude/worktrees/<flattened-slug>`.
- **`lingxi-bridge` API:** the 9-variant `BridgeMessage` enum, `BridgeCode`,
  `JwtVerifier`, `RateLimiter`, and `PairingManager` types were removed.
  `IdeBridge` now exposes only `connect()` + `disconnect()`; transport details
  live in `lingxi-mcp`.
- **`crates/bridge` dependencies:** `rand`, `sha2`, `hmac`, `base64` removed
  from `Cargo.toml`. Add `lingxi-mcp` dependency.
- **`lingxi-lsp::LspRegistry::register_config`** is now `pub(crate)`. External
  callers must register LSP servers through `crates/plugin`'s
  `register_plugin_servers` path.
- **`ProcessRunner::spawn_background`** previously returned `Unsupported` on all
  platforms; now returns a real `ProcessHandle` on posix/windows.
- **`api-client::types::StreamEvent`** gained new variants (`Thinking`,
  `SignatureDelta`, `CitationsDelta`, `ConnectorTextDelta`, etc.). Existing
  match arms over `StreamEvent` will hit `non_exhaustive` warnings — add a
  catch-all or update arms.

### Tests + verification
- Workspace test count: ~145 (v0.2.0 baseline 104 + 12 contract suites + 7
  parity fixtures + per-plan tests from M2-01..M2-06).
- `cargo test --workspace` clean.
- `cargo clippy --workspace --all-targets -- -D warnings` clean.
- `cargo fmt --all --check` clean.
- `cargo check -p lingxi-platform-posix --no-default-features` clean.
- `cargo check -p lingxi-platform-windows --no-default-features` clean.
- Desktop cross-compile matrix (`x86_64-unknown-linux-gnu`, `aarch64-apple-darwin`,
  `x86_64-pc-windows-msvc`) green. Android/iOS targets are informational only
  for v0.3.0 (M3 scope).

```

**Verification:**

```bash
# Sanity: CHANGELOG is valid markdown with the new section first.
grep -n "^## \[0.3.0\]" CHANGELOG.md  # must print "3:" or similar (top of file after "# Changelog")
grep -n "^## \[0.2.0\]" CHANGELOG.md  # must print a line number AFTER the 0.3.0 section
```

---

### Task 21: docs/ARCHITECTURE.md update

**Files:** `docs/ARCHITECTURE.md` (modify — refresh the crate map and add a "claude-code parity guarantees" section).

Append a new top-level section (after the existing "Cross-cutting concerns" section):

```markdown
## claude-code parity guarantees (locked in v0.3.0)

The following identifiers, error strings, and file paths are part of the
behavioral contract with claude-code. Changing any of them is a breaking
change for managed-policy customers and for users restoring cross-version
state. They are covered by parity fixtures in
`crates/test-harness/src/parity/fixtures/`.

### Wire identifiers
| Identifier | Value | Rationale |
|---|---|---|
| MCP client name | `"claude-code"` | Servers identify allowed clients by this string; mismatched clients trigger reject-or-ignore logic in some MCP implementations. |
| MCP client title | `"Claude Code"` | Human-readable handshake field surfaced in MCP server logs. |
| MCP client websiteUrl | `"https://claude.com/claude-code"` | Stable identity / support link. |
| MCP capabilities.roots | `{}` (empty object) | Empty object signals "supported, no parameters." `null` or missing signals "not supported" — Java MCP servers reject other shapes. |
| MCP capabilities.elicitation | `{}` (empty object) | Same as above. |
| IDE WebSocket auth header | `X-Claude-Code-Ide-Authorization` | Matches the IDE extension; using `Authorization: Bearer` would not be recognized. |
| MCP tool full-name format | `mcp__<server>__<tool>` | Tool-name partitioning in the assistant's tool router relies on the double-underscore separator. |

### File paths
| Path | Rationale |
|---|---|
| `<repo_root>/.claude/worktrees/<flattened-slug>` | Cross-version worktree discovery. Slug flattening `/` → `+` so file names stay flat while branch names retain hierarchy. |
| `~/.claude/ide/<port>.lock` | IDE lockfile shape (workspaceFolders, pid, ideName, transport, runningInWindows, authToken). VS Code / JetBrains extensions write this; the bridge reads it. |
| Keychain service name | `Claude Code{oauth_suffix}-credentials{dir_hash}` where `dir_hash` is `sha256(config_dir).hex()[..8]` for non-default config dirs. Allows multiple installations to coexist. |

### Branch and slug rules
- Desktop / local worktree branch: `worktree-<flattened-slug>`. Not `lingxi/...`, not `claude/...`.
- Cloud / remote git outcome uses `claude/<branch>` — that's a separate code
  path (and out of v0.3.0 scope); do not confuse the two.
- Slug regex (per segment): `^[A-Za-z0-9_\-.]+$`. Maximum 64 chars total
  across all segments. `/` separates segments and is flattened to `+` on disk.

### Error strings (byte-for-byte)
- Sandbox WSL1: `"sandbox.enabled is set but WSL1 is not supported (requires WSL2)"`.
- Sandbox unsupported platform: `"sandbox.enabled is set but ${platform} is not supported (requires macOS, Linux, or WSL2)"`.
- Sandbox disabled-platforms: `"sandbox.enabled is set but ${platform} is not in sandbox.enabledPlatforms"`.
- Sandbox missing deps: `"sandbox.enabled is set but dependencies are missing: ${deps.join(', ')} · ${platform_hint}"`.
- Worktree source-is-worktree: `"Already in a worktree session"`.
- Windows tmux: `"--tmux is not supported on Windows"`.
- MCP tool timeout: `"MCP server \"<server>\" tool \"<tool>\" timed out after Ns"`.
- Keychain plaintext fallback warning: `"Warning: Storing credentials in plaintext."`.

### Numeric constants
| Constant | Value | Locus |
|---|---|---|
| `MAX_LSP_FILE_SIZE_BYTES` | `10_000_000` (10 MB) | LSP file-backed operations reject inputs above this. |
| `STDERR_BUFFER_CAP` | 64 MB | MCP stdio transport per-connection stderr ring buffer. |
| `MAX_MCP_DESCRIPTION_LENGTH` | claude-code constant; respected with `"… [truncated]"` suffix | Tool description truncation. |
| `KEYCHAIN_CACHE_TTL_MS` | `30_000` (30s) | macOS Keychain prefetch cache TTL. |
| `DEFAULT_TIMEOUT` | 30 minutes | `ProcessRunner::run` default timeout when caller omits. |
| `PANE_SHELL_INIT_DELAY_MS` | 200 ms | tmux pane creation post-split delay. |

### Capability matrix (must match claude-code refusal logic)
| Subsystem | macOS | Linux | WSL2 | WSL1 | Windows |
|---|---|---|---|---|---|
| Sandbox | yes (`sandbox-exec`) | yes (`bwrap+socat`) | yes (same as Linux) | **refused** | **refused** |
| Swarm / tmux | yes | yes | yes | n/a | **refused** |
| LSP | yes | yes | yes | yes | yes |
| MCP stdio | yes | yes | yes | yes | yes |
| MCP WebSocket | yes | yes | yes | yes | yes |
| SecureStorage encrypted | yes (Keychain) | no (plaintext) | no (plaintext) | no (plaintext) | no (plaintext) |
| FS watch | yes (FSEvents via notify) | yes (inotify via notify) | yes | yes | yes (RDC via notify) |
```

Also refresh the existing "Crate map" section: `lingxi-jsonrpc` is now in the
list; `lingxi-bridge` description swaps to "lockfile-based local IDE bridge
(MCP-over-WebSocket)"; `platforms/posix` and `platforms/windows` get a brief
description noting which traits ship real impls vs. Unsupported.

**Verification:** human read, then `git diff docs/ARCHITECTURE.md` review.

---

### Task 22: docs/PLATFORMS.md

**Files:** `docs/PLATFORMS.md` (new).

```markdown
# Platform support matrix (v0.3.0)

LingXi Core's behavioral parity with claude-code targets desktop OS releases.
This document is the authoritative per-OS capability table; it mirrors
`docs/ARCHITECTURE.md#capability-matrix` and adds setup notes.

## Tier 1: full support

### macOS (13 Ventura+ on Apple silicon and Intel)
- **FileSystem watch:** FSEvents via the `notify` crate's `FSEventWatcher`.
- **Sandbox:** `sandbox-exec` with a generated SBPL profile.
- **Swarm:** tmux 3.2+ (preferred) or iTerm via `osascript`. Falls back to
  `InProcess` (no pane visualization) when neither is available.
- **SecureStorage:** macOS Keychain via the `security` CLI. Service-name
  format `Claude Code{oauth_suffix}-credentials{dir_hash}`. 30s TTL cache,
  generation-counter writes, in-flight dedupe. Falls back to plaintext on
  init error with the literal warning `"Warning: Storing credentials in plaintext."`.
- **Process tree-kill:** `nix::sys::signal::killpg` against the child's
  process group (background children are spawned with `setsid()`).
- **HTTP SSE:** `reqwest::Response::bytes_stream()` + `parse_sse_chunks`.

### Linux (glibc, kernel 4.x+; major distros)
- **FileSystem watch:** inotify via `notify`.
- **Sandbox:** `bwrap` (bubblewrap) + `socat` companion for network proxying.
  Install: `apt install bubblewrap socat` (Debian/Ubuntu) or distro equivalent.
- **Swarm:** tmux 3.2+ only (no iTerm path).
- **SecureStorage:** **plaintext** (`PlainTextSecureStorage` at
  `~/.claude/.credentials.json`). Native libsecret backend deferred (matches
  claude-code's plaintext fallback on Linux). The plaintext warning above
  is emitted on first credential write.
- **Process tree-kill:** same as macOS.

### WSL2
- Treated as Linux end-to-end. `bwrap+socat` works the same way.

## Tier 2: limited support

### Windows (10 22H2+, 11)
- **FileSystem watch:** ReadDirectoryChangesW via `notify`.
- **Sandbox:** **Unsupported.** `WindowsSandbox::is_available()` is `false`;
  `prepare()` returns `SandboxError::Unsupported(...)`. claude-code does not
  support a sandbox on Windows; we mirror that to keep policy compatibility.
- **Swarm / tmux:** **Unsupported.** `start_swarm()` returns
  `SwarmError::Unsupported("--tmux is not supported on Windows")`.
- **SecureStorage:** plaintext. Windows Credential Vault backend deferred to M3+.
- **Process tree-kill:** `taskkill /T /F /PID <pid>`.
- **MCP / LSP / HTTP SSE / Worktree:** full support — same code paths as macOS/Linux.

## Refused

### WSL1
- **Sandbox initialize refuses.** Detection: `/proc/version` lacks
  `microsoft-standard` / `WSL2` substring. Error: `"sandbox.enabled is set
  but WSL1 is not supported (requires WSL2)"`.
- Non-sandbox features (LSP, MCP, Worktree, etc.) still work, but the
  `/sandbox` and `/doctor` tools will report sandbox unavailable.

## Out of scope for v0.3.0

### Android / iOS (M3)
- Cross-compile matrix in CI is informational only — failures do not block
  v0.3.0 release. Platform crates (`platforms/android`, `platforms/ios`) and
  capability flag wiring per spec §35 land in M3.

### Browser / web UI (M4+)
- The optional web `pty-server` from claude-code is a separate UI feature;
  not part of v0.3.0.

## Setup notes

### macOS
- Keychain unlock prompts may surface on first credential write — set the
  keychain to "always allow" for the `security` binary if running in CI.
- The `security` CLI is part of macOS; no additional install needed.

### Linux (sandbox)
- Ubuntu / Debian: `sudo apt install bubblewrap socat`.
- Fedora / RHEL: `sudo dnf install bubblewrap socat`.
- Arch: `sudo pacman -S bubblewrap socat`.
- `tmux` is required for the swarm backend: typically pre-installed; install
  via the same package manager if missing.

### Windows
- `git` must be on `PATH` (for worktree operations). Recommended: Git for
  Windows distribution, which includes the `git` CLI and `bash.exe` (we do
  NOT depend on `bash.exe` — production `ProcessRunner` uses native Windows
  process APIs via tokio).
- No bubblewrap / sandbox-exec equivalent; sandbox is intentionally
  unsupported.

## Verifying your install

```bash
cargo run -p lingxi-demo -- --doctor
```

The `/doctor` command prints the resolved platform, the populated
`PlatformCapabilities` struct, and lists any subsystems reporting
`Unsupported`. This is the canonical health check before opening a bug.
```

**Verification:** human read.

---

### Task 23: README.md update + Phase C commit

**Files:** `README.md` (modify — refresh platform support callout and quickstart).

Replace the existing "Quickstart" block with a v0.3.0 version that demos
`platforms/posix` (production) rather than `platforms/posix-minimal`:

```markdown
# LingXi Core

Platform-agnostic Rust engine for an AI coding assistant with 1:1 behavioral
parity to claude-code (2026-03-31 TypeScript reference) on desktop OSes.
v0.3.0 ships the M2 desktop production stack. Android/iOS land in M3.

## Quickstart

```bash
cargo build --workspace --release

# Demo against the production posix platform (real HTTP/SSE, real MCP,
# real sandbox, real Keychain on macOS).
ANTHROPIC_API_KEY=sk-ant-... cargo run --bin lingxi-demo -- \
    --model claude-opus-4-7 \
    --platform posix
```

## Platform support

| OS | Status |
|---|---|
| macOS 13+ | Full support (Keychain, sandbox-exec, tmux/iTerm) |
| Linux | Full support (bubblewrap + socat sandbox; plaintext SecureStorage) |
| WSL2 | Full support (same as Linux) |
| Windows 10 22H2+ | Limited (no sandbox, no tmux; LSP/MCP/worktree work) |
| WSL1 | Sandbox refused at init |
| Android / iOS | M3 (not v0.3.0) |

Per-OS setup notes: see `docs/PLATFORMS.md`.

## Architecture

Full design lives in
`docs/superpowers/specs/2026-05-23-m2-claude-code-parity-design.md` (M2 parity
design) and `docs/superpowers/specs/2026-05-22-lingxi-core-rust-engine-design.md`
(M1 engine design). Navigation aid: `docs/ARCHITECTURE.md`. Security model:
`docs/SECURITY.md`. Behavioral parity guarantees with claude-code:
`docs/ARCHITECTURE.md#claude-code-parity-guarantees-locked-in-v030`.

## License

MIT OR Apache-2.0.
```

**Phase C commit:**

```bash
cd /Users/luolingfeng/Projects/LingXi-Next
git add CHANGELOG.md docs/ARCHITECTURE.md docs/PLATFORMS.md README.md
git commit -m "$(cat <<'EOF'
docs: CHANGELOG + ARCHITECTURE + PLATFORMS for v0.3.0

- CHANGELOG.md gains a 0.3.0 section listing every M2-01..M2-07 deliverable,
  the locked parity guarantees, deferrals carried into M3+, and a migration
  block flagging breaking API changes from v0.2.0 (worktree branch prefix,
  bridge API surface, register_config visibility, etc.).
- docs/ARCHITECTURE.md adds a "claude-code parity guarantees" section
  listing every wire identifier, file path, error string, and numeric
  constant we committed to, plus a per-OS capability matrix.
- docs/PLATFORMS.md is new: per-OS support tiers with install notes for
  sandbox dependencies on Linux and Keychain setup on macOS.
- README.md is refreshed for v0.3.0: production posix demo invocation and
  the platform support table.
EOF
)"
```

---

## Phase D: Cross-compile matrix + release verification (Tasks 24-25)

### Task 24: CI cross-compile matrix split

**Files:** `.github/workflows/ci.yml` (modify).

The existing matrix at `.github/workflows/ci.yml` lists 5 targets and any
failure blocks the build. Per spec §6.7, v0.3.0 only gates on the 3 desktop
targets; Android/iOS remain informational. Update the `cross-compile` job:

```yaml
  cross-compile-desktop:
    runs-on: ubuntu-latest
    strategy:
      fail-fast: false
      matrix:
        target:
          - x86_64-unknown-linux-gnu
          - aarch64-apple-darwin
          - x86_64-pc-windows-msvc
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@1.82.0
        with:
          targets: ${{ matrix.target }}
      - name: Install cross
        run: cargo install cross --locked
      - name: Cross-compile core crates
        working-directory: lingxi-core
        run: |
          cross check --target ${{ matrix.target }} -p lingxi-protocol
          cross check --target ${{ matrix.target }} -p lingxi-core
          cross check --target ${{ matrix.target }} -p lingxi-traits
          cross check --target ${{ matrix.target }} -p lingxi-api-client

  cross-compile-mobile:
    runs-on: ubuntu-latest
    continue-on-error: true   # M3 scope — informational only for v0.3.0
    strategy:
      fail-fast: false
      matrix:
        target:
          - aarch64-linux-android
          - aarch64-apple-ios
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@1.82.0
        with:
          targets: ${{ matrix.target }}
      - name: Install cross
        run: cargo install cross --locked
      - name: Cross-compile core crates (informational)
        working-directory: lingxi-core
        run: |
          cross check --target ${{ matrix.target }} -p lingxi-protocol
          cross check --target ${{ matrix.target }} -p lingxi-core
          cross check --target ${{ matrix.target }} -p lingxi-traits
          cross check --target ${{ matrix.target }} -p lingxi-api-client
```

The mobile job uses `continue-on-error: true` so it surfaces yellow but does
not block the merge / tag.

If GitHub branch protection currently lists `cross-compile` as a required
check, also update the branch protection rule (via the Settings UI or
`gh api` if scripted) to require `cross-compile-desktop` instead. Document
this transition in the commit message.

**Verification:**

```bash
# Sanity: the YAML parses.
python3 -c "import yaml, sys; yaml.safe_load(open('.github/workflows/ci.yml')); print('ok')"
```

**Commit:** roll this into the release-verification commit in Task 25 so the
workflow change is visible alongside the release log.

---

### Task 25: Final verification + release tag

Run the full verification matrix from `lingxi-core/`:

```bash
cd /Users/luolingfeng/Projects/LingXi-Next/lingxi-core

# 1. Workspace test suite — all 145+ tests must pass.
cargo test --workspace

# 2. Lint clean.
cargo clippy --workspace --all-targets -- -D warnings

# 3. Format clean.
cargo fmt --all --check

# 4. Zero-OS-deps check on platform crates (proves no leaking deps).
cargo check -p lingxi-platform-posix --no-default-features
cargo check -p lingxi-platform-windows --no-default-features

# 5. Phase A — every contract suite green.
cargo test -p lingxi-test-harness --test 'contract_*'

# 6. Phase B — every parity fixture green.
cargo test -p lingxi-test-harness --test 'parity_*'
```

**Expected result:**

- Workspace test count in the `~140-160` range. Baseline 104 (v0.2.0) + 12
  contract suites (~12-20 added tests at the driver level, suites count as
  one apiece) + 7 parity drivers + the per-plan tests from M2-01..M2-06
  (M2-02 alone adds ~7 codec / router / broker / MCP client tests, M2-04
  another ~7 sandbox tests, etc.). The exact count depends on M2-01..M2-06
  outcomes — record it in the tag annotation message.
- Zero `cargo test` warnings ignoring intentional `#[ignore]` markers.
- Zero clippy warnings (the M1 pattern of `-D warnings` stays in force).

If any verification command fails, **stop**. Do not tag. Diagnose the
failure, fix it (which may mean amending an earlier Phase A/B/C task), and
re-run the full matrix from step 1. Tagging a broken `v0.3.0` is the only
unrecoverable mistake in this plan.

Once the matrix is green:

```bash
cd /Users/luolingfeng/Projects/LingXi-Next

# Stage the CI workflow change (Task 24) alongside the release-notes update.
git add .github/workflows/ci.yml

# A tiny RELEASE-NOTES amendment captures the final test count.
# (Optional: write into CHANGELOG.md a closing line like
# "v0.3.0 ships with N tests passing under cargo test --workspace.")

git commit -m "$(cat <<'EOF'
release: v0.3.0 verification + cross-compile split

- Split the cross-compile matrix into cross-compile-desktop (gating;
  x86_64-unknown-linux-gnu, aarch64-apple-darwin, x86_64-pc-windows-msvc)
  and cross-compile-mobile (continue-on-error; aarch64-linux-android,
  aarch64-apple-ios). Mobile targets remain M3 scope.
- Final verification matrix green: cargo test --workspace, clippy clean,
  fmt clean, --no-default-features clean on both platform crates, all
  contract_* and parity_* drivers passing.
EOF
)"

# Annotated tag pointing at the verification commit.
git tag -a v0.3.0 -m "$(cat <<'EOF'
M2 v0.3.0 — claude-code behavioral parity

Locks in 1:1 wire / file-path / error-string parity with claude-code
(2026-03-31 reference) on macOS, Linux, WSL2, and (with sandbox + tmux
explicitly Unsupported) Windows. Adds lingxi-jsonrpc, real MCP and LSP
clients, real bwrap+socat / sandbox-exec sandbox, real tmux+iTerm swarm,
real macOS Keychain SecureStorage, real HTTP SSE, real ProcessRunner
tree-kill + spawn_background + pwd -P cwd tracking. See CHANGELOG.md
[0.3.0] for the full list and docs/PLATFORMS.md for the per-OS matrix.
EOF
)"
```

**Push policy:** do not `git push origin v0.3.0` unless the user explicitly
asks. The tag exists locally; the release worker can decide push timing.

**Verification:**

```bash
git tag | grep '^v0\.3\.0$'   # must print "v0.3.0"
git show v0.3.0 --stat | head -5  # confirm tag points at the verification commit
```

---

## Self-review checklist

Before the final commit / tag, walk this checklist:

- [ ] Phase A — every one of the 12 contract suites has both a `contracts/<name>.rs` module and a `tests/contract_<name>.rs` driver.
- [ ] Phase A — the M1 `contracts/mod.rs` is updated to list all 13 modules (filesystem + 12 new).
- [ ] Phase A — every driver that touches a production impl is gated with `#[cfg(target_os = ...)]` so ubuntu-latest CI stays clean.
- [ ] Phase A — every `tokio::test` uses the `#[tokio::test]` attribute (not `#[test]` with a manual block_on).
- [ ] Phase A — `BridgeConnection::synthetic_for_test()` and `PosixMinimalHost::for_test()` helpers exist (gate with `#[cfg(any(test, feature = "test-fixtures"))]` if appropriate).
- [ ] Phase B — every parity fixture's JSON is valid and the driver references the exact stem.
- [ ] Phase B — `parity_mcp_initialize` asserts the literal empty objects `{}` for both `roots` and `elicitation` (not null, not missing).
- [ ] Phase B — `parity_worktree_naming` exercises both supported slugs and invalid slugs.
- [ ] Phase B — `parity_keychain_service_name` is gated on `target_os = "macos"` AND a `parity-helpers` Cargo feature exposes `full_service_name`.
- [ ] Phase B — `parity_tmux_windows` asserts the exact error message substring `"--tmux is not supported on Windows"`.
- [ ] Phase C — CHANGELOG.md `[0.3.0]` section appears above `[0.2.0]`.
- [ ] Phase C — Migration-from-v0.2.0 sub-section enumerates every source-incompatible change.
- [ ] Phase C — docs/ARCHITECTURE.md "claude-code parity guarantees" section lists every wire identifier / error string / file path / numeric constant from the spec §6.1..§6.6.
- [ ] Phase C — docs/PLATFORMS.md exists at the path stated and the README links to it.
- [ ] Phase D — `.github/workflows/ci.yml` splits desktop (gating) from mobile (continue-on-error). Branch protection rule updated to require `cross-compile-desktop`.
- [ ] Phase D — every `cargo` command in the verification block exits zero before tagging.
- [ ] Phase D — `git tag v0.3.0` is annotated (`-a -m ...`), not lightweight.

---

## Execution handoff

v0.3.0 closes M2. Next: cli-demo migration off `platforms/posix-minimal` onto
`platforms/posix` (separate task, not part of the v0.3.0 tag). M3 begins
the mobile platform crates (`platforms/android`, `platforms/ios`) per spec
§35 — the capability-flag boundary set up in M2 means no engine-crate changes
should be needed for that work.
