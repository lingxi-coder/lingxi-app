//! 本地应用的三段 LLM 调用：出题 / 出方案 / 写码。
//!
//! 每段的产物一律先过校验器再返回——LLM 的输出是提议，校验器的判定
//! 才是事实。模型经 [`LocalAppsModel`] trait 注入，使三段逻辑能脱离真实
//! `ApiService` 单测。
//!
//! 失败一律 fail-closed：模版已经删除，系统里不存在静默降级路径。
//! `AppError::LlmUnavailable` 用于模型够不到（离线/鉴权失败/超时）；
//! `AppError::LlmOutputRejected` 用于模型答了但答案不合格（形状不对、
//! 超过校验器或闸门的上限）。两者在客户端渲染不同文案、提供不同操作，
//! 绝不能混用。

use crate::local_apps_sources::{screen_writes, FileWrite};
use async_trait::async_trait;
use futures_util::StreamExt;
use llm_client::stream_accumulator::accumulate_stream_salvaging;
use llm_client::{ApiService, ContentBlock};
use local_apps::questionnaire::{
    normalize_plan, validate_plan, validate_questionnaire, AppDesignStep, AppPlan,
    MAX_COLLECTIONS, MAX_DOMAINS, MAX_FIELDS_PER_STEP, MAX_OPTIONS, MAX_STEPS,
};
use local_apps::{AppError, DesignValue};
use protocol::{ConversationMessage, MessageId};
use std::collections::BTreeMap;
use std::sync::{Arc, RwLock};

const AUTHOR_PROMPT: &str = include_str!("../assets/prompts/author_questionnaire.md");
const PLAN_PROMPT: &str = include_str!("../assets/prompts/plan.md");
const SOURCES_PROMPT: &str = include_str!("../assets/prompts/generate_sources.md");

// Forced-tool names. Hoisted to constants (rather than inline string literals
// at each call site) so the name a stage ASKS for and the name
// `extract_single_tool_call` REQUIRES cannot drift apart — a rename that
// updated only one of the two spots would make every response of that stage
// fail as "the model did not call the required tool".
const TOOL_QUESTIONNAIRE: &str = "emit_questionnaire";
const TOOL_PLAN: &str = "emit_plan";
const TOOL_SOURCES: &str = "emit_sources";

/// What a live delta is, for the client that renders it.
///
/// The two are rendered differently (thinking is collapsed/greyed, text is
/// the answer), so the kind travels with the chunk rather than being guessed
/// downstream from its content.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GenerationDeltaKind {
    /// Extended-thinking output — the model reasoning about the task.
    Thinking,
    /// Assistant text.
    Text,
}

/// Where a structured call's live output goes while it is still running.
///
/// The three local-app stages take tens of seconds each, most of it inside one
/// model call that used to be a black box: the client could only show a
/// spinner. Implementations receive chunks AS THEY ARRIVE.
///
/// `on_delta` is synchronous and MUST NOT block — it runs inline on the stream
/// consumer. An implementation that cannot keep up must drop chunks; stalling
/// here stalls the model call itself.
pub trait GenerationDeltaSink: Send + Sync {
    /// One chunk of live output. Chunks are fragments, not whole lines or
    /// tokens — the receiver concatenates.
    fn on_delta(&self, kind: GenerationDeltaKind, chunk: &str);
}

/// A single structured model call. The implementation owns auth, routing,
/// retry and timeout — the three call sites below only see a proposal in,
/// a validated JSON value or an error out.
///
/// 一次结构化模型调用。实现负责鉴权、路由、重试与超时；三段调用只看到
/// "提议进、经校验的 JSON 值或错误出"。
#[async_trait]
pub trait LocalAppsModel: Send + Sync {
    /// Force a tool call named `tool_name`, whose input must match `schema`
    /// (a hint to the model — the real gate is the caller's validator), and
    /// return the tool call's `input` value.
    ///
    /// `deltas`, when present, receives the model's output as it streams. It is
    /// observation only: the value returned is identical with or without it.
    async fn structured(
        &self,
        system: &str,
        user: String,
        tool_name: &str,
        schema: serde_json::Value,
        deltas: Option<Arc<dyn GenerationDeltaSink>>,
    ) -> Result<serde_json::Value, AppError>;

    /// Update the default model/profile future `structured` calls route
    /// through — `ClientCommand::SetModel` calls this so the three local-app
    /// LLM stages follow a `/model` switch instead of staying pinned to
    /// whatever was live at engine build time. Default is a no-op: only
    /// [`ApiServiceModel`] (the production implementation) has a live
    /// selection to update; test doubles ignore it.
    fn set_model(&self, _model: String, _profile: Option<String>) {}
}

/// Real [`LocalAppsModel`] over the shared `ApiService` — a tool call via
/// [`ApiService::messages_create_side_query`] (`llm-client/src/service.rs:2672`),
/// forced with `tool_choice` wherever the provider accepts one.
///
/// This is deliberately NOT the shape the repo's other side queries use, and an
/// earlier version of this comment wrongly claimed otherwise. The only two
/// `SideQueryRequest` callers — `tools/web/src/web_fetch.rs` and
/// `memory/src/selector.rs` — both send `tools: vec![]` + `tool_choice: None`
/// and recover structure by parsing JSON out of the reply TEXT
/// (`sidequery::decode_response`). That shape is portable but unenforced: it
/// asks for a schema and hopes. The forced-tool-call shape — what
/// `agent/src/runner.rs:834` uses for subagent structured output — makes a
/// schema-shaped answer a wire-level guarantee instead. Local apps want the
/// guarantee, because a malformed plan here is not a bad answer, it is a wedged
/// app. So `structured` keeps the guarantee where it exists and degrades to the
/// portable shape only where the provider refuses it.
///
/// `max_tokens` is not part of the [`LocalAppsModel::structured`] signature and
/// no longer part of this module at all: all three stages send `None` and take
/// the model's own output ceiling. See [`ApiServiceModel::send_side_query`] for
/// why a per-stage figure was the wrong shape.
pub struct ApiServiceModel {
    service: Arc<ApiService>,
    /// `(model, profile)` behind ONE `RwLock`, NOT two independent locks:
    /// `ClientCommand::SetModel` updates this in place (via
    /// [`LocalAppsModel::set_model`]) so the three local-app LLM stages
    /// follow a live `/model` switch instead of staying pinned to whatever
    /// `default_model_id`/`default_model_profile` were at engine build time
    /// — the same class of "silently stale after the user changed
    /// something" bug `SharedLlm` closes for a reconnect. Two separate locks
    /// (an earlier version of this code) let a read land BETWEEN `set_model`'s
    /// two writes and see a new model id paired with the OLD provider
    /// profile, routing the new model through the wrong provider — a single
    /// lock over the pair makes that torn read structurally impossible.
    selection: RwLock<(String, Option<String>)>,
}

