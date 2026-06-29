# T2b — Connect Picker Detail Panel — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task on the CURRENT branch `t2b-remains` (do NOT use isolated worktrees — the user wants it on this worktree/branch for live verification). Steps use `- [ ]` syntax.

**Goal:** When a provider is highlighted in the `/connect` picker, show a per-provider **detail section** (its models, connected state, login method) inside the picker box — reusing the model data the TUI already has.

**Architecture:** A pure function builds detail lines for the highlighted provider from the existing `AppState.model_providers` (`request_model → (profile, label)`) + `provider_availability` + the provider's `ConnectMethod` set. The `Screen::ConnectPicker` render arm in `app.rs` appends those lines to the `render_picker_popup` box after the list. No engine change, no new threading, no two-pane rewrite. This is the achievable + verifiable form of the T2b detail panel.

**Tech Stack:** Rust, iocraft TUI, the shared `picker_popup` render, pure-reducer + render-oracle.

## Global Constraints

- Do NOT touch the `/model` picker, the OAuth flow, github_deploy, T1 theming, or the engine. Reuse `AppState.model_providers` (already threaded for `/model`) — no new engine data.
- Render via the EXISTING `render_picker_popup` (append `PopupLine`s); do NOT build a new two-pane layout component.
- The detail section is for the HIGHLIGHTED provider only; an empty model map / unknown provider must render gracefully ("no models listed"), never panic.
- Keep the picker's pure reducer (`handle_connect_picker_key`, grouping, `selectable`) UNCHANGED except for adding a read-only `highlighted_provider_id()` accessor.
- Stay on branch `t2b-remains` in the current worktree. Leave it UNMERGED for the user to live-verify (iTerm2/Warp) before any merge.
- ⛔ NEVER pipe cargo into tail/grep (redirect + echo EXIT). ⛔ NEVER broad-`pkill`. Temp files under /Users/luolingfeng/.claude/jobs/9d9ad4c5/tmp.

---

### Task 1: detail-line builder + highlighted-provider accessor

**Files:**
- Modify: `tui/src/screens/connect_picker.rs` (add `highlighted_provider_id()` + `provider_detail_lines(...)`)
- Test: same file test module

**Interfaces:**
- Produces: `ConnectPickerState::highlighted_provider_id(&self) -> Option<&str>`; free fn `provider_detail_lines(provider_id: &str, label: &str, connected: bool, methods: &[ConnectMethod], model_providers: &BTreeMap<String,(String,String)>) -> Vec<String>`.

- [ ] **Step 1: Write the failing tests:**

```rust
#[test]
fn highlighted_provider_id_tracks_selection() {
    let mut auth = std::collections::BTreeMap::new();
    auth.insert("anthropic".to_string(), "api_key".to_string());
    auth.insert("openai".to_string(), "api_key".to_string());
    let st = ConnectPickerState::from_connectable(&auth, &std::collections::BTreeMap::new());
    // first selectable row highlighted by default
    assert!(st.highlighted_provider_id().is_some());
}

#[test]
fn provider_detail_lines_shows_models_state_method() {
    let mut mp = std::collections::BTreeMap::new();
    mp.insert("claude-opus-4-8".to_string(), ("anthropic".to_string(), "Anthropic".to_string()));
    mp.insert("claude-sonnet-4-6".to_string(), ("anthropic".to_string(), "Anthropic".to_string()));
    mp.insert("gpt-5.5".to_string(), ("openai".to_string(), "OpenAI".to_string()));
    let lines = provider_detail_lines("anthropic", "Anthropic", true,
        &[ConnectMethod::Oauth, ConnectMethod::ApiKey], &mp);
    let joined = lines.join("\n");
    assert!(joined.contains("Anthropic"));
    assert!(joined.contains("connected"));         // connected state
    assert!(joined.contains("claude-opus-4-8"));    // its model
    assert!(!joined.contains("gpt-5.5"));           // NOT another provider's model
    assert!(joined.contains("Pro/Max") || joined.contains("API key")); // method(s)
    // unknown provider → graceful
    let empty = provider_detail_lines("nope", "Nope", false, &[ConnectMethod::ApiKey], &mp);
    assert!(empty.join("\n").to_lowercase().contains("no models"));
}
```

- [ ] **Step 2: Run, verify fail** — `cargo test -p tui -- detail provider_detail highlighted > $T/o.txt 2>&1; echo EXIT=$?` → FAIL.

