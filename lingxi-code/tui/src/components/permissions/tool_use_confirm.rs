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
    /// (M9-07) Worker badge `● @name` prepended when worker-originated.
    /// `None` (the live default) leaves the dialog byte-identical to before.
    pub worker_badge: Option<String>,
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
/// numbered prefixes; the 1/2/n keys remain as hidden accelerators).
#[component]
pub fn ToolUseConfirm(props: &ToolUseConfirmProps) -> impl Into<AnyElement<'static>> {
    use crate::components::messages::assistant_tool_use::{
        render_tool_use_message, user_facing_name,
    };
    let name = user_facing_name(&props.tool_name).to_string();
    // The tool-use preview body, e.g. `Read(src/x.rs)` / `Bash(npm test)`.
    let body = match render_tool_use_message(&props.tool_name, &props.tool_input, &props.cwd) {
        Some(s) if s.is_empty() => name.clone(),
        Some(s) => format!("{name}({s})"),
        None => format!("{name}({})", props.tool_input),
    };
    let cwd_disp = props.cwd.display().to_string();
    let focus = props.focus;
    let worker_badge = props.worker_badge.clone();
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
            padding: 1,
        ) {
            #(worker_badge.as_deref().map(|badge| element! {
                Text(content: badge.to_string())
            }))
            Text(content: "Tool use".to_string(), weight: Weight::Bold)
            Text(content: body)
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
