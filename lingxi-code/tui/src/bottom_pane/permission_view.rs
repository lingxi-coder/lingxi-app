//! The permission prompt view: a bottom-pane view over a
//! [`PermissionExchange`], owning the one-shot response channel (plan
//! Phases 4 + 10; ported from the former `RataApp::open_permission` +
//! `PendingPermission`, with the per-variant dialogs ported from the iocraft
//! `components::permissions::{tool_use_confirm, exit_plan_mode,
//! bypass_permissions}` modules).
//!
//! One view covers all three [`PermissionRequest`] variants:
//! - `ToolUseConfirm` — a 3-option select dialog (allow once / allow always /
//!   deny) over the tool name + input preview.
//! - `ExitPlanMode` — the plan-approval select dialog ("Ready to code?" with
//!   the claude-code option grammar) over the multi-line plan body.
//! - `BypassPermissionsMode` — the typed-`yes` confirmation (the locked
//!   `LingXi` friction step: Enter resolves `AllowOnce` only after typing `yes`).
//!
//! While on the view stack it owns the keyboard: `Enter`/`1`–`3` resolve the
//! select dialogs with the highlighted/numbered option, `n`/`N`/`Esc` deny.
//! The response sender is consumed by the FIRST resolution (single-shot
//! guarantee — the regression net lives in
//! `app.rs`:`permission_resolution_is_single_shot_and_releases_keyboard`);
//! dropping the view unresolved closes the channel, which the permission gate
//! maps to a deny.

use std::any::Any;

use crossterm::cursor::SetCursorStyle;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use permission::gate::{PermissionRequest, PermissionResponse};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Widget, Wrap};
use tokio::sync::oneshot;
use tui_core::permission_bridge::PermissionExchange;

use crate::bottom_pane::dialog_view::{centered_rect, DialogOutcome, DialogView};
use crate::bottom_pane::view::{BottomPaneView, ViewOutcome};
use crate::renderable::Renderable;

/// The bottom-viewport rows the `ToolUseConfirm` prompt claims. Locked layout
/// value (80x24 behavior lock: permission viewport = 9); the dialog clips its
/// footer chrome to it, exactly as the pre-view-stack `viewport_height` did.
const VIEWPORT_HEIGHT: u16 = 9;

/// Plan body lines shown in the `ExitPlanMode` dialog before eliding (the
/// bottom viewport clamps at 20 rows, and clipping the dialog's BOTTOM would
/// hide the option list — so long plans elide instead).
const MAX_PLAN_LINES: usize = 6;

/// `BypassPermissionsMode` literals — byte-locked from the iocraft
/// `bypass_permissions` dialog (itself locked from claude-code's
/// `BypassPermissionsModeDialog.tsx`).
const BYPASS_TITLE: &str = "WARNING: LingXi running in Bypass Permissions mode";
const BYPASS_BODY_1: &str = "In Bypass Permissions mode, LingXi will not ask for your approval before running potentially dangerous commands.\nThis mode should only be used in a sandboxed container/VM that has restricted internet access and can easily be restored if damaged.";
const BYPASS_BODY_2: &str = "By proceeding, you accept all responsibility for actions taken while running in Bypass Permissions mode.";

/// The variant-specific interaction model behind the shared response channel.
enum Prompt {
    /// `ToolUseConfirm` / `ExitPlanMode`: a select dialog whose tool option
    /// order is allow-once / (allow-always or auto) / deny. The persistent
    /// option is omitted when the request carries the suppression flag.
    Select {
        dialog: DialogView,
        /// `Some(rows)` pins the locked `ToolUseConfirm` viewport height;
        /// `None` sizes to the dialog content (plan approval).
        fixed_height: Option<u16>,
        /// `false` ⇒ the dialog was built with 2 rows (allow-once / deny),
        /// so a `Selected(1)` outcome means Deny, not AllowAlways.
        always_allow_offered: bool,
        /// Engine-computed optional Auto action; clients must not infer it.
        auto_mode_prompt: Option<permission::gate::AutoModePrompt>,
    },
    /// `BypassPermissionsMode`: type `yes` + Enter to enable (`AllowOnce`);
    /// `Esc`/`n` deny. The buffer keeps typos visible for Backspace editing.
    TypedConfirm {
        /// Letters typed so far (lowercased). `"yes"` arms Enter.
        typed: String,
    },
}

