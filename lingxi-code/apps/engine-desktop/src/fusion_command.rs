//! Desktop `/fusion` slash handler and completion sink.
//!
//! Registered at composition time (not in `BUILTIN_COMMAND_NAMES`). `/fusion`
//! is explicit per-run and works even when `fusion.enabled` is false.

use async_trait::async_trait;
use command_api::model::{BuiltinCommandHandler, CommandResult};
use command_api::parser::ParsedSlashCommand;
use command_core::{fusion_request_from_slash, parse_fusion_slash};
use platform_api::{
    FusionCompletionSink, FusionExecutor, FusionResult, FusionStatus, OrchestratorHandle,
};
use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;
use tasks::{TaskSpawnInput, TaskType};
use tokio::sync::Mutex;

const FUSION_ARGUMENT_HINT: &str = "[--quality|--fast] [--same-provider|--cross-provider] PROMPT";

/// Idempotent parent-history sink for Fusion results.
pub struct DesktopFusionCompletionSink {
    handle: Arc<dyn OrchestratorHandle>,
    published: Mutex<HashSet<(String, String)>>,
}

impl DesktopFusionCompletionSink {
    /// Bind to the live orchestrator.
    #[must_use]
    pub fn new(handle: Arc<dyn OrchestratorHandle>) -> Self {
        Self {
            handle,
            published: Mutex::new(HashSet::new()),
        }
    }
}

#[async_trait]
impl FusionCompletionSink for DesktopFusionCompletionSink {
    async fn publish(&self, conversation_id: &str, result: &FusionResult) {
        let key = (conversation_id.to_string(), result.run_id.clone());
        let mut seen = self.published.lock().await;
        if seen.contains(&key) {
            return;
        }
        let xml = tasks::fusion_result_xml(result);
        if let Err(err) = self
            .handle
            .append_meta_user_message_to_session(conversation_id, &xml)
            .await
        {
            tracing::warn!(error = %err, "fusion completion sink failed; task status unchanged");
            return;
        }
        // F006: before this, a finished background run's only trace was the
        // meta-message row above — nothing live ever told the user it had
        // finished. Fire a best-effort UI notice (whichever session/client is
        // currently connected); the durable record above is the source of
        // truth regardless of whether this reaches anyone live.
        self.handle
            .emit_background_system_notice(&fusion_completion_notice(result))
            .await;
        seen.insert(key);
    }
}

/// Short, content-free notice text for [`DesktopFusionCompletionSink::publish`]
/// — no prompt, no final text, no model/provider names, just the status and a
/// pointer at the durable record.
fn fusion_completion_notice(result: &FusionResult) -> String {
    match result.status {
        FusionStatus::Completed => {
            "Fusion run finished — see the result appended to this conversation.".to_string()
        }
        FusionStatus::NeedsParent => {
            "Fusion run finished — it needs your judgment; see the summary appended to this conversation.".to_string()
        }
    }
}

/// Fill-later wrapper so the task handler can be registered before the
/// orchestrator exists.
pub struct DeferredFusionCompletionSink {
    state: Mutex<DeferredFusionCompletionState>,
}

#[derive(Default)]
struct DeferredFusionCompletionState {
    inner: Option<Arc<dyn FusionCompletionSink>>,
    pending_keys: HashSet<(String, String)>,
    pending: Vec<(String, FusionResult)>,
}

impl DeferredFusionCompletionSink {
    /// Empty sink.
    #[must_use]
    pub fn new() -> Self {
        Self {
            state: Mutex::new(DeferredFusionCompletionState::default()),
        }
    }

    /// Bind the live sink and flush anything queued before it existed.
    pub async fn bind(&self, sink: Arc<dyn FusionCompletionSink>) {
        let (sink, pending) = {
            let mut state = self.state.lock().await;
            let sink = state.inner.get_or_insert_with(|| sink.clone()).clone();
            state.pending_keys.clear();
            (sink, std::mem::take(&mut state.pending))
        };
        for (conversation_id, result) in pending {
            sink.publish(&conversation_id, &result).await;
        }
    }
}

