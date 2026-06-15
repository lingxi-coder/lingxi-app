# llm-client provider catalog — Phase 3-B (grouped /model picker + recent) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development. Fresh implementer subagent per task + two-stage review (spec then quality). Run edit-agents SEQUENTIALLY in one checkout (concurrent-agent worktree hazard). Steps use checkbox (`- [ ]`).

**Goal:** Replace the flat `/model` picker (`tui/src/screens/model.rs`) with an opencode-style grouped picker — a **Recent** section, **provider-grouped** sections, and a **search** filter — consuming both the existing routable model list AND the new catalog listings (Phase 3-A's `OrchestratorHandle::list_model_listings()`), with recent selections persisted across sessions.

**Architecture:** The picker merges two sources at open time: `list_available_models() -> Vec<String>` (currently-routable models — Anthropic/configured; MUST keep working) and `list_model_listings() -> Vec<traits::orchestrator::ModelListing>` (catalog, pretty display names + provider labels; routing via 3c). A pure `build_model_entries()` converts both into a uniform `Vec<ModelRow>`. `ModelScreenState` holds the rows + recent keys + a search query + the selected index; it computes a grouped, filtered visible list (Recent first, then provider groups — a model may appear in both, matching the screenshot). `handle_model_key` navigates with arrows (vim `j/k` is dropped so printable chars feed the query), filters on char/backspace, commits the highlighted row's wire id on Enter, cancels on Esc. Recent selections persist to `~/.claude/settings.json` (`recentModels` key) mirroring `theme_persist.rs`.

**Tech Stack:** Rust, `iocraft` TUI, `serde_json`, existing `crate::screens` patterns (`memory.rs` char-input, `theme_persist.rs` persistence).

**Spec:** `docs/superpowers/specs/2026-06-13-llm-client-provider-catalog-design.md` (Phase 3). **Base:** `parity-llm-client-3a` (Phases 1+2+3-A merged; `list_model_listings()` is live on the handle).

---

## Conventions (every task)

- Cargo root `lingxi-code/`; cargo from there, git from worktree repo root, explicit paths. NEVER `git add -A`.
- Lints: `cargo clippy -p tui --all-targets --no-deps -- -D warnings`; `-D missing-docs`; backtick doc symbols if `doc_markdown` fires.
- TDD: failing test, OBSERVE RED, implement.
- Commit `git commit -F <tempfile>`; trailer EXACTLY (own line, blank line before):
  ```
  Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
  ```
- Frozen `traits/` + `protocol/`: do NOT touch (this plan touches only `tui/`).
- `tui` must build; `engine-desktop`/`engine-mobile` unaffected.

## File structure (Phase 3-B)

- Create `lingxi-code/tui/src/recent_models.rs` — persisted recent-model store (mirrors `theme_persist.rs`).
- Modify `lingxi-code/tui/src/screens/model.rs` — `ModelRow`, `build_model_entries`, grouped `ModelScreenState`, `handle_model_key`, `render_model_to_string` (rewrite; update tests).
- Modify `lingxi-code/tui/src/state.rs` — `open_model` signature (takes rows + recent + current).
- Modify `lingxi-code/tui/src/root.rs` — `pump_open_model` fetches both sources + builds rows + loads recent; the Model key path records the recent on Commit.
- Modify `lingxi-code/tui/src/lib.rs` (or wherever modules are declared) — `mod recent_models;`.
- `app.rs` render is unchanged (it already calls `render_model_to_string`).

## Out of scope (3-B)

- Live routing to catalog providers (Plan 3c) — selecting one records it + calls `switch_model` (which accepts any id); request-time availability is unchanged.
- Favorite (`ctrl+f`), Connect-provider (`ctrl+a`), Free badges — deferred.
- Mouse support.

---

### Task 0: worktree + baseline

- [ ] **Step 1:** Worktree via `superpowers:using-git-worktrees`, off `parity-llm-client-3a` HEAD. Suggested `provider-catalog-p3b`. NOT the primary checkout.
- [ ] **Step 2:** `cd lingxi-code && cargo build -p tui` → clean.
- [ ] **Step 3:** `cd lingxi-code && cargo test -p tui 2>&1 | grep -E 'test result:' | awk '{s+=$4} END{print "baseline tui passed:", s}'` — record. No commit.

---

### Task 1: persisted recent-model store

**Files:** Create `lingxi-code/tui/src/recent_models.rs`; modify the module-declaration file (`lib.rs` — confirm where `mod theme_persist;` is declared and add `mod recent_models;` / `pub mod` to match).

- [ ] **Step 1:** Write `lingxi-code/tui/src/recent_models.rs` with EXACTLY:

```rust
//! Best-effort recent-model persistence via `~/.claude/settings.json`
//! `recentModels` field (mirrors `theme_persist.rs`; no new persistence engine).
//!
//! Stored as an ordered array (most-recent-first) of `{ "provider": ..,
//! "model": .. }` objects, capped at [`MAX_RECENT`]. Save/load degrade to a
//! no-op on any error — recents are a convenience, never load-bearing.
#![forbid(unsafe_code)]

use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

/// Max recent entries retained.
pub const MAX_RECENT: usize = 8;

/// One recent selection: the provider grouping key + the wire model id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecentModel {
    /// Provider grouping key (matches `ModelRow.provider_id`).
    pub provider_id: String,
    /// Wire model id (what `switch_model` accepts; matches `ModelRow.request_model`).
    pub request_model: String,
}

fn settings_path() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".claude").join("settings.json"))
}

