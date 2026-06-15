//! `/permissions` viewer (claude-code `commands/permissions` → the
//! `PermissionRuleList` UI): a read-only list↔detail view of the configured
//! permission rules (behavior · rule · source) plus the active permission mode.
//! Pure reducer over a selected index + a dialog mode, mirroring `hooks.rs` /
//! `mcp.rs`. The rows are loaded OFF-DISK (like `skills.rs`) from the three
//! persistable settings tiers via [`load_permission_sections`].
//!
//! ## Data source
//! Reads the SAME three settings files the enforcement loader reads
//! (`engine-desktop`): `<claude_home>/settings.json` (user),
//! `<cwd>/.claude/settings.json` (project), `<cwd>/.claude/settings.local.json`
//! (local). These are the PERSISTED rules — the ones `LINGXI_ENFORCE_PERMISSIONS`
//! enforces and that an Ask→"always allow" (3c) writes to. The frozen
//! `PermissionGate` trait exposes no live-policy accessor, so a session-only
//! in-memory `AllowAlways` rule (transport gate) is NOT shown until it is
//! persisted — a documented limitation, not a parity gap.
//!
//! ## Scope
//! Read-only VIEWER. claude-code's full `/permissions` is an interactive MANAGER
//! (add/remove a rule, switch mode → settings write). Adding rules already has
//! the persistence mechanism (3c `persist_permission_update`); wiring an
//! interactive add/remove into this screen is a deferred follow-up.

use permission::{PermissionBehavior, PermissionRuleSource};

/// One permission-rule row. Owned/pre-rendered fields so the carrying `Screen`
/// variant keeps `PartialEq`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PermRuleRow {
    /// Behavior label: `"Allow"` / `"Deny"` / `"Ask"`.
    pub behavior: String,
    /// The rule string (`"Bash(npm run *)"`, `"Read(./secrets/**)"`, `"Edit"`).
    pub rule: String,
    /// Human source label: `"User"` / `"Project"` / `"Local"`.
    pub source: String,
}

/// List vs. detail.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum PermissionsDialogMode {
    /// Browsing the rule list.
    #[default]
    List,
    /// Viewing one rule's detail.
    Detail,
}

/// Screen state.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PermissionsScreenState {
    /// Active permission mode wire name (`"default"` / `"acceptEdits"` / …).
    pub mode: String,
    /// The configured-rule rows (user → project → local order).
    pub rows: Vec<PermRuleRow>,
    /// Selected row index.
    pub selected: usize,
    /// List or detail.
    pub dialog_mode: PermissionsDialogMode,
}

/// Controller outcome after a key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PermissionsOutcome {
    /// Stay open.
    Stay,
    /// Close the screen.
    Close,
}

/// Behavior → display label.
fn behavior_label(b: PermissionBehavior) -> &'static str {
    match b {
        PermissionBehavior::Allow => "Allow",
        PermissionBehavior::Deny => "Deny",
        PermissionBehavior::Ask => "Ask",
    }
}

/// Source → display label.
fn source_label(s: PermissionRuleSource) -> &'static str {
    match s {
        PermissionRuleSource::UserSettings => "User",
        PermissionRuleSource::ProjectSettings => "Project",
        PermissionRuleSource::LocalSettings => "Local",
        PermissionRuleSource::FlagSettings => "Flag",
        PermissionRuleSource::PolicySettings => "Policy",
        PermissionRuleSource::CliArg => "CLI",
        PermissionRuleSource::Command => "Command",
        PermissionRuleSource::Session => "Session",
    }
}

/// Wire name for a permission mode (claude-code `defaultMode` values). The
/// internal-only `Bubble`/`Auto` are never produced by
/// `default_mode_from_settings_json` (settings/CLI reject them) but are covered
/// for exhaustiveness.
fn mode_wire_name(m: permission::PermissionMode) -> &'static str {
    match m {
        permission::PermissionMode::Default => "default",
        permission::PermissionMode::Plan => "plan",
        permission::PermissionMode::AcceptEdits => "acceptEdits",
        permission::PermissionMode::BypassPermissions => "bypassPermissions",
        permission::PermissionMode::DontAsk => "dontAsk",
        permission::PermissionMode::Bubble => "bubble",
        permission::PermissionMode::Auto => "auto",
    }
}

