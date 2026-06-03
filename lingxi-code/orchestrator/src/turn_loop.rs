//! Inner turn-by-turn loop helpers. Private to `ConversationOrchestrator`.

use crate::conversation::ConversationOrchestrator;
use crate::error::OrchestratorError;
use crate::test_support::PermissionDecision;
use api_client::types::ContentBlockApi;
use hooks::events::HookEvent;
use hooks::registry::HookContext;
use hooks::response::HookDecision;
use protocol::{ContentBlock, ConversationMessage, MessageId, ToolUseId};
use std::path::{Component, Path, PathBuf};
use telemetry::tengu::orchestrator as orch_events;
use tool_api::context::{ToolUseContext, ToolUseOptions};

/// Tools whose successful execution records `file_path` (or `notebook_path`)
/// into the orchestrator's read-file-state cache. Mirrors the TS sites that
/// call `readFileState.set(expandPath(file_path), …)` (`FileReadTool` +
/// `FileEditTool`/`FileWriteTool`/`MultiEditTool`/`NotebookEditTool`).
/// `/files` then renders this set (TS `cacheKeys(context.readFileState)`).
const READ_FILE_STATE_TOOLS: &[&str] =
    &["Read", "Edit", "Write", "MultiEdit", "NotebookEdit"];

/// Lexically expand a tool's `file_path` argument to an absolute, normalized
/// path — the cache key for [`ConversationOrchestrator::files_in_context`].
///
/// 1:1 with the observable behavior of TS `expandPath(path, baseDir)`
/// (`src/utils/path.ts`): trim; bare `~` / `~/…` expand against the home
/// directory; absolute paths are kept; relative paths resolve against `cwd`;
/// the result is then collapsed lexically (`.` dropped, `..` popped).
///
/// FORCED divergence from `expandPath` (documented, not a parity gap):
/// - Windows POSIX-path conversion (`/c/Users/…`) is skipped — the port's
///   parity target is the macOS/Linux path shape, and the cache key only
///   feeds `relative(cwd, key)` rendering which is already platform-native.
/// - Unicode NFC normalization is a no-op for the ASCII paths exercised
///   here and `OsStr` carries no portable NFC primitive, so it is omitted.
/// - This is LEXICAL only (mirrors `expandPath`, NOT `realpath`): it never
///   touches the disk, so symlinks are preserved and a non-existent path
///   still resolves to the joined string — matching the TS keys and the
///   `relative()` output `/files` renders.
fn absolutize(cwd: &Path, raw: &str) -> PathBuf {
    let trimmed = raw.trim();
    let expanded: PathBuf = if trimmed == "~" {
        dirs::home_dir().unwrap_or_else(|| PathBuf::from(trimmed))
    } else if let Some(rest) = trimmed.strip_prefix("~/") {
        dirs::home_dir().map_or_else(|| PathBuf::from(trimmed), |h| h.join(rest))
    } else {
        let p = Path::new(trimmed);
        if p.is_absolute() {
            p.to_path_buf()
        } else {
            cwd.join(p)
        }
    };
    normalize_lexically(&expanded)
}

