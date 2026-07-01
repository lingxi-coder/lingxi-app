//! `VirtualMessageList` — windowed scrollback (M7-03).
//!
//! Replaces M6's capped-500 [`Scrollback`](crate::components::scrollback).
//! The full message log is retained; only the lines intersecting the
//! viewport (+ overscan) are rendered each frame.
//!
//! ## Scroll model (LINES, not message rows)
//! - `scroll_offset` = lines scrolled up from the bottom.
//! - `offset = 0` → the last `viewport_height` lines (latest) are shown.
//! - `max_offset = total_lines.saturating_sub(viewport_height)` shows the
//!   oldest content.
//!
//! ## Height cache
//! A [`HeightCache`] maps message-index → rendered line count at a given
//! viewport width. Variable-height messages (a 1-line text vs a 200-line
//! diff) make a line-based model mandatory. The cache is invalidated and
//! recomputed whenever the viewport width changes.

use std::collections::HashMap;

use iocraft::prelude::*;
use protocol::ToolUseId;
use unicode_width::UnicodeWidthStr;

use crate::state::RenderedMessage;

// Re-export the per-variant message dispatch from `scrollback` so the
// windowed renderer reuses the exact same per-variant rendering
// (including M7-02's StructuredDiff branch) without duplicating it.
pub use crate::components::scrollback::render_message;

/// Overscan: render this many extra lines above and below the viewport so
/// a fast line-step doesn't flash blank rows.
pub const OVERSCAN_LINES: usize = 3;

/// Measure the rendered height (line count) of one message at a given
/// viewport width. Counts explicit `\n`-separated rows and adds wrap rows
/// for any line wider than `width` (unicode display columns). A message
/// always occupies at least one line.
///
/// # Invariant: this MUST track [`render_message`]'s actual output
/// The whole scroll model is line-based, so the height this returns drives
/// `scroll_offset`/window math. It is computed from the text proxy in
/// [`render_text_for_measure`], which reconstructs per-variant text rather
/// than reading what [`render_message`] actually draws. The two can DRIFT:
/// if a renderer changes how a variant is laid out (extra prefix rows,
/// truncation, expanded JSON, a multi-line diff body) without the proxy
/// being updated to match, the scroll math desyncs from the rendered
/// output (rows skipped/duplicated at the viewport edges).
///
/// Therefore: **any change to a `render_message` variant's line layout — or
/// any new [`RenderedMessage`] variant — REQUIRES updating
/// [`render_text_for_measure`] to match, and updating the
/// `measured_height_pins_*` lock tests in this module.** The richer
/// per-variant renderers in M7-04/05 will eventually unify measurement with
/// rendering; until then this proxy + its lock tests are the guardrail.
#[must_use]
pub fn measured_height(msg: &RenderedMessage, width: usize) -> usize {
    // §A4 empty-message guard: a user-text body that is only stripped
    // prompt-XML tags (or `(no content)`) renders as an EMPTY View (zero rows)
    // in `UserTextMessage` — so it must measure as 0, not the 1 the
    // empty-string branch below would otherwise return. Mirrors claude-code's
    // `return null` for these bodies.
    if let RenderedMessage::UserText { body, .. } = msg {
        if crate::components::messages::text_guard::is_empty_message_text(body) {
            return 0;
        }
    }
    let text = render_text_for_measure(msg, width);
    if text.is_empty() {
        return 1;
    }
    let w = width.max(1);
    let mut rows = 0usize;
    for line in text.split('\n') {
        let cols = UnicodeWidthStr::width(line);
        // A blank logical line still occupies one row.
        rows += (cols / w) + usize::from(cols % w != 0 || cols == 0);
    }
    rows.max(1)
}

