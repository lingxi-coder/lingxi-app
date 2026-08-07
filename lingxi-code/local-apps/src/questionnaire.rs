//! 问卷与方案的领域类型和校验器。
//!
//! 出题与出方案都由 LLM 完成，本模块是那些输出唯一的验收关卡:
//! LLM 的输出是提议，这里的判定才是事实。纯函数，零 I/O。

use crate::error::AppError;
use crate::manifest::{DataCollectionSchema, DataFieldKind, DataFieldSchema};
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
/// Byte cap mirroring `AppManifest::validate`'s collection name / field
/// label limit (manifest.rs:158/182: `> 200` bytes). A char cap alone
/// (`MAX_LABEL_CHARS`) is not enough here — up to 80 CJK characters can
/// exceed 200 bytes in UTF-8, so a plan that satisfies `MAX_LABEL_CHARS`
/// could still be rejected by the manifest downstream of the human gate
/// (review NEW-2). Only applies to the two plan-carried labels that
/// actually flow into the manifest (`DataCollectionSchema::name`,
/// `DataFieldSchema::label`) — questionnaire step/field labels never reach
/// the manifest and keep the char-only cap.
pub const MAX_LABEL_BYTES: usize = 200;
/// Mirrors `AppManifest::validate`'s enum option count limit
/// (manifest.rs:191: `1..=100`).
pub const MAX_ENUM_OPTIONS: usize = 100;
/// Mirrors `AppManifest::validate`'s per-option byte limit
/// (manifest.rs:202: `> 500`).
pub const MAX_ENUM_OPTION_BYTES: usize = 500;

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

/// Like `ensure_chars`, but for the two plan labels that flow straight into
/// `AppManifest` (`DataCollectionSchema::name`, `DataFieldSchema::label`):
/// also rejects an empty (post-trim) label and enforces a BYTE cap
/// alongside the char cap — a CJK label can clear `max_chars` while still
/// exceeding the manifest's byte limit. See `MAX_LABEL_BYTES`.
fn ensure_label(context: &str, text: &str, max_chars: usize, max_bytes: usize) -> Result<(), AppError> {
    if text.trim().is_empty() {
        return Err(reject(format!("{context} must not be empty")));
    }
    ensure_chars(context, text, max_chars)?;
    if text.len() > max_bytes {
        return Err(reject(format!("{context} exceeds {max_bytes} bytes")));
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
        ensure_label("collection name", &collection.name, MAX_LABEL_CHARS, MAX_LABEL_BYTES)?;
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
            ensure_label("collection field label", &field.label, MAX_LABEL_CHARS, MAX_LABEL_BYTES)?;
            // Mirrors `AppManifest::validate`'s `match field.kind` exactly
            // (manifest.rs:189-217): an enum field needs 1..=MAX_ENUM_OPTIONS
            // unique, non-empty, <=MAX_ENUM_OPTION_BYTES options; a non-enum
            // field must not carry any. Previously this only checked
            // "an enum field has at least one option" — every other manifest
            // rule on this field was a plan-step no-op that failed only once
            // it reached `manifest.validate()` past the human gate (review
            // NEW-2).
            match field.kind {
                DataFieldKind::Enum => {
                    if field.enum_options.is_empty() || field.enum_options.len() > MAX_ENUM_OPTIONS {
                        return Err(reject(format!(
                            "enum field `{}` must declare 1..={MAX_ENUM_OPTIONS} options",
                            field.id
                        )));
                    }
                    let unique: BTreeSet<&str> =
                        field.enum_options.iter().map(String::as_str).collect();
                    if unique.len() != field.enum_options.len()
                        || field.enum_options.iter().any(|option| {
                            option.is_empty() || option.len() > MAX_ENUM_OPTION_BYTES
                        })
                    {
                        return Err(reject(format!(
                            "enum field `{}` has duplicate, empty, or oversized options",
                            field.id
                        )));
                    }
                }
                _ if !field.enum_options.is_empty() => {
                    return Err(reject(format!(
                        "non-enum field `{}` cannot declare enum options",
                        field.id
                    )));
                }
                _ => {}
            }
        }
    }
    if plan.domains.len() > MAX_DOMAINS {
        return Err(reject(format!(
            "plan declares {} domains, the limit is {MAX_DOMAINS}",
            plan.domains.len()
        )));
    }
    // Case-insensitive dedup: `normalize_plan` is expected to have already
    // folded case and dropped duplicates by the time a plan reaches here
    // (`AppState::plan_ready`), but `validate_plan` is the actual gate — it
    // must reject on its own, not merely trust an upstream normalization
    // step, or a caller that skips normalization silently produces a plan
    // `AppManifest::validate` then rejects as a "duplicate allowed domain"
    // past the human gate (review NEW-2).
    let mut seen_domains = BTreeSet::new();
    for domain in &plan.domains {
        validate_domain(domain)?;
        if !seen_domains.insert(domain.to_ascii_lowercase()) {
            return Err(reject(format!("duplicate domain `{domain}`")));
        }
    }
    Ok(())
}