/// Collapse `.` and `..` segments without touching the filesystem, mirroring
/// Node's `path.normalize`/`resolve` (used by `expandPath`). A `..` pops the
/// previous normal component; a leading `..` with nothing to pop is kept.
fn normalize_lexically(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for comp in path.components() {
        match comp {
            Component::CurDir => {}
            Component::ParentDir => {
                // Pop the last NORMAL component; otherwise keep the `..`
                // (e.g. above the root prefix or a leading relative `..`).
                if matches!(out.components().next_back(), Some(Component::Normal(_))) {
                    out.pop();
                } else {
                    out.push("..");
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// Record a successful `Read`/`Edit`/`Write`/… into the read-file-state cache.
///
/// Best-effort and order-preserving: pulls `file_path` (or `notebook_path`
/// for `NotebookEdit`) from the post-hook effective input, absolutizes it
/// against `orch.cwd`, and inserts it into `orch.read_file_state` keeping the
/// FIRST insertion (a re-read is a no-op; the 2-file case `a,b,a → [a, b]`
/// matches TS, but TS's MRU-promoting LRU diverges at ≥3 files — read a,b,c,a
/// → TS `[a, c, b]` vs this `[a, b, c]`). Never fails the tool: an absent /
/// non-string path or an unknown tool is silently skipped.
///
/// Two TS write-sites are intentionally NOT mirrored: TS `NotebookEditTool`
/// resolves `notebook_path` WITHOUT `expandPath` (no `~`/trim), whereas this
/// routes it through the shared [`absolutize`] (harmless unless a notebook path
/// literally starts with `~` or has surrounding whitespace); and `BashTool`'s
/// `readFileState.set` for files a bash command writes is out of scope (the
/// bash file-write interception is itself unported).
async fn record_read_file_state(
    orch: &ConversationOrchestrator,
    name: &str,
    effective_input: &serde_json::Value,
) {
    if !READ_FILE_STATE_TOOLS.contains(&name) {
        return;
    }
    let key = if name == "NotebookEdit" {
        "notebook_path"
    } else {
        "file_path"
    };
    let Some(raw) = effective_input.get(key).and_then(serde_json::Value::as_str) else {
        return;
    };
    let absolute = absolutize(&orch.cwd, raw);
    let mut cache = orch.read_file_state.lock().await;
    if !cache.contains(&absolute) {
        cache.push(absolute);
    }
}

/// What one turn step decided.
pub(crate) enum TurnStepOutcome {
    /// Continue the loop (e.g. model returned `tool_use`).
    Continue,
    /// Loop should terminate — model returned `end_turn`.
    Ended {
        final_message_id: MessageId,
        stop_reason: String,
    },
}

/// Execute one `messages_create_non_stream` round-trip + tool dispatches.
///
/// `system` is the assembled system prompt for the conversation (built
/// once by `ConversationOrchestrator::try_run_turn`). It is passed
/// through to every API round-trip in the conversation, NOT re-built
/// per turn-step — the prompt is stable across the conversation lifetime
/// (see M5-03 plan "Out of scope" note: SSE M5-04 will not re-assemble
/// per turn-step either).
pub(crate) async fn execute_one_turn(
    orch: &ConversationOrchestrator,
    system: Option<&str>,
) -> Result<TurnStepOutcome, OrchestratorError> {
    // Snapshot the current session history for the API call.
    let (history_snapshot, model) = {
        let s = orch.session.lock().await;
        (s.history.clone(), s.model.clone())
    };

    // 1. Call the API.
    let response = orch
        .api
        .messages_create(&model, system, history_snapshot)
        .await?;

    // 1.5 M6-06: record this response's usage into the wired CostTracker (if any).
    // We pass `Duration::ZERO` (the api-client adapter does not currently
    // surface per-call wall-clock duration) and `retries = 0` (retries are
    // swallowed internally). Both inaccuracies are documented in v0.7.0
    // release notes; M7 wires through real timing.
    if let Some(tracker) = orch.cost_tracker.as_ref() {
        let usage = crate::cost_wiring::usage_api_to_cost_usage(&response.usage);
        let cache_read = response.usage.cache_read_input_tokens;
        let cache_create = response.usage.cache_creation_input_tokens;
        let model_ref = crate::cost_wiring::model_ref_from_string(&model);
        let _cost_for_this_call = tracker
            .record_api_response_v2(
                model_ref,
                usage,
                std::time::Duration::ZERO,
                0, // retries — not exposed from api-client adapter today
                cache_read,
                cache_create,
                false, // is_batch_request — M6 always false
                None,  // bus — orchestrator does not yet carry an AnalyticsBus (M7 work)
            )
            .await;
        orch.api_calls_recorded
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }

    // 2. Translate `MessageResponse.content` -> `ContentBlock` history entry.
    let assistant_blocks = translate_response_blocks(&response.content);

    // 3. Append the assistant message to the session. We need the
    //    `final_message_id` to return to the caller.
    let assistant_id = MessageId::new();
    let assistant_msg = ConversationMessage::Assistant {
        id: assistant_id,
        content: assistant_blocks.clone(),
        stop_reason: response.stop_reason.clone(),
    };
    {
        let mut s = orch.session.lock().await;
        s.history.push(assistant_msg.clone());
    }
    // M5-07 T13: mirror the in-memory append to the optional JSONL writer.
    // Best-effort — write failures never fail the turn.
    orch.persist_message_to_jsonl(&assistant_msg).await;

    // 4. Emit each Text block to the output stream (whole-body in M5-02;
    //    M5-04 will switch to per-delta).
    for blk in &assistant_blocks {
        if let ContentBlock::Text { text } = blk {
            orch.output.emit_text(text).await;
        }
    }

    // 5. If there are tool_use blocks, dispatch them and feed results back.
    let tool_uses: Vec<(ToolUseId, String, serde_json::Value)> = assistant_blocks
        .iter()
        .filter_map(|b| match b {
            ContentBlock::ToolUse { id, name, input } => Some((*id, name.clone(), input.clone())),
            _ => None,
        })
        .collect();

    if !tool_uses.is_empty() {
        let tool_results = dispatch_tool_uses(orch, &tool_uses).await?;
        // Append a fresh user message carrying the tool results.
        let user_id = MessageId::new();
        let tool_results_msg = ConversationMessage::User {
            id: user_id,
            content: tool_results,
        };
        {
            let mut s = orch.session.lock().await;
            s.history.push(tool_results_msg.clone());
        }
        // M5-07 T13: persist the tool_result user message. Best-effort.
        orch.persist_message_to_jsonl(&tool_results_msg).await;
    }

    // 6. Decide loop disposition.
    match response.stop_reason.as_deref() {
        Some("end_turn") => Ok(TurnStepOutcome::Ended {
            final_message_id: assistant_id,
            stop_reason: "end_turn".to_string(),
        }),
        _ => Ok(TurnStepOutcome::Continue),
    }
}

/// Translate api-client content blocks into protocol content blocks.
/// Server-side variants (`ServerToolUse`, `ConnectorText`, `AdvisorToolResult`)
/// are dropped in M5-02. M5-04 may revisit Thinking.
fn translate_response_blocks(content: &[ContentBlockApi]) -> Vec<ContentBlock> {
    content
        .iter()
        .filter_map(|b| match b {
            ContentBlockApi::Text { text } => Some(ContentBlock::Text { text: text.clone() }),
            ContentBlockApi::ToolUse { id, name, input } => Some(ContentBlock::ToolUse {
                id: *id,
                name: name.clone(),
                input: input.clone(),
            }),
            ContentBlockApi::Thinking {
                thinking,
                signature,
            } => Some(ContentBlock::Thinking {
                thinking: thinking.clone(),
                signature: signature.clone(),
            }),
            // Server-side variants are skipped in M5-02; M5-04 may revisit.
            ContentBlockApi::ServerToolUse { .. }
            | ContentBlockApi::ConnectorText { .. }
            | ContentBlockApi::AdvisorToolResult { .. } => None,
        })
        .collect()
}

/// Dispatch each `tool_use` block through hooks -> permission -> registry ->
/// hooks. Returns a list of `ContentBlock::ToolResult` blocks for the
/// next user message.
#[allow(clippy::too_many_lines)]
pub(crate) async fn dispatch_tool_uses(
    orch: &ConversationOrchestrator,
    tool_uses: &[(ToolUseId, String, serde_json::Value)],
) -> Result<Vec<ContentBlock>, OrchestratorError> {
    let mut results = Vec::with_capacity(tool_uses.len());
    for (tool_use_id, name, input) in tool_uses {
        orch.output.emit_tool_call(tool_use_id, name, input).await;

        // M5-06 Task 14: PreToolUse hook chain. Build the event + context,
        // call the executor, and either Block (turn the response into an
        // error ToolResult), apply modified_input, or continue.
        let session_id = { orch.session.lock().await.session_id };
        let hook_ctx = HookContext {
            session_id,
            cwd: orch.cwd.clone(),
            ..Default::default()
        };
        let pre_event = HookEvent::PreToolUse {
            tool_name: name.clone(),
            tool_input: input.clone(),
            tool_use_id: *tool_use_id,
        };
        let pre_started = std::time::Instant::now();
        tracing::info!(
            event = orch_events::HOOK_PRE_STARTED,
            tool_name = %name,
        );
        let pre_agg = orch.hooks.execute(pre_event, hook_ctx.clone()).await;
        // hook duration bounded by tokio timeout — u128 ms cannot exceed u64::MAX
        #[allow(clippy::cast_possible_truncation)]
        let pre_dur_ms = pre_started.elapsed().as_millis() as u64;

        if matches!(pre_agg.decision, Some(HookDecision::Block)) {
            let reason = pre_agg
                .reason
                .clone()
                .unwrap_or_else(|| "blocked by hook".into());
            tracing::info!(
                event = orch_events::HOOK_PRE_COMPLETED,
                tool_name = %name,
                decision = "block",
                duration_ms = pre_dur_ms,
            );
            let result_block = ContentBlock::ToolResult {
                tool_use_id: *tool_use_id,
                content: format!("Hook blocked: {reason}"),
                is_error: true,
            };
            orch.output
                .emit_tool_result(
                    tool_use_id,
                    name,
                    &serde_json::json!({ "error": format!("Hook blocked: {reason}") }),
                )
                .await;
            results.push(result_block);
            continue;
        }

        // Apply modified_input if any hook mutated the tool input.
        let effective_input = pre_agg
            .modified_input
            .clone()
            .unwrap_or_else(|| input.clone());
        tracing::info!(
            event = orch_events::HOOK_PRE_COMPLETED,
            tool_name = %name,
            decision = match pre_agg.decision {
                Some(HookDecision::Allow) => "allow",
                Some(HookDecision::Approve) => "approve",
                Some(HookDecision::Continue) => "continue",
                Some(HookDecision::Block) => "block",
                None => "none",
            },
            duration_ms = pre_dur_ms,
        );

        // Permission gate. Use the post-hook effective_input so a Pre
        // hook can rewrite a tool argument before the permission check
        // sees it.
        match orch.perms.check(name, &effective_input).await {
            PermissionDecision::Allow => {}
            PermissionDecision::Deny { reason } => {
                let result_block = ContentBlock::ToolResult {
                    tool_use_id: *tool_use_id,
                    content: format!("Permission denied: {reason}"),
                    is_error: true,
                };
                orch.output
                    .emit_tool_result(
                        tool_use_id,
                        name,
                        &serde_json::json!({ "error": format!("Permission denied: {reason}") }),
                    )
                    .await;
                results.push(result_block);
                continue;
            }
        }

        // Dispatch through ToolRegistry.
        let Some(tool_handle) = orch.tools.find_by_name(name) else {
            let result_block = ContentBlock::ToolResult {
                tool_use_id: *tool_use_id,
                content: format!("Error: tool not found: {name}"),
                is_error: true,
            };
            orch.output
                .emit_tool_result(
                    tool_use_id,
                    name,
                    &serde_json::json!({ "error": format!("tool not found: {name}") }),
                )
                .await;
            results.push(result_block);
            continue;
        };

        // Synthesize a minimal ToolUseContext.
        let messages = {
            let s = orch.session.lock().await;
            s.history.clone()
        };
        let ctx = ToolUseContext {
            options: ToolUseOptions {
                debug: false,
                verbose: false,
                main_loop_model: orch.config.model.clone(),
                max_budget_nano_usd: None,
                mcp_clients: Vec::new(),
                is_non_interactive_session: true,
                custom_system_prompt: orch.config.system_prompt_override.clone(),
                append_system_prompt: None,
            },
            messages,
            tool_use_id: Some(*tool_use_id),
            agent_id: None,
            content_replacement_state: None,
            session: Some(orch.session.clone()),
            subagent_registry: Some(orch.tools.clone()),
        };

        // One-shot progress channel — receiver dropped immediately.
        let (progress_tx, _progress_rx) =
            tokio::sync::mpsc::channel::<tool_api::progress::ToolProgress>(8);

        let tool_outcome = tool_handle
            .call(effective_input.clone(), ctx, progress_tx)
            .await;

        let (content, is_error, emit_payload) = match tool_outcome {
            Ok(result) => {
                let text = serde_json::to_string(&result.data)
                    .unwrap_or_else(|_| "<unserializable>".into());
                (text, false, result.data)
            }
            Err(err) => {
                let text = format!("Error: {err}");
                (text, true, serde_json::json!({ "error": format!("{err}") }))
            }
        };

        orch.output
            .emit_tool_result(tool_use_id, name, &emit_payload)
            .await;

        // Record the file into the read-file-state cache backing `/files`
        // (TS `readFileState.set(expandPath(file_path), …)` in FileReadTool /
        // FileEditTool / FileWriteTool / MultiEditTool / NotebookEditTool).
        // Only on success — an errored tool never populates the cache.
        if !is_error {
            record_read_file_state(orch, name, &effective_input).await;
        }

        // M5-06 Task 14: PostToolUse hook chain. Best-effort — a Post
        // hook's system_messages are appended to the result text, but
        // failures do NOT mutate `content` or `is_error`.
        let post_event = HookEvent::PostToolUse {
            tool_name: name.clone(),
            tool_input: effective_input.clone(),
            tool_output: emit_payload.clone(),
            tool_use_id: *tool_use_id,
        };
        let post_started = std::time::Instant::now();
        tracing::info!(
            event = orch_events::HOOK_POST_STARTED,
            tool_name = %name,
        );
        let post_agg = orch.hooks.execute(post_event, hook_ctx).await;
        // hook duration bounded by tokio timeout — u128 ms cannot exceed u64::MAX
        #[allow(clippy::cast_possible_truncation)]
        let post_dur_ms = post_started.elapsed().as_millis() as u64;

        let mutated = !post_agg.system_messages.is_empty();
        let final_content = if mutated {
            let mut out = content.clone();
            for msg in &post_agg.system_messages {
                out.push('\n');
                out.push_str(msg);
            }
            out
        } else {
            content
        };

        tracing::info!(
            event = orch_events::HOOK_POST_COMPLETED,
            tool_name = %name,
            duration_ms = post_dur_ms,
            mutated_response = mutated,
        );

        results.push(ContentBlock::ToolResult {
            tool_use_id: *tool_use_id,
            content: final_content,
            is_error,
        });
    }
    Ok(results)
}

#[cfg(test)]
mod read_file_state_tests {
    use super::{absolutize, dispatch_tool_uses};
    use crate::conversation::ConversationOrchestrator;
    use crate::test_support::{
        MockApiClient, MockOutputStream, NoOpPermissionGate, StaticMemoryProvider,
    };
    use crate::OrchestratorConfig;
    use async_trait::async_trait;
    use protocol::ToolUseId;
    use serde_json::json;
    use std::path::PathBuf;
    use std::sync::Arc;
    use tool_api::context::ToolUseContext;
    use tool_api::progress::ToolProgressSender;
    use tool_api::registry::ToolRegistry;
    use tool_api::tool_trait::{
        DescriptionOptions, PromptOptions, Tool, ToolCallResult, ToolError, ToolStaticContext,
        ValidationError,
    };
    use traits::OrchestratorHandle;

    /// Minimal Read/Edit/Write-shaped stub. Resolves `file_path` against
    /// `cwd` (mirroring how the real `FileReadTool` resolves against
    /// `getCwd()`) and reads it, so a missing file yields `is_error = true`
    /// exactly like the real tool. `name` is configurable so one stub can
    /// stand in for Read/Edit/Write.
    struct StubFileTool {
        name: &'static str,
        cwd: PathBuf,
    }

    #[async_trait]
    impl Tool for StubFileTool {
        fn name(&self) -> &str {
            self.name
        }
        fn input_schema(&self) -> &serde_json::Value {
            static SCHEMA: once_cell::sync::Lazy<serde_json::Value> =
                once_cell::sync::Lazy::new(|| {
                    json!({
                        "type": "object",
                        "properties": { "file_path": { "type": "string" } },
                        "required": ["file_path"],
                    })
                });
            &SCHEMA
        }
        fn is_enabled(&self, _ctx: &ToolStaticContext) -> bool {
            true
        }
        fn max_result_size_chars(&self) -> usize {
            1024 * 1024
        }
        fn is_concurrency_safe(&self, _input: &serde_json::Value) -> bool {
            true
        }
        fn is_read_only(&self, _input: &serde_json::Value) -> bool {
            true
        }
        async fn validate_input(
            &self,
            _input: &serde_json::Value,
            _ctx: &ToolUseContext,
        ) -> Result<(), ValidationError> {
            Ok(())
        }
        async fn check_permissions(
            &self,
            _input: &serde_json::Value,
            _ctx: &ToolUseContext,
        ) -> permission::PermissionResult {
            permission::PermissionResult::Allow {
                reason: permission::PermissionDecisionReason::Other {
                    reason: "test".into(),
                },
                updated_input: None,
                update_destination: None,
                metadata: permission::result::PermissionMetadata::default(),
            }
        }
        async fn description(
            &self,
            _input: &serde_json::Value,
            _opts: &DescriptionOptions,
        ) -> String {
            "stub file tool".into()
        }
        async fn prompt(&self, _opts: &PromptOptions) -> String {
            String::new()
        }
        async fn call(
            &self,
            input: serde_json::Value,
            _ctx: ToolUseContext,
            _tx: ToolProgressSender,
        ) -> Result<ToolCallResult, ToolError> {
            let path = input
                .get("file_path")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::InvalidInput("file_path required".into()))?;
            // Resolve against the tool's cwd (mirrors getCwd()), then touch
            // disk so a missing file is a genuine error (mirrors Read).
            let resolved = self.cwd.join(path);
            let content = tokio::fs::read_to_string(&resolved)
                .await
                .map_err(|e| ToolError::Io(format!("read {}: {e}", resolved.display())))?;
            Ok(ToolCallResult {
                data: json!({ "content": content }),
                new_messages: vec![],
                context_modifier: None,
                mcp_meta: None,
            })
        }
    }

    /// Build an orchestrator whose registry contains the given stub tools,
    /// rooted at `cwd`. The API queue is empty (these tests drive
    /// `dispatch_tool_uses` directly, never `run_turn`).
    fn orch_with_tools(cwd: PathBuf, tools: Vec<Arc<dyn Tool>>) -> ConversationOrchestrator {
        let mut registry = ToolRegistry::new();
        for t in tools {
            registry.register_builtin(t);
        }
        ConversationOrchestrator::new(
            OrchestratorConfig::default(),
            Arc::new(MockApiClient::new(vec![])),
            Arc::new(registry),
            crate::test_support::noop_hook_executor(),
            Arc::new(NoOpPermissionGate),
            Arc::new(MockOutputStream::new()),
            Arc::new(StaticMemoryProvider::empty()),
            cwd,
        )
    }

    /// Drive one `(name, input)` `tool_use` through the dispatch chokepoint.
    async fn dispatch_one(orch: &ConversationOrchestrator, name: &str, input: serde_json::Value) {
        let uses = vec![(ToolUseId::new(), name.to_string(), input)];
        dispatch_tool_uses(orch, &uses).await.expect("dispatch");
    }

    // ----- absolutize (pure, lexical — NOT realpath) -----

    #[test]
    fn absolutize_helper_resolves_lexically() {
        let cwd = PathBuf::from("/repo");
        assert_eq!(absolutize(&cwd, "src/main.rs"), PathBuf::from("/repo/src/main.rs"));
        assert_eq!(absolutize(&cwd, "/abs/x.rs"), PathBuf::from("/abs/x.rs"));
        // `.` dropped, `..` popped — purely lexical.
        assert_eq!(absolutize(&cwd, "./a/../b.rs"), PathBuf::from("/repo/b.rs"));
        // Surrounding whitespace is trimmed (mirrors expandPath).
        assert_eq!(absolutize(&cwd, "  src/a.rs  "), PathBuf::from("/repo/src/a.rs"));
    }

    #[test]
    fn absolutize_does_not_canonicalize_disk() {
        // A path that does NOT exist must still resolve to the joined string
        // (expandPath is lexical, not realpath — no fs canonicalization).
        let cwd = PathBuf::from("/nonexistent-root-xyz");
        let got = absolutize(&cwd, "does/not/exist.rs");
        assert_eq!(got, PathBuf::from("/nonexistent-root-xyz/does/not/exist.rs"));
    }

    #[test]
    fn absolutize_expands_tilde() {
        let Some(home) = dirs::home_dir() else {
            return; // no home dir on this platform — skip
        };
        assert_eq!(absolutize(&PathBuf::from("/repo"), "~/x"), home.join("x"));
        assert_eq!(absolutize(&PathBuf::from("/repo"), "~"), home);
    }

    // ----- cache population through the dispatch loop -----

    #[tokio::test]
    async fn cache_records_successful_read() {
        let dir = tempfile::tempdir().expect("tempdir");
        let sub = dir.path().join("src");
        std::fs::create_dir_all(&sub).expect("mkdir");
        std::fs::write(sub.join("a.rs"), b"fn a() {}").expect("write");

        let cwd = dir.path().to_path_buf();
        let orch = orch_with_tools(
            cwd.clone(),
            vec![Arc::new(StubFileTool { name: "Read", cwd: cwd.clone() }) as Arc<dyn Tool>],
        );
        dispatch_one(&orch, "Read", json!({ "file_path": "src/a.rs" })).await;

        let files = orch.files_in_context().await;
        assert_eq!(files, vec![cwd.join("src").join("a.rs")]);
    }

    #[tokio::test]
    async fn cache_skips_errored_read() {
        let dir = tempfile::tempdir().expect("tempdir");
        let cwd = dir.path().to_path_buf();
        let orch = orch_with_tools(
            cwd.clone(),
            vec![Arc::new(StubFileTool { name: "Read", cwd }) as Arc<dyn Tool>],
        );
        // File does not exist → the tool errors → nothing is cached.
        dispatch_one(&orch, "Read", json!({ "file_path": "missing.rs" })).await;
        assert!(orch.files_in_context().await.is_empty());
    }

    #[tokio::test]
    async fn cache_insertion_order_and_dedup() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("a.rs"), b"a").expect("write a");
        std::fs::write(dir.path().join("b.rs"), b"b").expect("write b");

        let cwd = dir.path().to_path_buf();
        let orch = orch_with_tools(
            cwd.clone(),
            vec![Arc::new(StubFileTool { name: "Read", cwd: cwd.clone() }) as Arc<dyn Tool>],
        );
        // a, b, then a again — first-insertion order [a, b], a not duplicated.
        // This 2-file re-read case coincides with TS's MRU LRU (also [a, b]);
        // the divergence only appears at ≥3 files — see
        // `three_file_reread_locks_first_insertion_order` below.
        dispatch_one(&orch, "Read", json!({ "file_path": "a.rs" })).await;
        dispatch_one(&orch, "Read", json!({ "file_path": "b.rs" })).await;
        dispatch_one(&orch, "Read", json!({ "file_path": "a.rs" })).await;

        let files = orch.files_in_context().await;
        assert_eq!(files, vec![cwd.join("a.rs"), cwd.join("b.rs")]);
    }

    #[tokio::test]
    async fn three_file_reread_locks_first_insertion_order() {
        let dir = tempfile::tempdir().expect("tempdir");
        for f in ["a.rs", "b.rs", "c.rs"] {
            std::fs::write(dir.path().join(f), b"x").expect("write");
        }
        let cwd = dir.path().to_path_buf();
        let orch = orch_with_tools(
            cwd.clone(),
            vec![Arc::new(StubFileTool { name: "Read", cwd: cwd.clone() }) as Arc<dyn Tool>],
        );
        // read a, b, c, then a again. This `Vec` keeps first-insertion order
        // [a, b, c]; TS's MRU-promoting LRU would diverge to [a, c, b]. Locking
        // [a, b, c] pins the documented divergence so a future switch to MRU
        // semantics cannot pass silently.
        for f in ["a.rs", "b.rs", "c.rs", "a.rs"] {
            dispatch_one(&orch, "Read", json!({ "file_path": f })).await;
        }
        let files = orch.files_in_context().await;
        assert_eq!(files, vec![cwd.join("a.rs"), cwd.join("b.rs"), cwd.join("c.rs")]);
    }

    #[tokio::test]
    async fn notebook_edit_records_notebook_path() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("nb.ipynb"), b"{}").expect("write nb");
        let cwd = dir.path().to_path_buf();
        let orch = orch_with_tools(
            cwd.clone(),
            vec![Arc::new(StubFileTool { name: "NotebookEdit", cwd: cwd.clone() }) as Arc<dyn Tool>],
        );
        // `NotebookEdit` keys the cache on `notebook_path`; the stub reads
        // `file_path` to confirm the file exists, so pass both (same path).
        dispatch_one(
            &orch,
            "NotebookEdit",
            json!({ "notebook_path": "nb.ipynb", "file_path": "nb.ipynb" }),
        )
        .await;
        let files = orch.files_in_context().await;
        assert_eq!(files, vec![cwd.join("nb.ipynb")]);
    }

    #[tokio::test]
    async fn edit_and_write_record_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("e.rs"), b"e").expect("write e");
        std::fs::write(dir.path().join("w.rs"), b"w").expect("write w");

        let cwd = dir.path().to_path_buf();
        let orch = orch_with_tools(
            cwd.clone(),
            vec![
                Arc::new(StubFileTool { name: "Edit", cwd: cwd.clone() }) as Arc<dyn Tool>,
                Arc::new(StubFileTool { name: "Write", cwd: cwd.clone() }) as Arc<dyn Tool>,
            ],
        );
        dispatch_one(&orch, "Edit", json!({ "file_path": "e.rs" })).await;
        dispatch_one(&orch, "Write", json!({ "file_path": "w.rs" })).await;

        let files = orch.files_in_context().await;
        assert_eq!(files, vec![cwd.join("e.rs"), cwd.join("w.rs")]);
    }
}