- [ ] **Step 3: Implement.**
```rust
impl ConnectPickerState {
    /// The provider id of the currently highlighted selectable row (None if empty).
    #[must_use]
    pub fn highlighted_provider_id(&self) -> Option<&str> {
        let idx = *self.selectable().get(self.selected)?;
        Some(self.rows.get(idx)?.provider_id.as_str())
    }
}

/// Detail lines for the highlighted provider: connected state, its models
/// (filtered from `model_providers` by profile name), and its login method(s).
/// Reuses the `/model` data; no engine change. Graceful on unknown/empty.
#[must_use]
pub fn provider_detail_lines(
    provider_id: &str,
    label: &str,
    connected: bool,
    methods: &[ConnectMethod],
    model_providers: &std::collections::BTreeMap<String, (String, String)>,
) -> Vec<String> {
    let mut out = Vec::new();
    out.push(if connected { format!("{label} \u{2014} \u{2713} connected") }
             else { format!("{label} \u{2014} not connected") });
    // its models (request-model ids whose profile == provider_id), capped + elided
    let mut models: Vec<&str> = model_providers.iter()
        .filter(|(_, (profile, _))| profile == provider_id)
        .map(|(m, _)| m.as_str()).collect();
    models.sort_unstable();
    if models.is_empty() {
        out.push("Models: (no models listed)".to_string());
    } else {
        let shown: Vec<&str> = models.iter().take(4).copied().collect();
        let more = models.len().saturating_sub(shown.len());
        let mut s = format!("Models: {}", shown.join(", "));
        if more > 0 { s.push_str(&format!(", +{more} more")); }
        out.push(s);
    }
    // login method(s)
    let m: Vec<&str> = methods.iter().map(|x| match x {
        ConnectMethod::Oauth => "Pro/Max sign-in",
        ConnectMethod::ApiKey => "API key",
        ConnectMethod::CopilotDevice => "GitHub sign-in",
        ConnectMethod::OAuthSoon => "browser sign-in (coming soon)",
    }).collect();
    out.push(format!("Sign in: {}", m.join(" or ")));
    out
}
```

- [ ] **Step 4: Run, verify pass** — same cargo cmd → PASS.
- [ ] **Step 5: Commit** — `git add tui/src/screens/connect_picker.rs && git commit -m "feat(tui): connect-picker provider detail-line builder (T2b)"`

---

### Task 2: render the detail section in the picker box

**Files:**
- Modify: `tui/src/app.rs` (the `Screen::ConnectPicker` render arm ~:1054-1085)
- Test: a render/snapshot test (in app.rs tests or a tui test) asserting the highlighted provider's detail appears

**Interfaces:**
- Consumes: `highlighted_provider_id`, `provider_detail_lines` (Task 1), `provider_methods` + `provider_label` (existing), `AppState.model_providers` + `provider_availability`.

- [ ] **Step 1: Write the failing test** — drive the ConnectPicker render arm (or a small helper that builds the popup lines) and assert the highlighted provider's detail lines are present after the list. (Mirror the existing app.rs ConnectPicker render-arm test if one exists; else add a helper `connect_picker_popup_lines(state, model_providers, availability) -> Vec<PopupLine>` in app.rs and test THAT.)

- [ ] **Step 2: Run, verify fail.**

- [ ] **Step 3: Implement.** In the `Screen::ConnectPicker` render arm: after building the list `PopupLine`s, compute `let Some(pid) = c.highlighted_provider_id()`; build `provider_detail_lines(pid, &provider_label(pid), availability.get(pid).copied().unwrap_or(false), &provider_methods(pid, auth_tag_for(pid)), &st.model_providers)`; push a blank/separator `PopupLine` then each detail line as a non-selectable `PopupLine` (use the existing dim/sub-line `PopupLine`/`PopupMarker` variant the title/sub-header uses). Keep it inside the same `render_picker_popup(...)` call (append to the `lines` vec). Do NOT add a second box.

  (Resolve `auth_tag_for(pid)` from the row's existing `method`/the auth map already in scope in that arm; the row already carries `method`, so derive methods from `provider_methods(pid, ...)` or directly from the highlighted `ConnectRow`.)

- [ ] **Step 4: Run, verify pass** — `cargo test -p tui --lib > $T/o.txt 2>&1; echo EXIT=$?` → PASS (full crate; fix any breakage).
- [ ] **Step 5: Commit** — `git add tui/src/app.rs && git commit -m "feat(tui): render per-provider detail in the /connect picker (T2b)"`

---

### Task 3: PTY verification test

**Files:**
- Create: `tui/tests/connect_detail_pty.py`

- [ ] **Step 1: Build** `cargo build -p cli --bin lingxi-cli > $T/b.txt 2>&1; echo EXIT=$?`.
- [ ] **Step 2: Write a pexpect+pyte test** that opens `/connect` (via the command palette or `LINGXI_EXTRA_COMMANDS`), waits for the picker, and asserts the highlighted provider's detail line (e.g. "Models:" or "connected") is on the framebuffer; moving the selection updates the detail. Use a temp HOME; spawn the SPECIFIC child; `child.close(force=True)`; ⛔ NEVER broad-`pkill`. (Mirror `tui/tests/osc11_theme_pty.py` structure.)
- [ ] **Step 3: Run** `python tui/tests/connect_detail_pty.py > $T/p.txt 2>&1; echo EXIT=$?`. If the picker isn't reachable headless, fall back to asserting the render via a Rust integration test and note the limitation.
- [ ] **Step 4: Commit** — `git add tui/tests/connect_detail_pty.py && git commit -m "test(tui): PTY check for /connect picker detail panel (T2b)"`

---

## Self-Review
- Detail panel (models + state + method for highlighted provider) → Tasks 1+2. ✓
- Reuses existing model data, no engine change, renders in the existing box → constraints + Task 2. ✓
- Graceful on empty/unknown → Task 1 test. ✓
- Verifiable: unit tests (Task 1), render test (Task 2), PTY (Task 3). The two-pane side-by-side layout is intentionally OUT of scope (higher risk, the detail-section delivers the value); note for a future refinement.
- **Note for the implementer:** stay on `t2b-remains`; do NOT use isolated worktrees. After Task 2, run full `cargo test -p tui --lib`. The full two-pane "unified visual" + the T3 Static-rendering fix remain documented-deferred (see the T3 findings doc) — they need live-terminal verification and (T3) iocraft-level work.
