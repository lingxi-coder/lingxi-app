//! Per-variant history cells for the tool-side messages: assistant tool
//! calls, user tool results, bash/local command output, and the grouped /
//! collapsed read-search folds.
//!
//! Split out of `message.rs` in the message-cells phase (plan Phase 9). The
//! styled-line renderers here are the single source for these variants: the
//! cells consume them through [`StyledCell`], and the legacy
//! [`crate::message::render_message`] dispatcher delegates to them so its
//! output stays line-identical to the pre-split renderer.

use tui_core::render::ansi::parse_ansi;
use tui_core::render::{diff, SpanStyle, StyleColor, StyledLine, StyledSpan};
use tui_core::theme::{Theme, ThemeName};
use tui_core::tool_display;

use super::{colored_lines, dim_span, truncate, StyledCell};

/// Tool-call header marker (claude-code `figures.BLACK_CIRCLE`): `⏺` (U+23FA)
/// on macOS, `●` (U+25CF) elsewhere — platform-conditional at compile time,
/// mirroring `message::ASSISTANT_MARKER` and the iocraft
/// `assistant_tool_use::MARKER`. (The grouped-fold header stays an
/// unconditional `●` to match the byte-locked iocraft `grouped_tool_use`.)
const TOOL_MARKER: &str = if cfg!(target_os = "macos") {
    "\u{23FA} "
} else {
    "\u{25CF} "
};

/// A tool call: `⏺ {verb}({arg})` header + arguments.
///
/// The header is the shared, parameterized one from
/// `tui_core::tool_display::header` — `Update(src/host.rs)` rather than the
/// bare `Edit` this used to print. Collapsed → the header plus the tool's
/// sub-line (a Bash `$ command`) when it has one, or — when the header names
/// no argument AND there is no sub-line — the truncated input JSON, so a call
/// is never printed without its arguments; verbose → the pretty-printed JSON
/// input, dim-indented.
pub(crate) fn tool_use_lines(
    tool: &str,
    input: &serde_json::Value,
    theme: &Theme,
    verbose: bool,
) -> Vec<StyledLine> {
    let derived = tool_display::tool_header(tool, input);
    if tool == "LocalApp" && !verbose {
        return vec![StyledLine {
            spans: vec![dim_span(
                format!(
                    "▦ Local app{}",
                    derived
                        .primary
                        .as_ref()
                        .map(|primary| format!(" {}", truncate(primary, 100)))
                        .unwrap_or_default()
                ),
                theme,
            )],
        }];
    }
    let header_carries_an_argument = derived.primary.is_some();
    let header = StyledLine {
        spans: vec![
            StyledSpan::styled(
                if tool.eq_ignore_ascii_case("computer") {
                    "▣ ".to_string()
                } else {
                    TOOL_MARKER.to_string()
                },
                SpanStyle {
                    fg: theme.success,
                    ..SpanStyle::default()
                },
            ),
            StyledSpan::plain(derived.title()),
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
    } else if let Some(sub) = derived.sub_line {
        out.push(StyledLine {
            spans: vec![dim_span(
                format!("  {} {}", sub.prefix, truncate(&sub.text, 100)),
                theme,
            )],
        });
    } else if !header_carries_an_argument {
        // The pre-shared-header fallback, restored for the calls that lost it.
        // `sub_line` is set only for the shell family and `primary` only from
        // the header table / `GENERIC_PRIMARY_KEYS`, so a tool with NEITHER —
        // `TodoWrite`, or an MCP tool whose input carries none of the generic
        // keys — rendered with no argument information at all. Tools whose
        // header already names their argument (`Read(src/lib.rs)`) keep the
        // one-line form the shared header bought.
        out.push(StyledLine {
            spans: vec![dim_span(
                format!("  {}", truncate(&input.to_string(), 100)),
                theme,
            )],
        });
    }
    out
}

/// Presentation-only families. Questions, permissions, skills and unrelated MCP calls
/// never join a task fold. Discovery joins only for an explicit workflow selection.
pub(crate) fn activity_fold_key(tool: &str, input: &serde_json::Value) -> Option<&'static str> {
    let lower = tool.to_ascii_lowercase();
    if matches!(
        lower.as_str(),
        "agent" | "task" | "spawn_agent" | "fork_agent"
    ) {
        return Some("agents");
    }
    fn workflow(tool: &str) -> bool {
        matches!(
            tool.to_ascii_lowercase().replace('_', "").as_str(),
            "taskcreate"
                | "taskget"
                | "taskupdate"
                | "tasklist"
                | "todowrite"
                | "listchecks"
                | "updateplan"
                | "sendmessage"
        )
    }
    if workflow(tool) {
        return Some("tasks");
    }
    if tool == "ToolSearch" {
        let header = tool_display::tool_header(tool, input);
        if let Some(primary) = header.primary {
            let targets = primary
                .trim()
                .strip_prefix("select:")
                .unwrap_or(primary.trim());
            let names: Vec<_> = targets
                .split(|c: char| c.is_whitespace() || c == ',')
                .filter(|s| !s.is_empty())
                .collect();
            if !names.is_empty()
                && names
                    .iter()
                    .all(|name| workflow(name.trim_start_matches('+')))
            {
                return Some("tasks");
            }
        }
    }
    None
}

/// Shared live/replay projection. Interactive tools retain their own rows.
pub(crate) fn transcript_activity_key(
    tool: &str,
    input: &serde_json::Value,
) -> Option<&'static str> {
    activity_fold_key(tool, input)
        .map(|key| if key == "agents" { key } else { "activity" })
        .or_else(|| {
            let name = tool.to_ascii_lowercase();
            (!name.contains("permission")
                && !matches!(
                    name.as_str(),
                    "askuserquestion"
                        | "localapp"
                        | "enterplanmode"
                        | "exitplanmode"
                        | "submitplan"
                ))
            .then_some("activity")
        })
}

#[derive(Debug, Clone)]
pub(crate) struct ActivityEntry {
    pub id: String,
    pub tool: String,
    pub input: serde_json::Value,
    pub result: Option<serde_json::Value>,
}

/// One active contiguous fold. Ctrl-O exposes every original call/result; the
/// terminal scrollback receives the group once, when a visible boundary seals it.
#[derive(Debug, Clone)]
pub(crate) struct ActivityFoldCell {
    pub key: &'static str,
    pub reasoning: String,
    pub thinking: bool,
    pub sealed: bool,
    pub entries: Vec<ActivityEntry>,
}

fn fit_tool_text(text: &str, budget: usize) -> String {
    use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};
    let flat = text.replace(['\n', '\r', '\t'], " ");
    if flat.width() <= budget {
        return flat;
    }
    let mut out = String::new();
    let mut used = 0;
    for c in flat.chars() {
        let size = c.width().unwrap_or(0);
        if used + size > budget.saturating_sub(1) {
            break;
        }
        out.push(c);
        used += size;
    }
    if budget > 0 {
        out.push('…');
    }
    out
}

/// Wrap styled output inside a terminal panel without losing ANSI/diff colors.
fn tool_detail_panel(
    label: &str,
    details: Vec<StyledLine>,
    status: &str,
    width: usize,
    theme: &Theme,
) -> Vec<StyledLine> {
    use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};
    // Extremely narrow terminals retain the content and let Paragraph wrap it.
    if width < 6 {
        return details;
    }
    let inner = width - 4;
    let background = match theme.text {
        StyleColor::Rgb(0, 0, 0) => StyleColor::Rgb(242, 242, 244),
        StyleColor::Rgb(..) => StyleColor::Rgb(40, 40, 42),
        // ANSI themes honor the terminal's background and color capability.
        _ => StyleColor::Default,
    };
    let border = SpanStyle {
        fg: theme.dim,
        bg: background,
        ..SpanStyle::default()
    };
    let mut out = vec![StyledLine {
        spans: vec![StyledSpan::styled(
            format!("╭{}╮", "─".repeat(width - 2)),
            border,
        )],
    }];
    let mut framed = |spans: Vec<StyledSpan>, used: usize| {
        let mut row = vec![StyledSpan::styled("│ ", border)];
        row.extend(spans);
        row.push(StyledSpan::styled(
            format!("{} │", " ".repeat(inner - used)),
            border,
        ));
        out.push(StyledLine { spans: row });
    };
    for line in std::iter::once(StyledLine {
        spans: vec![dim_span(label.into(), theme)],
    })
    .chain(std::iter::once(StyledLine::empty()))
    .chain(details)
    .chain(std::iter::once(StyledLine::empty()))
    .chain(std::iter::once(StyledLine {
        spans: vec![dim_span(
            format!(
                "{}{status}",
                " ".repeat(inner.saturating_sub(status.width()))
            ),
            theme,
        )],
    })) {
        let mut row = Vec::new();
        let mut used = 0;
        for span in line.spans {
            let mut style = span.style;
            if style.bg == StyleColor::Default {
                style.bg = background;
            }
            if style.fg == StyleColor::Default {
                style.fg = theme.text;
            }
            let mut chunk = String::new();
            for c in span.text.replace('\t', "    ").chars() {
                let columns = c.width().unwrap_or(0);
                if used + columns > inner {
                    row.push(StyledSpan::styled(std::mem::take(&mut chunk), style));
                    framed(std::mem::take(&mut row), used);
                    used = 0;
                }
                chunk.push(c);
                used += columns;
            }
            if !chunk.is_empty() {
                row.push(StyledSpan::styled(chunk, style));
            }
        }
        framed(row, used);
    }
    out.push(StyledLine {
        spans: vec![StyledSpan::styled(
            format!("╰{}╯", "─".repeat(width - 2)),
            border,
        )],
    });
    // Keep every border inside the viewport even for wide Unicode output.
    debug_assert!(out.iter().all(|line| line.plain_text().width() <= width));
    out
}

