//! Model-call preparation, model controls, usage accounting, and error handling.

use super::*;

impl ConversationOrchestrator {
    /// `tengu_api_success` `timeSinceLastApiCallMs`: ms since the previous
    /// successful API call, then record this call's timestamp. Returns `None`
    /// on the first call (claude `W=G!==null?Math.max(0,Math.round(M-G)):void 0`).
    #[allow(clippy::cast_sign_loss)]
    pub(crate) fn record_api_call_gap_ms(&self) -> Option<u64> {
        use std::sync::atomic::Ordering;
        let now_ms = i64::try_from(
            self.model_runtime
                .session_started_at
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .elapsed()
                .as_millis(),
        )
        .unwrap_or(i64::MAX);
        let prev = self
            .model_runtime
            .last_api_call_at_ms
            .swap(now_ms, Ordering::SeqCst);
        // `prev < 0` is the `-1` sentinel = no prior call → OMIT the field.
        (prev >= 0).then(|| (now_ms - prev).max(0) as u64)
    }

    /// Drop a recorded kind whose block never reaches persistence.
    ///
    /// Needed because `take_tool_denial_kind` REMOVES on read: when the
    /// streaming executor substitutes a synthetic result for a tool that
    /// already recorded a kind at dispatch, the recorded kind must either be
    /// rewritten to the synthetic's own kind or dropped, or it would both stamp
    /// the wrong provenance and leak an entry for the rest of the session.
    pub(crate) async fn remove_tool_denial_kind(&self, id: &protocol::ToolUseId) {
        self.transcript
            .tool_denial_kinds
            .lock()
            .await
            .remove(&id.to_string());
    }

    /// Record the ASSISTANT line uuid that carried this `tool_use` block —
    /// claude's `sourceToolAssistantUUID`.
    pub(crate) async fn record_source_tool_assistant_uuid(
        &self,
        id: &protocol::ToolUseId,
        assistant_line_uuid: String,
    ) {
        self.transcript
            .tool_source_assistant_uuids
            .lock()
            .await
            .insert(id.to_string(), assistant_line_uuid);
    }

    /// Remove and return the payloads queued for `id`, in production order.
    pub(crate) async fn take_queued_hook_attachments(
        &self,
        id: &protocol::ToolUseId,
    ) -> Vec<serde_json::Value> {
        self.transcript
            .pending_hook_attachments
            .lock()
            .await
            .remove(&id.to_string())
            .unwrap_or_default()
    }

    /// Take the recorded `sourceToolAssistantUUID` under the same guard.
    ///
    /// Emitted ONLY on a map HIT: `run_turn`'s parent derivation falls back to
    /// the turn's last assistant block uuid when the id is missing (a
    /// defensive, in-practice-unreachable branch), and writing a uuid that is
    /// not the tool_use's own line would be worse than omitting the key.
    pub(super) async fn take_source_tool_assistant_uuid(
        &self,
        msg: &ConversationMessage,
    ) -> Option<String> {
        let only = Self::sole_tool_result_id(msg)?;
        self.transcript
            .tool_source_assistant_uuids
            .lock()
            .await
            .remove(&only)
    }

    /// The CURRENT working directory for hook payloads (the post-`cd` shell cwd
    /// when a firer is wired, else the static init `cwd`). Clones out of the
    /// shared cell so no lock is held across an await; a poisoned lock falls
    /// back to the static `cwd`.
    pub(crate) fn current_cwd(&self) -> std::path::PathBuf {
        self.current_cwd
            .lock()
            .map_or_else(|_| self.cwd.clone(), |g| g.clone())
    }

    /// Deterministically derive this session's transcript path from the resolved
    /// claude-home + cwd + session id — the parity analog of claude-code's
    /// `getTranscriptPathForSession(sessionId)` (`utils/sessionStorage.ts:207`),
    /// which `createBaseHookInput` (`utils/hooks.ts:322`) ALWAYS stamps onto every
    /// hook payload. Shape: `<config_home>/projects/<sanitize(cwd)>/<uuid>.jsonl`
    /// (= `session::jsonl::path::session_path`). The id is formatted as the BARE
    /// uuid (`SessionId::as_uuid`) to match claude-code's `${sessionId}.jsonl` and
    /// the on-disk JSONL filename the writer/loader use — NOT the `sess:`-prefixed
    /// [`std::fmt::Display`] form. Returns an empty path when no `config_home` is
    /// wired (test/library builds that also wire no writer), preserving the prior
    /// `""` hook field for those.
    #[must_use]
    pub(crate) fn computed_transcript_path(&self, session_id: &SessionId) -> std::path::PathBuf {
        match &self.config_home {
            Some(home) => session::jsonl::path::session_path(
                home,
                &self.cwd.to_string_lossy(),
                &session_id.as_uuid().to_string(),
            ),
            None => std::path::PathBuf::new(),
        }
    }

    /// Set the mid-turn input source on a SHARED orchestrator (`&self`), so the
    /// bridge can wire its per-connection queue adapter AFTER `engine_desktop::build`
    /// returns the orchestrator as an `Arc`. Set-once: a second call is ignored
    /// (the first wiring wins). See [`Self::with_mid_turn_input`].
    pub fn set_mid_turn_input(
        &self,
        source: Arc<dyn crate::prompt::mid_turn_input::MidTurnInputSource>,
    ) {
        let _ = self.mid_turn_input.set(source);
    }

    /// Set the abort-reason flag on a SHARED orchestrator (`&self`). Set-once.
    /// See [`Self::with_cancel_reason`].
    pub fn set_cancel_reason(&self, flag: crate::prompt::mid_turn_input::CancelReasonFlag) {
        let _ = self.cancel_reason.set(flag);
    }

    /// Byte-exact `/recap` prompt (probed from the 2.1.198 binary). Sent as the
    /// single user turn of the isolated recap side query. Kept in lockstep with
    /// the byte-audit copy in `command-core`'s `recap.rs` test.
    pub(crate) const RECAP_PROMPT: &str = "The user stepped away and is coming back. Recap in under 40 words, 1-2 plain sentences, no markdown. Lead with the overall goal and current task, then the one next action. Skip root-cause narrative, fix internals, secondary to-dos, and em-dash tangents.";

    /// Real `/recap` body — a read-only, tool-denied, single-turn side query,
    /// cancelable via a [`CancellationToken`]. HISTORY-INERT by construction:
    /// unlike [`Self::force_compact_with_cancel`], it reuses the
    /// [`sidequery::ForkedAgentRunner`] directly (the SAME single-turn primitive
    /// the autocompact summarizer uses) and NEVER touches `session.history`,
    /// `apply_post_compact`, `save_cache_safe_params`, or the pre/post-compact
    /// hooks. The runner replays `cache_safe_params.fork_context_messages` +
    /// the recap prompt, exposes no tools, and issues exactly one query.
    ///
    /// On cancel returns `Ok(RecapOutcome::Cancelled)` (a fixed-string outcome),
    /// not `Err`. An empty cache-safe slot (no successful turn recorded yet — a
    /// resumed session whose transcript loaded but which has run no live turn)
    /// maps to `Err(ActionFailed)` rather than a panic.
    pub(crate) async fn generate_recap_query(
        &self,
        cancel: tokio_util::sync::CancellationToken,
    ) -> Result<traits::RecapOutcome, traits::HandleError> {
        let runner = self
            .recap_runner
            .clone()
            .ok_or_else(|| traits::HandleError::ActionFailed("recap unavailable".into()))?;
        let params = self
            .model_runtime
            .cache_safe_slot
            .as_ref()
            .ok_or_else(|| traits::HandleError::ActionFailed("recap: no cache-safe slot".into()))?
            .get_last()
            .await
            .ok_or_else(|| {
                traits::HandleError::ActionFailed("recap: no cache-safe params".into())
            })?;

        // Fast-path cancel (deterministic even when the runner completes
        // synchronously, e.g. the stub side-query path), mirroring
        // `force_compact_with_cancel`.
        if cancel.is_cancelled() {
            return Ok(traits::RecapOutcome::Cancelled);
        }

        let req = sidequery::ForkedAgentRequest {
            prompt_messages: vec![ConversationMessage::user(
                MessageId::new(),
                Self::RECAP_PROMPT.to_string(),
            )],
            cache_safe_params: params,
            fork_label: "recap".into(),
            query_source: sidequery::QuerySource::Custom("recap".into()),
            max_output_tokens: Some(256),
        };

        tokio::select! {
            biased;
            () = cancel.cancelled() => Ok(traits::RecapOutcome::Cancelled),
            r = runner.run(req) => match r {
                Ok(res) => Ok(traits::RecapOutcome::Text(res.final_text.trim().to_string())),
                Err(e) => Err(traits::HandleError::ActionFailed(e.to_string())),
            }
        }
    }

    /// Claude Code 2.1.217's `rename_generate_name` prompt. The response is a
    /// JSON object with a single `name` field; this side query is history-inert
    /// and tool-less, sharing the same fork runner as `/recap`.
    pub(crate) const SESSION_NAME_PROMPT: &str = "Generate a short kebab-case name (2-4 words) that captures the main topic of this conversation. Use lowercase words separated by hyphens. Examples: \"fix-login-bug\", \"add-auth-feature\", \"refactor-api-client\", \"debug-test-failures\". Return JSON with a \"name\" field.";

