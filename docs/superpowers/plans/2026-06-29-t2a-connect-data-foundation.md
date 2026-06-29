# T2a — Connect UI Data Foundation — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Drive the `/connect` provider list from the real `llm-client` catalog (not a hardcoded list), make `✓` trustworthy by keying on `profile_name`, and select each provider's login flow from its real auth method — surfacing OAuth honestly.

**Architecture:** engine-desktop assembles one new plain-std map `provider_auth_methods: BTreeMap<profile_name, auth_tag>` from `builtin_presets()`, threaded into `AppState` exactly like the existing `provider_availability` map. The `tui` connect-picker joins the two maps (auth_methods = the real provider SET + method; availability = connected) to build rows; `connect.rs` gains an honest `Unavailable` flow; `pump_open_connect` routes by the real method instead of the hardcoded `== "github-copilot"` check.

**Tech Stack:** Rust, iocraft 0.8 TUI, the existing pure-reducer + `-> String` render-oracle pattern. Tests are plain `#[test]` units + one engine-desktop async integration test.

## Global Constraints

- Do NOT modify the `llm-client` catalog data model (`ProviderProfile`, `AuthStrategy`) or any byte-locked wire surface. `builtin_presets()` is read-only here.
- Do NOT touch the `/model` picker (`model.rs`), `github_deploy.rs`, `picker_popup.rs` rendering, or T1 theming.
- Preserve existing behaviour: the API-key field flow and the GitHub Copilot deployment→device-flow path must keep working byte-identically.
- `✓` MUST be keyed directly by `profile_name` against `provider_availability` — no per-row candidate-key lists (delete `any_connected`).
- OAuth methods are SURFACED honestly ("browser sign-in — coming soon"); do NOT wire any new OAuth flow.
- Thread new data as plain std types (mirror `provider_availability: BTreeMap<String,bool>`); introduce no new cross-crate DTO.
- Keep the pure-reducer + render-oracle architecture. New logic is unit-tested pure functions.
- Auth-tag vocabulary (the ONLY allowed values): `"api_key"`, `"copilot_device"`, `"oauth"`. `AuthStrategy::None` providers are skipped (not connectable).

---

## File Structure

- `apps/engine-desktop/src/lib.rs` — add `provider_auth_methods` to `DesktopRuntime` + assemble it in `build()` (Task 1).
- `tui/src/state.rs` — add `AppState.provider_auth_methods` field + `set_provider_auth_methods` setter (Task 2).
- `tui/src/session.rs` — thread the map at mount, beside `set_provider_availability` (Task 2).
- `tui/src/screens/connect_picker.rs` — `ConnectMethod` enum, `connect_display_meta`, `connect_rows_from`, `ConnectRow.method`; delete `any_connected`/`default_connect_rows` (Task 3).
- `tui/src/screens/connect.rs` — `ConnectFlow::Unavailable` + constructor + render + reducer arm (Task 4).
- `tui/src/app.rs:297`, `tui/src/root.rs:1046` — picker build sites → `from_connectable` (Task 5).
- `tui/src/root.rs:2011 pump_open_connect` — route by real method (Task 5).

---

### Task 1: engine-desktop assembles `provider_auth_methods`

**Files:**
- Modify: `apps/engine-desktop/src/lib.rs` (`DesktopRuntime` struct ~:1515; `build()` near the `provider_availability` assembly ~:4714)
- Test: same file (`#[cfg(test)] mod tests`)

**Interfaces:**
- Produces: `DesktopRuntime.provider_auth_methods: std::collections::BTreeMap<String, String>` (profile_name → one of `"api_key"`/`"copilot_device"`/`"oauth"`).

- [ ] **Step 1: Write the failing test** (in the engine-desktop test module, mirroring `build_surfaces_provider_availability_and_adapter`):

```rust
#[tokio::test]
async fn build_surfaces_provider_auth_methods_from_catalog() {
    let rt = /* build a DesktopRuntime via the same harness the availability test uses */;
    let m = &rt.provider_auth_methods;
    // Catalog-derived: keys are real profile_names, values are the tag vocabulary.
    assert_eq!(m.get("anthropic").map(String::as_str), Some("api_key"));
    assert_eq!(m.get("github-copilot").map(String::as_str), Some("copilot_device"));
    assert_eq!(m.get("openai-chatgpt").map(String::as_str), Some("oauth"));
    assert!(m.values().all(|v| matches!(v.as_str(), "api_key" | "copilot_device" | "oauth")));
}
```

