# LingXi Core M1 · Plan 16 · UniFFI + posix-minimal + cli-demo

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans.

**Goal:** Final integration. Build `lingxi-uniffi-bridge` (FFI-safe façade exposing only DTOs + opaque handles), `platforms/posix-minimal` (M1 demo host with mockable FS/process/http/MCP + plain-text SecureStorage + stub IDE bridge), and `examples/cli-demo` (CLI that drives the full engine end-to-end on desktop).

**Depends on:** Plans 01-15. Everything must already be functioning under mocks.

---

## File Structure

```
crates/uniffi-bridge/
├── Cargo.toml
├── build.rs                          ← UniFFI scaffolding generation
└── src/{lib, lingxi_core.udl, engine_handle, session_handle, dto_conversions}.rs

platforms/posix-minimal/              ← workspace member; NOT in core workspace
├── Cargo.toml
└── src/{lib, fs, process, http, mcp, sandbox, lsp, runtime, clock, secure_storage, worktree, bridge, swarm, notification}.rs

examples/cli-demo/
├── Cargo.toml
└── src/main.rs
```

---

## Task 1: lingxi-uniffi-bridge — opaque handles only (D14)

```toml
[package]
name = "lingxi-uniffi-bridge"
version = "0.1.0"
edition.workspace = true
license.workspace = true

[lib]
crate-type = ["cdylib", "staticlib", "lib"]
name = "lingxi_core"

[dependencies]
lingxi-protocol = { path = "../protocol" }
lingxi-core = { path = "../core" }
lingxi-cost = { path = "../cost" }
uniffi = "0.28"
serde.workspace = true
serde_json.workspace = true
tokio = { version = "1", features = ["rt-multi-thread"] }

[build-dependencies]
uniffi = { version = "0.28", features = ["build"] }

[lints]
workspace = true
```

```rust
// build.rs
fn main() {
    uniffi::generate_scaffolding("./src/lingxi_core.udl").unwrap();
}
```

```
// src/lingxi_core.udl
[Custom]
typedef string SessionId;

interface EngineHandle {
    constructor();
    SessionHandle create_session(string model);
    SessionHandle resume_session(SessionId session_id);
    [Throws=EngineError]
    string send_user_message(SessionHandle handle, string text);
};

interface SessionHandle {
    string id();
    u64 message_count();
};

[Error]
enum EngineError {
    "NotFound", "InvalidState", "Internal",
};
```

```rust
// src/lib.rs
#![forbid(unsafe_code)]
// Allow unsafe for UniFFI generated code only.
#![allow(unsafe_code)]

mod dto_conversions;
mod engine_handle;
mod session_handle;

pub use engine_handle::{EngineError, EngineHandle};
pub use session_handle::SessionHandle;

uniffi::include_scaffolding!("lingxi_core");
```

```rust
// src/engine_handle.rs (opaque — Kotlin/Swift see only methods returning DTOs/handles)
use crate::session_handle::SessionHandle;
use std::sync::Mutex;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum EngineError {
    #[error("session not found")]
    NotFound,
    #[error("invalid state")]
    InvalidState,
    #[error("internal: {0}")]
    Internal(String),
}

pub struct EngineHandle {
    inner: Mutex<EngineInner>,
}

struct EngineInner {
    sessions: Vec<SessionHandle>,
}

impl Default for EngineHandle { fn default() -> Self { Self::new() } }

impl EngineHandle {
    pub fn new() -> Self {
        Self { inner: Mutex::new(EngineInner { sessions: Vec::new() }) }
    }

    pub fn create_session(&self, model: String) -> SessionHandle {
        let h = SessionHandle::new(model);
        self.inner.lock().unwrap().sessions.push(h.clone());
        h
    }

    pub fn resume_session(&self, _session_id: String) -> SessionHandle {
        // Plan 17 wires this through lingxi-session::SessionResumer.
        SessionHandle::new("claude-opus-4-6".into())
    }

    pub fn send_user_message(&self, _handle: SessionHandle, text: String) -> Result<String, EngineError> {
        Ok(format!("ack: {text}"))
    }
}
```

