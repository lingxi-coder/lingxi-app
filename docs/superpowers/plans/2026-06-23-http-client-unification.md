# http-client Unification Implementation Plan (Plan A)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Collapse the LLM HTTP socket onto one pure-Rust `reqwest+rustls` transport in a new shared `http-client` crate, delete the duplicate copies + the stub, and move the `HttpTransport → llm_client::Transport` bridge into `llm-client`.

**Architecture:** New leaf crate `http-client` owns `ReqwestHttp` (impl of `platform_api::http::HttpTransport`). The platform crates (common/windows/posix/posix-minimal/ios/android) become thin re-export shims pointing at it. The generic `LlmTransportBridge<T: HttpTransport>` (impl `llm_client::Transport`) moves into `llm-client`, which gains `traits`+`protocol` deps. This is **byte-identical relocation** — the same reqwest code, in one place.

**Tech Stack:** Rust (edition/MSRV from workspace = 1.82), `reqwest` (rustls-tls), `tokio-tungstenite`, `eventsource-stream`, `tokio`.

## Global Constraints

- **MSRV 1.82** — no crate may raise the floor.
- **Byte-identical behavior.** This is relocation; no logic changes. Parity vs claude-code v2.1.185 is preserved.
- **reqwest deps verbatim:** `reqwest = { version = "0.12", default-features = false, features = ["rustls-tls", "json", "stream"] }`.
- **tungstenite deps verbatim:** `tokio-tungstenite = { version = "0.21", default-features = false, features = ["connect", "rustls-tls-webpki-roots"] }`.
- **Build/test env (guarded-ff):** prefix every cargo invocation with `CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0`.
- **Green at every task:** `cargo build --workspace --tests` must pass before each commit; run the owning crate's tests too.
- **Commit per task**, directly on the execution branch.
- **No cycles:** `http-client` MUST NOT depend on `llm-client`. `llm-client` MAY depend on `traits`+`protocol` (verified cycle-free).

---

## File Structure

- **Create** `lingxi-code/http-client/Cargo.toml` — new leaf crate manifest.
- **Create** `lingxi-code/http-client/src/lib.rs` — declares `reqwest_http`, re-exports `ReqwestHttp`.
- **Move** `lingxi-code/platforms/common/src/http.rs` (1264 lines) → `lingxi-code/http-client/src/reqwest_http.rs`.
- **Move** `lingxi-code/platforms/common/src/llm_transport.rs` (395 lines) → `lingxi-code/llm-client/src/transport_bridge.rs`.
- **Modify** `lingxi-code/Cargo.toml` — add `"http-client"` to `members`.
- **Modify** `lingxi-code/llm-client/Cargo.toml` — add `traits`, `protocol` deps.
- **Modify** `lingxi-code/llm-client/src/lib.rs` — add `pub mod transport_bridge;` + re-exports.
- **Modify** `platforms/common/{Cargo.toml,src/lib.rs,src/http.rs,src/llm_transport.rs}` — become re-export shims.
- **Modify** `platforms/windows/src/http.rs` (delete 326-line dup → re-export), `platforms/windows/Cargo.toml`.
- **Modify** `platforms/posix/src/http.rs`, `platforms/posix-minimal/src/http.rs` (drop stub), their Cargo.toml.
- **Modify** `platforms/ios/src/lib.rs:81`, `platforms/android/src/lib.rs:121` (ReqwestHttp path) + Cargo.toml.
- **Modify** `platforms/common/tests/llm_transport_test.rs` — re-point imports.

---

## Task 1: Scaffold `http-client`, move `ReqwestHttp` in, shim `platforms/common`

**Files:**
- Create: `lingxi-code/http-client/Cargo.toml`, `lingxi-code/http-client/src/lib.rs`
- Move: `lingxi-code/platforms/common/src/http.rs` → `lingxi-code/http-client/src/reqwest_http.rs`
- Modify: `lingxi-code/Cargo.toml` (members), `lingxi-code/platforms/common/Cargo.toml`, `lingxi-code/platforms/common/src/lib.rs`