/// A permission request awaiting the user's decision.
pub struct PermissionView {
    prompt: Prompt,
    /// One-shot response sender, consumed by the first resolution.
    resp_tx: Option<oneshot::Sender<PermissionResponse>>,
}

impl PermissionView {
    /// Build the prompt for `exchange` (dialog shape mirrors the request
    /// variant; the response channel is taken from the exchange).
    #[must_use]
    pub fn new(exchange: PermissionExchange) -> Self {
        let who = exchange
            .worker
            .as_ref()
            .map_or_else(|| "The assistant".to_string(), |w| format!("@{}", w.name));
        let prompt = match &exchange.request {
            PermissionRequest::ToolUseConfirm {
                tool_name,
                tool_input,
                suppress_always_allow_rule: request_suppression,
                ..
            } => {
                let suppress_always_allow_rule =
                    exchange.suppress_always_allow_rule || *request_suppression;
                let auto_mode_prompt = exchange.auto_mode_prompt;
                let mut input = tool_input.to_string();
                if input.chars().count() > 68 {
                    input = format!("{}…", input.chars().take(67).collect::<String>());
                }
                let always_allow_offered = !suppress_always_allow_rule;
                let deny_row = format!(
                    "No, and tell {} what to do differently (esc)",
                    branding::PRODUCT_NAME
                );
                // PERM-02 (claude-code 2.1.238 @302829842). The oracle
                // builds this row set as
                //   [{label:"Yes",value:"yes"},
                //    ...(persistRow ? [{label:<composed>,value:"yes-dont-ask-again"}] : []),
                //    {label:"No, and tell Claude what to do differently (esc)",value:"no"}]
                // Rows 1 and 3 are fixed strings and are now byte-exact
                // (with the LingXi rebrand in row 3).
                //
                // Row 2 is NOT fixed upstream: the oracle composes it
                // from the permission result's `suggestions`
                // (`ma0` @302931138 renders "Yes, and don't ask again
                // for " + the bolded rule display) and OMITS the row
                // entirely when no rule can be derived. The port has
                // `permission_suggestions` as a field but no engine
                // source for it (hooks/src/hook_payload.rs:639,
                // executor.rs:2184 pass None), so the rule display
                // cannot be composed here yet and the row stays
                // unconditional with its own wording. Closing that gap
                // is the suggestions engine, not a copy fix.
                //
                // §27b: the oracle ALSO omits row 2 outright when the tool's
                // `suppressesAlwaysAllowRule()` is true (@182520462/@172369864
                // `showAlwaysAllow:...&&e.tool.suppressesAlwaysAllowRule?.(e.input)!==!0&&...`)
                // — a persisted rule would be written but then ignored by a
                // tool that needs fresh interaction on every call.
                let rows = if !always_allow_offered {
                    vec!["Yes".to_string(), deny_row]
                } else if let Some(auto_mode_prompt) = auto_mode_prompt {
                    vec![
                        "Yes".to_string(),
                        auto_mode_prompt.label().to_string(),
                        deny_row,
                    ]
                } else {
                    vec!["Yes".to_string(), "Yes, allow always".to_string(), deny_row]
                };
                Prompt::Select {
                    dialog: DialogView::new(
                        "Permission required",
                        vec![format!("{who} wants to use {tool_name}:"), input],
                        rows,
                    ),
                    fixed_height: Some(VIEWPORT_HEIGHT),
                    always_allow_offered,
                    auto_mode_prompt,
                }
            }
            PermissionRequest::ExitPlanMode { plan } => {
                let auto_mode_prompt = exchange.auto_mode_prompt;
                let suppress_always_allow_rule = exchange.suppress_always_allow_rule;
                let mut body = Vec::new();
                if exchange.worker.is_some() {
                    body.push(format!("{who} wants to exit plan mode:"));
                }
                let lines: Vec<&str> = plan.lines().collect();
                for line in lines.iter().take(MAX_PLAN_LINES) {
                    body.push((*line).to_string());
                }
                if lines.len() > MAX_PLAN_LINES {
                    body.push(format!(
                        "… (+{} more plan lines)",
                        lines.len() - MAX_PLAN_LINES
                    ));
                }
                let rows = if suppress_always_allow_rule {
                    vec![
                        "Yes, manually approve edits".to_string(),
                        "No, keep planning".to_string(),
                    ]
                } else {
                    vec![
                        "Yes, manually approve edits".to_string(),
                        auto_mode_prompt.map_or_else(
                            || "Yes, auto-accept edits".to_string(),
                            |prompt| prompt.label().to_string(),
                        ),
                        "No, keep planning".to_string(),
                    ]
                };
                Prompt::Select {
                    // claude-code `ExitPlanMode` grammar: AllowOnce proceeds
                    // with manual edit approval, AllowAlways auto-accepts
                    // edits, Deny stays in plan mode.
                    dialog: DialogView::new("Ready to code?", body, rows),
                    fixed_height: None,
                    always_allow_offered: !suppress_always_allow_rule,
                    auto_mode_prompt,
                }
            }
            PermissionRequest::BypassPermissionsMode => Prompt::TypedConfirm {
                typed: String::new(),
            },
        };
        Self {
            prompt,
            resp_tx: Some(exchange.resp_tx),
        }
    }