/// Match only gate-refusal prefixes, not arbitrary stderr containing "permission denied".
fn tool_denial_reason(
    failed: bool,
    headline: Option<&str>,
    body: Option<&str>,
) -> Option<(String, bool)> {
    if !failed {
        return None;
    }
    let reason = body
        .map(str::trim)
        .filter(|body| !body.is_empty())
        .or(headline.map(str::trim))
        .unwrap_or("");
    let text = reason.to_ascii_lowercase();
    let prefixes = [
        "permission to use ",
        "permission denied:",
        "permission denied by ",
        "tool use denied",
        "user denied ",
        "automatic approval review failed",
        "command declined by auto-review",
    ];
    if text != "permission denied" && !prefixes.iter().any(|prefix| text.starts_with(prefix)) {
        return None;
    }
    Some((
        reason.into(),
        text.contains("auto-review")
            || text.contains("auto review")
            || text.contains("automatic approval review"),
    ))
}

fn single_tool_lines(
    entry: &ActivityEntry,
    width: usize,
    theme: &Theme,
    verbose: bool,
    sealed: bool,
) -> Vec<StyledLine> {
    use unicode_width::UnicodeWidthStr;
    let header = tool_display::tool_header(&entry.tool, &entry.input);
    let mut summary = header.title();
    if header.primary.is_none() {
        if let Some(sub) = &header.sub_line {
            summary.push_str(&format!(" · {} {}", sub.prefix, sub.text));
        } else if !entry.input.is_null()
            && entry
                .input
                .as_object()
                .is_none_or(|input| !input.is_empty())
        {
            summary.push_str(&format!(" · {}", entry.input));
        }
    }
    let failed = entry
        .result
        .as_ref()
        .is_some_and(tool_display::result_is_error);
    let headline = entry.result.as_ref().and_then(|result| {
        tool_display::result_headline(&entry.tool, Some(&entry.input), result, failed)
    });
    let body = entry
        .result
        .as_ref()
        .and_then(|result| {
            tool_display::result_body(&entry.tool, result).or_else(|| {
                entry
                    .tool
                    .eq_ignore_ascii_case("Skill")
                    .then(|| {
                        result
                            .get("body")
                            .and_then(serde_json::Value::as_str)
                            .map(str::to_owned)
                    })
                    .flatten()
            })
        })
        .map(|body| tool_display::result::clamp_body(&body));
    if let Some((reason, auto_review)) = tool_denial_reason(
        failed,
        headline.as_deref(),
        body.as_ref().map(|body| body.text.as_str()),
    ) {
        let title = if auto_review {
            "Command declined by auto-review"
        } else {
            "Command declined"
        };
        let mut lines = vec![StyledLine {
            spans: vec![dim_span(
                format!("◇ {title} {}", if verbose { "⌄" } else { "›" }),
                theme,
            )],
        }];
        if verbose {
            let mut details = Vec::new();
            if let Some(sub) = &header.sub_line {
                details.extend(parse_ansi(&format!("{} {}", sub.prefix, sub.text)));
            } else if let Some(primary) = &header.primary {
                details.extend(parse_ansi(primary));
            }
            details.push(StyledLine {
                spans: vec![dim_span("No output".into(), theme)],
            });
            let shell = matches!(
                entry.tool.to_ascii_lowercase().as_str(),
                "shell" | "bash" | "powershell"
            );
            lines.extend(tool_detail_panel(
                if shell { "Shell" } else { &header.label },
                details,
                if shell { "Exit code unknown" } else { "" },
                width,
                theme,
            ));
            lines.push(StyledLine {
                spans: vec![dim_span("Why it was denied".into(), theme)],
            });
            // Render the reason literally: terminal control sequences in a refusal are not instructions.
            lines.extend(reason.lines().map(|line| StyledLine {
                spans: vec![dim_span(
                    line.chars().filter(|c| !c.is_control()).collect(),
                    theme,
                )],
            }));
            if body.as_ref().is_some_and(|body| body.truncated) {
                lines.push(StyledLine {
                    spans: vec![dim_span("… output truncated".into(), theme)],
                });
            }
        }
        return lines;
    }
    let (old, new, path) = tool_display::diff_inputs_for(&entry.tool, &entry.input);
    let has_details = old.is_some()
        || new.is_some()
        || body.as_ref().is_some_and(|body| {
            let content = body.text.trim();
            !content.is_empty()
                && (body.truncated
                    || body.total_lines > 1
                    || content.contains(['\n', '\r'])
                    || Some(content) != headline.as_deref().map(str::trim))
        });
    if let Some(headline) = &headline {
        summary.push_str(" · ");
        summary.push_str(headline);
    } else if entry.result.is_none() && !sealed {
        summary.push_str(" · running");
    }
    let marker = if entry.tool.eq_ignore_ascii_case("computer") {
        "▣ "
    } else {
        TOOL_MARKER
    };
    let suffix = if has_details {
        if verbose {
            " ⌄"
        } else {
            " ›"
        }
    } else {
        ""
    };
    // A summary-only result stays complete, wrapping naturally in a narrow terminal.
    let shown = if has_details {
        fit_tool_text(
            &summary,
            width.saturating_sub(marker.width() + suffix.width()),
        )
    } else {
        summary.replace(['\n', '\r', '\t'], " ")
    };
    let mut lines = vec![StyledLine {
        spans: vec![StyledSpan::styled(
            format!("{marker}{shown}{suffix}"),
            SpanStyle {
                fg: if failed { theme.error } else { theme.dim },
                ..SpanStyle::default()
            },
        )],
    }];
    if verbose && has_details {
        let mut details = Vec::new();
        if let Some(sub) = &header.sub_line {
            details.extend(parse_ansi(&format!("{} {}", sub.prefix, sub.text)));
        } else {
            details.extend(
                tool_use_lines(&entry.tool, &entry.input, theme, true)
                    .into_iter()
                    .skip(1)
                    .map(|mut line| {
                        if let Some(span) = line.spans.first_mut() {
                            if let Some(text) = span.text.strip_prefix("  ") {
                                span.text = text.into();
                            }
                        }
                        line
                    }),
            );
        }
        if old.is_some() || new.is_some() {
            details.extend(edit_write_diff_lines(
                old.as_deref(),
                new.as_deref(),
                path.as_deref(),
                width.saturating_sub(4).max(1),
                theme,
            ));
        }
        if let Some(body) = body {
            details.extend(parse_ansi(&body.text));
            if body.truncated {
                details.push(StyledLine {
                    spans: vec![dim_span("… output truncated".into(), theme)],
                });
            }
        }
        let status = if failed {
            "× Unsuccessful"
        } else if entry.result.is_some() {
            "✓ Success"
        } else if sealed {
            "No result"
        } else {
            "◷ Running"
        };
        let label = if header.sub_line.is_some() {
            "Shell"
        } else {
            &header.label
        };
        lines.extend(tool_detail_panel(label, details, status, width, theme));
    }
    lines
}