impl Default for DeferredFusionCompletionSink {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl FusionCompletionSink for DeferredFusionCompletionSink {
    async fn publish(&self, conversation_id: &str, result: &FusionResult) {
        let inner = {
            let mut state = self.state.lock().await;
            if let Some(sink) = state.inner.clone() {
                Some(sink)
            } else {
                let key = (conversation_id.to_string(), result.run_id.clone());
                if state.pending_keys.insert(key) {
                    state
                        .pending
                        .push((conversation_id.to_string(), result.clone()));
                }
                None
            }
        };
        if let Some(sink) = inner {
            sink.publish(conversation_id, result).await;
        }
    }
}

/// Build the task-row description: `Fusion {preset} {scope}: <first line,
/// first 80 chars>` instead of the raw, unbounded prompt (G012) — the prompt
/// otherwise duplicates verbatim into `TaskCreated` hook payloads, `list()`,
/// notifications, and the Electron detail pane / RuntimeCenter row title.
fn fusion_task_description(preset: &str, scope: &str, prompt: &str) -> String {
    let first_line = prompt.lines().next().unwrap_or("");
    const MAX_CHARS: usize = 80;
    let mut truncated: String = first_line.chars().take(MAX_CHARS).collect();
    if first_line.chars().count() > MAX_CHARS {
        truncated.push('…');
    }
    format!("Fusion {preset} {scope}: {truncated}")
}

/// Desktop `/fusion` command.
pub struct DesktopFusionCommandHandler {
    registry: Arc<tasks::registry::TaskRegistry>,
    executor: Arc<dyn FusionExecutor>,
    handle: Arc<dyn OrchestratorHandle>,
    parent_profiles: BTreeMap<String, String>,
}

impl DesktopFusionCommandHandler {
    /// Construct.
    #[must_use]
    pub fn new(
        registry: Arc<tasks::registry::TaskRegistry>,
        executor: Arc<dyn FusionExecutor>,
        handle: Arc<dyn OrchestratorHandle>,
        parent_profiles: BTreeMap<String, String>,
    ) -> Self {
        Self {
            registry,
            executor,
            handle,
            parent_profiles,
        }
    }
}

#[async_trait]
impl BuiltinCommandHandler for DesktopFusionCommandHandler {
    async fn handle(&self, args: &ParsedSlashCommand) -> CommandResult {
        let parsed = match parse_fusion_slash(args) {
            Ok(parsed) => parsed,
            Err(msg) => {
                return CommandResult::Done { display: Some(msg) };
            }
        };
        let snapshot = self.handle.get_status_snapshot().await;
        let conversation_id = self.handle.current_session_id().await.to_string();
        let surface = self.executor.agent_surface();
        let preset = parsed.preset.unwrap_or(surface.default_preset);
        let cross = parsed
            .cross_provider
            .unwrap_or(surface.slash_cross_provider_default);
        let request = fusion_request_from_slash(
            parsed,
            snapshot.model_profile.clone().unwrap_or_else(|| {
                self.parent_profiles
                    .get(&snapshot.model)
                    .cloned()
                    .unwrap_or_default()
            }),
            snapshot.model.clone(),
            conversation_id.clone(),
            surface.slash_cross_provider_default,
            surface.default_preset,
            surface.default_partial_ok,
        );
        let preset_word = match preset {
            platform_api::FusionPreset::Quality => "quality",
            platform_api::FusionPreset::Fast => "fast",
        };
        let scope_word = if cross {
            "cross-provider"
        } else {
            "same-provider"
        };
        let description = fusion_task_description(preset_word, scope_word, &request.prompt);
        match self
            .registry
            .spawn(
                TaskType::LocalFusion,
                TaskSpawnInput::LocalFusion {
                    request,
                    conversation_id,
                },
                description,
            )
            .await
        {
            Ok(task_id) => CommandResult::Done {
                display: Some(format!("{task_id}  {preset_word}  {scope_word}")),
            },
            Err(err) => CommandResult::Done {
                display: Some(format!("fusion failed to start: {err}")),
            },
        }
    }

    fn name(&self) -> &str {
        "fusion"
    }

    fn description(&self) -> &str {
        "Run a multi-model Fusion deliberation (may add cost and cross-provider egress)"
    }

