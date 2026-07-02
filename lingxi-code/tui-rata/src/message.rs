//! Render `RenderedMessage`s into styled lines for the ratatui scrollback.
//!
//! This is the `tui-rata` side of message rendering: it consumes the neutral
//! `tui_core` message model + render primitives and produces `StyledLine`s that
//! [`crate::render::styled_line_to_ratatui`] turns into ratatui text. The
//! common variants are rendered directly here in ratatui-land; the iocraft
//! component renderers (for the interactive/expandable variants) stay in `tui`.

use tui_core::message::{AdvisorKind, PlanApprovalKind, RenderedMessage, SystemLevel, UserTeammateKind};
use tui_core::render::markdown::{render_with_width, MarkdownTheme};
use tui_core::render::{agent_color_from_name, SpanStyle, StyleColor, StyledLine, StyledSpan};
use tui_core::theme::{Theme, ThemeName};

/// Assistant dot marker (iocraft `assistant_text::MARKER` parity): `⏺ `
/// (U+23FA) on macOS — renders as the reddish record glyph — `● ` (U+25CF)
/// elsewhere, each + a trailing space.
const ASSISTANT_MARKER: &str = if cfg!(target_os = "macos") {
    "\u{23FA} "
} else {
    "\u{25CF} "
};
const CONT_INDENT: &str = "  ";
const DEFAULT_WIDTH: usize = 80;

fn markdown_theme() -> MarkdownTheme {
    MarkdownTheme {
        inline_code: StyleColor::Rgb(177, 185, 249),
        code_theme: ThemeName::Dark,
    }
}