/// Load the recent list (most-recent-first). Empty on any error.
#[must_use]
pub fn load_recent_models() -> Vec<RecentModel> {
    settings_path()
        .as_deref()
        .map(load_recent_models_from)
        .unwrap_or_default()
}

/// Test seam: load from an explicit path.
#[must_use]
pub fn load_recent_models_from(path: &Path) -> Vec<RecentModel> {
    let Ok(body) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let Ok(obj) = serde_json::from_str::<Map<String, Value>>(&body) else {
        return Vec::new();
    };
    let Some(arr) = obj.get("recentModels").and_then(Value::as_array) else {
        return Vec::new();
    };
    arr.iter()
        .filter_map(|v| {
            Some(RecentModel {
                provider_id: v.get("provider")?.as_str()?.to_string(),
                request_model: v.get("model")?.as_str()?.to_string(),
            })
        })
        .take(MAX_RECENT)
        .collect()
}

/// Record a selection at the front (dedup by `request_model`), cap at
/// [`MAX_RECENT`], best-effort persist. Logs + swallows errors.
pub fn record_recent_model(provider_id: &str, request_model: &str) {
    let Some(path) = settings_path() else {
        tracing::debug!("recent-model persist skipped: no home dir");
        return;
    };
    if let Err(e) = record_recent_model_to(&path, provider_id, request_model) {
        tracing::debug!(error = %e, "recent-model persist failed (session-only)");
    }
}

/// Test seam: read-modify-write `recentModels` at an explicit path, preserving
/// other keys. Pretty JSON + trailing newline (config-tool shape).
pub fn record_recent_model_to(
    path: &Path,
    provider_id: &str,
    request_model: &str,
) -> std::io::Result<()> {
    let mut list = load_recent_models_from(path);
    list.retain(|r| r.request_model != request_model);
    list.insert(
        0,
        RecentModel {
            provider_id: provider_id.to_string(),
            request_model: request_model.to_string(),
        },
    );
    list.truncate(MAX_RECENT);

    let mut obj: Map<String, Value> = std::fs::read_to_string(path)
        .ok()
        .and_then(|b| serde_json::from_str(&b).ok())
        .unwrap_or_default();
    let arr: Vec<Value> = list
        .iter()
        .map(|r| {
            let mut m = Map::new();
            m.insert("provider".to_string(), Value::String(r.provider_id.clone()));
            m.insert("model".to_string(), Value::String(r.request_model.clone()));
            Value::Object(m)
        })
        .collect();
    obj.insert("recentModels".to_string(), Value::Array(arr));
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut body = serde_json::to_string_pretty(&obj)?;
    body.push('\n');
    std::fs::write(path, body)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp() -> PathBuf {
        // Unique per-test temp file under the OS temp dir (no Date/rand needed:
        // use the test thread name via a fixed nonce per assertion site).
        let mut p = std::env::temp_dir();
        p.push(format!("lingxi-recent-{}.json", std::process::id()));
        let _ = std::fs::remove_file(&p);
        p
    }

    #[test]
    fn roundtrip_dedup_order_and_cap() {
        let path = tmp();
        record_recent_model_to(&path, "deepseek", "deepseek-chat").unwrap();
        record_recent_model_to(&path, "github-copilot", "gpt-5.4-nano").unwrap();
        // Re-select deepseek-chat → moves to front, no dup.
        record_recent_model_to(&path, "deepseek", "deepseek-chat").unwrap();
        let got = load_recent_models_from(&path);
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].request_model, "deepseek-chat");
        assert_eq!(got[0].provider_id, "deepseek");
        assert_eq!(got[1].request_model, "gpt-5.4-nano");

        // Cap at MAX_RECENT.
        for i in 0..(MAX_RECENT + 4) {
            record_recent_model_to(&path, "p", &format!("m{i}")).unwrap();
        }
        assert_eq!(load_recent_models_from(&path).len(), MAX_RECENT);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn preserves_other_keys() {
        let path = tmp();
        std::fs::write(&path, "{\n  \"theme\": \"dark\"\n}\n").unwrap();
        record_recent_model_to(&path, "deepseek", "deepseek-chat").unwrap();
        let body = std::fs::read_to_string(&path).unwrap();
        assert!(body.contains("\"theme\": \"dark\""));
        assert!(body.contains("\"recentModels\""));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn load_missing_or_garbage_is_empty() {
        assert!(load_recent_models_from(Path::new("/nonexistent/x.json")).is_empty());
    }
}
```

- [ ] **Step 2:** Declare the module. Find where `theme_persist` is declared (grep `mod theme_persist` in `tui/src/lib.rs` or `main`/`session`); add `mod recent_models;` (or `pub mod`) in the same place, matching visibility style.

- [ ] **Step 3:** OBSERVE RED first: temporarily change `assert_eq!(got[0].request_model, "deepseek-chat");` to `"WRONG"` and run:

Run: `cd lingxi-code && cargo test -p tui -- recent_models`
Expected: FAIL. Restore.

- [ ] **Step 4:** Run real → PASS:

Run: `cd lingxi-code && cargo test -p tui -- recent_models`
Expected: 3 tests PASS.

- [ ] **Step 5:** Clippy: `cd lingxi-code && cargo clippy -p tui --all-targets --no-deps -- -D warnings` → clean.

- [ ] **Step 6: Commit.**

```bash
git add lingxi-code/tui/src/recent_models.rs lingxi-code/tui/src/lib.rs
git commit -F - <<'EOF'
feat(tui): persisted recent-model store (~/.claude/settings.json recentModels)