/// Project a message to the plain text used for height measurement. This
/// mirrors what each renderer prints to screen at the line level (prefixes
/// add columns but not rows for these single-line-prefixed variants).
///
/// # MUST stay in lock-step with [`render_message`]
/// This is a measurement *proxy*: it reconstructs the text each variant
/// renders so [`measured_height`] can count rows without a terminal. It is
/// NOT derived from the real render path (which yields an iocraft element,
/// not text), so it can silently drift from what `render_message` draws.
/// When you add a [`RenderedMessage`] variant, or change how an existing
/// variant lays out rows in `render_message` / its per-variant component,
/// you MUST update this function to match and extend the
/// `measured_height_pins_*` lock tests below. Drift here corrupts the
/// line-based scroll math.
#[allow(clippy::too_many_lines)] // one arm per RenderedMessage variant (28 variants)
fn render_text_for_measure(msg: &RenderedMessage, width: usize) -> String {
    match msg {
        RenderedMessage::UserText { body, .. } | RenderedMessage::SystemText { body, .. } => {
            body.clone()
        }
        // Measurement == render: route through the markdown-flattening oracle
        // (marker + 2-col continuation indent) so the height cache counts the
        // SAME rows the component draws. A raw `body.clone()` over-counts
        // dropped ``` fence rows / trailing blanks once markdown is applied.
        // (A2) Pass the SAME width the component renders at so a markdown table
        // flattens to the identical row count.
        RenderedMessage::AssistantText { body, .. } => {
            crate::components::messages::assistant_text::render_assistant_text_to_string(body, width)
        }
        RenderedMessage::AssistantToolUse { tool, .. } => format!("● {tool}(…)"),
        // (gap-3) A live Bash result is the structured `{"stdout":…}` object;
        // the renderer extracts stdout/stderr through the bash-output span
        // pipeline (ANSI-stripped), so measure the SAME parsed body — otherwise
        // height counts the raw single-line JSON while the component draws the
        // multi-line output. Replay / non-Bash results keep the raw-payload
        // proxy (bare string verbatim, else compact JSON).
        RenderedMessage::UserToolResult { tool, result, .. } => {
            if tool == "Bash" {
                if let Some((stdout, stderr)) =
                    crate::components::messages::user_tool_result::bash_structured_output(result)
                {
                    return crate::components::messages::bash_output::render_bash_output_spans(
                        &stdout, &stderr,
                    )
                    .into_iter()
                    .map(|s| s.text)
                    .collect::<String>();
                }
            }
            result
                .as_str()
                .map_or_else(|| result.to_string(), str::to_string)
        }
        // ---- (M7-04) batch-1 system/assistant renderers ----------------
        // Each arm reproduces the line layout that `render_message` draws for
        // the variant (via its `render_*_to_string` pure renderer). Markers
        // (`∴ `/`✻ `/`● `) add columns, not rows. Kept in lock-step with the
        // renderers in `components::messages::*`; pinned by the
        // `measured_height_pins_*_m7_04` lock tests below.
        // Measurement == render by construction: route through the renderer's
        // own string oracle so the expanded body counts the SAME
        // markdown-FLATTENED lines the component draws (the raw `thinking`
        // text over-counts dropped ``` fence rows / trailing blanks). Covers
        // the collapsed header+hint line too.
        RenderedMessage::AssistantThinking { thinking, expanded } => {
            crate::components::messages::thinking::render_thinking_to_string(
                crate::components::messages::thinking::ThinkingProps {
                    thinking: thinking.clone(),
                    expanded: *expanded,
                },
            )
        }
        // Single dim+italic line `✻ Thinking…`.
        RenderedMessage::AssistantRedactedThinking => "\u{273B} Thinking\u{2026}".to_string(),
        // (compact-boundary-marginy) dim boundary line + blank row above/below
        // (counts not rendered — claude-code parity).
        RenderedMessage::CompactBoundary { .. } => {
            "\n\u{273B} Conversation compacted (ctrl+o for history)\n".to_string()
        }
        // Info → body verbatim; warning/error → `● ` marker (cols) + body.
        RenderedMessage::SystemTextRich { body, level } => match level {
            crate::state::SystemLevel::Info => body.clone(),
            crate::state::SystemLevel::Warning | crate::state::SystemLevel::Error => {
                format!("\u{25CF} {body}")
            }
        },
        // error body (+ optional `…` + expand-hint line when truncated) then a
        // retry-countdown footer line.
        RenderedMessage::SystemApiError {
            error,
            retry_attempt,
            retry_in_seconds,
            max_retries,
            truncated,
        } => {
            let mut out = error.clone();
            if *truncated {
                out.push('\u{2026}');
                out.push('\n');
                out.push_str("(ctrl+o to expand)");
            }
            let unit = if *retry_in_seconds == 1 {
                "second"
            } else {
                "seconds"
            };
            out.push('\n');
            out.push_str(&format!(
                "Retrying in {retry_in_seconds} {unit}\u{2026} (attempt {retry_attempt}/{max_retries})"
            ));
            out
        }
        // (rate-limit-missing-gutter) `  ⎿  ` gutter + error text + optional
        // dim upsell line (indented to match).
        RenderedMessage::RateLimit { text, upsell } => {
            crate::components::messages::rate_limit::render_rate_limit_to_string(
                crate::components::messages::rate_limit::RateLimitProps {
                    text: text.clone(),
                    upsell: upsell.clone(),
                },
            )
        }
        // header + optional `Reason:` line + (rejected) tail line.
        RenderedMessage::Shutdown {
            from,
            reason,
            rejected,
        } => {
            let mut out = if *rejected {
                format!("Shutdown rejected by {from}")
            } else {
                format!("Shutdown request from {from}")
            };
            if let Some(r) = reason {
                out.push('\n');
                out.push_str(&format!("Reason: {r}"));
            }
            if *rejected {
                out.push('\n');
                out.push_str(
                    "Teammate is continuing to work. You may request shutdown again later.",
                );
            }
            out
        }
        // Measurement == render by construction: route through the renderer's
        // own string oracle. The verbose `Result` body counts the SAME
        // markdown-FLATTENED lines the component draws (raw `text` over-counts
        // dropped ``` fence rows / trailing blanks); the other kinds remain
        // their fixed one-liners.
        RenderedMessage::Advisor { kind, verbose } => {
            crate::components::messages::advisor::render_advisor_to_string(
                crate::components::messages::advisor::AdvisorProps {
                    kind: kind.clone(),
                    verbose: *verbose,
                    ..Default::default()
                },
            )
        }
        // (hook-progress-missing-gutter) `  ⎿  ` gutter + single dim line.
        RenderedMessage::HookProgress {
            event,
            count,
            transcript_summary,
        } => crate::components::messages::hook_progress::render_hook_progress_to_string(
            crate::components::messages::hook_progress::HookProgressProps {
                event: event.clone(),
                count: *count,
                transcript_summary: *transcript_summary,
                ..Default::default()
            },
        ),
        // Mirrors `render_plan_approval_to_string`.
        RenderedMessage::PlanApproval { kind } => match kind {
            crate::state::PlanApprovalKind::Request {
                from,
                plan_content,
                plan_file_path,
            } => {
                let mut out = format!("Plan Approval Request from {from}\n");
                out.push_str(plan_content);
                if let Some(p) = plan_file_path {
                    out.push('\n');
                    out.push_str(&format!("Plan file: {p}"));
                }
                out
            }
            crate::state::PlanApprovalKind::Approved { name } => {
                format!("\u{2713} Plan Approved by {name}\nYou can now proceed with implementation. Your plan mode restrictions have been lifted.")
            }
            crate::state::PlanApprovalKind::Rejected { name, feedback } => {
                let mut out = format!("\u{2717} Plan Rejected by {name}");
                if let Some(f) = feedback {
                    out.push('\n');
                    out.push_str(&format!("Feedback: {f}"));
                }
                out.push('\n');
                out.push_str(
                    "Please revise your plan based on the feedback and call ExitPlanMode again.",
                );
                out
            }
        },
        // ---- (M7-05) batch-2 user renderers --------------------------------
        //
        // Each arm reproduces the line layout `render_message` draws for the
        // variant. For markdown/ANSI-bodied variants (plan, local-command
        // output, bash output) the arm routes THROUGH the renderer's own string
        // oracle so measurement counts the SAME flattened/parsed lines the
        // component draws (raw bodies over-count dropped ``` fence rows /
        // trailing blanks / ANSI escapes). Pinned by the
        // `measured_height_pins_*_m7_05` lock tests below.
        RenderedMessage::UserBashInput { command } => {
            crate::components::messages::bash_input::render_bash_input_to_string(command)
        }
        // Measurement == render: route through the ANSI span pipeline and rejoin
        // the span texts so the row count matches the parsed (escape-stripped)
        // body the component draws.
        RenderedMessage::UserBashOutput { stdout, stderr } => {
            crate::components::messages::bash_output::render_bash_output_spans(stdout, stderr)
                .into_iter()
                .map(|s| s.text)
                .collect::<String>()
        }
        RenderedMessage::UserCommand {
            command,
            args,
            is_skill,
        } => {
            crate::components::messages::command::render_command_to_string(command, args, *is_skill)
        }
        // Measurement == render: route through the markdown-flattening oracle so
        // the gutter-prefixed body counts the SAME rows the component draws.
        RenderedMessage::UserLocalCommandOutput { stdout, stderr } => {
            crate::components::messages::local_command_output::render_local_output_to_string(
                stdout, stderr,
            )
        }
        RenderedMessage::UserMemoryInput { input } => {
            crate::components::messages::memory_input::render_memory_to_string(input)
        }
        // Measurement == render: route through the plan markdown oracle (header
        // + flattened body); the round border adds no body rows here.
        RenderedMessage::UserPlan { plan_content } => {
            crate::components::messages::plan::render_plan_to_string(plan_content)
        }
        RenderedMessage::UserPrompt { text } => {
            crate::components::messages::prompt::render_prompt_to_string(text)
        }
        RenderedMessage::UserResourceUpdate { updates } => {
            let parsed: Vec<crate::components::messages::resource_update::ResourceUpdate> = updates
                .iter()
                .map(
                    |(s, t, r)| crate::components::messages::resource_update::ResourceUpdate {
                        server: s.clone(),
                        target: t.clone(),
                        reason: r.clone(),
                    },
                )
                .collect();
            crate::components::messages::resource_update::render_resource_update_to_string(&parsed)
        }
        RenderedMessage::UserImage { image_id, metadata, .. } => {
            crate::components::messages::image::render_image_label(*image_id, metadata.as_deref())
        }
        RenderedMessage::Attachment { attachment } => {
            crate::components::messages::attachment::render_attachment_to_string(attachment)
        }
        // Collapsed → header line only; expanded → header + children (the proxy
        // mirrors `render_grouped_to_string`'s default-collapsed string since
        // `measured_height`/the cache do not have the per-id expanded flag here;
        // matches the M7-03 proxy convention for fold variants).
        RenderedMessage::GroupedToolUse { tool, entries, .. } => {
            crate::components::messages::grouped_tool_use::render_grouped_to_string(
                tool, entries, false,
            )
        }
        RenderedMessage::CollapsedReadSearch {
            search_count,
            read_count,
            list_count,
            is_active,
            mem_read,
            mem_search,
            mem_write,
            ..
        } => {
            let counts = crate::components::messages::collapsed_read_search::CollapsedCounts {
                search: *search_count,
                read: *read_count,
                list: *list_count,
                is_active: *is_active,
                mem_read: *mem_read,
                mem_search: *mem_search,
                mem_write: *mem_write,
            };
            crate::components::messages::collapsed_read_search::render_collapsed_to_string(
                &counts,
                &[],
                false,
            )
        }
        RenderedMessage::TaskAssignment {
            task_id,
            assigned_by,
            subject,
            description,
        } => crate::components::messages::task_assignment::render_task_assignment_to_string(
            crate::components::messages::task_assignment::TaskAssignmentProps {
                task_id: task_id.clone(),
                assigned_by: assigned_by.clone(),
                subject: subject.clone(),
                description: description.clone(),
                theme: crate::theme::Theme::dark(),
            },
        ),
        RenderedMessage::AgentNotification { summary, status } => {
            crate::components::messages::user_agent_notification::render_user_agent_notification_to_string(
                crate::components::messages::user_agent_notification::UserAgentNotificationProps {
                    summary: summary.clone(),
                    status: status.clone(),
                    theme: crate::theme::Theme::dark(),
                },
            )
        }
        RenderedMessage::ChannelMessage { server, user, content } => {
            crate::components::messages::user_channel::render_user_channel_to_string(
                crate::components::messages::user_channel::UserChannelProps {
                    server: server.clone(),
                    user: user.clone(),
                    content: content.clone(),
                    theme: crate::theme::Theme::dark(),
                },
            )
        }
        RenderedMessage::UserTeammate { display_name, color, kind } => {
            crate::components::messages::user_teammate::render_user_teammate_to_string(
                crate::components::messages::user_teammate::UserTeammateProps {
                    display_name: display_name.clone(),
                    color: color.clone(),
                    kind: kind.clone(),
                    theme: crate::theme::Theme::dark(),
                },
            )
        }
    }
}

