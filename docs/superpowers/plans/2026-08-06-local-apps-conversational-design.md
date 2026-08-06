# 本地应用对话式设计 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 把本地应用的创建流程从「四选一模版 + 固定表单向导」改成「一句话描述 → LLM 现场出问卷 → 出方案 → LLM 写源码 → 自然语言持续迭代」。

**Architecture:** 三段 LLM 调用全部由 host（`engine-mobile`）托管，经 `ApiService::messages_create_side_query` 发起，用强制工具调用拿结构化输出。每段产物先过 host 校验器再落盘——LLM 的输出是提议，校验器的判定才是事实。既有的 `AppGenerationExecutor` trait、generation job 状态机、单 worker 串行、`source_validator`、checkpoint、两道人工门全部保留。

**Tech Stack:** Rust（`local-apps` / `client-protocol` / `engine-mobile` crates）、uniffi FFI、SwiftUI（`clients/ios`）。

## Global Constraints

- 设计依据：`docs/superpowers/specs/2026-08-06-local-apps-conversational-design.md`。本计划与它冲突时以 spec 为准。
- **跑测试必须带 `--all-features`**。`local-apps` 的全部 engine-mobile 侧模块在 `#[cfg(feature = "uniffi")]` 门控内，`cargo test --workspace` 编译不到其中任何一个测试，绿了不说明任何问题。
- **禁止 `cargo fmt`**（仓库既有约定）。只改你要改的行。
- 不放宽 `source_validator.rs` 的任何安全策略。
- 两道人工门（`confirm_design`、`confirm_preview`）一道不能少，不得由 agent 自动点。
- 写盘只允许落在 `WRITABLE_ROOTS`（`source_validator.rs:17`）：`app` / `components` / `lib` / `styles` / `public`。
- 持久化 JSON 风格沿用仓库既有约定：struct 字段 camelCase、enum 变体 `snake_case`、时间戳 epoch 毫秒 `u64`、`Option` 为 `None` 时省略。
- 每个 task 结束时提交一次。commit message 末尾加：
  `Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>`

## 文件结构

**新建**

| 路径 | 职责 |
|---|---|
| `lingxi-code/local-apps/src/questionnaire.rs` | 问卷 / 方案的领域类型与校验器。纯函数，零 I/O，零 LLM |
| `lingxi-code/apps/engine-mobile/src/local_apps_sources.rs` | `FileWrite` 路径与体积闸门。纯函数 |
| `lingxi-code/apps/engine-mobile/src/local_apps_llm.rs` | 三段 LLM 调用：出题 / 出方案 / 写码。含 `LocalAppsModel` trait 与其 `ApiService` 实现 |
| `lingxi-code/apps/engine-mobile/assets/prompts/author_questionnaire.md` | 出题 prompt |
| `lingxi-code/apps/engine-mobile/assets/prompts/plan.md` | 出方案 prompt |
| `lingxi-code/apps/engine-mobile/assets/prompts/generate_sources.md` | 写码 prompt |
| `clients/ios/Sources/LocalApps/LocalAppPlanConfirmView.swift` | 方案确认 sheet |

`questionnaire.rs` 独立于 `types.rs`：后者已 663 行，且校验器是一整块新责任，与持久化类型定义无关。`local_apps_sources.rs` 独立于 `local_apps_llm.rs`：闸门是纯函数、可脱离 LLM 单测，混在一起会让它只能靠 mock 覆盖。

**修改**

`local-apps`：`types.rs`、`state.rs`、`service.rs`、`manifest.rs`、`storage.rs`、`lib.rs`
`client-protocol`：`src/local_apps.rs`、`tests/version_guard_test.rs`
`engine-mobile`：`local_apps_generation.rs`、`local_apps_bridge.rs`、`local_apps_mcp.rs`、`local_apps_profile.rs`、`host.rs`
`skills`：`create-local-app/SKILL.md`
iOS：`LocalAppsModels.swift`、`LocalAppsStore.swift`、`LocalAppDesignerView.swift`、`LocalAppDetailView.swift`、`LocalAppsLibraryView.swift`、`LocalAppsProtocolAdapter.swift`、`Tests/LocalAppsStoreTests.swift`

## 任务依赖

```
T1 问卷类型+校验器 ──┬─▶ T2 删模版/draft三段化 ──▶ T3 状态机 ──▶ T4 AppService
                     │                                              │
                     └──────────────────────────────────────────────┤
T7 FileWrite闸门 ────────────────────────────────────────────────┐  │
                                                                 │  ▼
                                              T5 DTO ──▶ T6 bridge ──┐
                                                                 │   │
                                              T8 LLM三段 ◀───────┘   │
                                                  │                  │
                                                  ▼                  │
                                     T9 generation 接 LLM ◀──────────┘
                                                  │
                          ┌───────────────────────┼────────────────┐
                          ▼                       ▼                ▼
                     T10 MCP              T11 host 编排        T12 SKILL.md
                                                  │
                                                  ▼
                    T13 iOS 模型/store ──▶ T14 iOS 设计器 ──▶ T15 确认 sheet ──▶ T16 iOS 入口
```

---

### Task 1: 问卷与方案的领域类型 + 校验器

零依赖、纯函数，先做完这块，后面所有 LLM 输出都有地方去校验。

**Files:**
- Create: `lingxi-code/local-apps/src/questionnaire.rs`
- Modify: `lingxi-code/local-apps/src/lib.rs`（加 `pub mod questionnaire;` 与 re-export）

**Interfaces:**
- Consumes: `crate::error::AppError`、`crate::types::DesignValue`、`crate::manifest::{DataCollectionSchema, DataFieldKind}`、`crate::permissions::AppCapability`
- Produces:
  - `AppDesignFieldType`（enum：`ShortText`/`LongText`/`SingleChoice`/`MultipleChoice`/`Boolean`/`Color`/`Density`/`ScreenList`/`FeatureList`/`DataFieldList`/`DomainList`）
  - `AppDesignFieldOption { value: String, label: String }`
  - `AppDesignField { id, label, description: Option<String>, field_type, required: bool, allows_custom: bool, allows_defer: bool, default_value: Option<DesignValue>, options: Vec<AppDesignFieldOption> }`
  - `AppDesignStep { id, order: u32, title, description: Option<String>, fields: Vec<AppDesignField> }`
  - `AppPlan { collections: Vec<DataCollectionSchema>, capabilities: Vec<AppCapability>, domains: Vec<String>, summary: String }`
  - `pub fn validate_questionnaire(steps: &[AppDesignStep]) -> Result<(), AppError>`
  - `pub fn validate_plan(plan: &AppPlan) -> Result<(), AppError>`
  - `pub fn validate_answers(steps: &[AppDesignStep], answers: &BTreeMap<String, DesignValue>) -> Result<(), AppError>`
  - 常量：`MAX_STEPS: usize = 5`、`MAX_FIELDS_PER_STEP: usize = 8`、`MAX_FIELDS_TOTAL: usize = 24`、`MAX_OPTIONS: usize = 12`、`MAX_LABEL_CHARS: usize = 80`、`MAX_DESCRIPTION_CHARS: usize = 240`、`MAX_COLLECTIONS: usize = 8`、`MAX_COLLECTION_FIELDS: usize = 24`、`MAX_DOMAINS: usize = 8`、`MAX_SUMMARY_CHARS: usize = 1200`

- [ ] **Step 1: 写失败测试**

在 `lingxi-code/local-apps/src/questionnaire.rs` 末尾建 `#[cfg(test)] mod tests`：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn field(id: &str, field_type: AppDesignFieldType) -> AppDesignField {
        AppDesignField {
            id: id.into(),
            label: "Label".into(),
            description: None,
            field_type,
            required: false,
            allows_custom: false,
            allows_defer: false,
            default_value: None,
            options: Vec::new(),
        }
    }

    fn choice(id: &str) -> AppDesignField {
        let mut f = field(id, AppDesignFieldType::SingleChoice);
        f.options = vec![AppDesignFieldOption { value: "a".into(), label: "A".into() }];
        f
    }

    fn step(id: &str, order: u32, fields: Vec<AppDesignField>) -> AppDesignStep {
        AppDesignStep { id: id.into(), order, title: "T".into(), description: None, fields }
    }

    #[test]
    fn accepts_a_minimal_questionnaire() {
        let steps = vec![step("basics", 0, vec![choice("tone")])];
        validate_questionnaire(&steps).expect("minimal questionnaire is valid");
    }

    #[test]
    fn rejects_more_than_five_steps() {
        let steps: Vec<_> = (0..6)
            .map(|i| step(&format!("s{i}"), i, vec![choice(&format!("f{i}"))]))
            .collect();
        let error = validate_questionnaire(&steps).expect_err("6 steps must be rejected");
        assert!(format!("{error}").contains("step"), "message names the offending limit: {error}");
    }

    #[test]
    fn rejects_duplicate_field_ids_across_steps() {
        let steps = vec![
            step("one", 0, vec![choice("shared")]),
            step("two", 1, vec![choice("shared")]),
        ];
        validate_questionnaire(&steps).expect_err("field ids are globally unique");
    }

    #[test]
    fn rejects_a_field_id_that_breaks_the_id_grammar() {
        let steps = vec![step("basics", 0, vec![choice("Bad-Id")])];
        validate_questionnaire(&steps).expect_err("ids are ^[a-z][a-z0-9_]{0,39}$");
    }

    #[test]
    fn rejects_a_choice_field_with_no_options() {
        let steps = vec![step("basics", 0, vec![field("tone", AppDesignFieldType::SingleChoice)])];
        validate_questionnaire(&steps).expect_err("single_choice needs options");
    }

    #[test]
    fn rejects_duplicate_option_values_within_one_field() {
        let mut f = field("tone", AppDesignFieldType::SingleChoice);
        f.options = vec![
            AppDesignFieldOption { value: "a".into(), label: "A".into() },
            AppDesignFieldOption { value: "a".into(), label: "A again".into() },
        ];
        validate_questionnaire(&[step("basics", 0, vec![f])]).expect_err("option values are unique");
    }

    #[test]
    fn rejects_a_default_value_whose_variant_mismatches_the_field_type() {
        let mut f = choice("tone");
        f.default_value = Some(DesignValue::Boolean(true));
        validate_questionnaire(&[step("basics", 0, vec![f])])
            .expect_err("default_value must match field_type");
    }

    #[test]
    fn a_required_field_left_empty_fails_answer_validation() {
        let mut f = choice("tone");
        f.required = true;
        let steps = vec![step("basics", 0, vec![f])];
        validate_answers(&steps, &BTreeMap::new()).expect_err("required field must be answered");
    }

    #[test]
    fn a_required_field_answered_deferred_passes_answer_validation() {
        let mut f = choice("tone");
        f.required = true;
        f.allows_defer = true;
        let steps = vec![step("basics", 0, vec![f])];
        let mut answers = BTreeMap::new();
        answers.insert("tone".to_string(), DesignValue::Deferred);
        validate_answers(&steps, &answers).expect("explicit defer satisfies required");
    }

    #[test]
    fn deferred_is_rejected_on_a_field_that_does_not_allow_it() {
        let mut f = choice("tone");
        f.required = true;
        let steps = vec![step("basics", 0, vec![f])];
        let mut answers = BTreeMap::new();
        answers.insert("tone".to_string(), DesignValue::Deferred);
        validate_answers(&steps, &answers).expect_err("defer needs allows_defer");
    }

    fn plan_with_domain(domain: &str) -> AppPlan {
        AppPlan {
            collections: vec![DataCollectionSchema {
                id: "notes".into(),
                name: "Notes".into(),
                fields: vec![DataFieldSchema {
                    id: "title".into(),
                    label: "Title".into(),
                    kind: DataFieldKind::Text,
                    required: true,
                    enum_options: Vec::new(),
                }],
            }],
            capabilities: Vec::new(),
            domains: vec![domain.into()],
            summary: "Notes app".into(),
        }
    }

    #[test]
    fn accepts_a_plan_with_a_public_https_host() {
        validate_plan(&plan_with_domain("api.example.com")).expect("public host is allowed");
    }

    #[test]
    fn rejects_an_ip_literal_domain() {
        validate_plan(&plan_with_domain("203.0.113.10")).expect_err("IP literals are rejected");
    }

    #[test]
    fn rejects_a_loopback_domain() {
        validate_plan(&plan_with_domain("localhost")).expect_err("loopback is rejected");
    }

    #[test]
    fn rejects_a_private_range_domain() {
        validate_plan(&plan_with_domain("192.168.1.4")).expect_err("private ranges are rejected");
    }

    #[test]
    fn rejects_an_enum_field_with_no_options() {
        let mut plan = plan_with_domain("api.example.com");
        plan.collections[0].fields[0].kind = DataFieldKind::Enum;
        validate_plan(&plan).expect_err("enum fields need enum_options");
    }
}
```

- [ ] **Step 2: 运行测试确认失败**

Run: `cargo test -p local-apps --all-features questionnaire:: 2>&1 | tail -30`
Expected: 编译失败，`questionnaire` 模块不存在。

- [ ] **Step 3: 写实现**

`questionnaire.rs` 顶部（类型定义 + 校验器主体）：

```rust
//! 问卷与方案的领域类型和校验器。
//!
//! 出题与出方案都由 LLM 完成，本模块是那些输出唯一的验收关卡:
//! LLM 的输出是提议，这里的判定才是事实。纯函数，零 I/O。

use crate::error::AppError;
use crate::manifest::{DataCollectionSchema, DataFieldKind};
use crate::permissions::AppCapability;
use crate::types::DesignValue;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub const MAX_STEPS: usize = 5;
pub const MAX_FIELDS_PER_STEP: usize = 8;
pub const MAX_FIELDS_TOTAL: usize = 24;
pub const MAX_OPTIONS: usize = 12;
pub const MAX_LABEL_CHARS: usize = 80;
pub const MAX_DESCRIPTION_CHARS: usize = 240;
pub const MAX_COLLECTIONS: usize = 8;
pub const MAX_COLLECTION_FIELDS: usize = 24;
pub const MAX_DOMAINS: usize = 8;
pub const MAX_SUMMARY_CHARS: usize = 1200;

/// 动态问卷字段的输入类型。与 `DesignValue` 的变体一一对应。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AppDesignFieldType {
    ShortText,
    LongText,
    SingleChoice,
    MultipleChoice,
    Boolean,
    Color,
    Density,
    ScreenList,
    FeatureList,
    DataFieldList,
    DomainList,
}

impl AppDesignFieldType {
    /// 该类型是否接受这个值变体。用于校验 `default_value` 与用户答案。
    #[must_use]
    pub fn accepts(self, value: &DesignValue) -> bool {
        matches!(
            (self, value),
            (Self::ShortText, DesignValue::ShortText(_))
                | (Self::LongText, DesignValue::LongText(_))
                | (Self::SingleChoice, DesignValue::SingleChoice(_))
                | (Self::MultipleChoice, DesignValue::MultipleChoice(_))
                | (Self::Boolean, DesignValue::Boolean(_))
                | (Self::Color, DesignValue::Color(_))
                | (Self::Density, DesignValue::Density(_))
                | (Self::ScreenList, DesignValue::ScreenList(_))
                | (Self::FeatureList, DesignValue::FeatureList(_))
                | (Self::DataFieldList, DesignValue::DataFieldList(_))
                | (Self::DomainList, DesignValue::DomainList(_))
        )
    }

    fn needs_options(self) -> bool {
        matches!(self, Self::SingleChoice | Self::MultipleChoice)
    }
}

/// 一个可选项。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppDesignFieldOption {
    pub value: String,
    pub label: String,
}

/// 一道题。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppDesignField {
    pub id: String,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub field_type: AppDesignFieldType,
    #[serde(default)]
    pub required: bool,
    /// 渲染 `Other…` 自由文本框。值直接写进对应的文本型 `DesignValue`。
    #[serde(default)]
    pub allows_custom: bool,
    /// 渲染「由你决定」。选中后答案是 `DesignValue::Deferred`。
    #[serde(default)]
    pub allows_defer: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_value: Option<DesignValue>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<AppDesignFieldOption>,
}

/// 问卷的一组题。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppDesignStep {
    pub id: String,
    pub order: u32,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub fields: Vec<AppDesignField>,
}

