//! `PlanApprovalMessage` — plan approval request/response.
//!
//! Literal lock (claude-code `PlanApprovalMessage.tsx`): request →
//! `Plan Approval Request from {from}` (planMode, bold) + markdown plan
//! content + `Plan file: {path}` dim, round planMode border. approved →
//! `✓ Plan Approved by {name}` (success, bold) + `You can now proceed with
//! implementation. Your plan mode restrictions have been lifted.`. rejected →
//! `✗ Plan Rejected by {name}` (error, bold) + optional `Feedback: {feedback}`
//! + `Please revise your plan based on the feedback and call ExitPlanMode
//! again.` (dim).
#![allow(clippy::needless_pass_by_value)]

use iocraft::prelude::*;

use crate::render_iocraft::StyleColorIocraftExt;
use crate::state::PlanApprovalKind;
use crate::theme::Theme;

/// U+2713 check mark (claude-code `✓`).
pub const CHECK: &str = "\u{2713}";
/// U+2717 ballot X (claude-code `✗`).
pub const CROSS: &str = "\u{2717}";
/// Locked approved tail.
pub const APPROVED_TAIL: &str =
    "You can now proceed with implementation. Your plan mode restrictions have been lifted.";
/// Locked rejected tail.
pub const REJECTED_TAIL: &str =
    "Please revise your plan based on the feedback and call ExitPlanMode again.";

/// Props for [`PlanApprovalMessage`].
#[derive(Debug, Clone, Default, Props)]
pub struct PlanApprovalProps {
    /// Request/approved/rejected content.
    pub kind: PlanApprovalKind,
    /// (M7-15) Active palette — `plan_mode`/success/error colors centralized.
    pub theme: Theme,
}

/// Pure-string renderer.
#[must_use]
pub fn render_plan_approval_to_string(props: PlanApprovalProps) -> String {
    match &props.kind {
        PlanApprovalKind::Request {
            from,
            plan_content,
            plan_file_path,
        } => {
            let mut out = format!("Plan Approval Request from {from}\n");
            // Plan content is markdown (render::markdown styles it in the
            // component; the snapshot oracle keeps the raw text).
            out.push_str(plan_content);
            if let Some(p) = plan_file_path {
                out.push('\n');
                out.push_str(&format!("Plan file: {p}"));
            }
            out
        }
        PlanApprovalKind::Approved { name } => {
            format!("{CHECK} Plan Approved by {name}\n{APPROVED_TAIL}")
        }
        PlanApprovalKind::Rejected { name, feedback } => {
            let mut out = format!("{CROSS} Plan Rejected by {name}");
            if let Some(f) = feedback {
                out.push('\n');
                out.push_str(&format!("Feedback: {f}"));
            }
            out.push('\n');
            out.push_str(REJECTED_TAIL);
            out
        }
    }
}

/// iocraft component.
///
/// (M7-15) Colors centralized into the active [`Theme`] and the claude-code
/// round borders restored: request → `theme.plan_mode` (header + border),
/// approved → `theme.success`, rejected → `theme.error`. The header line is
/// bold; the body follows in the same accent color.
#[component]
pub fn PlanApprovalMessage(props: &PlanApprovalProps) -> impl Into<AnyElement<'static>> {
    let body = render_plan_approval_to_string(props.clone());
    let theme = props.theme;
    let accent = match &props.kind {
        PlanApprovalKind::Request { .. } => theme.plan_mode,
        PlanApprovalKind::Approved { .. } => theme.success,
        PlanApprovalKind::Rejected { .. } => theme.error,
    };
    // Header is the first line (bold); the remaining lines are the body.
    let mut lines = body.lines();
    let header = lines.next().unwrap_or("").to_string();
    let body_text = lines.collect::<Vec<_>>().join("\n");
    element! {
        View(
            flex_direction: FlexDirection::Column,
            border_style: BorderStyle::Round,
            border_color: accent.to_iocraft(),
        ) {
            Text(content: header, color: accent.to_iocraft(), weight: Weight::Bold)
            Text(content: body_text, color: accent.to_iocraft())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn glyphs_are_check_and_cross() {
        assert_eq!(CHECK, "\u{2713}");
        assert_eq!(CROSS, "\u{2717}");
    }
}
