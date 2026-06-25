//! Worker-permission chrome (claude-code `WorkerBadge.tsx` +
//! `WorkerPendingPermission.tsx`): a colored `● @name` badge and the
//! "waiting for team lead approval" view. (perm-10) `BLACK_CIRCLE` is
//! platform-dependent: `⏺` (U+23FA) on macOS, `●` (U+25CF) elsewhere.
#![allow(clippy::needless_pass_by_value)]

use iocraft::prelude::*;

use crate::multiagent::style::agent_color_from_name;
use crate::theme::Theme;

/// `BLACK_CIRCLE` + ` ` (claude-code `figures.ts`): `⏺` (U+23FA) on macOS,
/// `●` (U+25CF) elsewhere.
pub const BADGE_CIRCLE: &str = if cfg!(target_os = "macos") {
    "\u{23FA} "
} else {
    "\u{25CF} "
};

/// Worker identity carried on a pending permission (TUI-side; not on the wire).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WorkerPermissionInfo {
    /// Worker display name (rendered `@name`).
    pub name: String,
    /// Worker color name (→ `agent_color_from_name`).
    pub color: String,
    /// Optional team name (for the "sent to team … leader" line).
    pub team: Option<String>,
}

/// `{BADGE_CIRCLE}@{name}` (claude-code `WorkerBadge`). Color applied by the
/// component.
#[must_use]
pub fn render_worker_badge(name: &str) -> String {
    format!("{BADGE_CIRCLE}@{name}")
}

/// The worker-pending body (claude-code `WorkerPendingPermission`).
#[must_use]
pub fn render_worker_pending_to_string(
    tool: &str,
    description: &str,
    worker_name: Option<&str>,
    team: Option<&str>,
) -> String {
    let mut out = String::from("Waiting for team lead approval");
    if let Some(name) = worker_name {
        out.push('\n');
        out.push_str(&render_worker_badge(name));
    }
    out.push_str(&format!("\nTool: {tool}\nAction: {description}"));
    if let Some(t) = team {
        out.push_str(&format!("\nPermission request sent to team \"{t}\" leader"));
    }
    out
}

/// Props for [`WorkerBadge`].
#[derive(Debug, Clone, Default, Props)]
pub struct WorkerBadgeProps {
    /// Worker display name.
    pub name: String,
    /// Worker color name.
    pub color: String,
}

/// iocraft component: colored circle + bold `@name`.
#[component]
pub fn WorkerBadge(props: &WorkerBadgeProps) -> impl Into<AnyElement<'static>> {
    let circle_color = agent_color_from_name(&props.color);
    let name = format!("@{}", props.name);
    element! {
        View(flex_direction: FlexDirection::Row) {
            Text(content: BADGE_CIRCLE, color: circle_color)
            Text(content: name, weight: Weight::Bold)
        }
    }
}

/// Props for [`WorkerPendingPermission`].
#[derive(Debug, Clone, Default, Props)]
pub struct WorkerPendingProps {
    /// Requested tool.
    pub tool: String,
    /// Action description.
    pub description: String,
    /// Worker identity (badge shown when present).
    pub worker: Option<WorkerPermissionInfo>,
    /// Active palette.
    pub theme: Theme,
}

/// iocraft component: round warning border, "Waiting…" header (bold warning),
/// worker badge, Tool/Action lines, optional team line.
#[component]
pub fn WorkerPendingPermission(props: &WorkerPendingProps) -> impl Into<AnyElement<'static>> {
    let theme = props.theme;
    let (worker_name, team) = props
        .worker
        .as_ref()
        .map_or((None, None), |w| (Some(w.name.clone()), w.team.clone()));
    let body = render_worker_pending_to_string(
        &props.tool,
        &props.description,
        worker_name.as_deref(),
        team.as_deref(),
    );
    let mut lines = body.lines();
    let header = lines.next().unwrap_or("").to_string();
    let rest = lines.collect::<Vec<_>>().join("\n");
    element! {
        View(
            flex_direction: FlexDirection::Column,
            border_style: BorderStyle::Round,
            border_color: theme.warning,
        ) {
            Text(content: header, color: theme.warning, weight: Weight::Bold)
            Text(content: rest, color: theme.text)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn badge_bytes_and_format() {
        // (perm-10) Platform-dependent glyph: ⏺ (macOS) or ● (elsewhere).
        #[cfg(target_os = "macos")]
        assert_eq!(BADGE_CIRCLE.as_bytes(), &[0xE2, 0x8F, 0xBA, 0x20]); // ⏺ + space
        #[cfg(not(target_os = "macos"))]
        assert_eq!(BADGE_CIRCLE.as_bytes(), &[0xE2, 0x97, 0x8F, 0x20]); // ● + space
        assert_eq!(render_worker_badge("alice"), format!("{BADGE_CIRCLE}@alice"));
    }

    #[test]
    fn pending_minimal() {
        assert_eq!(
            render_worker_pending_to_string("Bash", "run ls", None, None),
            "Waiting for team lead approval\nTool: Bash\nAction: run ls"
        );
    }

    #[test]
    fn pending_full() {
        assert_eq!(
            render_worker_pending_to_string("Bash", "run ls", Some("alice"), Some("my-team")),
            format!(
                "Waiting for team lead approval\n{BADGE_CIRCLE}@alice\nTool: Bash\nAction: run ls\nPermission request sent to team \"my-team\" leader"
            )
        );
    }
}