/// 确认页展示的「将创建」摘要。由 LLM 从答案推导，经本模块校验。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppPlan {
    #[serde(default)]
    pub collections: Vec<DataCollectionSchema>,
    #[serde(default)]
    pub capabilities: Vec<AppCapability>,
    /// 外部 HTTPS 主机名。
    #[serde(default)]
    pub domains: Vec<String>,
    /// 给用户读的一段人话。每个被 defer 的字段最终定成什么，必须写在这里。
    pub summary: String,
}

fn reject(message: impl Into<String>) -> AppError {
    AppError::InvalidRequest(message.into())
}

/// `^[a-z][a-z0-9_]{0,39}$`
fn valid_id(id: &str) -> bool {
    let mut chars = id.chars();
    let Some(first) = chars.next() else { return false };
    if !first.is_ascii_lowercase() {
        return false;
    }
    if id.len() > 40 {
        return false;
    }
    chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

fn ensure_chars(label: &str, text: &str, max: usize) -> Result<(), AppError> {
    if text.chars().count() > max {
        return Err(reject(format!("{label} exceeds {max} characters")));
    }
    Ok(())
}

/// 校验一份 LLM 出的问卷。任一条不过则整份拒收。
pub fn validate_questionnaire(steps: &[AppDesignStep]) -> Result<(), AppError> {
    if steps.is_empty() {
        return Err(reject("questionnaire has no steps"));
    }
    if steps.len() > MAX_STEPS {
        return Err(reject(format!(
            "questionnaire has {} steps, the limit is {MAX_STEPS}",
            steps.len()
        )));
    }
    let mut step_ids = BTreeSet::new();
    let mut field_ids = BTreeSet::new();
    let mut total_fields = 0usize;
    for step in steps {
        if !valid_id(&step.id) {
            return Err(reject(format!("step id `{}` breaks the id grammar", step.id)));
        }
        if !step_ids.insert(step.id.as_str()) {
            return Err(reject(format!("duplicate step id `{}`", step.id)));
        }
        ensure_chars("step title", &step.title, MAX_LABEL_CHARS)?;
        if let Some(description) = &step.description {
            ensure_chars("step description", description, MAX_DESCRIPTION_CHARS)?;
        }
        if step.fields.is_empty() {
            return Err(reject(format!("step `{}` has no fields", step.id)));
        }
        if step.fields.len() > MAX_FIELDS_PER_STEP {
            return Err(reject(format!(
                "step `{}` has {} fields, the limit is {MAX_FIELDS_PER_STEP}",
                step.id,
                step.fields.len()
            )));
        }
        total_fields += step.fields.len();
        for field in &step.fields {
            validate_field(field, &mut field_ids)?;
        }
    }
    if total_fields > MAX_FIELDS_TOTAL {
        return Err(reject(format!(
            "questionnaire has {total_fields} fields, the limit is {MAX_FIELDS_TOTAL}"
        )));
    }
    Ok(())
}

fn validate_field<'a>(
    field: &'a AppDesignField,
    seen: &mut BTreeSet<&'a str>,
) -> Result<(), AppError> {
    if !valid_id(&field.id) {
        return Err(reject(format!("field id `{}` breaks the id grammar", field.id)));
    }
    if !seen.insert(field.id.as_str()) {
        return Err(reject(format!("duplicate field id `{}`", field.id)));
    }
    ensure_chars("field label", &field.label, MAX_LABEL_CHARS)?;
    if let Some(description) = &field.description {
        ensure_chars("field description", description, MAX_DESCRIPTION_CHARS)?;
    }
    if field.field_type.needs_options() && field.options.is_empty() {
        return Err(reject(format!(
            "field `{}` is a choice field with no options",
            field.id
        )));
    }
    if field.options.len() > MAX_OPTIONS {
        return Err(reject(format!(
            "field `{}` has {} options, the limit is {MAX_OPTIONS}",
            field.id,
            field.options.len()
        )));
    }
    let mut option_values = BTreeSet::new();
    for option in &field.options {
        ensure_chars("option label", &option.label, MAX_LABEL_CHARS)?;
        if !option_values.insert(option.value.as_str()) {
            return Err(reject(format!(
                "field `{}` repeats option value `{}`",
                field.id, option.value
            )));
        }
    }
    if let Some(default_value) = &field.default_value {
        if !field.field_type.accepts(default_value) {
            return Err(reject(format!(
                "field `{}` has a default_value whose variant does not match its field_type",
                field.id
            )));
        }
    }
    Ok(())
}

/// 校验用户答案是否可以推进到出方案。
pub fn validate_answers(
    steps: &[AppDesignStep],
    answers: &BTreeMap<String, DesignValue>,
) -> Result<(), AppError> {
    for field in steps.iter().flat_map(|step| step.fields.iter()) {
        match answers.get(&field.id) {
            None => {
                if field.required {
                    return Err(reject(format!("field `{}` is required", field.id)));
                }
            }
            Some(DesignValue::Deferred) => {
                if !field.allows_defer {
                    return Err(reject(format!(
                        "field `{}` cannot be deferred",
                        field.id
                    )));
                }
            }
            Some(value) => {
                if !field.field_type.accepts(value) {
                    return Err(reject(format!(
                        "field `{}` was answered with a value of the wrong kind",
                        field.id
                    )));
                }
            }
        }
    }
    Ok(())
}

/// 校验一份 LLM 出的方案。
pub fn validate_plan(plan: &AppPlan) -> Result<(), AppError> {
    ensure_chars("plan summary", &plan.summary, MAX_SUMMARY_CHARS)?;
    if plan.summary.trim().is_empty() {
        return Err(reject("plan summary is empty"));
    }
    if plan.collections.len() > MAX_COLLECTIONS {
        return Err(reject(format!(
            "plan declares {} collections, the limit is {MAX_COLLECTIONS}",
            plan.collections.len()
        )));
    }
    let mut collection_ids = BTreeSet::new();
    for collection in &plan.collections {
        if !valid_id(&collection.id) {
            return Err(reject(format!(
                "collection id `{}` breaks the id grammar",
                collection.id
            )));
        }
        if !collection_ids.insert(collection.id.as_str()) {
            return Err(reject(format!("duplicate collection id `{}`", collection.id)));
        }
        ensure_chars("collection name", &collection.name, MAX_LABEL_CHARS)?;
        if collection.fields.len() > MAX_COLLECTION_FIELDS {
            return Err(reject(format!(
                "collection `{}` declares {} fields, the limit is {MAX_COLLECTION_FIELDS}",
                collection.id,
                collection.fields.len()
            )));
        }
        let mut field_ids = BTreeSet::new();
        for field in &collection.fields {
            if !valid_id(&field.id) {
                return Err(reject(format!(
                    "collection field id `{}` breaks the id grammar",
                    field.id
                )));
            }
            if !field_ids.insert(field.id.as_str()) {
                return Err(reject(format!(
                    "collection `{}` repeats field id `{}`",
                    collection.id, field.id
                )));
            }
            ensure_chars("collection field label", &field.label, MAX_LABEL_CHARS)?;
            if field.kind == DataFieldKind::Enum && field.enum_options.is_empty() {
                return Err(reject(format!(
                    "enum field `{}` declares no options",
                    field.id
                )));
            }
        }
    }
    if plan.domains.len() > MAX_DOMAINS {
        return Err(reject(format!(
            "plan declares {} domains, the limit is {MAX_DOMAINS}",
            plan.domains.len()
        )));
    }
    for domain in &plan.domains {
        validate_domain(domain)?;
    }
    Ok(())
}

/// 域名必须是可公开解析的主机名。IP 字面量、loopback、私网一律拒绝——
/// 网络桥是给 app 访问外部服务的，不是给它探测设备本地网络的。
fn validate_domain(domain: &str) -> Result<(), AppError> {
    let lowered = domain.to_ascii_lowercase();
    if lowered.is_empty() || lowered.len() > 253 {
        return Err(reject(format!("domain `{domain}` has an invalid length")));
    }
    if lowered.parse::<std::net::IpAddr>().is_ok() {
        return Err(reject(format!("domain `{domain}` is an IP literal")));
    }
    if lowered == "localhost" || lowered.ends_with(".localhost") || lowered.ends_with(".local") {
        return Err(reject(format!("domain `{domain}` is a loopback/link-local name")));
    }
    if !lowered.contains('.') {
        return Err(reject(format!("domain `{domain}` is not a fully qualified host")));
    }
    let label_ok = |label: &str| {
        !label.is_empty()
            && label.len() <= 63
            && !label.starts_with('-')
            && !label.ends_with('-')
            && label
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    };
    if !lowered.split('.').all(label_ok) {
        return Err(reject(format!("domain `{domain}` has an invalid label")));
    }
    Ok(())
}
```

注意 `192.168.1.4` 会被 `parse::<IpAddr>()` 这一关拦下（它是合法 IPv4 字面量），所以私网测试用例走的是 IP 字面量分支，不需要另写私网段判断。

在 `lib.rs` 的 `pub mod permissions;` 后加 `pub mod questionnaire;`，并加 re-export：

```rust
pub use questionnaire::{
    validate_answers, validate_plan, validate_questionnaire, AppDesignField,
    AppDesignFieldOption, AppDesignFieldType, AppDesignStep, AppPlan,
};
```

- [ ] **Step 4: 运行测试确认通过**

Run: `cargo test -p local-apps --all-features questionnaire:: 2>&1 | tail -20`
Expected: 15 passed。`DesignValue::Deferred` 尚不存在会导致编译失败——若如此，先在 `types.rs` 的 `DesignValue` 末尾加一个无载荷变体 `Deferred`（Task 2 会补全它的其余处理），再重跑。

- [ ] **Step 5: 提交**

```bash
git add lingxi-code/local-apps/src/questionnaire.rs lingxi-code/local-apps/src/types.rs lingxi-code/local-apps/src/lib.rs
git commit -m "$(cat <<'EOF'
feat(local-apps): 问卷与方案的领域类型 + 校验器

LLM 出的题和方案唯一的验收关卡。纯函数、零 I/O，
覆盖 id 语法、数量上限、选项唯一性、默认值变体匹配、
required/defer 语义、域名的 IP/loopback 拒绝。

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
EOF
)"
```

---

### Task 2: 删除 AppTemplateKind，draft 三段化

**Files:**
- Modify: `lingxi-code/local-apps/src/types.rs:16-50`（删枚举）、`:141-162`（`AppRecord`）、`:179-204`（`DesignValue`）、`:253-272`（`AppDesignDraft`）
- Modify: `lingxi-code/local-apps/src/manifest.rs:113-135`（`AppManifest::for_new_app`）、`:100`（`AppManifest.template` 字段）
- Modify: `lingxi-code/local-apps/src/state.rs:129-168`（`AppState::create`）
- Modify: `lingxi-code/local-apps/src/storage.rs`（旧数据可读错误）
- Modify: `lingxi-code/local-apps/src/lib.rs`（re-export 去掉 `AppTemplateKind`）

**Interfaces:**
- Consumes: Task 1 的 `AppDesignStep`、`AppPlan`
- Produces:
  - `AppRecord { id, name, brief: String, created_at_ms, updated_at_ms, workflow_state, conversation_id, workspace_rel }`（`template` 字段删除，新增 `brief`）
  - `AppDesignDraft { schema_version, revision, questionnaire: Vec<AppDesignStep>, fields, plan: Option<AppPlan>, plan_for_revision: Option<u64>, pending_suggestion, confirmed_revision }`
  - `DesignValue::Deferred`（无载荷变体）
  - `AppState::create(id: String, name: String, brief: String, conversation_id: Option<String>, now_ms: u64) -> Self`
  - `AppManifest::for_new_app(app_id: impl Into<String>, name: impl Into<String>) -> Self`（`collections` 初始为空，由方案填充）
  - `AppErrorCode::{LlmUnavailable, LlmOutputRejected}` 与对应的 `AppError::{LlmUnavailable(String), LlmOutputRejected(String)}`

- [ ] **Step 1: 写失败测试**

加进 `lingxi-code/local-apps/src/storage.rs` 的 `#[cfg(test)] mod tests`：

```rust
#[tokio::test]
async fn a_template_era_index_reports_a_readable_error() {
    let fs = crate::test_support::memory_fs();
    fs.write_atomic(
        "apps/index.json",
        br#"{"schemaVersion":1,"apps":[{"id":"old","name":"Old","template":"dashboard","createdAtMs":1,"updatedAtMs":1,"workflowState":"ready","workspaceRel":"apps/old/workspace"}]}"#,
    )
    .await
    .expect("seed a template-era index");

    let error = load_index(&fs)
        .await
        .expect_err("a template-era index is not loadable");
    let message = format!("{error}");
    assert!(
        message.contains("不再支持") || message.contains("no longer supports"),
        "the error explains WHY rather than leaking a serde path: {message}"
    );
}
```

加进 `lingxi-code/local-apps/src/manifest.rs` 的测试模块：

```rust
#[test]
fn a_new_app_manifest_declares_no_collections() {
    let manifest = AppManifest::for_new_app("notes", "Notes");
    assert!(
        manifest.collections.is_empty(),
        "collections now come from the LLM plan, not from a template"
    );
}
```

- [ ] **Step 2: 运行测试确认失败**

Run: `cargo test -p local-apps --all-features 2>&1 | tail -30`
Expected: 编译失败——`for_new_app` 仍要求 `template` 参数；`load_index` 对旧 JSON 仍返回 serde 原始错误。

- [ ] **Step 3: 写实现**

`types.rs`：删除 `AppTemplateKind` 枚举及其 `as_str` / `Display` impl（`:16-50` 整段）。`AppRecord` 的 `template: AppTemplateKind` 替换为：

```rust
    /// 用户创建时给出的一句话描述。出题、出方案、写码三段都要读它。
    /// 只存这一份——列表页要展示、出题失败要重试、`generate_source` 已经
    /// 在调 `service.record()`。存两份必然分叉。
    pub brief: String,
```

`DesignValue` 末尾加：

```rust
    /// 用户明确选择「由你决定」。与「没作答」是两回事：确认门放行前者、
    /// 拦住后者，出方案的那次 LLM 调用负责给它定值。
    Deferred,
```

`AppDesignDraft` 改为：

```rust
pub struct AppDesignDraft {
    pub schema_version: u32,
    pub revision: u64,
    /// LLM 出的题。authoring 成功后不可变。
    #[serde(default)]
    pub questionnaire: Vec<crate::questionnaire::AppDesignStep>,
    #[serde(default)]
    pub fields: BTreeMap<String, DesignValue>,
    /// 确认页展示的「将创建」摘要。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan: Option<crate::questionnaire::AppPlan>,
    /// `plan` 是针对哪个 revision 算出来的。答案一改就作废——
    /// 抄的是 `AppDesignSuggestion.based_on_revision` 那套防陈旧机制。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan_for_revision: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_suggestion: Option<AppDesignSuggestion>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confirmed_revision: Option<u64>,
}
```

`manifest.rs`：删 `AppManifest.template` 字段，`for_new_app` 改为：

```rust
    /// 新建应用的初始原生契约。集合此时为空——它们由 LLM 出的方案填充，
    /// 经 `questionnaire::validate_plan` 校验后写入。
    #[must_use]
    pub fn for_new_app(app_id: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            schema_version: crate::types::APPS_SCHEMA_VERSION,
            app_id: app_id.into(),
            revision: 0,
            name: name.into(),
            collections: Vec::new(),
            allowed_domains: Vec::new(),
        }
    }
```

`state.rs` 的 `AppState::create`：`template: AppTemplateKind` 参数换成 `brief: String`，`record` 里 `template` 换成 `brief`，`draft` 里去掉 `template`、加 `questionnaire: Vec::new()`、`plan: None`、`plan_for_revision: None`。

`storage.rs` 的索引读取路径，在 serde 失败时包一层可读错误：

```rust
        serde_json::from_slice::<AppIndexFile>(&bytes).map_err(|error| {
            if bytes.windows(10).any(|w| w == b"\"template\"") {
                AppError::StorageCorrupt(
                    "此版本不再支持模版时代的 app 记录（apps/index.json 含 template 字段）；\
                     请删除 apps/ 目录后重新创建应用 / no longer supports template-era app records"
                        .into(),
                )
            } else {
                AppError::StorageCorrupt(format!("parse apps/index.json: {error}"))
            }
        })
```

