//! `UserTeammateMessage` — teammate transcript message.
//!
//! Literal lock (claude-code `UserTeammateMessage.tsx`): `@{display_name}❯`
//! header in the teammate's agent color. `TaskCompleted` → TWO lines: the header
//! line, then a `MessageResponse`-guttered line
//! `  ⎿  ✓ Completed task #{task_id}` + optional ` ({task_subject})` (dim).
//! Note → `@name❯` + optional ` {summary}` (same line); transcript mode
//! appends the full content, each line indented 2. (plan-approval/shutdown
//! sub-types reuse the existing renderers; `idle_notification` is suppressed
//! upstream.)
#![allow(clippy::needless_pass_by_value)]

use iocraft::prelude::*;

use crate::multiagent::style::agent_color_from_name;
use crate::state::UserTeammateKind;
use crate::theme::Theme;
use crate::render_iocraft::StyleColorIocraftExt;

/// `❯` teammate-header pointer (U+276F).
pub const POINTER: &str = "\u{276F}";
/// `✓` completed check (U+2713).
pub const CHECK: &str = "\u{2713}";
/// `  ⎿  ` `MessageResponse` gutter (2 spaces + U+23BF + 2 spaces).
pub const GUTTER: &str = "  \u{23BF}  ";

/// Props for [`UserTeammateMessage`].
#[derive(Debug, Clone, Props)]
pub struct UserTeammateProps {
    /// Display name.
    pub display_name: String,
    /// Optional agent color name.
    pub color: Option<String>,
    /// Sub-type payload.
    pub kind: UserTeammateKind,
    /// Active palette.
    pub theme: Theme,
}

impl Default for UserTeammateProps {
    fn default() -> Self {
        Self {
            display_name: String::new(),
            color: None,
            kind: UserTeammateKind::Note {
                summary: None,
                content: None,
                is_transcript_mode: false,
            },
            theme: Theme::dark(),
        }
    }
}

/// Pure-string renderer.
#[must_use]
pub fn render_user_teammate_to_string(props: UserTeammateProps) -> String {
    let header = format!("@{}{POINTER}", props.display_name);
    match &props.kind {
        UserTeammateKind::TaskCompleted {
            task_id,
            task_subject,
        } => {
            // Two lines: header, then the MessageResponse-guttered completed line.
            let mut line2 = format!("{GUTTER}{CHECK} Completed task #{task_id}");
            if let Some(s) = task_subject {
                line2.push_str(&format!(" ({s})"));
            }
            format!("{header}\n{line2}")
        }
        UserTeammateKind::Note {
            summary,
            content,
            is_transcript_mode,
        } => {
            let mut out = header;
            if let Some(s) = summary {
                out.push(' ');
                out.push_str(s);
            }
            if *is_transcript_mode {
                if let Some(c) = content {
                    for line in c.lines() {
                        out.push('\n');
                        out.push_str("  ");
                        out.push_str(line);
                    }
                }
            }
            out
        }
    }
}

/// iocraft component.
#[component]
pub fn UserTeammateMessage(props: &UserTeammateProps) -> impl Into<AnyElement<'static>> {
    let theme = props.theme;
    let accent = match &props.color {
        Some(name) => agent_color_from_name(name),
        None => agent_color_from_name(""), // cyan fallback
    };
    let header = format!("@{}{POINTER}", props.display_name);
    match &props.kind {
        UserTeammateKind::TaskCompleted {
            task_id,
            task_subject,
        } => {
            let completed = format!(" Completed task #{task_id}");
            let subject = task_subject.as_ref().map(|s| format!(" ({s})"));
            element! {
                View(flex_direction: FlexDirection::Column) {
                    Text(content: header.clone(), color: accent)
                    View(flex_direction: FlexDirection::Row) {
                        Text(content: GUTTER, color: theme.dim.to_iocraft())
                        Text(content: CHECK, color: theme.success.to_iocraft())
                        Text(content: completed, color: theme.text.to_iocraft())
                        #(subject.map(|s| element! {
                            Text(content: s, color: theme.dim.to_iocraft())
                        }))
                    }
                }
            }
            .into_any()
        }
        UserTeammateKind::Note {
            summary,
            content,
            is_transcript_mode,
        } => {
            let head = match summary {
                Some(s) => format!("{header} {s}"),
                None => header.clone(),
            };
            let body = if *is_transcript_mode {
                content.as_ref().map(|c| {
                    c.lines()
                        .map(|l| format!("  {l}"))
                        .collect::<Vec<_>>()
                        .join("\n")
                })
            } else {
                None
            };
            element! {
                View(flex_direction: FlexDirection::Column) {
                    Text(content: head, color: accent)
                    #(body.map(|b| element! {
                        Text(content: b, color: theme.text.to_iocraft())
                    }))
                }
            }
            .into_any()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glyph_bytes() {
        assert_eq!(POINTER, "\u{276F}");
        assert_eq!(CHECK, "\u{2713}");
        // ⎿ = U+23BF; gutter = 2 spaces + ⎿ + 2 spaces.
        assert_eq!(
            GUTTER.as_bytes(),
            &[0x20, 0x20, 0xE2, 0x8E, 0xBF, 0x20, 0x20]
        );
    }

    #[test]
    fn task_completed_with_subject() {
        let out = render_user_teammate_to_string(UserTeammateProps {
            display_name: "alice".into(),
            color: Some("magenta".into()),
            kind: UserTeammateKind::TaskCompleted {
                task_id: "456".into(),
                task_subject: Some("Setup DB".into()),
            },
            theme: Theme::dark(),
        });
        assert_eq!(
            out,
            "@alice\u{276F}\n  \u{23BF}  \u{2713} Completed task #456 (Setup DB)"
        );
    }

    #[test]
    fn task_completed_no_subject() {
        let out = render_user_teammate_to_string(UserTeammateProps {
            display_name: "lead".into(),
            color: None,
            kind: UserTeammateKind::TaskCompleted {
                task_id: "1".into(),
                task_subject: None,
            },
            theme: Theme::dark(),
        });
        assert_eq!(out, "@lead\u{276F}\n  \u{23BF}  \u{2713} Completed task #1");
    }

    #[test]
    fn note_summary_only() {
        let out = render_user_teammate_to_string(UserTeammateProps {
            display_name: "bob".into(),
            color: None,
            kind: UserTeammateKind::Note {
                summary: Some("on it".into()),
                content: Some("full body".into()),
                is_transcript_mode: false,
            },
            theme: Theme::dark(),
        });
        assert_eq!(out, "@bob\u{276F} on it");
    }

    #[test]
    fn note_transcript_indents_content() {
        let out = render_user_teammate_to_string(UserTeammateProps {
            display_name: "bob".into(),
            color: None,
            kind: UserTeammateKind::Note {
                summary: Some("done".into()),
                content: Some("line1\nline2".into()),
                is_transcript_mode: true,
            },
            theme: Theme::dark(),
        });
        assert_eq!(out, "@bob\u{276F} done\n  line1\n  line2");
    }
}