    pub(crate) async fn generate_session_name_query(
        &self,
        cancel: tokio_util::sync::CancellationToken,
    ) -> Result<Option<String>, traits::HandleError> {
        let runner = self.recap_runner.clone().ok_or_else(|| {
            traits::HandleError::ActionFailed("session name generation unavailable".into())
        })?;
        let Some(params) = self
            .model_runtime
            .cache_safe_slot
            .as_ref()
            .ok_or_else(|| {
                traits::HandleError::ActionFailed("session name generation unavailable".into())
            })?
            .get_last()
            .await
        else {
            return Ok(None);
        };

        if cancel.is_cancelled() {
            return Ok(None);
        }
        let req = sidequery::ForkedAgentRequest {
            prompt_messages: vec![ConversationMessage::user(
                MessageId::new(),
                Self::SESSION_NAME_PROMPT.to_string(),
            )],
            cache_safe_params: params,
            fork_label: "rename".into(),
            query_source: sidequery::QuerySource::Custom("rename_generate_name".into()),
            max_output_tokens: Some(128),
        };
        tokio::select! {
            biased;
            () = cancel.cancelled() => Ok(None),
            result = runner.run(req) => match result {
                Ok(result) => parse_generated_session_name(&result.final_text)
                    .map(Some)
                    .ok_or_else(|| traits::HandleError::ActionFailed(
                        "session name response did not contain a non-empty name".into()
                    )),
                Err(error) => Err(traits::HandleError::ActionFailed(error.to_string())),
            }
        }
    }

    /// Byte-faithful `/btw` side-question wrapper, ported verbatim from
    /// claude-code `utils/sideQuestion.ts`: the `<system-reminder>` that turns
    /// the shared context into a one-off, tool-less answer. Prepended (with a
    /// blank line) to the user's question as the single user turn of the
    /// isolated side query.
    pub(crate) const SIDE_QUESTION_SYSTEM_REMINDER: &str = "<system-reminder>This is a side question from the user. You must answer this question directly in a single response.\n\nIMPORTANT CONTEXT:\n- You are a separate, lightweight agent spawned to answer this one question\n- The main agent is NOT interrupted - it continues working independently in the background\n- You share the conversation context but are a completely separate instance\n- Do NOT reference being interrupted or what you were \"previously doing\" - that framing is incorrect\n\nCRITICAL CONSTRAINTS:\n- You have NO tools available - you cannot read files, run commands, search, or take any actions\n- This is a one-off response - there will be no follow-up turns\n- You can ONLY provide information based on what you already know from the conversation context\n- NEVER say things like \"Let me try...\", \"I'll now...\", \"Let me check...\", or promise to take any action\n- If you don't know the answer, say so - do not offer to look it up or investigate\n\nSimply answer the question with the information you have.</system-reminder>";

    /// Real `/btw` body — the SAME read-only, tool-denied, single-turn,
    /// HISTORY-INERT side query as [`Self::generate_recap_query`], differing
    /// ONLY in the prompt (the wrapped side question instead of `RECAP_PROMPT`).
    /// Reuses the SAME `recap_runner` (the single-turn `ForkedAgentRunner`) and
    /// `cache_safe_slot`, so it needs no new composition-root wiring. NEVER
    /// touches `session.history`, cache writes, or the pre/post-compact hooks;
    /// tool-denial is structural (the single-turn runner exposes no tools).
    ///
    /// Empty cache-safe slot (no successful turn yet) maps to `Err(ActionFailed)`
    /// (the TUI renders "Couldn't answer side question: …") rather than a panic —
    /// the same limitation as `/recap` (the LingXi single-turn runner requires a
    /// captured prefix; cc's from-scratch rebuild fallback is not modeled).
    pub(crate) async fn answer_side_question_query(
        &self,
        question: &str,
        cancel: tokio_util::sync::CancellationToken,
    ) -> Result<traits::RecapOutcome, traits::HandleError> {
        let runner = self
            .recap_runner
            .clone()
            .ok_or_else(|| traits::HandleError::ActionFailed("side question unavailable".into()))?;
        let params = self
            .model_runtime
            .cache_safe_slot
            .as_ref()
            .ok_or_else(|| {
                traits::HandleError::ActionFailed("side question: no cache-safe slot".into())
            })?
            .get_last()
            .await
            .ok_or_else(|| {
                traits::HandleError::ActionFailed(
                    "side question: no context yet — send a message first".into(),
                )
            })?;

        if cancel.is_cancelled() {
            return Ok(traits::RecapOutcome::Cancelled);
        }

        let wrapped = format!("{}\n\n{}", Self::SIDE_QUESTION_SYSTEM_REMINDER, question);
        let req = sidequery::ForkedAgentRequest {
            prompt_messages: vec![ConversationMessage::user(MessageId::new(), wrapped)],
            cache_safe_params: params,
            fork_label: "side_question".into(),
            query_source: sidequery::QuerySource::Custom("side_question".into()),
            // Uncapped like cc's `runSideQuestion` (maxTurns=1, no maxTokens):
            // `None` → the runner's DEFAULT_FORK_MAX_TOKENS.
            max_output_tokens: None,
        };

        tokio::select! {
            biased;
            () = cancel.cancelled() => Ok(traits::RecapOutcome::Cancelled),
            r = runner.run(req) => match r {
                Ok(res) => Ok(traits::RecapOutcome::Text(res.final_text.trim().to_string())),
                Err(e) => Err(traits::HandleError::ActionFailed(e.to_string())),
            }
        }
    }

    /// Snapshot the cache-safe prompt prefix into the wired slot after a
    /// successful API call (In-Loop Compaction Batch 6).
    ///
    /// Strict no-op when no slot is wired (every test + any binary that has not
    /// wired the forked runner), so it adds zero work — and crucially no history
    /// clone — off the production path. When wired, it stores the CURRENT
    /// `session.history` as `fork_context_messages`: callers invoke this right
    /// after the model call returns successfully but BEFORE appending the
    /// assistant reply, so the snapshot is exactly the message set the model
    /// saw (including any PTL truncation / reactive compaction the call applied).
    ///
    /// `user_context` / `system_context` / `tool_use_options` are not consulted
    /// by the single-turn forked summary path (it exposes no tools and replays
    /// `system_prompt` + `fork_context_messages` verbatim — see
    /// `sidequery::ForkedAgentRunner::run`), so they are filled minimally; only
    /// `system_prompt` and `fork_context_messages` drive the cache hit.
    pub(crate) async fn save_cache_safe_params(&self, system: Option<&str>, model: &str) {
        let Some(slot) = self.model_runtime.cache_safe_slot.as_ref() else {
            return;
        };
        let (fork_context_messages, session_id) = {
            let s = self.session.lock().await;
            (s.history.clone(), s.session_id)
        };
        let transcript_path = self
            .transcript
            .jsonl_writer
            .as_ref()
            .map(|writer| writer.active_path())
            .unwrap_or_else(|| self.computed_transcript_path(&session_id));
        slot.save(sidequery::CacheSafeParams {
            system_prompt: system.unwrap_or("").into(),
            user_context: std::collections::HashMap::new(),
            system_context: std::collections::HashMap::new(),
            tool_use_options: tool_api::ToolUseOptions {
                debug: false,
                verbose: false,
                main_loop_model: model.to_string(),
                model_profile: None,
                max_budget_nano_usd: None,
                mcp_clients: Vec::new(),
                is_non_interactive_session: false,
                custom_system_prompt: None,
                append_system_prompt: None,
            },
            fork_context_messages,
            transcript_path: (!transcript_path.as_os_str().is_empty()).then_some(transcript_path),
            // Overwritten by the slot on save; the value here is irrelevant.
            generation: 0,
        })
        .await;
    }

    /// FORK (codex #5 follow-up): record the rendered system-prompt bytes this
    /// turn handed the model, so a fork-subagent spawn dispatched later in the
    /// SAME turn can thread them onto its child (cache-identical prefix, claude
    /// `AgentTool.tsx:622-623`). Called by the turn drivers right after a
    /// successful API call (the `save_cache_safe_params` site). `None`/empty
    /// system collapses to `None` (a turn with no system prompt records nothing).
    pub(crate) async fn save_current_turn_system_prompt(&self, system: Option<&str>) {
        *self.prompt_runtime.current_turn_system_prompt.lock().await =
            system.filter(|s| !s.is_empty()).map(ToString::to_string);
    }

    /// FORK (codex #5 follow-up): the rendered system prompt recorded by the most
    /// recent successful turn ([`Self::save_current_turn_system_prompt`]), or
    /// `None` before the first successful turn / when that turn had no system
    /// prompt. Read by the fork dispatch path to seed each tool's
    /// `ToolUseContext::fork_parent_system_prompt`.
    pub(crate) async fn current_turn_system_prompt(&self) -> Option<String> {
        self.prompt_runtime
            .current_turn_system_prompt
            .lock()
            .await
            .clone()
    }

    /// Task 8 (llm-client future-work batch 3): forward the API client's
    /// latest unified rate-limit header snapshot to
    /// [`traits::OutputStream::emit_rate_limit`], emitting ONLY when it
    /// differs from the last emitted value (emit-on-change dedup against
    /// [`Self::last_emitted_rate_limit`]).
    ///
    /// Called by the turn drivers after each completed API call — the
    /// batched/cancelable seam in `turn_loop::execute_one_turn_with_recovery_tracked`
    /// and the streaming seam in `try_run_turn_streaming` (both right after
    /// `save_cache_safe_params`, the existing "API call succeeded" point).
    /// `self.api` and `self.streaming_api` are the same `ProviderApiAdapter`
    /// in production, and the adapter records headers on both
    /// `drive_non_stream` and the `drive_stream` connect-success path, so
    /// reading `self.api` covers both drivers.
    ///
    /// A strict no-op when the client has no snapshot (the default
    /// `last_rate_limit_full()` returns `None` — mocks / non-Anthropic
    /// providers), so all pre-existing fixtures see zero extra events.
    pub(crate) async fn emit_rate_limit_if_changed(&self) {
        let Some(info) = self.api.last_rate_limit_full() else {
            return;
        };
        let mut last = self.model_runtime.last_emitted_rate_limit.lock().await;
        if last.as_ref() == Some(&info) {
            return;
        }
        self.output
            .emit_rate_limit(
                info.status.as_deref(),
                info.rate_limit_type.as_deref(),
                info.utilization,
                info.resets_at,
                info.claim_resets_at,
                info.overage_status.as_deref(),
                info.overage_resets_at,
                info.overage_disabled_reason.as_deref(),
                info.fallback_available,
                info.upgrade_paths.as_deref(),
                info.credits_required,
            )
            .await;
        *last = Some(info);
    }