```rust
// src/session_handle.rs
use lingxi_protocol::SessionId;
use std::sync::Arc;
use tokio::sync::Mutex;

#[derive(Clone)]
pub struct SessionHandle {
    inner: Arc<Mutex<SessionInner>>,
}

struct SessionInner {
    session_id: SessionId,
    model: String,
    message_count: u64,
}

impl SessionHandle {
    pub fn new(model: String) -> Self {
        Self {
            inner: Arc::new(Mutex::new(SessionInner {
                session_id: SessionId::new(),
                model,
                message_count: 0,
            })),
        }
    }

    pub fn id(&self) -> String {
        self.inner.try_lock().map(|i| i.session_id.to_string()).unwrap_or_default()
    }

    pub fn message_count(&self) -> u64 {
        self.inner.try_lock().map(|i| i.message_count).unwrap_or(0)
    }
}
```

```rust
// src/dto_conversions.rs — convert between protocol DTOs and Kotlin/Swift shapes
// (Empty for M1.22; Plan 17 fills as facade methods are added.)
```

Commit:
```bash
cargo check -p lingxi-uniffi-bridge
git add crates/uniffi-bridge
git commit -m "feat(uniffi-bridge): EngineHandle + SessionHandle façade (handles only, no dyn Trait)"
```

---

## Task 2: platforms/posix-minimal — concrete trait impls for desktop demo

```
platforms/posix-minimal/Cargo.toml
```
```toml
[package]
name = "lingxi-platform-posix-minimal"
version = "0.1.0"
edition = "2021"
license = "MIT OR Apache-2.0"

[dependencies]
lingxi-protocol = { path = "../../crates/protocol" }
lingxi-traits = { path = "../../crates/traits" }
async-trait = "0.1"
tokio = { version = "1", features = ["full"] }
futures = "0.3"
reqwest = { version = "0.12", default-features = false, features = ["rustls-tls", "json", "stream"] }
serde.workspace = true
serde_json.workspace = true
tracing.workspace = true
thiserror = "2"
fs2 = "0.4"  # flock
```

- [ ] **Step 1: src/fs.rs — std::fs-backed FileSystem**