/// Per-message rendered-height cache. Maps message-index → line count at a
/// fixed viewport width. Backs all scroll-offset math. Rebuilt on width
/// change; appended to as new messages arrive (callers may simply rebuild
/// — `build` is O(n) over a 5k log and runs at most once per width change).
///
/// Alongside the per-message `heights`, the cache keeps a **prefix sum** of
/// cumulative line offsets (`prefix[i]` = absolute start line of message
/// `i`; `prefix[len] == total`). The prefix array makes two operations
/// O(1)/O(log n) instead of O(n):
/// - [`Self::span_start`] — the absolute start line of a message (O(1)).
/// - [`Self::message_index_at_line`] — the first message whose span
///   contains a given line, via binary search (O(log n)). This is what lets
///   [`render_window`] skip directly to the window start instead of walking
///   the whole log from index 0.
#[derive(Debug, Clone, Default)]
pub struct HeightCache {
    heights: Vec<usize>,
    /// Cumulative line offsets. `prefix.len() == heights.len() + 1`;
    /// `prefix[i]` is the absolute start line of message `i`, and
    /// `prefix[heights.len()] == total`. Empty caches keep `prefix == [0]`
    /// is NOT guaranteed — `Default` yields an empty `Vec`; treat an empty
    /// `prefix` as "no messages" (`span_start`/lookups fall back to 0/total).
    prefix: Vec<usize>,
    total: usize,
    width: usize,
}

