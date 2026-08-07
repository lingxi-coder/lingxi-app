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
use llm_client::{ApiService, ContentBlock, ToolChoice};
use local_apps::questionnaire::{
    validate_plan, validate_questionnaire, AppDesignStep, AppPlan, MAX_COLLECTIONS, MAX_DOMAINS,
    MAX_FIELDS_PER_STEP, MAX_OPTIONS, MAX_STEPS,
};
use local_apps::{AppError, DesignValue};
use protocol::{ConversationMessage, MessageId};
use std::collections::BTreeMap;
use std::sync::{Arc, RwLock};

const AUTHOR_PROMPT: &str = include_str!("../assets/prompts/author_questionnaire.md");
const PLAN_PROMPT: &str = include_str!("../assets/prompts/plan.md");
const SOURCES_PROMPT: &str = include_str!("../assets/prompts/generate_sources.md");

// Forced-tool names. Hoisted to constants (rather than inline string
// literals at each call site) so [`ApiServiceModel::max_tokens_for`] cannot
// silently desync from the names [`LocalAppsLlm`] actually calls with — a
// rename or a fourth call that only updated one of the two spots would
// otherwise fall through to the 4096 default and truncate mid-tool-call.
const TOOL_QUESTIONNAIRE: &str = "emit_questionnaire";
const TOOL_PLAN: &str = "emit_plan";
const TOOL_SOURCES: &str = "emit_sources";

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
    async fn structured(
        &self,
        system: &str,
        user: String,
        tool_name: &str,
        schema: serde_json::Value,
    ) -> Result<serde_json::Value, AppError>;

    /// Update the default model/profile future `structured` calls route
    /// through — `ClientCommand::SetModel` calls this so the three local-app
    /// LLM stages follow a `/model` switch instead of staying pinned to
    /// whatever was live at engine build time. Default is a no-op: only
    /// [`ApiServiceModel`] (the production implementation) has a live
    /// selection to update; test doubles ignore it.
    fn set_model(&self, _model: String, _profile: Option<String>) {}
}

/// Real [`LocalAppsModel`] over the shared `ApiService` — forced tool call via
/// [`ApiService::messages_create_side_query`], `tool_choice: Some(ToolChoice::Tool { name })`
/// (`llm-client/src/service.rs:2672`). This is the repo's existing structured-
/// output mechanism (`sidequery::ProviderSideQueryClient` uses the same call);
/// nothing new is invented here.
///
/// `max_tokens` is not part of the [`LocalAppsModel::structured`] signature —
/// the trait is shared by all three calls, but the brief assigns each a
/// different budget (author/plan 4096, write-code 32768). `tool_name` is the
/// only per-call signal `structured` receives, and [`LocalAppsLlm`] always
/// calls with one of exactly three names, so `structured` switches its
/// `max_tokens` on that name rather than growing the trait signature for one
/// caller's budget policy.
pub struct ApiServiceModel {
    service: Arc<ApiService>,
    /// `RwLock`-backed, NOT a plain `String`: `ClientCommand::SetModel`
    /// updates this in place (via [`LocalAppsModel::set_model`]) so the
    /// three local-app LLM stages follow a live `/model` switch instead of
    /// staying pinned to whatever `default_model_id`/`default_model_profile`
    /// were at engine build time — the same class of "silently stale after
    /// the user changed something" bug `SharedLlm` closes for a reconnect.
    model: RwLock<String>,
    profile: RwLock<Option<String>>,
}

impl ApiServiceModel {
    #[must_use]
    pub fn new(service: Arc<ApiService>, model: impl Into<String>, profile: Option<String>) -> Self {
        Self {
            service,
            model: RwLock::new(model.into()),
            profile: RwLock::new(profile),
        }
    }

