# Web Provider Configuration Screen Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add a `/web` TUI flow that lets users inspect, choose, configure, and test the client-side WebSearch provider.

**Architecture:** Mirror the existing `/connect` shape: pure picker/config reducers in `tui/src/screens`, small synchronous render helpers, and async root pumps for persistence/testing. Add a shared config/resolver layer in `tools/web` so TUI and runtime use the same active-provider + credential/settings resolution.

**Tech Stack:** Rust, iocraft TUI, `tool-api`/`platform_api::HttpTransport`, `secret::CredentialManager`, existing `~/.lingxi/settings.json` helpers, `cargo test`.

---

## Context

- Design spec: `docs/superpowers/specs/2026-06-30-web-config-screen-design.md`.
- Existing UI pattern: `crates/tui/src/screens/connect_picker.rs`, `connect.rs`, and `root.rs::pump_store_provider_key`.
- Existing settings-write patterns: `crates/tui/src/theme_persist.rs`, `recent_models.rs`.
- Existing web runtime: `crates/tools/web/src/web_search_client.rs`, `web_search.rs`.
- Existing credential store: `secret::CredentialManager::{set_provider_key,get_provider_key}` via `/connect`.

## Global rules

- Follow TDD for every behavior change: write test → watch fail → implement → watch pass.
- Keep secrets out of settings JSON. Tavily/Brave keys go only through `CredentialManager`.
- Preserve existing Anthropic hosted WebSearch behavior.
- Commit after each task.

---

### Task 1: Web search config model + settings persistence

**Files:**
- Create: `crates/tools/web/src/web_search_config.rs`
- Modify: `crates/tools/web/src/lib.rs`
- Test: `cargo test -p tool-web web_search_config`

**Step 1: Write failing tests**

Create `web_search_config.rs` with tests first (module can contain stubs that fail to compile until Step 3):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn provider_round_trips_settings_strings() {
        assert_eq!(WebSearchProvider::parse("auto"), Some(WebSearchProvider::Auto));
        assert_eq!(WebSearchProvider::parse("duckduckgo"), Some(WebSearchProvider::DuckDuckGo));
        assert_eq!(WebSearchProvider::Tavily.as_str(), "tavily");
    }

    #[test]
    fn reads_nested_settings_shape() {
        let v = json!({ "webSearch": { "provider": "brave", "searxngUrl": "https://search.local" } });
        let cfg = WebSearchConfig::from_settings_json(&v);
        assert_eq!(cfg.provider, WebSearchProvider::Brave);
        assert_eq!(cfg.searxng_url.as_deref(), Some("https://search.local"));
    }

    #[test]
    fn writes_nested_settings_shape_preserving_other_keys() {
        let mut v = json!({ "theme": "dark" });
        WebSearchConfig { provider: WebSearchProvider::Searxng, searxng_url: Some("https://s.example".into()) }
            .write_settings_json(&mut v);
        assert_eq!(v["theme"], "dark");
        assert_eq!(v["webSearch"]["provider"], "searxng");
        assert_eq!(v["webSearch"]["searxngUrl"], "https://s.example");
    }
}
```

**Step 2: Verify RED**

```bash
cd lingxi-code
cargo test -p tool-web web_search_config > /tmp/web-config-t1-red.txt 2>&1; echo EXIT=$?
```

Expected: FAIL because `WebSearchProvider` / `WebSearchConfig` do not exist.

**Step 3: Implement minimal config model**

Add:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WebSearchProvider { Auto, DuckDuckGo, Tavily, Brave, Searxng }

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebSearchConfig {
    pub provider: WebSearchProvider,
    pub searxng_url: Option<String>,
}
```

Implement `parse`, `as_str`, `Default` (`Auto`), `from_settings_json`, and `write_settings_json` using nested keys:

```json
{
  "webSearch": {
    "provider": "auto",
    "searxngUrl": "https://..."
  }
}
```

Export module in `tools/web/src/lib.rs`:

```rust
pub mod web_search_config;
```

**Step 4: Verify GREEN**

```bash
cargo test -p tool-web web_search_config > /tmp/web-config-t1-green.txt 2>&1; echo EXIT=$?
```

Expected: EXIT=0.