impl HeightCache {
    /// Build the cache for `messages` at `width` columns.
    #[must_use]
    pub fn build(messages: &[RenderedMessage], width: usize) -> Self {
        let heights: Vec<usize> = messages.iter().map(|m| measured_height(m, width)).collect();
        // Prefix sum: prefix[i] = sum(heights[0..i]); prefix.len() == n + 1.
        let mut prefix = Vec::with_capacity(heights.len() + 1);
        let mut acc = 0usize;
        prefix.push(0);
        for &h in &heights {
            acc += h;
            prefix.push(acc);
        }
        Self {
            heights,
            prefix,
            total: acc,
            width,
        }
    }

    /// Rebuild in place at a new width (or after the log changed).
    pub fn recompute(&mut self, messages: &[RenderedMessage], width: usize) {
        *self = Self::build(messages, width);
    }

    /// Line count for the message at `index`, or 0 if out of range.
    #[must_use]
    pub fn height_at(&self, index: usize) -> usize {
        self.heights.get(index).copied().unwrap_or(0)
    }

    /// Absolute start line of the message at `index` (sum of all prior
    /// heights). O(1) via the prefix sum. Out-of-range indices clamp to
    /// `total_lines` (the line just past the end).
    #[must_use]
    pub fn span_start(&self, index: usize) -> usize {
        self.prefix.get(index).copied().unwrap_or(self.total)
    }

