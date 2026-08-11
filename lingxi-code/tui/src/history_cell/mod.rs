//! Transcript history cells (plan Phase 3).
//!
//! Modeled on codex-rs `tui/src/history_cell/mod.rs` — the UI architecture
//! pattern only, over `LingXi`'s own data model: a [`HistoryCell`] is the unit
//! of conversation history. Committed cells are inserted into the terminal's
//! native scrollback exactly once; the transient active (in-flight) cell can
//! mutate in place while streaming and is rendered as the live tail until it
//! is finalized (see [`crate::transcript::Transcript`]).
//!
//! Every `RenderedMessage` variant renders through a concrete per-variant
//! cell (plan Phase 9, "message cells"): [`message`] (user/assistant text,
//! thinking/advisor blocks, prompt/command/bash/plan/memory echoes),
//! [`system`] (system text/rich/api-error/rate-limit/compact boundary),
//! [`tool`] (tool use/result, bash/local command output, grouped/collapsed
//! folds), [`team`] (task/notification/channel/teammate/shutdown/hook/plan
//! -approval), and [`attachments`] (attachment summaries, resource updates,
//! images). [`cell_for_message`] picks the concrete cell — the match is
//! exhaustive with no fallback, so a new `RenderedMessage` variant fails
//! compilation here until it gets a cell.

pub mod attachments;
pub mod message;
pub mod system;
pub mod team;
pub mod tool;

use std::any::Any;
use std::time::Instant;

use ratatui::text::{Line, Text};
use ratatui::widgets::{Paragraph, Wrap};
use tui_core::message::RenderedMessage;
use tui_core::render::{SpanStyle, StyleColor, StyledLine, StyledSpan};
use tui_core::theme::Theme;

/// How a [`HistoryCell`] renders its lines.
///
/// Folds codex's `HistoryRenderMode` (rich vs raw) together with `LingXi`'s
/// verbose/expanded toggle (Ctrl-O), since both select *how* the same cell
/// renders. The default is rich + collapsed — the pre-transcript behavior.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RenderMode {
    /// `true` → copy-friendly plain output ([`HistoryCell::raw_lines`])
    /// instead of rich styled output (codex "raw scrollback" mode).
    pub raw: bool,
    /// `true` → expand collapsible content (thinking bodies, tool-use JSON,
    /// grouped children). Toggled by Ctrl-O.
    pub verbose: bool,
}

/// A single renderable unit of conversation history.
///
/// Each cell produces logical [`Line`]s and reports how many viewport rows
/// those lines occupy at a given terminal width. The default height
/// implementation uses `Paragraph::line_count` with `Wrap { trim: false }`
/// (the codex height contract), which accounts for lines wider than the
/// viewport; concrete cells only override it when they apply layout logic
/// beyond what `Paragraph` captures.
pub trait HistoryCell: std::fmt::Debug + Send + Sync + Any {
    /// The styled lines for the chat surface at `width` columns, colored by
    /// `theme`, rendered per `mode` (rich/raw + verbose expansion).
    fn display_lines(&self, width: u16, theme: &Theme, mode: RenderMode) -> Vec<Line<'static>>;

    /// Copy-friendly plain (unstyled) logical lines for raw scrollback mode.
    fn raw_lines(&self) -> Vec<Line<'static>>;

    /// The number of viewport rows needed to render this cell at `width`.
    ///
    /// Measured with a fixed theme: themes only recolor spans — they never
    /// change text content or wrapping — so the row count is theme-invariant.
    fn desired_height(&self, width: u16, mode: RenderMode) -> u16 {
        Paragraph::new(Text::from(self.display_lines(width, &Theme::dark(), mode)))
            .wrap(Wrap { trim: false })
            .line_count(width)
            .try_into()
            .unwrap_or(u16::MAX)
    }

    /// Whether this cell has any visible output at `width` columns (cells that
    /// render to nothing are consumed by the commit cursor without inserting).
    fn is_visible(&self, width: u16) -> bool {
        self.desired_height(width, RenderMode::default()) > 0
    }

    /// Cache key for active cells whose rendering changes with elapsed time
    /// (spinners/animations): the live tail is re-rendered when the key
    /// changes. `None` (the default) means the cell is time-invariant.
    fn animation_key(&self, _now: Instant) -> Option<u64> {
        None
    }