impl StyledCell for ActivityFoldCell {
    fn styled_lines(&self, width: usize, theme: &Theme, verbose: bool) -> Vec<StyledLine> {
        use unicode_width::UnicodeWidthStr;
        if self.key != "agents" && self.entries.len() == 1 && self.reasoning.trim().is_empty() {
            return single_tool_lines(&self.entries[0], width, theme, verbose, self.sealed);
        }
        let failed = self
            .entries
            .iter()
            .filter(|entry| {
                entry.result.as_ref().is_some_and(|result| {
                    tool_display::result_is_error(result)
                        || result
                            .get("report_delivery")
                            .and_then(serde_json::Value::as_str)
                            == Some("withheld")
                })
            })
            .count();
        let running = self
            .entries
            .iter()
            .filter(|entry| !self.sealed && entry.result.is_none())
            .count();
        let label = if self.entries.is_empty() || (self.thinking && running == 0) {
            self.reasoning
                .lines()
                .filter(|line| !line.trim().is_empty())
                .last()
                .unwrap_or("Thinking…")
                .to_owned()
        } else if self.key == "activity" {
            let mut names: Vec<String> = Vec::new();
            for entry in &self.entries {
                if !names.contains(&entry.tool) {
                    names.push(entry.tool.clone());
                }
            }
            format!(
                "{} · {} {}",
                names.join(", "),
                self.entries.len(),
                if self.entries.len() == 1 {
                    "call"
                } else {
                    "calls"
                }
            )
        } else if self.key == "agents" {
            self.entries
                .iter()
                .map(|entry| {
                    entry
                        .input
                        .get("name")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_owned)
                        .or_else(|| tool_display::tool_header(&entry.tool, &entry.input).primary)
                        .unwrap_or_else(|| "Agent".into())
                })
                .collect::<Vec<_>>()
                .join(", ")
        } else if self.entries.len() == 1 {
            tool_display::tool_header(&self.entries[0].tool, &self.entries[0].input).title()
        } else {
            let coordination = self.entries.iter().any(|entry| {
                matches!(
                    entry.tool.to_ascii_lowercase().replace('_', "").as_str(),
                    "sendmessage" | "toolsearch"
                )
            });
            format!(
                "{} · {} calls",
                if coordination {
                    "Task coordination"
                } else {
                    "Task progress"
                },
                self.entries.len()
            )
        };
        let withheld = self.entries.iter().any(|entry| {
            entry.result.as_ref().is_some_and(|result| {
                result
                    .get("report_delivery")
                    .and_then(serde_json::Value::as_str)
                    == Some("withheld")
            })
        });
        let pending_report = self.entries.iter().any(|entry| {
            entry.result.as_ref().is_some_and(|result| {
                matches!(
                    result
                        .get("report_delivery")
                        .and_then(serde_json::Value::as_str),
                    Some("pending" | "flagged")
                )
            })
        });
        let state = if failed > 0 {
            format!(
                "{}{}",
                if withheld {
                    format!("Needs attention: {failed}")
                } else {
                    format!("{failed} unsuccessful")
                },
                if running > 0 {
                    format!(" · {running} running")
                } else {
                    String::new()
                }
            )
        } else if self.key == "agents" && pending_report {
            if self.entries.iter().any(|entry| {
                entry.result.as_ref().is_some_and(|result| {
                    result
                        .get("report_delivery")
                        .and_then(serde_json::Value::as_str)
                        == Some("flagged")
                })
            }) {
                "Report flagged".into()
            } else {
                "Report pending".into()
            }
        } else if self.key == "agents" {
            let launched = self.entries.iter().any(|entry| {
                entry.result.as_ref().is_none_or(|result| {
                    matches!(
                        result.get("status").and_then(serde_json::Value::as_str),
                        Some("teammate_spawned" | "async_launched" | "remote_launched")
                    )
                })
            });
            if launched {
                "started working".into()
            } else {
                "finished".into()
            }
        } else if running > 0 {
            format!("{running} running")
        } else {
            String::new()
        };
        let marker = if self.key == "agents" && width < 40 {
            "✳ ".to_owned()
        } else if self.key == "agents" {
            "✳ ".repeat(self.entries.len().min(3))
                + &if self.entries.len() > 3 {
                    format!("+{} ", self.entries.len() - 3)
                } else {
                    String::new()
                }
        } else if self.entries.iter().all(|entry| {
            matches!(
                entry.tool.to_ascii_lowercase().replace('_', "").as_str(),
                "sendmessage"
            )
        }) {
            "➤ ".into()
        } else {
            "☷ ".into()
        };
        // Reserve the status and adjacent arrow before truncating long names.
        let fit = fit_tool_text;
        let state = fit(&state, width.saturating_sub(marker.width() + 3));
        let suffix = format!(
            "{} {}",
            if state.is_empty() {
                String::new()
            } else {
                format!(" {state}")
            },
            if verbose { "⌄" } else { "›" }
        );
        let available = width.saturating_sub(marker.width() + suffix.width());
        let shown = fit(&label, available);
        let mut spans = vec![dim_span(marker + &shown, theme)];
        if failed > 0 && state.contains(" · ") {
            let (failure, pending) = state.split_once(" · ").unwrap();
            spans.push(StyledSpan::styled(
                format!(" {failure}"),
                SpanStyle {
                    fg: theme.error,
                    ..SpanStyle::default()
                },
            ));
            spans.push(dim_span(
                format!(" · {pending} {}", if verbose { "⌄" } else { "›" }),
                theme,
            ));
        } else {
            spans.push(StyledSpan::styled(
                suffix,
                SpanStyle {
                    fg: if failed > 0 { theme.error } else { theme.dim },
                    ..SpanStyle::default()
                },
            ));
        }
        let mut lines = vec![StyledLine { spans }];
        if verbose {
            if !self.reasoning.is_empty() {
                lines.extend(
                    crate::history_cell::message::ThinkingCell::new(self.reasoning.clone())
                        .styled_lines(width, theme, true),
                );
            }
            for entry in &self.entries {
                if self.key != "agents" {
                    lines.extend(single_tool_lines(entry, width, theme, true, self.sealed));
                    continue;
                }
                lines.extend(tool_use_lines(&entry.tool, &entry.input, theme, true));
                if let Some(result) = &entry.result {
                    lines.extend(tool_result_lines(
                        &entry.tool,
                        Some(&entry.input),
                        result,
                        None,
                        None,
                        None,
                        width,
                        theme,
                    ));
                }
            }
        }
        lines
    }
}

/// A tool result. Default: a dim `⎿  {summary}` one-liner — the string content
/// when present, else compact JSON. An Edit/Write result carrying the paired
/// diff inputs (`old_string`/`new_string`) renders a structured diff instead
/// (plan Phase 9 step 5): a dim added/removed summary header + the diff rows.
pub(crate) fn tool_result_lines(
    tool: &str,
    input: Option<&serde_json::Value>,
    result: &serde_json::Value,
    old_string: Option<&str>,
    new_string: Option<&str>,
    file_path: Option<&str>,
    width: usize,
    theme: &Theme,
) -> Vec<StyledLine> {
    // Claude Code 2.1.263 src_180132110.js b$n renders no standalone
    // result for teammate_spawned. The coordinator roster carries its state;
    // exposing its transport JSON here also leaks pane coordinates into chat.
    if tool == "Agent"
        && result.get("status").and_then(serde_json::Value::as_str) == Some("teammate_spawned")
        && !tool_display::result_is_error(result)
    {
        return Vec::new();
    }
    let refusal_body = tool_display::result_body(tool, result);
    if tool_denial_reason(
        tool_display::result_is_error(result),
        None,
        refusal_body.as_deref(),
    )
    .is_some()
    {
        return single_tool_lines(
            &ActivityEntry {
                id: String::new(),
                tool: tool.into(),
                input: input.cloned().unwrap_or(serde_json::Value::Null),
                result: Some(result.clone()),
            },
            width,
            theme,
            true,
            true,
        );
    }
    if old_string.is_some() || new_string.is_some() {
        return edit_write_diff_lines(old_string, new_string, file_path, width, theme);
    }
    // The shared headline (`Read 12 lines`, `Found 3 files`, the first line of
    // a command's output) — the same string the clients render. Falls back to
    // the raw payload for a tool with no headline rule.
    let is_error = tool_display::result_is_error(result);
    let summary =
        tool_display::result_headline(tool, input, result, is_error).unwrap_or_else(|| {
            if let Some(s) = result.as_str() {
                s.to_string()
            } else if let Some(s) = result.get("content").and_then(serde_json::Value::as_str) {
                s.to_string()
            } else {
                result.to_string()
            }
        });
    vec![StyledLine {
        spans: vec![dim_span(format!("  ⎿  {}", truncate(&summary, 100)), theme)],
    }]
}

/// The Edit/Write structured diff: a dim `  ⎿  Added N line(s)[, removed M
/// line(s)]` gutter header (iocraft `added_removed_header` parity), then the
/// diff rows from `tui_core::render::diff` — Write is a pure add (`old` =
/// empty), `file_path` drives syntax detection, and changed rows are
/// background-padded to `width`. The diff's syntax scheme is the renderer
/// default (dark), matching the markdown code-theme choice.
fn edit_write_diff_lines(
    old_string: Option<&str>,
    new_string: Option<&str>,
    file_path: Option<&str>,
    width: usize,
    theme: &Theme,
) -> Vec<StyledLine> {
    let old = old_string.unwrap_or("");
    let new = new_string.unwrap_or("");
    let (additions, removals) = diff::diff_stats(old, new);
    let summary = added_removed_header(additions, removals).unwrap_or_default();
    let mut out = vec![StyledLine {
        spans: vec![dim_span(format!("  ⎿  {summary}"), theme)],
    }];
    out.extend(diff::render_with_width(
        old,
        new,
        file_path,
        ThemeName::default(),
        width,
    ));
    out
}

