//! Backend-neutral active-turn (in-flight streaming) state. (M5, 2.1.198)
//!
//! Folds streaming [`TurnEvent`]s from the orchestrator bridge into the
//! scrollback message list LIVE, tracking the per-turn state a renderer needs
//! to paint the in-flight region: which assistant text block is currently
//! receiving deltas, which tool calls have started but not yet returned (so a
//! running indicator can be shown), and whether a turn is streaming at all.
//!
//! Backend-neutral by design — `tui-rata` consumes it directly; the legacy
//! iocraft backend keeps its own `tui::streaming::apply_event` (same
//! semantics, its own `TurnEvent`/`AppState` types) until it is deleted.

use std::collections::HashMap;

use crate::message::RenderedMessage;
use crate::orchestrator_bridge::TurnEvent;

/// In-flight turn state, folded from bridge [`TurnEvent`]s.
///
/// Owns NO messages — [`ActiveTurn::apply`] appends to the caller's message
/// list so completed content lives in the same scrollback it always did; this
/// struct only remembers which entries are still "live" (growing text, running
/// tools).
#[derive(Debug, Default)]
pub struct ActiveTurn {
    /// `true` between `TurnStarted` and `TurnEnded`.
    streaming: bool,
    /// Index (into the caller's message list) of the `AssistantText` block
    /// currently receiving `TextDelta`s. Reset by any non-text event so a
    /// text → tool → text sequence yields separate blocks (and a new turn
    /// never appends to the previous turn's final message).
    text_idx: Option<usize>,
    /// Tool ids started but not yet resulted, in start order.
    running_tools: Vec<protocol::ToolUseId>,
    /// Latest heartbeat age for each running tool. Kept outside the transcript
    /// so periodic liveness updates never grow scrollback.
    tool_heartbeats: HashMap<protocol::ToolUseId, u64>,
    /// Tool-call inputs stashed by id at `ToolUseStart`, consumed at
    /// `ToolUseResult` to derive the diff fields (`old_string`/`new_string`/
    /// `file_path`) the result renderer uses for Edit/Write.
    tool_inputs: HashMap<protocol::ToolUseId, serde_json::Value>,
}

impl ActiveTurn {
    /// Fresh (idle) state.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether a turn is currently streaming.
    #[must_use]
    pub fn is_streaming(&self) -> bool {
        self.streaming
    }

    /// Whether the tool call `id` has started but not yet returned (drives the
    /// renderer's running indicator on the matching tool-use row).
    #[must_use]
    pub fn is_tool_running(&self, id: &protocol::ToolUseId) -> bool {
        self.running_tools.contains(id)
    }

    /// Whether any tool call is still in flight.
    #[must_use]
    pub fn has_running_tools(&self) -> bool {
        !self.running_tools.is_empty()
    }

    /// Latest elapsed time reported for a running tool, if any.
    #[must_use]
    pub fn tool_elapsed_ms(&self, id: &protocol::ToolUseId) -> Option<u64> {
        self.tool_heartbeats.get(id).copied()
    }