    fn max_tokens_for(tool_name: &str) -> u32 {
        match tool_name {
            TOOL_SOURCES => 32768,
            _ => 4096,
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
    ) -> Result<serde_json::Value, AppError> {
        let tool = serde_json::json!({
            "name": tool_name,
            "description": format!(
                "Emit the {tool_name} result as a single structured JSON object matching the provided schema. This is the only way to answer — always call this tool."
            ),
            "input_schema": schema,
        });
        let message = ConversationMessage::user(MessageId::new(), user);
        // Snapshot both under their own locks (never held across the `.await`
        // below) rather than holding a guard across the network call — a
        // concurrent `set_model` must never block, or be blocked by, an
        // in-flight structured call.
        let model = self.model.read().expect("model lock poisoned").clone();
        let profile = self.profile.read().expect("profile lock poisoned").clone();
        let response = self
            .service
            .messages_create_side_query(
                &model,
                profile.as_deref(),
                Some(system),
                vec![message],
                vec![tool],
                Self::max_tokens_for(tool_name),
                Some(ToolChoice::Tool { name: tool_name.to_string() }),
                vec![],
                None,
            )
            .await
            .map_err(|error| AppError::LlmUnavailable(format!("{error}")))?;
        extract_single_tool_call(response.content, tool_name)
    }

    fn set_model(&self, model: String, profile: Option<String>) {
        *self.model.write().expect("model lock poisoned") = model;
        *self.profile.write().expect("profile lock poisoned") = profile;
    }
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
) -> Result<serde_json::Value, AppError> {
    let mut matches: Vec<serde_json::Value> = content
        .into_iter()
        .filter_map(|block| match block {
            ContentBlock::ToolCall { name, input, .. } if name == tool_name => Some(input),
            _ => None,
        })
        .collect();
    match matches.len() {
        1 => Ok(matches.remove(0)),
        0 => Err(AppError::LlmOutputRejected(
            "the model did not call the required tool".into(),
        )),
        count => Err(AppError::LlmOutputRejected(format!(
            "the model called `{tool_name}` {count} times; expected exactly one call"
        ))),
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
    ) -> Result<(Option<String>, Vec<AppDesignStep>), AppError> {
        let value = self
            .model
            .structured(
                AUTHOR_PROMPT,
                format!("用户的描述：\n{brief}"),
                TOOL_QUESTIONNAIRE,
                questionnaire_schema(),
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
            .structured(PLAN_PROMPT, user, TOOL_PLAN, plan_schema())
            .await?;
        let plan: AppPlan = serde_json::from_value(value)
            .map_err(|error| AppError::LlmOutputRejected(format!("plan is malformed: {error}")))?;
        validate_plan(&plan).map_err(as_llm_output_rejected)?;
        Ok(plan)
    }

    /// Write source. Returns writes that have already passed the Task 7 gate.
    ///
    /// 写源码。返回已过闸门的写盘请求。
    pub async fn generate_sources(
        &self,
        request: &SourceRequest,
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
            .structured(SOURCES_PROMPT, user, TOOL_SOURCES, sources_schema())
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
    use super::{AppError, LocalAppsModel};
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
        let (name, steps) = llm.author_questionnaire("一个记事本").await.expect("authoring");
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
            .author_questionnaire("一个记事本")
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
            .plan("一个记事本", &[], &BTreeMap::new())
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
            .plan("一个记事本", &[], &BTreeMap::new())
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
        llm.plan("一个记事本", &[], &BTreeMap::new())
            .await
            .expect_err("9 collections must be rejected by validate_plan, not passed through");
    }

    #[tokio::test]
    async fn the_brief_reaches_the_model_prompt() {
        let model = ScriptedModel::new(vec![Ok(good_questionnaire())]);
        let llm = LocalAppsLlm::new(model.clone());
        llm.author_questionnaire("一个带标签的记事本").await.expect("authoring");
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
        let writes = llm.generate_sources(&initial_request()).await.expect("generation");
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
            .generate_sources(&initial_request())
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
        llm.generate_sources(&request).await.expect("revision");
        let prompt = model.prompt_at(0);
        assert!(prompt.contains("把搜索框挪到顶部"), "got {prompt}");
    }

    #[tokio::test]
    async fn validator_feedback_reaches_the_model_on_a_repair_pass() {
        let model = ScriptedModel::new(vec![Ok(good_sources())]);
        let llm = LocalAppsLlm::new(model.clone());
        let mut request = initial_request();
        request.validator_feedback = Some("app/page.jsx uses eval()".into());
        llm.generate_sources(&request).await.expect("repair");
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
        llm.generate_sources(&request).await.expect("generation");
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
        let value = extract_single_tool_call(blocks, TOOL_SOURCES).expect("exactly one match");
        assert_eq!(value, serde_json::json!({"files": []}));
    }

    #[test]
    fn extract_single_tool_call_rejects_zero_matches() {
        let blocks = vec![ContentBlock::Text { text: "no tool call here".into(), cache_control: None }];
        let err = extract_single_tool_call(blocks, TOOL_SOURCES)
            .expect_err("no matching ToolCall block must be rejected");
        assert!(
            matches!(err, AppError::LlmOutputRejected(_)),
            "a missing tool call must surface as LlmOutputRejected: {err:?}"
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
        let err = extract_single_tool_call(blocks, TOOL_SOURCES)
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
            .author_questionnaire("一个记事本")
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
