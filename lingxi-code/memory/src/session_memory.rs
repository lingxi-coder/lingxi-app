//! Session-memory extraction (spec §6.5 `SessionMemoryExtractor`).
//!
//! Captures durable notes from an in-flight or finished session and writes them
//! to `<configHome>/agents/session-memory/<session_id>.md`. That file is NOT
//! injected through a second path: the next session re-loads it through the
//! normal memory-load → selector → prefetch pipeline and it surfaces via the
//! SURFACING-owned `relevant_memory_reminder_message`. So this module owns only
//! the WRITE + threshold-trigger side; it never adds a surfacing API of its own.
//!
//! 1:1 with the §6.5 sketch:
//!
//! ```text
//! pub struct SessionMemoryExtractor { config, last_extracted_message_id }
//! pub fn should_extract(&self, conversation) -> bool {
//!   let n = count_tool_calls_since(history, last_extracted_message_id);
//!   if !is_initialized() { n >= config.initialization_threshold }
//!   else                 { n >= config.update_threshold }
//! }
//! pub async fn extract(&mut self, conversation) -> Result<String, MemoryError>
//!   // Effect::ForkAgent — extract via forked agent
//! ```
//!
//! Gating is config-only: the sole gate is [`SessionMemoryConfig::enabled`]
//! (there is no `tengu_session_memory` flag in claude-code v2.1.181 — the stale
//! m3-02 plan invented one). `enabled == false` keeps the whole subsystem inert,
//! so the ~4000 locked fixtures stay byte-identical.

use crate::file::MemoryError;
use protocol::{ConversationMessage, MessageId};
use sidequery::{CacheSafeParams, ForkPurpose, ForkedAgentRequest, ForkedAgentRunner, QuerySource};
use std::path::{Path, PathBuf};

/// Configuration for the standalone session-memory extractor (spec §6.5).
///
/// `enabled` is the single master gate (no `tengu_session_memory` flag exists in
/// v2.1.181). The numeric `*_threshold` defaults are deliberately NOT pinned to
/// a literal here — they are unknown in the spec/binary/git history, so tests
/// assert behavior *relative to the configured field*, never a hard-coded
/// number. A composition root / settings loader supplies concrete values.
#[derive(Debug, Clone)]
pub struct SessionMemoryConfig {
    /// Master gate for the session-memory subsystem.
    pub enabled: bool,
    /// Tool-call count since the last extraction that triggers the FIRST
    /// extraction of a session (before any memory has been written).
    pub initialization_threshold: u32,
    /// Tool-call count since the last extraction that triggers a SUBSEQUENT
    /// (incremental) extraction once the session already has memory.
    pub update_threshold: u32,
    /// Model alias used for the cheap distillation fork. Haiku-class by default
    /// (matches `crate::selector::MemorySelector::new`).
    pub extraction_model: String,
}

impl Default for SessionMemoryConfig {
    fn default() -> Self {
        Self {
            // Inert by default — keeps the locked fixtures byte-identical until a
            // composition root opts in.
            enabled: false,
            // Thresholds are unknown upstream; tests assert against the field,
            // not these placeholders. They only matter once `enabled` is set.
            initialization_threshold: 0,
            update_threshold: 0,
            // Cheap standalone distillation fork — Haiku-class, matching the
            // memory selector's default model.
            extraction_model: "claude-haiku-4-5".to_string(),
        }
    }
}

/// Distils a session into reusable memory notes and writes them to the
/// per-session memory file (spec §6.5).
#[derive(Debug, Clone)]
pub struct SessionMemoryExtractor {
    config: SessionMemoryConfig,
    /// Id of the last message covered by the previous extraction; drives both
    /// the initialization-vs-update threshold choice and the "tool calls since"
    /// window. `None` until the first extraction completes.
    last_extracted_message_id: Option<MessageId>,
}

impl SessionMemoryExtractor {
    /// Build an extractor bound to `config`, with no prior extraction.
    #[must_use]
    pub fn new(config: SessionMemoryConfig) -> Self {
        Self {
            config,
            last_extracted_message_id: None,
        }
    }

    /// Borrow the active configuration.
    #[must_use]
    pub fn config(&self) -> &SessionMemoryConfig {
        &self.config
    }

    /// `true` once at least one extraction has completed (spec §6.5
    /// `is_initialized`). Drives the initialization-vs-update threshold choice.
    #[must_use]
    pub fn is_initialized(&self) -> bool {
        self.last_extracted_message_id.is_some()
    }

