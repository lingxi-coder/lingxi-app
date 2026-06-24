//! `ToolUseConfirm` dialog — generic per-tool permission prompt (M6-05).
//!
//! Renders the LingXi-locked three-button layout:
//! ```text
//! Claude needs your permission to use {tool}
//! Input: {pretty}
//!
//! > [1] Allow Once
//!   [2] Allow Always
//!   [N] Deny
//! ```
//!
//! Bindings:
//! - `1` → `AllowOnce`
//! - `2` → `AllowAlways`
//! - `n` / `N` / `Esc` → `Deny`
//! - `Enter` → resolve on the currently focused button
//! - `Down` / `Tab` → step focus forward
//! - `Up` / `BackTab` → step focus backward
#![forbid(unsafe_code)]

use crossterm::event::{KeyCode, KeyEvent};
use iocraft::prelude::*;

use super::{DialogFocus, DialogResolution};
use crate::theme::TuiTheme;

/// Mutable state carried across renders for this dialog.
#[derive(Debug, Clone, Default)]
pub struct ToolUseConfirmState {
    /// Which button is currently highlighted.
    pub focus: DialogFocus,
}

/// Props for [`ToolUseConfirm`].
#[derive(Default, Props)]
pub struct ToolUseConfirmProps {
    /// Tool name (mapped through `user_facing_name` for display).
    pub tool_name: String,
    /// Raw tool input — rendered through `render_tool_use_message` for the
    /// human tool-use preview body (claude-code `renderToolUseMessage`).
    pub tool_input: serde_json::Value,
    /// Session cwd — drives `getDisplayPath` path-shortening in the preview and
    /// the `… in {cwd}` always-allow label.
    pub cwd: std::path::PathBuf,
    /// Which button to highlight on this render.
    pub focus: DialogFocus,
    /// (perm-09) Worker name, when worker-originated — rendered as a dim
    /// `· @name` suffix on the title row (claude-code
    /// `PermissionRequestTitle`'s `workerBadge`), not a separate line. `None`
    /// (the live default) leaves the dialog byte-identical to before.
    pub worker_name: Option<String>,
    /// (perm-05) Active palette — drives the top-border + title accent
    /// (claude-code `PermissionDialog`'s default `color="permission"`).
    pub theme: crate::theme::Theme,
}

/// Pure key handler. Returns `Some(resolution)` when the user picks an
/// option, `None` otherwise (focus moved, or ignored key).
#[must_use]
pub fn handle_key(state: &mut ToolUseConfirmState, key: KeyEvent) -> Option<DialogResolution> {
    match key.code {
        KeyCode::Char('1') => Some(DialogResolution::allow_once()),
        KeyCode::Char('2') => Some(DialogResolution::allow_always()),
        KeyCode::Char('n' | 'N') | KeyCode::Esc => Some(DialogResolution::deny()),
        KeyCode::Enter => Some(state.focus.resolve()),
        KeyCode::Down | KeyCode::Tab => {
            state.focus = state.focus.next();
            None
        }
        KeyCode::Up | KeyCode::BackTab => {
            state.focus = state.focus.prev();
            None
        }
        _ => None,
    }
}