- [ ] **Step 2: Run it, verify it fails** — `cargo test -p engine-desktop build_surfaces_provider_auth_methods_from_catalog > /tmp/o.txt 2>&1; echo EXIT=$?` → FAIL (no field). NEVER pipe cargo to tail.

- [ ] **Step 3: Implement** — add the field + assembly. Add to `DesktopRuntime` (beside `provider_availability`):

```rust
/// (T2a) Per-provider login method tag, keyed by profile_name, derived from
/// the real catalog auth strategy: "api_key" | "copilot_device" | "oauth".
/// Threaded into the TUI so the /connect picker shows the real method.
pub provider_auth_methods: std::collections::BTreeMap<String, String>,
```

In `build()`, near the `provider_availability` assembly, derive from the catalog:

```rust
let provider_auth_methods: std::collections::BTreeMap<String, String> =
    llm_client::catalog::presets::builtin_presets()
        .providers
        .iter()
        .filter_map(|p| {
            use llm_client::config::AuthStrategy::*;
            let tag = match p.auth {
                ApiKey | Bearer => "api_key",
                CopilotBearer => "copilot_device",
                ChatGptOAuth | OAuthBearer | AwsSigV4 | GcpToken | AzureToken => "oauth",
                None => return Option::None, // not connectable
            };
            Some((p.profile_name.clone(), tag.to_string()))
        })
        .collect();
```

Add `provider_auth_methods,` to the `DesktopRuntime { .. }` construction. (Confirm the exact `builtin_presets()` / `AuthStrategy` import paths compile; adjust the `use` if the re-export path differs.)

- [ ] **Step 4: Run the test, verify it passes** — `cargo test -p engine-desktop build_surfaces_provider_auth_methods_from_catalog > /tmp/o.txt 2>&1; echo EXIT=$?` → PASS. Also `cargo check -p engine-desktop` (the new struct field must be set everywhere `DesktopRuntime` is built — fix any "missing field" errors).

- [ ] **Step 5: Commit** — `git add apps/engine-desktop/src/lib.rs && git commit -m "feat(engine-desktop): assemble provider_auth_methods from the real catalog (T2a)"`

---

### Task 2: thread `provider_auth_methods` into `AppState`

**Files:**
- Modify: `tui/src/state.rs` (field beside `provider_availability` ~:890; setter beside `set_provider_availability` ~:1325; constructor default ~:1138)
- Modify: `tui/src/session.rs` (thread at mount beside the `set_provider_availability` call ~:424)
- Test: `tui/src/state.rs` test module

**Interfaces:**
- Consumes: `DesktopRuntime.provider_auth_methods` (Task 1).
- Produces: `AppState.provider_auth_methods: BTreeMap<String,String>` + `AppState::set_provider_auth_methods(&mut self, map: BTreeMap<String,String>)`.

- [ ] **Step 1: Write the failing test:**

```rust
#[test]
fn set_provider_auth_methods_installs_map() {
    let mut st = AppState::new_for_test(); // use whatever the existing state tests use
    let mut m = std::collections::BTreeMap::new();
    m.insert("anthropic".to_string(), "api_key".to_string());
    st.set_provider_auth_methods(m);
    assert_eq!(st.provider_auth_methods.get("anthropic").map(String::as_str), Some("api_key"));
}
```

- [ ] **Step 2: Run it, verify it fails** — `cargo test -p tui set_provider_auth_methods_installs_map > /tmp/o.txt 2>&1; echo EXIT=$?` → FAIL.

- [ ] **Step 3: Implement** — mirror `provider_availability` exactly:
  - Field on `AppState`: `provider_auth_methods: std::collections::BTreeMap<String, String>,`
  - Constructor default(s): `provider_auth_methods: std::collections::BTreeMap::new(),` (every `AppState { .. }` literal — `cargo check` finds them).
  - Setter:
```rust
/// (T2a) Install the engine-computed per-provider login-method map
/// (`DesktopRuntime.provider_auth_methods`, keyed by profile_name). Threaded at
/// TUI init so the /connect picker shows each provider's REAL login method.
pub fn set_provider_auth_methods(&mut self, map: std::collections::BTreeMap<String, String>) {
    self.provider_auth_methods = map;
}
```
  - In `session.rs`, beside the existing `set_provider_availability` thread (~:424):