    /// Whether enough tool calls have accrued since the last extraction to
    /// trigger another one (spec §6.5 `should_extract`).
    ///
    /// Gated on [`SessionMemoryConfig::enabled`] first (a disabled extractor
    /// never fires). Then counts tool calls in `history` after
    /// `last_extracted_message_id` and compares against the
    /// initialization/update threshold depending on [`Self::is_initialized`].
    #[must_use]
    pub fn should_extract(&mut self, history: &[ConversationMessage]) -> bool {
        if !self.config.enabled {
            return false;
        }
        // A completed compaction can remove the exact watermark message. Treat
        // the current compacted tail as the new baseline instead of recounting
        // the entire retained window (which would repeatedly extract the same
        // tool calls). Subsequent messages are measured from this anchor.
        if self
            .last_extracted_message_id
            .as_ref()
            .is_some_and(|id| !history.iter().any(|message| &message.id() == id))
        {
            if let Some(last) = history.last() {
                self.last_extracted_message_id = Some(last.id());
            }
            return false;
        }
        // Thresholds are `u32`; clamp the count into `u32` for the compare (a
        // tool-call count beyond `u32::MAX` is not reachable in a real session).
        let n = u32::try_from(count_tool_calls_since(
            history,
            self.last_extracted_message_id.as_ref(),
        ))
        .unwrap_or(u32::MAX);
        if self.is_initialized() {
            n >= self.config.update_threshold
        } else {
            n >= self.config.initialization_threshold
        }
    }

    /// Run one forked extraction over `history`, write the distilled notes to
    /// `<config_home>/agents/session-memory/<session_id>.md`, advance the
    /// extraction watermark, and return the written content.
    ///
    /// 1:1 with §6.5 `extract` (`Effect::ForkAgent`): the distillation runs as a
    /// single-turn forked agent off the parent's cache-safe prefix so the prompt
    /// cache hits, then the result is persisted as the session-memory file the
    /// next session re-loads through the normal selector/prefetch/surfacing path
    /// (this module adds NO second injection).
    ///
    /// `config_home` is the resolved `$LINGXI_CONFIG_DIR ?? $HOME/.claude`
    /// directory (see [`session_memory_path`]); the caller passes it so this
    /// stays testable with a tempdir.
    ///
    /// # Errors
    ///
    /// Returns [`MemoryError::SelectorUnavailable`] if the forked extraction
    /// call fails, or [`MemoryError::Io`] if the file write fails.
    pub async fn extract(
        &mut self,
        runner: &ForkedAgentRunner,
        cache_safe_params: CacheSafeParams,
        session_id: &str,
        history: &[ConversationMessage],
        config_home: &Path,
    ) -> Result<String, MemoryError> {
        let request = ForkedAgentRequest {
            prompt_messages: vec![ConversationMessage::user(
                MessageId::new(),
                SESSION_MEMORY_EXTRACTION_PROMPT.to_string(),
            )],
            cache_safe_params,
            fork_label: "session-memory".to_string(),
            query_source: QuerySource::SessionMemoryExtraction,
            max_output_tokens: None,
        };

        let result = runner
            .run(request)
            .await
            .map_err(|e| MemoryError::SelectorUnavailable(e.to_string()))?;
        let content = result.final_text;

        let path = session_memory_path(config_home, session_id);
        write_session_memory(&path, &content)?;

        // Advance the watermark to the last message we covered so the next
        // `should_extract` measures tool calls *since this extraction*.
        self.last_extracted_message_id = history.last().map(ConversationMessage::id);

        Ok(content)
    }

    /// The `ForkPurpose` tag this extractor forks under (telemetry/log shape).
    #[must_use]
    pub fn fork_purpose() -> ForkPurpose {
        ForkPurpose::SessionMemoryExtraction
    }
}

/// Count assistant tool-call blocks in `history` that occur *after* the message
/// with id `since` (exclusive). When `since` is `None` (no prior extraction),
/// every tool call in `history` counts.
///
/// Counts individual `ToolUse` blocks (an assistant turn may request several),
/// matching the "tool calls since" intent of §6.5 — a multi-tool turn moves the
/// threshold by more than one.
#[must_use]
pub fn count_tool_calls_since(history: &[ConversationMessage], since: Option<&MessageId>) -> usize {
    // Skip everything up to and including the `since` message; count tool-use
    // blocks in the remainder.
    let tail: &[ConversationMessage] = match since {
        Some(id) => match history.iter().position(|m| &m.id() == id) {
            Some(idx) => &history[idx + 1..],
            // A missing watermark means the caller has crossed a compaction
            // boundary. Fail closed here; `should_extract` realigns its mutable
            // watermark to the compacted tail before a future count.
            None => &[],
        },
        None => history,
    };
    tail.iter().map(|m| m.tool_calls().len()).sum()
}

