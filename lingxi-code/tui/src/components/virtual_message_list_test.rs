use super::*;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::RenderedMessage;

    fn user(body: &str) -> RenderedMessage {
        RenderedMessage::UserText {
            body: body.to_string(),
            timestamp: 0,
        }
    }

    // ---- (M7-03 review) Per-variant measurement lock tests --------------
    //
    // These pin `measured_height` for EVERY current `RenderedMessage`
    // variant so a careless edit to `render_text_for_measure` (the proxy)
    // — or to a `render_message` renderer that the proxy is supposed to
    // mirror — trips a test. Expected counts are hand-specified (the real
    // `render_message` returns an iocraft element, not text, so we cannot
    // derive them from the render path here) and annotated with WHY they
    // hold. When you add a variant or change a renderer's line layout, you
    // MUST update `render_text_for_measure` and these pins together.
    //
    // KNOWN PROXY LIMITATION pinned on purpose: for `UserToolResult` the
    // proxy measures only the `result` payload — it does NOT account for the
    // collapsed first-line/`(+N lines)` form, line/byte truncation, or the
    // M7-02 diff body (`old_string`/`new_string`/`file_path`). The diff case
    // below documents that current behavior so the eventual M7-04/05
    // measurement↔renderer unification deliberately changes these pins.

    #[test]
    fn measured_height_pins_user_text() {
        // UserTextMessage draws the body verbatim (the `> ` prefix adds
        // columns, not rows). 3 newline-separated lines → 3 rows.
        let m = RenderedMessage::UserText {
            body: "l1\nl2\nl3".into(),
            timestamp: 0,
        };
        assert_eq!(measured_height(&m, 80), 3);
    }

    #[test]
    fn measured_height_pins_assistant_text() {
        // AssistantTextMessage now routes the body through markdown; the `● `
        // marker + 2-col continuation indent add columns, not rows. `one\ntwo`
        // is one paragraph with a soft break → 2 flattened lines → 2 rows.
        use crate::components::messages::assistant_text::render_assistant_text_to_string;
        let body = "one\ntwo";
        let m = RenderedMessage::AssistantText {
            body: body.into(),
            timestamp: 0,
        };
        assert_eq!(measured_height(&m, 80), 2);
        // measurement == render: pin against the renderer's own oracle at the
        // same width `measured_height` uses (A2).
        assert_eq!(
            measured_height(&m, 80),
            render_assistant_text_to_string(body, 80).lines().count()
        );
    }

    #[test]
    fn measured_height_assistant_text_matches_renderer_fenced_code() {
        use crate::components::messages::assistant_text::render_assistant_text_to_string;
        // Fenced code block: the ``` fence lines are dropped by markdown
        // flattening, so the renderer draws fewer rows than the raw input.
        // Measurement MUST equal the renderer's flattened row count, not the
        // raw `.lines()` proxy (which would over-count the dropped fences).
        let body = "intro\n```rust\nlet x = 1;\n```\noutro";
        let m = RenderedMessage::AssistantText {
            body: body.into(),
            timestamp: 0,
        };
        let rendered = render_assistant_text_to_string(body, 80);
        assert_eq!(
            measured_height(&m, 80),
            rendered.lines().count(),
            "assistant-text measurement must equal the renderer's flattened row count"
        );
    }

    #[test]
    fn measured_height_pins_system_text() {
        // SystemText draws the body verbatim (color only, no extra rows).
        let m = RenderedMessage::SystemText {
            body: "a\nb\nc\nd".into(),
            timestamp: 0,
            is_error: false,
        };
        assert_eq!(measured_height(&m, 80), 4);
    }

    #[test]
    fn measured_height_pins_assistant_tool_use() {
        // Proxy renders the single-line collapsed form `● {tool}(…)`.
        // Width 80 → 1 row regardless of `input`.
        let m = RenderedMessage::AssistantToolUse {
            id: ToolUseId::new(),
            tool: "Read".into(),
            input: serde_json::json!({ "file_path": "/x", "extra": [1, 2, 3] }),
        };
        assert_eq!(measured_height(&m, 80), 1);
    }

    #[test]
    fn measured_height_pins_user_tool_result_plain_string() {
        // Proxy uses the result string body. 2 lines → 2 rows.
        let m = RenderedMessage::UserToolResult {
            id: ToolUseId::new(),
            tool: "Bash".into(),
            result: serde_json::json!("line1\nline2"),
            old_string: None,
            new_string: None,
            file_path: None,
        };
        assert_eq!(measured_height(&m, 80), 2);
    }

    #[test]
    fn measured_height_structured_bash_matches_renderer() {
        // (gap-3) A live Bash result is the structured `{"stdout":…}` object.
        // The renderer extracts stdout/stderr through the bash-output span
        // pipeline, so measurement MUST route through the same pipeline (NOT
        // count the raw single-line JSON). 2 stdout lines + 1 stderr → 3 rows.
        use crate::components::messages::bash_output::render_bash_output_spans;
        let m = RenderedMessage::UserToolResult {
            id: ToolUseId::new(),
            tool: "Bash".into(),
            result: serde_json::json!({
                "stdout": "line1\nline2",
                "stderr": "err",
                "interrupted": false,
                "isImage": false,
                "noOutputExpected": false,
            }),
            old_string: None,
            new_string: None,
            file_path: None,
        };
        let rendered: String = render_bash_output_spans("line1\nline2", "err")
            .into_iter()
            .map(|s| s.text)
            .collect();
        assert_eq!(measured_height(&m, 80), rendered.lines().count());
        assert_eq!(measured_height(&m, 80), 3);
    }

    #[test]
    fn measured_height_pins_user_tool_result_edit_diff() {
        // Edit-diff case: old/new strings + file_path are set, but the
        // CURRENT proxy measures only the `result` payload (it ignores the
        // diff fields). A non-string `result` falls back to `to_string()`
        // → the JSON literal `"applied"` (with quotes), one line → 1 row.
        // This pins the known proxy/renderer drift so the M7-04/05
        // unification must consciously revise it.
        let m = RenderedMessage::UserToolResult {
            id: ToolUseId::new(),
            tool: "Edit".into(),
            result: serde_json::json!("applied"),
            old_string: Some("fn a() {}\nold line".into()),
            new_string: Some("fn a() {}\nnew line\nextra".into()),
            file_path: Some("/src/a.rs".into()),
        };
        // Proxy text == "applied" (str body) → 1 row. The 2-line old / 3-line
        // new diff body is NOT counted by today's proxy — documented above.
        assert_eq!(measured_height(&m, 80), 1);
    }

    // ---- (M7-04) Per-variant measurement lock tests --------------------
    //
    // Pin `measured_height` for each of the 10 batch-1 variants so the proxy
    // stays in lock-step with the `render_*_to_string` renderers. When a
    // renderer changes how a variant lays out rows you MUST update both the
    // proxy arm in `render_text_for_measure` AND the pin here.

    #[test]
    fn measured_height_pins_assistant_thinking_m7_04() {
        // Collapsed → single header+hint line.
        let collapsed = RenderedMessage::AssistantThinking {
            thinking: "anything".into(),
            expanded: false,
        };
        assert_eq!(measured_height(&collapsed, 80), 1);
        // Expanded → header `∴ Thinking…` (1) + gap=1 blank row (1) + 2 body
        // lines (indented 2) = 4.
        let expanded = RenderedMessage::AssistantThinking {
            thinking: "Step one.\nStep two.".into(),
            expanded: true,
        };
        assert_eq!(measured_height(&expanded, 80), 4);
    }

    // ---- (M7-04 review) measurement == render for markdown bodies --------
    //
    // The expanded thinking / verbose advisor renderers flatten their body
    // through `render::markdown` (fenced code blocks drop the ``` fences,
    // trailing blank lines collapse). `measured_height` MUST count the SAME
    // flattened text the renderer draws, so we derive the expected row count
    // from the renderer's own string output rather than the raw input. These
    // tests FAIL against a raw-line proxy (which over-counts the dropped
    // fence/blank rows) and pass once measurement routes through the
    // renderer's flattening.

    #[test]
    fn measured_height_thinking_expanded_matches_renderer_fenced_code() {
        use crate::components::messages::thinking::{render_thinking_to_string, ThinkingProps};
        // Fenced code block: the ``` fence lines are dropped by markdown
        // flattening, so the renderer draws fewer rows than the raw input.
        let body = "intro\n```rust\nlet x = 1;\n```\noutro";
        let msg = RenderedMessage::AssistantThinking {
            thinking: body.into(),
            expanded: true,
        };
        let rendered = render_thinking_to_string(ThinkingProps {
            thinking: body.into(),
            expanded: true,
        });
        let expected_rows = rendered.lines().count();
        assert_eq!(
            measured_height(&msg, 80),
            expected_rows,
            "measured_height must equal the renderer's flattened row count"
        );
    }

    #[test]
    fn measured_height_thinking_expanded_matches_renderer_trailing_blank() {
        use crate::components::messages::thinking::{render_thinking_to_string, ThinkingProps};
        // Trailing blank line: markdown flattening drops it, so the renderer
        // draws one fewer row than the raw `.lines()` proxy would (raw
        // `"text\n\n".lines()` yields `["text", ""]` → 2 body rows, but the
        // flattened body is just `"text"` → 1 body row).
        let body = "text\n\n";
        let msg = RenderedMessage::AssistantThinking {
            thinking: body.into(),
            expanded: true,
        };
        let rendered = render_thinking_to_string(ThinkingProps {
            thinking: body.into(),
            expanded: true,
        });
        assert_eq!(measured_height(&msg, 80), rendered.lines().count());
    }

    #[test]
    fn measured_height_pins_redacted_thinking_m7_04() {
        // `✻ Thinking…` — single line.
        assert_eq!(
            measured_height(&RenderedMessage::AssistantRedactedThinking, 80),
            1
        );
    }

    #[test]
    fn measured_height_pins_compact_boundary_m7_04() {
        // (compact-boundary-marginy) boundary line + blank row above/below =
        // 3 rows; counts are not rendered.
        let m = RenderedMessage::CompactBoundary {
            messages_before: 50,
            messages_after: 5,
        };
        assert_eq!(measured_height(&m, 80), 3);
    }

    #[test]
    fn measured_height_pins_system_text_rich_m7_04() {
        // Info → body verbatim (3 lines). Warning → `● ` marker (cols) + body
        // (1 line). Both: marker adds columns, not rows.
        let info = RenderedMessage::SystemTextRich {
            body: "a\nb\nc".into(),
            level: crate::state::SystemLevel::Info,
        };
        assert_eq!(measured_height(&info, 80), 3);
        let warn = RenderedMessage::SystemTextRich {
            body: "Approaching context limit.".into(),
            level: crate::state::SystemLevel::Warning,
        };
        assert_eq!(measured_height(&warn, 80), 1);
    }

    #[test]
    fn measured_height_pins_system_api_error_m7_04() {
        // Not truncated → error body (1) + retry footer (1) = 2 lines.
        let plain = RenderedMessage::SystemApiError {
            error: "529 Overloaded".into(),
            retry_attempt: 4,
            retry_in_seconds: 3,
            max_retries: 10,
            truncated: false,
        };
        assert_eq!(measured_height(&plain, 80), 2);
        // Truncated → error+`…` (1) + expand-hint (1) + retry footer (1) = 3.
        let trunc = RenderedMessage::SystemApiError {
            error: "boom".into(),
            retry_attempt: 5,
            retry_in_seconds: 1,
            max_retries: 10,
            truncated: true,
        };
        assert_eq!(measured_height(&trunc, 80), 3);
    }

    #[test]
    fn measured_height_pins_rate_limit_m7_04() {
        // No upsell → 1 line. With upsell → 2 lines.
        let no_upsell = RenderedMessage::RateLimit {
            text: "You've hit your usage limit.".into(),
            upsell: None,
        };
        assert_eq!(measured_height(&no_upsell, 80), 1);
        let with_upsell = RenderedMessage::RateLimit {
            text: "You've hit your usage limit.".into(),
            upsell: Some("/upgrade to increase your usage limit.".into()),
        };
        assert_eq!(measured_height(&with_upsell, 80), 2);
    }

    #[test]
    fn measured_height_pins_shutdown_m7_04() {
        // Request + reason → header (1) + Reason (1) = 2 lines.
        let request = RenderedMessage::Shutdown {
            from: "agent-2".into(),
            reason: Some("task done".into()),
            rejected: false,
        };
        assert_eq!(measured_height(&request, 80), 2);
        // Rejected + reason → header (1) + Reason (1) + tail (1) = 3 lines.
        let rejected = RenderedMessage::Shutdown {
            from: "agent-2".into(),
            reason: Some("still working".into()),
            rejected: true,
        };
        assert_eq!(measured_height(&rejected, 80), 3);
    }

    #[test]
    fn measured_height_pins_advisor_m7_04() {
        // Non-verbose result → single review line.
        let result = RenderedMessage::Advisor {
            kind: crate::state::AdvisorKind::Result {
                text: "Looks good.".into(),
            },
            verbose: false,
        };
        assert_eq!(measured_height(&result, 80), 1);
        // Error → single `Advisor unavailable (…)` line.
        let err = RenderedMessage::Advisor {
            kind: crate::state::AdvisorKind::Error {
                error_code: "503".into(),
            },
            verbose: false,
        };
        assert_eq!(measured_height(&err, 80), 1);
    }

    #[test]
    fn measured_height_advisor_verbose_matches_renderer_fenced_code() {
        use crate::components::messages::advisor::{render_advisor_to_string, AdvisorProps};
        // Verbose result body flows through markdown flattening, so the ```
        // fence lines are dropped — measurement must match the renderer's
        // flattened row count, not the raw `.lines()` proxy.
        let text = "summary\n```\ncode line\n```\ntail";
        let kind = crate::state::AdvisorKind::Result { text: text.into() };
        let msg = RenderedMessage::Advisor {
            kind: kind.clone(),
            verbose: true,
        };
        let rendered = render_advisor_to_string(AdvisorProps {
            kind,
            verbose: true,
            ..Default::default()
        });
        assert_eq!(
            measured_height(&msg, 80),
            rendered.lines().count(),
            "verbose advisor measurement must equal the renderer's flattened row count"
        );
    }

    #[test]
    fn measured_height_pins_hook_progress_m7_04() {
        // Running plural → single line.
        let running = RenderedMessage::HookProgress {
            event: "SessionStart".into(),
            count: 3,
            transcript_summary: false,
        };
        assert_eq!(measured_height(&running, 80), 1);
        // Transcript singular → single line.
        let transcript = RenderedMessage::HookProgress {
            event: "PreToolUse".into(),
            count: 1,
            transcript_summary: true,
        };
        assert_eq!(measured_height(&transcript, 80), 1);
    }

    #[test]
    fn measured_height_pins_plan_approval_m7_04() {
        // Request → header (1) + 2 plan-content lines (1+1) + Plan file (1) = 4.
        let request = RenderedMessage::PlanApproval {
            kind: crate::state::PlanApprovalKind::Request {
                from: "agent-3".into(),
                plan_content: "1. Do X\n2. Do Y".into(),
                plan_file_path: Some("/tmp/plan.md".into()),
            },
        };
        assert_eq!(measured_height(&request, 80), 4);
        // Approved → header (1) + tail. The tail line
        // "You can now proceed with implementation. Your plan mode
        // restrictions have been lifted." is 86 cols → wraps to 2 rows at
        // width 80, so total = 3 rows. (At a wider width it would be 2.)
        let approved = RenderedMessage::PlanApproval {
            kind: crate::state::PlanApprovalKind::Approved { name: "you".into() },
        };
        assert_eq!(measured_height(&approved, 80), 3);
        // Sanity: at a width that holds the tail on one line, total = 2 rows.
        assert_eq!(measured_height(&approved, 120), 2);
        // Rejected + feedback → header (1) + Feedback (1) + tail (1) = 3 lines.
        let rejected = RenderedMessage::PlanApproval {
            kind: crate::state::PlanApprovalKind::Rejected {
                name: "you".into(),
                feedback: Some("too risky".into()),
            },
        };
        assert_eq!(measured_height(&rejected, 80), 3);
    }

    // ---- (M7-05) Per-variant measurement lock tests --------------------
    //
    // Pin `measured_height` for each of the 12 batch-2 user variants so the
    // proxy stays in lock-step with the `render_*_to_string` renderers. For the
    // markdown/ANSI-bodied variants (plan, local-command output, bash output)
    // the EXPECTED row count is derived from the renderer's OWN string oracle —
    // not a raw `.lines()` recount — so measurement == render BY CONSTRUCTION
    // (the M7-04 fenced-code / trailing-blank desync class can't recur). Those
    // tests include a fenced code block / trailing blank in the body to pin it.

    #[test]
    fn measured_height_pins_user_bash_input_m7_05() {
        // `! {command}` — single line (prefix adds columns, not rows).
        let m = RenderedMessage::UserBashInput {
            command: "ls -la".into(),
        };
        assert_eq!(measured_height(&m, 80), 1);
    }

    #[test]
    fn measured_height_bash_output_matches_renderer_ansi() {
        use crate::components::messages::bash_output::render_bash_output_spans;
        // ANSI-coded multi-line body: the parser strips escape codes, so the
        // measured row count must equal the parsed (escape-free) row count, not
        // the raw byte body's `.lines()`.
        let stdout = "\x1b[31mred line\x1b[0m\nplain line";
        let m = RenderedMessage::UserBashOutput {
            stdout: stdout.into(),
            stderr: String::new(),
        };
        let rendered: String = render_bash_output_spans(stdout, "")
            .into_iter()
            .map(|s| s.text)
            .collect();
        assert_eq!(measured_height(&m, 80), rendered.lines().count());
        // Sanity: 2 visual lines.
        assert_eq!(measured_height(&m, 80), 2);
    }

    #[test]
    fn measured_height_pins_user_command_m7_05() {
        let m = RenderedMessage::UserCommand {
            command: "model".into(),
            args: "sonnet".into(),
            is_skill: false,
        };
        assert_eq!(measured_height(&m, 80), 1);
    }

    #[test]
    fn measured_height_local_output_matches_renderer_fenced_code() {
        use crate::components::messages::local_command_output::render_local_output_to_string;
        // Fenced code block in the markdown body: flattening drops the ```
        // fence lines, so measurement must equal the renderer's flattened row
        // count, NOT the raw `.lines()` of the input.
        let stdout = "intro\n```rust\nlet x = 1;\n```\noutro";
        let m = RenderedMessage::UserLocalCommandOutput {
            stdout: stdout.into(),
            stderr: String::new(),
        };
        let rendered = render_local_output_to_string(stdout, "");
        assert_eq!(
            measured_height(&m, 80),
            rendered.lines().count(),
            "local-output measurement must equal the renderer's flattened row count"
        );
    }

    #[test]
    fn measured_height_pins_user_memory_input_m7_05() {
        // `# {input}` (1) + saving line (1) = 2 rows.
        let m = RenderedMessage::UserMemoryInput {
            input: "prefer tabs".into(),
        };
        assert_eq!(measured_height(&m, 80), 2);
    }

    #[test]
    fn measured_height_plan_matches_renderer_trailing_blank() {
        use crate::components::messages::plan::render_plan_to_string;
        // Trailing blank line in the markdown body: flattening drops it, so the
        // measured row count must equal the renderer's flattened output (header
        // + flattened body), not a raw `.lines()` recount.
        let body = "step one\n\n";
        let m = RenderedMessage::UserPlan {
            plan_content: body.into(),
        };
        let rendered = render_plan_to_string(body);
        assert_eq!(
            measured_height(&m, 80),
            rendered.lines().count(),
            "plan measurement must equal the renderer's flattened row count"
        );
    }

    #[test]
    fn measured_height_pins_user_prompt_m7_05() {
        // Short prompt → body verbatim (3 lines → 3 rows).
        let m = RenderedMessage::UserPrompt {
            text: "a\nb\nc".into(),
        };
        assert_eq!(measured_height(&m, 80), 3);
    }

    #[test]
    fn measured_height_pins_user_resource_update_m7_05() {
        // One update line; with reason still one line.
        let m = RenderedMessage::UserResourceUpdate {
            updates: vec![("fs".into(), "x.rs".into(), Some("changed".into()))],
        };
        assert_eq!(measured_height(&m, 80), 1);
        // Two updates → two lines.
        let m2 = RenderedMessage::UserResourceUpdate {
            updates: vec![
                ("a".into(), "x".into(), None),
                ("b".into(), "y".into(), None),
            ],
        };
        assert_eq!(measured_height(&m2, 80), 2);
    }

    #[test]
    fn measured_height_pins_user_image_m7_05() {
        // `[Image #N]` — single line.
        let m = RenderedMessage::UserImage {
            image_id: Some(3),
            metadata: None,
            source_path: None,
        };
        assert_eq!(measured_height(&m, 80), 1);
    }

    #[test]
    fn measured_height_pins_attachment_m7_05() {
        // Single dim summary line.
        let m = RenderedMessage::Attachment {
            attachment: crate::components::messages::attachment::Attachment::File {
                display_path: "a.rs".into(),
                num_lines: 10,
                truncated: false,
            },
        };
        assert_eq!(measured_height(&m, 80), 1);
    }

    #[test]
    fn measured_height_pins_grouped_tool_use_m7_05() {
        // Proxy uses the collapsed form → single `● {tool} (×N)` header line.
        let m = RenderedMessage::GroupedToolUse {
            tool: "Read".into(),
            group_id: ToolUseId::new(),
            entries: vec![
                (serde_json::json!({}), serde_json::json!({"content": "a"})),
                (serde_json::json!({}), serde_json::json!({"content": "b"})),
            ],
        };
        assert_eq!(measured_height(&m, 80), 1);
    }

    #[test]
    fn measured_height_pins_collapsed_read_search_m7_05() {
        // Single gutter+summary line.
        let m = RenderedMessage::CollapsedReadSearch {
            search_count: 2,
            read_count: 1,
            list_count: 0,
            is_active: false,
            group_id: ToolUseId::new(),
            entries: vec![],
            mem_read: 0,
            mem_search: 0,
            mem_write: 0,
        };
        assert_eq!(measured_height(&m, 80), 1);
        // All-zero counts → empty proxy text → 1 row (a blank logical line).
        let empty = RenderedMessage::CollapsedReadSearch {
            search_count: 0,
            read_count: 0,
            list_count: 0,
            is_active: false,
            group_id: ToolUseId::new(),
            entries: vec![],
            mem_read: 0,
            mem_search: 0,
            mem_write: 0,
        };
        assert_eq!(measured_height(&empty, 80), 1);
    }

    #[test]
    fn single_line_message_measures_one() {
        assert_eq!(measured_height(&user("hi"), 80), 1);
    }

    #[test]
    fn three_newlines_measure_three_lines() {
        assert_eq!(measured_height(&user("a\nb\nc"), 80), 3);
    }

    #[test]
    fn long_line_wraps_at_width() {
        // 25 chars at width 10 → ceil(25/10) = 3 rows.
        let body = "x".repeat(25);
        assert_eq!(measured_height(&user(&body), 10), 3);
    }

    #[test]
    fn empty_user_body_measures_zero() {
        // §A4: an empty (or `(no content)`, or only-stripped-tags) user body is
        // suppressed → empty View → 0 rows. (Was 1 before the guard.)
        assert_eq!(measured_height(&user(""), 80), 0);
        assert_eq!(measured_height(&user("(no content)"), 80), 0);
        assert_eq!(measured_height(&user("<context>x</context>"), 80), 0);
    }

    #[test]
    fn empty_text_branch_measures_one() {
        // A genuinely empty *measured text* for a non-suppressed variant still
        // occupies one row (the `text.is_empty()` branch of `measured_height`).
        let empty_system = RenderedMessage::SystemText {
            body: String::new(),
            timestamp: 0,
            is_error: false,
        };
        assert_eq!(measured_height(&empty_system, 80), 1);
    }

    #[test]
    fn height_cache_builds_per_index_and_total() {
        let msgs = vec![user("one"), user("a\nb\nc"), user("two")];
        let cache = HeightCache::build(&msgs, 80);
        assert_eq!(cache.height_at(0), 1);
        assert_eq!(cache.height_at(1), 3);
        assert_eq!(cache.height_at(2), 1);
        assert_eq!(cache.total_lines(), 5);
        assert_eq!(cache.width(), 80);
    }

    #[test]
    fn height_cache_recomputes_on_width_change() {
        let msgs = vec![user(&"x".repeat(20))]; // 20 cols
        let mut cache = HeightCache::build(&msgs, 80); // ceil(20/80) = 1
        assert_eq!(cache.total_lines(), 1);
        cache.recompute(&msgs, 10); // ceil(20/10) = 2
        assert_eq!(cache.total_lines(), 2);
        assert_eq!(cache.width(), 10);
    }

    #[test]
    fn height_cache_empty_is_zero_total() {
        let cache = HeightCache::build(&[], 80);
        assert_eq!(cache.total_lines(), 0);
    }

    #[test]
    fn span_start_is_cumulative_prefix_sum() {
        // heights [1, 3, 1] → starts [0, 1, 4]; span_start(len) == total.
        let msgs = vec![user("one"), user("a\nb\nc"), user("two")];
        let cache = HeightCache::build(&msgs, 80);
        assert_eq!(cache.span_start(0), 0);
        assert_eq!(cache.span_start(1), 1);
        assert_eq!(cache.span_start(2), 4);
        assert_eq!(cache.span_start(3), 5); // one past the end == total
        assert_eq!(cache.span_start(99), 5); // out of range clamps to total
    }

    #[test]
    fn message_index_at_line_binary_search() {
        // heights [1, 3, 1] → spans m0=[0,1) m1=[1,4) m2=[4,5).
        let msgs = vec![user("one"), user("a\nb\nc"), user("two")];
        let cache = HeightCache::build(&msgs, 80);
        assert_eq!(cache.message_index_at_line(0), 0); // start of m0
        assert_eq!(cache.message_index_at_line(1), 1); // start of m1
        assert_eq!(cache.message_index_at_line(2), 1); // inside m1
        assert_eq!(cache.message_index_at_line(3), 1); // last line of m1
        assert_eq!(cache.message_index_at_line(4), 2); // start of m2
                                                       // line >= total clamps to the last valid index.
        assert_eq!(cache.message_index_at_line(5), 2);
        assert_eq!(cache.message_index_at_line(999), 2);
    }

    #[test]
    fn message_index_at_line_empty_cache_is_zero() {
        let cache = HeightCache::build(&[], 80);
        assert_eq!(cache.message_index_at_line(0), 0);
        assert_eq!(cache.message_index_at_line(42), 0);
    }

    #[test]
    fn message_index_at_line_matches_linear_scan_over_mixed_log() {
        // Cross-check the O(log n) binary search against a brute-force
        // linear scan for every line of a mixed-height log.
        let msgs = mixed_log(); // heights [1, 50, 1, 1], total 53
        let cache = HeightCache::build(&msgs, 80);
        let total = cache.total_lines();
        for line in 0..total {
            // Linear reference: first message whose span_end > line.
            let mut acc = 0usize;
            let mut expected = 0usize;
            for i in 0..msgs.len() {
                let end = acc + cache.height_at(i);
                if end > line {
                    expected = i;
                    break;
                }
                acc = end;
            }
            assert_eq!(
                cache.message_index_at_line(line),
                expected,
                "mismatch at line {line}"
            );
        }
    }

    #[test]
    fn window_at_offset_zero_shows_tail_lines() {
        // 4 messages of height [1,1,1,1] = 4 total lines; viewport 3.
        let msgs = vec![user("m0"), user("m1"), user("m2"), user("m3")];
        let cache = HeightCache::build(&msgs, 80);
        let win = render_window(&msgs, &cache, 0, 3);
        // bottom_line = 4, top_line = 1 → messages 1..=3 visible (m1,m2,m3).
        assert_eq!(win.first_index, 1);
        assert_eq!(win.last_index, 3);
        assert_eq!(win.skip_top_lines, 0);
        assert_eq!(win.take_lines, 3);
        assert_eq!(win.indices().collect::<Vec<_>>(), vec![1, 2, 3]);
    }

    #[test]
    fn window_empty_log_is_empty() {
        let cache = HeightCache::build(&[], 80);
        let win = render_window(&[], &cache, 0, 5);
        assert!(win.is_empty());
    }

    #[test]
    fn window_zero_viewport_is_empty() {
        let msgs = vec![user("m0")];
        let cache = HeightCache::build(&msgs, 80);
        let win = render_window(&msgs, &cache, 0, 0);
        assert!(win.is_empty());
    }

    // Heights: m0=1, m1=50, m2=1, m3=1  → total = 53 lines.
    fn mixed_log() -> Vec<RenderedMessage> {
        vec![
            user("m0"),                      // 1 line
            user(&vec!["x"; 50].join("\n")), // 50 lines
            user("m2"),                      // 1 line
            user("m3"),                      // 1 line
        ]
    }

    #[test]
    fn mixed_offset_zero_shows_tail_into_tall_message() {
        let msgs = mixed_log();
        let cache = HeightCache::build(&msgs, 80);
        assert_eq!(cache.total_lines(), 53);
        // viewport 10, offset 0 → top_line = 43, bottom_line = 53.
        // m1 spans [1,51), m2 [51,52), m3 [52,53).
        let win = render_window(&msgs, &cache, 0, 10);
        assert_eq!(win.first_index, 1); // tall message is partly visible
        assert_eq!(win.last_index, 3);
        assert_eq!(win.skip_top_lines, 42); // hide first 42 of m1's 50 lines
    }

    #[test]
    fn mixed_scrolled_into_tall_message_middle() {
        let msgs = mixed_log();
        let cache = HeightCache::build(&msgs, 80);
        // offset 20 → bottom_line = 33, top_line = 23. Only m1 (spans [1,51)).
        let win = render_window(&msgs, &cache, 20, 10);
        assert_eq!(win.first_index, 1);
        assert_eq!(win.last_index, 1);
        assert_eq!(win.skip_top_lines, 22); // top_line(23) - span_start(1) = 22
    }

    #[test]
    fn mixed_scrolled_to_top_shows_first_message() {
        let msgs = mixed_log();
        let cache = HeightCache::build(&msgs, 80);
        // max_offset = 53 - 10 = 43. offset 43 → top_line 0, bottom_line 10.
        let win = render_window(&msgs, &cache, 43, 10);
        assert_eq!(win.first_index, 0);
        assert_eq!(win.skip_top_lines, 0);
        // m0 [0,1), m1 [1,51) → window covers m0 and start of m1.
        assert_eq!(win.last_index, 1);
    }

    #[test]
    fn mixed_offset_over_max_is_clamped() {
        let msgs = mixed_log();
        let cache = HeightCache::build(&msgs, 80);
        // offset 9999 clamps to max_offset 43 → identical to the top window.
        let win = render_window(&msgs, &cache, 9999, 10);
        assert_eq!(win.first_index, 0);
        assert_eq!(win.skip_top_lines, 0);
    }

    #[test]
    fn window_render_count_bounded_by_viewport_not_log_size() {
        // 5000 single-line messages, viewport 20 → window holds ~20 (+overscan),
        // never 5000.
        let msgs: Vec<RenderedMessage> = (0..5000).map(|i| user(&format!("m{i}"))).collect();
        let cache = HeightCache::build(&msgs, 80);
        assert_eq!(cache.total_lines(), 5000);
        let win = render_window(&msgs, &cache, 0, 20);
        let count = win.indices().count();
        assert!(
            count <= 21,
            "window rendered {count} messages, expected <= 21"
        );
        assert!(count >= 20, "window should fill the viewport, got {count}");
    }
}