/// Render one message into styled lines for the scrollback. Returns an empty
/// vec for variants that have no native-scrollback text form yet (they remain
/// the iocraft renderers' responsibility until ported). `verbose` expands the
/// collapsible variants (thinking body, tool-use JSON, grouped children).
#[must_use]
pub fn render_message(
    entry: &RenderedMessage,
    width: usize,
    theme: &Theme,
    verbose: bool,
) -> Vec<StyledLine> {
    let width = if width == 0 { DEFAULT_WIDTH } else { width };
    match entry {
        RenderedMessage::UserText { body, .. } => {
            if body.is_empty() {
                Vec::new()
            } else {
                vec![StyledLine::plain(format!("> {body}"))]
            }
        }
        RenderedMessage::AssistantText { body, .. } => assistant_lines(body, width),
        RenderedMessage::SystemText { body, is_error, .. } => {
            let color = if *is_error { theme.error } else { theme.dim };
            colored_lines(body, color)
        }
        RenderedMessage::SystemTextRich { body, level } => {
            let color = match level {
                SystemLevel::Info => theme.dim,
                SystemLevel::Warning => theme.warning,
                SystemLevel::Error => theme.error,
            };
            colored_lines(body, color)
        }
        RenderedMessage::AssistantToolUse { tool, input, .. } => {
            tool_use_lines(tool, input, theme, verbose)
        }
        RenderedMessage::UserToolResult { result, .. } => tool_result_lines(result, theme),
        RenderedMessage::UserBashInput { command } => {
            colored_lines(&format!("! {command}"), theme.dim)
        }
        RenderedMessage::UserBashOutput { stdout, stderr }
        | RenderedMessage::UserLocalCommandOutput { stdout, stderr } => {
            bash_output_lines(stdout, stderr, theme)
        }
        RenderedMessage::UserCommand { command, args, .. } => {
            let text = if args.is_empty() {
                format!("/{command}")
            } else {
                format!("/{command} {args}")
            };
            vec![StyledLine::plain(text)]
        }
        RenderedMessage::UserMemoryInput { input } => {
            colored_lines(&format!("# {input}"), theme.dim)
        }
        RenderedMessage::UserPlan { plan_content } => plain_lines(plan_content),
        RenderedMessage::UserPrompt { text } => plain_lines(text),
        RenderedMessage::AgentNotification { summary, .. } => colored_lines(summary, theme.dim),
        RenderedMessage::CompactBoundary { .. } => {
            colored_lines("✻ Conversation compacted (ctrl+o for history)", theme.dim)
        }
        RenderedMessage::AssistantThinking { thinking, .. } => {
            if verbose {
                let mut out = colored_lines("✻ Thinking…", theme.dim);
                out.extend(colored_lines(thinking, theme.dim));
                out
            } else {
                colored_lines("✻ Thinking (ctrl+o to expand)", theme.dim)
            }
        }
        RenderedMessage::SystemApiError {
            error,
            retry_attempt,
            max_retries,
            ..
        } => colored_lines(
            &format!("API error: {error} (retry {retry_attempt}/{max_retries})"),
            theme.error,
        ),
        RenderedMessage::RateLimit { text, upsell } => {
            let mut out = colored_lines(text, theme.error);
            if let Some(upsell) = upsell {
                out.extend(colored_lines(upsell, theme.dim));
            }
            out
        }
        RenderedMessage::Shutdown { from, reason, .. } => {
            let msg = match reason {
                Some(r) => format!("{from} shut down: {r}"),
                None => format!("{from} shut down"),
            };
            colored_lines(&msg, theme.dim)
        }
        RenderedMessage::TaskAssignment {
            subject,
            description,
            ..
        } => {
            let mut out = vec![StyledLine::plain(format!("Task: {subject}"))];
            if let Some(d) = description {
                out.extend(colored_lines(d, theme.dim));
            }
            out
        }
        RenderedMessage::ChannelMessage {
            server,
            user,
            content,
        } => {
            let who = user.as_deref().unwrap_or("system");
            vec![StyledLine::plain(format!("[{server}] {who}: {content}"))]
        }
        RenderedMessage::UserTeammate {
            display_name,
            color,
            kind,
        } => teammate_lines(display_name, color.as_deref(), kind, theme),
        RenderedMessage::HookProgress { event, count, .. } => {
            colored_lines(&format!("hook: {event} (×{count})"), theme.dim)
        }
        RenderedMessage::UserResourceUpdate { updates } => updates
            .iter()
            .map(|(server, target, _)| StyledLine::plain(format!("resource updated: {server}/{target}")))
            .collect(),
        RenderedMessage::UserImage { image_id, metadata, .. } => {
            let head = match image_id {
                Some(n) => format!("[Image #{n}]"),
                None => "[Image]".to_string(),
            };
            let text = match metadata {
                Some(m) => format!("{head} ({m})"),
                None => head,
            };
            vec![StyledLine::plain(text)]
        }
        RenderedMessage::GroupedToolUse { tool, entries, .. } => {
            group_tool_use_lines(tool, entries, theme, verbose)
        }
        RenderedMessage::CollapsedReadSearch { entries, .. } => colored_lines(
            &format!("Read/Search ({} results)", entries.len()),
            theme.dim,
        ),
        RenderedMessage::Attachment { attachment } => attachment_lines(attachment),
        RenderedMessage::Advisor { kind, verbose } => advisor_lines(kind, *verbose, theme),
        RenderedMessage::PlanApproval { kind } => plan_approval_lines(kind, theme),
        RenderedMessage::AssistantRedactedThinking => colored_lines("✻ Thinking…", theme.dim),
        _ => Vec::new(),
    }
}

/// Markdown-render the assistant body, prefixing the first line with the
/// `● ` marker and indenting continuation lines (mirrors the iocraft renderer).
fn assistant_lines(body: &str, width: usize) -> Vec<StyledLine> {
    let mut out = Vec::new();
    let mut rendered = render_with_width(body, &markdown_theme(), width)
        .into_iter()
        .filter(|line| !line.spans.is_empty());

    if let Some(mut first) = rendered.next() {
        first.spans.insert(0, StyledSpan::plain(ASSISTANT_MARKER));
        out.push(first);
    }
    for mut line in rendered {
        line.spans.insert(0, StyledSpan::plain(CONT_INDENT));
        out.push(line);
    }
    if out.is_empty() {
        out.push(StyledLine::plain(ASSISTANT_MARKER));
    }
    out
}