impl ApiServiceModel {
    #[must_use]
    pub fn new(service: Arc<ApiService>, model: impl Into<String>, profile: Option<String>) -> Self {
        Self {
            service,
            selection: RwLock::new((model.into(), profile)),
        }
    }
}

#[async_trait]
impl LocalAppsModel for ApiServiceModel {
    async fn structured(
        &self,
        system: &str,
        user: String,
        tool_name: &str,
        schema: serde_json::Value,
        deltas: Option<Arc<dyn GenerationDeltaSink>>,
    ) -> Result<serde_json::Value, AppError> {
        let tool = serde_json::json!({
            "name": tool_name,
            "description": format!(
                "Emit the {tool_name} result as a single structured JSON object matching the provided schema. This is the only way to answer — always call this tool."
            ),
            "input_schema": schema,
        });
        // Snapshot the PAIR together under one lock acquisition (never held
        // across the `.await` below) — a concurrent `set_model` must never
        // block, or be blocked by, an in-flight structured call, but a
        // reader must also never see a model id paired with a profile from
        // a DIFFERENT `set_model` call.
        let (model, profile) = self.selection.read().expect("selection lock poisoned").clone();

        // Ask for a forced tool call first, and fall back to an UNFORCED one if
        // the provider rejects the directive itself.
        //
        // Measured against the live DeepSeek API (api.deepseek.com), not
        // inferred — every row below is an observed response:
        //
        //   thinking on   + tools + tool_choice=required    -> 400
        //   thinking on   + tools + tool_choice={named fn}  -> 400
        //   NO thinking field + tools + tool_choice         -> 400   (!)
        //   thinking on   + tools + NO tool_choice          -> 200, tool_calls
        //   thinking off  + tools + tool_choice=required    -> 200, tool_calls
        //   every 400: "Thinking mode does not support this tool_choice"
        //
        // Two things follow. (1) The trigger is NOT our session thinking config.
        // DeepSeek V4 has thinking enabled by default server-side, so omitting
        // the field entirely still 400s: `deepseek_legacy_model`
        // (`llm-client/src/providers/openai.rs:61`) only maps the legacy
        // `deepseek-chat`/`deepseek-reasoner` ids, so the native
        // `deepseek-v4-flash`/`deepseek-v4-pro` ids send no `thinking` at all
        // and inherit that default. Turning thinking off for side queries would
        // therefore not have fixed this. (2) Unforced tool calling genuinely
        // works there — including at the write-code stage's size (a 3-file,
        // 14 KB `emit_sources` call returning finish_reason `tool_calls`), which
        // is the case most likely to degrade into prose. This is a verified
        // fallback, not a hopeful one.
        //
        // The restriction is undocumented: DeepSeek's thinking-mode guide says
        // only "thinking mode supports tool calls" and never mentions
        // tool_choice. LangChain, pydantic-ai, opencode and claude-code-router
        // have all filed the same 400.
        //
        // Dropping the directive unconditionally would be the wrong trade: on
        // Anthropic the forced call is what makes a schema-shaped answer a
        // guarantee. So: try forced, and retry once without it only when the
        // provider says the directive is the problem.
        //
        // Nothing is weakened by the fallback. `extract_single_tool_call` still
        // requires exactly one tool call naming THIS tool, so an unforced reply
        // that answers in prose fails loudly as `LlmOutputRejected` rather than
        // silently returning junk.
        //
        // The principled fix — consulting `Capabilities { reasoning,
        // structured_output }` per profile and preferring
        // `ResponseFormat::JsonSchema` where supported — is tracked in
        // `docs/superpowers/specs/2026-08-07-structured-output-capability-gating.md`.
        // This stays a narrow, self-correcting workaround until that lands.
        let forced = self
            .open_stream(&model, profile.as_deref(), system, user.clone(), &tool, Some(tool_name))
            .await;
        let stream = match forced {
            Ok(stream) => stream,
            Err(error) if rejects_tool_choice(&error) => {
                tracing::warn!(
                    model = %model,
                    tool = %tool_name,
                    error = %error,
                    "provider rejected an explicit tool_choice; retrying unforced"
                );
                self.open_stream(&model, profile.as_deref(), system, user, &tool, None)
                    .await
                    .map_err(|error| AppError::LlmUnavailable(format!("{error}")))?
            }
            Err(error) => return Err(AppError::LlmUnavailable(format!("{error}"))),
        };

        // Tee the live text out to the caller's sink WITHOUT altering the
        // stream: `inspect` observes each event and passes it through
        // untouched, so assembly below sees exactly the sequence the provider
        // sent. The sink is synchronous and must never block — a UI consumer
        // that falls behind must drop deltas, not stall the model.
        let observed = deltas.clone();
        let stream = stream.inspect(move |event| {
            let (Some(sink), Ok(event)) = (observed.as_ref(), event) else {
                return;
            };
            if let llm_client::LlmEvent::ContentBlockDelta { delta, .. } = event {
                match delta {
                    llm_client::ContentDelta::ThinkingDelta { thinking } => {
                        sink.on_delta(GenerationDeltaKind::Thinking, thinking);
                    }
                    llm_client::ContentDelta::TextDelta { text } => {
                        sink.on_delta(GenerationDeltaKind::Text, text);
                    }
                    // `InputJsonDelta` is the tool call's arguments mid-flight:
                    // half-written JSON, not something to show a user. The
                    // assembled call is what the caller gets, and the stages
                    // narrate their own progress around it.
                    _ => {}
                }
            }
        });

        // Assemble with the SAME accumulator the subagent path uses
        // (`llm_client::stream_accumulator`, moved there from `agent` so this
        // call site reuses it rather than growing a second, weaker one). Its
        // salvaged partial blocks are dropped here: unlike a subagent turn,
        // half a plan is not a lesser answer, it is an unusable one — and
        // `extract_single_tool_call` would reject it anyway.
        let response = accumulate_stream_salvaging(Box::pin(stream))
            .await
            .map_err(|(_partial, error)| AppError::LlmUnavailable(format!("{error}")))?;
        extract_single_tool_call(response.content, tool_name, response.stop_reason.as_deref())
    }

    fn set_model(&self, model: String, profile: Option<String>) {
        *self.selection.write().expect("selection lock poisoned") = (model, profile);
    }
}

