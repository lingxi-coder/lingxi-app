//! Google `Gemini` `:streamGenerateContent?alt=sse` → canonical `StreamEvent`s.
//!
//! Each SSE chunk is a `generateContent`-shaped JSON (`candidates[0].content.
//! parts` + optional `finishReason` + `usageMetadata`). Text arrives as
//! incremental `text` parts; `functionCall` parts arrive WHOLE (no fragment
//! reassembly). There is no `[DONE]` sentinel — `finish()` emits the terminal
//! `MessageDelta` + `MessageStop` exactly once.

use crate::codec::SseDecoder;
use api_client::types::{
    ContentBlockApi, ContentDelta, MessageDeltaPayload, MessageResponse, StreamEvent, UsageApi,
};
use protocol::ToolUseId;
use serde_json::Value;

use super::decode::{map_finish_reason, usage_from_value};

/// Reassembles a `Gemini` streaming response into canonical events.
#[allow(clippy::struct_excessive_bools)]
pub struct GeminiSseDecoder {
    started: bool,
    text_open: bool,
    text_index: u32,
    next_index: u32,
    saw_tool: bool,
    stop_reason: Option<String>,
    usage: Option<UsageApi>,
    done: bool,
}

impl GeminiSseDecoder {
    /// Construct a fresh decoder.
    #[must_use]
    pub fn new() -> Self {
        Self {
            started: false,
            text_open: false,
            text_index: 0,
            next_index: 0,
            saw_tool: false,
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
        let model = root
            .get("modelVersion")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        out.push(StreamEvent::MessageStart {
            message: MessageResponse {
                id: String::new(),
                model,
                content: Vec::new(),
                stop_reason: None,
                usage: UsageApi::default(),
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

    fn handle_function_call(&mut self, fc: &Value, out: &mut Vec<StreamEvent>) {
        self.saw_tool = true;
        let index = self.next_index;
        self.next_index += 1;
        let name = fc
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let args = fc
            .get("args")
            .cloned()
            .unwrap_or_else(|| Value::Object(serde_json::Map::new()));
        out.push(StreamEvent::ContentBlockStart {
            index,
            content_block: ContentBlockApi::ToolUse {
                id: ToolUseId::new(),
                name,
                input: Value::Object(serde_json::Map::new()),
            },
        });
        // Gemini sends the whole args at once → one InputJsonDelta, then close.
        out.push(StreamEvent::ContentBlockDelta {
            index,
            delta: ContentDelta::InputJsonDelta {
                partial_json: args.to_string(),
            },
        });
        out.push(StreamEvent::ContentBlockStop { index });
    }

    fn terminal_stop_reason(&self) -> Option<String> {
        if self.saw_tool {
            Some("tool_use".to_string())
        } else {
            self.stop_reason.clone()
        }
    }
}

impl Default for GeminiSseDecoder {
    fn default() -> Self {
        Self::new()
    }
}

impl SseDecoder for GeminiSseDecoder {
    fn push(&mut self, data: &str) -> Vec<StreamEvent> {
        let mut out = Vec::new();
        let data = data.trim();
        let Ok(root) = serde_json::from_str::<Value>(data) else {
            return out; // ignore blanks / keepalives
        };
        self.ensure_started(&root, &mut out);

        if let Some(u) = root.get("usageMetadata").filter(|v| !v.is_null()) {
            self.usage = Some(usage_from_value(Some(u)));
        }
        let Some(candidate) = root.get("candidates").and_then(|c| c.get(0)) else {
            return out;
        };
        if let Some(parts) = candidate
            .get("content")
            .and_then(|c| c.get("parts"))
            .and_then(Value::as_array)
        {
            for part in parts {
                if let Some(text) = part.get("text").and_then(Value::as_str) {
                    self.handle_text(text, &mut out);
                } else if let Some(fc) = part.get("functionCall") {
                    self.handle_function_call(fc, &mut out);
                }
            }
        }
        if let Some(fr) = candidate.get("finishReason").and_then(Value::as_str) {
            self.stop_reason = Some(map_finish_reason(fr));
        }
        out
    }

    fn finish(&mut self) -> Vec<StreamEvent> {
        // Gemini has no `[DONE]` — finish() emits the single terminal sequence.
        let mut out = Vec::new();
        if self.started && !self.done {
            self.done = true;
            if self.text_open {
                out.push(StreamEvent::ContentBlockStop {
                    index: self.text_index,
                });
                self.text_open = false;
            }
            out.push(StreamEvent::MessageDelta {
                delta: MessageDeltaPayload {
                    stop_reason: self.terminal_stop_reason(),
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
        let mut d = GeminiSseDecoder::new();
        let mut out = Vec::new();
        for f in frames {
            out.extend(d.push(f));
        }
        out.extend(d.finish());
        out
    }

    #[test]
    fn text_stream_terminates_via_finish() {
        let events = run(&[
            r#"{"candidates":[{"content":{"role":"model","parts":[{"text":"hel"}]}}]}"#,
            r#"{"candidates":[{"content":{"role":"model","parts":[{"text":"lo"}]}}],"usageMetadata":{"promptTokenCount":3,"candidatesTokenCount":1}}"#,
            r#"{"candidates":[{"content":{"role":"model","parts":[]},"finishReason":"STOP"}]}"#,
        ]);
        assert!(matches!(
            events.first(),
            Some(StreamEvent::MessageStart { .. })
        ));
        assert!(events.iter().any(|e| matches!(
            e, StreamEvent::ContentBlockDelta { delta: ContentDelta::TextDelta { text }, .. } if text == "hel"
        )));
        // exactly one terminal MessageStop + one MessageDelta (from finish, no [DONE])
        assert_eq!(
            events
                .iter()
                .filter(|e| matches!(e, StreamEvent::MessageStop))
                .count(),
            1
        );
        assert_eq!(
            events
                .iter()
                .filter(|e| matches!(e, StreamEvent::MessageDelta { .. }))
                .count(),
            1
        );
        assert!(matches!(events.last(), Some(StreamEvent::MessageStop)));
    }

    #[test]
    fn function_call_stream_emits_tool_use_block_and_tool_use_stop() {
        let events = run(&[
            r#"{"candidates":[{"content":{"role":"model","parts":[{"functionCall":{"name":"Bash","args":{"command":"ls"}}}]},"finishReason":"STOP"}]}"#,
        ]);
        assert!(events.iter().any(|e| matches!(
            e, StreamEvent::ContentBlockStart { content_block: ContentBlockApi::ToolUse { name, .. }, .. } if name == "Bash"
        )));
        // the whole args arrive as one InputJsonDelta
        let args: String = events
            .iter()
            .filter_map(|e| match e {
                StreamEvent::ContentBlockDelta {
                    delta: ContentDelta::InputJsonDelta { partial_json },
                    ..
                } => Some(partial_json.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&args).unwrap()["command"],
            "ls"
        );
        // functionCall present → terminal stop_reason is tool_use
        let sr = events
            .iter()
            .find_map(|e| match e {
                StreamEvent::MessageDelta { delta, .. } => Some(delta.stop_reason.clone()),
                _ => None,
            })
            .flatten();
        assert_eq!(sr.as_deref(), Some("tool_use"));
    }

    #[test]
    fn unparseable_lines_ignored() {
        let events = run(&["", "not json"]);
        // never started → no events
        assert!(events.is_empty());
    }
}