`error.rs`：新增两个错误码与两个错误变体。界面要能区分「模型够不到」（提示联网/配模型）和「模型给的东西不合格」（提示重试），两者揉进 `Io` 就分不开了：

```rust
    /// 模型不可达：离线、鉴权失败、超时。
    LlmUnavailable,
    /// 模型返回的结构不合法或越过上限。
    LlmOutputRejected,
```

```rust
    /// 模型不可达。没有模版可以兜底——本设计刻意不留静默降级路径。
    #[error("llm unavailable: {0}")]
    LlmUnavailable(String),
    /// 模型输出被校验器拒收。
    #[error("llm output rejected: {0}")]
    LlmOutputRejected(String),
```

`AppError::code()` 的 `match` 补上这两条映射。

其余编译错误由编译器逐个指出——`AppTemplateKind` 的每个引用点都是必须处理的清单。测试里的 `create_app("X", AppTemplateKind::Dashboard, None)` 统一改为 `create_app("X", "a test app", None)`。

- [ ] **Step 4: 运行测试确认通过**

Run: `cargo test -p local-apps --all-features --no-fail-fast 2>&1 | tail -30`
Expected: 全绿。**记下总测试数**——下一个 task 结束时它只能增不能减。

- [ ] **Step 5: 提交**

```bash
git add lingxi-code/local-apps/src
git commit -m "$(cat <<'EOF'
feat(local-apps)!: 删除 AppTemplateKind，draft 升级为问卷+答案+方案

AppRecord.template -> brief（只存一份，在 record 上）。
AppDesignDraft 新增 questionnaire / plan / plan_for_revision。
DesignValue 新增 Deferred，区分「没作答」与「由你决定」。
AppManifest 不再按模版预置集合。

BREAKING: 模版时代的 apps/index.json 不再可读，storage 层给出
可读错误而非 serde 原始报错。

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
EOF
)"
```

---

### Task 3: 状态机的四个新状态

**Files:**
- Modify: `lingxi-code/local-apps/src/types.rs:52-99`（`AppWorkflowState`）
- Modify: `lingxi-code/local-apps/src/state.rs:21`（`DRAFT_EDITABLE_STATES`）、`:129`（create 的初始态）、新增转移方法

**Interfaces:**
- Consumes: Task 2 的 `AppState`
- Produces（全部在 `impl AppState` 上）：
  - `AppWorkflowState::{AuthoringQuestionnaire, QuestionnaireFailed, Planning, PlanFailed}`
  - `pub fn questionnaire_ready(&mut self, steps: Vec<AppDesignStep>, name: Option<String>, now_ms: u64) -> Result<(), AppError>`
  - `pub fn questionnaire_failed(&mut self, now_ms: u64) -> Result<(), AppError>`
  - `pub fn retry_questionnaire(&mut self, now_ms: u64) -> Result<(), AppError>`
  - `pub fn update_brief(&mut self, brief: String, now_ms: u64) -> Result<(), AppError>`
  - `pub fn begin_planning(&mut self, now_ms: u64) -> Result<(), AppError>`
  - `pub fn plan_ready(&mut self, plan: AppPlan, interaction_id: String, now_ms: u64) -> Result<AppInteractionRequest, AppError>`
  - `pub fn plan_failed(&mut self, now_ms: u64) -> Result<(), AppError>`
  - `pub fn retry_plan(&mut self, now_ms: u64) -> Result<(), AppError>`

- [ ] **Step 1: 写失败测试**

加进 `state.rs` 的测试模块：

```rust
fn authoring_app() -> AppState {
    AppState::create("notes".into(), "Notes".into(), "一个记事本".into(), None, 1)
}

fn one_step() -> Vec<AppDesignStep> {
    vec![AppDesignStep {
        id: "basics".into(),
        order: 0,
        title: "基础".into(),
        description: None,
        fields: vec![AppDesignField {
            id: "tone".into(),
            label: "语气".into(),
            description: None,
            field_type: AppDesignFieldType::SingleChoice,
            required: false,
            allows_custom: false,
            allows_defer: false,
            default_value: None,
            options: vec![AppDesignFieldOption { value: "a".into(), label: "A".into() }],
        }],
    }]
}

fn a_plan() -> AppPlan {
    AppPlan { collections: Vec::new(), capabilities: Vec::new(), domains: Vec::new(), summary: "s".into() }
}

#[test]
fn a_new_app_starts_in_authoring_questionnaire() {
    assert_eq!(authoring_app().record.workflow_state, AppWorkflowState::AuthoringQuestionnaire);
}

#[test]
fn questionnaire_ready_moves_to_collecting_spec_and_stores_the_steps() {
    let mut app = authoring_app();
    app.questionnaire_ready(one_step(), Some("记事本".into()), 2).expect("authoring succeeds");
    assert_eq!(app.record.workflow_state, AppWorkflowState::CollectingSpec);
    assert_eq!(app.draft.questionnaire.len(), 1);
    assert_eq!(app.record.name, "记事本", "a suggested name replaces the placeholder");
}

#[test]
fn a_failed_authoring_retry_returns_to_authoring_not_to_collecting_spec() {
    let mut app = authoring_app();
    app.questionnaire_failed(2).expect("authoring can fail");
    assert_eq!(app.record.workflow_state, AppWorkflowState::QuestionnaireFailed);
    app.retry_questionnaire(3).expect("a failed authoring can be retried");
    assert_eq!(
        app.record.workflow_state,
        AppWorkflowState::AuthoringQuestionnaire,
        "retry re-runs authoring; it does not skip ahead"
    );
}

#[test]
fn updating_the_brief_clears_the_questionnaire_answers_and_plan() {
    let mut app = authoring_app();
    app.questionnaire_ready(one_step(), None, 2).expect("authoring succeeds");
    app.draft.fields.insert("tone".into(), DesignValue::SingleChoice("a".into()));
    app.draft.plan = Some(a_plan());
    app.draft.plan_for_revision = Some(app.draft.revision);

    app.update_brief("换成一个待办清单".into(), 3).expect("brief is editable while collecting");

    assert_eq!(app.record.brief, "换成一个待办清单");
    assert!(app.draft.questionnaire.is_empty(), "the questionnaire must be re-authored");
    assert!(app.draft.fields.is_empty(), "old answers reference field ids that no longer exist");
    assert!(app.draft.plan.is_none());
    assert_eq!(app.record.workflow_state, AppWorkflowState::AuthoringQuestionnaire);
}

#[test]
fn the_brief_is_not_editable_once_generation_has_been_confirmed() {
    let mut app = authoring_app();
    app.questionnaire_ready(one_step(), None, 2).expect("authoring succeeds");
    app.begin_planning(3).expect("planning starts");
    app.plan_ready(a_plan(), "i-1".into(), 4).expect("planning succeeds");
    app.confirm_design("i-1", app.draft.revision, 5).expect("the user confirms");
    app.update_brief("太晚了".into(), 6).expect_err("the brief is frozen after confirmation");
}

#[test]
fn plan_ready_opens_the_spec_confirmation_gate_and_stamps_the_revision() {
    let mut app = authoring_app();
    app.questionnaire_ready(one_step(), None, 2).expect("authoring succeeds");
    app.begin_planning(3).expect("planning starts");
    let interaction = app.plan_ready(a_plan(), "i-1".into(), 4).expect("planning succeeds");

    assert_eq!(app.record.workflow_state, AppWorkflowState::AwaitingSpecConfirmation);
    assert_eq!(interaction.kind, AppInteractionKind::Designer);
    assert_eq!(app.draft.plan_for_revision, Some(app.draft.revision));
}

#[test]
fn confirming_a_plan_computed_for_an_older_revision_is_refused() {
    let mut app = authoring_app();
    app.questionnaire_ready(one_step(), None, 2).expect("authoring succeeds");
    app.begin_planning(3).expect("planning starts");
    app.plan_ready(a_plan(), "i-1".into(), 4).expect("planning succeeds");

    // 用户回头改了一个答案：revision 前进，方案作废。
    app.draft.revision += 1;

    let error = app
        .confirm_design("i-1", app.draft.revision, 5)
        .expect_err("a stale plan must never be confirmed");
    assert!(matches!(error, AppError::RevisionConflict { .. }), "got {error}");
}

#[test]
fn a_failed_plan_retry_returns_to_planning() {
    let mut app = authoring_app();
    app.questionnaire_ready(one_step(), None, 2).expect("authoring succeeds");
    app.begin_planning(3).expect("planning starts");
    app.plan_failed(4).expect("planning can fail");
    assert_eq!(app.record.workflow_state, AppWorkflowState::PlanFailed);
    app.retry_plan(5).expect("a failed plan can be retried");
    assert_eq!(app.record.workflow_state, AppWorkflowState::Planning);
}

#[test]
fn the_draft_is_read_only_while_the_llm_is_authoring_or_planning() {
    let patch = AppDesignPatch {
        ops: vec![AppDesignPatchOp::Set {
            field_id: "tone".into(),
            value: DesignValue::SingleChoice("a".into()),
        }],
        note: None,
    };

    let mut app = authoring_app();
    app.update_draft(0, &patch, 2)
        .expect_err("no edits while authoring — the answers would race the questions");

    let mut app = authoring_app();
    app.questionnaire_ready(one_step(), None, 2).expect("authoring succeeds");
    app.begin_planning(3).expect("planning starts");
    app.update_draft(app.draft.revision, &patch, 4)
        .expect_err("no edits while planning — the plan would be computed against stale answers");
}
```

- [ ] **Step 2: 运行测试确认失败**

Run: `cargo test -p local-apps --all-features state:: 2>&1 | tail -30`
Expected: 编译失败，四个新状态与六个新方法都不存在。

- [ ] **Step 3: 写实现**

`types.rs` 的 `AppWorkflowState` 在 `CollectingSpec` 之前插入两个、在它之后插入两个，并补 `as_str`：

```rust
    /// LLM 正在为这次 brief 出题。设计器只读。
    AuthoringQuestionnaire,
    /// 出题失败；可重试或改 brief 重来。
    QuestionnaireFailed,
    /// LLM 正在从答案推导方案。设计器只读。
    Planning,
    /// 出方案失败；可重试。
    PlanFailed,
```

`as_str` 分别返回 `"authoring_questionnaire"` / `"questionnaire_failed"` / `"planning"` / `"plan_failed"`。

`state.rs`：`DRAFT_EDITABLE_STATES` 保持只含 `CollectingSpec` / `AwaitingSpecConfirmation` / `GenerationFailed` 三个（**不要**把新态加进去——`AuthoringQuestionnaire` 与 `Planning` 期间必须只读）。`AppState::create` 的初始 `workflow_state` 改为 `AppWorkflowState::AuthoringQuestionnaire`。

新增转移：

```rust
    /// `authoring_questionnaire -> collecting_spec`，落盘问卷；`name` 是
    /// LLM 建议的正式名，用来替换创建时的占位名。
    pub fn questionnaire_ready(
        &mut self,
        steps: Vec<AppDesignStep>,
        name: Option<String>,
        now_ms: u64,
    ) -> Result<(), AppError> {
        self.ensure_workflow(
            "questionnaire_ready",
            &[AppWorkflowState::AuthoringQuestionnaire],
        )?;
        crate::questionnaire::validate_questionnaire(&steps)?;
        self.draft.questionnaire = steps;
        if let Some(name) = name {
            let trimmed = name.trim();
            if !trimmed.is_empty() {
                self.record.name = trimmed.to_string();
            }
        }
        self.set_workflow(AppWorkflowState::CollectingSpec, now_ms);
        Ok(())
    }

    /// `authoring_questionnaire -> questionnaire_failed`.
    pub fn questionnaire_failed(&mut self, now_ms: u64) -> Result<(), AppError> {
        self.ensure_workflow(
            "questionnaire_failed",
            &[AppWorkflowState::AuthoringQuestionnaire],
        )?;
        self.set_workflow(AppWorkflowState::QuestionnaireFailed, now_ms);
        Ok(())
    }

    /// `questionnaire_failed -> authoring_questionnaire`. 重试回到执行态，
    /// 不是跳过它。
    pub fn retry_questionnaire(&mut self, now_ms: u64) -> Result<(), AppError> {
        self.ensure_workflow(
            "retry_questionnaire",
            &[AppWorkflowState::QuestionnaireFailed],
        )?;
        self.set_workflow(AppWorkflowState::AuthoringQuestionnaire, now_ms);
        Ok(())
    }

    /// 改 brief 并重新出题。旧答案的 field id 在新问卷里已不存在，
    /// 保留它们只会让后续校验对着幽灵字段报错——一并清掉。
    pub fn update_brief(&mut self, brief: String, now_ms: u64) -> Result<(), AppError> {
        self.ensure_workflow(
            "update_brief",
            &[
                AppWorkflowState::CollectingSpec,
                AppWorkflowState::QuestionnaireFailed,
            ],
        )?;
        let trimmed = brief.trim();
        if trimmed.is_empty() {
            return Err(AppError::InvalidRequest("brief is empty".into()));
        }
        self.record.brief = trimmed.to_string();
        self.draft.questionnaire.clear();
        self.draft.fields.clear();
        self.draft.plan = None;
        self.draft.plan_for_revision = None;
        self.draft.pending_suggestion = None;
        self.draft.revision += 1;
        self.set_workflow(AppWorkflowState::AuthoringQuestionnaire, now_ms);
        Ok(())
    }

    /// `collecting_spec -> planning`。先确认答案自洽，别拿一份残缺答案
    /// 去换一次 LLM 往返。
    pub fn begin_planning(&mut self, now_ms: u64) -> Result<(), AppError> {
        self.ensure_workflow("begin_planning", &[AppWorkflowState::CollectingSpec])?;
        crate::questionnaire::validate_answers(&self.draft.questionnaire, &self.draft.fields)?;
        self.set_workflow(AppWorkflowState::Planning, now_ms);
        Ok(())
    }

    /// `planning -> awaiting_spec_confirmation`，落盘方案并开确认门。
    pub fn plan_ready(
        &mut self,
        plan: AppPlan,
        interaction_id: String,
        now_ms: u64,
    ) -> Result<AppInteractionRequest, AppError> {
        self.ensure_workflow("plan_ready", &[AppWorkflowState::Planning])?;
        crate::questionnaire::validate_plan(&plan)?;
        self.draft.plan = Some(plan);
        self.draft.plan_for_revision = Some(self.draft.revision);
        let interaction = AppInteractionRequest {
            interaction_id,
            app_id: self.record.id.clone(),
            kind: AppInteractionKind::Designer,
            revision: self.draft.revision,
            created_at_ms: now_ms,
        };
        self.interactions.pending = Some(interaction.clone());
        self.set_workflow(AppWorkflowState::AwaitingSpecConfirmation, now_ms);
        Ok(interaction)
    }

    /// `planning -> plan_failed`.
    pub fn plan_failed(&mut self, now_ms: u64) -> Result<(), AppError> {
        self.ensure_workflow("plan_failed", &[AppWorkflowState::Planning])?;
        self.set_workflow(AppWorkflowState::PlanFailed, now_ms);
        Ok(())
    }

    /// `plan_failed -> planning`.
    pub fn retry_plan(&mut self, now_ms: u64) -> Result<(), AppError> {
        self.ensure_workflow("retry_plan", &[AppWorkflowState::PlanFailed])?;
        self.set_workflow(AppWorkflowState::Planning, now_ms);
        Ok(())
    }
```

`confirm_design`（`state.rs:404`）开头加一道方案新鲜度检查，放在既有的 `ensure_current_revision` 之后：

```rust
        if self.draft.plan_for_revision != Some(self.draft.revision) {
            return Err(AppError::RevisionConflict {
                expected: self.draft.plan_for_revision.unwrap_or_default(),
                actual: self.draft.revision,
            });
        }
```

`update_draft`（`state.rs:287`）在 `self.draft.revision += 1;` 之后清方案：

```rust
        // 答案变了，方案就不再是对这份答案的方案。
        self.draft.plan = None;
        self.draft.plan_for_revision = None;
```