/// Load the permission rules + mode from the three persistable settings tiers,
/// building a ready-to-render [`PermissionsScreenState`]. Pure-ish: synchronous
/// `std::fs` reads (the pump calls it on the blocking pool, off the UI executor,
/// mirroring `skills::load_skill_sections`). A missing file is skipped; a
/// malformed file's rules are skipped (best-effort, like the enforcement loader).
#[must_use]
pub fn load_permission_sections(
    cwd: &std::path::Path,
    claude_home: &std::path::Path,
) -> PermissionsScreenState {
    let mut rows = Vec::new();
    // Default mode unless a tier overrides it; local read LAST wins (matching
    // the enforcement loader's ascending-priority order).
    let mut mode = "default".to_string();

    for (path, source) in [
        (
            claude_home.join("settings.json"),
            PermissionRuleSource::UserSettings,
        ),
        (
            cwd.join(".claude").join("settings.json"),
            PermissionRuleSource::ProjectSettings,
        ),
        (
            cwd.join(".claude").join("settings.local.json"),
            PermissionRuleSource::LocalSettings,
        ),
    ] {
        let Ok(raw) = std::fs::read_to_string(&path) else {
            continue;
        };
        if let Ok(rules) = permission::permission_rules_from_settings_json(&raw, source) {
            for r in rules {
                rows.push(PermRuleRow {
                    behavior: behavior_label(r.behavior).to_string(),
                    rule: r.value.to_rule_string(),
                    source: source_label(source).to_string(),
                });
            }
        }
        if let Some(m) = permission::default_mode_from_settings_json(&raw) {
            mode = mode_wire_name(m).to_string();
        }
    }

    PermissionsScreenState {
        mode,
        rows,
        selected: 0,
        dialog_mode: PermissionsDialogMode::List,
    }
}

/// Reduce a key (mirrors `handle_hooks_key`).
#[must_use]
pub fn handle_permissions_key(
    state: &mut PermissionsScreenState,
    key: crossterm::event::KeyCode,
) -> PermissionsOutcome {
    use crossterm::event::KeyCode;
    match state.dialog_mode {
        PermissionsDialogMode::List => match key {
            KeyCode::Up | KeyCode::Char('k') => {
                state.selected = state.selected.saturating_sub(1);
                PermissionsOutcome::Stay
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if !state.rows.is_empty() {
                    state.selected = (state.selected + 1).min(state.rows.len() - 1);
                }
                PermissionsOutcome::Stay
            }
            KeyCode::Enter => {
                if !state.rows.is_empty() {
                    state.dialog_mode = PermissionsDialogMode::Detail;
                }
                PermissionsOutcome::Stay
            }
            KeyCode::Esc | KeyCode::Char('q') => PermissionsOutcome::Close,
            _ => PermissionsOutcome::Stay,
        },
        PermissionsDialogMode::Detail => match key {
            KeyCode::Esc | KeyCode::Left | KeyCode::Char('q') => {
                state.dialog_mode = PermissionsDialogMode::List;
                PermissionsOutcome::Stay
            }
            _ => PermissionsOutcome::Stay,
        },
    }
}

/// Render the screen body (list / detail).
#[must_use]
pub fn render_permissions_to_string(state: &PermissionsScreenState) -> String {
    match state.dialog_mode {
        PermissionsDialogMode::List => {
            let mut out = format!("Permissions\nMode: {}\n", state.mode);
            if state.rows.is_empty() {
                out.push_str("No permission rules configured.");
                return out;
            }
            for (i, row) in state.rows.iter().enumerate() {
                let marker = if i == state.selected {
                    "\u{276F} "
                } else {
                    "  "
                };
                out.push_str(marker);
                out.push_str(&format!("{} \u{00B7} {}", row.behavior, row.rule));
                out.push('\n');
            }
            out.push_str("Press \u{2191}\u{2193} to navigate \u{00B7} Enter to select \u{00B7} Esc to go back");
            out
        }
        PermissionsDialogMode::Detail => match state.rows.get(state.selected) {
            Some(row) => render_rule_detail(row),
            None => "Permissions\n(rule no longer available)".to_string(),
        },
    }
}

