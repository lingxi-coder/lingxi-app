//! `OpenAI` streaming SSE → canonical `StreamEvent`s.
//!
//! `OpenAI` streams `choices[0].delta`. Text arrives as `delta.content`
//! fragments; tool calls arrive as `delta.tool_calls[]` fragments keyed by an
//! `index`, where the FIRST fragment for an index carries `id`+`function.name`
//! and later fragments carry `function.arguments` string chunks. This decoder
//! reassembles those into Anthropic-shaped content blocks: a text block at
//! canonical index 0 (opened lazily) and one `tool_use` block per `OpenAI`
//! tool-call index, assigned canonical indices after the text block.

use crate::codec::SseDecoder;
use api_client::types::{
    ContentBlockApi, ContentDelta, MessageDeltaPayload, MessageResponse, StreamEvent, UsageApi,
};
use protocol::ToolUseId;
use serde_json::Value;
use std::collections::BTreeMap;

use super::decode::map_finish_reason;

/// Reassembles an `OpenAI` chat-completions stream into canonical events.
#[allow(clippy::struct_excessive_bools)]
pub struct OpenAiSseDecoder {
    started: bool,
    reasoning_open: bool,
    reasoning_index: u32,
    text_open: bool,
    text_index: u32,
    /// next canonical block index to assign.
    next_index: u32,
    /// `OpenAI` tool-call index → canonical block index (for opened tool blocks).
    tool_index: BTreeMap<u64, u32>,
    stop_reason: Option<String>,
    usage: Option<UsageApi>,
    /// Set once a terminal sequence (`MessageDelta` + `MessageStop`) has been
    /// emitted, so the `[DONE]` and `finish()` paths stay mutually exclusive.
    done: bool,
}

impl OpenAiSseDecoder {
    /// Construct a fresh decoder.
    #[must_use]
    pub fn new() -> Self {
        Self {
            started: false,
            reasoning_open: false,
            reasoning_index: 0,
            text_open: false,
            text_index: 0,
            next_index: 0,
            tool_index: BTreeMap::new(),
            stop_reason: None,
            usage: None,
            done: false,
        }
    }

    fn ensure_started(&mut self, root: &Value, out: &mut Vec<StreamEvent>) {
        if self.started {
            return;
        }
        self.started = true;
        let id = root
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let model = root
            .get("model")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        out.push(StreamEvent::MessageStart {
            message: MessageResponse {
                id,
                model,
                content: Vec::new(),
                stop_reason: None,
                usage: UsageApi::default(),
            },
        });
    }

    fn handle_reasoning(&mut self, text: &str, out: &mut Vec<StreamEvent>) {
        if text.is_empty() {
            return;
        }
        if !self.reasoning_open {
            self.reasoning_open = true;
            self.reasoning_index = self.next_index;
            self.next_index += 1;
            out.push(StreamEvent::ContentBlockStart {
                index: self.reasoning_index,
                content_block: ContentBlockApi::Thinking {
                    thinking: String::new(),
                    signature: None,
                },
            });
        }
        out.push(StreamEvent::ContentBlockDelta {
            index: self.reasoning_index,
            delta: ContentDelta::ThinkingDelta {
                thinking: text.to_string(),
            },
        });
    }

    fn handle_text(&mut self, text: &str, out: &mut Vec<StreamEvent>) {
        if text.is_empty() {
            return;
        }
        if !self.text_open {
            self.text_open = true;
            self.text_index = self.next_index;
            self.next_index += 1;
            out.push(StreamEvent::ContentBlockStart {
                index: self.text_index,
                content_block: ContentBlockApi::Text {
                    text: String::new(),
                },
            });
        }
        out.push(StreamEvent::ContentBlockDelta {
            index: self.text_index,
            delta: ContentDelta::TextDelta {
                text: text.to_string(),
            },
        });
    }