    /// Deliver `response` through the one-shot channel (first resolution only)
    /// and report it as the view's outcome.
    fn resolve(&mut self, response: PermissionResponse) -> ViewOutcome {
        if let Some(tx) = self.resp_tx.take() {
            let _ = tx.send(response);
        }
        ViewOutcome::PermissionResponse(response)
    }

    /// The typed-confirmation dialog's body lines (prompt line last, showing
    /// the live buffer).
    fn bypass_lines(typed: &str) -> Vec<Line<'static>> {
        let mut lines: Vec<Line<'static>> = BYPASS_BODY_1
            .lines()
            .chain(std::iter::once(BYPASS_BODY_2))
            .map(|s| Line::from(s.to_string()))
            .collect();
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            format!("Type \"yes\" + Enter to enable, Esc to cancel: {typed}"),
            Style::default().add_modifier(Modifier::BOLD),
        )));
        lines
    }

    /// The typed-confirmation modal's total height at `width` pane columns:
    /// the wrapped body rows plus the border chrome.
    fn bypass_height(typed: &str, width: u16) -> u16 {
        let modal_width = width.saturating_sub(4).max(20);
        let inner_width = modal_width.saturating_sub(2).max(1);
        let body = Paragraph::new(Self::bypass_lines(typed))
            .wrap(Wrap { trim: false })
            .line_count(inner_width);
        u16::try_from(body).unwrap_or(u16::MAX).saturating_add(2)
    }
}

impl Renderable for PermissionView {
    fn render(&self, area: Rect, buf: &mut Buffer) {
        match &self.prompt {
            Prompt::Select { dialog, .. } => dialog.render(area, buf),
            Prompt::TypedConfirm { typed } => {
                let width = area.width.saturating_sub(4).max(20).min(area.width);
                let height = Self::bypass_height(typed, area.width).min(area.height);
                let rect = centered_rect(width, height, area);
                Clear.render(rect, buf);
                let block = Block::new().borders(Borders::ALL).title(BYPASS_TITLE);
                let inner = block.inner(rect);
                block.render(rect, buf);
                Paragraph::new(Self::bypass_lines(typed))
                    .wrap(Wrap { trim: false })
                    .render(inner, buf);
            }
        }
    }

    fn desired_height(&self, width: u16) -> u16 {
        match &self.prompt {
            Prompt::Select {
                fixed_height: Some(rows),
                ..
            } => *rows,
            Prompt::Select { dialog, .. } => dialog.desired_height(width).max(VIEWPORT_HEIGHT),
            Prompt::TypedConfirm { typed } => Self::bypass_height(typed, width),
        }
    }

    fn cursor_pos(&self, area: Rect) -> Option<(u16, u16)> {
        match &self.prompt {
            Prompt::Select { dialog, .. } => dialog.cursor_pos(area),
            Prompt::TypedConfirm { .. } => None,
        }
    }

    fn cursor_style(&self, area: Rect) -> SetCursorStyle {
        match &self.prompt {
            Prompt::Select { dialog, .. } => dialog.cursor_style(area),
            Prompt::TypedConfirm { .. } => SetCursorStyle::DefaultUserShape,
        }
    }
}