**Interfaces:**
- Produces: crate `http-client` exposing `http_client::ReqwestHttp` (`impl platform_api::http::HttpTransport`), constructor `ReqwestHttp::new() -> Self`, `Default`.
- Consumes: `platform_api::http::HttpTransport`, `protocol::{HttpRequest, HttpResponse, ...}`.

- [ ] **Step 1: Create the crate manifest**

Create `lingxi-code/http-client/Cargo.toml`:

```toml
[package]
name = "http-client"
version = "0.1.0"
edition.workspace = true
rust-version.workspace = true
license.workspace = true

[dependencies]
platform-api = { path = "../platform-api" }
protocol = { path = "../protocol" }
async-trait = { workspace = true }
tokio = { workspace = true, features = ["full"] }
tokio-util = { version = "0.7", features = ["codec"] }
tokio-tungstenite = { version = "0.21", default-features = false, features = ["connect", "rustls-tls-webpki-roots"] }
reqwest = { version = "0.12", default-features = false, features = ["rustls-tls", "json", "stream"] }
eventsource-stream = "0.2"
bytes = { workspace = true }
futures = "0.3"
futures-core = { workspace = true }
futures-util = "0.3"
serde = { workspace = true }
serde_json = { workspace = true }
thiserror = { workspace = true }
tracing = { workspace = true }
url = "2"
http = "1"

[dev-dependencies]
axum = "0.7"
tokio = { workspace = true, features = ["macros", "rt-multi-thread"] }
futures-util = { workspace = true }
serde_json = { workspace = true }
async-trait = { workspace = true }

[lints]
workspace = true
```

- [ ] **Step 2: Register the crate in the workspace**

In `lingxi-code/Cargo.toml`, add `"http-client",` to the `members = [ ... ]` array, on the line immediately after `"llm-client",` (line 45).

- [ ] **Step 3: Move the transport source file**

Run:
```bash
cd lingxi-code
git mv platforms/common/src/http.rs http-client/src/reqwest_http.rs
```

- [ ] **Step 4: Create the crate root**

Create `lingxi-code/http-client/src/lib.rs`:

```rust
//! Shared pure-Rust HTTP transport (`reqwest` + `rustls-tls`).
//!
//! Single source of truth for the production [`platform_api::http::HttpTransport`]
//! used by every host (desktop, windows, mobile). Replaces the formerly
//! duplicated `platforms/{common,windows,posix}` copies.

pub mod reqwest_http;

pub use reqwest_http::ReqwestHttp;
```

- [ ] **Step 5: Fix module-internal references in the moved file**

In `lingxi-code/http-client/src/reqwest_http.rs`, the file previously lived in `platform_common`. Fix any references so it stands alone:
- Replace any `crate::` paths that pointed at `platform_common` siblings with the real crate (`platform_api::…`, `protocol::…`). The transport only needs `platform_api::http::*` and `protocol::{HttpRequest, HttpResponse, HttpMethod, ...}`.
- Remove any `use super::…`.
- If the file references `platform_common::…`, that is a smell — the transport must be self-contained; resolve by importing from `traits`/`protocol`.

- [ ] **Step 6: Turn `platforms/common`'s `http` into a re-export shim**

`platforms/common` already does `pub mod http;` + `pub use http::ReqwestHttp;` (lib.rs:14,22). Re-create the file it just lost as a shim — create `lingxi-code/platforms/common/src/http.rs`:

```rust
//! Re-export shim: the real `ReqwestHttp` now lives in the shared `http-client`
//! crate. Kept so `platform_common::http::ReqwestHttp` stays a valid path.

pub use http_client::ReqwestHttp;
```

Add the dep — in `lingxi-code/platforms/common/Cargo.toml` `[dependencies]`, add:
```toml
http-client = { path = "../../http-client" }
```

- [ ] **Step 7: Build the new crate and the shimmed common crate**