`open_designer`（`state.rs:259`）的允许态加上 `AppWorkflowState::QuestionnaireFailed` 与 `AppWorkflowState::PlanFailed`，让失败后还能回到设计器。

- [ ] **Step 4: 运行测试确认通过**

Run: `cargo test -p local-apps --all-features --no-fail-fast 2>&1 | tail -20`
Expected: 全绿，且总数 ≥ Task 2 记下的数字。

- [ ] **Step 5: 提交**

```bash
git add lingxi-code/local-apps/src
git commit -m "$(cat <<'EOF'
feat(local-apps): 状态机新增出题/出方案四态

authoring_questionnaire / questionnaire_failed / planning / plan_failed。
两个失败态的重试回到各自执行态而非跳过它。
LLM 往返期间 draft 只读，避免答案与题目/方案竞态。
confirm_design 增加方案新鲜度门：plan_for_revision 必须等于当前 revision。

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
EOF
)"
```

---

### Task 4: AppService 的新方法

**Files:**
- Modify: `lingxi-code/local-apps/src/service.rs:855`（`create_app`）、`:1023` 后新增方法
- Modify: `lingxi-code/local-apps/src/events.rs`（新增事件变体）

**Interfaces:**
- Consumes: Task 3 的全部 `AppState` 转移
- Produces:
  - `pub async fn create_app(&self, name: Option<&str>, brief: &str, conversation_id: Option<String>) -> Result<AppRecord, AppError>`
  - `pub async fn questionnaire_ready(&self, app_id: &str, steps: Vec<AppDesignStep>, name: Option<String>) -> Result<(), AppError>`
  - `pub async fn questionnaire_failed(&self, app_id: &str, reason: &str) -> Result<(), AppError>`
  - `pub async fn retry_questionnaire(&self, app_id: &str) -> Result<(), AppError>`
  - `pub async fn update_brief(&self, app_id: &str, brief: &str) -> Result<(), AppError>`
  - `pub async fn begin_planning(&self, app_id: &str) -> Result<(), AppError>`
  - `pub async fn plan_ready(&self, app_id: &str, plan: AppPlan) -> Result<AppInteractionRequest, AppError>`
  - `pub async fn plan_failed(&self, app_id: &str, reason: &str) -> Result<(), AppError>`
  - `pub async fn retry_plan(&self, app_id: &str) -> Result<(), AppError>`
  - `AppEvent::QuestionnaireChanged { app_id, revision, steps }`、`AppEvent::PlanChanged { app_id, revision, plan }`

- [ ] **Step 1: 写失败测试**

加进 `service.rs` 的测试模块：

```rust
#[tokio::test]
async fn create_app_without_a_name_uses_a_placeholder_until_the_llm_suggests_one() {
    let service = test_service().await;
    let record = service
        .create_app(None, "一个记事本 app", None)
        .await
        .expect("brief alone is enough to create");
    assert_eq!(record.brief, "一个记事本 app");
    assert!(!record.name.trim().is_empty(), "a placeholder name is always present");
    assert_eq!(record.workflow_state, AppWorkflowState::AuthoringQuestionnaire);
}

#[tokio::test]
async fn create_app_rejects_an_empty_brief() {
    let service = test_service().await;
    service
        .create_app(Some("Notes"), "   ", None)
        .await
        .expect_err("an empty brief cannot drive authoring");
}

#[tokio::test]
async fn a_stored_questionnaire_survives_a_reload() {
    let service = test_service().await;
    let record = service.create_app(None, "一个记事本", None).await.expect("create");
    service
        .questionnaire_ready(&record.id, one_step(), Some("记事本".into()))
        .await
        .expect("authoring succeeds");

    let reloaded = reload_service(&service).await;
    let draft = reloaded.draft(&record.id).await.expect("draft is readable");
    assert_eq!(draft.questionnaire.len(), 1);
    assert_eq!(
        reloaded.record(&record.id).await.expect("record").name,
        "记事本"
    );
}

#[tokio::test]
async fn a_rejected_questionnaire_leaves_the_app_in_authoring() {
    let service = test_service().await;
    let record = service.create_app(None, "一个记事本", None).await.expect("create");

    // 6 个 step 超上限，校验器必须拒收，且状态不能前进。
    let too_many: Vec<_> = (0..6).map(|i| step_named(&format!("s{i}"), i)).collect();
    service
        .questionnaire_ready(&record.id, too_many, None)
        .await
        .expect_err("an over-limit questionnaire is rejected");

    assert_eq!(
        service.record(&record.id).await.expect("record").workflow_state,
        AppWorkflowState::AuthoringQuestionnaire,
        "a rejected questionnaire must not advance the workflow"
    );
}

#[tokio::test]
async fn changing_the_brief_after_a_failure_reauthors_from_scratch() {
    let service = test_service().await;
    let record = service.create_app(None, "一个记事本", None).await.expect("create");
    service.questionnaire_failed(&record.id, "model offline").await.expect("fail");
    service.update_brief(&record.id, "改成一个待办清单").await.expect("brief is editable");

    let refreshed = service.record(&record.id).await.expect("record");
    assert_eq!(refreshed.brief, "改成一个待办清单");
    assert_eq!(refreshed.workflow_state, AppWorkflowState::AuthoringQuestionnaire);
}
```

若测试模块尚无 `test_service` / `reload_service` / `step_named` 助手，按该模块既有的构造方式补：`test_service` 用 `crate::test_support` 的内存 fs 建一个 `AppService`，`reload_service` 用同一 fs 重新 `AppService::load`，`step_named(id, order)` 返回一个含单个 `SingleChoice` 字段（选项非空、field id 为 `format!("{id}_f")`）的 `AppDesignStep`。

- [ ] **Step 2: 运行测试确认失败**

Run: `cargo test -p local-apps --all-features service:: 2>&1 | tail -30`
Expected: 编译失败，`create_app` 仍是三参数模版签名。

- [ ] **Step 3: 写实现**

`create_app` 改签名。`name` 为 `None` 或空白时用 brief 的前 24 个字符（按 `char` 截断，不是按字节——中文 brief 按字节截会切出非法 UTF-8）作占位名：

```rust
    pub async fn create_app(
        &self,
        name: Option<&str>,
        brief: &str,
        conversation_id: Option<String>,
    ) -> Result<AppRecord, AppError> {
        let brief = brief.trim();
        if brief.is_empty() {
            return Err(AppError::InvalidRequest("brief is empty".into()));
        }
        ensure_within("brief", brief.len(), MAX_PROMPT_BYTES)?;
        let name = name
            .map(str::trim)
            .filter(|candidate| !candidate.is_empty())
            .map_or_else(|| brief.chars().take(24).collect::<String>(), str::to_string);
        // …既有的 id 生成 / 落盘 / 事件逻辑保持不变，
        // 只把 AppState::create 的 template 实参换成 brief.to_string()
    }
```

其余方法一律走该文件既有的 `workflow_step` 包装，与 `begin_revision`（`service.rs:1286`）同形：

```rust
    /// 落盘 LLM 出的问卷（`authoring_questionnaire -> collecting_spec`）。
    pub async fn questionnaire_ready(
        &self,
        app_id: &str,
        steps: Vec<AppDesignStep>,
        name: Option<String>,
    ) -> Result<(), AppError> {
        self.workflow_step(app_id, None, move |app, now| {
            app.questionnaire_ready(steps.clone(), name.clone(), now)
        })
        .await
    }

    /// 出题失败（`-> questionnaire_failed`）。`reason` 进事件供界面展示。
    pub async fn questionnaire_failed(&self, app_id: &str, reason: &str) -> Result<(), AppError> {
        let reason = reason.to_string();
        self.workflow_step(app_id, Some(&reason), AppState::questionnaire_failed)
            .await
    }

    /// 重试出题（`questionnaire_failed -> authoring_questionnaire`）。
    pub async fn retry_questionnaire(&self, app_id: &str) -> Result<(), AppError> {
        self.workflow_step(app_id, None, AppState::retry_questionnaire).await
    }

    /// 改 brief 并重新出题。会丢弃问卷、答案与方案。
    pub async fn update_brief(&self, app_id: &str, brief: &str) -> Result<(), AppError> {
        ensure_within("brief", brief.len(), MAX_PROMPT_BYTES)?;
        let brief = brief.to_string();
        self.workflow_step(app_id, None, move |app, now| {
            app.update_brief(brief.clone(), now)
        })
        .await
    }

    /// `collecting_spec -> planning`。
    pub async fn begin_planning(&self, app_id: &str) -> Result<(), AppError> {
        self.workflow_step(app_id, None, AppState::begin_planning).await
    }

    /// 落盘方案并开设计确认门（`planning -> awaiting_spec_confirmation`）。
    pub async fn plan_ready(
        &self,
        app_id: &str,
        plan: AppPlan,
    ) -> Result<AppInteractionRequest, AppError> {
        let interaction_id = ids::generate_interaction_id();
        self.with_app(app_id, move |app, now| {
            match app.plan_ready(plan.clone(), interaction_id.clone(), now) {
                Ok(interaction) => {
                    let events = vec![
                        AppEvent::WorkflowChanged {
                            app_id: app.record.id.clone(),
                            state: app.record.workflow_state,
                        },
                        AppEvent::PlanChanged {
                            app_id: app.record.id.clone(),
                            revision: app.draft.revision,
                            plan: app.draft.plan.clone(),
                        },
                        AppEvent::InteractionRequested {
                            interaction: interaction.clone(),
                        },
                    ];
                    Ok((interaction, events))
                }
                Err(error) => Err(error),
            }
        })
        .await
    }

    /// 出方案失败（`-> plan_failed`）。
    pub async fn plan_failed(&self, app_id: &str, reason: &str) -> Result<(), AppError> {
        let reason = reason.to_string();
        self.workflow_step(app_id, Some(&reason), AppState::plan_failed).await
    }

    /// 重试出方案（`plan_failed -> planning`）。
    pub async fn retry_plan(&self, app_id: &str) -> Result<(), AppError> {
        self.workflow_step(app_id, None, AppState::retry_plan).await
    }
```

`plan_ready` 的闭包返回形状（`(值, Vec<AppEvent>)`）与事件变体名以 `open_designer`（`service.rs:1023-1046`）为准——若那里的门事件不叫 `InteractionRequested`，照它的实际名字改，不要新造一个。

`events.rs` 加两个变体（字段命名与该文件既有变体保持一致）：

```rust
    /// 问卷已就绪或被清空。
    QuestionnaireChanged {
        app_id: String,
        revision: u64,
        steps: Vec<crate::questionnaire::AppDesignStep>,
    },
    /// 方案已就绪或被作废（`plan: None`）。
    PlanChanged {
        app_id: String,
        revision: u64,
        plan: Option<crate::questionnaire::AppPlan>,
    },
```

- [ ] **Step 4: 运行测试确认通过**

Run: `cargo test -p local-apps --all-features --no-fail-fast 2>&1 | tail -20`
Expected: 全绿。

- [ ] **Step 5: 提交**

```bash
git add lingxi-code/local-apps/src
git commit -m "$(cat <<'EOF'
feat(local-apps): AppService 的出题/出方案/改 brief 方法

create_app(name: Option, brief) —— 一句话即可创建，名字可由 LLM 后补。
新增 questionnaire_ready/failed/retry、update_brief、
begin_planning、plan_ready/failed/retry，以及两个领域事件。

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
EOF
)"
```

---

### Task 5: client-protocol DTO

**Files:**
- Modify: `lingxi-code/client-protocol/src/local_apps.rs:47`（删 `AppTemplateKindDto`）、`:253`（`AppDesignFieldDto`）、`:280`（删 `AppTemplateDto`）、`:293`（`AppRecordDto`）、`:324`（`DesignValueDto`）、`:537`（`AppDetailsDto`）、`:681`（`AppEventDto`）
- Modify: `lingxi-code/client-protocol/tests/version_guard_test.rs:1054-1059, 1256, 1520, 1785`

**Interfaces:**
- Consumes: Task 4 的领域事件
- Produces:
  - `AppDesignFieldDto` 新增 `allows_custom: bool`、`allows_defer: bool`
  - `DesignValueDto::Deferred`
  - `AppPlanDto { collections: Vec<AppDataCollectionDto>, capabilities: Vec<AppCapabilityKindDto>, domains: Vec<String>, summary: String }`
  - `AppRecordDto`：删 `template`，加 `brief: String`
  - `AppDetailsDto`：加 `questionnaire: Vec<AppDesignStepDto>`、`plan: Option<AppPlanDto>`
  - `AppEventDto::{AppQuestionnaireChanged, AppPlanChanged}`
  - `AppsListResponse` 删除 `templates` 字段

- [ ] **Step 1: 写失败测试**

在 `version_guard_test.rs` 的字段表里，删掉 `AppTemplateDto.*` 与 `AppRecordDto.template` 条目，并加入：

```rust
    put("AppRecordDto.brief", "String");
    put("AppDesignFieldDto.allows_custom", "bool");
    put("AppDesignFieldDto.allows_defer", "bool");
    put("AppPlanDto.collections", "Vec<AppDataCollectionDto>");
    put("AppPlanDto.capabilities", "Vec<AppCapabilityKindDto>");
    put("AppPlanDto.domains", "Vec<String>");
    put("AppPlanDto.summary", "String");
    put("AppDetailsDto.questionnaire", "Vec<AppDesignStepDto>");
    put("AppDetailsDto.plan", "Option<AppPlanDto>");
```

`AppErrorCodeDto`（`:128`）加 `LlmUnavailable` 与 `LlmOutputRejected`，线值 `"llm_unavailable"` / `"llm_output_rejected"`，并在该枚举的 guard 条目里登记。

并新增一个断言，锁住 `Deferred` 的线格式：

```rust
#[test]
fn deferred_design_value_serialises_as_a_bare_tagged_variant() {
    let json = serde_json::to_value(DesignValueDto::Deferred).expect("serialise");
    assert_eq!(
        json,
        serde_json::json!({ "kind": "deferred" }),
        "Deferred carries no payload; the tag alone must round-trip"
    );
    let back: DesignValueDto = serde_json::from_value(json).expect("deserialise");
    assert_eq!(back, DesignValueDto::Deferred);
}
```

- [ ] **Step 2: 运行测试确认失败**

Run: `cargo test -p client-protocol --all-features 2>&1 | tail -30`
Expected: 编译失败，`AppPlanDto` / `DesignValueDto::Deferred` 不存在。

- [ ] **Step 3: 写实现**

删除 `AppTemplateKindDto`（`:47-64`）与 `AppTemplateDto`（`:277-287`）整段。`AppDesignFieldDto` 的 `options` 之前插入：

```rust
    /// 渲染 `Other…` 自由文本框。
    #[serde(default)]
    pub allows_custom: bool,
    /// 渲染「由你决定」。
    #[serde(default)]
    pub allows_defer: bool,
```

`DesignValueDto` 末尾加 `Deferred,`。`AppRecordDto` 的 `template` 换成：

```rust
    /// 用户创建时给出的一句话描述。
    pub brief: String,
```

新增：

```rust
/// 确认页展示的「将创建」摘要。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
pub struct AppPlanDto {
    pub collections: Vec<AppDataCollectionDto>,
    pub capabilities: Vec<AppCapabilityKindDto>,
    pub domains: Vec<String>,
    pub summary: String,
}
```

`AppDetailsDto` 加 `questionnaire: Vec<AppDesignStepDto>` 与 `plan: Option<AppPlanDto>`。`AppEventDto` 加：

```rust
    AppQuestionnaireChanged {
        app_id: String,
        revision: u64,
        steps: Vec<AppDesignStepDto>,
    },
    AppPlanChanged {
        app_id: String,
        revision: u64,
        plan: Option<AppPlanDto>,
    },
```

`AppDataCollectionDto.enabled_by_default` 的 doc comment 里 "Whether this template enables…" 改为 "Whether the plan enables this collection by default."。`AppsListResponse`（`:1256` 附近引用的那个类型）删掉 `templates` 字段。

- [ ] **Step 4: 运行测试确认通过**

Run: `cargo test -p client-protocol --all-features --no-fail-fast 2>&1 | tail -20`
Expected: 全绿。

- [ ] **Step 5: 提交**