    fn argument_hint(&self) -> Option<&str> {
        Some(FUSION_ARGUMENT_HINT)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use platform_api::{
        FusionDecision, FusionNeedsParentReason, FusionStatus, FusionTiming, FusionUsage,
    };
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn dummy_result(run_id: &str) -> FusionResult {
        FusionResult {
            schema_version: 1,
            run_id: run_id.into(),
            status: FusionStatus::NeedsParent,
            decision: FusionDecision::NeedsParent {
                reason: FusionNeedsParentReason::LowConfidence,
            },
            final_text: "needs parent".into(),
            analysis: None,
            panels: vec![],
            usage: FusionUsage::default(),
            timing: FusionTiming::default(),
            egress_profiles: vec![],
        }
    }

    struct CountingSink(AtomicUsize);

    #[async_trait]
    impl FusionCompletionSink for CountingSink {
        async fn publish(&self, _conversation_id: &str, _result: &FusionResult) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    // ---- G012: `description` was `request.prompt.clone()` verbatim — an
    // unbounded, un-labeled string that leaked the full prompt into
    // `TaskCreated` hook payloads, `list()`, notifications, and RuntimeCenter
    // (which has no label for `local_fusion` and shows the raw string). ----

    #[test]
    fn description_is_labeled_with_preset_scope_and_truncated_first_line() {
        let desc = fusion_task_description(
            "quality",
            "cross-provider",
            "review this diff for races\nand also check the locking",
        );
        assert_eq!(
            desc,
            "Fusion quality cross-provider: review this diff for races"
        );
    }

    #[test]
    fn description_truncates_a_long_first_line_at_eighty_chars() {
        let long_line = "x".repeat(200);
        let desc = fusion_task_description("fast", "same-provider", &long_line);
        assert_eq!(desc, format!("Fusion fast same-provider: {}…", "x".repeat(80)));
    }

    #[test]
    fn description_does_not_truncate_a_short_prompt() {
        let desc = fusion_task_description("fast", "same-provider", "short");
        assert_eq!(desc, "Fusion fast same-provider: short");
    }

    #[tokio::test]
    async fn deferred_sink_replays_prebind_result_once_after_bind() {
        let deferred = DeferredFusionCompletionSink::new();
        let result = dummy_result("fu_1");
        deferred.publish("c", &result).await;
        deferred.publish("c", &result).await;
        let counter = Arc::new(CountingSink(AtomicUsize::new(0)));
        deferred.bind(counter.clone()).await;
        deferred.publish("c", &result).await;
        assert_eq!(counter.0.load(Ordering::SeqCst), 2);
    }

    // ---- F006/WP6 item 3: before this, a finished background run's only
    // trace was the `<fusion-result>` meta row appended to history — nothing
    // live ever told the connected client the run had finished. ----------

    #[tokio::test]
    async fn publish_emits_exactly_one_content_free_background_notice() {
        let mock = Arc::new(orchestrator::test_support::MockOrchestratorHandle::new());
        let session_id = mock.current_session_id().await.to_string();
        let sink = DesktopFusionCompletionSink::new(mock.clone());
        let mut result = dummy_result("fu_1");
        result.status = FusionStatus::Completed;
        result.final_text = "the secret final answer".into();

        sink.publish(&session_id, &result).await;

        let notices = mock.background_notices();
        assert_eq!(notices.len(), 1, "exactly one notice: {notices:?}");
        assert!(
            notices[0].contains("Fusion run finished"),
            "got: {notices:?}"
        );
        // Content-free: no prompt/final-text leak into the live notice — the
        // durable `<fusion-result>` meta row carries that, not this notice.
        assert!(
            !notices[0].contains(&result.final_text),
            "notice must not leak final_text: {notices:?}"
        );

        // Idempotent publish (same run id) must not double-notify.
        sink.publish(&session_id, &result).await;
        assert_eq!(mock.background_notices().len(), 1);
    }

    #[tokio::test]
    async fn publish_notice_differs_for_needs_parent_vs_completed() {
        let mock = Arc::new(orchestrator::test_support::MockOrchestratorHandle::new());
        let session_id = mock.current_session_id().await.to_string();
        let sink = DesktopFusionCompletionSink::new(mock.clone());
        let mut result = dummy_result("fu_needs_parent");
        result.status = FusionStatus::NeedsParent;

        sink.publish(&session_id, &result).await;

        let notices = mock.background_notices();
        assert_eq!(notices.len(), 1);
        assert!(
            notices[0].contains("needs your judgment"),
            "got: {notices:?}"
        );
    }
}