    /// Task 2 (llm-client future-work batch 5): forward the API client's
    /// latest RAW per-window utilization snapshot to
    /// [`traits::OutputStream::emit_raw_utilization`] when it CHANGED since
    /// the last emit. The empty snapshot is significant: it clears a
    /// previously-rendered utilization window in downstream clients.
    ///
    /// Called immediately next to [`Self::emit_rate_limit_if_changed`] at
    /// both turn-driver seams (the batched/cancelable funnel in
    /// `turn_loop::execute_one_turn_with_recovery_tracked` and the streaming
    /// seam in `try_run_turn_streaming`), reading the same
    /// `self.api`-cached snapshot source.
    ///
    /// TS updates `rawUtilization` unconditionally on every headers pass
    /// (`claudeAiLimits.ts:476` and the 429 path `:500`) because it is module
    /// state polled by `getRawUtilization()`. Our event channel emits only on
    /// change to avoid spamming the stream, while still forwarding the empty
    /// snapshot as `(None, None, None, None)` so event-driven clients observe
    /// the same state transition.
    ///
    /// Atomic-window invariant: each window contributes either both `Some`
    /// values or both `None` — guaranteed by construction, since
    /// [`crate::model::rate_limit::RawWindow`] only exists with both fields.
    pub(crate) async fn emit_raw_utilization_if_changed(&self) {
        let Some(raw) = self.api.last_raw_utilization() else {
            return;
        };
        let mut last = self.model_runtime.last_emitted_raw_utilization.lock().await;
        if last.as_ref() == Some(&raw) {
            return;
        }
        self.output
            .emit_raw_utilization(
                raw.five_hour.map(|w| w.utilization),
                raw.five_hour.map(|w| w.resets_at),
                raw.seven_day.map(|w| w.utilization),
                raw.seven_day.map(|w| w.resets_at),
            )
            .await;
        *last = Some(raw);
    }

    pub(crate) async fn record_vision_delegation_usage(
        &self,
        model: &str,
        profile: Option<&str>,
        result: &sidequery::VisionDelegationResult,
    ) {
        self.record_vision_delegation_accounting(
            model,
            profile,
            result.usage.clone(),
            result.elapsed,
            result.retry_count,
            result.api_calls,
        )
        .await;
    }

    pub(crate) async fn record_vision_delegation_accounting(
        &self,
        model: &str,
        profile: Option<&str>,
        usage: cost::Usage,
        elapsed: std::time::Duration,
        retry_count: u32,
        api_calls: u32,
    ) {
        if api_calls == 0 {
            return;
        }
        let model_ref = crate::cost_wiring::model_ref_from_string(model, profile);
        if let Some(tracker) = self.model_runtime.cost_tracker.as_ref() {
            tracker
                .record_api_response_v2(
                    model_ref,
                    usage.clone(),
                    elapsed,
                    retry_count,
                    usage.tokens.cache_read,
                    usage
                        .tokens
                        .cache_write
                        .saturating_add(usage.tokens.cache_write_1h),
                    false,
                    self.model_runtime.analytics_bus.as_ref(),
                )
                .await;
        }
        self.model_runtime
            .api_calls_recorded
            .fetch_add(api_calls, std::sync::atomic::Ordering::SeqCst);
    }

    /// Read the current cost state from the wired tracker, if any.
    /// Returns `None` if no tracker was attached. Exposed so future M7
    /// renderers (per-model breakdown view) can access
    /// `CostState.per_model_usage` without going through the leaf-friendly
    /// [`traits::CostSnapshot`] projection. (M6-06)
    pub async fn cost_state(&self) -> Option<cost::CostState> {
        let t = self.model_runtime.cost_tracker.as_ref()?;
        Some(t.snapshot().await)
    }

    /// Seed the wired [`cost::CostTracker`]'s cumulative total from a restored
    /// session (resume). No-op when no tracker is wired. Paired with the CLI
    /// mount's project-config `lastCost`/`lastSessionId` persistence so a
    /// `--resume`d session's footer continues from the prior accumulated cost
    /// instead of resetting to `$0.0000` (claude-code `restoreCostStateForSession`).
    pub async fn restore_session_cost(&self, total_nano_usd: u64) {
        if let Some(tracker) = self.model_runtime.cost_tracker.as_ref() {
            tracker.restore_total_nano_usd(total_nano_usd).await;
        }
    }

    /// Project the wired [`cost::CostTracker`] state onto the
    /// leaf-friendly [`traits::CostSnapshot`]. Used by both the
    /// trait method `snapshot_cost` and the per-turn end-of-turn emitter
    /// (`OutputStream::emit_end_turn`). (M6-06)
    ///
    /// If no tracker is wired, returns a zero-valued snapshot keyed to
    /// the current session id (backward-compat shape).
    pub async fn snapshot_cost_real(&self) -> traits::CostSnapshot {
        let session_id = self.session.lock().await.session_id;
        let Some(tracker) = self.model_runtime.cost_tracker.as_ref() else {
            return traits::CostSnapshot {
                session_id,
                ..traits::CostSnapshot::default()
            };
        };
        let state = tracker.snapshot().await;
        // Sum per-model usage into aggregate token counters. api_calls comes
        // from our own counter because cost::Usage does not carry a
        // per-call count (its `add()` merges token totals only).
        let (mut input_tokens, mut output_tokens, mut cache_read_tokens, mut cache_creation_tokens) =
            (0u64, 0u64, 0u64, 0u64);
        for entry in state.per_model_usage.values() {
            input_tokens = input_tokens.saturating_add(entry.usage.tokens.input);
            output_tokens = output_tokens.saturating_add(entry.usage.tokens.output);
            cache_read_tokens = cache_read_tokens.saturating_add(entry.cache_read_input_tokens);
            cache_creation_tokens =
                cache_creation_tokens.saturating_add(entry.cache_creation_input_tokens);
        }
        let api_calls = self
            .model_runtime
            .api_calls_recorded
            .load(std::sync::atomic::Ordering::SeqCst);
        #[allow(clippy::cast_precision_loss)]
        let total_usd = (state.total_nano_usd as f64) / 1_000_000_000.0;
        let session_duration = self
            .model_runtime
            .session_started_at
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .elapsed();
        traits::CostSnapshot {
            session_id,
            total_nano_usd: state.total_nano_usd,
            total_tokens: input_tokens.saturating_add(output_tokens),
            total_usd,
            input_tokens,
            output_tokens,
            cache_read_tokens,
            cache_creation_tokens,
            api_calls,
            session_duration,
            api_duration: std::time::Duration::from_millis(state.total_api_duration_ms),
            code_lines_added: state.total_lines_added,
            code_lines_removed: state.total_lines_removed,
            by_model: state
                .per_model_usage
                .values()
                .map(|mu| traits::orchestrator::ModelUsageRow {
                    model: mu.model_ref.model.clone(),
                    // (cc 2.1.218) `n.provider=n_(r)` — the serving provider,
                    // pre-stringified so the transport row stays cost-free.
                    provider: Some(mu.model_ref.provider.usage_wire_name()),
                    total_nano_usd: mu.cost_nano_usd,
                    input_tokens: mu.usage.tokens.input,
                    output_tokens: mu.usage.tokens.output,
                    cache_read_input_tokens: mu.cache_read_input_tokens,
                    cache_creation_input_tokens: mu.cache_creation_input_tokens,
                })
                .collect(),
            unknown_models: !state.unpriced_models.is_empty(),
            current_usage: state.last_usage.map(|usage| traits::CurrentUsageSnapshot {
                input_tokens: usage.tokens.input,
                output_tokens: usage.tokens.output,
                cache_read_input_tokens: state.last_cache_read_input_tokens,
                cache_creation_input_tokens: state.last_cache_creation_input_tokens,
            }),
        }
    }

    /// True when a `max_budget_nano_usd` cost ceiling is set AND the session's
    /// cumulative cost has reached it — 1:1 with claude-code
    /// `getTotalCost() >= maxBudgetUsd` (`QueryEngine.ts:972`). Always false when
    /// no cap is set, OR when no [`cost::CostTracker`] is wired (the cap cannot
    /// be enforced without cost tracking — a headless `--max-budget` run, which
    /// wires the tracker, is the primary consumer). Checked at each turn-loop
    /// iteration so a run stops once it crosses the ceiling.
    pub(super) async fn over_budget(&self) -> bool {
        let Some(budget) = self.config.max_budget_nano_usd else {
            return false;
        };
        let Some(tracker) = self.model_runtime.cost_tracker.as_ref() else {
            return false;
        };
        tracker.snapshot().await.total_nano_usd >= budget
    }