```rust
initial_state.set_provider_auth_methods(std::mem::take(&mut runtime.provider_auth_methods));
```

- [ ] **Step 4: Run the test, verify it passes** — `cargo test -p tui set_provider_auth_methods_installs_map > /tmp/o.txt 2>&1; echo EXIT=$?` → PASS. `cargo check -p tui` clean.

- [ ] **Step 5: Commit** — `git add tui/src/state.rs tui/src/session.rs && git commit -m "feat(tui): thread provider_auth_methods into AppState (T2a)"`

---

### Task 3: data-driven connect-picker rows

**Files:**
- Modify: `tui/src/screens/connect_picker.rs` (replace `default_connect_rows`/`any_connected`/`from_availability`; add `ConnectMethod`, `connect_display_meta`, `connect_rows_from`; `ConnectRow.method`)
- Test: same file (`#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: `AppState.provider_auth_methods` + `AppState.provider_availability` (Task 2).
- Produces: `ConnectMethod` enum; `ConnectRow.method: ConnectMethod`; `connect_rows_from(auth_methods: &BTreeMap<String,String>, availability: &BTreeMap<String,bool>) -> Vec<ConnectRow>`; `ConnectPickerState::from_connectable(auth_methods, availability)`.

- [ ] **Step 1: Write the failing tests:**

```rust
#[test]
fn method_from_tag_table() {
    assert_eq!(ConnectMethod::from_tag("api_key"), ConnectMethod::ApiKey);
    assert_eq!(ConnectMethod::from_tag("copilot_device"), ConnectMethod::CopilotDevice);
    assert_eq!(ConnectMethod::from_tag("oauth"), ConnectMethod::OAuthSoon);
    assert_eq!(ConnectMethod::from_tag("???"), ConnectMethod::OAuthSoon); // unknown → honest soon
}

#[test]
fn rows_are_data_driven_with_trustworthy_check_and_method() {
    let mut auth = std::collections::BTreeMap::new();
    auth.insert("anthropic".to_string(), "api_key".to_string());
    auth.insert("github-copilot".to_string(), "copilot_device".to_string());
    auth.insert("brand-new-provider".to_string(), "api_key".to_string()); // not in curated map
    let mut avail = std::collections::BTreeMap::new();
    avail.insert("anthropic".to_string(), true);   // connected
    // github-copilot absent ⇒ not connected
    let rows = connect_rows_from(&auth, &avail);

    let a = rows.iter().find(|r| r.provider_id == "anthropic").unwrap();
    assert_eq!(a.label, "Anthropic");          // curated label
    assert!(a.connected);                        // ✓ keyed by profile_name
    assert_eq!(a.method, ConnectMethod::ApiKey);
    assert!(a.description.ends_with("API key")); // method-derived suffix, cannot lie

    let g = rows.iter().find(|r| r.provider_id == "github-copilot").unwrap();
    assert!(!g.connected);
    assert_eq!(g.method, ConnectMethod::CopilotDevice);

    let n = rows.iter().find(|r| r.provider_id == "brand-new-provider").unwrap();
    assert_eq!(n.label, "Brand New Provider");  // title-cased fallback — still renders
}
```

- [ ] **Step 2: Run, verify fail** — `cargo test -p tui -- connect_picker > /tmp/o.txt 2>&1; echo EXIT=$?` → FAIL.

- [ ] **Step 3: Implement.**
  - Add the method enum:
```rust
/// The real login method for a provider, derived from the catalog auth tag.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectMethod { ApiKey, CopilotDevice, OAuthSoon }

impl ConnectMethod {
    #[must_use]
    pub fn from_tag(tag: &str) -> Self {
        match tag {
            "api_key" => Self::ApiKey,
            "copilot_device" => Self::CopilotDevice,
            _ => Self::OAuthSoon, // "oauth" + any unknown → honest "coming soon"
        }
    }
    /// Human suffix appended to the row description (so it reflects reality).
    fn suffix(self) -> &'static str {
        match self {
            Self::ApiKey => " \u{2014} API key",
            Self::CopilotDevice => " \u{2014} device sign-in",
            Self::OAuthSoon => " \u{2014} browser sign-in (coming soon)",
        }
    }
}
```
  - Add `pub method: ConnectMethod,` to `ConnectRow`.
  - Curated display metadata (seed from the current 9 labels/blurbs/popular flags) + title-case fallback:
```rust
struct DisplayMeta { label: String, blurb: String, popular: bool }