Run:
```bash
CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 cargo build -p http-client -p platform-common
```
Expected: PASS (compiles; `platform_common::http::ReqwestHttp` resolves to the re-export).

- [ ] **Step 8: Commit**

```bash
cd lingxi-code
git add http-client Cargo.toml platforms/common/Cargo.toml platforms/common/src/http.rs
git commit -m "feat(http-client): new crate owns ReqwestHttp; platform-common re-exports it"
```

---

## Task 2: Collapse the windows dup + posix-minimal stub onto `http-client`

**Files:**
- Modify: `platforms/windows/src/http.rs` (delete 326-line dup), `platforms/windows/Cargo.toml`
- Modify: `platforms/posix/src/http.rs`, `platforms/posix-minimal/src/http.rs`, their `Cargo.toml`
- Modify: `platforms/ios/src/lib.rs`, `platforms/android/src/lib.rs`, their `Cargo.toml`

**Interfaces:**
- Consumes: `http_client::ReqwestHttp` (from Task 1).
- Produces: `platform_windows::http::WindowsHttp`, `platform_posix::http::PosixHttp`, `platform_posix_minimal::http::PosixHttp` all alias the one `ReqwestHttp`.

- [ ] **Step 1: Replace the Windows duplicate with a re-export**

Replace the entire contents of `lingxi-code/platforms/windows/src/http.rs` with:

```rust
//! Re-export shim: Windows uses the shared `http-client` transport
//! (`reqwest` + `rustls-tls`), identical to every other host. The former
//! duplicate `WindowsHttp` impl was deleted in the http-client unification.

/// Production HTTP transport. Alias to the shared [`http_client::ReqwestHttp`];
/// preserves the historical `platform_windows::http::WindowsHttp` path.
pub use http_client::ReqwestHttp as WindowsHttp;
```

In `lingxi-code/platforms/windows/Cargo.toml`: add `http-client = { path = "../../http-client" }`; remove the now-unused `reqwest`, `tokio-tungstenite`, `tokio-stream`, `eventsource-stream` deps **only if** nothing else in `platforms/windows` uses them (grep first: `grep -rnE 'reqwest|tungstenite|eventsource' lingxi-code/platforms/windows/src`). If other modules use them, leave them.

- [ ] **Step 2: Re-point posix's re-export at http-client directly**

Replace the body line of `lingxi-code/platforms/posix/src/http.rs` (currently `pub use platform_common::http::ReqwestHttp as PosixHttp;`) with:

```rust
pub use http_client::ReqwestHttp as PosixHttp;
```

In `lingxi-code/platforms/posix/Cargo.toml`: add `http-client = { path = "../../http-client" }` (keep `platform-common` if used elsewhere).

- [ ] **Step 3: Replace the posix-minimal stub with the real transport**

Per the design decision (drop the no-network stub). Replace the entire contents of `lingxi-code/platforms/posix-minimal/src/http.rs` with:

```rust
//! The cli-demo / mobile-minimal host now uses the real shared transport
//! (`http-client`, `reqwest` + `rustls-tls`) instead of the former erroring
//! stub. Re-exported under the historical `PosixHttp` name.

pub use http_client::ReqwestHttp as PosixHttp;
```

In `lingxi-code/platforms/posix-minimal/Cargo.toml`: add `http-client = { path = "../../http-client" }`.

- [ ] **Step 4: Point ios/android constructors at http-client**

In `lingxi-code/platforms/ios/src/lib.rs:81`, change `platform_common::http::ReqwestHttp::new()` to `http_client::ReqwestHttp::new()`. Same at `lingxi-code/platforms/android/src/lib.rs:121`. Add `http-client = { path = "../../http-client" }` to both `platforms/ios/Cargo.toml` and `platforms/android/Cargo.toml`. (Leave their `platform-common` dep if used elsewhere.)

- [ ] **Step 5: Build all touched platform crates + their tests**

Run:
```bash
CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 cargo build --tests \
  -p platform-windows -p platform-posix -p platform-posix-minimal -p platform-ios -p platform-android
```
Expected: PASS. (The windows/posix SSE smoke tests now exercise the shared transport via the alias.)