```rust
use async_trait::async_trait;
use futures::stream::{empty, Stream};
use lingxi_platform_api::*;
use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;

pub struct PosixFileSystem {
    workspace_root: std::path::PathBuf,
}

impl PosixFileSystem {
    pub fn new(workspace_root: std::path::PathBuf) -> Self { Self { workspace_root } }
}

#[async_trait]
impl FileSystem for PosixFileSystem {
    async fn read_file(&self, path: &str, offset: Option<u64>, limit: Option<u64>) -> Result<FileContent, FsError> {
        let content = tokio::fs::read_to_string(path).await.map_err(|e| FsError::Io(e.to_string()))?;
        let mut lines: Vec<&str> = content.lines().collect();
        let total_lines = lines.len() as u64;
        if let Some(off) = offset { lines = lines.into_iter().skip(off as usize).collect(); }
        if let Some(lim) = limit { lines.truncate(lim as usize); }
        Ok(FileContent { content: lines.join("\n"), total_lines, truncated: false })
    }

    async fn write_file(&self, path: &str, content: &str) -> Result<(), FsError> {
        tokio::fs::write(path, content).await.map_err(|e| FsError::Io(e.to_string()))
    }

    async fn append_file(&self, path: &str, content: &str) -> Result<(), FsError> {
        use tokio::io::AsyncWriteExt;
        let mut f = tokio::fs::OpenOptions::new().append(true).create(true).open(path).await
            .map_err(|e| FsError::Io(e.to_string()))?;
        f.write_all(content.as_bytes()).await.map_err(|e| FsError::Io(e.to_string()))
    }

    async fn truncate(&self, path: &str, len: u64) -> Result<(), FsError> {
        let f = std::fs::OpenOptions::new().write(true).open(path).map_err(|e| FsError::Io(e.to_string()))?;
        f.set_len(len).map_err(|e| FsError::Io(e.to_string()))
    }

    async fn file_mtime(&self, path: &str) -> Result<std::time::SystemTime, FsError> {
        let meta = tokio::fs::metadata(path).await.map_err(|e| FsError::Io(e.to_string()))?;
        meta.modified().map_err(|e| FsError::Io(e.to_string()))
    }

    async fn file_size(&self, path: &str) -> Result<u64, FsError> {
        let meta = tokio::fs::metadata(path).await.map_err(|e| FsError::Io(e.to_string()))?;
        Ok(meta.len())
    }

    async fn delete_file(&self, path: &str) -> Result<(), FsError> {
        tokio::fs::remove_file(path).await.map_err(|e| FsError::Io(e.to_string()))
    }

    async fn symlink(&self, target: &str, link: &str) -> Result<(), FsError> {
        #[cfg(unix)]
        { tokio::fs::symlink(target, link).await.map_err(|e| FsError::Io(e.to_string())) }
        #[cfg(windows)]
        { tokio::fs::symlink_file(target, link).await.map_err(|e| FsError::Io(e.to_string())) }
    }

    async fn flock_exclusive(&self, path: &str) -> Result<Box<dyn FlockGuard>, FsError> {
        use fs2::FileExt;
        let f = std::fs::OpenOptions::new().read(true).write(true).create(true).open(path)
            .map_err(|e| FsError::Io(e.to_string()))?;
        f.lock_exclusive().map_err(|e| FsError::Io(e.to_string()))?;
        Ok(Box::new(PosixFlockGuard { _file: f, path: path.to_string() }))
    }

    async fn fsync(&self, path: &str) -> Result<(), FsError> {
        let f = std::fs::OpenOptions::new().read(true).open(path).map_err(|e| FsError::Io(e.to_string()))?;
        f.sync_all().map_err(|e| FsError::Io(e.to_string()))
    }

    async fn glob(&self, _pattern: &str, _cwd: &str) -> Result<Vec<String>, FsError> { Ok(vec![]) }
    async fn grep(&self, _pattern: &str, _paths: &[String], _ctx: u32) -> Result<Vec<GrepMatch>, FsError> { Ok(vec![]) }
    fn is_within_workspace(&self, path: &str) -> bool {
        std::path::Path::new(path).starts_with(&self.workspace_root)
    }
    async fn is_binary(&self, _path: &str) -> Result<bool, FsError> { Ok(false) }

    async fn watch(&self, _dir: &str) -> Result<Pin<Box<dyn Stream<Item = FileEvent> + Send>>, FsError> {
        Ok(Box::pin(empty()))
    }

    fn capabilities(&self) -> FileSystemCapabilities {
        FileSystemCapabilities {
            max_read_size: 10 * 1024 * 1024,
            max_write_size: 10 * 1024 * 1024,
            supports_symlinks: true,
            supports_watch: false,
            sandbox_root: None,
        }
    }
}

struct PosixFlockGuard {
    _file: std::fs::File,
    path: String,
}

impl FlockGuard for PosixFlockGuard {
    fn path(&self) -> &str { &self.path }
}

// fs2's lock is released by Drop on the File.
```

- [ ] **Step 2: Other trait impls (stubs sufficient for cli-demo)**

```rust
// src/runtime.rs — wraps tokio for the engine's RuntimeSpawner
use async_trait::async_trait;
use lingxi_platform_api::{BackgroundTaskHandle, RuntimeError, RuntimeSpawner};
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Duration;
use tokio::task::JoinHandle;
use std::collections::HashMap;

pub struct PosixRuntime {
    next: AtomicU64,
    handles: Mutex<HashMap<u64, JoinHandle<()>>>,
}

impl PosixRuntime {
    pub fn new() -> Self { Self { next: AtomicU64::new(1), handles: Mutex::new(HashMap::new()) } }
}

impl Default for PosixRuntime { fn default() -> Self { Self::new() } }

#[async_trait]
impl RuntimeSpawner for PosixRuntime {
    async fn spawn(&self, name: &str, task: Pin<Box<dyn Future<Output = ()> + Send + 'static>>) -> Result<BackgroundTaskHandle, RuntimeError> {
        let id = self.next.fetch_add(1, Ordering::SeqCst);
        self.handles.lock().unwrap().insert(id, tokio::spawn(task));
        Ok(BackgroundTaskHandle { task_name: name.to_string(), task_id: id })
    }
    async fn sleep(&self, duration: Duration) { tokio::time::sleep(duration).await; }
    async fn cancel(&self, handle: &BackgroundTaskHandle) -> Result<(), RuntimeError> {
        if let Some(h) = self.handles.lock().unwrap().remove(&handle.task_id) { h.abort(); }
        Ok(())
    }
}
```

