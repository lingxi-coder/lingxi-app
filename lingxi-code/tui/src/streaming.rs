#![forbid(unsafe_code)]
//! Streaming subscriber: applies `TurnEvent`s to `AppState`. (M6-03)
//!
//! Pure mutator. No side effects beyond `state` mutation and
//! `notify.notify_one()`. The render loop calls this on every event
//! received from the bridge channel.

use crate::events::orchestrator_bridge::TurnEvent;
use crate::state::{AppState, CurrentTodo, RenderedMessage, StreamingState};
use tokio::sync::Notify;

/// (SS-08) Resolve the spinner's "current todo" from a `TodoWrite` tool input.
///
/// A `TodoWrite` call replaces the entire session todo list; the spinner shows
/// the first todo that is neither `pending` nor `completed` (claude-code
/// `Spinner.tsx:162` — `tasksV2?.find(t => t.status !== 'pending' && t.status
/// !== 'completed')`). Returns `None` when the input is malformed, the `todos`
/// array is missing/empty, or every todo is pending/completed.
///
/// The TodoWrite wire shape is `{ todos: [{ content, status, activeForm }] }`
/// (`tools/task/src/todo_write.rs`); `content` is the spinner's `subject`
/// fallback and `activeForm` (camelCase) its preferred verb.
fn current_todo_from_todowrite_input(input: &serde_json::Value) -> Option<CurrentTodo> {
    let todos = input.get("todos")?.as_array()?;
    let item = todos.iter().find(|t| {
        let status = t.get("status").and_then(serde_json::Value::as_str);
        // Anything not pending/completed is "active" (in_progress, or any
        // other forward state). A missing status is treated as active too,
        // matching the `!==` semantics of the TS predicate.
        !matches!(status, Some("pending" | "completed"))
    })?;
    let subject = item.get("content").and_then(serde_json::Value::as_str)?.to_string();
    let active_form = item
        .get("activeForm")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string);
    Some(CurrentTodo {
        subject,
        active_form,
    })
}