Mirrors theme_persist: load/record with test seams, dedup-front + cap at 8,
preserves other settings keys. Best-effort (session-only on error).

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
```
(Adjust the second path if the module is declared somewhere other than `lib.rs`.)

---

### Task 2: ModelRow + build_model_entries (dual-source merge)

**Files:** Modify `lingxi-code/tui/src/screens/model.rs` (add the type + function + tests; do NOT yet touch `ModelScreenState`).

- [ ] **Step 1:** At the TOP of `model.rs` (after the module doc), add the row type + builder. Use the Phase 3-A DTO `traits::orchestrator::ModelListing`:

```rust
/// One selectable model row in the grouped picker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelRow {
    /// Human label shown in the row (e.g. "DeepSeek Chat" or "gpt-4o").
    pub display_model: String,
    /// Wire id passed to `switch_model` (and the dedup/recents key).
    pub request_model: String,
    /// Provider grouping key (e.g. "deepseek", "anthropic", "builtin").
    pub provider_id: String,
    /// Human provider header (e.g. "DeepSeek", "Anthropic", "Built-in").
    pub provider_label: String,
}

/// Human header for an EXISTING (`list_available_models`) provider prefix.
fn existing_provider_label(prefix: &str) -> String {
    match prefix {
        "anthropic" => "Anthropic",
        "openai" => "OpenAI",
        "gemini" => "Gemini",
        other => other,
    }
    .to_string()
}

/// Merge the routable model ids (`list_available_models`) and the catalog
/// listings (`list_model_listings`) into a uniform, de-duplicated row list.
/// Existing (routable) models come first, then catalog providers; a wire id
/// seen twice keeps its first (routable) occurrence.
#[must_use]
pub fn build_model_entries(
    existing: Vec<String>,
    catalog: Vec<traits::orchestrator::ModelListing>,
) -> Vec<ModelRow> {
    let mut rows = Vec::new();
    let mut seen = std::collections::HashSet::new();

    for id in existing {
        if !seen.insert(id.clone()) {
            continue;
        }
        let row = if let Some(rest) = id.strip_prefix('@') {
            ModelRow {
                display_model: format!("@{rest}"),
                request_model: id.clone(),
                provider_id: "alias".to_string(),
                provider_label: "Aliases".to_string(),
            }
        } else if let Some((p, m)) = id.split_once('/') {
            ModelRow {
                display_model: m.to_string(),
                request_model: id.clone(),
                provider_id: p.to_string(),
                provider_label: existing_provider_label(p),
            }
        } else {
            ModelRow {
                display_model: id.clone(),
                request_model: id.clone(),
                provider_id: "builtin".to_string(),
                provider_label: "Built-in".to_string(),
            }
        };
        rows.push(row);
    }

    for m in catalog {
        if !seen.insert(m.request_model.clone()) {
            continue;
        }
        rows.push(ModelRow {
            display_model: m.display_model,
            request_model: m.request_model,
            provider_id: m.provider_id,
            provider_label: m.provider_label,
        });
    }
    rows
}