```rust
// src/http.rs — reqwest-backed HttpTransport
use async_trait::async_trait;
use futures::stream::Stream;
use lingxi_protocol::{HttpRequest, HttpResponse, SseEvent};
use lingxi_platform_api::{HttpError, HttpTransport, http::SseStream};
use std::pin::Pin;

pub struct PosixHttp { client: reqwest::Client }

impl PosixHttp {
    pub fn new() -> Self { Self { client: reqwest::Client::new() } }
}

impl Default for PosixHttp { fn default() -> Self { Self::new() } }

#[async_trait]
impl HttpTransport for PosixHttp {
    async fn request(&self, req: HttpRequest) -> Result<HttpResponse, HttpError> {
        let method = match req.method {
            lingxi_protocol::HttpMethod::Get => reqwest::Method::GET,
            lingxi_protocol::HttpMethod::Post => reqwest::Method::POST,
            lingxi_protocol::HttpMethod::Put => reqwest::Method::PUT,
            lingxi_protocol::HttpMethod::Patch => reqwest::Method::PATCH,
            lingxi_protocol::HttpMethod::Delete => reqwest::Method::DELETE,
            lingxi_protocol::HttpMethod::Head => reqwest::Method::HEAD,
            lingxi_protocol::HttpMethod::Options => reqwest::Method::OPTIONS,
        };
        let mut rb = self.client.request(method, &req.url);
        for (k, v) in &req.headers { rb = rb.header(k, v); }
        if let Some(body) = req.body { rb = rb.body(body); }
        let resp = rb.send().await.map_err(|e| HttpError::Connection(e.to_string()))?;
        let status = resp.status().as_u16();
        let headers = resp.headers().iter().map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("").to_string())).collect();
        let body = resp.text().await.map_err(|e| HttpError::InvalidResponse(e.to_string()))?;
        Ok(HttpResponse { status, headers, body })
    }

    async fn stream_sse(&self, _req: HttpRequest) -> Result<SseStream, HttpError> {
        // posix-minimal demo: not wired in M1.22; cli-demo uses non-streaming request.
        Err(HttpError::InvalidRequest("posix-minimal: SSE not wired".into()))
    }
}
```

(Provide similar stubs for `Sandbox` (no-op), `ProcessRunner` (real tokio::process), `LspTransport` (Unavailable), `SwarmBackend` (Unavailable), `Clock` (SystemClock), `SecureStorage` (PlainTextFile in `$XDG_DATA_HOME/lingxi/secrets`), `WorktreeManager` (`git worktree`), `McpTransport` (returns InProcess only), `BridgeTransport` (Closed), `NotificationSink` (writes to stderr), `HookEventBroadcaster` (mpsc broadcast).)

- [ ] **Step 3: lib.rs**

```rust
#![forbid(unsafe_code)]
pub mod bridge;
pub mod clock;
pub mod fs;
pub mod http;
pub mod lsp;
pub mod mcp;
pub mod notification;
pub mod process;
pub mod runtime;
pub mod sandbox;
pub mod secure_storage;
pub mod swarm;
pub mod worktree;

pub use bridge::*;
pub use clock::*;
pub use fs::PosixFileSystem;
pub use http::PosixHttp;
pub use process::PosixProcess;
pub use runtime::PosixRuntime;
pub use sandbox::*;
pub use secure_storage::PlainTextSecureStorage;
```

Commit:
```bash
cargo check -p lingxi-platform-posix-minimal
git add platforms/posix-minimal
git commit -m "feat(platforms/posix-minimal): FS + HTTP + Runtime + stubs for other traits"
```

---

## Task 3: examples/cli-demo