/// Apply one `TurnEvent` to `state` and signal the renderer.
///
/// Behavior contract:
/// - `TurnStarted` → `state.streaming = Some(StreamingState::new())`.
/// - `TextDelta(s)` → if the last message in `state.messages` is an
///   `AssistantText`, append `s` to its `body`. Otherwise push a NEW
///   `AssistantText` message with body `s`.
/// - `ToolUseStart{..}` / `ToolUseResult{..}` → push placeholder
///   `SystemText` rows (proper rendering lands in M6-04).
/// - `PermissionRequest{..}` → set `state.pending_permission` (M6-05).
/// - `TurnEnded(_)` → clear `state.streaming` and `state.cancel_token`, and set
///   `state.status_line_dirty` to arm one statusline-pump pass (A6).
///
/// After mutation, calls `notify.notify_one()`. The render loop is
/// expected to debounce these to ~30fps.
#[allow(clippy::too_many_lines, reason = "flat per-TurnEvent match dispatcher; one arm per variant")]
pub fn apply_event(state: &mut AppState, ev: TurnEvent, notify: &Notify) {
    match ev {
        TurnEvent::TurnStarted => {
            state.streaming = Some(StreamingState::new());
        }
        TurnEvent::TextDelta(text) => {
            // Append to last AssistantText if present; otherwise push new.
            if let Some(RenderedMessage::AssistantText { body, .. }) = state.messages.last_mut() {
                body.push_str(&text);
            } else {
                state.messages.push(RenderedMessage::AssistantText {
                    body: text,
                    timestamp: chrono::Utc::now().timestamp(),
                });
            }
        }
        TurnEvent::ToolUseStart { id, tool, input } => {
            // M6-04: rich tool-use block. Per-id expanded state lives in
            // `state.expanded` (default false → collapsed header).
            //
            // M7-02 (T13-wire): the tool-call INPUT carries the diff source
            // (Edit's `old_string`/`new_string`/`file_path`, Write's `content`).
            // It must reach the LATER `UserToolResult` render site. We stash it
            // by id here — chosen over a backward scan of `messages` so it stays
            // correct once M7-03 windows the visible message slice.
            // (SS-08) A `TodoWrite` replaces the whole session todo list each
            // call, so its input is the authoritative source for the spinner's
            // "current todo" (claude-code derives `currentTodo` from the live
            // `tasksV2` list — `Spinner.tsx:162`). Refresh on the START event so
            // the verb tracks the new in-progress task as soon as it's written,
            // not only after the (later) tool result returns.
            if tool == "TodoWrite" {
                state.current_todo = current_todo_from_todowrite_input(&input);
            }
            state.tool_call_inputs.insert(id.clone(), input.clone());
            state
                .messages
                .push(RenderedMessage::AssistantToolUse { id, tool, input });
        }
        TurnEvent::ToolUseResult { id, tool, result } => {
            let (old_string, new_string, file_path) = state
                .tool_call_inputs
                .remove(&id)
                .map_or((None, None, None), |input| diff_inputs_for(&tool, &input));
            state.messages.push(RenderedMessage::UserToolResult {
                id,
                tool,
                result,
                old_string,
                new_string,
                file_path,
            });
        }
        TurnEvent::PermissionRequest { tool, input } => {
            // M6-03 bridge variant carries the legacy {tool, input} shape and
            // resolves over a separate channel, so NO resp_tx is attached here.
            // The richer in-process variant rides `TuiPermissionGate`'s
            // `mpsc<PermissionExchange>` (see `permission_bridge.rs` + the
            // root.rs permission pump). Shared helper keeps `started_at` + the
            // dialog-reset + telemetry identical across both paths.
            let default_decision = permission::tool_default(&tool);
            let request = permission::gate::PermissionRequest::ToolUseConfirm {
                tool_name: tool,
                tool_input: input,
                default_decision,
            };
            crate::state::open_permission_dialog(state, request, None, None);
        }
        TurnEvent::TurnEnded(_outcome) => {
            state.streaming = None;
            state.cancel_token = None;
            // (SS-08) Clear the active-todo verb source on turn end, so the
            // next turn's spinner doesn't keep showing the previous turn's
            // `activeForm` until a fresh TodoWrite arrives (claude-code's
            // `currentTodo` is per-turn; documented on `AppState.current_todo`).
            state.current_todo = None;
            // (A6 batch-6 Task 2) Arm one statusline-pump pass — the TUI analog
            // of claude-code's `StatusLine.tsx` re-run on `lastAssistantMessageId`
            // (a turn just produced its final assistant message). The 300ms
            // pump in `root.rs` consumes the flag, builds + runs the command,
            // and re-paints on change. The SOLE dirty trigger: a terminal 429
            // emits `ClientEvent::Error` (not `TurnEnded`), so it does NOT
            // re-arm the statusline — TS-faithful (M8).
            state.status_line_dirty = true;
        }
        TurnEvent::CostUpdated(cost_str) => {
            // M6-06: update the StatusSnapshot cost so the next render
            // pass shows the post-turn dollar amount in the status line.
            state.status.cost = cost_str;
        }
        TurnEvent::ContextPressure { banner } => {
            // (TokenWarning) Store the orchestrator-computed context-pressure
            // banner so the prompt chrome renders it next pass; `None` clears a
            // previously-shown banner once the context drops below the warning
            // threshold (claude-code's `<TokenWarning>` returning null).
            state.context_pressure = banner;
        }
        TurnEvent::TerminalSequence { seq } => {
            // #6: stage the validated terminal escape sequence; the async pump
            // (`pump_terminal_sequence`) writes the bytes to the TUI's stdout —
            // the host that owns the controlling terminal (claude-code `BEo`).
            state.pending_terminal_sequence = Some(seq);
        }
        TurnEvent::CompactionCompleted {
            messages_before,
            messages_after,
            ..
        } => {
            // M7-04: real CompactBoundaryMessage (replaces M6-08's
            // `[Compacted N → M messages]` SystemText placeholder). Renders
            // `✻ Conversation compacted (ctrl+o for history)` (counts retained
            // on the variant for debug/telemetry parity but not rendered).
            state.messages.push(RenderedMessage::CompactBoundary {
                messages_before,
                messages_after,
            });
        }
        TurnEvent::RateLimit {
            status,
            rate_limit_type,
            utilization,
            resets_at,
            claim_resets_at,
            overage_status,
            overage_resets_at,
            overage_disabled_reason,
            fallback_available,
        } => {
            // (Batch-3 Task 9) Compose the claude-code rate-limit notice from
            // the header snapshot. Push only when the composed text DIFFERS
            // from the last rendered one (`state.last_rate_limit_text`), so
            // identical consecutive notices never stack — the orchestrator
            // already dedupes on raw headers, but distinct snapshots can
            // compose to the same text.
            let info = crate::rate_limit_messages::RateLimitInfo {
                status,
                rate_limit_type,
                utilization,
                resets_at,
                claim_resets_at,
                overage_status,
                overage_resets_at,
                overage_disabled_reason,
                fallback_available,
            };
            // (Batch-4 Task 6) Subscription-granular copy: the snapshot the
            // composition root resolved (None until the background fetch
            // lands → default = unknown subscription, TS-conservative).
            let sub = state.subscription_snapshot().unwrap_or_default();
            if let Some(composed) = crate::rate_limit_messages::compose_rate_limit(&info, &sub) {
                if state.last_rate_limit_text.as_deref() != Some(composed.text.as_str()) {
                    state.last_rate_limit_text = Some(composed.text.clone());
                    state.messages.push(RenderedMessage::RateLimit {
                        text: composed.text,
                        upsell: composed.upsell,
                    });
                }
            }
            // (Batch-5 Task 5) Overage-transition notice
            // (`useRateLimitWarningNotification.tsx`): fire ONCE on entering
            // overage when `!isTeamOrEnterprise || hasBillingAccess` (tsx
            // :62); reset the one-shot flag on leaving overage (tsx :70-72).
            // The TS `getIsRemoteMode()` skip is structurally false — the TUI
            // is never remote. The flag is guarded here (not by the text
            // dedupe slot) and survives `/clear`, like the TS component state.
            if crate::rate_limit_messages::is_using_overage(&info) {
                if !state.has_shown_overage_notification
                    && (!sub.is_team_or_enterprise() || sub.has_claude_ai_billing_access())
                {
                    state.has_shown_overage_notification = true;
                    state.messages.push(RenderedMessage::RateLimit {
                        text: crate::rate_limit_messages::using_overage_text(&info, &sub),
                        upsell: None,
                    });
                }
            } else {
                state.has_shown_overage_notification = false;
            }
        }
        TurnEvent::RawUtilization {
            five_hour_utilization,
            five_hour_resets_at,
            seven_day_utilization,
            seven_day_resets_at,
        } => {
            // (Batch-5 Task 4) Store the latest raw per-window snapshot
            // (last-write-wins) for the statusline command input's
            // `rate_limits` field (StatusLine.tsx:50-65). No transcript
            // message — this track is statusline-only, unlike RateLimit.
            state.raw_utilization =
                Some(crate::components::status_line_command::RawUtilizationSnapshot {
                    five_hour_utilization,
                    five_hour_resets_at,
                    seven_day_utilization,
                    seven_day_resets_at,
                });
        }
    }
    notify.notify_one();
}