/// iocraft component rendering the dialog frame + 3 buttons.
///
/// (perm-03) Titled `Tool use` dialog: the rendered tool-use message + a
/// `Do you want to proceed?` question — NOT "Claude needs your permission to
/// use {tool}" + "Input: {json}". (perm-01) Options are `Yes` / `Yes, and don't
/// ask again for {tool} commands in {cwd}` / `No` (no `[1]`/`[2]`/`[N]`
/// numbered prefixes; the 1/2/n keys remain as hidden accelerators). (perm-05)
/// Top-only round border colored `theme.permission` (claude-code
/// `PermissionDialog`'s `borderLeft/Right/Bottom=false`), not a full round
/// box. (perm-09) The worker name renders as a dim `· @name` suffix on the
/// title row (claude-code `PermissionRequestTitle`), not a separate line.
#[component]
pub fn ToolUseConfirm(props: &ToolUseConfirmProps) -> impl Into<AnyElement<'static>> {
    use crate::components::messages::assistant_tool_use::{
        render_tool_use_message, user_facing_name,
    };
    use crate::components::messages::user_tool_result::{
        is_diff_tool, render_edit_write_diff_lines,
    };
    let name = user_facing_name(&props.tool_name).to_string();
    // The tool-use preview body, e.g. `Read(src/x.rs)` / `Bash(npm test)`.
    let body = match render_tool_use_message(&props.tool_name, &props.tool_input, &props.cwd) {
        Some(s) if s.is_empty() => name.clone(),
        Some(s) => format!("{name}({s})"),
        None => format!("{name}({})", props.tool_input),
    };
    // (perm-02) File-edit tools render the structured diff inside the dialog so
    // the user sees the exact change before approving (claude-code's per-tool
    // permission confirmation shows the FileEdit diff, not just the tool name).
    let diff_rows: Vec<AnyElement<'static>> = if is_diff_tool(&props.tool_name) {
        let inp = &props.tool_input;
        let s = |k: &str| inp.get(k).and_then(serde_json::Value::as_str);
        let path = s("file_path");
        let (old, new) = if props.tool_name == "Write" {
            (None, s("content"))
        } else {
            (s("old_string"), s("new_string"))
        };
        if old.is_some() || new.is_some() {
            render_edit_write_diff_lines(
                &props.tool_name,
                old,
                new,
                path,
                crate::theme::ThemeName::Dark,
            )
            .into_iter()
            .map(|line| {
                let spans: Vec<AnyElement<'static>> = line
                    .spans
                    .into_iter()
                    .map(|sp| {
                        let color = sp.style.fg.to_iocraft();
                        let bg = sp.style.bg.to_iocraft();
                        let weight = if sp.style.bold {
                            Weight::Bold
                        } else {
                            Weight::Normal
                        };
                        element! {
                            View(background_color: bg) {
                                Text(content: sp.text, color: color, weight: weight)
                            }
                        }
                        .into_any()
                    })
                    .collect();
                element! { View(flex_direction: FlexDirection::Row) { #(spans) } }.into_any()
            })
            .collect()
        } else {
            Vec::new()
        }
    } else {
        Vec::new()
    };
    let cwd_disp = props.cwd.display().to_string();
    let focus = props.focus;
    let worker_name = props.worker_name.clone();
    let accent = props.theme.permission;
    let button_label = move |for_focus: DialogFocus, label: String| -> String {
        if for_focus == focus {
            format!("> {label}")
        } else {
            format!("  {label}")
        }
    };
    let allow_once = button_label(DialogFocus::AllowOnce, "Yes".to_string());
    let allow_always = button_label(
        DialogFocus::AllowAlways,
        format!("Yes, and don't ask again for {name} commands in {cwd_disp}"),
    );
    let deny = button_label(DialogFocus::Deny, "No".to_string());
    element! {
        View(
            flex_direction: FlexDirection::Column,
            border_style: BorderStyle::Round,
            border_color: accent,
            border_edges: Edges::Top,
            padding_left: 1,
            padding_right: 1,
        ) {
            View(flex_direction: FlexDirection::Row, gap: 1) {
                Text(content: "Tool use".to_string(), weight: Weight::Bold, color: accent)
                #(worker_name.as_deref().map(|n| element! {
                    Text(content: format!("\u{00B7} @{n}"), color: TuiTheme::DIM)
                }))
            }
            Text(content: body)
            #(diff_rows)
            Text(content: "Do you want to proceed?".to_string())
            View(flex_direction: FlexDirection::Column, padding_top: 1) {
                Text(content: allow_once)
                Text(content: allow_always)
                Text(content: deny)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use permission::gate::PermissionResponse;

    fn k(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn key_1_returns_allow_once() {
        let mut state = ToolUseConfirmState::default();
        let res = handle_key(&mut state, k(KeyCode::Char('1')));
        assert_eq!(res.unwrap().response, PermissionResponse::AllowOnce);
    }

    #[test]
    fn key_2_returns_allow_always() {
        let mut state = ToolUseConfirmState::default();
        let res = handle_key(&mut state, k(KeyCode::Char('2')));
        let r = res.unwrap();
        assert_eq!(r.response, PermissionResponse::AllowAlways);
        assert!(r.persist);
    }

    #[test]
    fn key_lowercase_n_returns_deny() {
        let mut state = ToolUseConfirmState::default();
        let res = handle_key(&mut state, k(KeyCode::Char('n')));
        assert_eq!(res.unwrap().response, PermissionResponse::Deny);
    }

    #[test]
    fn key_uppercase_n_returns_deny() {
        let mut state = ToolUseConfirmState::default();
        let res = handle_key(&mut state, k(KeyCode::Char('N')));
        assert_eq!(res.unwrap().response, PermissionResponse::Deny);
    }

    #[test]
    fn key_esc_returns_deny() {
        let mut state = ToolUseConfirmState::default();
        let res = handle_key(&mut state, k(KeyCode::Esc));
        assert_eq!(res.unwrap().response, PermissionResponse::Deny);
    }

    #[test]
    fn key_enter_resolves_on_highlighted_button() {
        let mut state = ToolUseConfirmState::default();
        // Default focus = AllowOnce.
        let res = handle_key(&mut state, k(KeyCode::Enter));
        assert_eq!(res.unwrap().response, PermissionResponse::AllowOnce);
    }

    #[test]
    fn key_down_arrow_advances_focus_no_resolution() {
        let mut state = ToolUseConfirmState::default();
        assert_eq!(state.focus, DialogFocus::AllowOnce);
        let res = handle_key(&mut state, k(KeyCode::Down));
        assert!(res.is_none());
        assert_eq!(state.focus, DialogFocus::AllowAlways);
    }

    #[test]
    fn key_text_does_not_resolve() {
        let mut state = ToolUseConfirmState::default();
        let res = handle_key(&mut state, k(KeyCode::Char('x')));
        assert!(res.is_none());
    }
}