    /// A raw terminal escape block this cell contributes to native scrollback
    /// BELOW its display lines when committed in rich mode (real inline
    /// images). `None` (the default) for text-only cells. The flusher
    /// reserves [`ScrollbackEscape::rows`] rows above the viewport and
    /// anchors the escape there (see
    /// [`crate::terminal::Terminal::insert_history_image`]).
    fn scrollback_escape(&self) -> Option<ScrollbackEscape> {
        None
    }

    /// Upcast for downcasting to the concrete cell type.
    ///
    /// Explicit instead of relying on `dyn HistoryCell` → `dyn Any` trait
    /// upcasting, which needs rustc ≥ 1.86 (> workspace MSRV 1.82).
    fn as_any(&self) -> &dyn Any;

    /// Mutable upcast for downcasting (in-place mutation of the active cell).
    fn as_any_mut(&mut self) -> &mut dyn Any;
}

/// A raw escape block a committed cell emits into native scrollback (real
/// inline images): `rows` terminal rows tall, printed after the cell's
/// display lines in rich mode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScrollbackEscape {
    /// How many scrollback rows the escape's output occupies (the flusher
    /// reserves exactly this much room above the viewport).
    pub rows: u16,
    /// The raw escape bytes to print at the reserved block's top-left.
    pub escape: String,
}

/// Renderer default width when a caller passes 0 columns (pre-split
/// `render_message` behavior, kept by every cell).
pub(crate) const DEFAULT_WIDTH: usize = 80;

/// Internal single-source render contract for the concrete cells: produce the
/// neutral [`StyledLine`]s for one message. The blanket [`HistoryCell`] impl
/// below derives rich/raw display lines, width defaulting, and downcasting
/// uniformly from it, so each cell only owns its variant's rendering.
pub(crate) trait StyledCell: std::fmt::Debug + Send + Sync + Any {
    /// The styled lines at `width` columns (never 0), colored by `theme`,
    /// with collapsible content expanded per `verbose`.
    fn styled_lines(&self, width: usize, theme: &Theme, verbose: bool) -> Vec<StyledLine>;

    /// See [`HistoryCell::scrollback_escape`] (forwarded by the blanket
    /// impl); `None` (the default) for text-only cells.
    fn scrollback_escape(&self) -> Option<ScrollbackEscape> {
        None
    }
}

