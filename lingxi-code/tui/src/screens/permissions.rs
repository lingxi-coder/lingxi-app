//! `/permissions` viewer (claude-code `commands/permissions` → the
//! `PermissionRuleList` UI): a read-only list↔detail view of the configured
//! permission rules (behavior · rule · source) plus the active permission mode.
//! Pure reducer over a selected index + a dialog mode, mirroring `hooks.rs` /
//! `mcp.rs`. The rows are loaded OFF-DISK (like `skills.rs`) from the three
//! persistable settings tiers via [`load_permission_sections`].
//!
//! ## Data source
//! Reads the SAME three settings files the enforcement loader reads
//! (`engine-desktop`): `<lingxi_home>/settings.json` (user),
//! `<cwd>/.lingxi/settings.json` (project), `<cwd>/.lingxi/settings.local.json`
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
    /// (PERM-1) Typing a new rule to add to the active tab.
    AddInput,
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
    /// (PERM-1) Workspace directories (`permissions.additionalDirectories`) —
    /// not behavior rules; lists extra writable working directories.
    Workspace,
}

impl PermTab {
    /// Tab cycle order (Tab): Allow → Ask → Deny → Workspace → Allow.
    #[must_use]
    pub fn next(self) -> Self {
        match self {
            PermTab::Allow => PermTab::Ask,
            PermTab::Ask => PermTab::Deny,
            PermTab::Deny => PermTab::Workspace,
            PermTab::Workspace => PermTab::Allow,
        }
    }

    /// Reverse cycle (BackTab).
    #[must_use]
    pub fn prev(self) -> Self {
        match self {
            PermTab::Allow => PermTab::Workspace,
            PermTab::Ask => PermTab::Allow,
            PermTab::Deny => PermTab::Ask,
            PermTab::Workspace => PermTab::Deny,
        }
    }

    /// Title in the tab header.
    #[must_use]
    pub fn title(self) -> &'static str {
        match self {
            PermTab::Allow => "Allow",
            PermTab::Ask => "Ask",
            PermTab::Deny => "Deny",
            PermTab::Workspace => "Workspace",
        }
    }

    /// The `behavior` label rows in this tab carry. `None` for the Workspace
    /// tab, which lists directories rather than behavior rules.
    #[must_use]
    fn behavior(self) -> &'static str {
        match self {
            PermTab::Allow => "Allow",
            PermTab::Ask => "Ask",
            PermTab::Deny => "Deny",
            // Workspace has no behavior; `tab_rows` never filters by it.
            PermTab::Workspace => "Workspace",
        }
    }

    /// `true` for the directory-listing Workspace tab (vs. the rule tabs).
    #[must_use]
    pub fn is_workspace(self) -> bool {
        matches!(self, PermTab::Workspace)
    }

    /// Per-tab subtitle (claude-code `PermissionRuleList`).
    #[must_use]
    pub fn subtitle(self) -> &'static str {
        match self {
            PermTab::Allow => "LingXi won't ask before using allowed tools.",
            PermTab::Ask => "LingXi will always ask for confirmation before using these tools.",
            PermTab::Deny => "LingXi will always reject requests to use denied tools.",
            PermTab::Workspace => {
                "LingXi can read and write files in these directories without asking."
            }
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
    /// (PERM-1) Buffer for the new-rule text in [`PermissionsDialogMode::AddInput`].
    pub add_input: String,
    /// (PERM-1 Workspace tab) The configured `additionalDirectories`, shown in
    /// the Workspace tab. Loaded alongside the rule rows.
    pub workspace_dirs: Vec<String>,
}

impl PermissionsScreenState {
    /// The rows belonging to the active tab (filtered by behavior). Empty on
    /// the Workspace tab (which lists directories, not rule rows — use
    /// [`Self::workspace_dirs`]).
    #[must_use]
    pub fn tab_rows(&self) -> Vec<&PermRuleRow> {
        if self.tab.is_workspace() {
            return Vec::new();
        }
        self.rows
            .iter()
            .filter(|r| r.behavior == self.tab.behavior())
            .collect()
    }