impl ApiServiceModel {
    /// Open one streaming side query, with or without the forced tool.
    ///
    /// Split out so the forced attempt and the unforced retry above are the
    /// SAME request in every other respect — a retry that quietly differed in
    /// system prompt, budget or tool schema would make the fallback's success
    /// mean something other than "the directive was the only problem".
    ///
    /// Streaming (rather than `messages_create_side_query`) is what makes the
    /// live transcript possible: these stages take tens of seconds inside a
    /// single call, and a non-streaming round-trip has nothing to report until
    /// it is over. `stream_forced` is the same request shape the subagent's
    /// structured output uses, and — like the batched side query before it —
    /// sets no `max_tokens` of its own, so the model's own ceiling applies.
    async fn open_stream(
        &self,
        model: &str,
        profile: Option<&str>,
        system: &str,
        user: String,
        tool: &serde_json::Value,
        forced_tool: Option<&str>,
    ) -> Result<
        futures_util::stream::BoxStream<'static, Result<llm_client::LlmEvent, llm_client::LlmError>>,
        llm_client::LlmError,
    > {
        self.service
            .stream_forced(
                model,
                profile,
                Some(system),
                vec![ConversationMessage::user(MessageId::new(), user)],
                vec![tool.clone()],
                forced_tool,
                None,
            )
            .await
    }
}

/// Whether a provider error is specifically "I don't accept a `tool_choice`
/// directive", as opposed to any other 4xx.
///
/// Matched on the message because the status lives there rather than in a
/// field. Deliberately narrow: it must not swallow a genuine bad-request (a
/// malformed schema, an over-long prompt), or the unforced retry would mask a
/// real defect as a provider quirk. Observed shape, DeepSeek with reasoning on:
///
///   400 {"error":{"message":"Thinking mode does not support this tool_choice",
///        "type":"invalid_request_error", …}}
fn rejects_tool_choice(error: &llm_client::LlmError) -> bool {
    let message = error.to_string();
    message.contains("tool_choice") && message.contains("400")
}

/// `ToolChoice::Tool { name }` forces the model to call the named tool, but
/// it carries only a name — it does NOT disable parallel tool use, so a
/// response can legally contain more than one `ToolCall` block naming
/// `tool_name`. Taking only the first match (an earlier version of this
/// scan) would silently discard every later one: under the overlay write
/// semantics an `emit_sources` response with two tool calls would drop the
/// second batch of files with no error anywhere — `screen_writes` only ever
/// sees the truncated first batch. Collect every match and require exactly
/// one. A free function (rather than inlined in [`ApiServiceModel::structured`])
/// so the three-way branch is unit-testable against hand-built
/// `ContentBlock` values, with no `ApiService` or network involved.
fn extract_single_tool_call(
    content: Vec<ContentBlock>,
    tool_name: &str,
    stop_reason: Option<&str>,
) -> Result<serde_json::Value, AppError> {
    let summary = describe_content(&content);
    let mut matches: Vec<serde_json::Value> = content
        .into_iter()
        .filter_map(|block| match block {
            ContentBlock::ToolCall { name, input, .. } if name == tool_name => Some(input),
            _ => None,
        })
        .collect();
    match matches.len() {
        1 => Ok(matches.remove(0)),
        // `max_tokens` means the answer was CUT OFF, not withheld: a reasoning
        // model can spend the whole budget thinking and never reach the tool
        // call. That is a budget defect on our side, so it must not read as
        // "the model refused" — the two have opposite fixes.
        0 if stop_reason == Some("max_tokens") => Err(AppError::LlmOutputRejected(format!(
            "the model ran out of output budget before it finished calling `{tool_name}` \
             (stop_reason=max_tokens, {summary}); a reasoning model can spend the whole \
             budget thinking"
        ))),
        0 => Err(AppError::LlmOutputRejected(format!(
            "the model did not call the required tool `{tool_name}` \
             (stop_reason={}, {summary})",
            stop_reason.unwrap_or("none")
        ))),
        count => Err(AppError::LlmOutputRejected(format!(
            "the model called `{tool_name}` {count} times; expected exactly one call"
        ))),
    }
}

/// A one-line, log-free description of what the model actually returned.
///
/// Mobile installs no `tracing` subscriber, so `tracing::warn!` from this
/// module reaches nobody on a device — the error string IS the only channel a
/// failure has. A bare "the model did not call the required tool" is therefore
/// unactionable in exactly the situation that needs action most. Block kinds
/// plus a short text prefix distinguish "answered in prose", "emitted a
/// DIFFERENT tool", and "returned nothing" without leaking a whole response
/// into a user-facing alert.
fn describe_content(content: &[ContentBlock]) -> String {
    if content.is_empty() {
        return "no content blocks".to_string();
    }
    let mut kinds: Vec<String> = Vec::new();
    let mut text_prefix: Option<String> = None;
    for block in content {
        match block {
            ContentBlock::Text { text, .. } => {
                kinds.push("text".to_string());
                if text_prefix.is_none() && !text.trim().is_empty() {
                    text_prefix = Some(text.chars().take(160).collect());
                }
            }
            ContentBlock::ToolCall { name, .. } => kinds.push(format!("tool_call:{name}")),
            ContentBlock::Reasoning { .. } => kinds.push("reasoning".to_string()),
            other => kinds.push(format!("{}", ContentBlockKind(other))),
        }
    }
    match text_prefix {
        Some(prefix) => format!("blocks=[{}], text starts: {prefix:?}", kinds.join(", ")),
        None => format!("blocks=[{}]", kinds.join(", ")),
    }
}

/// `Display` for the block kinds [`describe_content`] does not name
/// explicitly, without matching every variant of a non-exhaustive enum.
struct ContentBlockKind<'a>(&'a ContentBlock);

impl std::fmt::Display for ContentBlockKind<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The serde tag is the block's wire kind — stable, and it stays correct
        // when a new variant lands without this match being updated.
        let tag = serde_json::to_value(self.0)
            .ok()
            .and_then(|value| value.get("type").and_then(|t| t.as_str()).map(str::to_string))
            .unwrap_or_else(|| "unknown".to_string());
        f.write_str(&tag)
    }
}