    /// A3: construct a fresh [`BudgetTracker`] for this turn IFF the
    /// token-budget feature is enabled AND a positive budget is configured.
    ///
    /// Returns `None` (the parity default) when
    /// [`OrchestratorConfig::enable_token_budget`] is `false` or
    /// [`OrchestratorConfig::token_budget`] is `None`/`Some(0)` — in which case
    /// the turn drivers skip the budget check entirely and stop at the first
    /// `end_turn`, preserving the locked turn-loop behaviour.
    pub(super) fn new_budget_tracker(&self) -> Option<BudgetTracker> {
        if self.config.enable_token_budget && matches!(self.config.token_budget, Some(b) if b > 0) {
            Some(BudgetTracker::new())
        } else {
            None
        }
    }

    /// A3: consult the token budget at a natural end-of-turn.
    ///
    /// Returns `true` if the loop should CONTINUE (a continuation nudge was
    /// injected as a meta user message and the A1 recovery count was reset per
    /// `query.ts:1332`); `false` if the loop should stop (budget off, agent
    /// context, threshold reached, or diminishing returns).
    ///
    /// 1:1 with TS `query.ts:1308-1355`: gated by `feature('TOKEN_BUDGET')`,
    /// drives [`check_token_budget`], and on `continue` appends a meta user
    /// message carrying the byte-exact `getBudgetContinuationMessage` nudge.
    /// The completion telemetry is emitted as a `tracing` event on stop.
    /// The continuation nudge is injected as a META user message
    /// ([`Self::inject_meta_user_message`] / [`ConversationMessage::user_meta`])
    /// into both the live session history and the JSONL persistence stream,
    /// where it persists with top-level `isMeta:true`. The same META injection
    /// pattern is shared by the malformed-tool-use retry (#77), thinking-only
    /// (#78), and max-output-tokens recovery nudges.
    /// Finding #80: refusal → fallback-model swap (claude-code `bin/claude.exe`
    /// offset ~205871579, `vr === "refusal" && rc !== void 0`). When the active
    /// turn's response has `stop_reason == "refusal"`, a
    /// [`OrchestratorConfig::refusal_fallback_model`] is configured, AND the
    /// once-per-session latch ([`Self::refusal_fallback_latched`]) is not yet set:
    ///
    /// 1. set the latch (so the fallback fires at most once per session — the
    ///    binary's `refusalFallbackModelLatch` makes the override sticky);
    /// 2. persistently swap the session model to the fallback (the binary's
    ///    `setAppState mainLoopModel = fallbackModel` + `jT(fallbackModel)` —
    ///    every subsequent turn re-snapshots `session.model`, so the swap sticks);
    /// 3. surface the user-visible warning on the output stream (the binary's
    ///    `type:"system", subtype:"model_refusal_fallback", level:"warning",
    ///    content: Bwn(originalModel, fallbackModel, category)`), reproduced
    ///    byte-exact for the common `category == "other"` shape.
    ///
    /// Returns `true` when the swap happened (the caller must retry/continue the
    /// turn against the fallback model), `false` when no fallback is configured or
    /// the latch is already set (the caller preserves today's terminal behavior).
    /// The user-visible refusal-fallback line (2.1.206 `VPn(e,t,r)` for the
    /// common `category == "other"` path: the generic `$7m` prefix, then
    /// `Switched to {marketing name}`, then the feedback line).
    ///
    /// Lives here rather than inline so the emit site and the tests read the
    /// same bytes. The cyber/bio "intentionally broad" variant still needs the
    /// refusal category routed through — see the typed notice's
    /// `api_refusal_category`, which now carries it.
    pub(crate) fn refusal_warning_text(fallback: &str) -> String {
        let display = crate::prompt::env_meta::marketing_name_for_model(fallback)
            .map(String::from)
            .unwrap_or_else(|| fallback.to_string());
        format!(
            "This model's safeguards flagged this message. \
This sometimes happens with safe, normal conversations. Switched to {display}. \
Send feedback with /feedback or learn more: https://support.claude.com/en/articles/15363606"
        )
    }

    pub(crate) async fn maybe_swap_to_refusal_fallback(&self) -> bool {
        // The CASCADE: an ordered chain of models, each tried as the previous
        // one refuses. An empty chain falls back to the historical single
        // `refusal_fallback_model`, which is exactly a one-element chain — so
        // the default path is byte-identical to before the cascade existed.
        let chain: Vec<String> = if self.config.refusal_fallback_chain.is_empty() {
            self.config
                .refusal_fallback_model
                .clone()
                .into_iter()
                .collect()
        } else {
            self.config.refusal_fallback_chain.clone()
        };
        if chain.is_empty() {
            return false;
        }
        // Models already tried THIS EPISODE, so a cascade cannot loop back onto
        // one that has already refused.
        let tried = self.model_runtime.refusal_tried_models.lock().await.clone();
        let route = crate::refusal_cascade::route_refusal(
            &crate::refusal_cascade::RouteInputs {
                chain: Some(&chain),
                armed_fallback_model: None,
                armed_target_is_refusing_model: false,
                catch_all_enabled: false,
            },
            |stage| {
                // A stage is reachable when it has not already been routed to
                // this episode. Claude's exclusion is exactly `triedModels`,
                // which resets with the session — deliberately NOT "differs
                // from the current model": after a hop the current model IS the
                // previous fallback, and excluding it would make a cleared
                // session unable to route to that model again.
                (!tried.iter().any(|m| m == stage)).then(|| stage.to_string())
            },
        );
        // Report every stage the walk passed over. A chain that silently
        // degraded to its last entry is otherwise indistinguishable from one
        // that worked first try.
        for report in crate::refusal_cascade::decline_reports(&route) {
            tracing::info!(
                event = "tengu_refusal_fallback_route_declined",
                reason = report.as_str(),
            );
        }
        let crate::refusal_cascade::RefusalRoute::Category { stage, .. } = route else {
            return false;
        };
        // What the cascade still has left. A hop with stages remaining may be
        // superseded, so its notice is provisional.
        let stage_remaining = stage.remaining_chain;
        let fallback = stage.model;
        // Once-per-session latch (refusalFallbackModelLatch analog) — applies
        // only to a SINGLE-hop chain, which is the historical shape. A real
        // cascade is bounded by the chain instead: each hop is consumed by
        // `tried`, so the walk terminates on its own without needing the latch
        // to cap it.
        if chain.len() <= 1
            && self
                .model_runtime
                .refusal_fallback_latched
                .swap(true, std::sync::atomic::Ordering::SeqCst)
        {
            return false;
        }
        self.model_runtime
            .refusal_tried_models
            .lock()
            .await
            .push(fallback.clone());
        // Persistently swap the session model to the fallback.
        let original_model = {
            let mut s = self.session.lock().await;
            let prev = std::mem::replace(&mut s.model, fallback.clone());
            // The fallback model has no associated provider profile (mirrors the
            // overload-fallback re-issue, which passes `profile = None`).
            s.model_profile = None;
            prev
        };
        // User-visible warning. 2.1.206 `VPn(e,t,r)` =
        //   `${f_t(r) ? mmi(e) : hmi(e,r)} Switched to ${Mf(t)}. ${bxr(e)}`
        // for the common `category == "other"` path: `f_t("other")` is false, so
        // `hmi(e,"other")` fires with the generic `$7m` prefix ("This model's
        // safeguards flagged this message. This sometimes happens with safe,
        // normal conversations."); `bxr(e)` is the feedback line. `Mf(t)` = the
        // fallback's MARKETING NAME (byte-verified: 206 uses the friendly name,
        // not the raw id) — resolve it, falling back to the id for an unknown
        // model. (The cyber/bio `mmi(e)` "intentionally broad" variant needs the
        // refusal category routed through here — deferred with the typed
        // model_refusal_fallback system frame.)
        // Route the notice through the episode accumulator and the collapse
        // queue rather than emitting it directly. A hop that a LATER hop
        // supersedes must not reach the user: "switched to X" stops being true
        // the moment the cascade moves on from X. So an intermediate hop is
        // held PROVISIONALLY and folded into the notice that finally settles,
        // which reports how many hops it collapsed.
        let more_hops_possible = !stage_remaining.is_empty();
        let notice_uuid = uuid::Uuid::new_v4().to_string();
        let emitted = {
            let mut episode = self.model_runtime.refusal_episode.lock().await;
            episode.merge(crate::refusal_notice::RefusalNotice {
                uuid: notice_uuid.clone(),
                origin_model: original_model.clone(),
                serving_model: fallback.clone(),
                ..crate::refusal_notice::RefusalNotice::default()
            });
            let taken = if more_hops_possible {
                episode.take_provisional(&notice_uuid)
            } else {
                episode.settle()
            };
            drop(episode);
            match taken {
                Some(notice) => self
                    .model_runtime
                    .refusal_notice_queue
                    .lock()
                    .await
                    .accept(notice, more_hops_possible),
                None => Vec::new(),
            }
        };
        for e in emitted {
            if e.suppressed_count > 0 {
                tracing::info!(
                    event = "tengu_refusal_fallback_notice_collapsed",
                    suppressed_count = e.suppressed_count,
                    emitted_via = e.emitted_via.as_str(),
                );
            }
            self.output
                .emit_text(&Self::refusal_warning_text(&e.banner.serving_model))
                .await;
        }
        // Success-path analytics — inline event name (NOT a locked telemetry
        // const), so the 347-entry `ALL_EVENT_NAMES` fixture lock is unperturbed.
        tracing::info!(
            event = "tengu_refusal_fallback_triggered",
            original_model = %original_model,
            fallback_model = %fallback,
            trigger = "refusal",
        );
        true
    }

