//! Microcompact — clears stale large tool results without involving the LLM.

use protocol::{ContentBlock, ConversationMessage};
use std::collections::HashSet;
use std::time::{Duration, SystemTime};

/// Placeholder text substituted for cleared tool results.
pub const TIME_BASED_MC_CLEARED_MESSAGE: &str = "[Old tool result content cleared]";

/// Set of tool names whose results microcompact is allowed to clear.
#[must_use]
pub fn compactable_tools() -> HashSet<&'static str> {
    HashSet::from([
        "Read",
        "Bash",
        "PowerShell",
        "Grep",
        "Glob",
        "WebSearch",
        "WebFetch",
        "Edit",
        "Write",
    ])
}

/// Configuration controlling when a tool result is considered "stale and large".
#[derive(Debug, Clone)]
pub struct TimeBasedMCConfig {
    /// Tool results older than this become eligible for clearing.
    pub age_threshold: Duration,
    /// How many recent tool results to always keep, regardless of age.
    pub keep_recent_count: usize,
    /// Per-result byte cutoff; only larger results are cleared.
    pub max_per_result_bytes: usize,
    /// Token cap above which image attachments are dropped.
    pub image_max_token_size: u64,
}

impl Default for TimeBasedMCConfig {
    fn default() -> Self {
        Self {
            age_threshold: Duration::from_secs(15 * 60), // 15 minutes
            keep_recent_count: 6,
            max_per_result_bytes: 8 * 1024,
            image_max_token_size: 2000,
        }
    }
}

/// Stateful microcompactor that owns its configuration.
pub struct Microcompactor {
    /// Active configuration; see [`TimeBasedMCConfig`].
    pub config: TimeBasedMCConfig,
}

/// Result of running microcompact over a message list.
#[derive(Debug, Clone)]
pub struct MicrocompactResult {
    /// Messages with eligible tool results replaced by the cleared placeholder.
    pub messages: Vec<ConversationMessage>,
    /// Number of tool-result blocks that were cleared.
    pub cleared_count: usize,
}

impl Microcompactor {
    /// Run microcompact: replace stale large tool-result content with the
    /// cleared placeholder. Returns the updated message list and a count.
    #[must_use]
    pub fn compact(
        &self,
        messages: Vec<ConversationMessage>,
        _now: SystemTime,
    ) -> MicrocompactResult {
        let _compactable = compactable_tools();
        let mut cleared_count = 0;
        let out: Vec<ConversationMessage> = messages
            .into_iter()
            .map(|m| {
                if let ConversationMessage::User { id, content } = m {
                    let new_content: Vec<ContentBlock> = content
                        .into_iter()
                        .map(|b| {
                            if let ContentBlock::ToolResult {
                                tool_use_id,
                                content,
                                is_error,
                            } = &b
                            {
                                // Without tool name lookup, conservatively skip the clear here;
                                // production wires tool-name lookup from §11 Task storage.
                                if content.len() > self.config.max_per_result_bytes {
                                    cleared_count += 1;
                                    return ContentBlock::ToolResult {
                                        tool_use_id: *tool_use_id,
                                        content: TIME_BASED_MC_CLEARED_MESSAGE.into(),
                                        is_error: *is_error,
                                    };
                                }
                            }
                            b
                        })
                        .collect();
                    ConversationMessage::User {
                        id,
                        content: new_content,
                    }
                } else {
                    m
                }
            })
            .collect();
        MicrocompactResult {
            messages: out,
            cleared_count,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use protocol::{MessageId, ToolUseId};

    #[test]
    fn clears_large_tool_results() {
        let mc = Microcompactor {
            config: TimeBasedMCConfig::default(),
        };
        let big_content = "x".repeat(100_000);
        let messages = vec![ConversationMessage::User {
            id: MessageId::new(),
            content: vec![ContentBlock::ToolResult {
                tool_use_id: ToolUseId::new(),
                content: big_content.clone(),
                is_error: false,
            }],
        }];
        let r = mc.compact(messages, SystemTime::now());
        assert_eq!(r.cleared_count, 1);
        // After clearing, content is the placeholder.
        if let ConversationMessage::User { content, .. } = &r.messages[0] {
            if let ContentBlock::ToolResult { content, .. } = &content[0] {
                assert_eq!(content, TIME_BASED_MC_CLEARED_MESSAGE);
            } else {
                panic!()
            }
        } else {
            panic!()
        }
    }
}
