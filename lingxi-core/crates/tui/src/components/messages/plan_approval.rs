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

use crate::state::PlanApprovalKind;
use crate::theme::TuiTheme;

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
#[component]
pub fn PlanApprovalMessage(props: &PlanApprovalProps) -> impl Into<AnyElement<'static>> {
    let body = render_plan_approval_to_string(props.clone());
    // TODO(M7-15): planMode/success/error round borders via theme.
    let color = match &props.kind {
        PlanApprovalKind::Request { .. } => Color::Magenta,
        PlanApprovalKind::Approved { .. } => Color::Green,
        PlanApprovalKind::Rejected { .. } => TuiTheme::ERROR,
    };
    element! {
        View(flex_direction: FlexDirection::Column) {
            Text(content: body, color: color)
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