/// Everything the write-code call needs.
///
/// 写码调用的全部输入。
#[derive(Debug, Clone)]
pub struct SourceRequest {
    pub brief: String,
    pub plan: AppPlan,
    pub answers: BTreeMap<String, DesignValue>,
    /// Current workspace source. Empty for the FIRST attempt of an initial
    /// generation (there is nothing to show yet); populated for a revision's
    /// first attempt, AND refreshed by the caller before every repair
    /// attempt of any job kind — once a rejected write has landed on disk,
    /// the model must see it, or a repair pass reads as "nothing to fix"
    /// against `validator_feedback` describing bytes it can't see.
    ///
    /// 当前工作区源码。初次生成的第一次尝试为空（还没有可展示的内容）；
    /// 修订的第一次尝试会带上现有源码；此外无论哪种 job kind，调用方都会
    /// 在每次修复重试前刷新它——一旦被拒绝的写入已经落盘，模型就必须看到，
    /// 否则修复这一轮会被读成"没什么要改的"，跟同时给出的
    /// `validator_feedback` 自相矛盾。
    pub existing: Vec<FileWrite>,
    /// A note about the `existing` dump above — e.g. how many files were left
    /// out because the tree exceeded the read budget. Kept separate from
    /// `revision_prompt` (the user's own words) and never a fabricated
    /// `FileWrite`: a placeholder entry describing the omission would read to
    /// the model as a real file that exists in the workspace, when it is
    /// really just missing from this prompt.
    ///
    /// 关于上面 `existing` 转储的说明——例如因为超出读取预算而省略了多少个
    /// 文件。与 `revision_prompt`（用户原话）分开存放，也绝不伪造一个
    /// `FileWrite`：一个描述"被省略"的占位条目会被模型读成工作区里真实存在
    /// 的文件，而它其实只是没被塞进这次 prompt。
    pub existing_note: Option<String>,
    /// The user's own natural-language revision request.
    ///
    /// 用户的自然语言修改要求。
    pub revision_prompt: Option<String>,
    /// The previous validator failure's raw message, for a repair pass.
    ///
    /// 上一轮 validator 的错误原文，用于修复循环。
    pub validator_feedback: Option<String>,
}

/// `validate_questionnaire`/`validate_plan`/`screen_writes` all build
/// `AppError::InvalidRequest` — the vocabulary they share with every OTHER
/// caller in the codebase, most of which really are describing a malformed
/// *user* request. At this seam the rejected "request" was authored by the
/// model, not the user: a bare `?` on any of the three would let that kind
/// leak through unchanged, and the client renders `InvalidRequest` as "the
/// request itself is malformed (bad id, empty name…)" — wrong copy, wrong
/// recovery action, for a failure the user neither caused nor can fix by
/// retyping anything. Remap `InvalidRequest` to `LlmOutputRejected` at each
/// of the three validator/gate call sites; every other `AppError` variant
/// (there currently are none from these three functions, but the match stays
/// total for whatever a future validator might add) passes through as-is.
fn as_llm_output_rejected(error: AppError) -> AppError {
    match error {
        AppError::InvalidRequest(message) => AppError::LlmOutputRejected(message),
        other => other,
    }
}

/// The three-call seam: author a questionnaire, derive a plan, write source.
pub struct LocalAppsLlm {
    model: Arc<dyn LocalAppsModel>,
}

impl LocalAppsLlm {
    #[must_use]
    pub fn new(model: Arc<dyn LocalAppsModel>) -> Self {
        Self { model }
    }

    /// Follow a live `/model` switch: see [`LocalAppsModel::set_model`].
    pub fn set_model(&self, model: String, profile: Option<String>) {
        self.model.set_model(model, profile);
    }

    /// Author a questionnaire for this brief. Returns `(suggested name, questionnaire)`.
    ///
    /// 为这次 brief 出一份问卷。返回 (建议名, 问卷)。
    pub async fn author_questionnaire(
        &self,
        brief: &str,
        deltas: Option<Arc<dyn GenerationDeltaSink>>,
    ) -> Result<(Option<String>, Vec<AppDesignStep>), AppError> {
        let value = self
            .model
            .structured(
                AUTHOR_PROMPT,
                format!("用户的描述：\n{brief}"),
                TOOL_QUESTIONNAIRE,
                questionnaire_schema(),
                deltas,
            )
            .await?;
        let name = value
            .get("suggestedName")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string);
        let steps: Vec<AppDesignStep> = serde_json::from_value(
            value.get("steps").cloned().unwrap_or(serde_json::Value::Null),
        )
        .map_err(|error| {
            AppError::LlmOutputRejected(format!("questionnaire is malformed: {error}"))
        })?;
        validate_questionnaire(&steps).map_err(as_llm_output_rejected)?;
        Ok((name, steps))
    }

    /// Derive a plan from the user's answers.
    ///
    /// 从答案推导方案。
    pub async fn plan(
        &self,
        brief: &str,
        steps: &[AppDesignStep],
        answers: &BTreeMap<String, DesignValue>,
        deltas: Option<Arc<dyn GenerationDeltaSink>>,
    ) -> Result<AppPlan, AppError> {
        let user = format!(
            "用户的描述：\n{brief}\n\n问卷：\n{}\n\n用户的回答：\n{}\n\n\
             凡是回答为 {{\"kind\":\"deferred\"}} 的字段由你定夺，\
             并在 summary 里如实说明你定成了什么。",
            serde_json::to_string_pretty(steps).unwrap_or_default(),
            serde_json::to_string_pretty(answers).unwrap_or_default(),
        );
        let value = self
            .model
            .structured(PLAN_PROMPT, user, TOOL_PLAN, plan_schema(), deltas)
            .await?;
        let mut plan: AppPlan = serde_json::from_value(value)
            .map_err(|error| AppError::LlmOutputRejected(format!("plan is malformed: {error}")))?;
        // Normalize BEFORE validating (same order `AppState::plan_ready`
        // uses, and for the same reason — see `normalize_plan`'s doc
        // comment): `validate_plan` now rejects a duplicate domain outright
        // (review NEW-2 tightened it to match `AppManifest::validate`), and
        // a model producing `API.Example.com` alongside `api.example.com`
        // is a cosmetic duplicate, not a bad plan. Normalizing here first
        // means this early gate does not manufacture an avoidable
        // `LlmOutputRejected` retry for something `plan_ready` would have
        // silently repaired anyway — and it means the plan `plan_ready`
        // later normalizes again is already in its final, validated shape
        // (normalization is idempotent). `normalize_plan` can itself reject
        // (an over-`MAX_DOMAINS` raw count, checked before dedup — review
        // NEW-2 round 2), which is a genuine "the model over-produced"
        // failure and belongs behind the same `LlmOutputRejected` remap as
        // `validate_plan`'s own rejections.
        normalize_plan(&mut plan).map_err(as_llm_output_rejected)?;
        validate_plan(&plan).map_err(as_llm_output_rejected)?;
        Ok(plan)
    }

    /// Write source. Returns writes that have already passed the Task 7 gate.
    ///
    /// 写源码。返回已过闸门的写盘请求。
    pub async fn generate_sources(
        &self,
        request: &SourceRequest,
        deltas: Option<Arc<dyn GenerationDeltaSink>>,
    ) -> Result<Vec<FileWrite>, AppError> {
        let mut user = format!(
            "用户的描述：\n{}\n\n方案：\n{}\n\n用户的回答：\n{}",
            request.brief,
            serde_json::to_string_pretty(&request.plan).unwrap_or_default(),
            serde_json::to_string_pretty(&request.answers).unwrap_or_default(),
        );
        if !request.existing.is_empty() {
            user.push_str("\n\n现有源码：\n");
            for file in &request.existing {
                user.push_str(&format!("--- {} ---\n{}\n", file.path, file.contents));
            }
        }
        if let Some(note) = &request.existing_note {
            user.push_str(&format!("\n\n{note}"));
        }
        if let Some(prompt) = &request.revision_prompt {
            user.push_str(&format!("\n\n用户要求的修改：\n{prompt}"));
        }
        if let Some(feedback) = &request.validator_feedback {
            // "完整文件" alone reads two ways — "the whole app" (pulls against
            // the overlay-write ruling) or "each file's full contents, not a
            // patch" (the intended meaning). Spelled out so only the second
            // reading survives: only the files touched by the fix, each given
            // in full, everything else left unsent. The "files not mentioned
            // need not be resent" half only makes sense when the model was
            // actually shown a tree to compare against (`existing` above) —
            // an empty `existing` with that clause still attached would read
            // as "don't resend anything", which is nonsensical advice.
            let skip_unchanged = if request.existing.is_empty() {
                ""
            } else {
                "，没有改动的文件无需重新发送"
            };
            user.push_str(&format!(
                "\n\n上一次生成没有通过校验，原文如下。请修正问题，只需重新给出改动涉及的\
                 那些文件——每个文件都给出完整内容（不是补丁片段）{skip_unchanged}：\n{feedback}"
            ));
        }
        let value = self
            .model
            .structured(SOURCES_PROMPT, user, TOOL_SOURCES, sources_schema(), deltas)
            .await?;
        let files = value.get("files").and_then(serde_json::Value::as_array).ok_or_else(|| {
            AppError::LlmOutputRejected("generator returned no `files` array".into())
        })?;
        let writes: Vec<FileWrite> = files
            .iter()
            .map(|file| {
                let path = file
                    .get("path")
                    .and_then(serde_json::Value::as_str)
                    .ok_or_else(|| AppError::LlmOutputRejected("a file entry has no path".into()))?;
                let contents = file
                    .get("contents")
                    .and_then(serde_json::Value::as_str)
                    .ok_or_else(|| {
                        AppError::LlmOutputRejected(format!("`{path}` has no contents"))
                    })?;
                Ok(FileWrite { path: path.to_string(), contents: contents.to_string() })
            })
            .collect::<Result<_, AppError>>()?;
        screen_writes(&writes).map_err(as_llm_output_rejected)?;
        Ok(writes)
    }
}