fn colored_lines(text: &str, color: StyleColor) -> Vec<StyledLine> {
    if text.is_empty() {
        return Vec::new();
    }
    text.split('\n')
        .map(|line| StyledLine {
            spans: vec![StyledSpan::styled(
                line.to_string(),
                SpanStyle {
                    fg: color,
                    ..SpanStyle::default()
                },
            )],
        })
        .collect()
}

fn plain_lines(text: &str) -> Vec<StyledLine> {
    if text.is_empty() {
        return Vec::new();
    }
    text.split('\n').map(StyledLine::plain).collect()
}

/// One-line truncation to `max` chars (newlines flattened to spaces).
fn truncate(s: &str, max: usize) -> String {
    let flat = s.replace('\n', " ");
    if flat.chars().count() <= max {
        flat
    } else {
        let cut: String = flat.chars().take(max.saturating_sub(1)).collect();
        format!("{cut}…")
    }
}

fn dim_span(text: String, theme: &Theme) -> StyledSpan {
    StyledSpan::styled(
        text,
        SpanStyle {
            fg: theme.dim,
            ..SpanStyle::default()
        },
    )
}

/// A tool call: `● {tool}` header + arguments. Collapsed → a dim, truncated
/// one-line summary; verbose → the pretty-printed JSON input, dim-indented.
fn tool_use_lines(
    tool: &str,
    input: &serde_json::Value,
    theme: &Theme,
    verbose: bool,
) -> Vec<StyledLine> {
    let header = StyledLine {
        spans: vec![
            StyledSpan::styled(
                "● ".to_string(),
                SpanStyle {
                    fg: theme.success,
                    ..SpanStyle::default()
                },
            ),
            StyledSpan::plain(tool.to_string()),
        ],
    };
    let mut out = vec![header];
    if verbose {
        let pretty = serde_json::to_string_pretty(input).unwrap_or_else(|_| input.to_string());
        for line in pretty.split('\n') {
            out.push(StyledLine {
                spans: vec![dim_span(format!("  {line}"), theme)],
            });
        }
    } else {
        out.push(StyledLine {
            spans: vec![dim_span(format!("  {}", truncate(&input.to_string(), 100)), theme)],
        });
    }
    out
}

/// A tool result: `⎿ {summary}` — the string content when present, else compact JSON.
fn tool_result_lines(result: &serde_json::Value, theme: &Theme) -> Vec<StyledLine> {
    let summary = if let Some(s) = result.as_str() {
        s.to_string()
    } else if let Some(s) = result.get("content").and_then(serde_json::Value::as_str) {
        s.to_string()
    } else {
        result.to_string()
    };
    vec![StyledLine {
        spans: vec![dim_span(format!("  ⎿ {}", truncate(&summary, 100)), theme)],
    }]
}

fn bash_output_lines(stdout: &str, stderr: &str, theme: &Theme) -> Vec<StyledLine> {
    let mut out = plain_lines(stdout);
    if !stderr.is_empty() {
        out.extend(colored_lines(stderr, theme.error));
    }
    out
}

/// A grouped tool-use block: `● {tool} (×{count})` header. Collapsed → header
/// only; verbose → header + each child's truncated input → result line.
fn group_tool_use_lines(
    tool: &str,
    entries: &[(serde_json::Value, serde_json::Value)],
    theme: &Theme,
    verbose: bool,
) -> Vec<StyledLine> {
    let mut out = vec![StyledLine {
        spans: vec![
            StyledSpan::styled(
                "● ".to_string(),
                SpanStyle {
                    fg: theme.success,
                    ..SpanStyle::default()
                },
            ),
            StyledSpan::plain(format!("{tool} (×{})", entries.len())),
        ],
    }];
    if verbose {
        for (input, result) in entries {
            out.push(StyledLine {
                spans: vec![dim_span(
                    format!(
                        "  ⎿ {} → {}",
                        truncate(&input.to_string(), 60),
                        truncate(&result.to_string(), 60)
                    ),
                    theme,
                )],
            });
        }
    }
    out
}