/// Resolve the on-disk path for a session's memory file:
/// `<config_home>/agents/session-memory/<session_id>.md`.
///
/// The `agents/session-memory/*.md` layout matches the existing
/// `detect_session_file_type` reader (`tools/file/src/read.rs`), which classifies
/// files under `<configHome>/.../session-memory/*.md` as `"session_memory"`.
#[must_use]
pub fn session_memory_path(config_home: &Path, session_id: &str) -> PathBuf {
    config_home
        .join("agents")
        .join("session-memory")
        .join(format!("{session_id}.md"))
}

/// Write `content` to `path`, creating parent directories as needed.
///
/// # Errors
///
/// Returns [`MemoryError::Io`] if directory creation or the write fails.
fn write_session_memory(path: &Path, content: &str) -> Result<(), MemoryError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| MemoryError::Io(e.to_string()))?;
    }
    std::fs::write(path, content).map_err(|e| MemoryError::Io(e.to_string()))
}

/// Resolve the config-home directory used for session-memory writes:
/// `$LINGXI_CONFIG_DIR` when set (claude-code `??`: an empty value is honored
/// verbatim → cwd-relative) else `$HOME/.claude` else `$USERPROFILE/.claude`
/// else a bare `.claude`.
///
/// Delegates the `$LINGXI_CONFIG_DIR`-vs-home resolution to the canonical
/// [`crate::lingxi_md::user_config_dir`]; this fn only resolves the fallback home
/// from `$HOME`/`$USERPROFILE` (the memory crate has no `dirs` dependency).
/// `Path::new("").join(".lingxi") == ".lingxi"`, so the no-home case stays the
/// bare `.claude` form — byte-identical to the prior inline implementation.
#[must_use]
pub fn config_home_dir() -> PathBuf {
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map_or_else(|| PathBuf::from(""), PathBuf::from);
    crate::lingxi_md::user_config_dir(&home)
}

/// Distillation instruction handed to the forked extractor. Kept terse — the
/// fork replays the parent's full cache-safe prefix (the conversation), so this
/// only needs to steer the distillation, not restate the history.
const SESSION_MEMORY_EXTRACTION_PROMPT: &str = "Distil durable, reusable notes from this session that a future session should remember — stable facts about the project, conventions, tooling, and decisions. Omit transient state and anything already obvious from the code. Respond with the notes only, as concise markdown.";

#[cfg(test)]
mod tests {
    use super::*;
    use protocol::{ContentBlock, MessageId, ToolUseId};
    use std::sync::Arc;

    fn enabled_config(init: u32, update: u32) -> SessionMemoryConfig {
        SessionMemoryConfig {
            enabled: true,
            initialization_threshold: init,
            update_threshold: update,
            ..SessionMemoryConfig::default()
        }
    }

    fn user(text: &str) -> ConversationMessage {
        ConversationMessage::user(MessageId::new(), text.to_string())
    }

    /// An assistant message requesting `n` tool calls.
    fn assistant_tools(n: usize) -> ConversationMessage {
        let content = (0..n)
            .map(|i| ContentBlock::ToolUse {
                id: ToolUseId::new(),
                provider_id: None,
                name: format!("Tool{i}"),
                input: serde_json::json!({}),
            })
            .collect();
        ConversationMessage::Assistant {
            id: MessageId::new(),
            content,
            stop_reason: Some("tool_use".into()),
        }
    }

    #[test]
    fn default_config_is_inert() {
        let c = SessionMemoryConfig::default();
        assert!(
            !c.enabled,
            "must be off by default (locked fixtures stay byte-identical)"
        );
        assert_eq!(c.extraction_model, "claude-haiku-4-5");
    }

    #[test]
    fn disabled_extractor_never_fires() {
        // Even with a flood of tool calls, a disabled config never extracts.
        let mut ex = SessionMemoryExtractor::new(SessionMemoryConfig::default());
        let history = vec![assistant_tools(100)];
        assert!(!ex.should_extract(&history));
    }

    #[test]
    fn should_extract_uses_initialization_threshold_before_first_extraction() {
        let mut ex = SessionMemoryExtractor::new(enabled_config(3, 1));
        assert!(!ex.is_initialized());
        // 2 tool calls < init threshold 3 => no.
        assert!(!ex.should_extract(&[assistant_tools(2)]));
        // 3 tool calls >= init threshold 3 => yes.
        assert!(ex.should_extract(&[assistant_tools(3)]));
    }

    #[test]
    fn should_extract_uses_update_threshold_after_first_extraction() {
        let mut ex = SessionMemoryExtractor::new(enabled_config(10, 2));
        // Simulate a completed first extraction by setting the watermark.
        let watermark = user("watermark");
        ex.last_extracted_message_id = Some(watermark.id());
        assert!(ex.is_initialized());

        // History: watermark, then 1 tool call (since the watermark) => below
        // the update threshold of 2.
        let one = assistant_tools(1);
        let history = vec![watermark.clone(), one];
        assert!(!ex.should_extract(&history));

        // Add another tool call after the watermark => 2 >= update threshold.
        let history2 = vec![watermark, assistant_tools(1), assistant_tools(1)];
        assert!(ex.should_extract(&history2));
    }

