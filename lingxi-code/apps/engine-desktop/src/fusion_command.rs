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
            // [Finding 13] Before this, the append failure left the run with
            // NO trace at all: `finalize_fusion_outcome` still calls
            // `mark_fusion_result_published` unconditionally after
            // `publish()` returns, so the durable `<fusion-result>` row is
            // missing here AND the one live signal (this F006 notice) was
            // skipped too — the user got nothing until the next turn's task
            // notification. Fire the notice anyway, worded so it does not
            // point at a durable record that does not exist.
            self.handle
                .emit_background_system_notice(&fusion_completion_notice_unrecorded())
                .await;
            return;
        }
        // F006: before this, a finished background run's only trace was the
        // meta-message row above — nothing live ever told the user it had
        // finished. Fire a best-effort UI notice (whichever session/client is
        // currently connected); the durable record above is the source of
        // truth regardless of whether this reaches anyone live.
        //
        // [Round-5 finding 13] `emit_background_system_notice` has NO session
        // argument — it routes to whatever session is connected RIGHT NOW
        // (`handle_impl.rs`: `self.output.emit_system_notice(body, false)`).
        // The append above, by contrast, is session-TARGETED and returns
        // `Ok(())` for a target that is no longer current as long as the
        // durable write landed (it only pushes into live history inside its
        // `current == target` branch). So `Ok` does NOT mean "this
        // conversation": after a `/clear` or a hot-resume mid-run the result
        // is persisted to the OLD session's JSONL while this notice appears
        // in the NEW one. Ask which session is live and pick copy that is
        // true of the session the user is actually looking at.
        let appended_to_current =
            self.handle.current_session_id().await.to_string() == conversation_id;
        let body = if appended_to_current {
            fusion_completion_notice(result)
        } else {
            fusion_completion_notice_other_session(result, conversation_id)
        };
        self.handle.emit_background_system_notice(&body).await;
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

/// [Round-5 finding 13] Notice text for the append-SUCCEEDED-ELSEWHERE path:
/// the durable `<fusion-result>` row exists, but it is in the session the run
/// was started in, not the one this notice is about to be rendered into.
/// Saying "appended to this conversation" there points the user at a
/// conversation that contains no fusion result anywhere and gives them no
/// pointer to where the (already paid-for) result actually went.
///
/// [Rework r1] The id is truncated to 8 characters OF ITS UUID BODY, not of
/// the string as it arrives. Every production caller feeds this
/// `SessionId::to_string()` (`fusion_command.rs`'s own handler and
/// `tasks/src/registry.rs`'s `LocalFusionTaskState.conversation_id`, both from
/// `current_session_id().await.to_string()`), and `protocol`'s
/// `id_newtype!(SessionId, "sess")` renders that as `"sess:<uuid>"` — so
/// truncating the raw string spent 5 of the 8 characters on the constant
/// prefix and printed `"sess:111"`, three hex digits of the real id. The
/// pointer has to be matchable against `/resume`'s listing, which keys rows by
/// the JSONL file stem, i.e. the BARE uuid (`session/src/jsonl/loader.rs`:
/// `let sid = stem.as_str()`), so the prefix is stripped rather than shown.
/// Eight uuid characters is short enough not to dominate a one-line notice.
fn fusion_completion_notice_other_session(result: &FusionResult, conversation_id: &str) -> String {
    // `parse_prefixed` accepts both the prefixed display form and a bare uuid,
    // so this stays correct if a caller ever hands over an unprefixed id; a
    // string that is neither falls back to being truncated as-is.
    let body = protocol::SessionId::parse_prefixed(conversation_id)
        .map_or_else(|| conversation_id.to_string(), |id| id.as_uuid().to_string());
    let short: String = body.chars().take(8).collect();
    let what = match result.status {
        FusionStatus::Completed => "its result",
        FusionStatus::NeedsParent => "its summary (it needs your judgment)",
    };
    format!(
        "Fusion run finished — {what} was saved to the conversation it was started in \
(session {short}…), not this one. Resume that session, or check the task list, to see it."
    )
}