    /// (PERM-1) Number of selectable items in the active tab: directory count
    /// on the Workspace tab, else the filtered rule-row count. Drives the
    /// shared Up/Down clamp + selection logic.
    #[must_use]
    pub fn active_item_count(&self) -> usize {
        if self.tab.is_workspace() {
            self.workspace_dirs.len()
        } else {
            self.tab_rows().len()
        }
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
    /// (PERM-1) The user submitted a new rule to add — the caller persists it
    /// (async) to Local settings + reloads. Carries the new rule (source
    /// `"Local"`, behavior = the active tab).
    AddRule(PermRuleRow),
    /// (PERM-1 Workspace tab) The user submitted a new workspace directory —
    /// the caller persists it to Local settings (`additionalDirectories`) +
    /// reloads. Carries the directory path.
    AddWorkspaceDir(String),
    /// (PERM-1 Workspace tab) The user confirmed removing a workspace
    /// directory — the caller persists the removal + reloads.
    RemoveWorkspaceDir(String),
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
    lingxi_home: &std::path::Path,
) -> PermissionsScreenState {
    let mut rows = Vec::new();
    let mut workspace_dirs: Vec<String> = Vec::new();
    // Default mode unless a tier overrides it; local read LAST wins (matching
    // the enforcement loader's ascending-priority order).
    let mut mode = "default".to_string();

    for (path, source) in [
        (
            lingxi_home.join("settings.json"),
            PermissionRuleSource::UserSettings,
        ),
        (
            cwd.join(branding::DOT_DIR).join("settings.json"),
            PermissionRuleSource::ProjectSettings,
        ),
        (
            cwd.join(branding::DOT_DIR).join("settings.local.json"),
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
        // (PERM-1 Workspace tab) accumulate `permissions.additionalDirectories`
        // across tiers (claude-code merges them ConcatDedup); de-dup by path.
        for dir in permission::additional_directories_from_settings_json(&raw) {
            let s = dir.to_string_lossy().into_owned();
            if !workspace_dirs.contains(&s) {
                workspace_dirs.push(s);
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
        add_input: String::new(),
        workspace_dirs,
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
                let n = state.active_item_count();
                if n > 0 {
                    state.selected = (state.selected + 1).min(n - 1);
                }
                PermissionsOutcome::Stay
            }
            // Enter views a rule's detail; the Workspace tab has no detail
            // view (directories carry no extra fields), so Enter is inert there.
            KeyCode::Enter => {
                if !state.tab.is_workspace() && !state.tab_rows().is_empty() {
                    state.dialog_mode = PermissionsDialogMode::Detail;
                }
                PermissionsOutcome::Stay
            }
            // (PERM-1) `d` asks to delete the selected rule / workspace dir.
            KeyCode::Char('d') if state.active_item_count() > 0 => {
                state.dialog_mode = PermissionsDialogMode::ConfirmDelete;
                PermissionsOutcome::Stay
            }
            // (PERM-1) `a` opens the new-rule / new-directory input.
            KeyCode::Char('a') => {
                state.add_input.clear();
                state.dialog_mode = PermissionsDialogMode::AddInput;
                PermissionsOutcome::Stay
            }
            KeyCode::Esc | KeyCode::Char('q') => PermissionsOutcome::Close,
            _ => PermissionsOutcome::Stay,
        },
        PermissionsDialogMode::AddInput => match key {
            KeyCode::Esc => {
                state.add_input.clear();
                state.dialog_mode = PermissionsDialogMode::List;
                PermissionsOutcome::Stay
            }
            KeyCode::Backspace => {
                state.add_input.pop();
                PermissionsOutcome::Stay
            }
            KeyCode::Enter => {
                let entry = state.add_input.trim().to_string();
                state.add_input.clear();
                state.dialog_mode = PermissionsDialogMode::List;
                if entry.is_empty() {
                    PermissionsOutcome::Stay
                } else if state.tab.is_workspace() {
                    // (PERM-1 Workspace tab) the typed value is a directory path.
                    PermissionsOutcome::AddWorkspaceDir(entry)
                } else {
                    PermissionsOutcome::AddRule(PermRuleRow {
                        behavior: state.tab.behavior().to_string(),
                        rule: entry,
                        source: "Local".to_string(),
                    })
                }
            }
            KeyCode::Char(c) => {
                state.add_input.push(c);
                PermissionsOutcome::Stay
            }
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
            // `y` confirms → emit the rule / workspace-dir to delete (caller
            // persists + reloads).
            KeyCode::Char('y' | 'Y') if state.tab.is_workspace() => {
                let dir = state.workspace_dirs.get(state.selected).cloned();
                state.dialog_mode = PermissionsDialogMode::List;
                match dir {
                    Some(d) => PermissionsOutcome::RemoveWorkspaceDir(d),
                    None => PermissionsOutcome::Stay,
                }
            }
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
                "Permissions\nMode: {}\n{} {} {} {}\n{}\n",
                state.mode,
                tab_mark(PermTab::Allow),
                tab_mark(PermTab::Ask),
                tab_mark(PermTab::Deny),
                tab_mark(PermTab::Workspace),
                state.tab.subtitle(),
            );
            if state.tab.is_workspace() {
                // (PERM-1 Workspace tab) list the configured directories.
                if state.workspace_dirs.is_empty() {
                    out.push_str("No workspace directories configured.\n");
                } else {
                    for (i, dir) in state.workspace_dirs.iter().enumerate() {
                        let marker = if i == state.selected {
                            "\u{276F} "
                        } else {
                            "  "
                        };
                        out.push_str(marker);
                        out.push_str(dir);
                        out.push('\n');
                    }
                }
                // Workspace rows have no detail view → no `Enter view` hint.
                out.push_str("\u{2191}\u{2193} navigate \u{00B7} \u{21c6} tabs \u{00B7} a add \u{00B7} d delete \u{00B7} Esc close");
                return out;
            }
            let rows = state.tab_rows();
            if rows.is_empty() {
                out.push_str(&format!(
                    "No {} rules configured.",
                    state.tab.title().to_lowercase()
                ));
                out.push('\n');
            } else {
                for (i, row) in rows.iter().enumerate() {
                    let marker = if i == state.selected {
                        "\u{276F} "
                    } else {
                        "  "
                    };
                    out.push_str(marker);
                    // The behavior is the active tab, so the row shows just the rule.
                    out.push_str(&row.rule);
                    out.push('\n');
                }
            }
            out.push_str("\u{2191}\u{2193} navigate \u{00B7} \u{21c6} tabs \u{00B7} Enter view \u{00B7} a add \u{00B7} d delete \u{00B7} Esc close");
            out
        }
        // (PERM-1) Delete confirmation for the selected rule / workspace dir.
        PermissionsDialogMode::ConfirmDelete if state.tab.is_workspace() => {
            match state.workspace_dirs.get(state.selected) {
                Some(dir) => {
                    format!("Remove workspace directory {dir}?\ny to remove \u{00B7} n to cancel")
                }
                None => "Permissions\n(directory no longer available)".to_string(),
            }
        }
        PermissionsDialogMode::ConfirmDelete => match state.tab_rows().get(state.selected).copied()
        {
            Some(row) => format!(
                "Delete rule {}?\ny to delete \u{00B7} n to cancel",
                row.rule
            ),
            None => "Permissions\n(rule no longer available)".to_string(),
        },
        // (PERM-1 Workspace tab) New-directory input.
        PermissionsDialogMode::AddInput if state.tab.is_workspace() => format!(
            "Add workspace directory:\n{}\u{2588}\nEnter add \u{00B7} Esc cancel",
            state.add_input,
        ),
        // (PERM-1) New-rule input for the active tab.
        PermissionsDialogMode::AddInput => format!(
            "Add {} rule:\n{}\u{2588}\nEnter add \u{00B7} Esc cancel",
            state.tab.title().to_lowercase(),
            state.add_input,
        ),
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
            ..PermissionsScreenState::default()
        };
        let out = render_permissions_to_string(&s);
        assert!(out.starts_with(
            "Permissions\nMode: acceptEdits\n[Allow]  Ask   Deny   Workspace \nLingXi won't ask before using allowed tools.\n\u{276F} Bash\n"
        ), "got: {out}");
        // The Deny rule is NOT in the Allow tab.
        assert!(!out.contains("Read(./s/**)"), "got: {out}");
        assert!(out.ends_with(
            "\u{2191}\u{2193} navigate \u{00B7} \u{21c6} tabs \u{00B7} Enter view \u{00B7} a add \u{00B7} d delete \u{00B7} Esc close"
        ));
    }