#[cfg(test)]
mod entries_tests {
    use super::*;
    use traits::orchestrator::ModelListing;

    #[test]
    fn merges_existing_and_catalog_with_groups() {
        let existing = vec![
            "claude-opus-4-7".to_string(),
            "openai/gpt-4o".to_string(),
            "@fast".to_string(),
        ];
        let catalog = vec![ModelListing {
            display_model: "DeepSeek Chat".to_string(),
            request_model: "deepseek-chat".to_string(),
            provider_id: "deepseek".to_string(),
            provider_label: "DeepSeek".to_string(),
        }];
        let rows = build_model_entries(existing, catalog);

        let opus = rows.iter().find(|r| r.request_model == "claude-opus-4-7").unwrap();
        assert_eq!(opus.provider_label, "Built-in");
        let gpt = rows.iter().find(|r| r.request_model == "openai/gpt-4o").unwrap();
        assert_eq!(gpt.display_model, "gpt-4o");
        assert_eq!(gpt.provider_label, "OpenAI");
        let alias = rows.iter().find(|r| r.request_model == "@fast").unwrap();
        assert_eq!(alias.provider_label, "Aliases");
        let ds = rows.iter().find(|r| r.request_model == "deepseek-chat").unwrap();
        assert_eq!(ds.display_model, "DeepSeek Chat");
        assert_eq!(ds.provider_label, "DeepSeek");
    }

    #[test]
    fn dedups_by_request_model_existing_wins() {
        let rows = build_model_entries(
            vec!["deepseek-chat".to_string()],
            vec![ModelListing {
                display_model: "DeepSeek Chat".to_string(),
                request_model: "deepseek-chat".to_string(),
                provider_id: "deepseek".to_string(),
                provider_label: "DeepSeek".to_string(),
            }],
        );
        assert_eq!(rows.iter().filter(|r| r.request_model == "deepseek-chat").count(), 1);
        // Existing wins → grouped as Built-in, not DeepSeek.
        assert_eq!(rows[0].provider_label, "Built-in");
    }
}
```

- [ ] **Step 2:** OBSERVE RED: run before the impl compiles is N/A (this IS the impl). Instead temporarily break one assert (`assert_eq!(gpt.provider_label, "WRONG")`), run, confirm FAIL, restore:

Run: `cd lingxi-code && cargo test -p tui -- entries_tests`
Expected: FAIL then (after restore) PASS.

- [ ] **Step 3:** Clippy clean. Commit:

```bash
git add lingxi-code/tui/src/screens/model.rs
git commit -F - <<'EOF'
feat(tui): ModelRow + build_model_entries (merge routable + catalog models)

Uniform de-duplicated row list grouped by provider; existing routable models
first (Built-in/Anthropic/Aliases), then catalog providers.

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
```

---

### Task 3: grouped `ModelScreenState` + `handle_model_key`

**Files:** Modify `lingxi-code/tui/src/screens/model.rs` — REPLACE the old `ModelScreenState`, `ModelOutcome`, `handle_model_key` (keep `ModelRow`/`build_model_entries` from Task 2). Update/replace the old `#[cfg(test)] mod tests` (the flat-list tests) — they will be rewritten in this task + Task 4.

- [ ] **Step 1:** Replace the state + reducer. New definitions:

```rust
/// A rendered line in the picker: a non-selectable group header, or a
/// selectable model row (carrying its index into `rows`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VisibleLine {
    /// Group header (e.g. "Recent", "DeepSeek").
    Header(String),
    /// Selectable row: (index into `ModelScreenState::rows`).
    Item(usize),
}

/// Grouped `/model` picker state: all rows, recent keys, the active model, a
/// search query, and the highlighted selectable position.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ModelScreenState {
    /// All selectable rows (built by `build_model_entries`).
    pub rows: Vec<ModelRow>,
    /// Recent selections, most-recent-first, as `(provider_id, request_model)`.
    pub recent: Vec<(String, String)>,
    /// Active model wire id (rendered with a `(current)` badge).
    pub current: String,
    /// Search query (printable chars typed in the picker).
    pub query: String,
    /// Highlighted index into the flat list of selectable Items (not headers).
    pub selected: usize,
}

/// Controller outcome after a key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelOutcome {
    /// Stay open (highlight moved / query edited / inert key).
    Stay,
    /// Enter — commit `(provider_id, request_model)` of the highlighted row.
    Commit { provider_id: String, request_model: String },
    /// Esc — cancel with no change.
    Cancel,
}

impl ModelScreenState {
    /// Build the picker from merged rows + recent keys + the active model.
    #[must_use]
    pub fn new(rows: Vec<ModelRow>, recent: Vec<(String, String)>, current: String) -> Self {
        Self { rows, recent, current, query: String::new(), selected: 0 }
    }

    /// Whether a row matches the current query (case-insensitive substring over
    /// display name, provider label, and wire id). Empty query matches all.
    fn matches(&self, row: &ModelRow) -> bool {
        if self.query.is_empty() {
            return true;
        }
        let q = self.query.to_lowercase();
        row.display_model.to_lowercase().contains(&q)
            || row.provider_label.to_lowercase().contains(&q)
            || row.request_model.to_lowercase().contains(&q)
    }

    /// Compute the ordered visible lines: a `Recent` group (rows whose
    /// `(provider_id, request_model)` is in `recent`, in recent order), then one
    /// group per provider (first-seen order). Only query-matching rows appear;
    /// empty groups are omitted. A row may appear in both Recent and its group.
    #[must_use]
    pub fn visible_lines(&self) -> Vec<VisibleLine> {
        let mut out = Vec::new();

        // Recent section.
        let mut recent_items: Vec<usize> = Vec::new();
        for (pid, rm) in &self.recent {
            if let Some(idx) = self
                .rows
                .iter()
                .position(|r| &r.provider_id == pid && &r.request_model == rm)
            {
                if self.matches(&self.rows[idx]) && !recent_items.contains(&idx) {
                    recent_items.push(idx);
                }
            }
        }
        if !recent_items.is_empty() {
            out.push(VisibleLine::Header("Recent".to_string()));
            out.extend(recent_items.into_iter().map(VisibleLine::Item));
        }

        // Provider groups in first-seen order.
        let mut seen_labels: Vec<String> = Vec::new();
        for r in &self.rows {
            if !seen_labels.contains(&r.provider_label) {
                seen_labels.push(r.provider_label.clone());
            }
        }
        for label in seen_labels {
            let items: Vec<usize> = self
                .rows
                .iter()
                .enumerate()
                .filter(|(_, r)| r.provider_label == label && self.matches(r))
                .map(|(i, _)| i)
                .collect();
            if !items.is_empty() {
                out.push(VisibleLine::Header(label));
                out.extend(items.into_iter().map(VisibleLine::Item));
            }
        }
        out
    }

    /// The `rows` indices of the selectable items, in visible order.
    fn selectable(&self) -> Vec<usize> {
        self.visible_lines()
            .into_iter()
            .filter_map(|l| match l {
                VisibleLine::Item(i) => Some(i),
                VisibleLine::Header(_) => None,
            })
            .collect()
    }
}

/// Reduce a key. Arrows/`Up`/`Down` move over selectable items; printable chars
/// edit the search query (vim `j/k` nav is intentionally dropped so typing
/// works); Backspace deletes; Enter commits the highlighted row; Esc cancels.
#[must_use]
pub fn handle_model_key(
    state: &mut ModelScreenState,
    key: crossterm::event::KeyCode,
) -> ModelOutcome {
    use crossterm::event::KeyCode;
    let count = state.selectable().len();
    match key {
        KeyCode::Up => {
            state.selected = state.selected.saturating_sub(1);
            ModelOutcome::Stay
        }
        KeyCode::Down => {
            if count > 0 {
                state.selected = (state.selected + 1).min(count - 1);
            }
            ModelOutcome::Stay
        }
        KeyCode::Char(c) => {
            state.query.push(c);
            state.selected = 0;
            ModelOutcome::Stay
        }
        KeyCode::Backspace => {
            state.query.pop();
            state.selected = 0;
            ModelOutcome::Stay
        }
        KeyCode::Enter => match state.selectable().get(state.selected) {
            Some(&idx) => {
                let row = &state.rows[idx];
                ModelOutcome::Commit {
                    provider_id: row.provider_id.clone(),
                    request_model: row.request_model.clone(),
                }
            }
            None => ModelOutcome::Stay,
        },
        KeyCode::Esc => ModelOutcome::Cancel,
        _ => ModelOutcome::Stay,
    }
}
```

- [ ] **Step 2:** Add reducer tests (replace the old flat-list `mod tests` nav/commit tests with these):