    fn handle_tool_fragment(&mut self, tc: &Value, out: &mut Vec<StreamEvent>) {
        let Some(oai_idx) = tc.get("index").and_then(Value::as_u64) else {
            return;
        };
        let idx = *self.tool_index.entry(oai_idx).or_insert_with(|| {
            // First fragment for this tool-call index: open a ToolUse block.
            let canonical = self.next_index;
            self.next_index += 1;
            let name = tc
                .get("function")
                .and_then(|f| f.get("name"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            out.push(StreamEvent::ContentBlockStart {
                index: canonical,
                content_block: ContentBlockApi::ToolUse {
                    id: ToolUseId::new(),
                    name,
                    input: Value::Object(serde_json::Map::new()),
                },
            });
            canonical
        });
        if let Some(args) = tc
            .get("function")
            .and_then(|f| f.get("arguments"))
            .and_then(Value::as_str)
        {
            if !args.is_empty() {
                out.push(StreamEvent::ContentBlockDelta {
                    index: idx,
                    delta: ContentDelta::InputJsonDelta {
                        partial_json: args.to_string(),
                    },
                });
            }
        }
    }

    fn close_open_blocks(&mut self, out: &mut Vec<StreamEvent>) {
        if self.reasoning_open {
            out.push(StreamEvent::ContentBlockStop {
                index: self.reasoning_index,
            });
            self.reasoning_open = false;
        }
        if self.text_open {
            out.push(StreamEvent::ContentBlockStop {
                index: self.text_index,
            });
            self.text_open = false;
        }
        let mut tool_indices: Vec<u32> = self.tool_index.values().copied().collect();
        tool_indices.sort_unstable();
        for idx in tool_indices {
            out.push(StreamEvent::ContentBlockStop { index: idx });
        }
        self.tool_index.clear();
    }
}

impl Default for OpenAiSseDecoder {
    fn default() -> Self {
        Self::new()
    }
}

impl SseDecoder for OpenAiSseDecoder {
    fn push(&mut self, data: &str) -> Vec<StreamEvent> {
        let mut out = Vec::new();
        let data = data.trim();
        if data == "[DONE]" {
            if !self.done {
                self.done = true;
                self.close_open_blocks(&mut out);
                out.push(StreamEvent::MessageDelta {
                    delta: MessageDeltaPayload {
                        stop_reason: self.stop_reason.clone(),
                    },
                    usage: self.usage,
                });
                out.push(StreamEvent::MessageStop);
            }
            return out;
        }
        let Ok(root) = serde_json::from_str::<Value>(data) else {
            return out; // ignore unparseable keepalive lines
        };
        self.ensure_started(&root, &mut out);

        // Usage-only chunk (empty choices + usage) when include_usage is set.
        if let Some(u) = root.get("usage").filter(|v| !v.is_null()) {
            self.usage = Some(super::decode::usage_from_value(u));
        }

        let Some(choice) = root.get("choices").and_then(|c| c.get(0)) else {
            return out;
        };
        if let Some(delta) = choice.get("delta") {
            if let Some(r) = delta.get("reasoning_content").and_then(Value::as_str) {
                self.handle_reasoning(r, &mut out);
            }
            if let Some(text) = delta.get("content").and_then(Value::as_str) {
                self.handle_text(text, &mut out);
            }
            if let Some(tcs) = delta.get("tool_calls").and_then(Value::as_array) {
                for tc in tcs {
                    self.handle_tool_fragment(tc, &mut out);
                }
            }
        }
        if let Some(fr) = choice.get("finish_reason").and_then(Value::as_str) {
            self.stop_reason = Some(map_finish_reason(fr));
        }
        out
    }

    fn finish(&mut self) -> Vec<StreamEvent> {
        // If the stream closed without a `[DONE]` sentinel, emit the same
        // terminal sequence `[DONE]` would have — a complete `MessageDelta`
        // carrying the captured stop_reason + usage, then `MessageStop` — so
        // neither is silently dropped. No-op once `[DONE]` already terminated.
        let mut out = Vec::new();
        if self.started && !self.done {
            self.done = true;
            self.close_open_blocks(&mut out);
            out.push(StreamEvent::MessageDelta {
                delta: MessageDeltaPayload {
                    stop_reason: self.stop_reason.clone(),
                },
                usage: self.usage,
            });
            out.push(StreamEvent::MessageStop);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use api_client::types::{ContentBlockApi, ContentDelta, StreamEvent};

    fn run(frames: &[&str]) -> Vec<StreamEvent> {
        let mut d = OpenAiSseDecoder::new();
        let mut out = Vec::new();
        for f in frames {
            out.extend(d.push(f));
        }
        out.extend(d.finish());
        out
    }

    #[test]
    fn text_stream_emits_start_delta_stop_and_message_stop() {
        let events = run(&[
            r#"{"id":"c1","model":"gpt-4o","choices":[{"index":0,"delta":{"role":"assistant","content":""}}]}"#,
            r#"{"choices":[{"index":0,"delta":{"content":"hel"}}]}"#,
            r#"{"choices":[{"index":0,"delta":{"content":"lo"}}]}"#,
            r#"{"choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}"#,
            "[DONE]",
        ]);
        assert!(matches!(
            events.first(),
            Some(StreamEvent::MessageStart { .. })
        ));
        // text block opened, two text deltas, then closed
        assert!(events.iter().any(|e| matches!(
            e,
            StreamEvent::ContentBlockDelta { delta: ContentDelta::TextDelta { text }, .. } if text == "hel"
        )));
        assert!(events
            .iter()
            .any(|e| matches!(e, StreamEvent::ContentBlockStop { .. })));
        assert!(matches!(events.last(), Some(StreamEvent::MessageStop)));
    }

    #[test]
    fn tool_call_stream_reassembles_index_keyed_fragments() {
        let events = run(&[
            r#"{"id":"c2","model":"gpt-4o","choices":[{"index":0,"delta":{"role":"assistant"}}]}"#,
            r#"{"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"call_a","type":"function","function":{"name":"Read","arguments":""}}]}}]}"#,
            r#"{"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"arguments":"{\"path\""}}]}}]}"#,
            r#"{"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"arguments":":\"/x\"}"}}]}}]}"#,
            r#"{"choices":[{"index":0,"delta":{},"finish_reason":"tool_calls"}]}"#,
            "[DONE]",
        ]);
        // A ToolUse block was started with name "Read"
        assert!(events.iter().any(|e| matches!(
            e,
            StreamEvent::ContentBlockStart { content_block: ContentBlockApi::ToolUse { name, .. }, .. } if name == "Read"
        )));
        // Its arguments arrived as InputJsonDelta fragments
        let json_frag: String = events
            .iter()
            .filter_map(|e| match e {
                StreamEvent::ContentBlockDelta {
                    delta: ContentDelta::InputJsonDelta { partial_json },
                    ..
                } => Some(partial_json.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(json_frag, "{\"path\":\"/x\"}");
        assert!(matches!(events.last(), Some(StreamEvent::MessageStop)));
    }

    #[test]
    fn done_without_finish_still_terminates() {
        let events = run(&[
            r#"{"choices":[{"index":0,"delta":{"content":"hi"}}]}"#,
            "[DONE]",
        ]);
        assert!(matches!(events.last(), Some(StreamEvent::MessageStop)));
    }

    #[test]
    fn ignores_blank_and_role_only_deltas() {
        let events = run(&[
            r#"{"id":"c","model":"gpt-4o","choices":[{"index":0,"delta":{"role":"assistant"}}]}"#,
            "[DONE]",
        ]);
        // Only MessageStart + MessageStop; no spurious content blocks.
        assert!(matches!(
            events.first(),
            Some(StreamEvent::MessageStart { .. })
        ));
        assert!(matches!(events.last(), Some(StreamEvent::MessageStop)));
        assert!(!events
            .iter()
            .any(|e| matches!(e, StreamEvent::ContentBlockStart { .. })));
    }

    #[test]
    fn exactly_one_message_stop_after_done_then_finish() {
        // The real pump pushes every frame (incl. `[DONE]`) then calls finish()
        // on wire close — it must NOT double-emit the terminal events.
        let events = run(&[
            r#"{"id":"c","model":"gpt-4o","choices":[{"index":0,"delta":{"content":"hi"}}]}"#,
            r#"{"choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}"#,
            "[DONE]",
        ]);
        assert_eq!(
            events
                .iter()
                .filter(|e| matches!(e, StreamEvent::MessageStop))
                .count(),
            1,
            "exactly one MessageStop"
        );
        assert_eq!(
            events
                .iter()
                .filter(|e| matches!(e, StreamEvent::MessageDelta { .. }))
                .count(),
            1,
            "exactly one MessageDelta"
        );
    }

    #[test]
    fn reasoning_stream_emits_thinking_block_before_text() {
        let events = run(&[
            r#"{"id":"c1","model":"deepseek-r1","choices":[{"index":0,"delta":{"role":"assistant","reasoning_content":"th"}}]}"#,
            r#"{"choices":[{"index":0,"delta":{"reasoning_content":"ink"}}]}"#,
            r#"{"choices":[{"index":0,"delta":{"content":"answer"}}]}"#,
            r#"{"choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}"#,
            "[DONE]",
        ]);
        // There is a ContentBlockStart with a Thinking block
        assert!(events.iter().any(|e| matches!(
            e,
            StreamEvent::ContentBlockStart {
                content_block: ContentBlockApi::Thinking { .. },
                ..
            }
        )));
        // Two ThinkingDelta events with "th" and "ink"
        let thinking_deltas: Vec<&str> = events
            .iter()
            .filter_map(|e| match e {
                StreamEvent::ContentBlockDelta {
                    delta: ContentDelta::ThinkingDelta { thinking },
                    ..
                } => Some(thinking.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(thinking_deltas, vec!["th", "ink"]);
        // There is also a text block
        assert!(events.iter().any(|e| matches!(
            e,
            StreamEvent::ContentBlockStart {
                content_block: ContentBlockApi::Text { .. },
                ..
            }
        )));
        // Exactly one MessageStop
        assert_eq!(
            events
                .iter()
                .filter(|e| matches!(e, StreamEvent::MessageStop))
                .count(),
            1,
            "exactly one MessageStop"
        );
    }

    #[test]
    fn finish_without_done_emits_terminal_delta_and_stop() {
        // Stream ends WITHOUT a `[DONE]` sentinel: finish() must still flush a
        // MessageDelta carrying the captured stop_reason + usage and a single
        // MessageStop, so neither is dropped.
        let events = run(&[
            r#"{"id":"c","model":"gpt-4o","choices":[{"index":0,"delta":{"content":"hi"}}]}"#,
            r#"{"choices":[{"index":0,"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":4,"completion_tokens":1}}"#,
        ]);
        let (stop_reason, usage) = events
            .iter()
            .find_map(|e| match e {
                StreamEvent::MessageDelta { delta, usage } => {
                    Some((delta.stop_reason.clone(), *usage))
                }
                _ => None,
            })
            .expect("finish() must emit a MessageDelta");
        assert_eq!(stop_reason.as_deref(), Some("end_turn"));
        assert_eq!(usage.expect("usage carried").input_tokens, 4);
        assert!(matches!(events.last(), Some(StreamEvent::MessageStop)));
        assert_eq!(
            events
                .iter()
                .filter(|e| matches!(e, StreamEvent::MessageStop))
                .count(),
            1
        );
    }
}