    /// First message index whose line span contains `line`, i.e. the
    /// message `i` with `span_start(i) <= line < span_start(i+1)`. O(log n)
    /// via binary search over the prefix sum.
    ///
    /// `line` is clamped to `[0, total_lines)`; for `line >= total_lines`
    /// (or an empty cache) this returns the last valid index (or 0 when
    /// empty). This is the entry point [`render_window`] uses to jump
    /// straight to the window start instead of scanning from index 0.
    #[must_use]
    pub fn message_index_at_line(&self, line: usize) -> usize {
        if self.heights.is_empty() {
            return 0;
        }
        // `prefix` is sorted ascending. `partition_point` finds the count of
        // entries `<= line`; the message starting at-or-before `line` is one
        // before that boundary. With prefix = [0, h0, h0+h1, …, total]:
        //   partition_point(p <= line) gives k where prefix[k-1] <= line.
        // The owning message index is k-1, clamped to the last message.
        let k = self.prefix.partition_point(|&p| p <= line);
        k.saturating_sub(1).min(self.heights.len() - 1)
    }

    /// Sum of all message heights (total rendered lines).
    #[must_use]
    pub fn total_lines(&self) -> usize {
        self.total
    }

    /// Number of cached messages.
    #[must_use]
    pub fn len(&self) -> usize {
        self.heights.len()
    }

    /// True when no messages are cached.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.heights.is_empty()
    }

    /// Width the cache was last built for.
    #[must_use]
    pub fn width(&self) -> usize {
        self.width
    }
}

/// The result of windowing: which messages intersect the viewport and how
/// many lines of the first/last message to skip/take. `skip_top_lines` are
/// the lines of `first_index`'s message hidden above the viewport top;
/// `take_lines` is the total number of rendered lines the viewport holds
/// (after `skip_top_lines`), spanning `first_index..=last_index`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WindowSlice {
    /// First message index intersecting the viewport (inclusive).
    pub first_index: usize,
    /// Last message index intersecting the viewport (inclusive).
    pub last_index: usize,
    /// Lines of `first_index`'s message hidden above the viewport top.
    pub skip_top_lines: usize,
    /// Total visible line budget across the window.
    pub take_lines: usize,
    /// True when nothing is visible (empty log / zero viewport).
    pub empty: bool,
}