```rust
#[cfg(test)]
mod reducer_tests {
    use super::*;
    use crossterm::event::KeyCode;

    fn rows() -> Vec<ModelRow> {
        build_model_entries(
            vec!["claude-opus-4-7".to_string()],
            vec![
                traits::orchestrator::ModelListing {
                    display_model: "DeepSeek Chat".to_string(),
                    request_model: "deepseek-chat".to_string(),
                    provider_id: "deepseek".to_string(),
                    provider_label: "DeepSeek".to_string(),
                },
                traits::orchestrator::ModelListing {
                    display_model: "GPT-5.4 nano".to_string(),
                    request_model: "gpt-5.4-nano".to_string(),
                    provider_id: "github-copilot".to_string(),
                    provider_label: "GitHub Copilot".to_string(),
                },
            ],
        )
    }

    #[test]
    fn recent_group_appears_first_and_resolves() {
        let st = ModelScreenState::new(
            rows(),
            vec![("github-copilot".to_string(), "gpt-5.4-nano".to_string())],
            "claude-opus-4-7".to_string(),
        );
        let lines = st.visible_lines();
        assert_eq!(lines[0], VisibleLine::Header("Recent".to_string()));
        // First selectable resolves to the recent gpt-5.4-nano row.
        let first = st.selectable()[0];
        assert_eq!(st.rows[first].request_model, "gpt-5.4-nano");
    }

    #[test]
    fn search_filters_and_resets_selection() {
        let mut st = ModelScreenState::new(rows(), vec![], "x".to_string());
        for c in "deep".chars() {
            assert_eq!(handle_model_key(&mut st, KeyCode::Char(c)), ModelOutcome::Stay);
        }
        let sel = st.selectable();
        assert_eq!(sel.len(), 1);
        assert_eq!(st.rows[sel[0]].request_model, "deepseek-chat");
        // Backspace widens again.
        let _ = handle_model_key(&mut st, KeyCode::Backspace); // "dee"
        assert!(st.selectable().len() >= 1);
    }

    #[test]
    fn enter_commits_provider_and_model() {
        let mut st = ModelScreenState::new(rows(), vec![], "x".to_string());
        // Filter to deepseek then commit.
        for c in "deepseek-chat".chars() {
            let _ = handle_model_key(&mut st, KeyCode::Char(c));
        }
        assert_eq!(
            handle_model_key(&mut st, KeyCode::Enter),
            ModelOutcome::Commit {
                provider_id: "deepseek".to_string(),
                request_model: "deepseek-chat".to_string()
            }
        );
        assert_eq!(handle_model_key(&mut st, KeyCode::Esc), ModelOutcome::Cancel);
    }

    #[test]
    fn nav_clamps_and_empty_is_inert() {
        let mut st = ModelScreenState::default();
        assert_eq!(handle_model_key(&mut st, KeyCode::Enter), ModelOutcome::Stay);
        assert_eq!(handle_model_key(&mut st, KeyCode::Down), ModelOutcome::Stay);
        assert_eq!(st.selected, 0);
    }
}
```

- [ ] **Step 3:** OBSERVE RED (the type changes won't compile against the old `render`/`state.rs`/`root.rs` yet — that's expected; Task 4 fixes render, Task 5 fixes wiring). To get a clean RED on the REDUCER specifically, this task will not fully compile the crate until Task 4/5. So: run the reducer tests via the module once `render_model_to_string` is updated. **Order note:** do Task 3 + Task 4 edits together before running `cargo test -p tui`, because the old `render_model_to_string` references removed fields. Implement Task 3's code, then immediately Task 4's render, THEN run tests. (Commit Task 3 + Task 4 separately is fine — but build only green after Task 4.)

If you prefer a strict RED here: temporarily stub `render_model_to_string` to `String::new()`, run `cargo test -p tui -- reducer_tests` (RED on a deliberately-broken assert, then green), then restore in Task 4.