/// A one-line summary of a user attachment (directory listing / file read /
/// generic fallback).
fn attachment_lines(attachment: &tui_core::message::Attachment) -> Vec<StyledLine> {
    use tui_core::message::Attachment;
    let text = match attachment {
        Attachment::Directory { display_path } => format!("Listed directory {display_path}/"),
        Attachment::File { display_path, .. } => format!("Read {display_path}"),
        Attachment::CompactFileReference { display_path }
        | Attachment::NestedMemory { display_path } => format!("Loaded {display_path}"),
        _ => "[attachment]".to_string(),
    };
    vec![StyledLine::plain(text)]
}

/// A teammate message: an agent-colored `@name` header then the kind-specific
/// body — a completed-task line (success) or a plain note (summary + optional
/// content). `color` is the teammate's claude-code color name.
fn teammate_lines(
    display_name: &str,
    color: Option<&str>,
    kind: &UserTeammateKind,
    theme: &Theme,
) -> Vec<StyledLine> {
    let name_color = agent_color_from_name(color.unwrap_or(""));
    let name_span = |text: String| {
        StyledSpan::styled(
            text,
            SpanStyle {
                fg: name_color,
                ..SpanStyle::default()
            },
        )
    };
    match kind {
        UserTeammateKind::TaskCompleted {
            task_id,
            task_subject,
        } => {
            let subject = task_subject
                .as_deref()
                .map(|s| format!(" ({s})"))
                .unwrap_or_default();
            vec![StyledLine {
                spans: vec![
                    name_span(format!("@{display_name}: ")),
                    StyledSpan::styled(
                        format!("✓ Completed task #{task_id}{subject}"),
                        SpanStyle {
                            fg: theme.success,
                            ..SpanStyle::default()
                        },
                    ),
                ],
            }]
        }
        UserTeammateKind::Note {
            summary,
            content,
            is_transcript_mode,
        } => {
            let mut out = vec![StyledLine {
                spans: vec![name_span(format!("@{display_name}"))],
            }];
            if let Some(s) = summary {
                out.extend(colored_lines(s, theme.dim));
            }
            if *is_transcript_mode {
                if let Some(c) = content {
                    out.extend(colored_lines(c, theme.dim));
                }
            }
            out
        }
    }
}

/// An advisor block: a `✻ Advisor…` marker line whose text depends on the kind.
fn advisor_lines(kind: &AdvisorKind, verbose: bool, theme: &Theme) -> Vec<StyledLine> {
    match kind {
        AdvisorKind::ServerToolUse { model, input } => {
            let mut header = "✻ Advising".to_string();
            if let Some(m) = model {
                header.push_str(&format!(" ({m})"));
            }
            let mut out = colored_lines(&header, theme.dim);
            if let Some(i) = input {
                out.extend(colored_lines(&format!("  {}", truncate(i, 100)), theme.dim));
            }
            out
        }
        AdvisorKind::Result { text } => {
            let mut out = colored_lines("✻ Advisor", theme.dim);
            let body = if verbose {
                text.clone()
            } else {
                truncate(text, 100)
            };
            out.extend(colored_lines(&format!("  {body}"), theme.dim));
            out
        }
        AdvisorKind::RedactedResult => colored_lines("✻ Advisor", theme.dim),
        AdvisorKind::Error { error_code } => {
            colored_lines(&format!("✻ Advisor unavailable ({error_code})"), theme.error)
        }
    }
}