    /// Git env-block probe + `gitStatus:` attachment, frozen from the first
    /// resolved host-side cwd for the conversation lifetime.
    pub(super) async fn cached_git_status(
        &self,
        cwd: &std::path::Path,
    ) -> (Option<crate::prompt::GitStatus>, Option<String>) {
        {
            let cache = self.prompt_runtime.git_status_snapshot.lock().await;
            if let Some(snap) = cache.as_ref() {
                return (snap.probe.clone(), snap.block.clone());
            }
        }
        let probe = crate::prompt::git_status::probe(cwd);
        let block = crate::prompt::git_status::render_git_status_block(cwd);
        *self.prompt_runtime.git_status_snapshot.lock().await = Some(GitStatusSnapshot {
            probe: probe.clone(),
            block: block.clone(),
        });
        (probe, block)
    }

    /// Schedule a best-effort startup Responses WebSocket prewarm.
    ///
    /// The prewarm uses the current model/profile, assembled system prompt, and
    /// current tool list with an empty conversation history. A later first turn
    /// can then reuse the returned `response.id` when its request is a strict
    /// extension of this prefix. Failures are intentionally ignored.
    pub fn spawn_startup_responses_websocket_prewarm(self: &Arc<Self>) {
        self.abort_startup_responses_websocket_prewarm();
        let orchestrator = Arc::clone(self);
        let handle = tokio::spawn(async move {
            let (model, profile) = {
                let session = orchestrator.session.lock().await;
                (session.model.clone(), session.model_profile.clone())
            };
            let system = orchestrator.build_system_prompt().await;
            let tools = orchestrator.build_wire_tools().await;
            let _ = orchestrator
                .api
                .prewarm_responses_websocket(
                    &model,
                    profile.as_deref(),
                    Some(&system),
                    Vec::new(),
                    tools,
                )
                .await;
        });
        *self
            .lifecycle_runtime
            .startup_responses_websocket_prewarm
            .lock()
            .expect("startup responses websocket prewarm") = Some(handle);
    }

    /// Abort any pending startup Responses WebSocket prewarm task.
    pub fn abort_startup_responses_websocket_prewarm(&self) {
        if let Some(handle) = self
            .lifecycle_runtime
            .startup_responses_websocket_prewarm
            .lock()
            .expect("startup responses websocket prewarm")
            .take()
        {
            handle.abort();
        }
    }

    /// Task 6 (llm-client future-work batch 5): re-map a terminal
    /// `RateLimited` error onto the limits-specific copy the API client
    /// composed from the 429's own unified headers (claude-code
    /// `errors.ts:480-524`). Applied by every public turn driver so each
    /// consumer of the error's `Display` (CLI stderr, TUI scrollback,
    /// `client-adapter` `ClientEvent::Error`) sees the
    /// `"You've hit your … limit · resets …"` copy instead of the generic
    /// `"api call failed: rate limited"`. No-op for non-429 errors and when
    /// the 429 carried no unified headers.
    /// Text for a graceful `model_error` surface. Verbatim for parity EXCEPT a
    /// `ModelUnavailable` (a provider 404): enrich it with the model id and a
    /// `/model` hint so the user knows WHICH model the provider rejected and how
    /// to switch — the bare "model unavailable" was opaque, especially with a
    /// stale third-party catalog entry (multi-provider UX).
    pub(crate) async fn model_error_text(&self, err: &LlmError) -> String {
        match err {
            LlmError::ModelUnavailable => {
                let model = self.session.lock().await.model.clone();
                format!(
                    "model unavailable: the provider does not serve '{model}' (HTTP 404). Pick another model with /model."
                )
            }
            // 413 request-too-large (accumulated images/attachments): render the
            // byte-exact `$Vi()` notice instead of the opaque "request too large".
            LlmError::RequestTooLarge => request_too_large_notice(self.prompt_is_interactive()),
            // Billing (`Flp`: `yu({content:LYr,error:"billing_error"})`) and
            // prompt-too-long (`content:Jq`) both render BARE — no `API Error:`
            // prefix. Both used to fall through to `Display`, i.e. the words
            // "quota exceeded" and "context overflow".
            LlmError::QuotaExceeded => crate::api_error_copy::CREDIT_BALANCE_TOO_LOW.to_string(),
            // Dead OAuth session (`e instanceof qQt`): the IdP REJECTED the
            // stored refresh token, so no retry can help and only a fresh
            // sign-in will. Keyed on the variant, mirroring the oracle's
            // instanceof check rather than sniffing message text.
            LlmError::OAuthRefreshDead => {
                crate::api_error_copy::oauth_refresh_dead_text(self.prompt_is_interactive())
                    .to_string()
            }
            // Revoked OAuth token (`Uke`): a 403 whose message names it. Checked
            // BEFORE the x-api-key branch, matching the oracle's order, and
            // split on interactivity because a non-interactive caller cannot
            // run the auth command.
            //
            // The non-interactive half names a product, so it takes the LIVE
            // provider profile: this renderer is shared by every provider, and
            // the oracle's hardcoded "Claude" would be wrong for a session
            // routed elsewhere.
            LlmError::Authentication { .. } | LlmError::PermissionDenied { .. }
                if crate::api_error_copy::is_oauth_revoked(
                    err.http_status(),
                    err.provider_message().unwrap_or_default(),
                ) =>
            {
                let profile = self.session.lock().await.model_profile.clone();
                crate::api_error_copy::oauth_revoked_text(
                    self.prompt_is_interactive(),
                    profile.as_deref(),
                )
            }
            // Org policy turned the SUBSCRIPTION path off (`de_()` → `ce_`, a
            // 401/403 naming it). The oracle checks this BEFORE the API-key
            // disablement below, and the two are mirror images: this one sends
            // the user to an API key, that one sends them to sign-in. Ordering
            // them wrongly would hand a blocked user the remedy their org just
            // disabled.
            LlmError::Authentication { .. } | LlmError::PermissionDenied { .. }
                if crate::api_error_copy::is_oauth_org_not_allowed(
                    err.http_status(),
                    err.provider_message().unwrap_or_default(),
                ) =>
            {
                crate::api_error_copy::OAUTH_ORG_NOT_ALLOWED.to_string()
            }
            // Org policy turned API-key auth off (403 naming it). Checked
            // before the generic credential branch, and names the specific
            // thing THIS user has to unset.
            LlmError::Authentication { .. } | LlmError::PermissionDenied { .. }
                if crate::api_error_copy::is_api_key_auth_disabled(
                    err.http_status(),
                    err.provider_message().unwrap_or_default(),
                ) =>
            {
                let profile = self.session.lock().await.model_profile.clone();
                crate::api_error_copy::api_key_auth_disabled_text(
                    &self.config.credential_origin,
                    self.config.has_oauth_token,
                    profile.as_deref(),
                )
            }
            // Credential rejection. The oracle gates this on the MESSAGE naming
            // `x-api-key`, not on the status, then splits on where the key came
            // from: an env var or `apiKeyHelper` gets "fix that", everything
            // else gets "/login" — because /login cannot fix an external key.
            //
            // A 401/403 that does NOT name the header falls through to the
            // variant's own text, matching the oracle's outer `if`.
            LlmError::Authentication { .. } | LlmError::PermissionDenied { .. }
                if crate::api_error_copy::mentions_api_key_header(
                    err.provider_message().unwrap_or_default(),
                ) =>
            {
                // A cloud-hosted route names ITS credential problem instead —
                // "run gcloud auth ..." is useful advice, "/login" is not. The
                // 401-vs-other split inside is only decidable because the status
                // now survives in the message.
                // `if(qOu()) return UOu` comes FIRST in the oracle: on a
                // remote session the failure is reported as possibly transient
                // before anything looks at the credential's source.
                if crate::api_error_copy::is_remote_session() {
                    return crate::api_error_copy::AUTH_TRANSIENT.to_string();
                }
                // `xn()==="gateway"` comes next in the oracle. It IS reachable:
                // `Mt.gatewayAuth` is bootstrapped from the environment
                // (`CLAUDE_CODE_USE_GATEWAY` + `ANTHROPIC_BASE_URL` +
                // `ANTHROPIC_AUTH_TOKEN`), so the route resolves without any
                // runtime credential object. When a gateway fronts the provider
                // and IT cannot authenticate upstream, no credential the user
                // holds is at fault — say so instead of sending them to /connect.
                let route = self
                    .config
                    .error_route
                    .clone()
                    .unwrap_or_else(crate::api_error_copy::ErrorRouteTag::from_env);
                if matches!(route, crate::api_error_copy::ErrorRouteTag::Gateway) {
                    return crate::api_error_copy::GATEWAY_UPSTREAM_AUTH_FAILED.to_string();
                }
                crate::api_error_copy::cloud_credential_text(&route, err.http_status())
                    .unwrap_or_else(|| {
                        crate::api_error_copy::credential_rejected_text(
                            &self.config.credential_origin,
                        )
                        .to_string()
                    })
            }
            // TERMINAL 401/403 arm (@230607344). Every specific auth branch
            // above declined, so the oracle still renders an `API Error:` line
            // carrying the provider detail rather than falling through to the
            // variant's bare `Display` ("authentication failed" /
            // "permission denied"), which is what this port used to show.
            LlmError::Authentication { .. } | LlmError::PermissionDenied { .. } => {
                crate::api_error_copy::auth_failed_fallback(
                    self.prompt_is_interactive(),
                    // Oracle: `let i = sir(e)` — the SAME normalizer the retry
                    // banner uses, so both surfaces agree.
                    &llm_client::error_display_text(err),
                )
            }
            LlmError::ContextOverflow { .. } => crate::api_error_copy::PROMPT_TOO_LONG.to_string(),
            // 429: the oracle renders `API Error: Request rejected (429) · …`,
            // pulling the detail out of the JSON body the decoder stringified
            // into the message. This used to fall through to `Display`, which is
            // the bare words "rate limited".
            //
            // Two first-party variants are NOT selected here and are documented
            // as such in `api_error_copy`: the `Server is temporarily limiting
            // requests` label and the `hpo()` status-page/gateway suffix both
            // need provider-route plumbing this layer does not have. The
            // fallback clause only shows when the body carries no detail, which
            // a real 429 does.
            LlmError::RateLimited { .. } => {
                let raw = err.to_string();
                let source = self.api.last_rate_limit_error_message().unwrap_or(raw);
                if crate::api_error_copy::is_long_context_credit_message(&source) {
                    crate::api_error_copy::usage_credits_required_for_1m_context(false)
                } else {
                    // `let p = i ? le_ : "Request rejected (429)"`.
                    //
                    // INFERRED, not read off `eir`'s body: `i = eir(ii())` gates
                    // three branches here — the 1M-credits clamp, the
                    // overage-disabled-reason lookup, and this label — all of
                    // which are subscription concepts, so it reads as "this is a
                    // claude.ai subscriber". `is_subscriber` is LingXi's
                    // equivalent and is already threaded from the composition
                    // root. The distinction matters: telling a subscriber their
                    // request was "rejected" implies they hit their own limit,
                    // which is exactly what `le_` exists to deny.
                    let label = if self.config.is_subscriber {
                        crate::api_error_copy::SERVER_LIMITING
                    } else {
                        crate::api_error_copy::REQUEST_REJECTED_429
                    };
                    crate::api_error_copy::rate_limited_text(
                        &source,
                        label,
                        &crate::api_error_copy::capacity_fallback(Some(
                            &self
                                .config
                                .error_route
                                .clone()
                                .unwrap_or_else(crate::api_error_copy::ErrorRouteTag::from_env),
                        )),
                    )
                }
            }
            other => other.to_string(),
        }
    }