    #[test]
    fn a_then_type_then_enter_adds_rule_to_active_tab() {
        // (PERM-1) `a` opens the input; typing builds the rule; Enter emits
        // AddRule with the active tab's behavior + Local source.
        let mut s = PermissionsScreenState::default(); // Allow tab.
        assert_eq!(
            handle_permissions_key(&mut s, KeyCode::Char('a')),
            PermissionsOutcome::Stay
        );
        assert_eq!(s.dialog_mode, PermissionsDialogMode::AddInput);
        for c in "Bash(ls)".chars() {
            let _ = handle_permissions_key(&mut s, KeyCode::Char(c));
        }
        // Backspace edits the buffer.
        let _ = handle_permissions_key(&mut s, KeyCode::Backspace);
        assert_eq!(s.add_input, "Bash(ls");
        for c in ")".chars() {
            let _ = handle_permissions_key(&mut s, KeyCode::Char(c));
        }
        assert!(render_permissions_to_string(&s).contains("Add allow rule:"));
        match handle_permissions_key(&mut s, KeyCode::Enter) {
            PermissionsOutcome::AddRule(r) => {
                assert_eq!(r.rule, "Bash(ls)");
                assert_eq!(r.behavior, "Allow");
                assert_eq!(r.source, "Local");
            }
            other => panic!("expected AddRule, got {other:?}"),
        }
        assert_eq!(s.dialog_mode, PermissionsDialogMode::List);
        // Empty input + Enter is a no-op (Stay, back to list).
        let _ = handle_permissions_key(&mut s, KeyCode::Char('a'));
        assert_eq!(
            handle_permissions_key(&mut s, KeyCode::Enter),
            PermissionsOutcome::Stay
        );
        assert_eq!(s.dialog_mode, PermissionsDialogMode::List);
    }