**Step 5: Commit**

```bash
git add crates/tools/web/src/web_search_config.rs crates/tools/web/src/lib.rs
git commit -m "feat(web): add WebSearch config settings model"
```

---

### Task 2: Runtime resolver reads config + credential store/env fallback

**Files:**
- Modify: `crates/tools/web/src/web_search_client.rs`
- Modify: `lingxi-code/tool-api/src/builtin_context.rs` (only if a credential-store seam is needed)
- Modify: `crates/apps/engine-desktop/src/lib.rs` (thread credential store into tool context if needed)
- Test: `cargo test -p tool-web web_search_client::tests::*resolve*`

**Step 1: Write failing resolver tests**

In `web_search_client.rs` tests, add pure tests for a new resolver function. Do not hit network:

```rust
#[test]
fn active_specific_provider_requires_configured_key() {
    let cfg = WebSearchConfig { provider: WebSearchProvider::Tavily, searxng_url: None };
    let keys = WebSearchKeyPresence { tavily: false, brave: false };
    let err = resolve_client_search_provider(&cfg, &keys, EnvSearchConfig::empty()).unwrap_err();
    assert!(err.contains("Tavily is selected but no API key is configured"));
}

#[test]
fn auto_fallback_prefers_secure_keys_then_searxng_then_duckduckgo() {
    let cfg = WebSearchConfig::default();
    assert!(matches!(resolve_client_search_provider(&cfg, &WebSearchKeyPresence { tavily: true, brave: true }, EnvSearchConfig::empty()).unwrap(), ClientSearchProvider::Tavily(_)));
    assert!(matches!(resolve_client_search_provider(&cfg, &WebSearchKeyPresence { tavily: false, brave: true }, EnvSearchConfig::empty()).unwrap(), ClientSearchProvider::Brave(_)));
    assert!(matches!(resolve_client_search_provider(&WebSearchConfig { provider: WebSearchProvider::Auto, searxng_url: Some("https://s.example".into()) }, &WebSearchKeyPresence { tavily: false, brave: false }, EnvSearchConfig::empty()).unwrap(), ClientSearchProvider::Searxng(_)));
}
```

**Step 2: Verify RED**

```bash
cargo test -p tool-web resolve_client_search_provider > /tmp/web-config-t2-red.txt 2>&1; echo EXIT=$?
```

Expected: FAIL because resolver types/functions do not exist.

**Step 3: Implement resolver**

Add:

```rust
pub struct WebSearchKeyPresence { pub tavily: bool, pub brave: bool }
pub struct EnvSearchConfig { pub tavily_key: Option<String>, pub brave_key: Option<String>, pub searxng_url: Option<String> }
```

Implement:

```rust
pub fn resolve_client_search_provider(
    cfg: &WebSearchConfig,
    keys: &WebSearchKeyPresence,
    env: EnvSearchConfig,
) -> Result<ClientSearchProvider, String>
```

Rules:
- Specific Tavily/Brave require secure key presence or env key.
- Specific SearXNG requires settings URL or env URL.
- DuckDuckGo always resolves.
- Auto tries Tavily → Brave → SearXNG → DuckDuckGo.

Keep pure resolver tests independent from secret storage by using fixed sample
key strings (`"secure-tavily"`, `"secure-brave"`) in resolver unit tests. The
async secure-store reads are covered later by the TUI/root persistence tests and
the runtime integration in Task 6.

**Step 4: Verify GREEN**

```bash
cargo test -p tool-web resolve_client_search_provider > /tmp/web-config-t2-green.txt 2>&1; echo EXIT=$?
```

Expected: EXIT=0.

**Step 5: Commit**

```bash
git add crates/tools/web/src/web_search_client.rs
git commit -m "feat(web): resolve active search provider from config"
```

---

### Task 3: `/web` picker pure reducer + detail lines

**Files:**
- Create: `crates/tui/src/screens/web_picker.rs`
- Modify: `crates/tui/src/screens/mod.rs`
- Test: `cargo test -p tui --lib web_picker`

**Step 1: Write failing reducer/detail tests**

Create tests in `web_picker.rs`:

```rust
#[test]
fn picker_rows_show_all_v1_providers_and_active_marker() {
    let snapshot = WebConfigSnapshot { active: WebSearchProvider::Auto, tavily_key: true, brave_key: false, searxng_url: Some("https://s.example".into()), last_test: None };
    let st = WebPickerState::from_snapshot(snapshot);
    assert_eq!(st.rows.iter().map(|r| r.id).collect::<Vec<_>>(), vec![WebSearchProvider::Auto, WebSearchProvider::DuckDuckGo, WebSearchProvider::Tavily, WebSearchProvider::Brave, WebSearchProvider::Searxng]);
    assert!(st.rows[0].active);
}

#[test]
fn detail_lines_show_configured_and_missing_status() {
    let snapshot = WebConfigSnapshot { active: WebSearchProvider::Auto, tavily_key: true, brave_key: false, searxng_url: None, last_test: None };
    let lines = web_provider_detail_lines(WebSearchProvider::Brave, &snapshot);
    assert!(lines.join("\n").contains("Missing API key"));
    let tavily = web_provider_detail_lines(WebSearchProvider::Tavily, &snapshot);
    assert!(tavily.join("\n").contains("Configured"));
}
```

**Step 2: Verify RED**

```bash
cargo test -p tui --lib web_picker > /tmp/web-config-t3-red.txt 2>&1; echo EXIT=$?
```

Expected: FAIL because module/types are missing.

**Step 3: Implement pure picker**

Implement:

- `WebConfigSnapshot`
- `WebProviderRow`
- `WebPickerState`
- `WebPickerOutcome::{Stay, Select(WebSearchProvider), Test(WebSearchProvider), Cancel}`
- `handle_web_picker_key`
- `web_provider_detail_lines`
- `render_web_picker_to_string` for snapshots.

Mirror `connect_picker.rs` patterns: selectable rows, search query, `Up`/`Down`, printable filter, `Enter`, `Esc`, and `t` for test.

**Step 4: Verify GREEN**

```bash
cargo test -p tui --lib web_picker > /tmp/web-config-t3-green.txt 2>&1; echo EXIT=$?
```

Expected: EXIT=0.

**Step 5: Commit**

```bash
git add crates/tui/src/screens/web_picker.rs crates/tui/src/screens/mod.rs
git commit -m "feat(tui): add pure /web provider picker"
```

---

### Task 4: `/web` config screen pure reducer

**Files:**
- Create: `crates/tui/src/screens/web_config.rs`
- Modify: `crates/tui/src/screens/mod.rs`
- Test: `cargo test -p tui --lib web_config`

**Step 1: Write failing reducer tests**

```rust
#[test]
fn tavily_key_buffer_edits_and_save_returns_secret_action() {
    let mut st = WebConfigState::new(WebSearchProvider::Tavily, WebConfigSnapshot::default());
    assert_eq!(handle_web_config_key(&mut st, key_char('a')), WebConfigOutcome::Stay);
    assert_eq!(handle_web_config_key(&mut st, key_enter()), WebConfigOutcome::SaveSecret { provider: WebSearchProvider::Tavily, secret: "a".into() });
}

#[test]
fn searxng_url_validates_before_save() {
    let mut st = WebConfigState::new(WebSearchProvider::Searxng, WebConfigSnapshot::default());
    for ch in "not a url".chars() { let _ = handle_web_config_key(&mut st, key_char(ch)); }
    assert_eq!(handle_web_config_key(&mut st, key_enter()), WebConfigOutcome::Stay);
    assert!(st.status_text().contains("Invalid URL"));
}
```

**Step 2: Verify RED**

```bash
cargo test -p tui --lib web_config > /tmp/web-config-t4-red.txt 2>&1; echo EXIT=$?
```

Expected: FAIL because module/types are missing.

**Step 3: Implement pure config reducer**

Implement:

- `WebConfigState { provider, snapshot, input, test_status }`
- `WebTestStatus::{Idle, Running, Success{provider,count,top_title,top_url}, Failed(String)}`
- `WebConfigOutcome::{Stay, Close, SaveSecret{provider,secret}, SaveSettings{provider,searxng_url}, Test(WebSearchProvider)}`
- `handle_web_config_key`
- `render_web_config_to_string`.