- [ ] **Step 6: Commit**

```bash
cd lingxi-code
git add platforms/windows platforms/posix platforms/posix-minimal platforms/ios platforms/android
git commit -m "refactor(platforms): all hosts re-export http-client::ReqwestHttp; drop windows dup + posix-minimal stub"
```

---

## Task 3: Move `LlmTransportBridge` into `llm-client`

**Files:**
- Modify: `lingxi-code/llm-client/Cargo.toml` (add `traits`, `protocol`)
- Move: `lingxi-code/platforms/common/src/llm_transport.rs` → `lingxi-code/llm-client/src/transport_bridge.rs`
- Modify: `lingxi-code/llm-client/src/lib.rs`, `platforms/common/src/lib.rs`, `platforms/common/tests/llm_transport_test.rs`

**Interfaces:**
- Consumes: `platform_api::http::HttpTransport`, `protocol::*`, `crate::Transport` (the `llm_client::Transport` trait).
- Produces: `llm_client::LlmTransportBridge<T: platform_api::http::HttpTransport>` (`impl llm_client::Transport`), `LlmTransportBridge::new(http: T) -> Self`, and convenience `llm_client::transport::from_http(http: Arc<dyn platform_api::http::HttpTransport>) -> Arc<dyn crate::Transport>`.

- [ ] **Step 1: Add the deps to llm-client**

In `lingxi-code/llm-client/Cargo.toml` `[dependencies]`, add:
```toml
platform-api = { path = "../platform-api" }
protocol = { path = "../protocol" }
async-trait = { workspace = true }
```
(Only add `async-trait` if not already present.)

- [ ] **Step 2: Move the bridge source**

Run:
```bash
cd lingxi-code
git mv platforms/common/src/llm_transport.rs llm-client/src/transport_bridge.rs
```

- [ ] **Step 3: Re-home the module + add `from_http`**

In `lingxi-code/llm-client/src/lib.rs`, add near the other `pub mod` lines:
```rust
pub mod transport_bridge;
pub use transport_bridge::{from_http, LlmTransportBridge};
```