/// The detail body for one rule.
fn render_rule_detail(row: &PermRuleRow) -> String {
    let mut out = String::new();
    out.push_str(&row.rule);
    out.push('\n');
    out.push_str(&format!("Behavior: {}\n", row.behavior));
    out.push_str(&format!("Source: {} settings", row.source));
    out.push_str("\nesc to go back");
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyCode;

    fn row(behavior: &str, rule: &str, source: &str) -> PermRuleRow {
        PermRuleRow {
            behavior: behavior.into(),
            rule: rule.into(),
            source: source.into(),
        }
    }

    #[test]
    fn nav_enter_and_esc() {
        let mut s = PermissionsScreenState {
            rows: vec![
                row("Allow", "Bash", "User"),
                row("Deny", "Read(./s/**)", "Project"),
            ],
            ..PermissionsScreenState::default()
        };
        assert_eq!(
            handle_permissions_key(&mut s, KeyCode::Down),
            PermissionsOutcome::Stay
        );
        assert_eq!(s.selected, 1);
        assert_eq!(
            handle_permissions_key(&mut s, KeyCode::Enter),
            PermissionsOutcome::Stay
        );
        assert_eq!(s.dialog_mode, PermissionsDialogMode::Detail);
        assert_eq!(
            handle_permissions_key(&mut s, KeyCode::Esc),
            PermissionsOutcome::Stay
        );
        assert_eq!(s.dialog_mode, PermissionsDialogMode::List);
        assert_eq!(
            handle_permissions_key(&mut s, KeyCode::Esc),
            PermissionsOutcome::Close
        );
    }

    #[test]
    fn list_render_marks_selection_mode_and_behavior() {
        let s = PermissionsScreenState {
            mode: "acceptEdits".into(),
            rows: vec![
                row("Allow", "Bash", "User"),
                row("Deny", "Read(./s/**)", "Local"),
            ],
            selected: 1,
            dialog_mode: PermissionsDialogMode::List,
        };
        let out = render_permissions_to_string(&s);
        assert!(out.starts_with(
            "Permissions\nMode: acceptEdits\n  Allow \u{00B7} Bash\n\u{276F} Deny \u{00B7} Read(./s/**)\n"
        ));
        assert!(out.ends_with(
            "Press \u{2191}\u{2193} to navigate \u{00B7} Enter to select \u{00B7} Esc to go back"
        ));
    }

    #[test]
    fn empty_list_shows_locked_empty_state() {
        let out = render_permissions_to_string(&PermissionsScreenState {
            mode: "default".into(),
            ..PermissionsScreenState::default()
        });
        assert_eq!(
            out,
            "Permissions\nMode: default\nNo permission rules configured."
        );
    }

    #[test]
    fn detail_render_fields() {
        let r = row("Deny", "Read(./secrets/**)", "Local");
        assert_eq!(
            render_rule_detail(&r),
            "Read(./secrets/**)\nBehavior: Deny\nSource: Local settings\nesc to go back"
        );
    }

    #[test]
    fn load_reads_tiers_and_mode() {
        let tmp = std::env::temp_dir().join(format!("lx-perm-view-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let claude_home = tmp.join("home/.claude");
        let cwd = tmp.join("proj");
        std::fs::create_dir_all(&claude_home).unwrap();
        std::fs::create_dir_all(cwd.join(".claude")).unwrap();
        std::fs::write(
            claude_home.join("settings.json"),
            r#"{ "permissions": { "allow": ["Bash"] } }"#,
        )
        .unwrap();
        std::fs::write(
            cwd.join(".claude").join("settings.local.json"),
            r#"{ "permissions": { "deny": ["Read(./secrets/**)"], "defaultMode": "acceptEdits" } }"#,
        )
        .unwrap();

        let st = load_permission_sections(&cwd, &claude_home);
        assert_eq!(st.mode, "acceptEdits", "local defaultMode wins");
        // user Bash allow + local Read deny.
        assert!(st
            .rows
            .iter()
            .any(|r| r.rule == "Bash" && r.behavior == "Allow" && r.source == "User"));
        assert!(st.rows.iter().any(|r| r.rule == "Read(./secrets/**)"
            && r.behavior == "Deny"
            && r.source == "Local"));

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn load_missing_files_is_empty_default() {
        let tmp = std::env::temp_dir().join(format!("lx-perm-view-empty-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let st = load_permission_sections(&tmp.join("proj"), &tmp.join("home/.claude"));
        assert_eq!(st.mode, "default");
        assert!(st.rows.is_empty());
    }
}