Rules:
- `Esc` closes.
- `Enter` saves.
- `t` requests test.
- Tavily/Brave require non-empty input before save.
- SearXNG requires parseable `http`/`https` URL.
- Auto/DuckDuckGo save provider with no input.

**Step 4: Verify GREEN**

```bash
cargo test -p tui --lib web_config > /tmp/web-config-t4-green.txt 2>&1; echo EXIT=$?
```

Expected: EXIT=0.

**Step 5: Commit**

```bash
git add crates/tui/src/screens/web_config.rs crates/tui/src/screens/mod.rs
git commit -m "feat(tui): add pure /web provider config screen"
```

---

### Task 5: TUI root/app wiring + persistence/test pumps

**Files:**
- Modify: `crates/tui/src/screens/mod.rs`
- Modify: `crates/tui/src/app.rs`
- Modify: `crates/tui/src/root.rs`
- Modify: `crates/tui/src/state.rs`
- Modify: `crates/tui/src/session.rs`
- Modify: `crates/tools/web/src/web_search_client.rs` (runtime load path if needed)
- Test: `cargo test -p tui --lib web_` and targeted root/app tests

**Step 1: Write failing integration-style unit tests**

Add tests around the same seams used by `/connect`:

```rust
#[test]
fn dispatch_bare_web_opens_picker() {
    let mut st = AppState::default();
    st.prompt_text = "/web".into();
    dispatch_submit(&mut st);
    assert!(matches!(st.active_screen, Some(Screen::WebPicker(_))));
}
```

Add root pump tests for persistence:

```rust
#[tokio::test]
async fn pump_save_web_secret_writes_credential_store() { /* mirror pump_store_provider_key */ }

#[tokio::test]
async fn pump_save_web_settings_writes_provider_and_searxng_url() { /* use tempfile settings path helper */ }
```

**Step 2: Verify RED**

```bash
cargo test -p tui --lib web_ > /tmp/web-config-t5-red.txt 2>&1; echo EXIT=$?
```

Expected: FAIL because screens/root state are not wired.

**Step 3: Implement screen variants + app render**

Add `Screen::WebPicker(WebPickerState)` and `Screen::WebConfig(WebConfigState)`.

In `app.rs`:

- Add `/web` submit intercept, next to `/connect`.
- Render picker via existing `render_picker_popup` style, appending detail lines.
- Render config screen as simple full-screen view, matching `/connect` text/input patterns.

**Step 4: Implement AppState pending actions**

Add to `AppState`:

```rust
pub pending_web_secret: Option<(WebSearchProvider, String)>;
pub pending_web_settings: Option<WebSearchConfig>;
pub pending_web_test: Option<WebSearchProvider>;
pub web_config_snapshot: WebConfigSnapshot;
```

Add helpers:

- `open_web_picker(snapshot)`
- `open_web_config(provider, snapshot)`
- take/clear pending methods.

**Step 5: Implement root pumps**

- `pump_save_web_secret`: credential id `web:tavily` / `web:brave`, writes secure store.
- `pump_save_web_settings`: read/update/write `~/.lingxi/settings.json` via `WebSearchConfig::write_settings_json`.
- `pump_test_web_search`: resolve config, run `run_client_web_search` with query `current weather Beijing`, update config screen status.

**Step 6: Verify GREEN**

```bash
cargo test -p tui --lib web_ > /tmp/web-config-t5-green.txt 2>&1; echo EXIT=$?
```

Expected: EXIT=0.

**Step 7: Commit**

```bash
git add crates/tui/src/screens/mod.rs crates/tui/src/app.rs crates/tui/src/root.rs crates/tui/src/state.rs crates/tui/src/session.rs crates/tools/web/src/web_search_client.rs
git commit -m "feat(tui): wire /web picker config persistence and test search"
```

---

### Task 6: Runtime WebSearch uses persisted credentials/settings

**Files:**
- Modify: `lingxi-code/tool-api/src/builtin_context.rs` if not already done
- Modify: `crates/apps/engine-desktop/src/lib.rs`
- Modify: `crates/tools/web/src/web_search.rs`
- Modify: `crates/tools/web/src/web_search_client.rs`
- Test: `cargo test -p tool-web web_search_client` plus targeted desktop tests if available