    pub(super) fn enrich_api_error(&self, err: OrchestratorError) -> OrchestratorError {
        enrich_rate_limited_error(err, self.api.last_rate_limit_error_message())
    }

    /// Advance the session-scoped Ultracode state at the one prompt-ingress
    /// seam shared by batched, streaming, and cancelable drivers.
    pub(super) async fn append_ultracode_attachments(&self, prompt: &str) {
        use tool_api::tool_trait::ToolStaticContext;
        use tool_workflow::{UltracodeConfig, UltracodeGate, UltracodeState};

        let workflows_enabled = self
            .tools
            .available_tools(&ToolStaticContext::default())
            .iter()
            .any(|tool| tool.name() == tool_workflow::TOOL_NAME);
        let effort = self
            .model_runtime
            .current_effort
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        let is_meta_turn = prompt.trim_start().starts_with('/');
        let attachments = {
            let mut session = self.session.lock().await;
            let mut state = UltracodeState {
                active: session.ultracode_active,
                non_meta_turns_since_reminder: session.ultracode_non_meta_turns_since_reminder,
            };
            let attachments = state.advance(
                UltracodeGate {
                    model: &session.model,
                    effort: effort.as_deref(),
                    workflows_enabled,
                },
                UltracodeConfig {
                    feature_flag_cadence: self.config.ultracode_feature_flag_cadence,
                    product_default_cadence: self.config.ultracode_product_default_cadence,
                    keyword_trigger_enabled: self.config.workflow_keyword_trigger_enabled,
                },
                prompt,
                is_meta_turn,
            );
            session.ultracode_active = state.active;
            session.ultracode_non_meta_turns_since_reminder = state.non_meta_turns_since_reminder;
            attachments
        };

        for attachment in attachments {
            let message = ConversationMessage::user_meta(
                MessageId::new(),
                format!("<system-reminder>\n{}\n</system-reminder>", attachment.text),
            );
            self.session.lock().await.history.push(message.clone());
            self.persist_message_to_jsonl(&message).await;
        }
    }

    pub(super) async fn sync_goal_checkin_idle_task(&self) {
        let should_run = self.lifecycle_runtime.stop_hook_snapshot.is_some()
            && crate::prompt::goal_checkin::checkin_interval_ms() > 0
            && self.session.lock().await.active_goal.is_some()
            && self
                .lifecycle_runtime
                .goal_checkin
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .deferred_since
                .is_some();

        if should_run {
            if self
                .lifecycle_runtime
                .goal_checkin_idle_running
                .load(std::sync::atomic::Ordering::Acquire)
            {
                return;
            }
            let Some(provider) = self.lifecycle_runtime.stop_hook_snapshot.clone() else {
                return;
            };
            let generation = self
                .lifecycle_runtime
                .goal_checkin_idle_generation
                .fetch_add(1, std::sync::atomic::Ordering::AcqRel)
                .saturating_add(1);
            self.lifecycle_runtime
                .goal_checkin_idle_running
                .store(true, std::sync::atomic::Ordering::Release);
            let writer = self.transcript.jsonl_writer.clone();
            let session = Arc::clone(&self.session);
            let turn_gate = Arc::clone(&self.turn_gate);
            let goal_checkin = Arc::clone(&self.lifecycle_runtime.goal_checkin);
            let last_jsonl_uuid = Arc::clone(&self.transcript.last_jsonl_uuid);
            let current_cwd = Arc::clone(&self.current_cwd);
            let fallback_cwd = self.cwd.clone();
            let running = Arc::clone(&self.lifecycle_runtime.goal_checkin_idle_running);
            let generation_counter =
                Arc::clone(&self.lifecycle_runtime.goal_checkin_idle_generation);
            let handle = tokio::spawn(async move {
                ConversationOrchestrator::run_goal_checkin_idle_loop(
                    provider,
                    writer,
                    session,
                    turn_gate,
                    goal_checkin,
                    last_jsonl_uuid,
                    current_cwd,
                    fallback_cwd,
                    running,
                    generation_counter,
                    generation,
                )
                .await;
            });
            *self
                .lifecycle_runtime
                .goal_checkin_idle_task
                .lock()
                .expect("goal checkin idle task") = Some(handle);
            return;
        }

        self.lifecycle_runtime
            .goal_checkin_idle_running
            .store(false, std::sync::atomic::Ordering::Release);
        self.lifecycle_runtime
            .goal_checkin_idle_generation
            .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
        if let Some(handle) = self
            .lifecycle_runtime
            .goal_checkin_idle_task
            .lock()
            .expect("goal checkin idle task")
            .take()
        {
            handle.abort();
        }
    }

    /// Whether the current in-memory session history already contains this raw
    /// top-level JSONL UUID. Structured stdin replay uses this to suppress
    /// duplicate user turns across process restarts / reattach flows.
    pub async fn session_contains_message_uuid(&self, raw_uuid: &str) -> bool {
        let Some(id) = MessageId::parse_prefixed(raw_uuid) else {
            return false;
        };
        let session = self.session.lock().await;
        session.history.iter().any(|message| message.id() == id)
    }

    /// The `bash_output_audience_note` attachment (2.1.238, gate `kpm`
    /// @294267076, emission @294300924), or `None` when the gate says no.
    ///
    /// Unlike the per-turn reminders this one belongs to the message STREAM: it
    /// follows the `tool_result` line for a Bash call whose stdout is longer
    /// than the few lines the user's terminal showed. Gated on the model
    /// capability `bash_output_audience_note` / the
    /// `CLAUDE_CODE_BASH_OUTPUT_AUDIENCE_NOTE` env var, which the port has no
    /// capability table for ⇒ **default OFF**.
    pub(crate) async fn bash_output_audience_note_message(
        &self,
        tool_use_id: &protocol::ToolUseId,
    ) -> Option<ConversationMessage> {
        if !crate::prompt::bash_output_note::is_enabled() {
            return None;
        }
        let tool_name = self.tool_name_for_use_id(tool_use_id).await?;
        let data = self
            .transcript
            .tool_use_results
            .lock()
            .await
            .get(&tool_use_id.to_string())
            .cloned();
        if !crate::prompt::bash_output_note::should_attach(
            &tool_name,
            data.as_ref(),
            self.config.interactive_session,
        ) {
            return None;
        }
        let content = format!(
            "<system-reminder>\n{}\n</system-reminder>",
            crate::prompt::bash_output_note::BASH_OUTPUT_AUDIENCE_NOTE
        );
        Some(ConversationMessage::user_meta(MessageId::new(), content))
    }

    /// Update the thinking policy for the next API request.
    pub fn set_thinking_config(&self, thinking: llm_client::model::thinking::ThinkingConfig) {
        self.api.set_thinking_config(thinking);
    }

    /// Update how the attached output sink presents subsequent thinking blocks.
    pub fn set_thinking_display(&self, mode: Option<&str>) {
        self.output.set_thinking_display(mode);
    }

    /// Update the effort carried by the next API request and by subsequently
    /// persisted assistant transcript rows.
    pub fn set_effort(&self, effort: Option<String>) {
        self.model_runtime
            .current_effort_explicit
            .store(true, std::sync::atomic::Ordering::Release);
        *self
            .model_runtime
            .current_reasoning_selection
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = effort
            .clone()
            .map(|id| traits::ReasoningSelection::Level { id })
            .unwrap_or(traits::ReasoningSelection::Automatic);
        self.apply_effort(effort);
    }