```bash
git add lingxi-code/client-protocol
git commit -m "$(cat <<'EOF'
feat(client-protocol)!: DTO 去模版化，新增问卷/方案线格式

删 AppTemplateKindDto / AppTemplateDto / AppRecordDto.template
及 list 响应的 templates。新增 AppPlanDto、AppRecordDto.brief、
AppDesignFieldDto.allows_custom/allows_defer、DesignValueDto::Deferred
以及两个事件变体。version_guard 同步更新。

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
EOF
)"
```

---

### Task 6: engine-mobile bridge 映射

**Files:**
- Modify: `lingxi-code/apps/engine-mobile/src/local_apps_bridge.rs:37` 起的映射函数

**Interfaces:**
- Consumes: Task 4 的领域类型、Task 5 的 DTO
- Produces: 领域 ↔ DTO 的双向映射，含 `AppDesignStep`↔`AppDesignStepDto`、`AppPlan`↔`AppPlanDto`、`DesignValue::Deferred`↔`DesignValueDto::Deferred`

- [ ] **Step 1: 写失败测试**

加进 `local_apps_bridge.rs` 的测试模块：

```rust
#[test]
fn a_questionnaire_round_trips_through_the_wire_types() {
    let step = AppDesignStep {
        id: "basics".into(),
        order: 0,
        title: "基础".into(),
        description: Some("说明".into()),
        fields: vec![AppDesignField {
            id: "tone".into(),
            label: "语气".into(),
            description: None,
            field_type: AppDesignFieldType::MultipleChoice,
            required: true,
            allows_custom: true,
            allows_defer: true,
            default_value: None,
            options: vec![AppDesignFieldOption { value: "a".into(), label: "A".into() }],
        }],
    };
    let dto = design_step_to_dto(&step);
    assert!(dto.fields[0].allows_custom, "allows_custom must survive the wire");
    assert!(dto.fields[0].allows_defer, "allows_defer must survive the wire");
    assert_eq!(design_step_from_dto(&dto), step);
}

#[test]
fn deferred_round_trips_through_the_wire_types() {
    let dto = design_value_to_dto(&DesignValue::Deferred);
    assert_eq!(dto, DesignValueDto::Deferred);
    assert_eq!(design_value_from_dto(&dto), Some(DesignValue::Deferred));
}

#[test]
fn a_plan_round_trips_through_the_wire_types() {
    let plan = AppPlan {
        collections: vec![DataCollectionSchema {
            id: "notes".into(),
            name: "Notes".into(),
            fields: vec![DataFieldSchema {
                id: "title".into(),
                label: "Title".into(),
                kind: DataFieldKind::Text,
                required: true,
                enum_options: Vec::new(),
            }],
        }],
        capabilities: Vec::new(),
        domains: vec!["api.example.com".into()],
        summary: "记事本".into(),
    };
    assert_eq!(plan_from_dto(&plan_to_dto(&plan)), plan);
}
```

- [ ] **Step 2: 运行测试确认失败**

Run: `cargo test -p engine-mobile --all-features local_apps_bridge:: 2>&1 | tail -30`
Expected: 编译失败，映射函数不存在。

- [ ] **Step 3: 写实现**

按该文件既有映射函数的写法补 `design_step_to_dto` / `design_step_from_dto` / `design_field_to_dto` / `design_field_from_dto` / `design_field_option_to_dto` / `design_field_option_from_dto` / `plan_to_dto` / `plan_from_dto`，并在 `design_value_to_dto` / `design_value_from_dto` 的 `match` 里加 `Deferred` 分支。删除所有 `AppTemplateKindDto` / `AppTemplateDto` 相关映射。`AppRecordDto` 映射把 `template` 换成 `brief`。`AppDetailsDto` 组装时填 `questionnaire` 与 `plan`。`AppEvent::{QuestionnaireChanged, PlanChanged}` 映射到对应的 `AppEventDto` 变体。

`design_value_from_dto` 返回 `Option` 是既有约定（`#[non_exhaustive]` 的 DTO 可能带来未知变体），保持不变。

- [ ] **Step 4: 运行测试确认通过**

Run: `cargo test -p engine-mobile --all-features local_apps 2>&1 | tail -20`
Expected: 全绿。

- [ ] **Step 5: 提交**

```bash
git add lingxi-code/apps/engine-mobile/src/local_apps_bridge.rs
git commit -m "$(cat <<'EOF'
feat(engine-mobile): bridge 映射问卷/方案/Deferred

删模版映射，新增 design_step/field/option 与 plan 的双向映射，
DesignValue::Deferred 打通线格式。

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
EOF
)"
```

---

### Task 7: FileWrite 闸门

纯函数，脱离 LLM 可测。**先于** LLM 调用实现，这样 Task 8 一落地就有地方去挡它。

**Files:**
- Create: `lingxi-code/apps/engine-mobile/src/local_apps_sources.rs`
- Modify: `lingxi-code/apps/engine-mobile/src/lib.rs`（加 `mod local_apps_sources;`，与其余 `local_apps_*` 同样的 `#[cfg(feature = "uniffi")]` 门控）

**Interfaces:**
- Consumes: `local_apps::AppError`、`local_apps::source_validator::WRITABLE_ROOTS`（若该常量当前私有，改为 `pub`）
- Produces:
  - `pub struct FileWrite { pub path: String, pub contents: String }`
  - `pub const MAX_GENERATED_FILES: usize = 60`
  - `pub const MAX_GENERATED_FILE_BYTES: usize = 256 * 1024`
  - `pub const MAX_GENERATED_TOTAL_BYTES: usize = 4 * 1024 * 1024`
  - `pub fn screen_writes(writes: &[FileWrite]) -> Result<(), AppError>`

- [ ] **Step 1: 写失败测试**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &str) -> FileWrite {
        FileWrite { path: path.into(), contents: "export default function P(){return null}".into() }
    }

    #[test]
    fn accepts_writes_under_every_writable_root() {
        let writes = vec![
            write("app/page.jsx"),
            write("components/NoteList.jsx"),
            write("lib/store.js"),
            write("styles/globals.css"),
            write("public/icon.svg"),
        ];
        screen_writes(&writes).expect("all five writable roots are allowed");
    }

    #[test]
    fn rejects_a_write_outside_the_writable_roots() {
        screen_writes(&[write("pages/index.jsx")]).expect_err("pages/ is not writable");
    }

    #[test]
    fn rejects_a_parent_traversal() {
        screen_writes(&[write("app/../../etc/passwd")]).expect_err("`..` is rejected");
    }

    #[test]
    fn rejects_an_absolute_path() {
        screen_writes(&[write("/etc/passwd")]).expect_err("absolute paths are rejected");
    }

    #[test]
    fn rejects_a_windows_style_separator() {
        screen_writes(&[write("app\\..\\secret.js")])
            .expect_err("backslashes must not smuggle a traversal past a `/`-only check");
    }

    #[test]
    fn rejects_the_lockfile_and_manifest() {
        screen_writes(&[write("package.json")]).expect_err("package.json is locked");
        screen_writes(&[write("package-lock.json")]).expect_err("the lockfile is locked");
    }

    #[test]
    fn rejects_a_duplicate_path_within_one_batch() {
        screen_writes(&[write("app/page.jsx"), write("app/page.jsx")])
            .expect_err("a batch that writes one path twice is ambiguous");
    }

    #[test]
    fn rejects_more_than_sixty_files() {
        let writes: Vec<_> = (0..61).map(|i| write(&format!("app/p{i}.jsx"))).collect();
        screen_writes(&writes).expect_err("the file count is capped");
    }

    #[test]
    fn rejects_a_single_file_over_the_byte_cap() {
        let big = FileWrite {
            path: "app/page.jsx".into(),
            contents: "x".repeat(MAX_GENERATED_FILE_BYTES + 1),
        };
        screen_writes(&[big]).expect_err("a single file is capped");
    }

    #[test]
    fn rejects_a_batch_over_the_total_byte_cap() {
        let each = MAX_GENERATED_FILE_BYTES;
        let count = MAX_GENERATED_TOTAL_BYTES / each + 1;
        let writes: Vec<_> = (0..count)
            .map(|i| FileWrite { path: format!("app/p{i}.jsx"), contents: "x".repeat(each) })
            .collect();
        screen_writes(&writes).expect_err("the batch total is capped");
    }

    #[test]
    fn rejects_an_empty_batch() {
        screen_writes(&[]).expect_err("a generation that writes nothing is a failure, not a success");
    }
}
```

- [ ] **Step 2: 运行测试确认失败**

Run: `cargo test -p engine-mobile --all-features local_apps_sources:: 2>&1 | tail -30`
Expected: 编译失败，模块不存在。

- [ ] **Step 3: 写实现**

```rust
//! LLM 写盘产物的闸门。
//!
//! 纯函数，零 I/O：整批要么全过要么全拒，**不做部分写入**——一半新
//! 一半旧的工作区比干脆失败更难诊断。通过后仍要交给
//! `local_apps::validate_workspace_source` 做完整策略校验；本模块只
//! 负责在文件落地之前把明显越界的东西挡在外面。

use local_apps::AppError;

/// 一个待写文件。路径是相对工作区根的 POSIX 路径。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileWrite {
    pub path: String,
    pub contents: String,
}

pub const MAX_GENERATED_FILES: usize = 60;
pub const MAX_GENERATED_FILE_BYTES: usize = 256 * 1024;
pub const MAX_GENERATED_TOTAL_BYTES: usize = 4 * 1024 * 1024;

const WRITABLE_ROOTS: &[&str] = &["app", "components", "lib", "styles", "public"];

fn reject(message: impl Into<String>) -> AppError {
    AppError::InvalidRequest(message.into())
}

/// 逐条筛查一批写盘请求。
pub fn screen_writes(writes: &[FileWrite]) -> Result<(), AppError> {
    if writes.is_empty() {
        return Err(reject("the generator produced no files"));
    }
    if writes.len() > MAX_GENERATED_FILES {
        return Err(reject(format!(
            "the generator produced {} files, the limit is {MAX_GENERATED_FILES}",
            writes.len()
        )));
    }
    let mut total = 0usize;
    let mut seen = std::collections::BTreeSet::new();
    for write in writes {
        screen_path(&write.path)?;
        if !seen.insert(write.path.as_str()) {
            return Err(reject(format!("duplicate path `{}` in one batch", write.path)));
        }
        let bytes = write.contents.len();
        if bytes > MAX_GENERATED_FILE_BYTES {
            return Err(reject(format!(
                "`{}` is {bytes} bytes, the per-file limit is {MAX_GENERATED_FILE_BYTES}",
                write.path
            )));
        }
        total += bytes;
    }
    if total > MAX_GENERATED_TOTAL_BYTES {
        return Err(reject(format!(
            "the batch is {total} bytes, the limit is {MAX_GENERATED_TOTAL_BYTES}"
        )));
    }
    Ok(())
}

fn screen_path(path: &str) -> Result<(), AppError> {
    if path.is_empty() {
        return Err(reject("empty write path"));
    }
    // 反斜杠先于任何 `/` 分段判断处理：否则 `app\..\secret.js` 会被
    // 当成单个合法分段混过去。
    if path.contains('\\') {
        return Err(reject(format!("`{path}` contains a backslash")));
    }
    if path.starts_with('/') {
        return Err(reject(format!("`{path}` is absolute")));
    }
    let mut segments = path.split('/');
    let Some(root) = segments.next() else {
        return Err(reject(format!("`{path}` has no root segment")));
    };
    if !WRITABLE_ROOTS.contains(&root) {
        return Err(reject(format!(
            "`{path}` is outside the writable roots {WRITABLE_ROOTS:?}"
        )));
    }
    for segment in path.split('/') {
        if segment.is_empty() || segment == "." || segment == ".." {
            return Err(reject(format!("`{path}` contains a `{segment}` segment")));
        }
    }
    Ok(())
}
```

`package.json` / `package-lock.json` 的拒绝由「不在 `WRITABLE_ROOTS` 之下」这一关天然覆盖（它们的根段是文件名本身）——测试断言的是行为而非某条具体规则，因此无需额外分支。

- [ ] **Step 4: 运行测试确认通过**

Run: `cargo test -p engine-mobile --all-features local_apps_sources:: 2>&1 | tail -20`
Expected: 11 passed。

- [ ] **Step 5: 提交**

```bash
git add lingxi-code/apps/engine-mobile/src/local_apps_sources.rs lingxi-code/apps/engine-mobile/src/lib.rs
git commit -m "$(cat <<'EOF'
feat(engine-mobile): LLM 写盘产物的路径与体积闸门

整批全过或全拒，不做部分写入。覆盖越界根、`..`、绝对路径、
反斜杠混淆、批内重复、文件数/单文件/总量三重上限。

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
EOF
)"
```

---

### Task 8: 三段 LLM 调用

**Files:**
- Create: `lingxi-code/apps/engine-mobile/src/local_apps_llm.rs`
- Create: `lingxi-code/apps/engine-mobile/assets/prompts/{author_questionnaire,plan,generate_sources}.md`
- Modify: `lingxi-code/apps/engine-mobile/src/lib.rs`

**Interfaces:**
- Consumes: Task 1 校验器、Task 7 闸门、`llm_client::ApiService::messages_create_side_query`（`llm-client/src/service.rs:2672`）
- Produces:
  - `#[async_trait] pub trait LocalAppsModel: Send + Sync { async fn structured(&self, system: &str, user: String, tool_name: &str, schema: serde_json::Value) -> Result<serde_json::Value, AppError>; }`
  - `pub struct ApiServiceModel { … }`，`impl LocalAppsModel for ApiServiceModel`
  - `pub struct LocalAppsLlm { model: Arc<dyn LocalAppsModel> }`
  - `pub async fn author_questionnaire(&self, brief: &str) -> Result<(Option<String>, Vec<AppDesignStep>), AppError>`
  - `pub async fn plan(&self, brief: &str, steps: &[AppDesignStep], answers: &BTreeMap<String, DesignValue>) -> Result<AppPlan, AppError>`
  - `pub async fn generate_sources(&self, req: &SourceRequest) -> Result<Vec<FileWrite>, AppError>`
  - `pub struct SourceRequest { pub brief: String, pub plan: AppPlan, pub answers: BTreeMap<String, DesignValue>, pub existing: Vec<FileWrite>, pub revision_prompt: Option<String>, pub validator_feedback: Option<String> }`

`LocalAppsModel` 这层 trait 是为了让三段逻辑能脱离真实 `ApiService` 单测。`structured` 用**强制工具调用**拿结构化输出——这是本仓库既有的做法，`messages_create_side_query` 的 `tool_choice` 参数就是为此存在的。