impl WindowSlice {
    /// Inclusive range of message indices in the window. Empty iterator
    /// when [`Self::is_empty`].
    pub fn indices(&self) -> impl Iterator<Item = usize> {
        let (lo, hi) = if self.empty {
            (1usize, 0usize) // empty range
        } else {
            (self.first_index, self.last_index)
        };
        lo..=hi
    }

    /// True when the window holds no messages.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.empty
    }
}

/// Core windowing function. Given the full `messages`, their `cache`d
/// heights, a line-based `scroll_offset`, and the `viewport_height` in
/// lines, return the contiguous message slice intersecting the viewport
/// plus the first-message top-skip and the total visible line budget.
///
/// `scroll_offset` is clamped here defensively, but callers
/// ([`crate::app::scroll_with_viewport`]) clamp it on input.
///
/// ## Complexity — O(log n + window), NOT O(total)
/// The first visible message is located by binary search over the cache's
/// prefix sum ([`HeightCache::message_index_at_line`]); from there we walk
/// forward only while messages still intersect the viewport. The number of
/// messages touched is therefore proportional to the **window size**, not
/// the log length — at the default bottom-anchored `offset == 0` over a 5k
/// log we touch the handful of tail messages, never all 5000. The earlier
/// implementation looped from index 0 (O(depth-from-top), i.e. O(n) at
/// `offset == 0`); the
/// [`gate_window_walk_is_sublinear`](self) gate guards against regressing
/// to that linear walk.
#[must_use]
pub fn render_window(
    messages: &[RenderedMessage],
    cache: &HeightCache,
    scroll_offset: usize,
    viewport_height: usize,
) -> WindowSlice {
    render_window_counted(messages, cache, scroll_offset, viewport_height).0
}

/// Like [`render_window`] but also returns the **number of messages the
/// forward walk visited** after the binary-search jump. This is the
/// load-bearing perf metric: it must be ~window-sized, never O(total). The
/// public `render_window` delegates here and discards the count; the
/// [`gate_window_walk_is_sublinear`](self) gate calls this directly and
/// asserts the count stays sub-linear, so a regression to a from-index-0
/// linear walk (which would visit ~`last_index + 1` messages) trips it.
#[must_use]
pub fn render_window_counted(
    messages: &[RenderedMessage],
    cache: &HeightCache,
    scroll_offset: usize,
    viewport_height: usize,
) -> (WindowSlice, usize) {
    if messages.is_empty() || viewport_height == 0 || cache.total_lines() == 0 {
        return (
            WindowSlice {
                empty: true,
                ..WindowSlice::default()
            },
            0,
        );
    }
    let total = cache.total_lines();
    let max_offset = total.saturating_sub(viewport_height);
    let offset = scroll_offset.min(max_offset);

    // The visible line range is [top_line, bottom_line) in absolute lines
    // from the top of the log.
    let bottom_line = total - offset;
    let top_line = bottom_line.saturating_sub(viewport_height);

    // O(log n): jump straight to the first message whose span contains
    // `top_line` (the first one intersecting the viewport) — no scan from 0.
    let first_index = cache.message_index_at_line(top_line);
    let skip_top_lines = top_line.saturating_sub(cache.span_start(first_index));

    // O(window): walk forward from `first_index` only while messages keep
    // intersecting [top_line, bottom_line). A message intersects iff its
    // `span_start < bottom_line`; we stop at the first that doesn't. This
    // touches exactly the messages in the window, not the tail of the log.
    // `walked` counts every message this loop inspects (including the one
    // that triggers the break) so the gate can assert the work is bounded.
    let mut last_index = first_index;
    let mut walked = 0usize;
    let n = messages.len();
    for i in first_index..n {
        walked += 1;
        if cache.span_start(i) >= bottom_line {
            break;
        }
        last_index = i;
    }

    (
        WindowSlice {
            first_index,
            last_index,
            skip_top_lines,
            take_lines: viewport_height.min(total),
            empty: false,
        },
        walked,
    )
}