/// One field option: `{ value, label }`.
fn field_option_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "value": { "type": "string" },
            "label": { "type": "string" }
        },
        "required": ["value", "label"],
        "additionalProperties": false
    })
}

/// One questionnaire field.
fn design_field_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "id": { "type": "string", "description": "^[a-z][a-z0-9_]{0,39}$，全问卷唯一" },
            "label": { "type": "string" },
            "description": { "type": "string" },
            "fieldType": {
                "type": "string",
                "enum": [
                    "short_text", "long_text", "single_choice", "multiple_choice",
                    "boolean", "color", "density", "screen_list", "feature_list",
                    "data_field_list", "domain_list"
                ]
            },
            "required": { "type": "boolean" },
            "allowsCustom": { "type": "boolean" },
            "allowsDefer": { "type": "boolean" },
            "options": {
                "type": "array",
                "maxItems": MAX_OPTIONS,
                "items": field_option_schema()
            }
        },
        "required": ["id", "label", "fieldType"],
        "additionalProperties": false
    })
}

/// One questionnaire step (a group of fields rendered as one screen).
fn design_step_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "id": { "type": "string", "description": "^[a-z][a-z0-9_]{0,39}$，问卷内唯一" },
            "order": { "type": "integer", "minimum": 0 },
            "title": { "type": "string" },
            "description": { "type": "string" },
            "fields": {
                "type": "array",
                "minItems": 1,
                "maxItems": MAX_FIELDS_PER_STEP,
                "items": design_field_schema()
            }
        },
        "required": ["id", "order", "title", "fields"],
        "additionalProperties": false
    })
}

/// `author_questionnaire`'s forced-tool schema. `steps.maxItems` mirrors
/// [`local_apps::questionnaire::MAX_STEPS`] — a hint to the model, NOT the
/// enforcement: [`validate_questionnaire`] is what actually rejects an
/// over-limit questionnaire.
fn questionnaire_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "suggestedName": { "type": "string" },
            "steps": {
                "type": "array",
                "minItems": 1,
                "maxItems": MAX_STEPS,
                "items": design_step_schema()
            }
        },
        "required": ["steps"],
        "additionalProperties": false
    })
}

/// One native data-collection field: `{ id, label, kind, required?, enumOptions? }`.
fn data_field_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "id": { "type": "string", "description": "^[a-z][a-z0-9_]{0,39}$，同一集合内唯一" },
            "label": { "type": "string" },
            "kind": {
                "type": "string",
                "enum": ["text", "long_text", "integer", "decimal", "boolean", "date_time", "enum", "image_ref"]
            },
            "required": { "type": "boolean" },
            "enumOptions": { "type": "array", "items": { "type": "string" } }
        },
        "required": ["id", "label", "kind"],
        "additionalProperties": false
    })
}

/// One native data collection: `{ id, name, fields }`.
fn data_collection_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "id": { "type": "string", "description": "^[a-z][a-z0-9_]{0,39}$，方案内唯一" },
            "name": { "type": "string" },
            "fields": { "type": "array", "items": data_field_schema() }
        },
        "required": ["id", "name", "fields"],
        "additionalProperties": false
    })
}

/// `plan`'s forced-tool schema. `collections.maxItems`/`domains.maxItems`
/// mirror [`MAX_COLLECTIONS`]/[`MAX_DOMAINS`] — hints only: [`validate_plan`]
/// is the real gate.
fn plan_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "collections": {
                "type": "array",
                "maxItems": MAX_COLLECTIONS,
                "items": data_collection_schema()
            },
            "capabilities": {
                "type": "array",
                "items": { "type": "string", "enum": ["data_mutation", "ui_control"] }
            },
            "domains": {
                "type": "array",
                "maxItems": MAX_DOMAINS,
                "items": { "type": "string" }
            },
            "summary": { "type": "string" }
        },
        "required": ["summary"],
        "additionalProperties": false
    })
}

