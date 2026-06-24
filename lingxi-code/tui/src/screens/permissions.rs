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

use permission::{
    PermissionBehavior, PermissionRule, PermissionRuleSource, PermissionRuleValue,
    PermissionUpdate, PermissionUpdateDestination,
};

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

/// List vs. detail vs. delete-confirmation.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum PermissionsDialogMode {
    /// Browsing the rule list.
    #[default]
    List,
    /// Viewing one rule's detail.
    Detail,
    /// (PERM-1) Confirming deletion of the selected rule.
    ConfirmDelete,
}

/// (PERM-1) The active behavior tab (claude-code `PermissionRuleList` tabs).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PermTab {
    /// Allowed rules.
    #[default]
    Allow,
    /// Ask-first rules.
    Ask,
    /// Denied rules.
    Deny,
}

impl PermTab {
    /// Tab cycle order (Tab): Allow → Ask → Deny → Allow.
    #[must_use]
    pub fn next(self) -> Self {
        match self {
            PermTab::Allow => PermTab::Ask,
            PermTab::Ask => PermTab::Deny,
            PermTab::Deny => PermTab::Allow,
        }
    }

    /// Reverse cycle (BackTab).
    #[must_use]
    pub fn prev(self) -> Self {
        match self {
            PermTab::Allow => PermTab::Deny,
            PermTab::Ask => PermTab::Allow,
            PermTab::Deny => PermTab::Ask,
        }
    }

    /// Title in the tab header.
    #[must_use]
    pub fn title(self) -> &'static str {
        match self {
            PermTab::Allow => "Allow",
            PermTab::Ask => "Ask",
            PermTab::Deny => "Deny",
        }
    }

    /// The `behavior` label rows in this tab carry.
    #[must_use]
    fn behavior(self) -> &'static str {
        match self {
            PermTab::Allow => "Allow",
            PermTab::Ask => "Ask",
            PermTab::Deny => "Deny",
        }
    }

    /// Per-tab subtitle (claude-code `PermissionRuleList`).
    #[must_use]
    pub fn subtitle(self) -> &'static str {
        match self {
            PermTab::Allow => "Claude Code won't ask before using allowed tools.",
            PermTab::Ask => {
                "Claude Code will always ask for confirmation before using these tools."
            }
            PermTab::Deny => "Claude Code will always reject requests to use denied tools.",
        }
    }
}

/// Screen state.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PermissionsScreenState {
    /// Active permission mode wire name (`"default"` / `"acceptEdits"` / …).
    pub mode: String,
    /// The configured-rule rows (user → project → local order).
    pub rows: Vec<PermRuleRow>,
    /// Selected row index (within the active tab's filtered rows).
    pub selected: usize,
    /// List or detail.
    pub dialog_mode: PermissionsDialogMode,
    /// (PERM-1) The active behavior tab.
    pub tab: PermTab,
}

impl PermissionsScreenState {
    /// The rows belonging to the active tab (filtered by behavior).
    #[must_use]
    pub fn tab_rows(&self) -> Vec<&PermRuleRow> {
        self.rows
            .iter()
            .filter(|r| r.behavior == self.tab.behavior())
            .collect()
    }
}