    #[test]
    fn count_tool_calls_since_counts_after_watermark_only() {
        let before = assistant_tools(5);
        let mark = user("mark");
        let after = assistant_tools(2);
        let history = vec![before, mark.clone(), after];
        // Everything (None watermark) counts both assistant turns.
        assert_eq!(count_tool_calls_since(&history, None), 7);
        // Only what comes after `mark` counts.
        assert_eq!(count_tool_calls_since(&history, Some(&mark.id())), 2);
    }

    #[test]
    fn missing_watermark_realigns_without_recounting_compacted_history() {
        let mut ex = SessionMemoryExtractor::new(enabled_config(10, 1));
        ex.last_extracted_message_id = Some(MessageId::new());
        let history = vec![assistant_tools(3)];
        let stale = MessageId::new();
        assert_eq!(count_tool_calls_since(&history, Some(&stale)), 0);
        assert!(!ex.should_extract(&history));
        assert_eq!(
            ex.last_extracted_message_id,
            history.last().map(ConversationMessage::id)
        );
        let mut next = history;
        next.push(assistant_tools(1));
        assert!(ex.should_extract(&next));
    }

    #[test]
    fn session_memory_path_uses_agents_session_memory_layout() {
        let p = session_memory_path(Path::new("/cfg"), "abc123");
        assert_eq!(p, PathBuf::from("/cfg/agents/session-memory/abc123.md"));
    }

    #[tokio::test]
    async fn extract_writes_file_and_advances_watermark() {
        use async_trait::async_trait;
        use sidequery::{SideQueryClient, SideQueryError, SideQueryRequest, SideQueryResponse};
        use tool_api::context::ToolUseOptions;

        // A side-query client that returns the distilled notes verbatim.
        struct Client;
        #[async_trait]
        impl SideQueryClient for Client {
            async fn query(
                &self,
                _req: SideQueryRequest,
            ) -> Result<SideQueryResponse, SideQueryError> {
                Ok(SideQueryResponse {
                    text: Some("DISTILLED NOTES".into()),
                    structured: None,
                    tool_calls: Vec::new(),
                    usage: cost::Usage::default(),
                    stop_reason: Some("end_turn".into()),
                })
            }
        }

        let runner = ForkedAgentRunner::new()
            .with_side_query_client(Arc::new(Client), "claude-haiku-4-5".into());

        let cache_safe = CacheSafeParams {
            system_prompt: Arc::from("SYS"),
            user_context: std::collections::HashMap::new(),
            system_context: std::collections::HashMap::new(),
            tool_use_options: ToolUseOptions {
                debug: false,
                verbose: false,
                main_loop_model: "claude-haiku-4-5".into(),
                model_profile: None,
                max_budget_nano_usd: None,
                mcp_clients: vec![],
                is_non_interactive_session: false,
                custom_system_prompt: None,
                append_system_prompt: None,
            },
            fork_context_messages: vec![],
            transcript_path: None,
            generation: 1,
        };

        let dir = tempfile::tempdir().unwrap();
        let mut ex = SessionMemoryExtractor::new(enabled_config(1, 1));
        let history = vec![assistant_tools(1), user("last")];
        let last_id = history.last().unwrap().id();

        let written = ex
            .extract(&runner, cache_safe, "sess-xyz", &history, dir.path())
            .await
            .expect("extract succeeds");

        assert_eq!(written, "DISTILLED NOTES");
        // File landed under agents/session-memory/<id>.md.
        let path = session_memory_path(dir.path(), "sess-xyz");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "DISTILLED NOTES");
        // Watermark advanced to the last message so the next window starts fresh.
        assert_eq!(ex.last_extracted_message_id, Some(last_id));
        assert!(ex.is_initialized());
    }

    #[test]
    fn config_home_dir_honors_claude_config_dir_then_home() {
        // We avoid mutating real process env across threads beyond a scoped check.
        let prev = std::env::var_os("LINGXI_CONFIG_DIR");
        std::env::set_var("LINGXI_CONFIG_DIR", "/explicit/cfg");
        assert_eq!(config_home_dir(), PathBuf::from("/explicit/cfg"));
        // A set-but-EMPTY string is honored verbatim (claude-code `??`).
        std::env::set_var("LINGXI_CONFIG_DIR", "");
        assert_eq!(config_home_dir(), PathBuf::from(""));
        match prev {
            Some(v) => std::env::set_var("LINGXI_CONFIG_DIR", v),
            None => std::env::remove_var("LINGXI_CONFIG_DIR"),
        }
    }
}