fn connect_display_meta(profile_name: &str) -> DisplayMeta {
    // (label, blurb, popular) — blurb is the "what it is", NOT the method (the
    // method suffix is appended separately so descriptions can't lie).
    let curated: &[(&str, &str, &str, bool)] = &[
        ("anthropic", "Anthropic", "Claude models", true),
        ("openai", "OpenAI", "GPT models", true),
        ("openai-chatgpt", "OpenAI (ChatGPT)", "ChatGPT Plus/Pro", true),
        ("github-copilot", "GitHub Copilot", "Your Copilot subscription", true),
        ("gemini", "Google Gemini", "Gemini models", true),
        ("deepseek", "DeepSeek", "Chat / Reasoner", true),
        ("openrouter", "OpenRouter", "Unified gateway to many models", false),
        ("zai", "Z.AI", "GLM models", false),
        ("glm-coding", "GLM Coding Plan", "Zhipu coding-plan subscription", false),
    ];
    if let Some((_, label, blurb, popular)) = curated.iter().find(|(id, ..)| *id == profile_name) {
        return DisplayMeta { label: (*label).to_string(), blurb: (*blurb).to_string(), popular: *popular };
    }
    DisplayMeta { label: title_case(profile_name), blurb: "Provider".to_string(), popular: false }
}