**Step 1: Write failing runtime tests**

Test resolver precedence:

```rust
#[test]
fn persisted_secure_key_precedes_env_key() {
    let cfg = WebSearchConfig { provider: WebSearchProvider::Tavily, searxng_url: None };
    let creds = ResolvedWebCredentials { tavily_key: Some("secure".into()), brave_key: None };
    let env = EnvSearchConfig { tavily_key: Some("env".into()), brave_key: None, searxng_url: None };
    assert!(matches!(resolve_runtime_search_provider(&cfg, &creds, env).unwrap(), ClientSearchProvider::Tavily(k) if k == "secure"));
}
```

**Step 2: Verify RED**

```bash
cargo test -p tool-web persisted_secure_key_precedes_env_key > /tmp/web-config-t6-red.txt 2>&1; echo EXIT=$?
```

Expected: FAIL because runtime resolver cannot load persisted credentials/settings yet.

**Step 3: Implement runtime credential/settings access**

Preferred minimal approach:

- Add a small WebSearch config provider seam to `BuiltinToolContext`, e.g.
  `web_search_config: Option<Arc<dyn WebSearchConfigProvider>>`, only if needed.
- Or thread enough concrete handles into `WebSearchTool` at registration time.

Implementation must let `WebSearchTool::run_client_side` load:

- active provider + SearXNG URL from settings;
- Tavily/Brave keys from secure store;
- env fallback values.

Avoid blocking inside the tool; use async credential calls where possible.

**Step 4: Verify GREEN**

```bash
cargo test -p tool-web web_search_client > /tmp/web-config-t6-green.txt 2>&1; echo EXIT=$?
```

Expected: EXIT=0.

**Step 5: Commit**

```bash
git add lingxi-code/tool-api/src/builtin_context.rs crates/apps/engine-desktop/src/lib.rs crates/tools/web/src/web_search.rs crates/tools/web/src/web_search_client.rs
git commit -m "feat(web): use persisted /web configuration at runtime"
```

---

### Task 7: Verification, PTY smoke, final build

**Files:**
- Optional create: `crates/tui/tests/web_config_pty.py`
- Modify only if PTY is practical.

**Step 1: Run full targeted test suite**

```bash
cd lingxi-code
cargo test -p tool-web > /tmp/web-config-tool-web.txt 2>&1; echo EXIT=$?
cargo test -p tui --lib > /tmp/web-config-tui-lib.txt 2>&1; echo EXIT=$?
cargo test -p engine-desktop --lib > /tmp/web-config-desktop.txt 2>&1; echo EXIT=$?
```

Expected: all EXIT=0.

**Step 2: Build CLI**

```bash
cargo build -p cli --bin lingxi-cli > /tmp/web-config-cli-build.txt 2>&1; echo EXIT=$?
```

Expected: EXIT=0.

**Step 3: Optional PTY test**

If reachable headless, create `tui/tests/web_config_pty.py` mirroring
`tui/tests/connect_detail_pty.py`:

- launch TUI with temp HOME;
- accept trust dialog;
- type `/web`;
- assert provider rows and detail text show;
- move selection to DuckDuckGo/Tavily;
- close without broad `pkill`.

Run:

```bash
python3 tui/tests/web_config_pty.py > /tmp/web-config-pty.txt 2>&1; echo EXIT=$?
```

Expected: EXIT=0. If not practical, document limitation in final report.

**Step 4: Commit final verification artifacts**

```bash
git add crates/tui/tests/web_config_pty.py
git commit -m "test(tui): PTY smoke for /web provider config" || true
```

Only commit if a PTY file was created.

---

## Final manual test

From any workspace:

```bash
/Users/luolingfeng/Projects/LingXi-Next/target/debug/lingxi-cli
```

Manual flow:

1. Run `/web`.
2. Select DuckDuckGo; save active provider; test search.
3. Ask: `今天的天气如何`.
4. Expected: model invokes WebSearch and receives client-side search results.

With an API provider:

1. Run `/web`.
2. Select Tavily or Brave.
3. Paste key.
4. Run test search.
5. Ask a current-events query.

Expected: configured provider is used and response cites sources.