/// Extract the `(old_string, new_string, file_path)` diff inputs for a diff
/// tool from its call `input` JSON. Returns all-`None` for non-diff tools.
///
/// Mapping (claude-code parity):
///   - `Edit`  → `old_string` / `new_string` / `file_path` keys verbatim.
///   - `Write` → `old = None` (pure add), `new = content`, `file_path`.
///   - `MultiEdit` / `NotebookEdit` → only `file_path` populated; the old/new
///     bodies are multi-hunk (`edits[]`) / cell-shaped and don't map to a
///     single old→new pair. TODO(M8): render their full multi-hunk diff.
///
/// `pub(crate)` so the resume transcript-replay mapper
/// ([`crate::replay::rebuild_messages`]) groups a persisted `ToolResult` under
/// its originating `ToolUse` through the *same* diff-input derivation the live
/// `ToolUseResult` path uses — no duplicated key-mapping logic.
pub(crate) fn diff_inputs_for(
    tool: &str,
    input: &serde_json::Value,
) -> (Option<String>, Option<String>, Option<String>) {
    let str_key = |k: &str| {
        input
            .get(k)
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned)
    };
    match tool {
        "Edit" => (
            str_key("old_string"),
            str_key("new_string"),
            str_key("file_path"),
        ),
        "Write" => (None, str_key("content"), str_key("file_path")),
        // TODO(M8): MultiEdit (`edits[]`) and NotebookEdit (cell-shaped) carry
        // no single old→new pair — surface only the path for now so the header
        // renders without a (wrong) single-hunk diff.
        "MultiEdit" | "NotebookEdit" => (None, None, str_key("file_path")),
        _ => (None, None, None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{AppState, StatusSnapshot};

    fn new_state() -> AppState {
        AppState::new(StatusSnapshot::default())
    }

    #[test]
    fn context_pressure_event_sets_and_clears_the_banner() {
        let mut s = new_state();
        let n = Notify::new();
        // Some(banner) → stored on AppState for the next render pass.
        apply_event(
            &mut s,
            TurnEvent::ContextPressure {
                banner: Some(traits::ContextPressureBanner {
                    text: "12% until auto-compact".into(),
                    level: traits::ContextPressureLevel::Dim,
                }),
            },
            &n,
        );
        let b = s.context_pressure.as_ref().expect("banner stored");
        assert_eq!(b.text, "12% until auto-compact");
        assert_eq!(b.level, traits::ContextPressureLevel::Dim);
        // None → cleared (claude-code's <TokenWarning> returning null once the
        // context drops back below the warning threshold).
        apply_event(&mut s, TurnEvent::ContextPressure { banner: None }, &n);
        assert!(s.context_pressure.is_none(), "None clears the banner");
    }

    /// #6: a `TerminalSequence` event stages the validated escape sequence on
    /// `AppState` for the async `pump_terminal_sequence` to write to stdout.
    #[test]
    fn terminal_sequence_event_stages_pending_write() {
        let mut s = new_state();
        let n = Notify::new();
        assert!(s.pending_terminal_sequence.is_none());
        apply_event(
            &mut s,
            TurnEvent::TerminalSequence {
                seq: "\u{001B}]9;hi\u{0007}".into(),
            },
            &n,
        );
        assert_eq!(
            s.pending_terminal_sequence.as_deref(),
            Some("\u{001B}]9;hi\u{0007}"),
            "the validated sequence must be staged for the terminal-write pump"
        );
    }

    #[test]
    fn text_delta_creates_new_assistant_message_when_buffer_empty() {
        let mut s = new_state();
        let n = Notify::new();
        apply_event(&mut s, TurnEvent::TextDelta("hello".into()), &n);
        assert_eq!(s.messages.len(), 1);
        assert!(matches!(
            s.messages.last(),
            Some(RenderedMessage::AssistantText { body, .. }) if body == "hello"
        ));
    }

    #[test]
    fn five_deltas_concatenate_into_single_message() {
        let mut s = new_state();
        let n = Notify::new();
        for chunk in ["he", "ll", "o ", "wo", "rld"] {
            apply_event(&mut s, TurnEvent::TextDelta(chunk.into()), &n);
        }
        assert_eq!(s.messages.len(), 1);
        assert!(matches!(
            s.messages.last(),
            Some(RenderedMessage::AssistantText { body, .. }) if body == "hello world"
        ));
    }

    #[test]
    fn turn_started_sets_streaming_some() {
        let mut s = new_state();
        let n = Notify::new();
        assert!(s.streaming.is_none());
        apply_event(&mut s, TurnEvent::TurnStarted, &n);
        assert!(s.streaming.is_some());
    }

    #[test]
    fn turn_ended_clears_streaming_and_cancel_token() {
        let mut s = new_state();
        let n = Notify::new();
        s.streaming = Some(StreamingState::new());
        s.cancel_token = Some(tokio_util::sync::CancellationToken::new());
        apply_event(
            &mut s,
            TurnEvent::TurnEnded(traits::TurnOutcome::EndTurn),
            &n,
        );
        assert!(s.streaming.is_none());
        assert!(s.cancel_token.is_none());
    }

    /// (A6 batch-6 Task 2) `TurnEnded` arms the statusline pump — the sole
    /// dirty trigger (TS `StatusLine.tsx` re-runs on `lastAssistantMessageId`,
    /// whose TUI analog is "a turn just ended"). A terminal 429 emits
    /// `ClientEvent::Error`, not `TurnEnded`, so it deliberately does NOT
    /// re-arm the statusline (M8 — TS-faithful).
    // ── SS-08: TodoWrite → AppState.current_todo ─────────────────────────

    #[test]
    fn todowrite_input_picks_first_active_todo() {
        // The first non-pending/non-completed item wins (claude-code
        // `tasksV2?.find(t => t.status !== 'pending' && t.status !== 'completed')`).
        let input = serde_json::json!({
            "todos": [
                { "content": "Setup", "status": "completed", "activeForm": "Setting up" },
                { "content": "Build project", "status": "in_progress", "activeForm": "Compiling" },
                { "content": "Ship", "status": "pending", "activeForm": "Shipping" },
            ]
        });
        let got = current_todo_from_todowrite_input(&input).expect("an active todo");
        assert_eq!(got.subject, "Build project");
        assert_eq!(got.active_form.as_deref(), Some("Compiling"));
    }

    #[test]
    fn todowrite_input_all_pending_or_completed_is_none() {
        let input = serde_json::json!({
            "todos": [
                { "content": "A", "status": "completed", "activeForm": "Doing A" },
                { "content": "B", "status": "pending", "activeForm": "Doing B" },
            ]
        });
        assert!(current_todo_from_todowrite_input(&input).is_none());
    }

    #[test]
    fn todowrite_input_missing_or_empty_todos_is_none() {
        assert!(current_todo_from_todowrite_input(&serde_json::json!({})).is_none());
        assert!(
            current_todo_from_todowrite_input(&serde_json::json!({ "todos": [] })).is_none()
        );
    }

    #[test]
    fn todowrite_tooluse_start_populates_current_todo() {
        use crate::events::orchestrator_bridge::TurnEvent;
        let mut s = new_state();
        let n = Notify::new();
        assert!(s.current_todo.is_none());
        apply_event(
            &mut s,
            TurnEvent::ToolUseStart {
                id: protocol::ToolUseId::new(),
                tool: "TodoWrite".into(),
                input: serde_json::json!({
                    "todos": [
                        { "content": "Run tests", "status": "in_progress", "activeForm": "Running tests" },
                    ]
                }),
            },
            &n,
        );
        let todo = s.current_todo.as_ref().expect("current todo set");
        assert_eq!(todo.subject, "Run tests");
        assert_eq!(todo.active_form.as_deref(), Some("Running tests"));
    }

    #[test]
    fn non_todowrite_tool_leaves_current_todo_untouched() {
        use crate::events::orchestrator_bridge::TurnEvent;
        let mut s = new_state();
        let n = Notify::new();
        s.current_todo = Some(crate::state::CurrentTodo {
            subject: "kept".into(),
            active_form: None,
        });
        apply_event(
            &mut s,
            TurnEvent::ToolUseStart {
                id: protocol::ToolUseId::new(),
                tool: "Read".into(),
                input: serde_json::json!({ "file_path": "/tmp/x" }),
            },
            &n,
        );
        // A non-TodoWrite tool must not clobber the active todo.
        assert_eq!(s.current_todo.as_ref().map(|t| t.subject.as_str()), Some("kept"));
    }

    #[test]
    fn turn_ended_clears_current_todo() {
        // (SS-08) `current_todo` is per-turn — `TurnEnded` must clear it so the
        // next turn's spinner doesn't inherit the stale `activeForm`.
        use crate::events::orchestrator_bridge::TurnEvent;
        let mut s = new_state();
        let n = Notify::new();
        s.current_todo = Some(crate::state::CurrentTodo {
            subject: "Build project".into(),
            active_form: Some("Compiling".into()),
        });
        apply_event(&mut s, TurnEvent::TurnEnded(traits::TurnOutcome::EndTurn), &n);
        assert!(s.current_todo.is_none(), "TurnEnded must clear current_todo");
    }

    #[test]
    fn turn_ended_sets_status_line_dirty() {
        let mut s = new_state();
        let n = Notify::new();
        assert!(!s.status_line_dirty, "dirty starts cleared");
        apply_event(
            &mut s,
            TurnEvent::TurnEnded(traits::TurnOutcome::EndTurn),
            &n,
        );
        assert!(s.status_line_dirty, "TurnEnded arms the statusline pump");
    }

    #[tokio::test]
    async fn apply_event_calls_notify_one() {
        let mut s = new_state();
        let n = Notify::new();
        // Pre-record a permit so we can detect notify_one.
        let waiter = n.notified();
        tokio::pin!(waiter);
        apply_event(&mut s, TurnEvent::TextDelta("x".into()), &n);
        let poll = futures::poll!(waiter.as_mut());
        assert!(matches!(poll, std::task::Poll::Ready(())));
    }

    // ── TurnEvent::RateLimit (batch-3 Task 9) ─────────────────────────────

    /// A `rejected`/`five_hour` snapshot that composes to
    /// `"You've hit your session limit"`.
    fn rate_limit_event(rate_limit_type: &str) -> TurnEvent {
        TurnEvent::RateLimit {
            status: Some("rejected".into()),
            rate_limit_type: Some(rate_limit_type.into()),
            utilization: None,
            resets_at: None,
            claim_resets_at: None,
            overage_status: None,
            overage_resets_at: None,
            overage_disabled_reason: None,
            fallback_available: None,
        }
    }

    fn rate_limit_texts(s: &AppState) -> Vec<&str> {
        s.messages
            .iter()
            .filter_map(|m| match m {
                RenderedMessage::RateLimit { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn rate_limit_event_pushes_one_rendered_message() {
        let mut s = new_state();
        let n = Notify::new();
        apply_event(&mut s, rate_limit_event("five_hour"), &n);
        assert_eq!(
            rate_limit_texts(&s),
            vec!["You've hit your session limit"]
        );
        assert_eq!(
            s.last_rate_limit_text.as_deref(),
            Some("You've hit your session limit")
        );
    }

    #[test]
    fn identical_consecutive_rate_limit_events_do_not_duplicate() {
        let mut s = new_state();
        let n = Notify::new();
        apply_event(&mut s, rate_limit_event("five_hour"), &n);
        apply_event(&mut s, rate_limit_event("five_hour"), &n);
        assert_eq!(rate_limit_texts(&s).len(), 1);
    }

    #[test]
    fn changed_rate_limit_text_pushes_second_message() {
        let mut s = new_state();
        let n = Notify::new();
        apply_event(&mut s, rate_limit_event("five_hour"), &n);
        apply_event(&mut s, rate_limit_event("seven_day"), &n);
        assert_eq!(
            rate_limit_texts(&s),
            vec![
                "You've hit your session limit",
                "You've hit your weekly limit"
            ]
        );
    }

    #[test]
    fn rate_limit_event_composing_nothing_pushes_nothing() {
        let mut s = new_state();
        let n = Notify::new();
        apply_event(
            &mut s,
            TurnEvent::RateLimit {
                status: Some("allowed".into()),
                rate_limit_type: None,
                utilization: None,
                resets_at: None,
                claim_resets_at: None,
                overage_status: None,
                overage_resets_at: None,
                overage_disabled_reason: None,
                fallback_available: None,
            },
            &n,
        );
        assert!(rate_limit_texts(&s).is_empty());
        assert!(s.last_rate_limit_text.is_none());
    }

    #[test]
    fn rejected_rate_limit_message_has_no_upsell_for_unknown_subscription() {
        // (Batch-4 Task 6) `getUpsellMessage` gates on `shouldShowUpsell =
        // isClaudeAISubscriber()` (RateLimitMessage.tsx:26 + :78); the test
        // state has no subscription snapshot → unknown subscription → no
        // upsell. (Pre-batch-4 this asserted the generic `upsell::UPGRADE`
        // proxy — a documented scope-guard.)
        let mut s = new_state();
        let n = Notify::new();
        apply_event(&mut s, rate_limit_event("five_hour"), &n);
        match s.messages.last() {
            Some(RenderedMessage::RateLimit { upsell, .. }) => {
                assert_eq!(upsell, &None);
            }
            other => panic!("expected RateLimit message, got: {other:?}"),
        }
    }

    // ── overage-transition notice (useRateLimitWarningNotification.tsx) ───

    /// A `rejected` snapshot WITH overage allowed — `isUsingOverage`
    /// (claudeAiLimits.ts:406-409). `compose_rate_limit` returns `None` for a
    /// plain `allowed` overage status (rateLimitMessages.ts:51-60), so the
    /// ONLY push from this event is the transition notice itself.
    fn overage_event() -> TurnEvent {
        TurnEvent::RateLimit {
            status: Some("rejected".into()),
            rate_limit_type: Some("five_hour".into()),
            utilization: None,
            resets_at: None,
            claim_resets_at: None,
            overage_status: Some("allowed".into()),
            overage_resets_at: None,
            overage_disabled_reason: None,
            fallback_available: None,
        }
    }

    fn subscription_slot(
        snap: traits::subscription::SubscriptionSnapshot,
    ) -> traits::subscription::SharedSubscription {
        std::sync::Arc::new(std::sync::RwLock::new(Some(snap)))
    }

    #[test]
    fn overage_transition_fires_once() {
        let mut s = new_state();
        let n = Notify::new();
        apply_event(&mut s, overage_event(), &n);
        // five_hour without resetsAt → the no-reset copy (TS :327-330).
        assert_eq!(rate_limit_texts(&s), vec!["You're now using extra usage"]);
        assert!(s.has_shown_overage_notification);
        // Identical second event → no second push: the one-shot flag guards
        // it (tsx :62 `!hasShownOverageNotification`), not the text dedupe.
        apply_event(&mut s, overage_event(), &n);
        assert_eq!(rate_limit_texts(&s).len(), 1);
    }

    #[test]
    fn overage_flag_resets_on_leaving_overage() {
        let mut s = new_state();
        let n = Notify::new();
        apply_event(&mut s, overage_event(), &n);
        assert!(s.has_shown_overage_notification);
        // Leaving overage (status allowed → !isUsingOverage) resets the flag
        // (tsx :70-72).
        apply_event(
            &mut s,
            TurnEvent::RateLimit {
                status: Some("allowed".into()),
                rate_limit_type: None,
                utilization: None,
                resets_at: None,
                claim_resets_at: None,
                overage_status: None,
                overage_resets_at: None,
                overage_disabled_reason: None,
                fallback_available: None,
            },
            &n,
        );
        assert!(!s.has_shown_overage_notification);
        // Re-entering overage fires a second notice.
        apply_event(&mut s, overage_event(), &n);
        assert_eq!(rate_limit_texts(&s).len(), 2);
    }

    #[test]
    fn team_without_billing_access_suppressed() {
        // Suppression arm (tsx :62): `isTeamOrEnterprise && !hasBillingAccess`
        // → no notice; the flag stays UNSET (TS only sets it when it fires).
        let mut s = new_state();
        let n = Notify::new();
        s.subscription = Some(subscription_slot(
            traits::subscription::SubscriptionSnapshot {
                is_subscriber: true,
                subscription_type: Some("team".into()),
                has_extra_usage_enabled: true,
                organization_role: None,
                ..Default::default()
            },
        ));
        apply_event(&mut s, overage_event(), &n);
        assert!(rate_limit_texts(&s).is_empty());
        assert!(!s.has_shown_overage_notification);

        // Team admin (billing access) → notice fires.
        let mut s = new_state();
        s.subscription = Some(subscription_slot(
            traits::subscription::SubscriptionSnapshot {
                is_subscriber: true,
                subscription_type: Some("team".into()),
                has_extra_usage_enabled: true,
                organization_role: Some("admin".into()),
                ..Default::default()
            },
        ));
        apply_event(&mut s, overage_event(), &n);
        assert_eq!(rate_limit_texts(&s), vec!["You're now using extra usage"]);
        assert!(s.has_shown_overage_notification);
    }

    // ── TurnEvent::RawUtilization (batch-5 Task 4) ────────────────────────

    /// A `RawUtilization` event stores the snapshot on
    /// `state.raw_utilization`; a second event OVERWRITES (last-write-wins,
    /// mirroring claude-code's per-response `rawUtilization` tracking).
    #[test]
    fn raw_utilization_event_updates_state() {
        let mut s = new_state();
        let n = Notify::new();
        // Defaults pin to None before any event.
        assert!(s.raw_utilization.is_none());
        apply_event(
            &mut s,
            TurnEvent::RawUtilization {
                five_hour_utilization: Some(0.42),
                five_hour_resets_at: Some(1_750_000_000),
                seven_day_utilization: Some(0.07),
                seven_day_resets_at: Some(1_750_600_000),
            },
            &n,
        );
        let snap = s.raw_utilization.expect("snapshot stored");
        assert_eq!(snap.five_hour_utilization, Some(0.42));
        assert_eq!(snap.five_hour_resets_at, Some(1_750_000_000));
        assert_eq!(snap.seven_day_utilization, Some(0.07));
        assert_eq!(snap.seven_day_resets_at, Some(1_750_600_000));

        // Second event overwrites (including dropping a window back to None).
        apply_event(
            &mut s,
            TurnEvent::RawUtilization {
                five_hour_utilization: Some(0.5),
                five_hour_resets_at: Some(1_750_000_100),
                seven_day_utilization: None,
                seven_day_resets_at: None,
            },
            &n,
        );
        let snap = s.raw_utilization.expect("snapshot stored");
        assert_eq!(snap.five_hour_utilization, Some(0.5));
        assert_eq!(snap.five_hour_resets_at, Some(1_750_000_100));
        assert_eq!(snap.seven_day_utilization, None);
        assert_eq!(snap.seven_day_resets_at, None);
    }

    #[test]
    fn permission_request_populates_pending_slot() {
        let mut s = new_state();
        let n = Notify::new();
        apply_event(
            &mut s,
            TurnEvent::PermissionRequest {
                tool: "Bash".into(),
                input: serde_json::json!({"command": "ls"}),
            },
            &n,
        );
        assert!(s.pending_permission.is_some());
        assert_eq!(s.pending_permission.as_ref().unwrap().tool(), "Bash");
    }
}