/// "brand-new-provider" -> "Brand New Provider" (split on '-'/'_').
fn title_case(id: &str) -> String {
    id.split(|c| c == '-' || c == '_')
        .filter(|s| !s.is_empty())
        .map(|w| { let mut c = w.chars(); c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default() })
        .collect::<Vec<_>>()
        .join(" ")
}
```
  - The builder (replaces `default_connect_rows`):
```rust
/// Build picker rows from the REAL catalog (auth_methods = the provider set +
/// method) joined with availability (connected, keyed by profile_name).
#[must_use]
pub fn connect_rows_from(
    auth_methods: &BTreeMap<String, String>,
    availability: &BTreeMap<String, bool>,
) -> Vec<ConnectRow> {
    auth_methods.iter().map(|(profile_name, tag)| {
        let method = ConnectMethod::from_tag(tag);
        let meta = connect_display_meta(profile_name);
        ConnectRow {
            provider_id: profile_name.clone(),
            label: meta.label,
            description: format!("{}{}", meta.blurb, method.suffix()),
            popular: meta.popular,
            connected: availability.get(profile_name).copied().unwrap_or(false),
            method,
        }
    }).collect()
}
```
  - Replace `ConnectPickerState::from_availability` with:
```rust
#[must_use]
pub fn from_connectable(
    auth_methods: &BTreeMap<String, String>,
    availability: &BTreeMap<String, bool>,
) -> Self {
    Self::new(connect_rows_from(auth_methods, availability))
}
```
  - DELETE `any_connected` and `default_connect_rows`. Update the in-file `rows()` test helper (used at lines ~306-349) to build via `connect_rows_from` with a small fixed map, OR construct `ConnectRow`s directly including the new `method` field.

- [ ] **Step 4: Run, verify pass** — `cargo test -p tui -- connect_picker > /tmp/o.txt 2>&1; echo EXIT=$?` → PASS.

- [ ] **Step 5: Commit** — `git add tui/src/screens/connect_picker.rs && git commit -m "feat(tui): data-driven /connect rows + trustworthy ✓ + real method (T2a)"`

---

### Task 4: honest `Unavailable` connect flow

**Files:**
- Modify: `tui/src/screens/connect.rs` (`ConnectFlow` enum; constructor; `render_connect_to_string`; `handle_connect_key`)
- Test: same file test module

**Interfaces:**
- Produces: `ConnectFlow::Unavailable { label: String, reason: String }`; `ConnectScreenState::unavailable(label: &str, reason: &str) -> Self`.

- [ ] **Step 1: Write the failing tests:**

```rust
#[test]
fn unavailable_renders_honest_message_and_any_key_closes() {
    let mut st = ConnectScreenState::unavailable("OpenAI (ChatGPT)", "browser sign-in isn't available in this build yet");
    let body = render_connect_to_string(&st);
    assert!(body.contains("OpenAI (ChatGPT)"));
    assert!(body.contains("isn't available in this build yet"));
    assert!(body.contains("API key")); // tells them what DOES work
    // Any key (and Esc) closes — never a dead field.
    assert_eq!(handle_connect_key(&mut st, crossterm::event::KeyCode::Enter), ConnectAction::Cancel);
    assert_eq!(handle_connect_key(&mut st, crossterm::event::KeyCode::Esc), ConnectAction::Cancel);
}
```

- [ ] **Step 2: Run, verify fail** — `cargo test -p tui -- connect:: > /tmp/o.txt 2>&1; echo EXIT=$?` → FAIL.

- [ ] **Step 3: Implement.**
  - Add to `ConnectFlow`:
```rust
/// (T2a) A login method whose flow isn't wired in this build (OAuth). Honest
/// terminal screen — never a dead key field. Any key returns to the REPL.
Unavailable { label: String, reason: String },
```
  - Constructor on `ConnectScreenState` (beside `api_key`/`copilot_pending`):
```rust
#[must_use]
pub fn unavailable(label: &str, reason: &str) -> Self {
    Self { flow: ConnectFlow::Unavailable { label: label.to_string(), reason: reason.to_string() },
           key_buffer: String::new(), copilot: CopilotPhase::Starting }
}
```
  - In `handle_connect_key`, the early `if key == KeyCode::Esc { return Cancel }` already covers Esc. Add, right after it: an Unavailable arm where ANY key closes:
```rust
if matches!(st.flow, ConnectFlow::Unavailable { .. }) {
    return ConnectAction::Cancel;
}
```
  - In `render_connect_to_string`, add the match arm:
```rust
ConnectFlow::Unavailable { label, reason } => {
    out.push_str(&format!("Connect {label}\n"));
    out.push_str(&format!("{reason}.\n"));
    out.push_str("Connect with an API key instead \u{00B7} press any key to go back");
}
```

- [ ] **Step 4: Run, verify pass** — `cargo test -p tui -- connect:: > /tmp/o.txt 2>&1; echo EXIT=$?` → PASS. `cargo check -p tui` clean (the new `ConnectFlow` variant must be handled in every `match &st.flow` — the render + key fns; fix exhaustiveness errors).

- [ ] **Step 5: Commit** — `git add tui/src/screens/connect.rs && git commit -m "feat(tui): honest Unavailable connect flow for unwired OAuth (T2a)"`

---

### Task 5: wire the data-driven picker + method routing

**Files:**
- Modify: `tui/src/app.rs:297` and `tui/src/root.rs:1046` (picker build sites)
- Modify: `tui/src/root.rs:2011` (`pump_open_connect` routing)
- Test: `tui/src/root.rs` test module (a unit test on a small method-routing helper)

**Interfaces:**
- Consumes: `from_connectable` (Task 3), `ConnectScreenState::unavailable` (Task 4), `AppState.provider_auth_methods` (Task 2).

- [ ] **Step 1: Write the failing test** — extract the routing into a pure helper and test it:

```rust
#[test]
fn connect_route_picks_flow_by_method() {
    use crate::screens::connect_picker::ConnectMethod;
    assert_eq!(connect_route_for("oauth_tag_provider", Some("oauth")), ConnectRoute::Unavailable);
    assert_eq!(connect_route_for("github-copilot", Some("copilot_device")), ConnectRoute::Copilot);
    assert_eq!(connect_route_for("anthropic", Some("api_key")), ConnectRoute::ApiKey);
    assert_eq!(connect_route_for("typed-unknown", None), ConnectRoute::ApiKey); // fallback preserves today
}
```

- [ ] **Step 2: Run, verify fail** — `cargo test -p tui connect_route_picks_flow_by_method > /tmp/o.txt 2>&1; echo EXIT=$?` → FAIL.

- [ ] **Step 3: Implement.**
  - Add the pure helper near `pump_open_connect`:
```rust
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ConnectRoute { ApiKey, Copilot, Unavailable }