- [ ] **Step 1: 写失败测试**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// 按顺序吐出预置响应的假模型。
    struct ScriptedModel {
        responses: Mutex<Vec<Result<serde_json::Value, AppError>>>,
        prompts: Mutex<Vec<String>>,
    }

    impl ScriptedModel {
        fn new(responses: Vec<Result<serde_json::Value, AppError>>) -> Arc<Self> {
            Arc::new(Self { responses: Mutex::new(responses), prompts: Mutex::new(Vec::new()) })
        }
    }

    #[async_trait::async_trait]
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
        llm.author_questionnaire("一个记事本")
            .await
            .expect_err("6 steps must be rejected by the validator, not passed through");
    }

    #[tokio::test]
    async fn the_brief_reaches_the_model_prompt() {
        let model = ScriptedModel::new(vec![Ok(good_questionnaire())]);
        let llm = LocalAppsLlm::new(model.clone());
        llm.author_questionnaire("一个带标签的记事本").await.expect("authoring");
        let prompts = model.prompts.lock().expect("lock");
        assert!(
            prompts[0].contains("一个带标签的记事本"),
            "the user's own words must reach the model verbatim: {}",
            prompts[0]
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
        llm.generate_sources(&initial_request())
            .await
            .expect_err("the gate must reject an escaping path before anything is written");
    }

    #[tokio::test]
    async fn a_revision_prompt_reaches_the_model() {
        let model = ScriptedModel::new(vec![Ok(good_sources())]);
        let llm = LocalAppsLlm::new(model.clone());
        let mut request = initial_request();
        request.revision_prompt = Some("把搜索框挪到顶部".into());
        llm.generate_sources(&request).await.expect("revision");
        let prompts = model.prompts.lock().expect("lock");
        assert!(prompts[0].contains("把搜索框挪到顶部"), "got {}", prompts[0]);
    }

    #[tokio::test]
    async fn validator_feedback_reaches_the_model_on_a_repair_pass() {
        let model = ScriptedModel::new(vec![Ok(good_sources())]);
        let llm = LocalAppsLlm::new(model.clone());
        let mut request = initial_request();
        request.validator_feedback = Some("app/page.jsx uses eval()".into());
        llm.generate_sources(&request).await.expect("repair");
        let prompts = model.prompts.lock().expect("lock");
        assert!(prompts[0].contains("eval()"), "got {}", prompts[0]);
    }

    #[tokio::test]
    async fn a_model_error_propagates_rather_than_falling_back() {
        let llm =
            LocalAppsLlm::new(ScriptedModel::new(vec![Err(AppError::LlmUnavailable("offline".into()))]));
        llm.author_questionnaire("一个记事本")
            .await
            .expect_err("there is no template to silently fall back to — fail closed");
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
            revision_prompt: None,
            validator_feedback: None,
        }
    }
}
```

- [ ] **Step 2: 运行测试确认失败**

Run: `cargo test -p engine-mobile --all-features local_apps_llm:: 2>&1 | tail -30`
Expected: 编译失败，模块不存在。

- [ ] **Step 3: 写实现**

`local_apps_llm.rs` 主体：

```rust
//! 本地应用的三段 LLM 调用：出题 / 出方案 / 写码。
//!
//! 每段的产物一律先过校验器再返回——LLM 的输出是提议，校验器的判定
//! 才是事实。模型经 `LocalAppsModel` trait 注入，使三段逻辑能脱离真实
//! `ApiService` 单测。
//!
//! 失败一律 fail-closed：模版已经删除，系统里不存在静默降级路径。

use crate::local_apps_sources::{screen_writes, FileWrite};
use async_trait::async_trait;
use local_apps::questionnaire::{
    validate_plan, validate_questionnaire, AppDesignStep, AppPlan,
};
use local_apps::{AppError, DesignValue};
use std::collections::BTreeMap;
use std::sync::Arc;

const AUTHOR_PROMPT: &str = include_str!("../assets/prompts/author_questionnaire.md");
const PLAN_PROMPT: &str = include_str!("../assets/prompts/plan.md");
const SOURCES_PROMPT: &str = include_str!("../assets/prompts/generate_sources.md");

/// 一次结构化模型调用。实现负责鉴权、路由、重试与超时。
#[async_trait]
pub trait LocalAppsModel: Send + Sync {
    async fn structured(
        &self,
        system: &str,
        user: String,
        tool_name: &str,
        schema: serde_json::Value,
    ) -> Result<serde_json::Value, AppError>;
}

/// 写码调用的全部输入。
#[derive(Debug, Clone)]
pub struct SourceRequest {
    pub brief: String,
    pub plan: AppPlan,
    pub answers: BTreeMap<String, DesignValue>,
    /// 修订时的现有源码；初次生成为空。
    pub existing: Vec<FileWrite>,
    /// 用户的自然语言修改要求。
    pub revision_prompt: Option<String>,
    /// 上一轮 validator 的错误原文，用于修复循环。
    pub validator_feedback: Option<String>,
}

pub struct LocalAppsLlm {
    model: Arc<dyn LocalAppsModel>,
}

impl LocalAppsLlm {
    #[must_use]
    pub fn new(model: Arc<dyn LocalAppsModel>) -> Self {
        Self { model }
    }

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
                "emit_questionnaire",
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
        validate_questionnaire(&steps)?;
        Ok((name, steps))
    }

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
            .structured(PLAN_PROMPT, user, "emit_plan", plan_schema())
            .await?;
        let plan: AppPlan = serde_json::from_value(value)
            .map_err(|error| AppError::LlmOutputRejected(format!("plan is malformed: {error}")))?;
        validate_plan(&plan)?;
        Ok(plan)
    }

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
        if let Some(prompt) = &request.revision_prompt {
            user.push_str(&format!("\n\n用户要求的修改：\n{prompt}"));
        }
        if let Some(feedback) = &request.validator_feedback {
            user.push_str(&format!(
                "\n\n上一次生成没有通过校验，原文如下。请修正后重新给出完整文件：\n{feedback}"
            ));
        }
        let value = self
            .model
            .structured(SOURCES_PROMPT, user, "emit_sources", sources_schema())
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
        screen_writes(&writes)?;
        Ok(writes)
    }
}
```

三个 schema 函数返回 JSON Schema 值，字段名与 DTO 的 camelCase 一致，都设 `"additionalProperties": false`。`sources_schema()` 是三者里最简单也最要紧的一个：

```rust
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
```

`questionnaire_schema()` 顶层为 `{ suggestedName?: string, steps: [AppDesignStep] }`，`steps` 的 `maxItems` 取 `local_apps::questionnaire::MAX_STEPS`；`plan_schema()` 顶层为 `AppPlan` 的形状，`collections` 的 `maxItems` 取 `MAX_COLLECTIONS`、`domains` 取 `MAX_DOMAINS`。**schema 里的上限是给模型的提示，不是保证**——真正的把关仍然在 Task 1 的校验器和 Task 7 的闸门，两处都不能因为 schema 写了上限就省掉。

`ApiServiceModel` 的 `structured` 实现：把 schema 包成一个单工具声明，用 `tool_choice: Some(ToolChoice::Tool { name })` 强制调用，走 `messages_create_side_query(model, None, Some(system), vec![user_message], vec![tool], max_tokens, tool_choice, vec![], None)`，从响应里取该工具调用的 `input` 作为返回值。`LlmError` 映射为 `AppError::LlmUnavailable(format!("{error}"))`；响应里找不到该工具调用则映射为 `AppError::LlmOutputRejected("the model did not call the required tool".into())`。max_tokens：出题 4096、出方案 4096、写码 32768。

三段逻辑里的 `serde_json::from_value` 失败、以及 `files` 数组缺失/条目缺字段，一律用 `AppError::LlmOutputRejected` 而非 `InvalidRequest`——界面据此区分「模型够不到」与「模型给的东西不合格」，两者的提示文案和可行动作都不同。

三个 prompt 文件写明各自的职责、输出契约与硬约束（写码那份必须列出 `WRITABLE_ROOTS`、禁 API routes / Server Actions / `eval` / 外部脚本 / 直接网络调用、必须走 `window.lingxi.v1`、必须静态导出兼容）。

- [ ] **Step 4: 运行测试确认通过**

Run: `cargo test -p engine-mobile --all-features local_apps_llm:: 2>&1 | tail -20`
Expected: 8 passed。

- [ ] **Step 5: 提交**

```bash
git add lingxi-code/apps/engine-mobile/src/local_apps_llm.rs lingxi-code/apps/engine-mobile/assets/prompts lingxi-code/apps/engine-mobile/src/lib.rs
git commit -m "$(cat <<'EOF'
feat(engine-mobile): 出题/出方案/写码三段 LLM 调用