impl<T: StyledCell> HistoryCell for T {
    fn display_lines(&self, width: u16, theme: &Theme, mode: RenderMode) -> Vec<Line<'static>> {
        if mode.raw {
            return self.raw_lines();
        }
        let width = if width == 0 {
            DEFAULT_WIDTH
        } else {
            usize::from(width)
        };
        self.styled_lines(width, theme, mode.verbose)
            .iter()
            .map(crate::render::styled_line_to_ratatui)
            .collect()
    }

    fn raw_lines(&self) -> Vec<Line<'static>> {
        // Plain text of the rich render at the renderer's default width; the
        // theme is irrelevant once styles are stripped.
        self.styled_lines(DEFAULT_WIDTH, &Theme::dark(), false)
            .iter()
            .map(|line| {
                Line::from(
                    line.spans
                        .iter()
                        .map(|span| span.text.as_str())
                        .collect::<String>(),
                )
            })
            .collect()
    }

    fn scrollback_escape(&self) -> Option<ScrollbackEscape> {
        StyledCell::scrollback_escape(self)
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

/// Convert one [`RenderedMessage`] into its concrete [`HistoryCell`]. Every
/// variant maps to a per-variant cell in [`message`]/[`system`]/[`tool`]/
/// [`team`]/[`attachments`] — the match is exhaustive with NO wildcard, so
/// adding a `RenderedMessage` variant breaks this build until a cell exists.
#[must_use]
pub fn cell_for_message(message: RenderedMessage) -> Box<dyn HistoryCell> {
    use RenderedMessage as M;
    match message {
        M::UserText { body, .. } => Box::new(message::UserTextCell::new(body)),
        M::AssistantText { body, .. } => Box::new(message::AssistantTextCell::new(body)),
        M::UserPrompt { text } => Box::new(message::UserPromptCell::new(text)),
        M::UserCommand {
            command,
            args,
            is_skill,
        } => Box::new(message::UserCommandCell::new(command, args, is_skill)),
        M::UserBashInput { command } => Box::new(message::UserBashInputCell::new(command)),
        M::AssistantThinking { thinking, .. } => Box::new(message::ThinkingCell::new(thinking)),
        M::AssistantRedactedThinking => Box::new(message::RedactedThinkingCell),
        M::Advisor { kind, verbose } => Box::new(message::AdvisorCell::new(kind, verbose)),
        M::UserPlan { plan_content } => Box::new(message::UserPlanCell::new(plan_content)),
        M::UserMemoryInput { input } => Box::new(message::UserMemoryInputCell::new(input)),
        M::SystemText { body, is_error, .. } => {
            Box::new(system::SystemTextCell::new(body, is_error))
        }
        M::SystemTextRich { body, level } => Box::new(system::SystemTextRichCell::new(body, level)),
        M::SystemApiError {
            error,
            retry_attempt,
            max_retries,
            ..
        } => Box::new(system::SystemApiErrorCell::new(
            error,
            retry_attempt,
            max_retries,
        )),
        M::RateLimit { text, upsell } => Box::new(system::RateLimitCell::new(text, upsell)),
        M::CompactBoundary { summary, .. } => Box::new(system::CompactBoundaryCell::new(summary)),
        M::AssistantToolUse { tool, input, .. } => Box::new(tool::ToolUseCell::new(tool, input)),
        M::UserToolResult {
            tool,
            result,
            old_string,
            new_string,
            file_path,
            input,
            ..
        } => Box::new(tool::ToolResultCell::new(
            tool, result, old_string, new_string, file_path, input,
        )),
        M::UserBashOutput { stdout, stderr } | M::UserLocalCommandOutput { stdout, stderr } => {
            Box::new(tool::CommandOutputCell::new(stdout, stderr))
        }
        M::GroupedToolUse { tool, entries, .. } => {
            Box::new(tool::GroupedToolUseCell::new(tool, entries))
        }
        M::CollapsedReadSearch {
            search_count,
            read_count,
            list_count,
            repl_count,
            mcp_call_count,
            mcp_server_names,
            bash_count,
            is_active,
            latest_hint,
            entries,
            mem_write,
            ..
        } => Box::new(tool::CollapsedReadSearchCell::new(
            search_count,
            read_count,
            list_count,
            repl_count,
            mcp_call_count,
            mcp_server_names,
            bash_count,
            mem_write,
            is_active,
            latest_hint,
            entries,
        )),
        M::Shutdown { from, reason, .. } => Box::new(team::ShutdownCell::new(from, reason)),
        M::TaskAssignment {
            subject,
            description,
            ..
        } => Box::new(team::TaskAssignmentCell::new(subject, description)),
        M::AgentNotification { summary, .. } => Box::new(team::AgentNotificationCell::new(summary)),
        M::ChannelMessage {
            server,
            user,
            content,
        } => Box::new(team::ChannelMessageCell::new(server, user, content)),
        M::UserTeammate {
            display_name,
            color,
            kind,
        } => Box::new(team::UserTeammateCell::new(display_name, color, kind)),
        M::HookProgress { event, count, .. } => Box::new(team::HookProgressCell::new(event, count)),
        M::SubagentActivity { text } => Box::new(team::SubagentActivityCell::new(text)),
        M::PlanApproval { kind } => Box::new(team::PlanApprovalCell::new(kind)),
        M::UserResourceUpdate { updates } => {
            Box::new(attachments::UserResourceUpdateCell::new(updates))
        }
        M::UserImage {
            image_id,
            metadata,
            source_path,
        } => Box::new(attachments::UserImageCell::new(
            image_id,
            metadata,
            source_path,
        )),
        M::Attachment { attachment } => Box::new(attachments::AttachmentCell::new(attachment)),
    }
}

// ===== Shared styled-line primitives (moved from `message.rs` in the
// message-cells split; used by the cell modules AND the legacy renderer). =====

/// One [`StyledLine`] per `\n`-separated line, single-span colored `color`
/// (empty text → no lines).
pub(crate) fn colored_lines(text: &str, color: StyleColor) -> Vec<StyledLine> {
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

/// One unstyled [`StyledLine`] per `\n`-separated line (empty text → none).
pub(crate) fn plain_lines(text: &str) -> Vec<StyledLine> {
    if text.is_empty() {
        return Vec::new();
    }
    text.split('\n').map(StyledLine::plain).collect()
}

/// One-line truncation to `max` chars (newlines flattened to spaces).
pub(crate) fn truncate(s: &str, max: usize) -> String {
    let flat = s.replace('\n', " ");
    if flat.chars().count() <= max {
        flat
    } else {
        let cut: String = flat.chars().take(max.saturating_sub(1)).collect();
        format!("{cut}…")
    }
}

/// A dim-colored span in the active theme.
pub(crate) fn dim_span(text: String, theme: &Theme) -> StyledSpan {
    StyledSpan::styled(
        text,
        SpanStyle {
            fg: theme.dim,
            ..SpanStyle::default()
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assistant(body: &str) -> message::AssistantTextCell {
        message::AssistantTextCell::new(body.to_string())
    }

    /// Factory-routed cells render byte-identically to the pre-transcript
    /// flush path: `render_message` → `styled_line_to_ratatui`. (The full
    /// fixture sweep lives in `crate::message::tests::
    /// ported_cells_render_line_identical_to_render_message`.)
    #[test]
    fn display_lines_match_legacy_render_message_pipeline() {
        let theme = Theme::dark();
        let fixtures = [
            RenderedMessage::AssistantText {
                body: "Hello **world**\n\n- a\n- b".to_string(),
                timestamp: 0,
            },
            RenderedMessage::UserText {
                body: "hi there".to_string(),
                timestamp: 0,
            },
            RenderedMessage::SystemText {
                body: "boom".to_string(),
                timestamp: 0,
                is_error: true,
            },
            RenderedMessage::AssistantThinking {
                thinking: "pondering deeply".to_string(),
                expanded: false,
            },
        ];
        for message in fixtures {
            for verbose in [false, true] {
                let expected: Vec<Line<'static>> =
                    crate::message::render_message(&message, 80, &theme, verbose)
                        .iter()
                        .map(crate::render::styled_line_to_ratatui)
                        .collect();
                let cell = cell_for_message(message.clone());
                let got = cell.display_lines(
                    80,
                    &theme,
                    RenderMode {
                        raw: false,
                        verbose,
                    },
                );
                assert_eq!(got, expected, "diverged for {message:?} verbose={verbose}");
            }
        }
    }

    #[test]
    fn raw_mode_display_lines_are_plain_raw_lines() {
        let cell = assistant("Hello **bold**");
        let raw = cell.display_lines(
            80,
            &Theme::dark(),
            RenderMode {
                raw: true,
                verbose: false,
            },
        );
        assert_eq!(raw, cell.raw_lines());
        for line in &raw {
            for span in &line.spans {
                assert_eq!(
                    span.style,
                    ratatui::style::Style::default(),
                    "raw lines carry no styling: {span:?}"
                );
            }
        }
        // The text itself is preserved (markdown-rendered, styles dropped).
        let all: String = raw
            .iter()
            .flat_map(|l| l.spans.iter())
            .map(|s| s.content.as_ref())
            .collect();
        assert!(all.contains("Hello"), "got: {all}");
        assert!(all.contains("bold"), "got: {all}");
    }

    #[test]
    fn verbose_mode_expands_thinking_body() {
        let cell = cell_for_message(RenderedMessage::AssistantThinking {
            thinking: "secret reasoning".to_string(),
            expanded: false,
        });
        let theme = Theme::dark();
        let collapsed: String = cell
            .display_lines(80, &theme, RenderMode::default())
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(collapsed.contains("ctrl+o to expand"), "got: {collapsed}");
        assert!(!collapsed.contains("secret reasoning"), "got: {collapsed}");
        let expanded: String = cell
            .display_lines(
                80,
                &theme,
                RenderMode {
                    raw: false,
                    verbose: true,
                },
            )
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(expanded.contains("secret reasoning"), "got: {expanded}");
    }

    #[test]
    fn desired_height_counts_wrapped_rows_not_logical_lines() {
        // One 100-char logical line: 1 row at width 100+, 10 rows at width 10.
        let cell = system::SystemTextCell::new("a".repeat(100), false);
        // Width passes through to the renderer, but this variant does not
        // wrap by itself — the Paragraph measurement must account for it.
        assert_eq!(cell.desired_height(120, RenderMode::default()), 1);
        assert_eq!(cell.desired_height(10, RenderMode::default()), 10);
    }

    #[test]
    fn is_visible_reflects_empty_render() {
        let empty = message::UserTextCell::new(String::new());
        assert!(!empty.is_visible(80), "empty user text renders nothing");
        assert!(assistant("hello").is_visible(80));
    }

    /// `true` when the factory maps `message` to the concrete cell type `T`.
    fn maps_to<T: 'static>(message: RenderedMessage) -> bool {
        cell_for_message(message)
            .as_any()
            .downcast_ref::<T>()
            .is_some()
    }

    #[test]
    fn factory_maps_text_and_system_variants_to_concrete_cells() {
        assert!(maps_to::<message::UserTextCell>(
            RenderedMessage::UserText {
                body: "hi".into(),
                timestamp: 0,
            }
        ));
        assert!(maps_to::<message::AssistantTextCell>(
            RenderedMessage::AssistantText {
                body: "hi".into(),
                timestamp: 0,
            }
        ));
        assert!(maps_to::<message::UserPromptCell>(
            RenderedMessage::UserPrompt { text: "p".into() }
        ));
        assert!(maps_to::<message::UserCommandCell>(
            RenderedMessage::UserCommand {
                command: "help".into(),
                args: String::new(),
                is_skill: false,
            }
        ));
        assert!(maps_to::<message::UserBashInputCell>(
            RenderedMessage::UserBashInput {
                command: "ls".into(),
            }
        ));
        assert!(maps_to::<system::SystemTextCell>(
            RenderedMessage::SystemText {
                body: "s".into(),
                timestamp: 0,
                is_error: false,
            }
        ));
        assert!(maps_to::<system::SystemTextRichCell>(
            RenderedMessage::SystemTextRich {
                body: "s".into(),
                level: tui_core::message::SystemLevel::Info,
            }
        ));
        assert!(maps_to::<system::SystemApiErrorCell>(
            RenderedMessage::SystemApiError {
                error: "e".into(),
                retry_attempt: 1,
                retry_in_seconds: 1,
                max_retries: 3,
                truncated: false,
            }
        ));
        assert!(maps_to::<system::RateLimitCell>(
            RenderedMessage::RateLimit {
                text: "r".into(),
                upsell: None,
            }
        ));
        assert!(maps_to::<system::CompactBoundaryCell>(
            RenderedMessage::CompactBoundary {
                messages_before: 4,
                messages_after: 1,
                summary: "Summary:\nkept context".into(),
            }
        ));
        assert!(maps_to::<message::ThinkingCell>(
            RenderedMessage::AssistantThinking {
                thinking: "t".into(),
                expanded: false,
            }
        ));
        assert!(maps_to::<message::RedactedThinkingCell>(
            RenderedMessage::AssistantRedactedThinking
        ));
        assert!(maps_to::<message::AdvisorCell>(RenderedMessage::Advisor {
            kind: tui_core::message::AdvisorKind::RedactedResult,
            verbose: false,
        }));
        assert!(maps_to::<message::UserPlanCell>(
            RenderedMessage::UserPlan {
                plan_content: "p".into(),
            }
        ));
        assert!(maps_to::<message::UserMemoryInputCell>(
            RenderedMessage::UserMemoryInput { input: "m".into() }
        ));
    }

    #[test]
    fn factory_maps_team_and_attachment_variants_to_concrete_cells() {
        assert!(maps_to::<team::ShutdownCell>(RenderedMessage::Shutdown {
            from: "w".into(),
            reason: None,
            rejected: false,
        }));
        assert!(maps_to::<team::TaskAssignmentCell>(
            RenderedMessage::TaskAssignment {
                task_id: "1".into(),
                assigned_by: "lead".into(),
                subject: "s".into(),
                description: None,
            }
        ));
        assert!(maps_to::<team::AgentNotificationCell>(
            RenderedMessage::AgentNotification {
                summary: "s".into(),
                status: None,
            }
        ));
        assert!(maps_to::<team::ChannelMessageCell>(
            RenderedMessage::ChannelMessage {
                server: "slack".into(),
                user: None,
                content: "c".into(),
            }
        ));
        assert!(maps_to::<team::UserTeammateCell>(
            RenderedMessage::UserTeammate {
                display_name: "n".into(),
                color: None,
                kind: tui_core::message::UserTeammateKind::Note {
                    summary: None,
                    content: None,
                    is_transcript_mode: false,
                },
            }
        ));
        assert!(maps_to::<team::HookProgressCell>(
            RenderedMessage::HookProgress {
                event: "PreToolUse".into(),
                count: 1,
                transcript_summary: true,
            }
        ));
        assert!(maps_to::<team::SubagentActivityCell>(
            RenderedMessage::SubagentActivity {
                text: "Read(/etc/hosts)".into(),
            }
        ));
        assert!(maps_to::<team::PlanApprovalCell>(
            RenderedMessage::PlanApproval {
                kind: tui_core::message::PlanApprovalKind::Approved { name: "a".into() },
            }
        ));
        assert!(maps_to::<attachments::AttachmentCell>(
            RenderedMessage::Attachment {
                attachment: tui_core::message::Attachment::Directory {
                    display_path: "src".into(),
                },
            }
        ));
        assert!(maps_to::<attachments::UserResourceUpdateCell>(
            RenderedMessage::UserResourceUpdate {
                updates: Vec::new(),
            }
        ));
        assert!(maps_to::<attachments::UserImageCell>(
            RenderedMessage::UserImage {
                image_id: None,
                metadata: None,
                source_path: None,
            }
        ));
    }

    #[test]
    fn factory_maps_tool_variants_to_concrete_cells() {
        assert!(maps_to::<tool::ToolUseCell>(
            RenderedMessage::AssistantToolUse {
                id: protocol::ToolUseId::new(),
                tool: "Read".into(),
                input: serde_json::json!({}),
            }
        ));
        assert!(maps_to::<tool::ToolResultCell>(
            RenderedMessage::UserToolResult {
                id: protocol::ToolUseId::new(),
                tool: "Read".into(),
                result: serde_json::json!("ok"),
                old_string: None,
                new_string: None,
                file_path: None,
                input: None,
            }
        ));
        assert!(maps_to::<tool::CommandOutputCell>(
            RenderedMessage::UserBashOutput {
                stdout: "o".into(),
                stderr: String::new(),
            }
        ));
        assert!(maps_to::<tool::CommandOutputCell>(
            RenderedMessage::UserLocalCommandOutput {
                stdout: "o".into(),
                stderr: String::new(),
            }
        ));
        assert!(maps_to::<tool::GroupedToolUseCell>(
            RenderedMessage::GroupedToolUse {
                tool: "Read".into(),
                group_id: protocol::ToolUseId::new(),
                entries: Vec::new(),
            }
        ));
        assert!(maps_to::<tool::CollapsedReadSearchCell>(
            RenderedMessage::CollapsedReadSearch {
                search_count: 0,
                read_count: 0,
                list_count: 0,
                repl_count: 0,
                mcp_call_count: 0,
                mcp_server_names: Vec::new(),
                bash_count: 0,
                is_active: false,
                group_id: protocol::ToolUseId::new(),
                latest_hint: None,
                entries: Vec::new(),
                mem_read: 0,
                mem_search: 0,
                mem_write: 0,
            }
        ));
    }

    #[test]
    fn animation_key_defaults_to_none_and_downcast_roundtrips() {
        let mut cell: Box<dyn HistoryCell> = cell_for_message(RenderedMessage::AssistantText {
            body: "hi".to_string(),
            timestamp: 0,
        });
        assert_eq!(
            cell.animation_key(Instant::now()),
            None,
            "static cells are time-invariant"
        );
        // as_any / as_any_mut expose the concrete cell for in-place mutation.
        cell.as_any_mut()
            .downcast_mut::<message::AssistantTextCell>()
            .expect("downcast")
            .append(" there");
        assert_eq!(
            cell.as_any()
                .downcast_ref::<message::AssistantTextCell>()
                .map(message::AssistantTextCell::body),
            Some("hi there")
        );
    }
}