// Moved to `tui_core::tool_display::result` so the terminal and the clients
// spell the edit summary identically. Re-exported under the old path so
// `super::added_removed_header` and its byte-lock test keep resolving.
pub(crate) use tui_core::tool_display::result::added_removed_header;

/// Bash / local-command output: ANSI-parsed stdout lines (SGR colors and
/// attributes preserved — the bodies carry raw escape codes), then stderr,
/// also ANSI-parsed, with spans that carry no explicit ANSI foreground tinted
/// the error color (preserving the plain-text stderr coloring).
pub(crate) fn command_output_lines(stdout: &str, stderr: &str, theme: &Theme) -> Vec<StyledLine> {
    let mut out = parse_ansi(stdout);
    if !stderr.is_empty() {
        out.extend(error_tinted(parse_ansi(stderr), theme.error));
    }
    out
}

/// Tint every span without an explicit foreground color with `error`.
fn error_tinted(lines: Vec<StyledLine>, error: StyleColor) -> Vec<StyledLine> {
    lines
        .into_iter()
        .map(|mut line| {
            for span in &mut line.spans {
                if span.style.fg == StyleColor::Default {
                    span.style.fg = error;
                }
            }
            line
        })
        .collect()
}

/// A grouped tool-use block: `● {tool} (×{count})` header. Collapsed → header
/// only; verbose → header + each child's truncated input → result line.
pub(crate) fn group_tool_use_lines(
    tool: &str,
    entries: &[(serde_json::Value, serde_json::Value)],
    theme: &Theme,
    verbose: bool,
) -> Vec<StyledLine> {
    if let [(input, result)] = entries {
        if activity_fold_key(tool, input) != Some("agents") {
            return single_tool_lines(
                &ActivityEntry {
                    id: String::new(),
                    tool: tool.to_owned(),
                    input: input.clone(),
                    result: Some(result.clone()),
                },
                100,
                theme,
                verbose,
                true,
            );
        }
    }
    if !verbose
        && entries
            .first()
            .and_then(|(input, _)| activity_fold_key(tool, input))
            .is_some()
    {
        return ActivityFoldCell {
            reasoning: String::new(),
            thinking: false,
            sealed: false,
            key: activity_fold_key(tool, &entries[0].0).unwrap(),
            entries: entries
                .iter()
                .enumerate()
                .map(|(i, (input, result))| ActivityEntry {
                    id: i.to_string(),
                    tool: tool.to_owned(),
                    input: input.clone(),
                    result: Some(result.clone()),
                })
                .collect(),
        }
        .styled_lines(100, theme, false);
    }
    if tool == "Agent"
        && entries.iter().any(|(_, result)| {
            result.get("status").and_then(serde_json::Value::as_str) == Some("teammate_spawned")
        })
    {
        return grouped_agent_lines(entries, theme);
    }
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
                        "  ⎿  {} → {}",
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

/// Completed Agent groups receive result-aware labels instead of exposing
/// transport JSON. Source: Claude Code 2.1.263 A$n / Zg.
fn grouped_agent_lines(
    entries: &[(serde_json::Value, serde_json::Value)],
    theme: &Theme,
) -> Vec<StyledLine> {
    let headers: Vec<_> = entries
        .iter()
        .map(|(input, result)| {
            let mut header = tool_display::tool_header_with_result("Agent", input, result);
            if header.label == "Task" {
                header.label = "Agent".into();
            }
            header
        })
        .collect();
    let homogeneous = headers
        .first()
        .is_some_and(|first| headers.iter().all(|header| header.label == first.label));
    let asynchronous = |input: &serde_json::Value, result: &serde_json::Value| {
        input
            .get("run_in_background")
            .and_then(serde_json::Value::as_bool)
            == Some(true)
            || matches!(
                result.get("status").and_then(serde_json::Value::as_str),
                Some("teammate_spawned" | "async_launched" | "remote_launched")
            )
    };
    let all_async = entries
        .iter()
        .all(|(input, result)| asynchronous(input, result));
    let summary = if all_async {
        format!("{} background agents launched (↓ manage)", entries.len())
    } else {
        format!("{} agents finished", entries.len())
    };
    let mut lines = vec![StyledLine {
        spans: vec![
            StyledSpan::styled(
                TOOL_MARKER,
                SpanStyle {
                    fg: theme.success,
                    ..SpanStyle::default()
                },
            ),
            StyledSpan::plain(summary),
        ],
    }];
    for (index, ((input, result), header)) in entries.iter().zip(headers).enumerate() {
        let last = index + 1 == entries.len();
        let label = if homogeneous {
            let name = input.get("name").and_then(serde_json::Value::as_str);
            match (name, header.primary.as_deref()) {
                (Some(name), Some(description)) => format!("{name}: {description}"),
                (Some(name), None) => name.to_owned(),
                (None, Some(description)) => description.to_owned(),
                (None, None) => header.label,
            }
        } else if let Some(description) = header.primary {
            format!("{} ({description})", header.label)
        } else {
            header.label
        };
        let metrics = if asynchronous(input, result) {
            String::new()
        } else {
            let count = result
                .get("totalToolUseCount")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0);
            format!(
                " · {count} tool {}",
                if count == 1 { "use" } else { "uses" }
            )
        };
        lines.push(StyledLine {
            spans: vec![StyledSpan::plain(format!(
                "   {} {label}{metrics}",
                if last { "└" } else { "├" }
            ))],
        });
        if !asynchronous(input, result) {
            lines.push(StyledLine {
                spans: vec![dim_span(
                    format!("   {} ⎿  Done", if last { "  " } else { "│ " }),
                    theme,
                )],
            });
        }
    }
    lines
}

/// The collapsed Read/Search fold: a dim `Read/Search (N results)` count
/// line; verbose expands the per-entry display lines under a `⎿  ` gutter
/// (tui-core `CollapsedReadSearch.entries` contract: "shown when expanded").
#[allow(clippy::too_many_arguments)]
pub(crate) fn collapsed_read_search_lines(
    search_count: u64,
    read_count: u64,
    list_count: u64,
    repl_count: u64,
    mcp_call_count: u64,
    mcp_server_names: &[String],
    bash_count: u64,
    memory_write_count: u64,
    is_active: bool,
    latest_hint: Option<&str>,
    entries: &[String],
    theme: &Theme,
    verbose: bool,
) -> Vec<StyledLine> {
    let summary = tui_core::collapse::search_read_summary_text_full(
        search_count,
        read_count,
        list_count,
        repl_count,
        mcp_call_count,
        mcp_server_names,
        bash_count,
        memory_write_count,
        is_active,
    );
    // Entries are kept in invocation order. Counts refer to attempts, not
    // successful operations or unique files; details retain each raw outcome.
    let summary = if let Some(first) = entries.first() {
        let label = first.split_whitespace().next().unwrap_or("Tools");
        format!(
            "{label} · {} {}{}",
            entries.len(),
            if entries.len() == 1 { "call" } else { "calls" },
            if is_active { " · working…" } else { "" }
        )
    } else {
        summary
    };
    let mut out = if summary.is_empty() {
        Vec::new()
    } else {
        colored_lines(&summary, theme.dim)
    };
    // The dim `⎿ <latest read>` hint renders ONLY while the group is active
    // (CollapsedReadSearchContent.tsx parity).
    if is_active {
        if let Some(hint) = latest_hint {
            out.push(StyledLine {
                spans: vec![dim_span(format!("  ⎿  {hint}"), theme)],
            });
        }
    }
    if verbose {
        for entry in entries {
            out.push(StyledLine {
                spans: vec![dim_span(format!("  ⎿  {entry}"), theme)],
            });
        }
    }
    out
}

/// [`RenderedMessage::AssistantToolUse`](tui_core::message::RenderedMessage::AssistantToolUse)
/// — the `● {tool}` call header + collapsed/verbose input.
#[derive(Debug)]
pub struct ToolUseCell {
    tool: String,
    input: serde_json::Value,
}

impl ToolUseCell {
    /// Wrap one tool call.
    #[must_use]
    pub fn new(tool: String, input: serde_json::Value) -> Self {
        Self { tool, input }
    }

    /// The tool name.
    #[must_use]
    pub fn tool(&self) -> &str {
        &self.tool
    }
}

impl StyledCell for ToolUseCell {
    fn styled_lines(&self, _width: usize, theme: &Theme, verbose: bool) -> Vec<StyledLine> {
        tool_use_lines(&self.tool, &self.input, theme, verbose)
    }
}