    #[test]
    fn d_then_y_confirms_delete_of_selected_rule() {
        // (PERM-1) `d` → confirm prompt → `y` emits DeleteRule for the selected
        // (tab-filtered) rule; `n`/Esc cancels back to the list.
        let mut s = PermissionsScreenState {
            rows: vec![row("Allow", "Bash(npm test:*)", "Local")],
            ..PermissionsScreenState::default()
        };
        assert_eq!(
            handle_permissions_key(&mut s, KeyCode::Char('d')),
            PermissionsOutcome::Stay
        );
        assert_eq!(s.dialog_mode, PermissionsDialogMode::ConfirmDelete);
        assert!(render_permissions_to_string(&s).contains("Delete rule Bash(npm test:*)?"));
        match handle_permissions_key(&mut s, KeyCode::Char('y')) {
            PermissionsOutcome::DeleteRule(r) => assert_eq!(r.rule, "Bash(npm test:*)"),
            other => panic!("expected DeleteRule, got {other:?}"),
        }
        assert_eq!(s.dialog_mode, PermissionsDialogMode::List);

        // `n` cancels.
        let _ = handle_permissions_key(&mut s, KeyCode::Char('d'));
        assert_eq!(
            handle_permissions_key(&mut s, KeyCode::Char('n')),
            PermissionsOutcome::Stay
        );
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
        // (PERM-1) Deny → Workspace → Allow completes the 4-tab cycle.
        let _ = handle_permissions_key(&mut s, KeyCode::Tab);
        assert_eq!(s.tab, PermTab::Workspace);
        let _ = handle_permissions_key(&mut s, KeyCode::Tab);
        assert_eq!(s.tab, PermTab::Allow);
        // BackTab from Allow wraps to Workspace.
        let _ = handle_permissions_key(&mut s, KeyCode::BackTab);
        assert_eq!(s.tab, PermTab::Workspace);
    }