/// `generate_sources`'s forced-tool schema. The real gate is
/// [`crate::local_apps_sources::screen_writes`] — `maxItems` here is only a
/// hint to the model, not a guarantee: nothing stops a model from returning
/// more than it, so the gate re-checks unconditionally after the call.
fn sources_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "files": {
                "type": "array",
                "minItems": 1,
                "maxItems": crate::local_apps_sources::MAX_GENERATED_FILES,
                "items": {
                    "type": "object",
                    "properties": {
                        "path": {
                            "type": "string",
                            "description": "工作区相对路径，必须以 app/、components/、lib/、styles/ 或 public/ 开头"
                        },
                        "contents": { "type": "string" }
                    },
                    "required": ["path", "contents"],
                    "additionalProperties": false
                }
            }
        },
        "required": ["files"],
        "additionalProperties": false
    })
}

/// Test double for [`LocalAppsModel`]: replays scripted responses in order
/// and records every prompt it was asked. Shared beyond this module's own
/// tests — the generation executor's repair-loop tests and the profile
/// registry's wiring tests reach it via `crate::local_apps_llm::test_support`
/// to drive [`LocalAppsLlm`] without a real `ApiService`.
///
/// 供 [`LocalAppsModel`] 使用的测试替身：按顺序吐出预置响应，并记录每次
/// 收到的 prompt。不止本模块自己的测试在用——生成执行器的修复循环测试、
/// profile 注册表的接线测试都经 `crate::local_apps_llm::test_support`
/// 复用它，绕开真实 `ApiService` 驱动 [`LocalAppsLlm`]。
#[cfg(test)]
pub(crate) mod test_support {
    use super::{AppError, GenerationDeltaSink, LocalAppsModel};
    use async_trait::async_trait;
    use std::sync::{Arc, Mutex};

    /// 按顺序吐出预置响应的假模型。
    pub(crate) struct ScriptedModel {
        responses: Mutex<Vec<Result<serde_json::Value, AppError>>>,
        prompts: Mutex<Vec<String>>,
    }

    impl ScriptedModel {
        pub(crate) fn new(responses: Vec<Result<serde_json::Value, AppError>>) -> Arc<Self> {
            Arc::new(Self { responses: Mutex::new(responses), prompts: Mutex::new(Vec::new()) })
        }

        /// How many times `structured` has been called so far.
        pub(crate) fn call_count(&self) -> usize {
            self.prompts.lock().expect("lock").len()
        }

        /// The `index`-th call's user prompt (0-based, call order).
        pub(crate) fn prompt_at(&self, index: usize) -> String {
            self.prompts.lock().expect("lock")[index].clone()
        }
    }