    /// Fold one streaming event into `messages` + this state. Message-list
    /// growth mirrors the iocraft `tui::streaming::apply_event` contract:
    ///
    /// - `TurnStarted` → mark streaming; reset per-turn state.
    /// - `TextDelta(s)` → append to the turn's live `AssistantText` block, or
    ///   open a new one when none is live.
    /// - `ThinkingDelta(t)` → push a collapsed `AssistantThinking` block (the
    ///   bridge fires once per completed block).
    /// - `ToolUseStart` → push an `AssistantToolUse` row immediately (rendered
    ///   with a running indicator until its result arrives) + stash the input.
    /// - `ToolUseResult` → push the paired `UserToolResult` row with diff
    ///   fields derived from the stashed input; the tool stops "running".
    /// - `CompactionCompleted` → push a `CompactBoundary` (de-duped when the
    ///   last message already is one).
    /// - `TurnEnded` → clear streaming + all per-turn state (a cancelled turn
    ///   may leave tools unresulted; their running markers are dropped here).
    /// - Everything else (cost/status/permission variants) is a no-op — the
    ///   app layer handles those out of band.
    pub fn apply(&mut self, event: TurnEvent, messages: &mut Vec<RenderedMessage>) {
        match event {
            TurnEvent::TurnStarted => {
                self.streaming = true;
                self.text_idx = None;
                self.running_tools.clear();
                self.tool_inputs.clear();
                self.tool_heartbeats.clear();
            }
            TurnEvent::TextDelta(delta) => {
                if let Some(RenderedMessage::AssistantText { body, .. }) =
                    self.text_idx.and_then(|i| messages.get_mut(i))
                {
                    body.push_str(&delta);
                } else {
                    messages.push(RenderedMessage::AssistantText {
                        body: delta,
                        timestamp: 0,
                    });
                    self.text_idx = Some(messages.len() - 1);
                }
            }
            TurnEvent::ThinkingDelta(thinking) => {
                self.text_idx = None;
                messages.push(RenderedMessage::AssistantThinking {
                    thinking,
                    expanded: false,
                });
            }
            TurnEvent::ToolUseStart { id, tool, input } => {
                self.text_idx = None;
                self.tool_inputs.insert(id.clone(), input.clone());
                self.running_tools.push(id.clone());
                messages.push(RenderedMessage::AssistantToolUse { id, tool, input });
            }
            TurnEvent::ToolHeartbeat { id, elapsed_ms, .. } => {
                if self.running_tools.contains(&id) {
                    self.tool_heartbeats.insert(id, elapsed_ms);
                }
            }
            TurnEvent::ToolHeartbeatBatch { heartbeats } => {
                for heartbeat in heartbeats.drain() {
                    self.apply(
                        TurnEvent::ToolHeartbeat {
                            id: heartbeat.id,
                            tool: heartbeat.tool,
                            elapsed_ms: heartbeat.elapsed_ms,
                        },
                        messages,
                    );
                }
            }
            TurnEvent::ToolUseResult { id, tool, result } => {
                self.text_idx = None;
                self.running_tools.retain(|r| r != &id);
                self.tool_heartbeats.remove(&id);
                let (old_string, new_string, file_path) = self
                    .tool_inputs
                    .remove(&id)
                    .map_or((None, None, None), |input| diff_inputs_for(&tool, &input));
                messages.push(RenderedMessage::UserToolResult {
                    id,
                    tool,
                    result,
                    old_string,
                    new_string,
                    file_path,
                });
            }
            TurnEvent::CompactionCompleted {
                messages_before,
                messages_after,
                summary,
                ..
            } => {
                if !matches!(
                    messages.last(),
                    Some(RenderedMessage::CompactBoundary { .. })
                ) {
                    messages.push(RenderedMessage::CompactBoundary {
                        messages_before,
                        messages_after,
                        summary,
                    });
                }
            }
            TurnEvent::TurnEnded(_) => {
                self.streaming = false;
                self.text_idx = None;
                self.running_tools.clear();
                self.tool_inputs.clear();
                self.tool_heartbeats.clear();
            }
            _ => {}
        }
    }
}