    #[test]
    fn workspace_tab_lists_directories_and_add_remove() {
        // (PERM-1 Workspace tab)
        let mut s = PermissionsScreenState {
            tab: PermTab::Workspace,
            workspace_dirs: vec!["/work/a".into(), "/work/b".into()],
            ..PermissionsScreenState::default()
        };
        let out = render_permissions_to_string(&s);
        assert!(out.contains("[Workspace]"), "4th tab in header: {out}");
        assert!(
            out.contains("\u{276F} /work/a\n  /work/b\n"),
            "directories listed with selection marker: {out}"
        );
        assert!(out.contains("Workspace"), "subtitle present");

        // `a` opens the directory input; typing + Enter emits AddWorkspaceDir.
        assert_eq!(
            handle_permissions_key(&mut s, KeyCode::Char('a')),
            PermissionsOutcome::Stay
        );
        assert_eq!(s.dialog_mode, PermissionsDialogMode::AddInput);
        assert!(render_permissions_to_string(&s).starts_with("Add workspace directory:\n"));
        for c in "/work/c".chars() {
            let _ = handle_permissions_key(&mut s, KeyCode::Char(c));
        }
        match handle_permissions_key(&mut s, KeyCode::Enter) {
            PermissionsOutcome::AddWorkspaceDir(d) => assert_eq!(d, "/work/c"),
            other => panic!("expected AddWorkspaceDir, got {other:?}"),
        }

        // `d` + `y` on the selected directory emits RemoveWorkspaceDir.
        s.selected = 1;
        assert_eq!(
            handle_permissions_key(&mut s, KeyCode::Char('d')),
            PermissionsOutcome::Stay
        );
        assert_eq!(s.dialog_mode, PermissionsDialogMode::ConfirmDelete);
        assert!(render_permissions_to_string(&s).contains("Remove workspace directory /work/b?"));
        match handle_permissions_key(&mut s, KeyCode::Char('y')) {
            PermissionsOutcome::RemoveWorkspaceDir(d) => assert_eq!(d, "/work/b"),
            other => panic!("expected RemoveWorkspaceDir, got {other:?}"),
        }
    }

    #[test]
    fn workspace_tab_empty_state_and_enter_is_inert() {
        // (PERM-1 Workspace tab) no dirs → locked empty state; Enter does not
        // open a detail view (directories have no detail).
        let mut s = PermissionsScreenState {
            tab: PermTab::Workspace,
            ..PermissionsScreenState::default()
        };
        assert!(render_permissions_to_string(&s).contains("No workspace directories configured."));
        let _ = handle_permissions_key(&mut s, KeyCode::Enter);
        assert_eq!(
            s.dialog_mode,
            PermissionsDialogMode::List,
            "Enter inert on Workspace"
        );
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
        assert_eq!(
            rule_description("Bash").as_deref(),
            Some("Any Bash command")
        );
        // Other tool, no content → "Any use of the {tool} tool".
        assert_eq!(
            rule_description("Read").as_deref(),
            Some("Any use of the Read tool")
        );
        // Other tool WITH content → no description.
        assert_eq!(rule_description("Read(./s/**)"), None);
    }

    #[test]
    fn load_reads_tiers_and_mode() {
        let tmp = std::env::temp_dir().join(format!("lx-perm-view-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let lingxi_home = tmp.join("home/.lingxi");
        let cwd = tmp.join("proj");
        std::fs::create_dir_all(&lingxi_home).unwrap();
        std::fs::create_dir_all(cwd.join(".lingxi")).unwrap();
        std::fs::write(
            lingxi_home.join("settings.json"),
            r#"{ "permissions": { "allow": ["Bash"] } }"#,
        )
        .unwrap();
        std::fs::write(
            cwd.join(".lingxi").join("settings.local.json"),
            r#"{ "permissions": { "deny": ["Read(./secrets/**)"], "defaultMode": "acceptEdits" } }"#,
        )
        .unwrap();

        let st = load_permission_sections(&cwd, &lingxi_home);
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
        let st = load_permission_sections(&tmp.join("proj"), &tmp.join("home/.lingxi"));
        assert_eq!(st.mode, "default");
        assert!(st.rows.is_empty());
    }
}