impl BottomPaneView for PermissionView {
    fn handle_key(&mut self, key: KeyEvent) -> ViewOutcome {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let response = match &mut self.prompt {
            Prompt::Select {
                dialog,
                always_allow_offered,
                auto_mode_prompt,
                ..
            } => {
                // `n`/`N` denies (iocraft dialog parity); Ctrl chords stay
                // swallowed by the view (locked keyboard-ownership behavior).
                if matches!(key.code, KeyCode::Char('n' | 'N')) && !ctrl {
                    Some(PermissionResponse::Deny)
                } else {
                    match dialog.on_key(key.code) {
                        DialogOutcome::Pending => None,
                        DialogOutcome::Selected(idx) => Some(match idx {
                            0 => PermissionResponse::AllowOnce,
                            // §27b: row 1 is "Yes, allow always" only when
                            // `always_allow_offered` and no Auto action is
                            // present. A suppressed dialog has only 2 rows,
                            // so idx 1 there is the deny row.
                            1 if *always_allow_offered && auto_mode_prompt.is_some() => {
                                PermissionResponse::AllowAuto
                            }
                            1 if *always_allow_offered => PermissionResponse::AllowAlways,
                            _ => PermissionResponse::Deny,
                        }),
                        // Esc denies (a resolution, not a silent dismissal):
                        // the waiting tool call must always receive an answer.
                        DialogOutcome::Cancelled => Some(PermissionResponse::Deny),
                    }
                }
            }
            Prompt::TypedConfirm { typed } => match key.code {
                KeyCode::Esc => Some(PermissionResponse::Deny),
                KeyCode::Char('n' | 'N') if !ctrl => Some(PermissionResponse::Deny),
                // Enter resolves only once the buffer reads `yes` — the
                // deliberate friction step for dangerous mode.
                KeyCode::Enter if typed == "yes" => Some(PermissionResponse::AllowOnce),
                KeyCode::Backspace => {
                    typed.pop();
                    None
                }
                // Letters buffer (lowercased) so typos stay visible and
                // Backspace-editable; Ctrl chords and non-letters are ignored.
                KeyCode::Char(c) if c.is_ascii_alphabetic() && !ctrl => {
                    typed.push(c.to_ascii_lowercase());
                    None
                }
                _ => None,
            },
        };
        response.map_or(ViewOutcome::Pending, |response| self.resolve(response))
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use crossterm::event::{KeyCode, KeyModifiers};
    use ratatui::layout::Position;

    use super::*;

    fn exchange_for(
        request: PermissionRequest,
    ) -> (PermissionExchange, oneshot::Receiver<PermissionResponse>) {
        let (resp_tx, resp_rx) = oneshot::channel();
        (
            PermissionExchange {
                request,
                resp_tx,
                worker: None,
                suppress_always_allow_rule: false,
                auto_mode_prompt: None,
            },
            resp_rx,
        )
    }

    fn tool_exchange() -> (PermissionExchange, oneshot::Receiver<PermissionResponse>) {
        exchange_for(PermissionRequest::ToolUseConfirm {
            tool_name: "Bash".to_string(),
            tool_input: serde_json::json!({ "command": "ls -la" }),
            default_decision: permission::gate::PromptDefault::DenyByDefault,
            suppress_always_allow_rule: false,
        })
    }

    /// §27b: a request from a tool marked `requiresUserInteraction` — the
    /// dialog must omit "Yes, allow always".
    fn suppressed_tool_exchange() -> (PermissionExchange, oneshot::Receiver<PermissionResponse>) {
        let (mut exchange, resp_rx) = exchange_for(PermissionRequest::ToolUseConfirm {
            tool_name: "mcp__server__tool".to_string(),
            tool_input: serde_json::json!({}),
            default_decision: permission::gate::PromptDefault::DenyByDefault,
            suppress_always_allow_rule: true,
        });
        exchange.suppress_always_allow_rule = true;
        (exchange, resp_rx)
    }

    fn auto_tool_exchange() -> (PermissionExchange, oneshot::Receiver<PermissionResponse>) {
        let (mut exchange, resp_rx) = tool_exchange();
        exchange.auto_mode_prompt = Some(permission::gate::AutoModePrompt::WorkflowBash);
        (exchange, resp_rx)
    }

    fn auto_plan_exchange(
        plan: &str,
    ) -> (PermissionExchange, oneshot::Receiver<PermissionResponse>) {
        let (resp_tx, resp_rx) = oneshot::channel();
        (
            PermissionExchange {
                request: PermissionRequest::ExitPlanMode {
                    plan: plan.to_string(),
                },
                resp_tx,
                worker: None,
                suppress_always_allow_rule: false,
                auto_mode_prompt: Some(permission::gate::AutoModePrompt::ExitPlanMode),
            },
            resp_rx,
        )
    }

    fn plan_exchange(plan: &str) -> (PermissionExchange, oneshot::Receiver<PermissionResponse>) {
        exchange_for(PermissionRequest::ExitPlanMode {
            plan: plan.to_string(),
        })
    }

    fn bypass_exchange() -> (PermissionExchange, oneshot::Receiver<PermissionResponse>) {
        exchange_for(PermissionRequest::BypassPermissionsMode)
    }

    fn press(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn typ(view: &mut PermissionView, s: &str) {
        for c in s.chars() {
            let _ = view.handle_key(press(KeyCode::Char(c)));
        }
    }

    fn buffer_text(view: &PermissionView, area: Rect) -> String {
        let mut buf = Buffer::empty(area);
        view.render(area, &mut buf);
        (area.top()..area.bottom())
            .map(|y| {
                (area.left()..area.right())
                    .map(|x| {
                        buf.cell(Position::new(x, y))
                            .map_or(" ", ratatui::buffer::Cell::symbol)
                    })
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    // ===== ToolUseConfirm =====

    #[test]
    fn enter_sends_allow_once_and_completes() {
        let (exchange, resp_rx) = tool_exchange();
        let mut view = PermissionView::new(exchange);
        assert!(matches!(
            view.handle_key(press(KeyCode::Char('x'))),
            ViewOutcome::Pending
        ));
        assert!(matches!(
            view.handle_key(press(KeyCode::Enter)),
            ViewOutcome::PermissionResponse(PermissionResponse::AllowOnce)
        ));
        assert_eq!(
            resp_rx.blocking_recv().unwrap(),
            PermissionResponse::AllowOnce
        );
    }

    #[test]
    fn arrow_navigation_plus_enter_sends_allow_always() {
        let (exchange, resp_rx) = tool_exchange();
        let mut view = PermissionView::new(exchange);
        assert!(matches!(
            view.handle_key(press(KeyCode::Down)),
            ViewOutcome::Pending
        ));
        assert!(matches!(
            view.handle_key(press(KeyCode::Enter)),
            ViewOutcome::PermissionResponse(PermissionResponse::AllowAlways)
        ));
        assert_eq!(
            resp_rx.blocking_recv().unwrap(),
            PermissionResponse::AllowAlways
        );
    }

    #[test]
    fn esc_denies() {
        let (exchange, resp_rx) = tool_exchange();
        let mut view = PermissionView::new(exchange);
        assert!(matches!(
            view.handle_key(press(KeyCode::Esc)),
            ViewOutcome::PermissionResponse(PermissionResponse::Deny)
        ));
        assert_eq!(resp_rx.blocking_recv().unwrap(), PermissionResponse::Deny);
    }

    #[test]
    fn number_shortcut_three_denies() {
        let (exchange, resp_rx) = tool_exchange();
        let mut view = PermissionView::new(exchange);
        assert!(matches!(
            view.handle_key(press(KeyCode::Char('3'))),
            ViewOutcome::PermissionResponse(PermissionResponse::Deny)
        ));
        assert_eq!(resp_rx.blocking_recv().unwrap(), PermissionResponse::Deny);
    }

    // ===== §27b: `suppress_always_allow_rule` =====

    #[test]
    fn suppressed_request_omits_the_allow_always_row() {
        let (exchange, _resp_rx) = suppressed_tool_exchange();
        let view = PermissionView::new(exchange);
        let text = buffer_text(&view, Rect::new(0, 0, 80, VIEWPORT_HEIGHT));
        assert!(
            !text.contains("allow always"),
            "suppressed dialog must not render \"Yes, allow always\": {text:?}"
        );
        // The other two rows are unaffected.
        assert!(text.contains("Yes"));
        assert!(text.contains("differently"));
    }

    #[test]
    fn unsuppressed_request_still_offers_allow_always() {
        let (exchange, _resp_rx) = tool_exchange();
        let view = PermissionView::new(exchange);
        let text = buffer_text(&view, Rect::new(0, 0, 80, VIEWPORT_HEIGHT));
        assert!(text.contains("allow always"));
    }

    #[test]
    fn suppressed_request_arrow_down_plus_enter_denies_not_allow_always() {
        // With the middle row gone, Down then Enter lands on the (now second)
        // row, which is Deny — NOT AllowAlways.
        let (exchange, resp_rx) = suppressed_tool_exchange();
        let mut view = PermissionView::new(exchange);
        assert!(matches!(
            view.handle_key(press(KeyCode::Down)),
            ViewOutcome::Pending
        ));
        assert!(matches!(
            view.handle_key(press(KeyCode::Enter)),
            ViewOutcome::PermissionResponse(PermissionResponse::Deny)
        ));
        assert_eq!(resp_rx.blocking_recv().unwrap(), PermissionResponse::Deny);
    }

    #[test]
    fn suppressed_request_number_shortcut_two_denies() {
        // Only 2 rows exist, so the `2` shortcut selects the deny row (there
        // is no row 3 to fall through to `_` from — this exercises the
        // `1 if *always_allow_offered` guard directly).
        let (exchange, resp_rx) = suppressed_tool_exchange();
        let mut view = PermissionView::new(exchange);
        assert!(matches!(
            view.handle_key(press(KeyCode::Char('2'))),
            ViewOutcome::PermissionResponse(PermissionResponse::Deny)
        ));
        assert_eq!(resp_rx.blocking_recv().unwrap(), PermissionResponse::Deny);
    }

    #[test]
    fn eligible_workflow_bash_renders_exact_auto_row_and_response() {
        let (exchange, resp_rx) = auto_tool_exchange();
        let mut view = PermissionView::new(exchange);
        let text = buffer_text(&view, Rect::new(0, 0, 80, VIEWPORT_HEIGHT));
        assert!(text.contains("Yes, and switch to auto mode"), "{text}");
        assert!(!text.contains("allow always"), "{text}");
        assert!(matches!(
            view.handle_key(press(KeyCode::Char('2'))),
            ViewOutcome::PermissionResponse(PermissionResponse::AllowAuto)
        ));
        assert_eq!(
            resp_rx.blocking_recv().unwrap(),
            PermissionResponse::AllowAuto
        );
    }

    #[test]
    fn n_key_denies_but_ctrl_n_stays_swallowed() {
        let (exchange, resp_rx) = tool_exchange();
        let mut view = PermissionView::new(exchange);
        // Ctrl-N is a swallowed chord (keyboard ownership), NOT a deny.
        assert!(matches!(
            view.handle_key(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::CONTROL)),
            ViewOutcome::Pending
        ));
        assert!(view.resp_tx.is_some());
        assert!(matches!(
            view.handle_key(press(KeyCode::Char('N'))),
            ViewOutcome::PermissionResponse(PermissionResponse::Deny)
        ));
        assert_eq!(resp_rx.blocking_recv().unwrap(), PermissionResponse::Deny);
    }

    #[test]
    fn response_sender_is_consumed_exactly_once() {
        let (exchange, resp_rx) = tool_exchange();
        let mut view = PermissionView::new(exchange);
        view.handle_key(press(KeyCode::Char('2')));
        assert_eq!(
            resp_rx.blocking_recv().unwrap(),
            PermissionResponse::AllowAlways
        );
        // A second resolution finds the sender gone and cannot re-send (the
        // stack pops the view on the first resolution; this guards the
        // consume-once property even if it did not).
        assert!(matches!(
            view.handle_key(press(KeyCode::Enter)),
            ViewOutcome::PermissionResponse(_)
        ));
        assert!(view.resp_tx.is_none());
    }

    #[test]
    fn dropping_the_view_unresolved_closes_the_channel_as_a_deny_signal() {
        // The gate maps a dropped resp_tx to Deny ("TUI permission response
        // dropped") — verify each variant's view drops the sender unsent.
        let requests = [
            PermissionRequest::ToolUseConfirm {
                tool_name: "Bash".to_string(),
                tool_input: serde_json::json!({}),
                default_decision: permission::gate::PromptDefault::DenyByDefault,
                suppress_always_allow_rule: false,
            },
            PermissionRequest::ExitPlanMode {
                plan: "1. Foo".to_string(),
            },
            PermissionRequest::BypassPermissionsMode,
        ];
        for request in requests {
            let (exchange, resp_rx) = exchange_for(request);
            let view = PermissionView::new(exchange);
            drop(view);
            assert!(
                resp_rx.blocking_recv().is_err(),
                "channel closes without a response"
            );
        }
    }

    #[test]
    fn desired_height_is_the_locked_permission_viewport() {
        let (exchange, _resp_rx) = tool_exchange();
        let view = PermissionView::new(exchange);
        assert_eq!(view.desired_height(80), 9);
        assert!(view.wants_status_line());
    }

    // ===== ExitPlanMode =====

    #[test]
    fn plan_approval_renders_plan_body_and_approval_options() {
        let (exchange, _resp_rx) = plan_exchange("1. Add tests\n2. Refactor");
        let view = PermissionView::new(exchange);
        let text = buffer_text(&view, Rect::new(0, 0, 80, 12));
        assert!(text.contains("Ready to code?"), "{text}");
        assert!(text.contains("1. Add tests"), "{text}");
        assert!(text.contains("2. Refactor"), "{text}");
        assert!(text.contains("› Yes, manually approve edits"), "{text}");
        assert!(text.contains("Yes, auto-accept edits"), "{text}");
        assert!(text.contains("No, keep planning"), "{text}");
    }

    #[test]
    fn plan_approval_elides_long_plans_so_options_stay_visible() {
        let plan: String = (1..=10)
            .map(|i| format!("{i}. step"))
            .collect::<Vec<_>>()
            .join("\n");
        let (exchange, _resp_rx) = plan_exchange(&plan);
        let view = PermissionView::new(exchange);
        // 6 plan lines + elision line + 3 options + separator + 4 chrome = 15,
        // safely under the 20-row viewport clamp.
        assert_eq!(view.desired_height(80), 15);
        let text = buffer_text(&view, Rect::new(0, 0, 80, 15));
        assert!(text.contains("6. step"), "{text}");
        assert!(!text.contains("7. step"), "elided: {text}");
        assert!(text.contains("… (+4 more plan lines)"), "{text}");
        assert!(
            text.contains("No, keep planning"),
            "options visible: {text}"
        );
    }

    #[test]
    fn plan_approval_allow_once_allow_always_and_deny_branches() {
        // '1' → AllowOnce (manually approve edits).
        let (exchange, resp_rx) = plan_exchange("plan");
        let mut view = PermissionView::new(exchange);
        assert!(matches!(
            view.handle_key(press(KeyCode::Char('1'))),
            ViewOutcome::PermissionResponse(PermissionResponse::AllowOnce)
        ));
        assert_eq!(
            resp_rx.blocking_recv().unwrap(),
            PermissionResponse::AllowOnce
        );

        // '2' → AllowAlways (auto-accept edits).
        let (exchange, resp_rx) = plan_exchange("plan");
        let mut view = PermissionView::new(exchange);
        assert!(matches!(
            view.handle_key(press(KeyCode::Char('2'))),
            ViewOutcome::PermissionResponse(PermissionResponse::AllowAlways)
        ));
        assert_eq!(
            resp_rx.blocking_recv().unwrap(),
            PermissionResponse::AllowAlways
        );

        // Esc, 'n', and the third option all keep planning (Deny).
        for deny_key in [KeyCode::Esc, KeyCode::Char('n'), KeyCode::Char('3')] {
            let (exchange, resp_rx) = plan_exchange("plan");
            let mut view = PermissionView::new(exchange);
            assert!(matches!(
                view.handle_key(press(deny_key)),
                ViewOutcome::PermissionResponse(PermissionResponse::Deny)
            ));
            assert_eq!(resp_rx.blocking_recv().unwrap(), PermissionResponse::Deny);
        }
    }

    #[test]
    fn eligible_exit_plan_renders_exact_auto_row_and_selects_allow_auto() {
        let (exchange, resp_rx) = auto_plan_exchange("plan");
        let mut view = PermissionView::new(exchange);
        let text = buffer_text(&view, Rect::new(0, 0, 80, 15));
        assert!(text.contains("Yes, and use auto mode"), "{text}");
        assert!(!text.contains("Yes, auto-accept edits"), "{text}");
        assert!(matches!(
            view.handle_key(press(KeyCode::Char('2'))),
            ViewOutcome::PermissionResponse(PermissionResponse::AllowAuto)
        ));
        assert_eq!(
            resp_rx.blocking_recv().unwrap(),
            PermissionResponse::AllowAuto
        );
    }

    #[test]
    fn plan_approval_arrows_navigate_and_enter_confirms_highlight() {
        let (exchange, resp_rx) = plan_exchange("plan");
        let mut view = PermissionView::new(exchange);
        assert!(matches!(
            view.handle_key(press(KeyCode::Down)),
            ViewOutcome::Pending
        ));
        assert!(matches!(
            view.handle_key(press(KeyCode::Down)),
            ViewOutcome::Pending
        ));
        assert!(matches!(
            view.handle_key(press(KeyCode::Enter)),
            ViewOutcome::PermissionResponse(PermissionResponse::Deny)
        ));
        assert_eq!(resp_rx.blocking_recv().unwrap(), PermissionResponse::Deny);
    }

    // ===== BypassPermissionsMode =====

    #[test]
    fn bypass_renders_warning_and_typed_buffer() {
        let (exchange, _resp_rx) = bypass_exchange();
        let mut view = PermissionView::new(exchange);
        typ(&mut view, "ye");
        let area = Rect::new(0, 0, 80, view.desired_height(80));
        let text = buffer_text(&view, area);
        assert!(text.contains("WARNING: LingXi running in Bypass"), "{text}");
        assert!(text.contains("will not ask for your approval"), "{text}");
        assert!(
            text.contains("you accept all responsibility"),
            "wrapped body visible: {text}"
        );
        assert!(
            text.contains("Esc to cancel: ye"),
            "typed buffer echoes: {text}"
        );
    }

    #[test]
    fn bypass_typed_yes_then_enter_allows_once_case_insensitively() {
        let (exchange, resp_rx) = bypass_exchange();
        let mut view = PermissionView::new(exchange);
        typ(&mut view, "YeS");
        assert!(matches!(
            view.handle_key(press(KeyCode::Enter)),
            ViewOutcome::PermissionResponse(PermissionResponse::AllowOnce)
        ));
        assert_eq!(
            resp_rx.blocking_recv().unwrap(),
            PermissionResponse::AllowOnce
        );
    }

    #[test]
    fn bypass_enter_without_yes_does_not_resolve() {
        let (exchange, _resp_rx) = bypass_exchange();
        let mut view = PermissionView::new(exchange);
        assert!(matches!(
            view.handle_key(press(KeyCode::Enter)),
            ViewOutcome::Pending
        ));
        typ(&mut view, "yo");
        assert!(matches!(
            view.handle_key(press(KeyCode::Enter)),
            ViewOutcome::Pending
        ));
        assert!(view.resp_tx.is_some(), "still unresolved");
    }

    #[test]
    fn bypass_backspace_edits_the_typo_then_enter_resolves() {
        let (exchange, resp_rx) = bypass_exchange();
        let mut view = PermissionView::new(exchange);
        typ(&mut view, "yez");
        view.handle_key(press(KeyCode::Backspace));
        typ(&mut view, "s");
        assert!(matches!(
            view.handle_key(press(KeyCode::Enter)),
            ViewOutcome::PermissionResponse(PermissionResponse::AllowOnce)
        ));
        assert_eq!(
            resp_rx.blocking_recv().unwrap(),
            PermissionResponse::AllowOnce
        );
    }

    #[test]
    fn bypass_esc_and_n_deny_even_with_partial_input() {
        for deny_key in [KeyCode::Esc, KeyCode::Char('n'), KeyCode::Char('N')] {
            let (exchange, resp_rx) = bypass_exchange();
            let mut view = PermissionView::new(exchange);
            typ(&mut view, "ye");
            assert!(matches!(
                view.handle_key(press(deny_key)),
                ViewOutcome::PermissionResponse(PermissionResponse::Deny)
            ));
            assert_eq!(resp_rx.blocking_recv().unwrap(), PermissionResponse::Deny);
        }
    }

    #[test]
    fn bypass_ignores_digits_and_ctrl_chords() {
        let (exchange, _resp_rx) = bypass_exchange();
        let mut view = PermissionView::new(exchange);
        // No '1'-'9' jump-select in the typed confirmation.
        assert!(matches!(
            view.handle_key(press(KeyCode::Char('1'))),
            ViewOutcome::Pending
        ));
        // Ctrl-C is swallowed (locked view keyboard ownership) and must NOT
        // leak a 'c' into the typed buffer.
        typ(&mut view, "ye");
        assert!(matches!(
            view.handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)),
            ViewOutcome::Pending
        ));
        typ(&mut view, "s");
        let Prompt::TypedConfirm { typed } = &view.prompt else {
            panic!("bypass prompt");
        };
        assert_eq!(typed, "yes", "digits/ctrl chords never buffer");
    }

    #[test]
    fn bypass_desired_height_wraps_the_locked_body_at_the_pane_width() {
        let (exchange, _resp_rx) = bypass_exchange();
        let view = PermissionView::new(exchange);
        let wide = view.desired_height(120);
        let narrow = view.desired_height(60);
        assert!(wide >= 7, "body + prompt + chrome: {wide}");
        assert!(
            narrow > wide,
            "narrower panes wrap to more rows: {narrow} vs {wide}"
        );
        assert!(view.cursor_pos(Rect::new(0, 0, 80, 12)).is_none());
    }
}