    #[async_trait]
    impl LocalAppsModel for ScriptedModel {
        async fn structured(
            &self,
            _system: &str,
            user: String,
            _tool_name: &str,
            _schema: serde_json::Value,
            _deltas: Option<Arc<dyn GenerationDeltaSink>>,
        ) -> Result<serde_json::Value, AppError> {
            self.prompts.lock().expect("lock").push(user);
            let mut responses = self.responses.lock().expect("lock");
            if responses.is_empty() {
                return Err(AppError::Io("the scripted model ran out of responses".into()));
            }
            responses.remove(0)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::ScriptedModel;
    use super::*;

    fn good_questionnaire() -> serde_json::Value {
        serde_json::json!({
            "suggestedName": "记事本",
            "steps": [{
                "id": "basics", "order": 0, "title": "功能",
                "fields": [{
                    "id": "features", "label": "需要哪些功能",
                    "fieldType": "multiple_choice", "required": true,
                    "allowsCustom": true, "allowsDefer": true,
                    "options": [{"value": "list", "label": "笔记列表"}]
                }]
            }]
        })
    }

    fn good_plan() -> serde_json::Value {
        serde_json::json!({
            "summary": "一个记事本，帮你记录日常想法。",
            "collections": [{
                "id": "notes", "name": "笔记",
                "fields": [{"id": "title", "label": "标题", "kind": "text", "required": true}]
            }],
            "capabilities": ["data_mutation"],
            "domains": []
        })
    }

    /// A [`LocalAppsModel`] double that only records `set_model` calls —
    /// proves [`LocalAppsLlm::set_model`] actually delegates to the
    /// underlying model instead of silently no-op'ing (the DEFAULT trait
    /// method every OTHER test double relies on).
    struct RecordingModel {
        calls: std::sync::Mutex<Vec<(String, Option<String>)>>,
    }

    #[async_trait]
    impl LocalAppsModel for RecordingModel {
        async fn structured(
            &self,
            _system: &str,
            _user: String,
            _tool_name: &str,
            _schema: serde_json::Value,
            _deltas: Option<Arc<dyn GenerationDeltaSink>>,
        ) -> Result<serde_json::Value, AppError> {
            unreachable!("not exercised by this test")
        }

        fn set_model(&self, model: String, profile: Option<String>) {
            self.calls.lock().expect("lock").push((model, profile));
        }
    }

    /// PINS the Important-3 fix from the Task 11 review: `ClientCommand::
    /// SetModel` must reach the local-apps LLM, not just the orchestrator's
    /// own model selection — otherwise the three local-app LLM stages stay
    /// silently pinned to whatever was live at engine build time forever.
    #[test]
    fn set_model_delegates_to_the_underlying_model() {
        let model = Arc::new(RecordingModel {
            calls: std::sync::Mutex::new(Vec::new()),
        });
        let llm = LocalAppsLlm::new(model.clone());
        llm.set_model("claude-opus-5".into(), Some("anthropic".into()));
        llm.set_model("gpt-5.5".into(), None);
        assert_eq!(
            model.calls.lock().expect("lock").as_slice(),
            &[
                ("claude-opus-5".to_string(), Some("anthropic".to_string())),
                ("gpt-5.5".to_string(), None),
            ]
        );
    }

    #[tokio::test]
    async fn author_questionnaire_returns_the_validated_steps_and_name() {
        let llm = LocalAppsLlm::new(ScriptedModel::new(vec![Ok(good_questionnaire())]));
        let (name, steps) = llm.author_questionnaire("一个记事本", None).await.expect("authoring");
        assert_eq!(name.as_deref(), Some("记事本"));
        assert_eq!(steps.len(), 1);
        assert!(steps[0].fields[0].allows_defer);
    }

    #[tokio::test]
    async fn author_questionnaire_rejects_output_that_fails_validation() {
        let over_limit = serde_json::json!({
            "steps": (0..6).map(|i| serde_json::json!({
                "id": format!("s{i}"), "order": i, "title": "T",
                "fields": [{"id": format!("f{i}"), "label": "L",
                            "fieldType": "boolean", "required": false}]
            })).collect::<Vec<_>>()
        });
        let llm = LocalAppsLlm::new(ScriptedModel::new(vec![Ok(over_limit)]));
        let err = llm
            .author_questionnaire("一个记事本", None)
            .await
            .expect_err("6 steps must be rejected by the validator, not passed through");
        // `validate_questionnaire` itself builds `AppError::InvalidRequest` —
        // this seam must remap it to `LlmOutputRejected` before returning,
        // since the "request" that failed validation was the model's output,
        // not the user's. Without the remap this assertion is what would
        // catch a regression back to the bare `?` (an `expect_err`-only test
        // stays green either way).
        assert!(
            matches!(err, AppError::LlmOutputRejected(_)),
            "a validator rejection must surface as LlmOutputRejected, not InvalidRequest: {err:?}"
        );
    }

    #[tokio::test]
    async fn plan_returns_the_validated_plan() {
        let llm = LocalAppsLlm::new(ScriptedModel::new(vec![Ok(good_plan())]));
        let plan = llm
            .plan("一个记事本", &[], &BTreeMap::new(), None)
            .await
            .expect("planning");
        assert_eq!(plan.collections.len(), 1);
        assert_eq!(plan.collections[0].id, "notes");
        assert_eq!(plan.capabilities, vec![local_apps::AppCapability::DataMutation]);
        assert!(plan.summary.contains("记事本"));
    }

    #[tokio::test]
    async fn plan_rejects_an_ip_literal_domain() {
        let mut bad = good_plan();
        bad["domains"] = serde_json::json!(["203.0.113.10"]);
        let llm = LocalAppsLlm::new(ScriptedModel::new(vec![Ok(bad)]));
        let err = llm
            .plan("一个记事本", &[], &BTreeMap::new(), None)
            .await
            .expect_err("an IP-literal domain must be rejected by validate_plan, not passed through");
        assert!(
            matches!(err, AppError::LlmOutputRejected(_)),
            "a validator rejection must surface as LlmOutputRejected, not InvalidRequest: {err:?}"
        );
    }

    #[tokio::test]
    async fn plan_rejects_more_than_eight_collections() {
        let mut bad = good_plan();
        bad["collections"] = serde_json::json!((0..9)
            .map(|i| serde_json::json!({
                "id": format!("c{i}"), "name": "C",
                "fields": [{"id": "f", "label": "F", "kind": "text"}]
            }))
            .collect::<Vec<_>>());
        let llm = LocalAppsLlm::new(ScriptedModel::new(vec![Ok(bad)]));
        llm.plan("一个记事本", &[], &BTreeMap::new(), None)
            .await
            .expect_err("9 collections must be rejected by validate_plan, not passed through");
    }

    /// review NEW-2, second-order check: tightening `validate_plan` to
    /// reject a duplicate domain (to match `AppManifest::validate`) must
    /// NOT turn a merely-differently-cased domain into a manufactured
    /// `LlmOutputRejected` retry here — `normalize_plan` runs first and
    /// silently merges it, exactly like it does before `AppState::plan_ready`
    /// stores the plan. Without the `normalize_plan` call added alongside
    /// the tightened `validate_plan`, this plan would have failed here even
    /// though it is perfectly legal after normalization.
    #[tokio::test]
    async fn plan_normalizes_a_case_duplicate_domain_instead_of_rejecting_it() {
        let mut value = good_plan();
        value["domains"] = serde_json::json!(["API.Example.com", "api.example.com"]);
        let llm = LocalAppsLlm::new(ScriptedModel::new(vec![Ok(value)]));
        let plan = llm
            .plan("一个记事本", &[], &BTreeMap::new(), None)
            .await
            .expect("a case-duplicate domain is normalized, not rejected");
        assert_eq!(plan.domains, vec!["api.example.com".to_string()]);
    }

    #[tokio::test]
    async fn the_brief_reaches_the_model_prompt() {
        let model = ScriptedModel::new(vec![Ok(good_questionnaire())]);
        let llm = LocalAppsLlm::new(model.clone());
        llm.author_questionnaire("一个带标签的记事本", None).await.expect("authoring");
        let prompt = model.prompt_at(0);
        assert!(
            prompt.contains("一个带标签的记事本"),
            "the user's own words must reach the model verbatim: {prompt}"
        );
    }

    fn good_sources() -> serde_json::Value {
        serde_json::json!({
            "files": [{"path": "app/page.jsx", "contents": "export default function P(){return null}"}]
        })
    }

    #[tokio::test]
    async fn generate_sources_returns_screened_writes() {
        let llm = LocalAppsLlm::new(ScriptedModel::new(vec![Ok(good_sources())]));
        let writes = llm.generate_sources(&initial_request(), None).await.expect("generation");
        assert_eq!(writes.len(), 1);
        assert_eq!(writes[0].path, "app/page.jsx");
    }

    #[tokio::test]
    async fn generate_sources_rejects_a_write_outside_the_writable_roots() {
        let escaping = serde_json::json!({
            "files": [{"path": "../secret.js", "contents": "x"}]
        });
        let llm = LocalAppsLlm::new(ScriptedModel::new(vec![Ok(escaping)]));
        let err = llm
            .generate_sources(&initial_request(), None)
            .await
            .expect_err("the gate must reject an escaping path before anything is written");
        // `screen_writes` builds `AppError::InvalidRequest` — this seam must
        // remap it to `LlmOutputRejected`, same reasoning as the
        // questionnaire/plan validator sites.
        assert!(
            matches!(err, AppError::LlmOutputRejected(_)),
            "a gate rejection must surface as LlmOutputRejected, not InvalidRequest: {err:?}"
        );
    }

    #[tokio::test]
    async fn a_revision_prompt_reaches_the_model() {
        let model = ScriptedModel::new(vec![Ok(good_sources())]);
        let llm = LocalAppsLlm::new(model.clone());
        let mut request = initial_request();
        request.revision_prompt = Some("把搜索框挪到顶部".into());
        llm.generate_sources(&request, None).await.expect("revision");
        let prompt = model.prompt_at(0);
        assert!(prompt.contains("把搜索框挪到顶部"), "got {prompt}");
    }

    #[tokio::test]
    async fn validator_feedback_reaches_the_model_on_a_repair_pass() {
        let model = ScriptedModel::new(vec![Ok(good_sources())]);
        let llm = LocalAppsLlm::new(model.clone());
        let mut request = initial_request();
        request.validator_feedback = Some("app/page.jsx uses eval()".into());
        llm.generate_sources(&request, None).await.expect("repair");
        let prompt = model.prompt_at(0);
        assert!(prompt.contains("eval()"), "got {prompt}");
    }

    #[tokio::test]
    async fn existing_note_reaches_the_model_prompt() {
        let model = ScriptedModel::new(vec![Ok(good_sources())]);
        let llm = LocalAppsLlm::new(model.clone());
        let mut request = initial_request();
        request.existing_note =
            Some("现有源码树超过了读取预算，省略了 3 个文件".into());
        llm.generate_sources(&request, None).await.expect("generation");
        let prompt = model.prompt_at(0);
        assert!(
            prompt.contains("省略了 3 个文件"),
            "the existing-tree truncation note must reach the model's prompt text: {prompt}"
        );
    }

    #[test]
    fn extract_single_tool_call_accepts_exactly_one_match() {
        let blocks = vec![
            ContentBlock::Text { text: "thinking out loud".into(), cache_control: None },
            ContentBlock::ToolCall {
                id: "call-1".into(),
                name: TOOL_SOURCES.into(),
                input: serde_json::json!({"files": []}),
            },
        ];
        let value = extract_single_tool_call(blocks, TOOL_SOURCES, Some("tool_use"))
            .expect("exactly one match");
        assert_eq!(value, serde_json::json!({"files": []}));
    }

    #[test]
    fn extract_single_tool_call_rejects_zero_matches() {
        let blocks = vec![ContentBlock::Text { text: "no tool call here".into(), cache_control: None }];
        let err = extract_single_tool_call(blocks, TOOL_SOURCES, Some("end_turn"))
            .expect_err("no matching ToolCall block must be rejected");
        assert!(
            matches!(err, AppError::LlmOutputRejected(_)),
            "a missing tool call must surface as LlmOutputRejected: {err:?}"
        );
    }

    /// Mobile installs no `tracing` subscriber, so this string is the ONLY
    /// channel a failure has on a device. A message that does not say what came
    /// back instead leaves the next failure as unactionable as the last one.
    #[test]
    fn a_missing_tool_call_reports_what_the_model_returned_instead() {
        let blocks = vec![ContentBlock::Text {
            text: "好的，我来帮你设计这个应用。首先我们需要确定几个关键点……".into(),
            cache_control: None,
        }];
        let AppError::LlmOutputRejected(message) =
            extract_single_tool_call(blocks, TOOL_QUESTIONNAIRE, Some("end_turn"))
                .expect_err("prose instead of a tool call must be rejected")
        else {
            panic!("prose must be rejected as LlmOutputRejected");
        };
        assert!(
            message.contains(TOOL_QUESTIONNAIRE),
            "the message must name the tool that was expected: {message}"
        );
        assert!(
            message.contains("blocks=[text]"),
            "the message must say what block kinds came back: {message}"
        );
        assert!(
            message.contains("好的，我来帮你设计"),
            "the message must quote the start of the prose so the cause is visible: {message}"
        );
    }

    /// A truncated answer and a withheld answer have OPPOSITE fixes: the first
    /// is our budget, the second is the model. Reporting both as "did not call
    /// the required tool" sends the reader after the wrong one.
    #[test]
    fn a_truncated_answer_is_reported_as_a_budget_overrun_not_a_refusal() {
        let blocks = vec![ContentBlock::Text { text: String::new(), cache_control: None }];
        let AppError::LlmOutputRejected(message) =
            extract_single_tool_call(blocks, TOOL_QUESTIONNAIRE, Some("max_tokens"))
                .expect_err("a truncated response must still be rejected")
        else {
            panic!("a truncated response must be rejected as LlmOutputRejected");
        };
        assert!(
            message.contains("ran out of output budget"),
            "a max_tokens stop must read as a budget overrun: {message}"
        );
        assert!(
            !message.contains("did not call"),
            "a truncated answer must NOT read as the model refusing: {message}"
        );
    }

    #[test]
    fn extract_single_tool_call_rejects_more_than_one_match() {
        // `ToolChoice::Tool { name }` forces the tool but does not disable
        // parallel tool use — a response can legally carry the SAME tool
        // name twice. Taking the first would silently drop the second batch
        // of files under the overlay write semantics.
        let blocks = vec![
            ContentBlock::ToolCall {
                id: "call-1".into(),
                name: TOOL_SOURCES.into(),
                input: serde_json::json!({"files": [{"path": "app/a.jsx", "contents": "a"}]}),
            },
            ContentBlock::ToolCall {
                id: "call-2".into(),
                name: TOOL_SOURCES.into(),
                input: serde_json::json!({"files": [{"path": "app/b.jsx", "contents": "b"}]}),
            },
        ];
        let err = extract_single_tool_call(blocks, TOOL_SOURCES, Some("tool_use"))
            .expect_err("two matching tool calls must be rejected, not silently truncated");
        assert!(
            matches!(err, AppError::LlmOutputRejected(_)),
            "an ambiguous multi-call response must surface as LlmOutputRejected: {err:?}"
        );
    }

    #[tokio::test]
    async fn a_model_error_propagates_rather_than_falling_back() {
        let llm =
            LocalAppsLlm::new(ScriptedModel::new(vec![Err(AppError::LlmUnavailable("offline".into()))]));
        let err = llm
            .author_questionnaire("一个记事本", None)
            .await
            .expect_err("there is no template to silently fall back to — fail closed");
        // The model itself was unreachable — this must stay `LlmUnavailable`,
        // never get relabeled `LlmOutputRejected` (which means the model DID
        // answer, just badly). Mixing the two would send the client down the
        // wrong recovery-action path (retry/reconnect vs. edit-and-resubmit).
        assert!(
            matches!(err, AppError::LlmUnavailable(_)),
            "an unreachable model must surface as LlmUnavailable: {err:?}"
        );
    }

    fn initial_request() -> SourceRequest {
        SourceRequest {
            brief: "一个记事本".into(),
            plan: AppPlan {
                collections: Vec::new(),
                capabilities: Vec::new(),
                domains: Vec::new(),
                summary: "记事本".into(),
            },
            answers: BTreeMap::new(),
            existing: Vec::new(),
            existing_note: None,
            revision_prompt: None,
            validator_feedback: None,
        }
    }
}