- [ ] **Step 4:** Commit (after the crate compiles — i.e. you may fold this commit with Task 4 if needed; if committing separately, ensure `cargo build -p tui` is green, which requires Task 4's render done):

```bash
git add lingxi-code/tui/src/screens/model.rs
git commit -F - <<'EOF'
feat(tui): grouped ModelScreenState + handle_model_key (recent/search/groups)

Visible lines = Recent group then provider groups; arrows navigate selectable
items, printable chars filter (vim j/k dropped), Enter commits (provider,model),
Esc cancels.

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
```

---

### Task 4: grouped `render_model_to_string`

**Files:** Modify `lingxi-code/tui/src/screens/model.rs` — replace `render_model_to_string`.

- [ ] **Step 1:** Replace the renderer:

```rust
/// Render the grouped picker body (plain text; the iocraft layer wraps it).
#[must_use]
pub fn render_model_to_string(state: &ModelScreenState) -> String {
    let mut out = String::from("Select model\n");
    out.push_str(&format!("Search: {}\n", state.query));
    let lines = state.visible_lines();
    if lines.is_empty() {
        out.push_str("No models available.");
        return out;
    }
    let mut item_pos = 0usize;
    for line in &lines {
        match line {
            VisibleLine::Header(label) => {
                out.push('\n');
                out.push_str(label);
                out.push('\n');
            }
            VisibleLine::Item(idx) => {
                let row = &state.rows[*idx];
                let marker = if item_pos == state.selected { "\u{276F} " } else { "  " };
                out.push_str(marker);
                out.push_str(&row.display_model);
                out.push_str(&format!("  \u{00B7} {}", row.provider_label));
                if row.request_model == state.current {
                    out.push_str(" (current)");
                }
                out.push('\n');
                item_pos += 1;
            }
        }
    }
    out.push_str(
        "Press \u{2191}\u{2193} to navigate \u{00B7} type to search \u{00B7} Enter to select \u{00B7} Esc to go back",
    );
    out
}
```

- [ ] **Step 2:** Render tests (replace the old render tests):

```rust
#[cfg(test)]
mod render_tests {
    use super::*;

    fn st() -> ModelScreenState {
        let rows = build_model_entries(
            vec!["claude-opus-4-7".to_string()],
            vec![traits::orchestrator::ModelListing {
                display_model: "DeepSeek Chat".to_string(),
                request_model: "deepseek-chat".to_string(),
                provider_id: "deepseek".to_string(),
                provider_label: "DeepSeek".to_string(),
            }],
        );
        ModelScreenState::new(rows, vec![], "claude-opus-4-7".to_string())
    }

    #[test]
    fn renders_groups_headers_and_current_badge() {
        let out = render_model_to_string(&st());
        assert!(out.starts_with("Select model\nSearch: \n"));
        assert!(out.contains("\nBuilt-in\n"));
        assert!(out.contains("\nDeepSeek\n"));
        // Current model highlighted (selected==0 is the first item) + badged.
        assert!(out.contains("\u{276F} claude-opus-4-7  \u{00B7} Built-in (current)\n"));
        assert!(out.contains("  DeepSeek Chat  \u{00B7} DeepSeek\n"));
        assert!(out.ends_with("Esc to go back"));
    }

    #[test]
    fn empty_shows_locked_state() {
        let out = render_model_to_string(&ModelScreenState::default());
        assert_eq!(out, "Select model\nSearch: \nNo models available.");
    }
}
```

- [ ] **Step 3:** Now the crate's `model.rs` is self-consistent. Run reducer + render + entries + recent tests:

Run: `cd lingxi-code && cargo test -p tui -- model`  (and `-- recent_models`)
Expected: model.rs tests compile + PASS. (The crate as a whole still needs Task 5 for `state.rs`/`root.rs` to compile — if `cargo test -p tui` fails to BUILD due to `open_model`/`pump` signature mismatch, that is expected until Task 5; you can `cargo test -p tui --lib screens::model` only after Task 5, OR fold Tasks 3–5 and run tests once at the end. Prefer: implement Tasks 3,4,5 then run the full suite.)

- [ ] **Step 4:** Clippy (after Task 5 compiles the crate). Commit:

```bash
git add lingxi-code/tui/src/screens/model.rs
git commit -F - <<'EOF'
feat(tui): grouped render_model_to_string (search line + groups + badges)

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
```

---

### Task 5: wire pump + open_model + commit-records-recent

**Files:** Modify `lingxi-code/tui/src/state.rs`, `lingxi-code/tui/src/root.rs`.

- [ ] **Step 1: `open_model` signature.** In `state.rs`, change `open_model` to take the merged rows + recent + current:

```rust
    pub fn open_model(
        &mut self,
        rows: Vec<crate::screens::model::ModelRow>,
        recent: Vec<(String, String)>,
        current: String,
    ) {
        self.active_screen = Some(crate::screens::Screen::Model(
            crate::screens::model::ModelScreenState::new(rows, recent, current),
        ));
        crate::telemetry::screen_opened("model");
    }
```

- [ ] **Step 2: `pump_open_model`.** In `root.rs`, replace the body that fetches `list_available_models` + `open_model`:

```rust
    let existing = handle.list_available_models().await;
    let catalog = handle.list_model_listings().await;
    let rows = crate::screens::model::build_model_entries(existing, catalog);
    let recent: Vec<(String, String)> = crate::recent_models::load_recent_models()
        .into_iter()
        .map(|r| (r.provider_id, r.request_model))
        .collect();

    let mut st = state.lock().await;
    if st.pending_permission.is_some() || st.active_screen.is_some() {
        st.pending_open_model = true;
        return false;
    }
    let current = st.status.model.clone();
    st.open_model(rows, recent, current);
    true
```
(Keep the surrounding guard/`pending_open_model` logic; only the fetch + `open_model` call change.)

- [ ] **Step 3: Commit records recent.** In `root.rs`'s Model key path (the `Some(Screen::Model(state)) => { match handle_model_key(...) { ModelOutcome::Commit ... } }`), update for the new `Commit { provider_id, request_model }`:

```rust
                ModelOutcome::Commit { provider_id, request_model } => {
                    crate::recent_models::record_recent_model(&provider_id, &request_model);
                    st.pending_switch_model = Some(request_model);
                    st.close_screen();
                }
```

- [ ] **Step 4: Build + full test.**

Run: `cd lingxi-code && cargo build -p tui` → clean.
Run: `cd lingxi-code && cargo test -p tui 2>&1 | grep -E 'test result:' | awk '{s+=$4; f+=$6} END{print "passed:", s, "failed:", f}'` → failed: 0.
Run: `cd lingxi-code && cargo clippy -p tui --all-targets --no-deps -- -D warnings` → clean.

- [ ] **Step 5: Commit.**

```bash
git add lingxi-code/tui/src/state.rs lingxi-code/tui/src/root.rs
git commit -F - <<'EOF'
feat(tui): wire grouped model picker (merge sources, load + record recents)

pump_open_model merges list_available_models + list_model_listings into rows and
loads persisted recents; Commit records the (provider, model) recent before the
async switch.

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
```

---

### Task 6: verification + consumers

- [ ] **Step 1:** Full tui test:

Run: `cd lingxi-code && cargo test -p tui 2>&1 | grep -E 'test result:' | awk '{s+=$4; f+=$6} END{print "passed:", s, "failed:", f}'` → failed: 0; record passed.

- [ ] **Step 2:** Build the apps that embed the TUI:

Run: `cd lingxi-code && cargo build -p tui -p engine-desktop` → clean. (engine-mobile does not embed the desktop TUI; build it too if it depends on `tui`: `cargo build -p engine-mobile`.)

- [ ] **Step 3:** Frozen guard (this plan must not touch traits/protocol):

Run: `git diff parity-llm-client-3a -- lingxi-code/traits lingxi-code/protocol | grep -c '^[-+]'` → `0`.

- [ ] **Step 4:** No stray untracked: `git status --short | grep -E '^\?\?' || echo "(clean)"`.

- [ ] **Step 5:** No commit. 3-B complete — ready for final review + finishing-a-development-branch.

---

## Self-review (plan author)

**Spec coverage (Phase 3 picker):** Recent group (persisted, resolved + deduped) → Tasks 1, 3, 5. Provider grouping (pretty labels from catalog + derived for existing) → Tasks 2, 3, 4. Search filter → Tasks 3, 4. Recents key on `(provider_id, request_model)` pairs → Task 1. Consumes `list_model_listings()` (3-A) without regressing existing routable models (dual-source merge) → Tasks 2, 5. ✓

**Placeholder scan:** complete code for the store, merge, reducer, render; precise edits for `open_model`/`pump_open_model`/commit path. The only judgment points: module-declaration location for `recent_models` (Task 1 Step 2 — grep `mod theme_persist`) and the build-ordering note (Tasks 3–5 compile together). ✓

**Type consistency:** `ModelRow { display_model, request_model, provider_id, provider_label }`, `ModelScreenState { rows, recent: Vec<(String,String)>, current, query, selected }`, `ModelOutcome::Commit { provider_id, request_model }`, `VisibleLine::{Header,Item}`, `build_model_entries(Vec<String>, Vec<traits::orchestrator::ModelListing>)`, `RecentModel { provider_id, request_model }` are used identically across tasks. `render_model_to_string`/`handle_model_key`/`open_model` signatures match their call sites in `app.rs`/`root.rs`/`state.rs`. ✓

**Known risk:** Tasks 3–5 are interdependent (the crate only re-compiles green after Task 5). The plan flags this explicitly and instructs running the full `cargo test -p tui` after Task 5; per-task commits are still made, but the build-green checkpoint is Task 5 Step 4. An implementer executing strictly one-task-at-a-time should treat Tasks 3–5 as one compile unit.