/// [`RenderedMessage::UserToolResult`](tui_core::message::RenderedMessage::UserToolResult)
/// — the `  ⎿  {summary}` result line, or the Edit/Write structured diff when
/// the paired `old_string`/`new_string` inputs are present.
#[derive(Debug)]
pub struct ToolResultCell {
    tool: String,
    result: serde_json::Value,
    old_string: Option<String>,
    new_string: Option<String>,
    file_path: Option<String>,
    input: Option<serde_json::Value>,
}

impl ToolResultCell {
    /// Wrap one tool result (`old_string`/`new_string`/`file_path` carry the
    /// paired Edit/Write inputs for diff rendering when present; `tool` and
    /// `input` drive the shared result headline).
    #[must_use]
    pub fn new(
        tool: String,
        result: serde_json::Value,
        old_string: Option<String>,
        new_string: Option<String>,
        file_path: Option<String>,
        input: Option<serde_json::Value>,
    ) -> Self {
        Self {
            tool,
            result,
            old_string,
            new_string,
            file_path,
            input,
        }
    }
}

impl StyledCell for ToolResultCell {
    fn styled_lines(&self, width: usize, theme: &Theme, verbose: bool) -> Vec<StyledLine> {
        let refusal_body = tool_display::result_body(&self.tool, &self.result);
        if tool_denial_reason(
            tool_display::result_is_error(&self.result),
            None,
            refusal_body.as_deref(),
        )
        .is_some()
        {
            return single_tool_lines(
                &ActivityEntry {
                    id: String::new(),
                    tool: self.tool.clone(),
                    input: self.input.clone().unwrap_or(serde_json::Value::Null),
                    result: Some(self.result.clone()),
                },
                width,
                theme,
                verbose,
                true,
            );
        }
        tool_result_lines(
            &self.tool,
            self.input.as_ref(),
            &self.result,
            self.old_string.as_deref(),
            self.new_string.as_deref(),
            self.file_path.as_deref(),
            width,
            theme,
        )
    }
}

/// [`RenderedMessage::UserBashOutput`](tui_core::message::RenderedMessage::UserBashOutput)
/// and
/// [`RenderedMessage::UserLocalCommandOutput`](tui_core::message::RenderedMessage::UserLocalCommandOutput)
/// — ANSI-parsed stdout + error-tinted ANSI-parsed stderr.
#[derive(Debug)]
pub struct CommandOutputCell {
    stdout: String,
    stderr: String,
}

impl CommandOutputCell {
    /// Wrap captured command output.
    #[must_use]
    pub fn new(stdout: String, stderr: String) -> Self {
        Self { stdout, stderr }
    }
}

impl StyledCell for CommandOutputCell {
    fn styled_lines(&self, _width: usize, theme: &Theme, _verbose: bool) -> Vec<StyledLine> {
        command_output_lines(&self.stdout, &self.stderr, theme)
    }
}

/// [`RenderedMessage::GroupedToolUse`](tui_core::message::RenderedMessage::GroupedToolUse)
/// — the `● {tool} (×N)` fold of consecutive same-tool calls.
#[derive(Debug)]
pub struct GroupedToolUseCell {
    tool: String,
    entries: Vec<(serde_json::Value, serde_json::Value)>,
}

impl GroupedToolUseCell {
    /// Wrap a fold of `(input, result)` pairs sharing one tool.
    #[must_use]
    pub fn new(tool: String, entries: Vec<(serde_json::Value, serde_json::Value)>) -> Self {
        Self { tool, entries }
    }
}

impl StyledCell for GroupedToolUseCell {
    fn styled_lines(&self, _width: usize, theme: &Theme, verbose: bool) -> Vec<StyledLine> {
        group_tool_use_lines(&self.tool, &self.entries, theme, verbose)
    }
}

/// [`RenderedMessage::CollapsedReadSearch`](tui_core::message::RenderedMessage::CollapsedReadSearch)
/// — the `Read/Search (N results)` fold of read/search/list runs.
#[derive(Debug)]
pub struct CollapsedReadSearchCell {
    search_count: u64,
    read_count: u64,
    list_count: u64,
    repl_count: u64,
    mcp_call_count: u64,
    mcp_server_names: Vec<String>,
    bash_count: u64,
    memory_write_count: u64,
    is_active: bool,
    latest_hint: Option<String>,
    entries: Vec<String>,
}

impl CollapsedReadSearchCell {
    /// Wrap the fold's counts + verbose entries.
    #[must_use]
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        search_count: u64,
        read_count: u64,
        list_count: u64,
        repl_count: u64,
        mcp_call_count: u64,
        mcp_server_names: Vec<String>,
        bash_count: u64,
        memory_write_count: u64,
        is_active: bool,
        latest_hint: Option<String>,
        entries: Vec<String>,
    ) -> Self {
        Self {
            search_count,
            read_count,
            list_count,
            repl_count,
            mcp_call_count,
            mcp_server_names,
            bash_count,
            memory_write_count,
            is_active,
            latest_hint,
            entries,
        }
    }
}