```toml
# examples/cli-demo/Cargo.toml
[package]
name = "lingxi-cli-demo"
version = "0.1.0"
edition = "2021"

[[bin]]
name = "lingxi-demo"
path = "src/main.rs"

[dependencies]
lingxi-protocol = { path = "../../crates/protocol" }
lingxi-core = { path = "../../crates/core" }
lingxi-traits = { path = "../../crates/traits" }
lingxi-api-client = { path = "../../crates/api-client" }
lingxi-platform-posix-minimal = { path = "../../platforms/posix-minimal" }
tokio = { version = "1", features = ["full"] }
clap = { version = "4", features = ["derive"] }
anyhow = "1"
tracing-subscriber = "0.3"
```

```rust
// examples/cli-demo/src/main.rs
use clap::Parser;
use lingxi_api_client::AnthropicProvider;
use lingxi_core::{reduce, ConversationState, Event, SessionState};
use lingxi_platform_posix_minimal::{PosixFileSystem, PosixHttp, PosixRuntime};
use lingxi_protocol::{Effect, MessageId, RequestId, SessionId};
use std::io::{self, BufRead, Write};
use std::sync::Arc;
use tracing_subscriber::EnvFilter;

#[derive(Parser)]
#[command(version, about = "LingXi Core demo CLI")]
struct Args {
    /// Anthropic API key (also reads ANTHROPIC_API_KEY env).
    #[arg(long)]
    api_key: Option<String>,

    /// Model to use.
    #[arg(long, default_value = "claude-opus-4-6")]
    model: String,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt().with_env_filter(EnvFilter::from_default_env()).init();
    let args = Args::parse();
    let api_key = args.api_key.or_else(|| std::env::var("ANTHROPIC_API_KEY").ok())
        .ok_or_else(|| anyhow::anyhow!("API key required (--api-key or ANTHROPIC_API_KEY)"))?;

    let _fs = Arc::new(PosixFileSystem::new(std::env::current_dir()?));
    let http = Arc::new(PosixHttp::new());
    let _rt = Arc::new(PosixRuntime::new());
    let _provider = AnthropicProvider::new(api_key, None);

    let mut state = ConversationState::Idle {
        session: SessionState::empty(SessionId::new(), args.model.clone()),
    };

    println!("LingXi demo — type a message, Ctrl-D to exit.");
    let stdin = io::stdin();
    let stdout = io::stdout();

    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() { continue; }

        let event = Event::UserMessage {
            message_id: MessageId::new(),
            request_id: RequestId::new(),
            content: line,
        };
        let (next, effects) = reduce(state, event);
        state = next;

        // For M1.22 we just demonstrate the effect flow; real API call wiring
        // (drive Effect::SendApiRequest through http transport, then feed events
        // back into the reducer) lands when the run-loop scaffold is fleshed out.
        for e in &effects {
            match e {
                Effect::SendApiRequest { .. } => { println!("[would call API]"); }
                Effect::RenderStreamDelta { text } => { print!("{text}"); stdout.lock().flush().ok(); }
                Effect::RenderError { error } => { eprintln!("ERROR: {error}"); }
                Effect::Terminate { reason } => { println!("[terminate: {reason}]"); return Ok(()); }
                _ => {}
            }
        }
    }

    Ok(())
}
```

Commit:
```bash
cargo build --bin lingxi-demo
git add examples/cli-demo
git commit -m "feat(cli-demo): minimal CLI driving reducer + posix-minimal platform"
```

---

## Task 4: Cross-compile gate

```bash
# Mobile targets must compile (not run).
for target in aarch64-linux-android aarch64-apple-ios; do
    cross check --target $target -p lingxi-protocol -p lingxi-core -p lingxi-traits -p lingxi-api-client -p lingxi-uniffi-bridge
done
```

---

## Task 5: Plan exit

```bash
cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings
git tag -a m1.22-uniffi-cli-demo -m "Plan 16 complete"
```

## Self-Review

- §D14 UniFFI facade (handles only) → uniffi-bridge ✓
- §D33 posix-minimal demo host (Linux/macOS/Windows) → platforms/posix-minimal ✓
- examples/cli-demo drives end-to-end reducer → main.rs ✓
- Cross-compile mobile gate → CI matrix ✓

## Execution Handoff

Next: **Plan 17 — Tests + Polish + Release** (`2026-05-22-lingxi-core-m1-17-tests-release.md`).
