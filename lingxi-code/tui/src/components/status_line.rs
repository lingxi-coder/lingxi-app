//! `StatusLine` — the 1-row top zone.
//!
//! Field order (left → right, space-separated per byte-lock L1):
//!     model  cwd  $cost  ctx%  mode
//!
//! Locked literals:
//!   L1 separator = " "
//!   L2 zero-cost = "$0.0000" (M6-06 — claude-code 4-decimal parity)
//!   L3 context% = "{:.0}%"

use std::path::PathBuf;

use iocraft::prelude::*;
use lingxi_permission::PermissionMode;

use crate::theme::Theme;

/// Props for `StatusLine`.
#[derive(Props)]
pub struct StatusLineProps {
    /// Display name of the active model.
    pub model: String,
    /// Working directory for the session.
    pub cwd: PathBuf,
    /// Pre-formatted cost string (e.g. `"$0.0000"`).
    pub cost: String,
    /// Context window utilisation in `[0.0, 1.0]`.
    pub context_pct: f32,
    /// Active permission mode.
    pub permission_mode: PermissionMode,
    /// (M7-15) Active palette — the status line text color reads from
    /// `theme.text`, so it recolors with the picker.
    pub theme: Theme,
}

impl Default for StatusLineProps {
    fn default() -> Self {
        Self {
            model: String::new(),
            cwd: PathBuf::from("."),
            cost: "$0.0000".to_string(),
            context_pct: 0.0,
            permission_mode: PermissionMode::Default,
            theme: Theme::dark(),
        }
    }
}

/// Map a `PermissionMode` to its short status-line label.
#[must_use]
pub fn mode_label(m: PermissionMode) -> &'static str {
    match m {
        PermissionMode::Default => "default",
        PermissionMode::Plan => "plan",
        PermissionMode::AcceptEdits => "acceptEdits",
        PermissionMode::BypassPermissions => "bypassPermissions",
        PermissionMode::DontAsk => "dontAsk",
        PermissionMode::Bubble => "bubble",
        PermissionMode::Auto => "auto",
    }
}

/// Format the full status line per the M6-02 byte-locks.
#[must_use]
pub fn format_status_line(
    model: &str,
    cwd: &std::path::Path,
    cost: &str,
    context_pct: f32,
    mode: PermissionMode,
) -> String {
    format!(
        "{} {} {} {:.0}% {}",
        model,
        cwd.display(),
        cost,
        context_pct * 100.0,
        mode_label(mode),
    )
}

/// Status-line component.
#[component]
pub fn StatusLine(props: &StatusLineProps) -> impl Into<AnyElement<'static>> {
    let line = format_status_line(
        &props.model,
        &props.cwd,
        &props.cost,
        props.context_pct,
        props.permission_mode,
    );
    element! {
        View(flex_direction: FlexDirection::Row, height: 1) {
            Text(content: line, color: props.theme.text)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn format_matches_byte_locks() {
        let s = format_status_line(
            "claude-sonnet-4.5",
            &PathBuf::from("/a/b"),
            "$0.0000",
            0.42,
            PermissionMode::Default,
        );
        assert_eq!(s, "claude-sonnet-4.5 /a/b $0.0000 42% default");
    }

    #[test]
    fn status_line_props_carry_theme() {
        // (M7-15) StatusLineProps gains a `theme` field; default is dark.
        let props = StatusLineProps::default();
        assert_eq!(props.theme, Theme::dark());
    }

    #[test]
    fn mode_label_covers_all_variants() {
        assert_eq!(mode_label(PermissionMode::Default), "default");
        assert_eq!(mode_label(PermissionMode::Plan), "plan");
        assert_eq!(mode_label(PermissionMode::AcceptEdits), "acceptEdits");
        assert_eq!(
            mode_label(PermissionMode::BypassPermissions),
            "bypassPermissions"
        );
        assert_eq!(mode_label(PermissionMode::DontAsk), "dontAsk");
    }
}