impl StyledCell for CollapsedReadSearchCell {
    fn styled_lines(&self, _width: usize, theme: &Theme, verbose: bool) -> Vec<StyledLine> {
        collapsed_read_search_lines(
            self.search_count,
            self.read_count,
            self.list_count,
            self.repl_count,
            self.mcp_call_count,
            &self.mcp_server_names,
            self.bash_count,
            self.memory_write_count,
            self.is_active,
            self.latest_hint.as_deref(),
            &self.entries,
            theme,
            verbose,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::super::{HistoryCell, RenderMode};
    use super::*;

    fn plain(cell: &dyn HistoryCell, verbose: bool) -> Vec<String> {
        cell.display_lines(
            80,
            &Theme::dark(),
            RenderMode {
                raw: false,
                verbose,
            },
        )
        .iter()
        .map(ToString::to_string)
        .collect()
    }

    #[test]
    fn fold_boundaries_and_icons() {
        use super::activity_fold_key as key;
        use serde_json::json;
        for name in ["TaskList", "TaskUpdate", "TodoWrite", "SendMessage"] {
            assert_eq!(key(name, &json!({})), Some("tasks"));
        }
        for name in ["Agent", "Task", "spawn_agent"] {
            assert_eq!(key(name, &json!({})), Some("agents"));
        }
        for name in [
            "TaskOutput",
            "TaskStop",
            "Skill",
            "AskUserQuestion",
            "PermissionRequest",
            "mcp__unrelated__SendMessage",
        ] {
            assert_eq!(key(name, &json!({})), None);
        }
        assert_eq!(
            key(
                "ToolSearch",
                &json!({"query":"select:TaskList,SendMessage"})
            ),
            Some("tasks")
        );
        assert_eq!(
            key("ToolSearch", &json!({"query":"select:TaskList,Read"})),
            None
        );
    }

    #[test]
    fn lone_summary_only_tools_render_once_without_disclosure() {
        use super::{ActivityEntry, ActivityFoldCell};
        use serde_json::json;
        for (tool, input, result) in [
            (
                "Skill",
                json!({"skill":"local-app"}),
                json!("Launching skill: local-app"),
            ),
            (
                "Bash",
                json!({"command":"echo ready"}),
                json!({"stdout":"Ready","stderr":""}),
            ),
            (
                "mcp__docs__search",
                json!({"query":"help"}),
                json!({"error":"Unavailable"}),
            ),
        ] {
            let cell = ActivityFoldCell {
                key: "activity",
                reasoning: String::new(),
                thinking: false,
                sealed: true,
                entries: vec![ActivityEntry {
                    id: "one".into(),
                    tool: tool.into(),
                    input,
                    result: Some(result),
                }],
            };
            for width in [20, 80] {
                let collapsed = cell.styled_lines(width, &Theme::dark(), false);
                let expanded = cell.styled_lines(width, &Theme::dark(), true);
                assert_eq!(collapsed, expanded);
                assert_eq!(collapsed.len(), 1);
                let text: String = collapsed[0]
                    .spans
                    .iter()
                    .map(|span| span.text.as_str())
                    .collect();
                assert!(
                    !text.contains('›') && !text.contains('⌄') && !text.contains("1 call"),
                    "{text}"
                );
                assert!(
                    text.contains(if tool == "Skill" {
                        "Launching skill"
                    } else if tool == "Bash" {
                        "Ready"
                    } else {
                        "Unavailable"
                    }),
                    "{text}"
                );
            }
        }
    }

    #[test]
    fn lone_tool_disclosure_preserves_multiline_output_and_diffs() {
        use super::{ActivityEntry, ActivityFoldCell};
        use serde_json::json;
        for (tool, input, result, expected) in [
            (
                "Bash",
                json!({"command":"printf output"}),
                json!({"stdout":"first\nsecond","stderr":""}),
                "second",
            ),
            (
                "Write",
                json!({"file_path":"test.txt","content":"new content"}),
                json!({}),
                "new content",
            ),
        ] {
            let cell = ActivityFoldCell {
                key: "activity",
                reasoning: String::new(),
                thinking: false,
                sealed: true,
                entries: vec![ActivityEntry {
                    id: "one".into(),
                    tool: tool.into(),
                    input,
                    result: Some(result),
                }],
            };
            let collapsed = cell.styled_lines(80, &Theme::dark(), false);
            assert_eq!(collapsed.len(), 1);
            assert!(collapsed[0]
                .spans
                .iter()
                .any(|span| span.text.ends_with('›')));
            let expanded = cell.styled_lines(80, &Theme::dark(), true);
            assert!(expanded.len() > 1);
            assert!(expanded
                .iter()
                .flat_map(|line| &line.spans)
                .any(|span| span.text.contains(expected)));
        }
    }

    #[test]
    fn expanded_panels_preserve_wide_text_colors_and_unindented_child_headers() {
        use super::{ActivityEntry, ActivityFoldCell};
        use unicode_width::UnicodeWidthStr;
        let cell = ActivityFoldCell {
            key: "activity",
            reasoning: String::new(),
            thinking: false,
            sealed: true,
            entries: vec![
                ActivityEntry {
                    id: "shell".into(),
                    tool: "Bash".into(),
                    input: serde_json::json!({"command":"printf 中文输出"}),
                    result: Some(
                        serde_json::json!({"stdout":"中文输出\\n\u{1b}[31mred\u{1b}[0m\nlast","stderr":""}),
                    ),
                },
                ActivityEntry {
                    id: "skill".into(),
                    tool: "Skill".into(),
                    input: serde_json::json!({"skill":"apple-design"}),
                    result: Some(
                        serde_json::json!({"body":"# Design\n\nKeep every detail readable."}),
                    ),
                },
            ],
        };
        for theme in [
            Theme::dark(),
            Theme::light(),
            Theme::dark_ansi(),
            Theme::light_ansi(),
        ] {
            for width in [6, 12, 24, 80] {
                let lines = cell.styled_lines(width, &theme, true);
                assert!(
                    lines.iter().all(|line| line.plain_text().width() <= width),
                    "width={width}: {:?}",
                    lines.iter().map(StyledLine::plain_text).collect::<Vec<_>>()
                );
                let headers: Vec<_> = lines
                    .iter()
                    .filter(|line| line.plain_text().starts_with(TOOL_MARKER))
                    .collect();
                assert_eq!(
                    headers.len(),
                    2,
                    "child headers share the group's leading edge"
                );
                assert!(lines.iter().any(|line| line.plain_text().starts_with('╭')));
                assert_eq!(
                    lines
                        .iter()
                        .flat_map(|line| &line.spans)
                        .filter(|span| span.style.fg
                            == tui_core::render::StyleColor::Named(
                                tui_core::render::NamedColor::Red
                            ))
                        .map(|span| span.text.as_str())
                        .collect::<String>(),
                    "red"
                );
                if width == 80 {
                    let output = lines
                        .iter()
                        .map(|line| line.plain_text())
                        .collect::<Vec<_>>()
                        .join("\n");
                    assert!(
                        output.contains("中文输出")
                            && output.contains("Keep every detail readable.")
                    );
                    assert!(output.contains("✓ Success"));
                }
            }
        }
    }

    #[test]
    fn panel_footer_distinguishes_failures_and_missing_results() {
        for (result, expected) in [
            (
                Some(serde_json::json!({"error":"first\nsecond"})),
                "Unsuccessful",
            ),
            (None, "No result"),
        ] {
            let lines = super::single_tool_lines(
                &super::ActivityEntry {
                    id: "write".into(),
                    tool: "Write".into(),
                    input: serde_json::json!({"file_path":"out.txt","content":"file body"}),
                    result,
                },
                80,
                &Theme::dark(),
                true,
                true,
            );
            let output = lines
                .iter()
                .map(|line| line.plain_text())
                .collect::<Vec<_>>()
                .join("\n");
            assert!(output.contains(expected), "{output}");
            assert!(!output.contains("✓ Success"), "{output}");
        }
    }
    #[test]
    fn collapsed_agents_and_verbose_details() {
        use super::{ActivityEntry, ActivityFoldCell};
        use serde_json::json;
        let cell = ActivityFoldCell {
            reasoning: String::new(),
            thinking: false,
            sealed: false,
            key: "agents",
            entries: (0..6)
                .map(|i| ActivityEntry {
                    id: i.to_string(),
                    tool: "Agent".into(),
                    input: json!({"name":format!("worker-{i}"),"description":"Check code"}),
                    result: Some(json!({"status":"async_launched"})),
                })
                .collect(),
        };
        for width in [40, 80, 120] {
            let lines = cell.styled_lines(width, &Theme::dark(), false);
            assert_eq!(lines.len(), 1);
            let text = lines[0]
                .spans
                .iter()
                .map(|s| s.text.as_str())
                .collect::<String>();
            assert!(text.contains("+3"));
            assert!(text.contains("started working"));
            assert!(text.ends_with("›"));
            assert!(
                unicode_width::UnicodeWidthStr::width(text.as_str()) <= width,
                "{text}"
            );
        }
        let details = cell.styled_lines(80, &Theme::dark(), true);
        assert!(details.len() > 6);
        assert!(details
            .iter()
            .flat_map(|l| &l.spans)
            .any(|s| s.text.contains("worker-5")));
    }
    #[test]
    fn mixed_failures_and_cjk_remain_visible() {
        use super::{ActivityEntry, ActivityFoldCell};
        use serde_json::json;
        let cell = ActivityFoldCell {
            reasoning: String::new(),
            thinking: false,
            sealed: false,
            key: "tasks",
            entries: vec![
                ActivityEntry {
                    id: "a".into(),
                    tool: "TaskList".into(),
                    input: json!({}),
                    result: Some(json!({"error":"failure"})),
                },
                ActivityEntry {
                    id: "b".into(),
                    tool: "SendMessage".into(),
                    input: json!({}),
                    result: None,
                },
            ],
        };
        let text = cell.styled_lines(60, &Theme::dark(), false)[0]
            .spans
            .iter()
            .map(|s| s.text.as_str())
            .collect::<String>();
        assert!(text.contains("1 unsuccessful"), "{text}");
        assert!(text.contains("1 running"), "{text}");
    }

    #[test]
    fn reports_are_not_mistaken_for_delivered_completion() {
        use super::{ActivityEntry, ActivityFoldCell};
        use serde_json::json;
        for delivery in ["withheld", "pending", "flagged"] {
            let cell = ActivityFoldCell {
                reasoning: String::new(),
                thinking: false,
                sealed: false,
                key: "agents",
                entries: vec![ActivityEntry {
                    id: "a".into(),
                    tool: "Agent".into(),
                    input: json!({"name":"review"}),
                    result: Some(json!({"status":"completed","report_delivery":delivery})),
                }],
            };
            let text = cell.styled_lines(80, &Theme::dark(), false)[0]
                .spans
                .iter()
                .map(|s| s.text.as_str())
                .collect::<String>();
            assert!(!text.contains("finished"), "{text}");
            assert!(
                text.contains(if delivery == "withheld" {
                    "Needs attention"
                } else if delivery == "flagged" {
                    "Report flagged"
                } else {
                    "Report pending"
                }),
                "{text}"
            );
        }
    }

    #[test]
    fn local_app_has_a_compact_application_header_and_preserves_verbose_details() {
        let input = serde_json::json!({"action":"read", "name":"Family learning space"});
        let lines = super::tool_use_lines("LocalApp", &input, &Theme::dark(), false);
        assert_eq!(lines.len(), 1);
        let text = lines[0]
            .spans
            .iter()
            .map(|s| s.text.as_str())
            .collect::<String>();
        assert!(text.starts_with("▦ Local app"), "{text}");
        let verbose = super::tool_use_lines("LocalApp", &input, &Theme::dark(), true);
        assert!(verbose
            .iter()
            .flat_map(|line| &line.spans)
            .any(|span| span.text.contains("Family learning space")));
    }

    fn rata(color: tui_core::render::StyleColor) -> ratatui::style::Color {
        crate::style_adapter::to_ratatui(color)
    }

    #[test]
    fn tool_use_cell_collapsed_renders_header_and_truncated_input() {
        let cell = ToolUseCell::new(
            "Read".to_string(),
            serde_json::json!({"file_path": "src/lib.rs"}),
        );
        let lines = plain(&cell, false);
        // The header is now parameterized (`Read(src/lib.rs)`), so a tool with
        // no sub-line renders as a single line instead of a header plus a
        // truncated raw-JSON line.
        assert_eq!(lines, vec![format!("{TOOL_MARKER}Read(src/lib.rs)")]);
        assert_eq!(cell.tool(), "Read");
        // Header marker is success-colored; the title is unstyled.
        let styled = cell.display_lines(80, &Theme::dark(), RenderMode::default());
        assert_eq!(
            styled[0].spans[0].style.fg,
            Some(rata(Theme::dark().success))
        );
        assert_eq!(styled[0].spans[1].style.fg, None);
    }

    /// REGRESSION: the shared-header extraction replaced the unconditional
    /// truncated-input line with `else if let Some(sub) = derived.sub_line`.
    /// `sub_line` is set only for the shell family and `primary` only from the
    /// header table / `GENERIC_PRIMARY_KEYS`, so a tool with NEITHER —
    /// `TodoWrite`, or an MCP tool whose input carries none of the generic
    /// keys — rendered its call with no argument information at all.
    #[test]
    fn a_tool_with_no_header_argument_still_shows_its_input() {
        let cell = ToolUseCell::new(
            "TodoWrite".to_string(),
            serde_json::json!({"todos": [{"content": "Ship it"}]}),
        );
        let lines = plain(&cell, false);
        assert_eq!(lines.len(), 2, "header + arguments, got {lines:?}");
        assert!(
            lines[1].contains("Ship it"),
            "the call's arguments must be visible: {lines:?}"
        );

        // Same for an MCP tool whose input has none of the generic keys.
        let mcp = ToolUseCell::new(
            "mcp__srv__thing".to_string(),
            serde_json::json!({"widget_id": 42}),
        );
        let lines = plain(&mcp, false);
        assert_eq!(lines.len(), 2, "header + arguments, got {lines:?}");
        assert!(lines[1].contains("widget_id"), "{lines:?}");
    }

    #[test]
    fn tool_use_cell_renders_a_bash_command_sub_line_truncated_at_100_chars() {
        let cell = ToolUseCell::new(
            "Bash".to_string(),
            serde_json::json!({"command": "x".repeat(200)}),
        );
        let lines = plain(&cell, false);
        assert_eq!(lines[0], format!("{TOOL_MARKER}Running 1 shell command…"));
        // Bash is the one tool with a sub-line: `$ {command}`, truncated.
        let sub = lines[1].trim_start();
        assert!(sub.starts_with("$ "), "got: {sub}");
        let command = sub.trim_start_matches("$ ");
        assert_eq!(command.chars().count(), 100, "99 chars + ellipsis");
        assert!(command.ends_with('…'), "got: {command}");
    }

    #[test]
    fn tool_use_cell_verbose_pretty_prints_json_input() {
        let cell = ToolUseCell::new(
            "Read".to_string(),
            serde_json::json!({"file_path": "src/lib.rs", "limit": 10}),
        );
        let lines = plain(&cell, true);
        assert!(
            lines.len() > 2,
            "pretty JSON spans multiple lines: {lines:?}"
        );
        // `limit: 10` makes it a partial read, which the header qualifies.
        assert_eq!(
            lines[0],
            format!("{TOOL_MARKER}Read(src/lib.rs) (lines 1-10)")
        );
        assert!(lines[1].starts_with("  {"), "indented JSON: {lines:?}");
        assert!(
            lines.iter().any(|l| l.contains("\"limit\": 10")),
            "pretty-printed field: {lines:?}"
        );
    }

    #[test]
    fn tool_result_cell_prefers_string_content_field() {
        let cell = ToolResultCell::new(
            "Whatever".to_string(),
            serde_json::json!({"content": "hello world"}),
            None,
            None,
            None,
            None,
        );
        assert_eq!(plain(&cell, false), vec!["  ⎿  hello world".to_string()]);
        let styled = cell.display_lines(80, &Theme::dark(), RenderMode::default());
        assert_eq!(styled[0].spans[0].style.fg, Some(rata(Theme::dark().dim)));
    }

    #[test]
    fn tool_result_cell_unwraps_bare_string_and_flattens_newlines() {
        // A bare-string result (replay path) renders directly, newlines
        // flattened into the one-line summary.
        let cell = ToolResultCell::new(
            "Whatever".to_string(),
            serde_json::Value::String("line one\nline two".to_string()),
            None,
            None,
            None,
            None,
        );
        assert_eq!(
            plain(&cell, false),
            vec!["  ⎿  line one line two".to_string()]
        );
    }

    #[test]
    fn teammate_spawn_has_no_standalone_result_row() {
        let result = serde_json::json!({
            "status": "teammate_spawned",
            "name": "reviewer",
            "agent_id": "agent-reviewer",
            "tmux_session_name": "current",
            "tmux_window_name": "current",
            "tmux_pane_id": "%4"
        });
        assert!(
            tool_result_lines("Agent", None, &result, None, None, None, 80, &Theme::dark())
                .is_empty()
        );
        let failure = serde_json::json!({"status": "teammate_spawned", "error": "Spawn failed"});
        let lines = tool_result_lines(
            "Agent",
            None,
            &failure,
            None,
            None,
            None,
            80,
            &Theme::dark(),
        );
        assert_eq!(lines.len(), 1);
        assert!(lines[0].plain_text().contains("Spawn failed"));
    }

    #[test]
    fn tool_result_cell_falls_back_to_compact_json() {
        let cell = ToolResultCell::new(
            "Whatever".to_string(),
            serde_json::json!({"status": "ok"}),
            None,
            None,
            None,
            None,
        );
        assert_eq!(
            plain(&cell, false),
            vec!["  ⎿  {\"status\":\"ok\"}".to_string()]
        );
    }

    #[test]
    fn command_output_cell_renders_stdout_then_error_colored_stderr() {
        let cell = CommandOutputCell::new("out line".to_string(), "err line".to_string());
        assert_eq!(
            plain(&cell, false),
            vec!["out line".to_string(), "err line".to_string()]
        );
        let styled = cell.display_lines(80, &Theme::dark(), RenderMode::default());
        assert_eq!(styled[0].spans[0].style.fg, None, "stdout keeps default fg");
        assert_eq!(styled[1].spans[0].style.fg, Some(rata(Theme::dark().error)));

        // Empty stderr adds nothing.
        let out_only = CommandOutputCell::new("just out".to_string(), String::new());
        assert_eq!(plain(&out_only, false), vec!["just out".to_string()]);
    }

    #[test]
    fn command_output_cell_preserves_ansi_colors_in_stdout() {
        // The bash-output bodies carry raw SGR codes (tui-core contract);
        // they must render as colored spans, not literal escape bytes.
        let cell = CommandOutputCell::new(
            "\u{1b}[31mred\u{1b}[0m plain\nsecond".to_string(),
            String::new(),
        );
        let lines = plain(&cell, false);
        assert_eq!(lines, vec!["red plain".to_string(), "second".to_string()]);
        let styled = cell.display_lines(80, &Theme::dark(), RenderMode::default());
        let red = &styled[0].spans[0];
        assert_eq!(red.content.as_ref(), "red");
        assert_eq!(red.style.fg, Some(ratatui::style::Color::Red));
        let tail = &styled[0].spans[1];
        assert_eq!(tail.content.as_ref(), " plain");
        assert_eq!(tail.style.fg, None, "unstyled stdout keeps default fg");
    }

    #[test]
    fn command_output_cell_stderr_keeps_ansi_colors_and_tints_plain_spans() {
        let cell =
            CommandOutputCell::new(String::new(), "\u{1b}[32mgreen\u{1b}[0m tail".to_string());
        let styled = cell.display_lines(80, &Theme::dark(), RenderMode::default());
        assert_eq!(styled.len(), 1);
        let green = &styled[0].spans[0];
        assert_eq!(green.content.as_ref(), "green");
        assert_eq!(
            green.style.fg,
            Some(ratatui::style::Color::Green),
            "explicit ANSI color wins over the error tint"
        );
        let tail = &styled[0].spans[1];
        assert_eq!(tail.content.as_ref(), " tail");
        assert_eq!(
            tail.style.fg,
            Some(rata(Theme::dark().error)),
            "plain stderr spans keep the error tint"
        );
    }

    #[test]
    fn added_removed_header_variants() {
        // Ported from the iocraft `user_tool_result` renderer, byte-for-byte.
        assert_eq!(added_removed_header(0, 0), None);
        assert_eq!(added_removed_header(1, 0).as_deref(), Some("Added 1 line"));
        assert_eq!(added_removed_header(3, 0).as_deref(), Some("Added 3 lines"));
        // Sole removal clause -> capitalized "Removed".
        assert_eq!(
            added_removed_header(0, 1).as_deref(),
            Some("Removed 1 line")
        );
        assert_eq!(
            added_removed_header(0, 2).as_deref(),
            Some("Removed 2 lines")
        );
        // Both present -> lowercase "removed" joined with ", ".
        assert_eq!(
            added_removed_header(2, 3).as_deref(),
            Some("Added 2 lines, removed 3 lines")
        );
    }

    #[test]
    fn tool_result_cell_renders_edit_diff_when_old_and_new_present() {
        let cell = ToolResultCell::new(
            "Edit".to_string(),
            serde_json::json!({"content": "ok"}),
            Some("alpha\nbeta".to_string()),
            Some("alpha\ngamma".to_string()),
            Some("x.rs".to_string()),
            None,
        );
        let lines = plain(&cell, false);
        assert_eq!(
            lines[0], "  ⎿  Added 1 line, removed 1 line",
            "summary header inside the gutter: {lines:?}"
        );
        let all = lines.join("\n");
        assert!(all.contains("beta"), "removed line shown: {all}");
        assert!(all.contains("gamma"), "added line shown: {all}");
        // Diff rows carry the add/remove line backgrounds.
        let styled = cell.display_lines(80, &Theme::dark(), RenderMode::default());
        assert!(
            styled
                .iter()
                .flat_map(|l| l.spans.iter())
                .any(|s| s.style.bg.is_some()),
            "diff rows carry background colors"
        );
        // The header line is dim, like every other result gutter.
        assert_eq!(styled[0].spans[0].style.fg, Some(rata(Theme::dark().dim)));
    }

    #[test]
    fn tool_result_cell_renders_write_as_pure_add_diff() {
        let cell = ToolResultCell::new(
            "Write".to_string(),
            serde_json::json!({"content": "ok"}),
            None,
            Some("line one\nline two".to_string()),
            Some("x.txt".to_string()),
            None,
        );
        let lines = plain(&cell, false);
        assert_eq!(
            lines[0], "  ⎿  Added 2 lines",
            "pure-add summary: {lines:?}"
        );
        let all = lines.join("\n");
        assert!(all.contains("line one"), "{all}");
        assert!(all.contains("line two"), "{all}");
        // The plain `⎿  ok` summary is replaced by the diff.
        assert!(!all.contains("⎿  ok"), "{all}");
    }

    #[test]
    fn collapsed_read_search_cell_verbose_expands_entries() {
        let cell = CollapsedReadSearchCell::new(
            1,
            1,
            0,
            0,
            0,
            Vec::new(),
            0,
            0,
            false,
            None,
            vec!["Read a.rs".to_string(), "Grep foo".to_string()],
        );
        assert_eq!(
            plain(&cell, true),
            vec![
                "Read · 2 calls".to_string(),
                "  ⎿  Read a.rs".to_string(),
                "  ⎿  Grep foo".to_string(),
            ]
        );
        let styled = cell.display_lines(
            80,
            &Theme::dark(),
            RenderMode {
                raw: false,
                verbose: true,
            },
        );
        assert!(
            styled
                .iter()
                .all(|l| l.spans[0].style.fg == Some(rata(Theme::dark().dim))),
            "entries render dim like the summary"
        );
    }

    #[test]
    fn grouped_tool_use_cell_collapsed_is_header_only() {
        let cell = GroupedToolUseCell::new(
            "Read".to_string(),
            vec![
                (serde_json::json!({"f": "a"}), serde_json::json!("ok")),
                (serde_json::json!({"f": "b"}), serde_json::json!("ok")),
            ],
        );
        assert_eq!(plain(&cell, false), vec!["● Read (×2)".to_string()]);
    }

    #[test]
    fn grouped_teammate_rows_use_at_names_only_for_spawned_results() {
        let cell = GroupedToolUseCell::new(
            "Agent".into(),
            vec![
                (
                    serde_json::json!({"name":"reviewer","subagent_type":"Explore","description":"Review code"}),
                    serde_json::json!({"status":"teammate_spawned"}),
                ),
                (
                    serde_json::json!({"name":"named-task","subagent_type":"general-purpose","description":"Run checks"}),
                    serde_json::json!({"status":"async_launched"}),
                ),
            ],
        );
        assert_eq!(
            plain(&cell, true),
            vec![
                format!("{TOOL_MARKER}2 background agents launched (↓ manage)"),
                "   ├ @reviewer (Explore)".into(),
                "   └ Agent (Run checks)".into(),
            ]
        );
        let collapsed = plain(&cell, false);
        assert_eq!(collapsed.len(), 1);
        assert!(collapsed[0].contains("reviewer, named-task"));
        assert!(collapsed[0].ends_with("started working ›"));
    }

    #[test]
    fn grouped_tool_use_cell_verbose_lists_child_input_result_pairs() {
        let cell = GroupedToolUseCell::new(
            "Read".to_string(),
            vec![
                (serde_json::json!({"f": "a"}), serde_json::json!("ok")),
                (serde_json::json!({"f": "b"}), serde_json::json!("ok")),
            ],
        );
        let lines = plain(&cell, true);
        assert_eq!(
            lines,
            vec![
                "● Read (×2)".to_string(),
                "  ⎿  {\"f\":\"a\"} → \"ok\"".to_string(),
                "  ⎿  {\"f\":\"b\"} → \"ok\"".to_string(),
            ]
        );
    }

    #[test]
    fn collapsed_read_search_cell_renders_dim_count_line() {
        let cell = CollapsedReadSearchCell::new(
            1,
            1,
            0,
            0,
            0,
            Vec::new(),
            0,
            0,
            false,
            None,
            vec!["Read a.rs".to_string(), "Grep foo".to_string()],
        );
        assert_eq!(plain(&cell, false), vec!["Read · 2 calls".to_string()]);
        let styled = cell.display_lines(80, &Theme::dark(), RenderMode::default());
        assert_eq!(styled[0].spans[0].style.fg, Some(rata(Theme::dark().dim)));
    }

    #[test]
    fn fullscreen_categories_render_in_summary() {
        let cell = CollapsedReadSearchCell::new(
            0,
            0,
            0,
            0,
            2,
            vec!["github".to_string()],
            1,
            1,
            false,
            None,
            Vec::new(),
        );
        assert_eq!(
            plain(&cell, false),
            vec!["Queried github 2 times, ran 1 bash command, wrote 1 memory".to_string()]
        );
    }
}

#[cfg(test)]
mod denial_presentation_tests {
    use super::*;
    #[test]
    fn refusal_recognition_keeps_command_failures_and_successful_quoted_text() {
        assert!(tool_denial_reason(false, Some("Permission denied"), None).is_none());
        assert!(
            tool_denial_reason(true, None, Some("bash: ./private: Permission denied")).is_none()
        );
        assert!(tool_denial_reason(true, None, Some("Process exited with code 1")).is_none());
        assert_eq!(
            tool_denial_reason(
                true,
                None,
                Some("Automatic approval review failed: Bad Request")
            ),
            Some(("Automatic approval review failed: Bad Request".into(), true))
        );
        assert_eq!(
            tool_denial_reason(true, Some("Permission to use Bash has been denied."), None)
                .unwrap()
                .1,
            false
        );
    }
    #[test]
    fn denied_command_panel_retains_command_and_reason() {
        let entry = ActivityEntry {
            id: "denied".into(),
            tool: "Bash".into(),
            input: serde_json::json!({"command":"echo hello"}),
            result: Some(
                serde_json::json!({"error":"Automatic approval review failed: Bad Request"}),
            ),
        };
        let collapsed = single_tool_lines(&entry, 80, &Theme::dark(), false, true);
        let collapsed_text = collapsed
            .iter()
            .map(StyledLine::plain_text)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(collapsed_text.contains("Command declined by auto-review"));
        assert!(!collapsed_text.contains("Bad Request"));
        for width in [12, 40, 80] {
            let expanded = single_tool_lines(&entry, width, &Theme::dark(), true, true);
            let text = expanded
                .iter()
                .map(StyledLine::plain_text)
                .collect::<Vec<_>>()
                .join("\n");
            assert!(text.contains("Why it was denied"));
            assert!(text.contains("Bad Request"));
            assert!(!text.contains("Unsuccessful"));
        }
    }
    #[test]
    fn standalone_refusals_use_the_same_disclosure_and_never_render_an_edit_diff() {
        let result = serde_json::json!({"error": "Permission to use Edit has been denied."});
        let input = serde_json::json!({"file_path":"/work/example.rs","old_string":"before","new_string":"after"});
        let cell = ToolResultCell::new(
            "Edit".into(),
            result.clone(),
            Some("before".into()),
            Some("after".into()),
            Some("/work/example.rs".into()),
            Some(input.clone()),
        );
        let compact = cell
            .styled_lines(80, &Theme::dark(), false)
            .iter()
            .map(StyledLine::plain_text)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(compact.contains("Command declined"));
        assert!(!compact.contains("Why it was denied"));
        let expanded = tool_result_lines(
            "Edit",
            Some(&input),
            &result,
            Some("before"),
            Some("after"),
            Some("/work/example.rs"),
            80,
            &Theme::dark(),
        )
        .iter()
        .map(StyledLine::plain_text)
        .collect::<Vec<_>>()
        .join("\n");
        assert!(expanded.contains("Why it was denied"));
        assert!(!expanded.contains("Added"));
    }
}