    /// Restore transcript effort only when no launch/control override owns the
    /// live value. Unlike [`Self::set_effort`], inheritance deliberately does
    /// not pin the value, so a later resume can adopt or clear it again.
    pub(crate) fn restore_effort_from_resume(
        &self,
        model: &str,
        provider_id: Option<&str>,
        effort: Option<String>,
    ) {
        if self
            .model_runtime
            .current_effort_explicit
            .load(std::sync::atomic::Ordering::Acquire)
        {
            return;
        }
        let selection = effort
            .clone()
            .map(|id| traits::ReasoningSelection::Level { id })
            .unwrap_or(traits::ReasoningSelection::Automatic);
        let (validated, thinking, provider_effort, legacy_effort) =
            self.reasoning_request_state(model, provider_id, &selection);
        *self
            .model_runtime
            .current_reasoning_selection
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = validated;
        *self
            .model_runtime
            .current_effort
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = legacy_effort;
        self.api.set_thinking_config(thinking);
        self.api.set_effort(provider_effort);
    }

    /// Restore a structured transcript selection without claiming it as an
    /// explicit live override. Older transcripts continue through
    /// `restore_effort_from_resume`; newer rows retain toggles and budgets.
    pub(crate) fn restore_reasoning_selection_from_resume(
        &self,
        model: &str,
        provider_id: Option<&str>,
        selection: traits::ReasoningSelection,
    ) {
        if self
            .model_runtime
            .current_effort_explicit
            .load(std::sync::atomic::Ordering::Acquire)
        {
            return;
        }
        let (validated, thinking, provider_effort, legacy_effort) =
            self.reasoning_request_state(model, provider_id, &selection);
        *self
            .model_runtime
            .current_reasoning_selection
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = validated;
        *self
            .model_runtime
            .current_effort
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = legacy_effort;
        self.api.set_thinking_config(thinking);
        self.api.set_effort(provider_effort);
    }

    fn apply_effort(&self, effort: Option<String>) {
        *self
            .model_runtime
            .current_effort
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = effort.clone();
        self.api.set_effort(effort.map(serde_json::Value::String));
    }