经 LocalAppsModel trait 注入模型，三段产物一律先过校验器再返回。
prompt 以 include_str! 从 assets/prompts/*.md 读入，走 git diff 可审。
失败 fail-closed：模版已删，不存在静默降级路径。

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
EOF
)"
```

---

### Task 9: 生成执行器接上 LLM

**Files:**
- Modify: `lingxi-code/apps/engine-mobile/src/local_apps_generation.rs:88`（删 `APP_SHELL_TEMPLATE`）、`:1370-1400`（`MobileAppGenerationExecutor`）、`:1607`（`generate_source`）、`:1863-1904`（删 `render_app_shell_source`）

**Interfaces:**
- Consumes: Task 8 的 `LocalAppsLlm` / `SourceRequest`、Task 7 的 `FileWrite`
- Produces: `MobileAppGenerationExecutor::new(mobile_linux, host, llm: Arc<LocalAppsLlm>)`；`generate_source` 改为 LLM 驱动并带最多 2 次的修复循环

- [ ] **Step 1: 写失败测试**

```rust
#[tokio::test]
async fn generate_source_writes_what_the_model_returned() {
    let harness = generation_harness(vec![Ok(serde_json::json!({
        "files": [{"path": "app/page.jsx", "contents": "export default function P(){return <div/>}"}]
    }))])
    .await;

    harness
        .executor
        .generate_source(&harness.initial_request(), &harness.layout)
        .await
        .expect("generation succeeds");

    let written = harness.read("app/page.jsx").await.expect("the file landed");
    assert!(written.contains("export default function P"));
}

#[tokio::test]
async fn a_restore_job_never_calls_the_model() {
    let harness = generation_harness(Vec::new()).await;
    let mut request = harness.initial_request();
    request.kind = GenerationRequestKind::Restore;

    harness
        .executor
        .generate_source(&request, &harness.layout)
        .await
        .expect("restore reuses existing source");

    assert_eq!(harness.model_calls(), 0, "a restore must not spend an LLM round trip");
}

#[tokio::test]
async fn a_validation_failure_is_fed_back_and_the_second_attempt_can_succeed() {
    let harness = generation_harness(vec![
        // 第一次带 eval，validator 会拒。
        Ok(serde_json::json!({
            "files": [{"path": "app/page.jsx", "contents": "export const x = eval('1')"}]
        })),
        // 第二次干净。
        Ok(serde_json::json!({
            "files": [{"path": "app/page.jsx", "contents": "export default function P(){return null}"}]
        })),
    ])
    .await;

    harness
        .executor
        .generate_source(&harness.initial_request(), &harness.layout)
        .await
        .expect("the repair pass succeeds");

    assert_eq!(harness.model_calls(), 2, "exactly one repair round trip");
    let second = harness.prompt_at(1);
    assert!(
        second.contains("eval"),
        "the validator's own words must reach the repair pass: {second}"
    );
}

#[tokio::test]
async fn three_consecutive_validation_failures_give_up() {
    let dirty = || Ok(serde_json::json!({
        "files": [{"path": "app/page.jsx", "contents": "export const x = eval('1')"}]
    }));
    let harness = generation_harness(vec![dirty(), dirty(), dirty()]).await;

    harness
        .executor
        .generate_source(&harness.initial_request(), &harness.layout)
        .await
        .expect_err("the repair loop is bounded");

    assert_eq!(
        harness.model_calls(),
        3,
        "one initial attempt plus at most two repairs — never an unbounded loop"
    );
}

#[tokio::test]
async fn a_revision_job_passes_the_prompt_and_the_existing_tree_to_the_model() {
    let harness = generation_harness(vec![Ok(serde_json::json!({
        "files": [{"path": "app/page.jsx", "contents": "export default function P(){return null}"}]
    }))])
    .await;
    harness.seed("components/Old.jsx", "export const Old = 1").await;

    let mut request = harness.initial_request();
    request.kind = GenerationRequestKind::Revision;
    request.prompt = Some("把搜索框挪到顶部".into());

    harness
        .executor
        .generate_source(&request, &harness.layout)
        .await
        .expect("revision succeeds");

    let prompt = harness.prompt_at(0);
    assert!(prompt.contains("把搜索框挪到顶部"), "the user's words: {prompt}");
    assert!(prompt.contains("components/Old.jsx"), "the existing tree: {prompt}");
}
```

`generation_harness(responses)` 建一个内存 fs 的 `AppLayout`、一个 `ScriptedModel`（复用 Task 8 那个，提到 `crate::local_apps_llm::test_support` 或在本模块内重建）、一个已 `attach_service` 的 `MobileAppGenerationExecutor`，并给出 `read` / `seed` / `model_calls` / `prompt_at` / `initial_request` 助手。

- [ ] **Step 2: 运行测试确认失败**

Run: `cargo test -p engine-mobile --all-features local_apps_generation:: 2>&1 | tail -30`
Expected: 编译失败，构造器还没有 `llm` 参数。

- [ ] **Step 3: 写实现**

删除 `APP_SHELL_TEMPLATE`（`:88` 整段 raw string）、`render_app_shell_source`（`:1863-1904`）、`default_collection_fields`、以及只被它们使用的 `embed_json_value`。

`MobileAppGenerationExecutor` 加字段 `llm: Arc<LocalAppsLlm>`，`new` 增加同名参数。`generate_source` 改为：

```rust
    async fn generate_source(
        &self,
        request: &GenerationRequest,
        layout: &AppLayout,
    ) -> Result<(), AppError> {
        if request.kind == GenerationRequestKind::Restore {
            return Ok(());
        }
        let service = self.service()?;
        let record = service.record(&request.key.app_id).await?;
        let draft = service.draft(&request.key.app_id).await?;
        let plan = draft.plan.clone().ok_or_else(|| {
            AppError::WorkflowStateInvalid("generation requires a confirmed plan".into())
        })?;
        let workspace = layout.root().join(layout.workspace_rel());

        let existing = if request.kind == GenerationRequestKind::Revision {
            read_generated_tree(&workspace)?
        } else {
            Vec::new()
        };

        let mut source_request = SourceRequest {
            brief: record.brief.clone(),
            plan,
            answers: draft.fields.clone(),
            existing,
            revision_prompt: request.prompt.clone(),
            validator_feedback: None,
        };

        // 一次首发 + 最多两次修复。validator 的错误原文回喂给模型——
        // 让它看见自己错在哪，比让它重猜一次有效得多。
        const MAX_ATTEMPTS: usize = 3;
        let mut last_error = None;
        for _ in 0..MAX_ATTEMPTS {
            let writes = self.llm.generate_sources(&source_request).await?;
            clear_generated_roots(&workspace)?;
            for write in &writes {
                write_file(&workspace, &write.path, write.contents.as_bytes(), true)?;
            }
            let policy = self.source_policy(request, layout).await?;
            match validate_workspace_source(&workspace, &policy) {
                Ok(()) => return Ok(()),
                Err(error) => {
                    let message = format!("{error}");
                    source_request.validator_feedback = Some(message.clone());
                    last_error = Some(error);
                }
            }
        }
        Err(last_error.unwrap_or_else(|| {
            AppError::Io("source generation exhausted its repair attempts".into())
        }))
    }
```

新增两个私有助手：`read_generated_tree(workspace)` 遍历五个可写根收集 `FileWrite`（受 `MAX_GENERATED_TOTAL_BYTES` 预算约束，超出则截断并在末尾追加一条说明文件，防止一棵大树把 prompt 撑爆）；`clear_generated_roots(workspace)` 在写入前清空五个可写根——**必须清空**，否则上一轮留下的文件会和这一轮的混在一起，validator 看到的是两代产物的并集。

`local_apps_profile.rs` 里构造 `MobileAppGenerationExecutor` 的地方补上 `llm` 实参。

- [ ] **Step 4: 运行测试确认通过**

Run: `cargo test -p engine-mobile --all-features --no-fail-fast 2>&1 | tail -20`
Expected: 全绿。

- [ ] **Step 5: 提交**

```bash
git add lingxi-code/apps/engine-mobile/src
git commit -m "$(cat <<'EOF'
feat(engine-mobile)!: 生成改由 LLM 写源码，删除模版渲染

删 APP_SHELL_TEMPLATE 与 render_app_shell_source —— 此前的「生成」
是一次 Rust 字符串替换，完全没有 LLM 参与。

改为 LLM 写入五个可写根，写前清空、写后过 validator，失败把
validator 原文回喂重生成，一次首发 + 最多两次修复。
Revision job 携带用户 prompt 与现有源码树；Restore 不调模型。

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
EOF
)"
```

---

### Task 10: MCP 工具表

**Files:**
- Modify: `lingxi-code/apps/engine-mobile/src/local_apps_mcp.rs:181`（`create`）、`:290` 后新增 `revise`、`:593` 的工具名清单

**Interfaces:**
- Consumes: Task 4 的 `create_app`、既有的 `AppService::request_revision`（`service.rs:1312`）
- Produces: `create` 参数由 `{name, template}` 改为 `{brief, name?}`；新增 `revise` 工具

**背景**：修订链路本身**已经存在且已全链路接通**（`request_revision` → `RevisionRequested` continuation → `GenerationJob.prompt` → `GenerationRequest.prompt`，见 `generation.rs:889-900` 与 `:558`）。缺的只是 MCP 这一层暴露——12 个工具里没有任何修订入口，所以会话里的 agent 发起不了迭代。不要重新实现服务层。

- [ ] **Step 1: 写失败测试**

```rust
#[tokio::test]
async fn create_takes_a_brief_instead_of_a_template() {
    let provider = test_provider().await;
    let result = provider
        .call("create", serde_json::json!({ "brief": "一个记事本 app" }))
        .await
        .expect("a brief alone creates an app");
    assert!(result.get("appId").is_some(), "got {result}");
}

#[tokio::test]
async fn create_rejects_a_legacy_template_argument() {
    let provider = test_provider().await;
    provider
        .call("create", serde_json::json!({ "name": "N", "template": "dashboard" }))
        .await
        .expect_err("the template argument no longer exists (additionalProperties: false)");
}

#[tokio::test]
async fn revise_is_exposed_and_reaches_the_service() {
    let provider = test_provider().await;
    let created = provider
        .call("create", serde_json::json!({ "brief": "一个记事本" }))
        .await
        .expect("create");
    let app_id = created["appId"].as_str().expect("appId").to_string();
    drive_to_ready(&provider, &app_id).await;

    provider
        .call("revise", serde_json::json!({ "app_id": app_id, "prompt": "把搜索框挪到顶部" }))
        .await
        .expect("revise is callable on a ready app");
}

#[test]
fn the_declared_catalog_contains_revise_and_no_template_surface() {
    let names: Vec<String> = LocalAppsMcpProvider::tools()
        .iter()
        .map(|tool| tool.name.clone())
        .collect();
    assert!(names.contains(&"revise".to_string()), "got {names:?}");
    let create = LocalAppsMcpProvider::tools()
        .into_iter()
        .find(|tool| tool.name == "create")
        .expect("create is declared");
    let schema = serde_json::to_string(&create.input_schema).expect("serialise");
    assert!(schema.contains("brief"), "create takes a brief: {schema}");
    assert!(!schema.contains("template"), "the template argument is gone: {schema}");
}
```

`LocalAppsMcpProvider::tools()` 的确切取法以既有测试 `catalog_is_fixed_and_exposes_no_arbitrary_execution_surface`（`local_apps_mcp.rs:587`）为准；那个测试的名字清单也要加上 `"revise"`。

`drive_to_ready` 依次走 `questionnaire_ready` → 填答案 → `begin_planning` → `plan_ready` → `confirm_design` → 生成完成 → `confirm_preview`，用测试用的 service 直接推进即可。

- [ ] **Step 2: 运行测试确认失败**

Run: `cargo test -p engine-mobile --all-features local_apps_mcp:: 2>&1 | tail -30`
Expected: `create` 仍要求 `template`；`revise` 未注册。

- [ ] **Step 3: 写实现**

`create` 的声明改为：

```rust
            Self::tool(
                "create",
                "Create a local app from a one-line description and start the LLM-authored design questionnaire. This never confirms the design or starts generation.",
                json!({"type":"object","properties":{
                    "brief":{"type":"string","minLength":1,"maxLength":2000},
                    "name":{"type":"string","minLength":1,"maxLength":200},
                    "conversation_id":{"type":"string","maxLength":128}
                },"required":["brief"],"additionalProperties":false}),
            ),
```

新增：

```rust
            Self::tool(
                "revise",
                "Ask for a revision of a generated app in the user's own words. The app rebuilds and re-opens the preview gate; the user still approves it.",
                json!({"type":"object","properties":{
                    "app_id":app_id.clone(),
                    "prompt":{"type":"string","minLength":1,"maxLength":4000}
                },"required":["app_id","prompt"],"additionalProperties":false}),
            ),
```

分发 `match` 加：

```rust
            "revise" => {
                let app_id = required_str(&args, "app_id")?;
                let prompt = required_str(&args, "prompt")?;
                service.request_revision(&app_id, &prompt).await?;
                Ok(json!({ "appId": app_id, "state": "revising" }))
            }
```

`:593` 与 `:595` 那两处工具名清单加 `"revise"`，`create` 的调用点参数同步改。

- [ ] **Step 4: 运行测试确认通过**

Run: `cargo test -p engine-mobile --all-features local_apps_mcp:: 2>&1 | tail -20`
Expected: 全绿。

- [ ] **Step 5: 提交**

```bash
git add lingxi-code/apps/engine-mobile/src/local_apps_mcp.rs
git commit -m "$(cat <<'EOF'
feat(engine-mobile): MCP create 收 brief，新增 revise 工具

create 的 template 枚举换成 brief 字符串。
revise 直接转调既有的 AppService::request_revision —— 服务层与
prompt 链路早已接通，缺的只是这一层暴露，会话中的 agent 此前
根本无法发起迭代。

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
EOF
)"
```

---

### Task 11: host 编排与客户端命令

**Files:**
- Modify: `lingxi-code/apps/engine-mobile/src/local_apps_profile.rs:97-160`（`ProfileApps` 持有 `LocalAppsLlm`）
- Modify: `lingxi-code/apps/engine-mobile/src/host.rs`（新增命令处理 + 出题/出方案的自动触发）

**Interfaces:**
- Consumes: Task 4 的 service 方法、Task 8 的 `LocalAppsLlm`
- Produces:
  - `ProfileApps.llm: Arc<LocalAppsLlm>`
  - `handle_create_app(name: Option<String>, brief: String, conversation_id: Option<String>)`
  - `handle_update_app_brief(app_id: String, brief: String)`
  - `handle_retry_app_questionnaire(app_id: String)`
  - `handle_begin_app_planning(app_id: String)`
  - `handle_retry_app_plan(app_id: String)`
  - 私有：`spawn_authoring(service, llm, app_id)`、`spawn_planning(service, llm, app_id)`

- [ ] **Step 1: 写失败测试**

```rust
#[tokio::test]
async fn creating_an_app_drives_authoring_to_collecting_spec() {
    let host = test_host_with_scripted_model(vec![Ok(good_questionnaire())]).await;
    let app_id = host.create_app(None, "一个记事本").await.expect("create");
    host.settle().await;

    let record = host.record(&app_id).await.expect("record");
    assert_eq!(record.workflow_state, AppWorkflowState::CollectingSpec);
    assert_eq!(record.name, "记事本", "the suggested name replaced the placeholder");
}

#[tokio::test]
async fn a_model_failure_lands_in_questionnaire_failed_and_stays_retryable() {
    let host = test_host_with_scripted_model(vec![
        Err(AppError::LlmUnavailable("offline".into())),
        Ok(good_questionnaire()),
    ])
    .await;
    let app_id = host.create_app(None, "一个记事本").await.expect("create");
    host.settle().await;
    assert_eq!(
        host.record(&app_id).await.expect("record").workflow_state,
        AppWorkflowState::QuestionnaireFailed
    );

    host.retry_questionnaire(&app_id).await.expect("retry");
    host.settle().await;
    assert_eq!(
        host.record(&app_id).await.expect("record").workflow_state,
        AppWorkflowState::CollectingSpec
    );
}

#[tokio::test]
async fn beginning_planning_drives_through_to_the_confirmation_gate() {
    let host = test_host_with_scripted_model(vec![Ok(good_questionnaire()), Ok(good_plan())]).await;
    let app_id = host.create_app(None, "一个记事本").await.expect("create");
    host.settle().await;
    host.answer(&app_id, "features", DesignValue::MultipleChoice(vec!["list".into()])).await;

    host.begin_planning(&app_id).await.expect("planning starts");
    host.settle().await;

    let record = host.record(&app_id).await.expect("record");
    assert_eq!(record.workflow_state, AppWorkflowState::AwaitingSpecConfirmation);
    let draft = host.draft(&app_id).await.expect("draft");
    assert_eq!(draft.plan_for_revision, Some(draft.revision));
}

#[tokio::test]
async fn changing_the_brief_reauthors_the_questionnaire() {
    let host = test_host_with_scripted_model(vec![
        Ok(good_questionnaire()),
        Ok(other_questionnaire()),
    ])
    .await;
    let app_id = host.create_app(None, "一个记事本").await.expect("create");
    host.settle().await;

    host.update_brief(&app_id, "改成一个待办清单").await.expect("brief is editable");
    host.settle().await;

    let draft = host.draft(&app_id).await.expect("draft");
    assert_eq!(draft.questionnaire[0].id, "todo_basics", "a fresh questionnaire replaced the old one");
    assert!(draft.fields.is_empty());
}
```

`host.settle()` 等待后台 spawn 的出题/出方案任务落定——用该文件既有的 `join_app_mutation` 同款等待方式，不要靠 sleep。

- [ ] **Step 2: 运行测试确认失败**

Run: `cargo test -p engine-mobile --all-features host::tests:: 2>&1 | tail -30`
Expected: 命令与触发逻辑都不存在。

- [ ] **Step 3: 写实现**

`ProfileApps` 加 `pub(crate) llm: Arc<LocalAppsLlm>`，在其构造处用 `ApiServiceModel` 包装该 profile 的 `ApiService`（构造点见 `host.rs:1130` 的 `api_service`）。

`host.rs` 新增命令处理，形状照抄 `handle_request_app_revision`（`:3130`）。核心是两个私有触发器：

```rust
    /// 出题跑在后台：创建命令立刻返回，界面进「出题中」，模型往返
    /// 落定后再推状态。失败落 questionnaire_failed，绝不静默降级
    /// —— 模版已经删了，没有可退的默认问卷。
    fn spawn_authoring(
        runtime: &RuntimeHandle,
        service: Arc<AppService>,
        llm: Arc<LocalAppsLlm>,
        emissions: Arc<AppEmissions>,
        app_id: String,
    ) {
        runtime.spawn(async move {
            let brief = match service.record(&app_id).await {
                Ok(record) => record.brief,
                Err(error) => {
                    emissions.emit_failure(Some(&service), Some(app_id), &error).await;
                    return;
                }
            };
            match llm.author_questionnaire(&brief).await {
                Ok((name, steps)) => {
                    if let Err(error) = service.questionnaire_ready(&app_id, steps, name).await {
                        let _ = service.questionnaire_failed(&app_id, &format!("{error}")).await;
                        emissions.emit_failure(Some(&service), Some(app_id.clone()), &error).await;
                    }
                }
                Err(error) => {
                    let _ = service.questionnaire_failed(&app_id, &format!("{error}")).await;
                    emissions.emit_failure(Some(&service), Some(app_id.clone()), &error).await;
                }
            }
            Self::emit_apps_snapshot(&service).await;
        });
    }
```

`spawn_planning` 同形：读 record + draft，调 `llm.plan(&brief, &draft.questionnaire, &draft.fields)`，成功走 `service.plan_ready`，失败走 `service.plan_failed`。

触发点：`handle_create_app` 落盘成功后调 `spawn_authoring`；`handle_update_app_brief` 与 `handle_retry_app_questionnaire` 同样调它；`handle_begin_app_planning` 先 `service.begin_planning`（它会先校验答案自洽）成功后再 `spawn_planning`；`handle_retry_app_plan` 走 `service.retry_plan` 后 `spawn_planning`。

- [ ] **Step 4: 运行测试确认通过**

Run: `cargo test -p engine-mobile --all-features --no-fail-fast 2>&1 | tail -20`
Expected: 全绿。

- [ ] **Step 5: 提交**

```bash
git add lingxi-code/apps/engine-mobile/src
git commit -m "$(cat <<'EOF'
feat(engine-mobile): host 编排出题与出方案

创建/改 brief/重试触发后台出题，begin_planning 触发后台出方案。
两者都在失败时落各自的失败态并发事件，不静默降级。
ProfileApps 持有 LocalAppsLlm，模型来自该 profile 的 ApiService。

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
EOF
)"
```

---

### Task 12: 改写 create-local-app skill

**Files:**
- Modify: `skills/create-local-app/SKILL.md`

**Interfaces:**
- Consumes: Task 10 的工具表
- Produces: 与新流程一致的 agent 指引

- [ ] **Step 1: 通读现状，列出所有失真句**

Run: `grep -n "template\|模版\|five steps\|next-static-v1" skills/create-local-app/SKILL.md`
Expected: 至少命中 "obtain the current template catalog"、"Match the request to a template"、"Call `mcp__local_apps__create` with the selected template"、"the host's five steps"、"The fixed scaffold is `next-static-v1`"。前四条已经失真，最后一条仍然成立。

- [ ] **Step 2: 改写「Create an app」小节**

替换为：

```markdown
## Create an app

1. Call `mcp__local_apps__create` with a `brief` — the user's own one-line
   description, in their words. Do not invent a template, a name, or a
   feature list. Success means "the host is authoring a questionnaire," not
   "an app exists."
2. The host asks the LLM for a questionnaire tailored to that brief and opens
   the designer. Follow progress with `mcp__local_apps__get`.
3. The user answers the questionnaire themselves. Relay what the app will do
   and what is still unanswered; do not fill the answers on their behalf.
4. Use `mcp__local_apps__propose_design` when a concrete suggestion would
   help. Present its field-level diff. Never apply or dismiss a suggestion on
   the user's behalf.
5. After the answers are in, the host derives a plan (data collections,
   capabilities, domains) and opens the design confirmation gate. Explain the
   plan in plain language — especially anything the user left to the model's
   discretion. Wait for explicit confirmation. Never automate that tap.
6. Follow generation with `mcp__local_apps__get`; use
   `mcp__local_apps__read_logs` when a job fails.
7. Open the preview after the generator reaches preview-ready. Ask the user to
   approve it or describe what to change; do not approve your own output.

## Keep improving an app

Generation is not one-shot. When the user describes a change in their own
words, call `mcp__local_apps__revise` with that description as `prompt`. The
app rebuilds and the preview gate re-opens; the user still approves it. There
is no limit on how many times this repeats, and every pass writes a checkpoint
that can be restored.

Return to `mcp__local_apps__create` only for a genuinely different app. Do not
try to change an existing app's brief — that discards every answer the user
gave and is theirs to trigger, not yours.
```

- [ ] **Step 3: 更新「Enforce the host contract」的工具清单**

在 `mcp__local_apps__propose_design` 之后加入 `mcp__local_apps__revise`。删除段落 "The fixed scaffold is `next-static-v1`" 之前那句关于模版的措辞；scaffold 与写入白名单、禁止项、`window.lingxi.v1`、静态导出这几段全部保留不动——它们仍然成立，而且现在是 LLM 写码时的硬约束。

- [ ] **Step 4: 核对无残留**

Run: `grep -n "template\|模版" skills/create-local-app/SKILL.md`
Expected: 只剩 `next-static-v1` scaffold 相关的表述，没有任何"选模版 / 模版目录 / five steps"。

- [ ] **Step 5: 提交**

```bash
git add skills/create-local-app/SKILL.md
git commit -m "$(cat <<'EOF'
docs(skill): create-local-app 改写为对话式流程

删除模版目录与五步向导的指引，改为「转述 brief → 跟进出题/出方案
→ 守住两道人工门 → 用 revise 持续迭代」。补上 revise 工具。
scaffold 白名单与禁止项保留 —— 它们现在是 LLM 写码的硬约束。

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
EOF
)"
```

---

### Task 13: iOS 模型层与 store

**Files:**
- Modify: `clients/ios/Sources/LocalApps/LocalAppsModels.swift:162-180`、`LocalAppsStore.swift`、`LocalAppsProtocolAdapter.swift`
- Modify: `clients/ios/Tests/LocalAppsStoreTests.swift`

**Interfaces:**
- Consumes: Task 5 的 DTO（uniffi 绑定需重新生成）
- Produces:
  - `LocalAppQuestionnaire = [LocalAppDesignStep]`（`LocalAppTemplate` 删除）
  - `LocalAppPlan { collections, capabilities, domains, summary }`
  - `LocalAppSummary.brief: String`（`template` 删除）
  - `LocalAppsStore.questionnaires: [String: [LocalAppDesignStep]]`、`.plans: [String: LocalAppPlan]`
  - `LocalAppsStore.createApp(brief:) async -> Bool`、`.updateBrief(appID:brief:)`、`.retryQuestionnaire(appID:)`、`.beginPlanning(appID:)`、`.retryPlan(appID:)`

- [ ] **Step 1: 写失败测试**

加进 `LocalAppsStoreTests.swift`：

```swift
@Test func questionnaireEventReplacesTheStoredSteps() async {
    let store = makeStore()
    await store.handle(.appQuestionnaireChanged(appId: "a", revision: 1, steps: [oneStepDTO()]))
    #expect(store.questionnaires["a"]?.count == 1)
    #expect(store.questionnaires["a"]?.first?.fields.first?.allowsDefer == true)
}

@Test func planEventStoresAndClears() async {
    let store = makeStore()
    await store.handle(.appPlanChanged(appId: "a", revision: 2, plan: onePlanDTO()))
    #expect(store.plans["a"]?.summary == "记事本")

    await store.handle(.appPlanChanged(appId: "a", revision: 3, plan: nil))
    #expect(store.plans["a"] == nil, "an answer edit voids the plan on the client too")
}

@Test func deferredAnswersRoundTripThroughThePatchWire() async {
    let store = makeStore()
    let sent = store.designPatch(fieldID: "tone", value: .deferred)
    #expect(sent.ops.first?.value == .deferred)
}
```

Task 13–16 的 Swift 测试共用一组构造助手，放在 `clients/ios/Tests/LocalAppsFixtures.swift`（新建）：

```swift
@MainActor func makeStore() -> LocalAppsStore { LocalAppsStore(adapter: StubProtocolAdapter()) }

func oneStepDTO() -> AppDesignStepDto {
    AppDesignStepDto(
        id: "basics", order: 0, title: "功能", description: nil,
        fields: [designFieldDTO(allowsCustom: true, allowsDefer: true)]
    )
}

func designFieldDTO(allowsCustom: Bool = false, allowsDefer: Bool = false) -> AppDesignFieldDto {
    AppDesignFieldDto(
        id: "features", label: "需要哪些功能", description: nil,
        fieldType: .multipleChoice, required: true,
        allowsCustom: allowsCustom, allowsDefer: allowsDefer,
        defaultValue: nil,
        options: [AppDesignFieldOptionDto(value: "list", label: "笔记列表")]
    )
}

func designField(allowsCustom: Bool = false, allowsDefer: Bool = false) -> LocalAppDesignField {
    LocalAppDesignField(from: designFieldDTO(allowsCustom: allowsCustom, allowsDefer: allowsDefer))
}

func oneStep() -> LocalAppDesignStep { LocalAppDesignStep(from: oneStepDTO()) }

func notesPlan() -> LocalAppPlan {
    LocalAppPlan(
        collections: [LocalAppDataCollection(
            id: "notes", label: "Notes",
            fields: [LocalAppDataField(id: "title", label: "Title", fieldType: .text, required: true, options: [])],
            enabledByDefault: true
        )],
        capabilities: [], domains: [], summary: "记事本"
    )
}

func planWithDomain(_ domain: String) -> LocalAppPlan {
    var plan = notesPlan()
    plan.domains = [domain]
    return plan
}

func onePlanDTO() -> AppPlanDto { /* notesPlan() 的 DTO 对应物 */ }
```

`StubProtocolAdapter` 记录发出的命令、不做真实 FFI 调用；若 `LocalAppsStoreTests.swift` 已有同类替身，复用它而不是新写一个。

- [ ] **Step 2: 运行测试确认失败**

Run: `cd clients/ios && swift test --filter LocalAppsStoreTests 2>&1 | tail -30`
Expected: 编译失败，`appQuestionnaireChanged` / `LocalAppPlan` / `.deferred` 不存在。

- [ ] **Step 3: 写实现**

先重新生成 uniffi 绑定（按 `clients/ios/scripts` 里既有的绑定生成脚本）。删除 `LocalAppTemplate` 结构体与 `LocalAppsStore.templates` 缓存及其 `template(_:)` 查询方法。`LocalAppSummary` 的 `template` 换成 `brief`。新增：

```swift
struct LocalAppPlan: Hashable, Sendable {
    let collections: [LocalAppDataCollection]
    let capabilities: [LocalAppCapabilityKind]
    let domains: [String]
    let summary: String
}
```

`LocalAppDesignField` 加 `allowsCustom: Bool` 与 `allowsDefer: Bool`；`LocalAppDesignValue` 加 `case deferred`。store 加两个字典与五个命令方法，事件 `switch` 加两个新 case。`LocalAppsProtocolAdapter` 补 DTO ↔ Swift 模型的映射。

- [ ] **Step 4: 运行测试确认通过**

Run: `cd clients/ios && swift test --filter LocalAppsStoreTests 2>&1 | tail -20`
Expected: 全绿。

- [ ] **Step 5: 提交**

```bash
git add clients/ios
git commit -m "$(cat <<'EOF'
feat(ios): 本地应用模型层去模版化，接入问卷与方案