/// A plan-approval request/response line.
fn plan_approval_lines(kind: &PlanApprovalKind, theme: &Theme) -> Vec<StyledLine> {
    match kind {
        PlanApprovalKind::Request {
            from, plan_content, ..
        } => {
            let mut out = colored_lines(&format!("Plan approval requested by {from}"), theme.warning);
            out.extend(plain_lines(plan_content));
            out
        }
        PlanApprovalKind::Approved { name } => {
            colored_lines(&format!("✓ Plan approved by {name}"), theme.success)
        }
        PlanApprovalKind::Rejected { name, feedback } => {
            let mut out = colored_lines(&format!("✗ Plan rejected by {name}"), theme.error);
            if let Some(f) = feedback {
                out.extend(colored_lines(&format!("  {f}"), theme.dim));
            }
            out
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_text_gets_prompt_prefix() {
        let m = RenderedMessage::UserText {
            body: "hello".to_string(),
            timestamp: 0,
        };
        let lines = render_message(&m, 80, &Theme::dark(), false);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].plain_text(), "> hello");
    }

    #[test]
    fn assistant_text_gets_marker_and_renders_markdown() {
        let m = RenderedMessage::AssistantText {
            body: "**bold** text".to_string(),
            timestamp: 0,
        };
        let lines = render_message(&m, 80, &Theme::dark(), false);
        assert!(!lines.is_empty());
        assert!(lines[0].plain_text().starts_with(ASSISTANT_MARKER));
        assert!(lines[0].plain_text().contains("bold"));
    }

    #[test]
    fn system_text_is_colored() {
        let m = RenderedMessage::SystemText {
            body: "ready".to_string(),
            timestamp: 0,
            is_error: false,
        };
        let lines = render_message(&m, 80, &Theme::dark(), false);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].spans[0].style.fg, Theme::dark().dim);
    }

    #[test]
    fn redacted_thinking_renders_dim_marker() {
        let m = RenderedMessage::AssistantRedactedThinking;
        let lines = render_message(&m, 80, &Theme::dark(), false);
        assert_eq!(lines.len(), 1);
        assert!(lines[0].plain_text().contains("Thinking"));
        assert_eq!(lines[0].spans[0].style.fg, Theme::dark().dim);
    }

    #[test]
    fn teammate_task_completed_and_note_render() {
        let done = RenderedMessage::UserTeammate {
            display_name: "worker-1".to_string(),
            color: Some("magenta".to_string()),
            kind: tui_core::message::UserTeammateKind::TaskCompleted {
                task_id: "42".to_string(),
                task_subject: Some("fix bug".to_string()),
            },
        };
        let lines = render_message(&done, 80, &Theme::dark(), false);
        assert!(lines[0].plain_text().contains("@worker-1"));
        assert!(lines[0].plain_text().contains("Completed task #42"));
        assert!(lines[0].plain_text().contains("fix bug"));
        // The `@name` span is tinted by the teammate's agent color.
        assert_eq!(
            lines[0].spans[0].style.fg,
            tui_core::render::agent_color_from_name("magenta")
        );

        let note = RenderedMessage::UserTeammate {
            display_name: "leader".to_string(),
            color: None,
            kind: tui_core::message::UserTeammateKind::Note {
                summary: Some("looks good".to_string()),
                content: Some("full body".to_string()),
                is_transcript_mode: true,
            },
        };
        let text = render_message(&note, 80, &Theme::dark(), false)
            .iter()
            .map(|l| l.plain_text())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains("@leader"));
        assert!(text.contains("looks good"));
        assert!(text.contains("full body"));
    }

    #[test]
    fn advisor_variants_render() {
        let err = RenderedMessage::Advisor {
            kind: tui_core::message::AdvisorKind::Error {
                error_code: "503".to_string(),
            },
            verbose: false,
        };
        let lines = render_message(&err, 80, &Theme::dark(), false);
        assert!(lines[0].plain_text().contains("Advisor unavailable (503)"));
        assert_eq!(lines[0].spans[0].style.fg, Theme::dark().error);

        let result = RenderedMessage::Advisor {
            kind: tui_core::message::AdvisorKind::Result {
                text: "consider caching".to_string(),
            },
            verbose: true,
        };
        let text = render_message(&result, 80, &Theme::dark(), false)
            .iter()
            .map(|l| l.plain_text())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains("Advisor"));
        assert!(text.contains("consider caching"));
    }

    #[test]
    fn plan_approval_variants_render() {
        let approved = RenderedMessage::PlanApproval {
            kind: tui_core::message::PlanApprovalKind::Approved {
                name: "alice".to_string(),
            },
        };
        let lines = render_message(&approved, 80, &Theme::dark(), false);
        assert!(lines[0].plain_text().contains("Plan approved by alice"));
        assert_eq!(lines[0].spans[0].style.fg, Theme::dark().success);

        let rejected = RenderedMessage::PlanApproval {
            kind: tui_core::message::PlanApprovalKind::Rejected {
                name: "bob".to_string(),
                feedback: Some("needs tests".to_string()),
            },
        };
        let text = render_message(&rejected, 80, &Theme::dark(), false)
            .iter()
            .map(|l| l.plain_text())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains("Plan rejected by bob"));
        assert!(text.contains("needs tests"));
    }

    #[test]
    fn tool_use_renders_header_and_truncated_args() {
        let m = RenderedMessage::AssistantToolUse {
            id: protocol::ToolUseId::new(),
            tool: "Read".to_string(),
            input: serde_json::json!({"file_path": "src/lib.rs"}),
        };
        let lines = render_message(&m, 80, &Theme::dark(), false);
        assert_eq!(lines.len(), 2);
        assert!(lines[0].plain_text().contains("Read"));
        assert!(lines[1].plain_text().contains("file_path"));
    }

    #[test]
    fn verbose_expands_thinking_tooluse_and_groups() {
        // Thinking: collapsed shows a hint; verbose shows the body.
        let think = RenderedMessage::AssistantThinking {
            thinking: "step one\nstep two".to_string(),
            expanded: false,
        };
        let collapsed = render_message(&think, 80, &Theme::dark(), false);
        assert_eq!(collapsed.len(), 1);
        assert!(collapsed[0].plain_text().contains("ctrl+o"));
        let expanded = render_message(&think, 80, &Theme::dark(), true);
        let text = expanded
            .iter()
            .map(|l| l.plain_text())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains("step one") && text.contains("step two"));

        // Tool use: verbose pretty-prints the JSON over multiple lines.
        let tool = RenderedMessage::AssistantToolUse {
            id: protocol::ToolUseId::new(),
            tool: "Read".to_string(),
            input: serde_json::json!({"file_path": "src/lib.rs", "limit": 10}),
        };
        assert!(render_message(&tool, 80, &Theme::dark(), true).len() > 2);

        // Grouped tool use: collapsed is header-only; verbose lists children.
        let group = RenderedMessage::GroupedToolUse {
            tool: "Read".to_string(),
            group_id: protocol::ToolUseId::new(),
            entries: vec![
                (serde_json::json!({"f": "a"}), serde_json::json!("ok")),
                (serde_json::json!({"f": "b"}), serde_json::json!("ok")),
            ],
        };
        assert_eq!(render_message(&group, 80, &Theme::dark(), false).len(), 1);
        assert_eq!(render_message(&group, 80, &Theme::dark(), true).len(), 3);
    }

    #[test]
    fn tool_result_prefers_string_content() {
        let m = RenderedMessage::UserToolResult {
            id: protocol::ToolUseId::new(),
            tool: "Read".to_string(),
            result: serde_json::json!({"content": "hello world"}),
            old_string: None,
            new_string: None,
            file_path: None,
        };
        let lines = render_message(&m, 80, &Theme::dark(), false);
        assert_eq!(lines.len(), 1);
        assert!(lines[0].plain_text().contains("hello world"));
    }

    #[test]
    fn bash_and_command_variants_render() {
        let bash = RenderedMessage::UserBashInput {
            command: "ls -la".to_string(),
        };
        assert!(render_message(&bash, 80, &Theme::dark(), false)[0]
            .plain_text()
            .contains("ls -la"));

        let cmd = RenderedMessage::UserCommand {
            command: "help".to_string(),
            args: String::new(),
            is_skill: false,
        };
        assert_eq!(
            render_message(&cmd, 80, &Theme::dark(), false)[0].plain_text(),
            "/help"
        );
    }

    #[test]
    fn rate_limit_and_shutdown_render() {
        let rl = RenderedMessage::RateLimit {
            text: "Rate limited".to_string(),
            upsell: Some("Upgrade".to_string()),
        };
        let lines = render_message(&rl, 80, &Theme::dark(), false);
        assert_eq!(lines.len(), 2);
        assert!(lines[0].plain_text().contains("Rate limited"));
        assert!(lines[1].plain_text().contains("Upgrade"));

        let sd = RenderedMessage::Shutdown {
            from: "worker-1".to_string(),
            reason: Some("done".to_string()),
            rejected: false,
        };
        assert!(render_message(&sd, 80, &Theme::dark(), false)[0]
            .plain_text()
            .contains("worker-1"));
    }
}