/// Route a /connect target to its flow by the real catalog method. Unknown
/// (typed `/connect foo` not in the catalog) → ApiKey, preserving today's behaviour.
pub(crate) fn connect_route_for(_provider: &str, auth_tag: Option<&str>) -> ConnectRoute {
    use crate::screens::connect_picker::ConnectMethod;
    match auth_tag.map(ConnectMethod::from_tag) {
        Some(ConnectMethod::CopilotDevice) => ConnectRoute::Copilot,
        Some(ConnectMethod::OAuthSoon) => ConnectRoute::Unavailable,
        _ => ConnectRoute::ApiKey, // ApiKey or unknown
    }
}
```
  - Rewrite the body of `pump_open_connect` (replace the `let is_copilot = provider == "github-copilot";` block + the `if is_copilot { .. } else { .. }`): look up the tag, route via the helper:
```rust
let tag = st.provider_auth_methods.get(&provider).cloned();
match connect_route_for(&provider, tag.as_deref()) {
    ConnectRoute::Copilot => st.open_github_deployment(),
    ConnectRoute::ApiKey => st.open_connect(
        crate::screens::connect::ConnectScreenState::api_key(&provider, &provider)),
    ConnectRoute::Unavailable => st.open_connect(
        crate::screens::connect::ConnectScreenState::unavailable(
            &provider, "browser sign-in isn't available in this build yet")),
}
```
  - Update BOTH picker build sites (`app.rs:297`, `root.rs:1046`): replace
    `ConnectPickerState::from_availability(&st.provider_availability)` (or the
    current arg) with
    `ConnectPickerState::from_connectable(&st.provider_auth_methods, &st.provider_availability)`.

- [ ] **Step 4: Run, verify pass** — `cargo test -p tui connect_route_picks_flow_by_method > /tmp/o.txt 2>&1; echo EXIT=$?` → PASS. Then full crate: `cargo test -p tui --lib > /tmp/o.txt 2>&1; echo EXIT=$?` → PASS (fix any call-site/exhaustiveness breaks). `cargo check -p engine-desktop` clean.

- [ ] **Step 5: Commit** — `git add tui/src/app.rs tui/src/root.rs && git commit -m "feat(tui): route /connect by real method + wire data-driven picker (T2a)"`

---

## Self-Review

**Spec coverage:**
- Real-catalog provider list → Task 1 (assemble from `builtin_presets()`) + Task 3 (`connect_rows_from` iterates the catalog keys) + Task 5 (call sites). ✓
- Trustworthy `✓` keyed by `profile_name` → Task 3 (`availability.get(profile_name)`, `any_connected` deleted). ✓
- Login method from real `AuthStrategy` → Task 1 (tag) + Task 3 (`ConnectMethod`) + Task 5 (routing). ✓
- OAuth surfaced honestly → Task 4 (`Unavailable`) + Task 5 (route `oauth` → Unavailable). ✓
- No new OAuth flow / no catalog-model change / `/model` + copilot path preserved → Global Constraints + Task 5 (copilot route unchanged). ✓
- Display label curated + title-case fallback → Task 3 (`connect_display_meta`). ✓
- Tests: pure units (Tasks 3,4,5) + engine integration (Task 1) + AppState (Task 2). ✓

**Placeholder scan:** No TBD/TODO; every code step shows code; commands show expected PASS/FAIL. The only "confirm the exact import path" notes (Task 1 `builtin_presets`/`AuthStrategy`, Task 2 `new_for_test`) are real-codebase lookups the implementer resolves by compiling, not placeholders.

**Type consistency:** `ConnectMethod {ApiKey, CopilotDevice, OAuthSoon}` + `from_tag` used identically in Tasks 3 & 5; `connect_rows_from(auth_methods, availability)` signature matches between Task 3 (def) and Task 5 (call); `provider_auth_methods: BTreeMap<String,String>` consistent across Tasks 1→5; `ConnectFlow::Unavailable { label, reason }` + `unavailable(label, reason)` consistent Tasks 4 & 5. ✓

**Note for the implementer:** Tasks are ordered (1→2 data seam, 3→4 pure logic, 5 wiring). Task 5 needs 2,3,4. After Task 5, run the full `cargo test -p tui --lib` + `cargo test -p engine-desktop` to confirm no regression in the `/model` picker or copilot path.