/// Normalize a plan's domains before `validate_plan`/storage: fold each to
/// lowercase and drop case-insensitive duplicates, keeping first-seen order.
///
/// `validate_domain` already lowercases internally to decide validity, but
/// historically the *raw* (possibly mixed-case) value was what got stored in
/// `draft.plan` and later copied verbatim into `AppManifest::allowed_domains`
/// — which requires `domain == domain.to_ascii_lowercase()` and rejects
/// duplicates outright. An LLM-authored domain like `API.Example.com` is not
/// wrong, just differently cased, so this repairs it rather than bouncing
/// the plan back to the LLM for a cosmetic retry; genuinely invalid domains
/// (IP literals, loopback, malformed labels) are still rejected by
/// `validate_domain` inside `validate_plan`, which MUST run after this.
pub fn normalize_plan(plan: &mut AppPlan) {
    let mut seen = BTreeSet::new();
    plan.domains.retain_mut(|domain| {
        *domain = domain.to_ascii_lowercase();
        seen.insert(domain.clone())
    });
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

    // review NEW-2: `validate_plan` disagreed with the stricter
    // `AppManifest::validate` that now consumes a confirmed plan
    // (`reconcile_manifest`, `local_apps_generation.rs:153`) in six ways, so
    // every one of these plans used to pass `validate_plan` and only die at
    // scaffold, past the human confirmation gate. Each case below asserts
    // the plan now fails HERE instead.

    #[test]
    fn rejects_an_empty_collection_name() {
        let mut plan = plan_with_domain("api.example.com");
        plan.collections[0].name = "   ".into();
        let error = validate_plan(&plan).expect_err("an empty display name must be rejected");
        assert!(
            format!("{error}").contains("empty"),
            "message names the offending rule: {error}"
        );
    }

    #[test]
    fn rejects_an_80_char_cjk_collection_name_that_exceeds_the_manifest_byte_limit() {
        // 80 CJK characters clears `MAX_LABEL_CHARS` (a char cap) but each
        // character is 3 UTF-8 bytes, so 80 * 3 = 240 bytes blows past the
        // manifest's 200-byte cap (`MAX_LABEL_BYTES`) — a char-only cap
        // cannot catch this.
        let mut plan = plan_with_domain("api.example.com");
        let label: String = std::iter::repeat('测').take(80).collect();
        assert_eq!(label.chars().count(), 80, "stays within MAX_LABEL_CHARS");
        assert!(label.len() > MAX_LABEL_BYTES, "but exceeds MAX_LABEL_BYTES in UTF-8");
        plan.collections[0].name = label;
        validate_plan(&plan).expect_err("a byte cap must catch what the char cap misses");
    }

    #[test]
    fn rejects_a_non_enum_field_that_carries_enum_options() {
        let mut plan = plan_with_domain("api.example.com");
        // `kind` defaults to `Text` from `plan_with_domain`.
        plan.collections[0].fields[0].enum_options = vec!["a".into()];
        let error = validate_plan(&plan).expect_err("only enum fields may declare options");
        assert!(
            format!("{error}").contains("enum options"),
            "message names the offending rule: {error}"
        );
    }

    #[test]
    fn rejects_more_than_100_enum_options() {
        let mut plan = plan_with_domain("api.example.com");
        plan.collections[0].fields[0].kind = DataFieldKind::Enum;
        plan.collections[0].fields[0].enum_options =
            (0..101).map(|i| format!("opt{i}")).collect();
        validate_plan(&plan).expect_err("more than 100 enum options must be rejected");
    }

    #[test]
    fn rejects_duplicate_enum_options() {
        let mut plan = plan_with_domain("api.example.com");
        plan.collections[0].fields[0].kind = DataFieldKind::Enum;
        plan.collections[0].fields[0].enum_options = vec!["a".into(), "a".into()];
        validate_plan(&plan).expect_err("duplicate enum options must be rejected");
    }

    #[test]
    fn rejects_an_empty_enum_option() {
        let mut plan = plan_with_domain("api.example.com");
        plan.collections[0].fields[0].kind = DataFieldKind::Enum;
        plan.collections[0].fields[0].enum_options = vec!["a".into(), String::new()];
        validate_plan(&plan).expect_err("an empty enum option must be rejected");
    }

    #[test]
    fn rejects_an_enum_option_over_500_bytes() {
        let mut plan = plan_with_domain("api.example.com");
        plan.collections[0].fields[0].kind = DataFieldKind::Enum;
        plan.collections[0].fields[0].enum_options = vec!["x".repeat(501)];
        validate_plan(&plan).expect_err("an oversized enum option must be rejected");
    }

    #[test]
    fn rejects_duplicate_domains_even_when_validate_plan_is_called_directly() {
        // `normalize_plan` is expected to dedupe before this ever runs in
        // production (`AppState::plan_ready`), but `validate_plan` is the
        // actual gate and must not rely on that: a caller that skips
        // normalization must still be refused here, not by the manifest.
        let mut plan = plan_with_domain("api.example.com");
        plan.domains.push("API.example.com".into());
        let error = validate_plan(&plan).expect_err("duplicate domains must be rejected");
        assert!(
            format!("{error}").contains("duplicate"),
            "message names the offending rule: {error}"
        );
    }

    #[test]
    fn normalize_plan_lowercases_domains() {
        let mut plan = plan_with_domain("API.Example.com");
        normalize_plan(&mut plan);
        assert_eq!(plan.domains, vec!["api.example.com".to_string()]);
    }

    #[test]
    fn normalize_plan_drops_case_insensitive_duplicates_keeping_first_seen_order() {
        let mut plan = plan_with_domain("api.example.com");
        plan.domains = vec!["Api.Example.com".into(), "OTHER.example.com".into(), "api.EXAMPLE.com".into()];
        normalize_plan(&mut plan);
        assert_eq!(
            plan.domains,
            vec!["api.example.com".to_string(), "other.example.com".to_string()]
        );
    }

    #[test]
    fn a_plan_normalized_then_validated_no_longer_trips_the_manifests_rules() {
        // End-to-end proof for the two divergences `normalize_plan` fixes
        // rather than rejects: build the exact plan the table's first row
        // describes (mixed-case, duplicated domain), run it through the real
        // `plan_ready` order (normalize, then validate), and confirm the
        // resulting `AppManifest` — the thing that actually gates
        // generation — also accepts it.
        let mut plan = plan_with_domain("API.Example.com");
        plan.domains.push("api.example.com".into());
        normalize_plan(&mut plan);
        validate_plan(&plan).expect("normalized plan passes its own gate");

        let mut manifest = crate::manifest::AppManifest::for_new_app("app-1", "App");
        manifest.collections = plan.collections;
        manifest.allowed_domains = plan.domains;
        manifest
            .validate()
            .expect("the value validate_plan approved is also what the manifest approves");
    }
}