/// Controller outcome after a key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PermissionsOutcome {
    /// Stay open.
    Stay,
    /// Close the screen.
    Close,
    /// (PERM-1) The user confirmed deletion of this rule — the caller persists
    /// the removal (async) + reloads. Carries the rule to remove.
    DeleteRule(PermRuleRow),
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
        tab: PermTab::Allow,
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
            // (PERM-1) Tab/BackTab cycle the Allow/Ask/Deny tabs, re-anchoring
            // the selection to the new tab's filtered rows.
            KeyCode::Tab => {
                state.tab = state.tab.next();
                state.selected = 0;
                PermissionsOutcome::Stay
            }
            KeyCode::BackTab => {
                state.tab = state.tab.prev();
                state.selected = 0;
                PermissionsOutcome::Stay
            }
            KeyCode::Up | KeyCode::Char('k') => {
                state.selected = state.selected.saturating_sub(1);
                PermissionsOutcome::Stay
            }
            KeyCode::Down | KeyCode::Char('j') => {
                let n = state.tab_rows().len();
                if n > 0 {
                    state.selected = (state.selected + 1).min(n - 1);
                }
                PermissionsOutcome::Stay
            }
            KeyCode::Enter => {
                if !state.tab_rows().is_empty() {
                    state.dialog_mode = PermissionsDialogMode::Detail;
                }
                PermissionsOutcome::Stay
            }
            // (PERM-1) `d` asks to delete the selected rule.
            KeyCode::Char('d') if !state.tab_rows().is_empty() => {
                state.dialog_mode = PermissionsDialogMode::ConfirmDelete;
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
            // (PERM-1) `d` from the detail also asks to delete.
            KeyCode::Char('d') => {
                state.dialog_mode = PermissionsDialogMode::ConfirmDelete;
                PermissionsOutcome::Stay
            }
            _ => PermissionsOutcome::Stay,
        },
        PermissionsDialogMode::ConfirmDelete => match key {
            // `y` confirms → emit the rule to delete (caller persists + reloads).
            KeyCode::Char('y' | 'Y') => match state.tab_rows().get(state.selected).copied() {
                Some(row) => {
                    let to_delete = row.clone();
                    state.dialog_mode = PermissionsDialogMode::List;
                    PermissionsOutcome::DeleteRule(to_delete)
                }
                None => {
                    state.dialog_mode = PermissionsDialogMode::List;
                    PermissionsOutcome::Stay
                }
            },
            // `n` / Esc cancels back to the list.
            KeyCode::Char('n' | 'N') | KeyCode::Esc => {
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
            // (PERM-1) Tab header (active in brackets) + (PERM-3) per-tab subtitle.
            let tab_mark = |t: PermTab| -> String {
                if t == state.tab {
                    format!("[{}]", t.title())
                } else {
                    format!(" {} ", t.title())
                }
            };
            let mut out = format!(
                "Permissions\nMode: {}\n{} {} {}\n{}\n",
                state.mode,
                tab_mark(PermTab::Allow),
                tab_mark(PermTab::Ask),
                tab_mark(PermTab::Deny),
                state.tab.subtitle(),
            );
            let rows = state.tab_rows();
            if rows.is_empty() {
                out.push_str(&format!("No {} rules configured.", state.tab.title().to_lowercase()));
                out.push('\n');
            } else {
                for (i, row) in rows.iter().enumerate() {
                    let marker = if i == state.selected { "\u{276F} " } else { "  " };
                    out.push_str(marker);
                    // The behavior is the active tab, so the row shows just the rule.
                    out.push_str(&row.rule);
                    out.push('\n');
                }
            }
            out.push_str("\u{2191}\u{2193} navigate \u{00B7} \u{21c6} tabs \u{00B7} Enter view \u{00B7} d delete \u{00B7} Esc close");
            out
        }
        // (PERM-1) Delete confirmation for the selected rule.
        PermissionsDialogMode::ConfirmDelete => match state.tab_rows().get(state.selected).copied() {
            Some(row) => format!("Delete rule {}?\ny to delete \u{00B7} n to cancel", row.rule),
            None => "Permissions\n(rule no longer available)".to_string(),
        },
        PermissionsDialogMode::Detail => match state.tab_rows().get(state.selected).copied() {
            Some(row) => render_rule_detail(row),
            None => "Permissions\n(rule no longer available)".to_string(),
        },
    }
}

/// (PERM-1) Reconstruct a [`PermissionUpdate`] from a display row for the
/// delete write. `None` for rows that aren't user-deletable (Flag / Policy /
/// CLI / Session / Command sources, or an unrecognised behavior).
#[must_use]
pub fn row_to_permission_update(row: &PermRuleRow) -> Option<PermissionUpdate> {
    let behavior = match row.behavior.as_str() {
        "Allow" => PermissionBehavior::Allow,
        "Deny" => PermissionBehavior::Deny,
        "Ask" => PermissionBehavior::Ask,
        _ => return None,
    };
    let (destination, source) = match row.source.as_str() {
        "User" => (
            PermissionUpdateDestination::UserSettings,
            PermissionRuleSource::UserSettings,
        ),
        "Project" => (
            PermissionUpdateDestination::ProjectSettings,
            PermissionRuleSource::ProjectSettings,
        ),
        "Local" => (
            PermissionUpdateDestination::LocalSettings,
            PermissionRuleSource::LocalSettings,
        ),
        _ => return None, // not persistable / not user-deletable.
    };
    Some(PermissionUpdate {
        rule: PermissionRule {
            value: PermissionRuleValue::from_rule_string(&row.rule),
            behavior,
            source,
        },
        destination,
    })
}