/// Derive the diff-source fields the `UserToolResult` renderer needs from a
/// tool-call input (mirrors the iocraft `tui::streaming::diff_inputs_for`,
/// which stays in place until the iocraft backend is deleted).
#[must_use]
pub fn diff_inputs_for(
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
        // MultiEdit (`edits[]`) and NotebookEdit (cell-shaped) carry no single
        // old→new pair — surface only the path so the header renders without a
        // (wrong) single-hunk diff.
        "MultiEdit" | "NotebookEdit" => (None, None, str_key("file_path")),
        _ => (None, None, None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use traits::TurnOutcome;

    fn start_tool(id: &protocol::ToolUseId, tool: &str, input: serde_json::Value) -> TurnEvent {
        TurnEvent::ToolUseStart {
            id: id.clone(),
            tool: tool.to_string(),
            input,
        }
    }

    #[test]
    fn text_delta_opens_then_grows_the_live_assistant_block() {
        let mut at = ActiveTurn::new();
        let mut msgs = Vec::new();
        at.apply(TurnEvent::TurnStarted, &mut msgs);
        assert!(at.is_streaming());
        at.apply(TurnEvent::TextDelta("Hel".into()), &mut msgs);
        at.apply(TurnEvent::TextDelta("lo".into()), &mut msgs);
        assert_eq!(msgs.len(), 1);
        match &msgs[0] {
            RenderedMessage::AssistantText { body, .. } => assert_eq!(body, "Hello"),
            other => panic!("expected AssistantText, got {other:?}"),
        }
    }

    #[test]
    fn new_turn_never_appends_to_previous_turns_text() {
        let mut at = ActiveTurn::new();
        let mut msgs = Vec::new();
        at.apply(TurnEvent::TurnStarted, &mut msgs);
        at.apply(TurnEvent::TextDelta("first turn".into()), &mut msgs);
        at.apply(TurnEvent::TurnEnded(TurnOutcome::EndTurn), &mut msgs);
        at.apply(TurnEvent::TurnStarted, &mut msgs);
        at.apply(TurnEvent::TextDelta("second turn".into()), &mut msgs);
        assert_eq!(msgs.len(), 2, "second turn opens a NEW text block");
        match &msgs[1] {
            RenderedMessage::AssistantText { body, .. } => assert_eq!(body, "second turn"),
            other => panic!("expected AssistantText, got {other:?}"),
        }
    }

    #[test]
    fn tool_start_marks_running_and_result_clears_it() {
        let mut at = ActiveTurn::new();
        let mut msgs = Vec::new();
        let id = protocol::ToolUseId::new();
        at.apply(TurnEvent::TurnStarted, &mut msgs);
        at.apply(
            start_tool(&id, "Read", serde_json::json!({"file_path": "/tmp/x"})),
            &mut msgs,
        );
        assert!(at.is_tool_running(&id));
        assert!(at.has_running_tools());
        assert!(matches!(
            msgs.last(),
            Some(RenderedMessage::AssistantToolUse { .. })
        ));
        at.apply(
            TurnEvent::ToolUseResult {
                id: id.clone(),
                tool: "Read".into(),
                result: serde_json::json!({"content": "hello"}),
            },
            &mut msgs,
        );
        assert!(!at.is_tool_running(&id));
        assert!(!at.has_running_tools());
        assert!(matches!(
            msgs.last(),
            Some(RenderedMessage::UserToolResult { .. })
        ));
    }

    #[test]
    fn heartbeat_updates_live_state_without_growing_transcript() {
        let mut at = ActiveTurn::new();
        let mut msgs = Vec::new();
        let id = protocol::ToolUseId::new();
        at.apply(TurnEvent::TurnStarted, &mut msgs);
        at.apply(start_tool(&id, "Bash", serde_json::json!({})), &mut msgs);
        let before = msgs.len();
        at.apply(
            TurnEvent::ToolHeartbeat {
                id: id.clone(),
                tool: "Bash".into(),
                elapsed_ms: 9_000,
            },
            &mut msgs,
        );
        assert_eq!(msgs.len(), before);
        assert_eq!(at.tool_elapsed_ms(&id), Some(9_000));
        at.apply(
            TurnEvent::ToolUseResult {
                id: id.clone(),
                tool: "Bash".into(),
                result: serde_json::json!({}),
            },
            &mut msgs,
        );
        assert_eq!(at.tool_elapsed_ms(&id), None);
    }

    #[test]
    fn text_after_tool_result_starts_a_new_block() {
        let mut at = ActiveTurn::new();
        let mut msgs = Vec::new();
        let id = protocol::ToolUseId::new();
        at.apply(TurnEvent::TurnStarted, &mut msgs);
        at.apply(TurnEvent::TextDelta("before".into()), &mut msgs);
        at.apply(start_tool(&id, "Read", serde_json::json!({})), &mut msgs);
        at.apply(
            TurnEvent::ToolUseResult {
                id,
                tool: "Read".into(),
                result: serde_json::json!("ok"),
            },
            &mut msgs,
        );
        at.apply(TurnEvent::TextDelta("after".into()), &mut msgs);
        assert_eq!(msgs.len(), 4);
        match (&msgs[0], &msgs[3]) {
            (
                RenderedMessage::AssistantText { body: b0, .. },
                RenderedMessage::AssistantText { body: b3, .. },
            ) => {
                assert_eq!(b0, "before");
                assert_eq!(b3, "after");
            }
            other => panic!("expected text blocks at 0 and 3, got {other:?}"),
        }
    }

    #[test]
    fn edit_result_carries_diff_fields_from_the_stashed_input() {
        let mut at = ActiveTurn::new();
        let mut msgs = Vec::new();
        let id = protocol::ToolUseId::new();
        at.apply(TurnEvent::TurnStarted, &mut msgs);
        at.apply(
            start_tool(
                &id,
                "Edit",
                serde_json::json!({
                    "file_path": "/tmp/a.rs",
                    "old_string": "old",
                    "new_string": "new"
                }),
            ),
            &mut msgs,
        );
        at.apply(
            TurnEvent::ToolUseResult {
                id,
                tool: "Edit".into(),
                result: serde_json::json!({}),
            },
            &mut msgs,
        );
        match msgs.last() {
            Some(RenderedMessage::UserToolResult {
                old_string,
                new_string,
                file_path,
                ..
            }) => {
                assert_eq!(old_string.as_deref(), Some("old"));
                assert_eq!(new_string.as_deref(), Some("new"));
                assert_eq!(file_path.as_deref(), Some("/tmp/a.rs"));
            }
            other => panic!("expected UserToolResult, got {other:?}"),
        }
    }

    #[test]
    fn thinking_delta_pushes_collapsed_thinking_block() {
        let mut at = ActiveTurn::new();
        let mut msgs = Vec::new();
        at.apply(TurnEvent::TurnStarted, &mut msgs);
        at.apply(TurnEvent::ThinkingDelta("reasoning...".into()), &mut msgs);
        match msgs.last() {
            Some(RenderedMessage::AssistantThinking { thinking, expanded }) => {
                assert_eq!(thinking, "reasoning...");
                assert!(!expanded);
            }
            other => panic!("expected AssistantThinking, got {other:?}"),
        }
        // Text after thinking opens a fresh block (doesn't append to thinking).
        at.apply(TurnEvent::TextDelta("answer".into()), &mut msgs);
        assert!(matches!(
            msgs.last(),
            Some(RenderedMessage::AssistantText { body, .. }) if body == "answer"
        ));
    }

    #[test]
    fn turn_ended_clears_streaming_and_dangling_running_tools() {
        let mut at = ActiveTurn::new();
        let mut msgs = Vec::new();
        let id = protocol::ToolUseId::new();
        at.apply(TurnEvent::TurnStarted, &mut msgs);
        at.apply(start_tool(&id, "Bash", serde_json::json!({})), &mut msgs);
        assert!(at.is_streaming());
        assert!(at.has_running_tools());
        // Cancelled turn: no result ever arrives; TurnEnded still clears.
        at.apply(TurnEvent::TurnEnded(TurnOutcome::EndTurn), &mut msgs);
        assert!(!at.is_streaming());
        assert!(!at.has_running_tools());
        // The already-appended rows STAY in scrollback.
        assert_eq!(msgs.len(), 1);
    }

    #[test]
    fn compaction_completed_pushes_deduped_boundary() {
        let mut at = ActiveTurn::new();
        let mut msgs = Vec::new();
        let ev = || TurnEvent::CompactionCompleted {
            messages_before: 10,
            messages_after: 3,
            bytes_saved: 1024,
            summary: "Summary:\nimportant context".to_string(),
        };
        at.apply(ev(), &mut msgs);
        at.apply(ev(), &mut msgs);
        assert_eq!(msgs.len(), 1, "consecutive boundaries de-dupe");
        assert!(matches!(
            msgs[0],
            RenderedMessage::CompactBoundary {
                messages_before: 10,
                messages_after: 3,
                ref summary
            } if summary == "Summary:\nimportant context"
        ));
    }

    #[test]
    fn diff_inputs_for_write_and_unknown_tools() {
        let (o, n, p) = diff_inputs_for(
            "Write",
            &serde_json::json!({"file_path": "/tmp/w.txt", "content": "body"}),
        );
        assert_eq!(o, None);
        assert_eq!(n.as_deref(), Some("body"));
        assert_eq!(p.as_deref(), Some("/tmp/w.txt"));
        assert_eq!(
            diff_inputs_for("Bash", &serde_json::json!({"command": "ls"})),
            (None, None, None)
        );
    }
}