删 LocalAppTemplate 与 templates 缓存，新增 questionnaires/plans
两个字典、LocalAppPlan、allowsCustom/allowsDefer 与 .deferred，
以及五个新命令方法。uniffi 绑定重新生成。

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
EOF
)"
```

---

### Task 14: iOS 设计器

**Files:**
- Modify: `clients/ios/Sources/LocalApps/LocalAppDesignerView.swift`

**Interfaces:**
- Consumes: Task 13 的 store
- Produces: 问卷驱动的设计器 + `Other…` / 「由你决定」两种 chip + 四个中间态

- [ ] **Step 1: 写失败测试**

```swift
@Test func stepsComeFromTheQuestionnaireNotATemplate() {
    let store = makeStore()
    store.questionnaires["a"] = [oneStep()]
    let view = LocalAppDesignerView(store: store, appID: "a", path: .constant([]))
    #expect(view.steps.count == 1)
}

@Test func aFieldThatAllowsDeferOffersTheDeferChip() {
    let field = designField(allowsDefer: true)
    #expect(DesignerFieldChips(field: field).chipValues.contains(.defer))
}

@Test func aFieldThatAllowsCustomOffersTheOtherBox() {
    let field = designField(allowsCustom: true)
    #expect(DesignerFieldChips(field: field).showsCustomInput)
}

@Test func choosingDeferStoresTheDeferredValueRatherThanClearingTheField() {
    var recorded: LocalAppDesignValue?
    let chips = DesignerFieldChips(field: designField(allowsDefer: true)) { recorded = $0 }
    chips.select(.defer)
    #expect(recorded == .deferred, "defer is an answer, not an absence")
}

@Test func theDesignerIsReadOnlyWhileTheModelIsWorking() {
    for state in [LocalAppWorkflowState.authoringQuestionnaire, .planning] {
        #expect(LocalAppDesignerView.isEditable(state) == false)
    }
    #expect(LocalAppDesignerView.isEditable(.collectingSpec))
}
```

- [ ] **Step 2: 运行测试确认失败**

Run: `cd clients/ios && swift test --filter LocalAppDesigner 2>&1 | tail -30`
Expected: `DesignerFieldChips` 与 `isEditable` 不存在。

- [ ] **Step 3: 写实现**

`steps` 计算属性由 `template?.orderedSteps ?? []` 改为 `store.questionnaires[appID]?.sorted { $0.order < $1.order } ?? []`。新增 `DesignerFieldChips` 视图承担选项渲染：普通选项 chip、`allowsCustom` 时追加 `Other…` 输入、`allowsDefer` 时追加「由你决定」chip（选中即发 `.deferred`）。

新增静态 `isEditable(_:)` 并用它 disable 整个表单。`ContentUnavailableView` 分支扩展为四个：`authoringQuestionnaire`（`ProgressView` + "正在准备问题…"）、`questionnaireFailed`（重试按钮 + 改描述入口）、`planning`（`ProgressView` + "正在整理方案…"）、`planFailed`（重试按钮）。

底部主按钮在最后一步时文案改为「生成方案」并调 `store.beginPlanning(appID:)`——它会触发一次 LLM 往返，措辞必须让用户预期到等待，不能沿用「下一步」。

- [ ] **Step 4: 运行测试确认通过**

Run: `cd clients/ios && swift test --filter LocalAppDesigner 2>&1 | tail -20`
Expected: 全绿。

- [ ] **Step 5: 提交**

```bash
git add clients/ios/Sources/LocalApps/LocalAppDesignerView.swift
git commit -m "$(cat <<'EOF'
feat(ios): 设计器改由问卷驱动，新增两种 chip 与四个中间态

steps 来源改为 store.questionnaires。allowsCustom 渲染 Other…，
allowsDefer 渲染「由你决定」并发 .deferred（是答案，不是留空）。
出题/出方案期间表单只读。末步按钮文案改为「生成方案」。

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
EOF
)"
```

---

### Task 15: iOS 方案确认 sheet

**Files:**
- Create: `clients/ios/Sources/LocalApps/LocalAppPlanConfirmView.swift`
- Modify: `clients/ios/Sources/LocalApps/LocalAppDesignerView.swift`（在 `awaitingSpecConfirmation` 时呈现它）

**Interfaces:**
- Consumes: Task 13 的 `LocalAppPlan`、既有的 `store.confirmDesign(appID:interactionID:revision:)`
- Produces: `LocalAppPlanConfirmView`

- [ ] **Step 1: 写失败测试**

```swift
@Test func thePlanSheetListsEveryCollectionAndField() {
    let view = LocalAppPlanConfirmView(plan: notesPlan(), onConfirm: {}, onBack: {})
    let rendered = view.summaryLines
    #expect(rendered.contains { $0.contains("notes") && $0.contains("title") })
}

@Test func thePlanSheetNamesTheDomainsItWillAllow() {
    let view = LocalAppPlanConfirmView(plan: planWithDomain("api.example.com"), onConfirm: {}, onBack: {})
    #expect(view.summaryLines.contains { $0.contains("api.example.com") })
}

@Test func thePlanSheetSaysSoWhenNoNetworkAccessIsRequested() {
    let view = LocalAppPlanConfirmView(plan: notesPlan(), onConfirm: {}, onBack: {})
    #expect(
        view.summaryLines.contains { $0.contains("不访问网络") },
        "silence about network access reads as an omission, not as a guarantee"
    )
}

@Test func theSheetHasExactlyTwoExits() {
    let view = LocalAppPlanConfirmView(plan: notesPlan(), onConfirm: {}, onBack: {})
    #expect(view.actionTitles == ["返回修改", "确认并生成"])
}
```

- [ ] **Step 2: 运行测试确认失败**

Run: `cd clients/ios && swift test --filter LocalAppPlanConfirm 2>&1 | tail -30`
Expected: 类型不存在。

- [ ] **Step 3: 写实现**

只读摘要 sheet，四个分节：`summary` 正文、数据表（每个 collection 一组，列出字段名与类型）、权限、外部域名（空时明写「不访问网络」——沉默会被读成遗漏而不是保证）。底部两个按钮：「返回修改」dismiss，「确认并生成」调 `store.confirmDesign`。

**做成独立 sheet 而不是问卷的第 N 步**：它不是问卷的一部分，不该继承步骤条的编辑语义（不能在它上面「上一步/下一步」地翻）。

在 `LocalAppDesignerView` 里以 `.sheet(isPresented:)` 呈现，条件是 `app.workflow == .awaitingSpecConfirmation && store.plans[appID] != nil`。

- [ ] **Step 4: 运行测试确认通过**

Run: `cd clients/ios && swift test --filter LocalAppPlanConfirm 2>&1 | tail -20`
Expected: 全绿。

- [ ] **Step 5: 提交**

```bash
git add clients/ios/Sources/LocalApps
git commit -m "$(cat <<'EOF'
feat(ios): 方案确认 sheet

只读展示数据表/权限/域名/摘要，两个出口：返回修改、确认并生成。
无网络需求时明写「不访问网络」—— 沉默会被读成遗漏而非保证。
独立 sheet 而非问卷的一步，不继承步骤条的编辑语义。

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
EOF
)"
```

---

### Task 16: iOS 创建入口与常驻迭代输入

**Files:**
- Modify: `clients/ios/Sources/LocalApps/LocalAppsLibraryView.swift:370-425`（`LocalAppCreateSheet`）
- Modify: `clients/ios/Sources/LocalApps/LocalAppDetailView.swift:476-499`

**Interfaces:**
- Consumes: Task 13 的 `store.createApp(brief:)`、既有的 `store.requestRevision(appID:feedback:)`
- Produces: 只要一句话的创建 sheet；`ready` 态常驻的迭代输入条

- [ ] **Step 1: 写失败测试**

```swift
@Test func theCreateSheetAsksOnlyForADescription() {
    let sheet = LocalAppCreateSheet(store: makeStore(), path: .constant([]))
    #expect(sheet.inputFieldCount == 1, "no template picker, no name field — one line is enough")
}

@Test func theCreateSheetRefusesAnEmptyDescription() {
    var sheet = LocalAppCreateSheet(store: makeStore(), path: .constant([]))
    sheet.brief = "   "
    #expect(sheet.canSubmit == false)
}

@Test func aReadyAppShowsAPersistentRevisionInput() {
    #expect(LocalAppDetailView.showsRevisionInput(for: .ready))
    #expect(LocalAppDetailView.showsRevisionInput(for: .awaitingPreviewConfirmation))
}

@Test func anAppStillGeneratingDoesNotShowTheRevisionInput() {
    #expect(LocalAppDetailView.showsRevisionInput(for: .generating) == false)
    #expect(LocalAppDetailView.showsRevisionInput(for: .authoringQuestionnaire) == false)
}
```

- [ ] **Step 2: 运行测试确认失败**

Run: `cd clients/ios && swift test --filter "LocalAppCreateSheet|LocalAppDetail" 2>&1 | tail -30`
Expected: 创建 sheet 仍带模版选择器；`showsRevisionInput` 不存在。

- [ ] **Step 3: 写实现**

`LocalAppCreateSheet` 删掉 `template` 参数、模版选择器与名字输入，只留一个多行描述框 + 提交按钮，提交调 `store.createApp(brief:)`。`LocalAppsLibraryView:370` 的呈现处同步去掉 `template` 实参。

`LocalAppDetailView` 加静态 `showsRevisionInput(for:)`（`ready` 与 `awaitingPreviewConfirmation` 为 true，其余 false），并在底部以 `safeAreaInset` 挂一条常驻输入条，提交复用**已有的** `store.requestRevision(appID:feedback:)`——不是新建能力，是把入口从一次性的反馈 sheet 扩展成持续可用的输入。原有的反馈 sheet 保留，两者走同一个调用。

- [ ] **Step 4: 运行测试确认通过**

Run: `cd clients/ios && swift test 2>&1 | tail -20`
Expected: 全绿。

- [ ] **Step 5: 全量验证并提交**

```bash
cargo test --workspace --all-features --no-fail-fast 2>&1 | tail -30
cd clients/ios && swift test 2>&1 | tail -20
```

Expected: 两边全绿，Rust 侧总测试数不低于 Task 4 后记录的数字。

```bash
git add clients/ios
git commit -m "$(cat <<'EOF'
feat(ios): 一句话创建 + ready 态常驻迭代输入

创建 sheet 去掉模版选择器与名字输入，只要一句描述。
详情页在 ready / 预览门两态常驻一条修改输入，复用已有的
requestRevision —— 入口从一次性的反馈 sheet 变成持续可用。

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
EOF
)"
```

---

## 收尾检查

全部 task 完成后逐条确认：

- [ ] `cargo test --workspace --all-features --no-fail-fast` 全绿。**必须带 `--all-features`**——local-apps 的 engine-mobile 侧模块全在 `uniffi` 门控内。
- [ ] `grep -rn "AppTemplateKind\|AppTemplateDto\|render_app_shell_source\|APP_SHELL_TEMPLATE" lingxi-code/ --include=*.rs` 零命中（排除 `target/`）。
- [ ] `grep -rn "LocalAppTemplate" clients/ios/Sources` 零命中。
- [ ] CHANGELOG 记一条 breaking：模版时代的 `apps/index.json` 不再可读。
- [ ] 真机手测一遍完整链路：一句话创建 → 出题 → 答题（含 `Other…` 与「由你决定」各一次）→ 生成方案 → 确认 → 预览 → 用自然语言改两轮 → 回滚一次 checkpoint。这条链路里没有一步是被自动化测试端到端覆盖的。