In `lingxi-code/llm-client/src/transport_bridge.rs`:
- Fix imports: references to `llm_client::Transport` / `llm_client::*` become `crate::…`; keep `platform_api::http::HttpTransport`, `protocol::…`. Remove any `platform_common::…` references (verify none remain — the bridge must be self-contained).
- Append the convenience constructor:
```rust
use std::sync::Arc;

/// Wrap any host [`platform_api::http::HttpTransport`] as an [`crate::Transport`].
#[must_use]
pub fn from_http(http: Arc<dyn platform_api::http::HttpTransport>) -> Arc<dyn crate::Transport> {
    Arc::new(LlmTransportBridge::new(http))
}
```
(If `LlmTransportBridge::new` requires `T: Sized` rather than `Arc<dyn …>`, add a blanket `impl platform_api::http::HttpTransport for Arc<dyn platform_api::http::HttpTransport>` is unnecessary — instead make `from_http` construct `LlmTransportBridge::new(http)` where `LlmTransportBridge<Arc<dyn HttpTransport>>`; confirm the existing `new` is generic `T: HttpTransport` and that `Arc<dyn HttpTransport>: HttpTransport` via the trait's existing blanket impl in `traits`. If no blanket impl exists, keep `from_http` generic: `pub fn from_http<T: platform_api::http::HttpTransport + 'static>(http: T) -> Arc<dyn crate::Transport>`.)

- [ ] **Step 4: Make platforms/common re-export the bridge from llm-client**

In `lingxi-code/platforms/common/src/lib.rs`, replace `pub mod llm_transport;` and `pub use llm_transport::LlmTransportBridge;` (lines 16,24) with:
```rust
pub use llm_client::LlmTransportBridge;
```
(`platform-common` already depends on `llm-client`.)

- [ ] **Step 5: Re-point the integration test imports**

In `lingxi-code/platforms/common/tests/llm_transport_test.rs:4`, change:
```rust
use platform_common::{LlmTransportBridge, ReqwestHttp};
```
to:
```rust
use http_client::ReqwestHttp;
use llm_client::LlmTransportBridge;
```
Add `http-client` and `llm-client` to `platforms/common/Cargo.toml` `[dev-dependencies]` if not already resolvable. (This test stays in `platform-common` as the cross-crate integration check exercising both the socket and the bridge.)

- [ ] **Step 6: Re-point the composition root**

Find where the production bridge is built and passed to `ProviderApiAdapter::new_with_routing` (engine-desktop `src/lib.rs`, the `llm_transport` argument near line 2091, and the `http_dyn` construction near line 1867). Change the construction to:
```rust
let llm_transport = llm_client::transport::from_http(http_dyn.clone());
```
or, if it used `platform_common::LlmTransportBridge::new(...)`, switch to `llm_client::LlmTransportBridge::new(...)`. Do the same in `engine-mobile/src/host.rs` if it constructs the bridge.

- [ ] **Step 7: Build + test the affected crates**

Run:
```bash
CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 cargo build --tests \
  -p llm-client -p platform-common -p engine-desktop
CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 cargo test -p llm-client -p platform-common
```
Expected: PASS (bridge tests + the integration test green).

- [ ] **Step 8: Commit**

```bash
cd lingxi-code
git add llm-client platforms/common apps/engine-desktop apps/engine-mobile
git commit -m "refactor(llm-client): own LlmTransportBridge + from_http; platform-common re-exports"
```

---

## Task 4: Workspace-wide green + straggler sweep

**Files:** none expected; this is the integration gate that catches any consumer of the old paths missed above.

- [ ] **Step 1: Full workspace build**

Run:
```bash
CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 cargo build --workspace --tests
```
Expected: PASS. If any crate fails on `platform_common::http::ReqwestHttp` / `LlmTransportBridge` / `PosixHttp`, fix it by importing from `http_client` / `llm_client` (or rely on the back-compat re-exports already added).

- [ ] **Step 2: Full workspace test (no fail-fast)**

Run:
```bash
CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 cargo test --workspace --no-fail-fast
```
Expected: PASS (known-flaky `powershell` / `web_fetch` may need isolated re-run, per repo norm).

- [ ] **Step 3: Confirm the dedup actually happened**

Run:
```bash
grep -rnE 'reqwest::Client' lingxi-code/platforms --include='*.rs' | grep -v '/target/'
```
Expected: **no hits** under `platforms/` (the only `reqwest::Client` for the LLM/web socket now lives in `http-client`). `platforms/common/{mcp_http,mcp_sse,mcp_ws}.rs` may still use reqwest/tungstenite for MCP — that is expected and out of scope.

- [ ] **Step 4: Commit (if any straggler fixes were needed)**

```bash
cd lingxi-code
git add -A
git commit -m "refactor(http-client): workspace-wide green; straggler re-points"
```

---

## Self-Review

**Spec coverage (Plan A = spec Phases 0-scaffold + 1):**
- ✅ One reqwest+rustls transport in `http-client` — Task 1.
- ✅ Delete win/posix dup copies — Task 2 (windows dup deleted; posix already a re-export, re-pointed).
- ✅ Finish posix-minimal/ios/android (drop stub) — Task 2/3.
- ✅ Move `LlmTransportBridge` into llm-client — Task 3.
- ✅ Byte-identical / green at each phase — build+test gates per task + Task 4.

**Placeholder scan:** No TBD/TODO. The one conditional (Step 3 of Task 3, `from_http` generic vs `Arc<dyn>`) is an explicit either/or with both forms given — resolve by reading the existing `LlmTransportBridge::new` signature.

**Type consistency:** `ReqwestHttp` (http-client) and `LlmTransportBridge`/`from_http` (llm-client) names match across all tasks and re-export shims.

**Note:** Plans B (`ApiService` consolidation) and C (OAuth merge) are separate documents, written next.