/// Split a rule string into `(toolName, ruleContent)`: `Read(./s/**)` →
/// `("Read", Some("./s/**"))`, `Bash` → `("Bash", None)`.
fn parse_rule(rule: &str) -> (&str, Option<&str>) {
    if let Some(open) = rule.find('(') {
        if rule.ends_with(')') {
            return (&rule[..open], Some(&rule[open + 1..rule.len() - 1]));
        }
    }
    (rule, None)
}

/// (PERM-2) Human-readable rule description (claude-code
/// `PermissionRuleDescription`): a Bash rule always describes itself; a
/// content-less rule for any other tool reads "Any use of the {tool} tool";
/// a content-bearing non-Bash rule has no extra description (`None`).
#[must_use]
pub fn rule_description(rule: &str) -> Option<String> {
    let (tool, content) = parse_rule(rule);
    match tool {
        "Bash" => Some(match content {
            Some(c) if c.ends_with(":*") => {
                format!("Any Bash command starting with {}", &c[..c.len() - 2])
            }
            Some(c) => format!("The Bash command {c}"),
            None => "Any Bash command".to_string(),
        }),
        _ => match content {
            None => Some(format!("Any use of the {tool} tool")),
            Some(_) => None,
        },
    }
}

/// The detail body for one rule (PERM-2): the rule value, its human-readable
/// description (when one applies), the behavior, and `From {source}`.
fn render_rule_detail(row: &PermRuleRow) -> String {
    let mut out = String::new();
    out.push_str(&row.rule);
    out.push('\n');
    if let Some(desc) = rule_description(&row.rule) {
        out.push_str(&desc);
        out.push('\n');
    }
    out.push_str(&format!("Behavior: {}\n", row.behavior));
    out.push_str(&format!("From {}", row.source));
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
        // Two rows in the default (Allow) tab so Down moves within the tab.
        let mut s = PermissionsScreenState {
            rows: vec![
                row("Allow", "Bash", "User"),
                row("Allow", "Read(./s/**)", "Project"),
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
    fn tabbed_list_filters_by_behavior_with_subtitle() {
        // (PERM-1/PERM-3) Allow tab shows only Allow rules (just the rule, since
        // the behavior IS the tab), the tab header marks the active tab, and the
        // per-tab subtitle appears.
        let s = PermissionsScreenState {
            mode: "acceptEdits".into(),
            rows: vec![
                row("Allow", "Bash", "User"),
                row("Deny", "Read(./s/**)", "Local"),
            ],
            selected: 0,
            dialog_mode: PermissionsDialogMode::List,
            tab: PermTab::Allow,
        };
        let out = render_permissions_to_string(&s);
        assert!(out.starts_with(
            "Permissions\nMode: acceptEdits\n[Allow]  Ask   Deny \nClaude Code won't ask before using allowed tools.\n\u{276F} Bash\n"
        ), "got: {out}");
        // The Deny rule is NOT in the Allow tab.
        assert!(!out.contains("Read(./s/**)"), "got: {out}");
        assert!(out.ends_with(
            "\u{2191}\u{2193} navigate \u{00B7} \u{21c6} tabs \u{00B7} Enter view \u{00B7} d delete \u{00B7} Esc close"
        ));
    }

    #[test]
    fn d_then_y_confirms_delete_of_selected_rule() {
        // (PERM-1) `d` → confirm prompt → `y` emits DeleteRule for the selected
        // (tab-filtered) rule; `n`/Esc cancels back to the list.
        let mut s = PermissionsScreenState {
            rows: vec![row("Allow", "Bash(npm test:*)", "Local")],
            ..PermissionsScreenState::default()
        };
        assert_eq!(handle_permissions_key(&mut s, KeyCode::Char('d')), PermissionsOutcome::Stay);
        assert_eq!(s.dialog_mode, PermissionsDialogMode::ConfirmDelete);
        assert!(render_permissions_to_string(&s).contains("Delete rule Bash(npm test:*)?"));
        match handle_permissions_key(&mut s, KeyCode::Char('y')) {
            PermissionsOutcome::DeleteRule(r) => assert_eq!(r.rule, "Bash(npm test:*)"),
            other => panic!("expected DeleteRule, got {other:?}"),
        }
        assert_eq!(s.dialog_mode, PermissionsDialogMode::List);

        // `n` cancels.
        let _ = handle_permissions_key(&mut s, KeyCode::Char('d'));
        assert_eq!(handle_permissions_key(&mut s, KeyCode::Char('n')), PermissionsOutcome::Stay);
        assert_eq!(s.dialog_mode, PermissionsDialogMode::List);
    }

    #[test]
    fn row_to_update_maps_deletable_sources_only() {
        // (PERM-1) Local/Project/User → persistable update; others → None.
        let local = row("Deny", "Bash(rm:*)", "Local");
        let u = row_to_permission_update(&local).expect("local is deletable");
        assert_eq!(u.destination, PermissionUpdateDestination::LocalSettings);
        assert!(row_to_permission_update(&row("Allow", "Read", "Policy")).is_none());
        assert!(row_to_permission_update(&row("Allow", "Read", "Session")).is_none());
    }

    #[test]
    fn tab_key_switches_behavior_tab() {
        let mut s = PermissionsScreenState {
            rows: vec![row("Allow", "Bash", "User"), row("Deny", "Edit", "Local")],
            ..PermissionsScreenState::default()
        };
        assert_eq!(s.tab, PermTab::Allow);
        let _ = handle_permissions_key(&mut s, KeyCode::Tab);
        assert_eq!(s.tab, PermTab::Ask);
        let _ = handle_permissions_key(&mut s, KeyCode::Tab);
        assert_eq!(s.tab, PermTab::Deny);
        // Deny tab shows the Edit rule.
        assert!(render_permissions_to_string(&s).contains("\u{276F} Edit"));
        let _ = handle_permissions_key(&mut s, KeyCode::BackTab);
        assert_eq!(s.tab, PermTab::Ask);
    }

    #[test]
    fn empty_tab_shows_locked_empty_state() {
        // Default tab (Allow) with no Allow rules.
        let out = render_permissions_to_string(&PermissionsScreenState {
            mode: "default".into(),
            rows: vec![row("Deny", "Bash", "User")],
            ..PermissionsScreenState::default()
        });
        assert!(out.contains("No allow rules configured."), "got: {out}");
    }

    #[test]
    fn detail_render_fields() {
        // (PERM-2) Read(content) → no description; `From {source}` not "Source:".
        let r = row("Deny", "Read(./secrets/**)", "Local");
        assert_eq!(
            render_rule_detail(&r),
            "Read(./secrets/**)\nBehavior: Deny\nFrom Local\nesc to go back"
        );
        // A content-less tool rule → "Any use of the {tool} tool" description.
        let r = row("Allow", "WebSearch", "Project");
        assert_eq!(
            render_rule_detail(&r),
            "WebSearch\nAny use of the WebSearch tool\nBehavior: Allow\nFrom Project\nesc to go back"
        );
    }

    #[test]
    fn rule_description_per_tool() {
        // (PERM-2) Bash variants.
        assert_eq!(
            rule_description("Bash(npm test:*)").as_deref(),
            Some("Any Bash command starting with npm test")
        );
        assert_eq!(
            rule_description("Bash(ls -la)").as_deref(),
            Some("The Bash command ls -la")
        );
        assert_eq!(rule_description("Bash").as_deref(), Some("Any Bash command"));
        // Other tool, no content → "Any use of the {tool} tool".
        assert_eq!(rule_description("Read").as_deref(), Some("Any use of the Read tool"));
        // Other tool WITH content → no description.
        assert_eq!(rule_description("Read(./s/**)"), None);
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