/// [Finding 13] Notice text for the append-failure path: unlike
/// [`fusion_completion_notice`] this must NOT claim the result was appended
/// to the conversation — the whole reason it fires is that the append
/// failed, so pointing at a durable record that does not exist would be
/// actively misleading.
fn fusion_completion_notice_unrecorded() -> String {
    "Fusion run finished, but its result could not be recorded in this conversation \
     — it may have been cleared or moved. Check the task list for the result."
        .to_string()
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
/// notifications, and the Electron detail pane / `RuntimeCenter` row title.
fn fusion_task_description(preset: &str, scope: &str, prompt: &str) -> String {
    const MAX_CHARS: usize = 80;
    let first_line = prompt.lines().next().unwrap_or("");
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

    /// [Round-5 finding 13] The PRODUCTION `append_meta_user_message_to_session`
    /// (`orchestrator/src/handle_impl.rs`) returns `Ok(())` for a target
    /// session that is no longer current as long as the durable write landed
    /// — it only touches live history inside its `current == target` branch,
    /// and only errors when nothing was persisted. `MockOrchestratorHandle`
    /// uses the platform-api TRAIT DEFAULT instead, which fails closed on a
    /// session mismatch, so every existing test drives the opposite of
    /// production on exactly the path this finding is about. This double
    /// delegates everything to the mock except that one method, whose
    /// production semantics it reproduces.
    struct AppendsToAnySessionHandle {
        inner: Arc<orchestrator::test_support::MockOrchestratorHandle>,
        appended_to: std::sync::Mutex<Vec<String>>,
    }

    #[async_trait]
    impl OrchestratorHandle for AppendsToAnySessionHandle {
        async fn append_meta_user_message_to_session(
            &self,
            session_id: &str,
            _text: &str,
        ) -> Result<(), platform_api::HandleError> {
            self.appended_to.lock().unwrap().push(session_id.to_string());
            Ok(())
        }
        async fn emit_background_system_notice(&self, body: &str) {
            self.inner.emit_background_system_notice(body).await;
        }
        async fn current_session_id(&self) -> protocol::SessionId {
            self.inner.current_session_id().await
        }
        async fn clear_session(&self) -> Result<(), platform_api::HandleError> {
            self.inner.clear_session().await
        }
        async fn force_compact(
            &self,
        ) -> Result<platform_api::CompactionSummary, platform_api::HandleError> {
            self.inner.force_compact().await
        }
        async fn snapshot_cost(&self) -> platform_api::CostSnapshot {
            self.inner.snapshot_cost().await
        }
        async fn switch_model(
            &self,
            model: &str,
            profile: Option<&str>,
        ) -> Result<(), platform_api::HandleError> {
            self.inner.switch_model(model, profile).await
        }
        async fn request_exit(&self) {
            self.inner.request_exit().await;
        }
        async fn current_should_exit(&self) -> bool {
            self.inner.current_should_exit().await
        }
        async fn open_memory_editor(
            &self,
        ) -> Result<platform_api::MemoryEditorOutcome, platform_api::HandleError> {
            self.inner.open_memory_editor().await
        }
        async fn list_mcp_servers(&self) -> Vec<platform_api::McpServerInfo> {
            self.inner.list_mcp_servers().await
        }
        async fn list_skills(&self) -> Vec<platform_api::SkillInfo> {
            self.inner.list_skills().await
        }
        async fn list_hooks(&self) -> Vec<platform_api::HookInfo> {
            self.inner.list_hooks().await
        }
        async fn list_agents(&self) -> Vec<platform_api::AgentInfo> {
            self.inner.list_agents().await
        }
        async fn run_doctor_checks(&self) -> platform_api::DoctorReport {
            self.inner.run_doctor_checks().await
        }
        async fn get_status_snapshot(&self) -> platform_api::StatusSnapshot {
            self.inner.get_status_snapshot().await
        }
        async fn edit_config_file(
            &self,
        ) -> Result<platform_api::MemoryEditorOutcome, platform_api::HandleError> {
            self.inner.edit_config_file().await
        }
        async fn edit_permissions_file(
            &self,
        ) -> Result<platform_api::MemoryEditorOutcome, platform_api::HandleError> {
            self.inner.edit_permissions_file().await
        }
        async fn list_available_models(&self) -> Vec<String> {
            self.inner.list_available_models().await
        }
    }

    /// [Round-5 finding 13] `/fusion` started in session A, `/clear` before it
    /// finishes, result lands in A's durable transcript while the user is
    /// looking at session B. The append SUCCEEDS, so the
    /// `could not be recorded` copy never fires; the success copy must not
    /// claim the result is in "this conversation", which contains no fusion
    /// result at all.
    #[tokio::test]
    async fn publish_does_not_claim_this_conversation_when_the_append_landed_elsewhere() {
        let mock = Arc::new(orchestrator::test_support::MockOrchestratorHandle::new());
        let current = mock.current_session_id().await.to_string();
        // [Rework r1] Production NEVER hands this sink a bare uuid: both
        // producers feed it `current_session_id().await.to_string()`
        // (fusion_command.rs:256, tasks/src/registry.rs:2342), and
        // `SessionId`'s `Display` (protocol/src/ids.rs, `id_newtype!(SessionId,
        // "sess")`) renders `"sess:<uuid>"`. A bare-uuid fixture here silently
        // hid a truncation that prints `"sess:111"` in production.
        let started_in = "sess:11112222-3333-4444-5555-666677778888";
        assert_eq!(
            started_in,
            protocol::SessionId::parse_prefixed(started_in)
                .expect("fixture must parse as a real SessionId")
                .to_string(),
            "sanity: the fixture must be exactly what `SessionId::to_string()` produces"
        );
        assert_ne!(
            current, started_in,
            "sanity: the run's session must differ from the connected one"
        );
        let handle = Arc::new(AppendsToAnySessionHandle {
            inner: mock.clone(),
            appended_to: std::sync::Mutex::new(Vec::new()),
        });
        let sink = DesktopFusionCompletionSink::new(handle.clone());
        let mut result = dummy_result("fu_cleared");
        result.status = FusionStatus::Completed;

        sink.publish(started_in, &result).await;

        assert_eq!(
            handle.appended_to.lock().unwrap().as_slice(),
            [started_in.to_string()],
            "sanity: the durable append targeted the ORIGINATING session and succeeded"
        );
        let notices = mock.background_notices();
        assert_eq!(notices.len(), 1, "exactly one notice: {notices:?}");
        assert!(
            !notices[0].contains("appended to this conversation"),
            "the result is NOT in this conversation — that copy is false here: {notices:?}"
        );
        assert!(
            notices[0].contains("saved to the conversation it was started in"),
            "the notice must say where the result actually went: {notices:?}"
        );
        assert!(
            notices[0].contains("11112222"),
            "the notice must name the originating session so the user can find it: {notices:?}"
        );
        // The pointer must be usable against `/resume`'s listing, which keys
        // rows by the JSONL file stem — the BARE uuid (`session/src/jsonl/
        // loader.rs`: `let sid = stem.as_str()`). Printing the `sess:` prefix
        // inside an 8-character budget spends 5 of them on a constant and
        // leaves 3 hex digits of the real id.
        assert!(
            !notices[0].contains("sess:"),
            "the id prefix must be stripped, not truncated into: {notices:?}"
        );
        assert!(
            !notices[0].contains("could not be recorded"),
            "the append SUCCEEDED — the unrecorded copy would be wrong too: {notices:?}"
        );
    }

    /// The other half of the same branch: when the run's session IS the
    /// connected one, the original F006 copy must be unchanged.
    #[tokio::test]
    async fn publish_still_says_this_conversation_when_the_append_landed_here() {
        let mock = Arc::new(orchestrator::test_support::MockOrchestratorHandle::new());
        let current = mock.current_session_id().await.to_string();
        let handle = Arc::new(AppendsToAnySessionHandle {
            inner: mock.clone(),
            appended_to: std::sync::Mutex::new(Vec::new()),
        });
        let sink = DesktopFusionCompletionSink::new(handle.clone());
        let mut result = dummy_result("fu_same_session");
        result.status = FusionStatus::Completed;

        sink.publish(&current, &result).await;

        let notices = mock.background_notices();
        assert_eq!(notices.len(), 1, "exactly one notice: {notices:?}");
        assert!(
            notices[0].contains("see the result appended to this conversation"),
            "got: {notices:?}"
        );
    }

    // ---- [Finding 13]: a durable-append failure (e.g. the target session
    // is no longer current — the default `append_meta_user_message_to_session`
    // fails exactly this way when the id it is given no longer matches
    // `current_session_id`, which is what a `/clear` mid-run or a JSONL
    // write error looks like from this sink's point of view) used to leave
    // the run with NO trace at all: the F006 notice sat after the early
    // `return`, so it never fired on the one path where a live signal is
    // the user's only timely trace. ----

    #[tokio::test]
    async fn publish_still_emits_a_notice_when_the_durable_append_fails() {
        let mock = Arc::new(orchestrator::test_support::MockOrchestratorHandle::new());
        let sink = DesktopFusionCompletionSink::new(mock.clone());
        let mut result = dummy_result("fu_append_fails");
        result.status = FusionStatus::Completed;

        // A conversation id that does NOT match the mock's current session id
        // drives the trait's default `append_meta_user_message_to_session`
        // straight into its `Err(ActionFailed(...))` arm — the same shape a
        // real desktop handle returns once the target session is no longer
        // current (e.g. after `/clear`) or the JSONL append itself fails.
        sink.publish("some-other-session-that-is-not-current", &result)
            .await;

        let notices = mock.background_notices();
        assert_eq!(
            notices.len(),
            1,
            "an append failure must still surface a live notice, not silence: {notices:?}"
        );
        assert!(
            notices[0].contains("could not be recorded"),
            "the failure notice must not claim the result was appended to \
             this conversation (it was not): {notices:?}"
        );
    }
}