    #[must_use]
    pub fn current_reasoning_selection(&self) -> traits::ReasoningSelection {
        self.model_runtime
            .current_reasoning_selection
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Project the llm-client's request-facing capability registry into the
    /// provider-neutral controls DTO. Keeping this conversion at the
    /// orchestrator seam means the same catalog that validates/encodes a
    /// request also drives the mobile UI; unknown/custom profiles remain
    /// Auto-only because they have no verified adapter contract.
    fn reasoning_spec_for_model(
        &self,
        model: &str,
        provider_id: Option<&str>,
    ) -> traits::ReasoningControlSpec {
        let mut matches = self
            .api
            .list_model_listings()
            .into_iter()
            .filter(|listing| {
                listing.request_model == model
                    && provider_id.is_none_or(|provider| listing.provider_id == provider)
            });
        if let Some(first) = matches.next() {
            if provider_id.is_some() || matches.next().is_none() {
                return first.reasoning;
            }
        }
        // A missing profile is the legacy/builtin route for known Anthropic
        // models. Infer it only from the shared model-capability registry;
        // arbitrary or custom ids remain Auto-only instead of inheriting
        // Anthropic controls by name.
        let inferred_provider = provider_id.or_else(|| {
            traits::model_capabilities::has_capability(
                model,
                traits::model_capabilities::ModelCapability::Effort,
            )
            .then_some("builtin")
        });
        let (protocol, base_url) = match inferred_provider.unwrap_or_default() {
            "anthropic" | "builtin" => (
                llm_client::ProtocolFamily::AnthropicMessages,
                "https://api.anthropic.com",
            ),
            "openai" => (
                llm_client::ProtocolFamily::OpenAiResponses,
                "https://api.openai.com/v1",
            ),
            "openai-chatgpt" => (
                llm_client::ProtocolFamily::OpenAiResponses,
                "https://chatgpt.com/backend-api/codex",
            ),
            "gemini" => (
                llm_client::ProtocolFamily::GeminiGenerateContent,
                "https://generativelanguage.googleapis.com/v1beta",
            ),
            "deepseek" => (
                llm_client::ProtocolFamily::OpenAiChat,
                "https://api.deepseek.com",
            ),
            "kimi" => (
                llm_client::ProtocolFamily::OpenAiChat,
                "https://api.moonshot.cn/v1",
            ),
            "kimi-code" => (
                llm_client::ProtocolFamily::OpenAiChat,
                "https://api.kimi.com/coding/v1",
            ),
            "openrouter" => (
                llm_client::ProtocolFamily::OpenAiChat,
                "https://openrouter.ai/api/v1",
            ),
            _ => return traits::reasoning_control_spec_for_model(model, inferred_provider),
        };

        let raw = llm_client::reasoning_controls::reasoning_control_spec(
            llm_client::reasoning_controls::ReasoningTarget {
                profile_name: inferred_provider,
                protocol: &protocol,
                base_url,
                model,
            },
        );
        let mandatory = raw
            .mandatory_selection
            .as_ref()
            .map(|selection| match selection {
                llm_client::reasoning_controls::ReasoningSelection::Automatic => {
                    traits::ReasoningSelection::Automatic
                }
                llm_client::reasoning_controls::ReasoningSelection::Disabled => {
                    traits::ReasoningSelection::Disabled
                }
                llm_client::reasoning_controls::ReasoningSelection::Enabled => {
                    traits::ReasoningSelection::Enabled
                }
                llm_client::reasoning_controls::ReasoningSelection::Level(id) => {
                    traits::ReasoningSelection::Level { id: id.clone() }
                }
                llm_client::reasoning_controls::ReasoningSelection::TokenBudget(tokens) => {
                    traits::ReasoningSelection::TokenBudget {
                        tokens: u64::from(*tokens),
                    }
                }
            });

        let mut available = Vec::new();
        if let Some(mandatory) = &mandatory {
            available.push(mandatory.clone());
        } else {
            available.push(traits::ReasoningSelection::Automatic);
            if raw.can_disable {
                available.push(traits::ReasoningSelection::Disabled);
            }
            if raw.can_enable {
                available.push(traits::ReasoningSelection::Enabled);
            }
            available.extend(
                raw.levels
                    .iter()
                    .cloned()
                    .map(|id| traits::ReasoningSelection::Level { id }),
            );
        }

        let auto_only = available.len() == 1
            && matches!(
                available.first(),
                Some(traits::ReasoningSelection::Automatic)
            )
            && raw.token_budget.is_none();
        traits::ReasoningControlSpec {
            available,
            selections_persistable: mandatory.is_none(),
            budget_range: raw.token_budget.map(|range| traits::ReasoningBudgetRange {
                min_tokens: range.min,
                max_tokens: range.max,
                supports_dynamic: false,
                supports_disabled: raw.can_disable,
            }),
            provider_default: mandatory.unwrap_or(traits::ReasoningSelection::Automatic),
            forced: raw.mandatory_selection.is_some(),
            modifiable: raw.mandatory_selection.is_none() && !auto_only,
            disabled_reason: if raw.mandatory_selection.is_some() {
                Some("reasoning_required".to_string())
            } else if auto_only {
                Some("reasoning_unavailable".to_string())
            } else {
                None
            },
        }
    }

    fn validate_reasoning_selection(
        &self,
        selection: &traits::ReasoningSelection,
        model: &str,
        provider_id: Option<&str>,
    ) -> traits::ReasoningSelection {
        let spec = self.reasoning_spec_for_model(model, provider_id);
        let supported = match selection {
            traits::ReasoningSelection::Automatic => true,
            traits::ReasoningSelection::TokenBudget { tokens } => {
                spec.budget_range.as_ref().is_some_and(|range| {
                    (*tokens >= u64::from(range.min_tokens)
                        && *tokens <= u64::from(range.max_tokens))
                        || (range.supports_disabled && *tokens == 0)
                })
            }
            other => spec.available.iter().any(|candidate| candidate == other),
        };
        if spec.forced && !spec.modifiable {
            spec.provider_default
        } else if supported {
            selection.clone()
        } else {
            traits::ReasoningSelection::Automatic
        }
    }

    fn reasoning_request_state(
        &self,
        model: &str,
        provider_id: Option<&str>,
        selection: &traits::ReasoningSelection,
    ) -> (
        traits::ReasoningSelection,
        llm_client::model::thinking::ThinkingConfig,
        Option<serde_json::Value>,
        Option<String>,
    ) {
        use llm_client::model::thinking::ThinkingConfig;
        let validated = self.validate_reasoning_selection(selection, model, provider_id);
        let effort_level = |id: &str| Some(serde_json::Value::String(id.to_string()));
        let legacy = |id: &str| Some(id.to_string());
        let provider_id = provider_id.or_else(|| {
            traits::model_capabilities::has_capability(
                model,
                traits::model_capabilities::ModelCapability::Effort,
            )
            .then_some("builtin")
        });
        let provider_id = provider_id.unwrap_or_default();
        let model_lc = model.to_ascii_lowercase();

        match provider_id {
            "anthropic" | "builtin" => match &validated {
                traits::ReasoningSelection::Automatic => {
                    (validated, ThinkingConfig::Automatic, None, None)
                }
                traits::ReasoningSelection::Disabled => {
                    (validated, ThinkingConfig::Disabled, None, None)
                }
                traits::ReasoningSelection::TokenBudget { tokens } => (
                    validated.clone(),
                    ThinkingConfig::Enabled {
                        budget_tokens: (*tokens).try_into().unwrap_or(u32::MAX),
                    },
                    None,
                    None,
                ),
                traits::ReasoningSelection::Level { id } => (
                    validated.clone(),
                    ThinkingConfig::Adaptive,
                    effort_level(id.as_str()),
                    legacy(id.as_str()),
                ),
                traits::ReasoningSelection::Enabled => {
                    (validated, ThinkingConfig::Adaptive, None, None)
                }
            },
            "openai" | "openai-chatgpt" => match &validated {
                traits::ReasoningSelection::Automatic => {
                    (validated, ThinkingConfig::Automatic, None, None)
                }
                traits::ReasoningSelection::Level { id } => (
                    validated.clone(),
                    ThinkingConfig::Adaptive,
                    effort_level(id.as_str()),
                    legacy(id.as_str()),
                ),
                traits::ReasoningSelection::Disabled => {
                    // Responses API uses the explicit `none` effort value to
                    // distinguish a user-off override from provider Auto.
                    (
                        validated,
                        ThinkingConfig::Disabled,
                        effort_level("none"),
                        None,
                    )
                }
                traits::ReasoningSelection::Enabled => {
                    (validated, ThinkingConfig::Adaptive, None, None)
                }
                traits::ReasoningSelection::TokenBudget { .. } => {
                    (validated, ThinkingConfig::Adaptive, None, None)
                }
            },
            "gemini" => {
                if model_lc.starts_with("gemini-3.") {
                    match &validated {
                        traits::ReasoningSelection::Automatic => {
                            (validated, ThinkingConfig::Automatic, None, None)
                        }
                        traits::ReasoningSelection::Level { id } => (
                            validated.clone(),
                            ThinkingConfig::Adaptive,
                            effort_level(id.as_str()),
                            legacy(id.as_str()),
                        ),
                        traits::ReasoningSelection::Disabled => {
                            (validated, ThinkingConfig::Disabled, None, None)
                        }
                        traits::ReasoningSelection::Enabled => {
                            (validated, ThinkingConfig::Adaptive, None, None)
                        }
                        traits::ReasoningSelection::TokenBudget { .. } => {
                            (validated, ThinkingConfig::Adaptive, None, None)
                        }
                    }
                } else {
                    match &validated {
                        traits::ReasoningSelection::Automatic => {
                            (validated, ThinkingConfig::Automatic, None, None)
                        }
                        traits::ReasoningSelection::TokenBudget { tokens } => (
                            validated.clone(),
                            ThinkingConfig::Enabled {
                                budget_tokens: (*tokens).try_into().unwrap_or(u32::MAX),
                            },
                            None,
                            None,
                        ),
                        traits::ReasoningSelection::Disabled => {
                            (validated, ThinkingConfig::Disabled, None, None)
                        }
                        traits::ReasoningSelection::Enabled => {
                            (validated, ThinkingConfig::Adaptive, None, None)
                        }
                        traits::ReasoningSelection::Level { .. } => {
                            (validated, ThinkingConfig::Adaptive, None, None)
                        }
                    }
                }
            }
            "deepseek" => match &validated {
                traits::ReasoningSelection::Automatic => {
                    (validated, ThinkingConfig::Automatic, None, None)
                }
                traits::ReasoningSelection::Disabled => (
                    validated,
                    ThinkingConfig::Disabled,
                    effort_level("off"),
                    None,
                ),
                traits::ReasoningSelection::Level { id } => (
                    validated.clone(),
                    ThinkingConfig::Adaptive,
                    effort_level(id.as_str()),
                    legacy(id.as_str()),
                ),
                traits::ReasoningSelection::Enabled => {
                    (validated, ThinkingConfig::Adaptive, None, None)
                }
                traits::ReasoningSelection::TokenBudget { .. } => {
                    (validated, ThinkingConfig::Adaptive, None, None)
                }
            },
            "kimi" | "kimi-code" => {
                if matches!(model_lc.as_str(), "kimi-k3" | "k3" | "k3-256k") {
                    match &validated {
                        traits::ReasoningSelection::Automatic => {
                            (validated, ThinkingConfig::Automatic, None, None)
                        }
                        traits::ReasoningSelection::Level { id } => (
                            validated.clone(),
                            ThinkingConfig::Adaptive,
                            effort_level(id.as_str()),
                            legacy(id.as_str()),
                        ),
                        traits::ReasoningSelection::Disabled => {
                            (validated, ThinkingConfig::Disabled, None, None)
                        }
                        traits::ReasoningSelection::Enabled => {
                            (validated, ThinkingConfig::Adaptive, None, None)
                        }
                        traits::ReasoningSelection::TokenBudget { .. } => {
                            (validated, ThinkingConfig::Adaptive, None, None)
                        }
                    }
                } else {
                    match &validated {
                        traits::ReasoningSelection::Automatic => {
                            (validated, ThinkingConfig::Automatic, None, None)
                        }
                        traits::ReasoningSelection::Disabled => (
                            validated,
                            ThinkingConfig::Disabled,
                            effort_level("off"),
                            None,
                        ),
                        traits::ReasoningSelection::Enabled => (
                            validated,
                            ThinkingConfig::Adaptive,
                            effort_level("on"),
                            None,
                        ),
                        traits::ReasoningSelection::Level { id } => (
                            validated.clone(),
                            ThinkingConfig::Adaptive,
                            effort_level(id.as_str()),
                            legacy(id.as_str()),
                        ),
                        traits::ReasoningSelection::TokenBudget { tokens } => (
                            validated.clone(),
                            ThinkingConfig::Enabled {
                                budget_tokens: (*tokens).try_into().unwrap_or(u32::MAX),
                            },
                            None,
                            None,
                        ),
                    }
                }
            }
            _ => match &validated {
                traits::ReasoningSelection::Automatic => {
                    (validated, ThinkingConfig::Automatic, None, None)
                }
                _ => (validated, ThinkingConfig::Adaptive, None, None),
            },
        }
    }

    pub fn set_reasoning_selection_for_model(
        &self,
        model: &str,
        provider_id: Option<&str>,
        selection: traits::ReasoningSelection,
    ) -> traits::ReasoningSelection {
        self.model_runtime
            .current_effort_explicit
            .store(true, std::sync::atomic::Ordering::Release);
        let (validated, thinking, effort, legacy_effort) =
            self.reasoning_request_state(model, provider_id, &selection);
        *self
            .model_runtime
            .current_reasoning_selection
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = validated.clone();
        *self
            .model_runtime
            .current_effort
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = legacy_effort.clone();
        self.api.set_thinking_config(thinking);
        self.api.set_effort(effort);
        validated
    }

    /// Seed a persisted new-session default without marking it as an explicit
    /// session override.  This lets resume restore a transcript's selection
    /// while still applying the user's default to a genuinely new session.
    pub fn initialize_reasoning_selection_for_model(
        &self,
        model: &str,
        provider_id: Option<&str>,
        selection: traits::ReasoningSelection,
    ) -> traits::ReasoningSelection {
        let (validated, thinking, effort, legacy_effort) =
            self.reasoning_request_state(model, provider_id, &selection);
        self.model_runtime
            .current_effort_explicit
            .store(false, std::sync::atomic::Ordering::Release);
        *self
            .model_runtime
            .current_reasoning_selection
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = validated.clone();
        *self
            .model_runtime
            .current_effort
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = legacy_effort;
        self.api.set_thinking_config(thinking);
        self.api.set_effort(effort);
        validated
    }

    #[must_use]
    pub fn conversation_controls_for_model(
        &self,
        model: &str,
        provider_id: Option<&str>,
    ) -> traits::ConversationControls {
        let reasoning_spec = self.reasoning_spec_for_model(model, provider_id);
        let requested_reasoning = self.current_reasoning_selection();
        let effective_reasoning =
            self.validate_reasoning_selection(&requested_reasoning, model, provider_id);
        let requested_permission = self
            .permission_mode()
            .unwrap_or_else(|| "default".to_string());
        let effective_permission = requested_permission.clone();
        let permission_modes = [
            "default",
            "acceptEdits",
            "plan",
            "auto",
            "dontAsk",
            "bypassPermissions",
        ]
        .into_iter()
        .map(|mode| {
            let unavailable =
                mode == "bypassPermissions" && !self.perms.can_request_bypass_permissions();
            traits::PermissionModeAvailability {
                mode: mode.to_string(),
                available: !unavailable,
                disabled_reason: unavailable.then(|| "not_yet_available".to_string()),
            }
        })
        .collect();
        traits::ConversationControls {
            model_reference: traits::qualified_model_ref(model, provider_id),
            permission: traits::PermissionControlState {
                requested: requested_permission,
                effective: effective_permission,
                modes: permission_modes,
            },
            requested_reasoning_selection: requested_reasoning,
            effective_reasoning_selection: effective_reasoning,
            reasoning_spec,
        }
    }

    /// Apply a LIVE session permission-mode change (stream-json
    /// `set_permission_mode` control_request). Delegates to the gate's
    /// [`traits::PermissionGate::set_permission_mode`]; only the enforcing
    /// `PolicyPermissionGate` actually mutates (other gates no-op). Returns the
    /// gate's validation error string on an invalid / disallowed mode.
    pub async fn set_permission_mode(&self, mode: &str) -> Result<(), String> {
        self.perms.set_permission_mode(mode).await
    }

    /// Apply or clear the LIVE per-MCP-server permission-mode override
    /// (stream-json `set_mcp_permission_mode_override` control_request).
    pub async fn set_mcp_permission_mode_override(
        &self,
        server_name: &str,
        mode: Option<&str>,
    ) -> Result<(), String> {
        self.perms
            .set_mcp_permission_mode_override(server_name, mode)
            .await
    }

    /// Return the enforcing gate's live permission-mode wire id.
    #[must_use]
    pub fn permission_mode(&self) -> Option<String> {
        self.perms.permission_mode()
    }
}