/// Props for [`VirtualMessageList`]. Mirrors the M6 `ScrollbackProps`
/// surface plus the line-based viewport. The full `messages` log is
/// passed; the component windows it.
///
/// ## Height cache is threaded in, NOT rebuilt
/// The per-frame render path passes the already-width-synced
/// [`HeightCache`] from `AppState` (kept fresh by `root.rs`'s
/// `refresh_height_cache(viewport_width)`, called immediately before each
/// render). The component does **not** call [`HeightCache::build`] — doing
/// so would be O(total messages) every frame and would defeat the whole
/// point of windowing. With the cache threaded in, the per-frame component
/// cost is O(window + log n).
///
/// `cache` MUST be consistent with `messages` (same length) and built for
/// `viewport_width`; `root.rs` guarantees this by calling
/// `refresh_height_cache` with the live viewport width before render.
#[derive(Default, Props)]
pub struct VirtualMessageListProps {
    /// Full retained message log (clone of `AppState::messages`).
    pub messages: Vec<RenderedMessage>,
    /// Pre-built, width-synced height cache (clone of
    /// `AppState::height_cache`). Backs the O(log n + window) windowing —
    /// the component never rebuilds it.
    pub cache: HeightCache,
    /// Line-based scroll offset (0 = latest at bottom).
    pub scroll_offset: usize,
    /// Viewport height in lines.
    pub viewport_height: usize,
    /// Per-tool expanded flags (clone of `AppState::expanded`).
    pub expanded: HashMap<ToolUseId, bool>,
    /// Focused tool id (clone of `AppState::focused_tool_id`).
    pub focused_tool_id: Option<ToolUseId>,
    /// (M7-15) Active render palette (clone of `AppState::theme`). Threaded
    /// into every windowed `render_message` so messages recolor on theme
    /// change.
    pub theme: crate::theme::Theme,
    /// (M7-15) Active theme name (clone of `AppState::theme_setting.resolve()`)
    /// — drives syntect-colored diffs.
    pub theme_name: crate::theme::ThemeName,
}

/// Windowed scrollback component. Renders only the messages whose line
/// spans intersect the viewport (+ overscan), not the whole log.
///
/// Per-frame cost is O(window + log n): the height cache is threaded in
/// (already built once per width change, never rebuilt here), and
/// [`render_window`] locates the window start via binary search.
#[component]
pub fn VirtualMessageList(props: &VirtualMessageListProps) -> impl Into<AnyElement<'static>> {
    // Overscan: render a few extra lines of viewport so a line-step does
    // not flash blank rows. Purely a visual buffer — offset math is exact.
    let vh = props.viewport_height.saturating_add(OVERSCAN_LINES);
    let win = render_window(&props.messages, &props.cache, props.scroll_offset, vh);

    let expanded = props.expanded.clone();
    let focused_tool_id = props.focused_tool_id.clone();
    let theme = props.theme;
    let theme_name = props.theme_name;
    // (ma-02) Resolution map: every tool result in the FULL log (not just the
    // window — a tool-use block can be visible while its result paginates in)
    // keyed by `tool_use_id` → `is_error`. Drives the `●` dot color
    // (claude-code `ToolUseLoader`: dim unresolved / green success / red error).
    let resolved: std::collections::HashMap<ToolUseId, bool> = props
        .messages
        .iter()
        .filter_map(|m| match m {
            crate::state::RenderedMessage::UserToolResult { id, result, .. } => Some((
                id.clone(),
                crate::components::messages::user_tool_result::tool_result_error(result).is_some(),
            )),
            _ => None,
        })
        .collect();
    // (A2) Thread the cache's render width into each message so markdown TABLES
    // in assistant bodies lay out to the live viewport width. The cache was
    // built at this same width, so measurement and render agree.
    let width = props.cache.width();
    let rendered: Vec<AnyElement<'static>> = if win.is_empty() {
        Vec::new()
    } else {
        win.indices()
            .filter_map(|i| props.messages.get(i).cloned())
            .map(|m| {
                render_message(
                    m,
                    &expanded,
                    focused_tool_id.clone(),
                    theme,
                    theme_name,
                    width,
                    &resolved,
                )
            })
            .collect()
    };
    element! {
        View(flex_direction: FlexDirection::Column, flex_grow: 1.0, overflow: Overflow::Hidden) {
            #(rendered)
        }
    }
}

#[cfg(test)]
#[path = "virtual_message_list_test.rs"]
mod virtual_message_list_test;
