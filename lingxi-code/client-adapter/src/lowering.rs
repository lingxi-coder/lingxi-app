//! Pure `From<engine type>` lowering fns — the parity surface (plan F1-11).
//!
//! This module is the single place every engine runtime type is mechanically
//! lowered into a `client-protocol` DTO. The fns are PURE (no `async`, no I/O,
//! no engine calls) so both transports (bridge-server WS, mobile `UniFFI`) and the
//! parity tests (F1-15) share one mapping. Keeping the mapping here — not inline
//! in the `OutputStream`/`PermissionGate` impls — is why structural parity is
//! testable without driving a live turn.
//!
//! ## Lowering rules (plan F1-11)
//!
//! - `serde_json::Value` → JSON **String** (`input_json` / `result_json`); this
//!   crate is the ONLY one allowed to touch `serde_json` (decision §0.4).
//! - `SystemTime` → RFC 3339 `String`, byte-identical to
//!   `session::jsonl::loader::format_rfc3339_seconds` so the session-picker
//!   timestamp renders the same on every surface (parity, plan line 152).
//! - `Duration` → `u64` whole seconds; `usize` → `u32` (saturating — a row count
//!   never realistically exceeds `u32::MAX`, and saturating is lossless in
//!   practice while avoiding a panic).
//! - `McpStatus::Error(String)` (tuple) → [`McpStatusDto::Error { reason }`]
//!   (struct, for `UniFFI` flatness — decision §0.4 / plan line 154).
//! - `PromptDefault` → `bool` (`AllowByDefault` ⇒ `true`) — the collapsed
//!   `default_allow` carried by `PermissionKindDto::ToolUseConfirm`.
//! - `CostSnapshot` → [`CostDto`] (`Duration` → secs; `formatted` rendered as
//!   `"${:.4}"`, matching `tui::events::orchestrator_bridge` line 152 parity).
//! - `SessionMetadata` → [`SessionRowDto`] (`.path` mapped DIRECTLY — it is a
//!   real field, NOT synthesized, plan line 152).
//! - `McpServerInfo` / `HookInfo` / `AgentInfo` / `StatusSnapshot` /
//!   `DoctorReport` / `TaskRecord` / `TaskOutputChunk` → their DTOs.
//!
//! `GroupedToolUse` / `CollapsedReadSearch` folding is deliberately NOT here —
//! that stays CLIENT-SIDE; the adapter emits the raw `ToolUseStarted` /
//! `ToolUseResult` per the events catalog (plan F1-11).

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use client_protocol::events::CostDto;
use client_protocol::listings::{
    AgentDto, CheckStatusDto, CoordinatorWorkerDto, DoctorCheckDto, DoctorReportDto,
    DoctorSummaryDto, HookDto, McpServerDto, McpStatusDto, ModelBillingModeDto,
    ModelCapabilitiesDto, ModelDetailsDto, ModelPricingDto, ModelPricingTierDto, SessionModeDto,
    SessionRowDto, SkillDto, StatusSnapshotDto, TaskRowDto, TaskStatusDto,
};
use client_protocol::message::{MessageBlockDto, MessageDto, MessageImageDto};

use protocol::ConversationMessage;

use permission::PromptDefault;
use platform_api::orchestrator::{
    AgentInfo, CheckStatus, CostSnapshot, DoctorCheck, DoctorReport, DoctorSummary, HookInfo,
    McpServerInfo, McpStatus, SkillInfo, StatusSnapshot,
};
use platform_api::task_registry::{TaskOutputChunk, TaskRecord};
use platform_api::team_registry::WorkerInfo;
use session::jsonl::loader::SessionMetadata;

fn lower_reasoning_selection(
    selection: &platform_api::ReasoningSelection,
) -> client_protocol::controls::ReasoningSelectionDto {
    use client_protocol::controls::ReasoningSelectionDto;
    match selection {
        platform_api::ReasoningSelection::Automatic => ReasoningSelectionDto::Automatic,
        platform_api::ReasoningSelection::Disabled => ReasoningSelectionDto::Disabled,
        platform_api::ReasoningSelection::Enabled => ReasoningSelectionDto::Enabled,
        platform_api::ReasoningSelection::Level { id } => {
            ReasoningSelectionDto::Level { id: id.clone() }
        }
        platform_api::ReasoningSelection::TokenBudget { tokens } => {
            ReasoningSelectionDto::TokenBudget { tokens: *tokens }
        }
    }
}

/// Lower the exact route-level reasoning contract used by request validation.
#[must_use]
pub fn lower_reasoning_control_spec(
    spec: &platform_api::ReasoningControlSpec,
) -> client_protocol::controls::ReasoningControlSpecDto {
    use client_protocol::controls::{
        ControlDisabledReasonDto, ReasoningBudgetRangeDto, ReasoningControlSpecDto,
        ReasoningOptionDto,
    };
    ReasoningControlSpecDto {
        options: spec
            .available
            .iter()
            .map(|selection| ReasoningOptionDto {
                selection: lower_reasoning_selection(selection),
                persistable: spec.selections_persistable,
            })
            .collect(),
        budget_range: spec
            .budget_range
            .as_ref()
            .map(|range| ReasoningBudgetRangeDto {
                min_tokens: u64::from(range.min_tokens),
                max_tokens: u64::from(range.max_tokens),
            }),
        provider_default: lower_reasoning_selection(&spec.provider_default),
        forced_reasoning: spec.forced,
        editable: spec.modifiable,
        disabled_reason: spec
            .disabled_reason
            .as_ref()
            .map(|code| ControlDisabledReasonDto {
                code: code.clone(),
                message: None,
            }),
    }
}

/// Lower one provider-qualified model listing without inventing missing facts.
#[must_use]
pub fn lower_model_details(listing: &platform_api::ModelListing) -> ModelDetailsDto {
    let pricing = listing
        .metadata
        .pricing
        .as_ref()
        .map(|pricing| ModelPricingDto {
            billing_mode: match pricing.billing_mode {
                platform_api::ModelBillingMode::PerToken => ModelBillingModeDto::PerToken,
                platform_api::ModelBillingMode::Subscription => ModelBillingModeDto::Subscription,
                platform_api::ModelBillingMode::Free => ModelBillingModeDto::Free,
                platform_api::ModelBillingMode::Unknown => ModelBillingModeDto::Unknown,
            },
            input_per_million: pricing.input_per_million,
            output_per_million: pricing.output_per_million,
            cache_read_per_million: pricing.cache_read_per_million,
            cache_write_per_million: pricing.cache_write_per_million,
            reasoning_per_million: pricing.reasoning_per_million,
            tiers: pricing
                .tiers
                .iter()
                .map(|tier| ModelPricingTierDto {
                    context_threshold_tokens: tier.context_threshold_tokens,
                    input_per_million: tier.input_per_million,
                    output_per_million: tier.output_per_million,
                    cache_read_per_million: tier.cache_read_per_million,
                    cache_write_per_million: tier.cache_write_per_million,
                    reasoning_per_million: tier.reasoning_per_million,
                })
                .collect(),
            source: pricing.source.clone(),
        });
    ModelDetailsDto {
        reference: platform_api::qualified_model_ref(
            &listing.request_model,
            Some(&listing.provider_id),
        ),
        provider_id: listing.provider_id.clone(),
        provider_label: listing.provider_label.clone(),
        display_name: listing.display_model.clone(),
        model_id: listing.request_model.clone(),
        description: listing.description.clone(),
        family: listing.metadata.family.clone(),
        status: listing.metadata.status.clone(),
        release_date: listing.metadata.release_date.clone(),
        last_updated: listing.metadata.last_updated.clone(),
        knowledge_cutoff: listing.metadata.knowledge_cutoff.clone(),
        input_modalities: listing.metadata.input_modalities.clone(),
        output_modalities: listing.metadata.output_modalities.clone(),
        context_window_tokens: listing.metadata.context_window_tokens,
        max_input_tokens: listing.metadata.max_input_tokens,
        max_output_tokens: listing.metadata.max_output_tokens,
        open_weights: listing.metadata.open_weights,
        attachments: listing.metadata.attachments,
        temperature_control: listing.metadata.temperature_control,
        pricing,
        capabilities: ModelCapabilitiesDto {
            streaming: listing.capabilities.streaming,
            tools: listing.capabilities.tools,
            vision: listing.capabilities.vision,
            documents: listing.capabilities.documents,
            reasoning: listing.capabilities.reasoning,
            structured_output: listing.capabilities.structured_output,
        },
        reasoning: lower_reasoning_control_spec(&listing.reasoning),
        supports_fast_mode: listing.provider_id == "anthropic"
            && platform_api::model_capabilities::has_capability(
                &listing.request_model,
                platform_api::model_capabilities::ModelCapability::FastMode,
            ),
    }
}

// ── Primitive lowering rules ───────────────────────────────────────────────

/// Lower a tool input/result `serde_json::Value` to its JSON **String** wire
/// form (`input_json` / `result_json`).
///
/// This is the boundary decision §0.4 names: `serde_json::Value` is not
/// UniFFI-representable, so it never enters `client-protocol`. The string keeps
/// `preserve_order` key ordering (the workspace `serde_json` feature) so the
/// lowered bytes match the inbound tool-call bytes. `to_string` on a `Value`
/// cannot fail, so this is infallible.
#[must_use]
pub fn value_to_json_string(value: &serde_json::Value) -> String {
    value.to_string()
}

/// Lower a `SystemTime` to a seconds-resolution RFC 3339 `String`.
///
/// Byte-identical to `session::jsonl::loader::format_rfc3339_seconds` (the
/// session-picker / resume-screen helper) so the lowered timestamp renders the
/// same on every surface. Pre-1970 inputs (never produced by file mtime on the
/// platforms we target) fall back to the Unix-epoch literal.
#[must_use]
pub fn system_time_to_rfc3339(t: SystemTime) -> String {
    let secs = t
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    #[allow(clippy::cast_possible_wrap)]
    let secs_i64 = secs as i64;
    chrono::DateTime::<chrono::Utc>::from_timestamp(secs_i64, 0).map_or_else(
        || "1970-01-01T00:00:00Z".to_string(),
        |dt| dt.format("%Y-%m-%dT%H:%M:%SZ").to_string(),
    )
}

/// Lower a `Duration` to whole seconds (`session_duration` → `*_secs`).
#[must_use]
pub fn duration_to_secs(d: Duration) -> u64 {
    d.as_secs()
}

/// Lower a `usize` count to a `u32` wire field, saturating at `u32::MAX`.
///
/// A message / row count never realistically exceeds `u32::MAX`; saturating is
/// lossless in practice and avoids a `try_into` panic on a pathological input.
#[must_use]
pub fn usize_to_u32(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

/// Lower a `PromptDefault` to the collapsed `default_allow: bool` carried by
/// `PermissionKindDto::ToolUseConfirm`. `AllowByDefault` ⇒ `true`.
#[must_use]
pub fn prompt_default_to_allow(d: PromptDefault) -> bool {
    matches!(d, PromptDefault::AllowByDefault)
}

// ── Enum lowering rules ────────────────────────────────────────────────────

/// Lower the engine's `McpStatus` (with a tuple `Error(String)`) to the DTO's
/// STRUCT-variant `McpStatusDto::Error { reason }` (`UniFFI` flatness, §0.4).
#[must_use]
pub fn lower_mcp_status(status: &McpStatus) -> McpStatusDto {
    match status {
        McpStatus::Connected => McpStatusDto::Connected,
        McpStatus::Disconnected => McpStatusDto::Disconnected,
        McpStatus::Error(reason) => McpStatusDto::Error {
            reason: reason.clone(),
        },
    }
}

/// Lower a `/doctor` `CheckStatus` to its DTO.
#[must_use]
pub fn lower_check_status(status: &CheckStatus) -> CheckStatusDto {
    match status {
        CheckStatus::Pass => CheckStatusDto::Pass,
        CheckStatus::Warn => CheckStatusDto::Warn,
        CheckStatus::Fail => CheckStatusDto::Fail,
    }
}

/// Lower a `TaskRecord`'s `status` wire `String` to a [`TaskStatusDto`].
///
/// The engine's terminal `"killed"` wire status maps to
/// [`TaskStatusDto::Cancelled`] (the DTO's user-stop variant — the names were
/// reconciled in F1-05). An unrecognized status falls back to
/// [`TaskStatusDto::Pending`] (the safest non-terminal default — a future
/// status would be additive on the `#[non_exhaustive]` enum).
#[must_use]
pub fn lower_task_status(wire: &str) -> TaskStatusDto {
    match wire {
        "running" => TaskStatusDto::Running,
        "paused" => TaskStatusDto::Paused,
        "completed" => TaskStatusDto::Completed,
        "failed" => TaskStatusDto::Failed,
        "killed" => TaskStatusDto::Cancelled,
        // "pending" and any unknown/future status fall back to Pending.
        _ => TaskStatusDto::Pending,
    }
}

// ── Struct lowering rules ──────────────────────────────────────────────────

/// Lower a `CostSnapshot` to a [`CostDto`] (the display-relevant fields).
///
/// `session_duration` (`Duration`) → `session_duration_secs` (`u64`);
/// `formatted` is rendered `"${:.4}"`, matching the TUI bridge (parity).
#[must_use]
pub fn lower_cost_snapshot(cost: &CostSnapshot) -> CostDto {
    CostDto {
        total_usd: cost.total_usd,
        input_tokens: cost.input_tokens,
        output_tokens: cost.output_tokens,
        api_calls: cost.api_calls,
        session_duration_secs: duration_to_secs(cost.session_duration),
        formatted: format!("${:.4}", cost.total_usd),
    }
}

/// Lower a `SessionMetadata` to a [`SessionRowDto`].
///
/// `uuid` → its string form; `modified` (`SystemTime`) → RFC 3339;
/// `message_count` (`usize`) → `u32`; `path` (`PathBuf`) is mapped DIRECTLY via
/// a lossy display string (plan line 152 — not synthesized).
#[must_use]
pub fn lower_session_metadata(meta: &SessionMetadata) -> SessionRowDto {
    SessionRowDto {
        uuid: meta.uuid.to_string(),
        title: meta.title.clone(),
        modified_rfc3339: system_time_to_rfc3339(meta.modified),
        message_count: usize_to_u32(meta.message_count),
        mode: match meta.mode {
            session::jsonl::SessionMode::Chat => SessionModeDto::Chat,
            session::jsonl::SessionMode::Code => SessionModeDto::Code,
        },
        path: meta.path.to_string_lossy().into_owned(),
    }
}

/// Lower a `McpServerInfo` to a [`McpServerDto`].
#[must_use]
pub fn lower_mcp_server_info(info: &McpServerInfo) -> McpServerDto {
    McpServerDto {
        name: info.name.clone(),
        status: lower_mcp_status(&info.status),
        transport: info.transport.clone(),
    }
}

/// Lower a `SkillInfo` to a [`SkillDto`]. `source_dir` (a `PathBuf`) is
/// rendered as a display string, matching `lower_session_metadata`'s
/// `path` handling — not synthesized, the engine field mapped directly.
#[must_use]
pub fn lower_skill_info(info: &SkillInfo) -> SkillDto {
    SkillDto {
        name: info.name.clone(),
        source_dir: info.source_dir.to_string_lossy().into_owned(),
    }
}

/// Lower a `HookInfo` to a [`HookDto`].
#[must_use]
pub fn lower_hook_info(info: &HookInfo) -> HookDto {
    HookDto {
        name: info.name.clone(),
        event: info.event.clone(),
        matcher: info.matcher.clone(),
        timeout_ms: info.timeout_ms,
    }
}

/// Lower an `AgentInfo` to an [`AgentDto`].
#[must_use]
pub fn lower_agent_info(info: &AgentInfo) -> AgentDto {
    AgentDto {
        name: info.name.clone(),
        description: info.description.clone(),
        tools_allowed: info.tools_allowed.clone(),
    }
}

/// Lower a `StatusSnapshot` to a [`StatusSnapshotDto`].
///
/// The traits-shape fields map 1:1; the appended optional `status_line` is left
/// `None` here (it is a status-line addition the engine struct does not carry —
/// plan line 155 — so a caller with a pre-rendered line sets it after lowering).
///
/// The engine's `active_workers` (`u32`) is carried through as
/// `Some(active_workers)` (T21) so `/status` echoes the live coordinator-team
/// worker count. It is `0` (still emitted as `Some(0)`) for a non-coordinator
/// session — the wire skip happens only when the optional is `None`, which this
/// lowering never produces, matching the always-present engine field.
#[must_use]
pub fn lower_status_snapshot(s: &StatusSnapshot) -> StatusSnapshotDto {
    StatusSnapshotDto {
        session_id: s.session_id.clone(),
        model: s.model.clone(),
        n_messages: s.n_messages,
        total_cost_usd: s.total_cost_usd,
        input_tokens: s.input_tokens,
        output_tokens: s.output_tokens,
        n_mcp_connected: s.n_mcp_connected,
        n_mcp_total: s.n_mcp_total,
        n_hooks: s.n_hooks,
        n_agents: s.n_agents,
        started_at: s.started_at.clone(),
        cwd: s.cwd.to_string_lossy().into_owned(),
        status_line: None,
        active_workers: Some(s.active_workers),
    }
}

/// Lower a `/doctor` `DoctorCheck` to a [`DoctorCheckDto`].
#[must_use]
pub fn lower_doctor_check(check: &DoctorCheck) -> DoctorCheckDto {
    DoctorCheckDto {
        name: check.name.clone(),
        status: lower_check_status(&check.status),
        detail: check.detail.clone(),
    }
}

/// Lower a `DoctorSummary` to a [`DoctorSummaryDto`].
#[must_use]
pub fn lower_doctor_summary(summary: &DoctorSummary) -> DoctorSummaryDto {
    DoctorSummaryDto {
        passed: summary.passed,
        warnings: summary.warnings,
        failed: summary.failed,
    }
}

/// Lower a `DoctorReport` to a [`DoctorReportDto`] (checks + summary).
#[must_use]
pub fn lower_doctor_report(report: &DoctorReport) -> DoctorReportDto {
    DoctorReportDto {
        checks: report.checks.iter().map(lower_doctor_check).collect(),
        summary: lower_doctor_summary(&report.summary),
    }
}

/// Lower a `TaskRecord` to a [`TaskRowDto`] (the `status` wire `String` is
/// lowered to a [`TaskStatusDto`] enum).
#[must_use]
pub fn lower_task_record(rec: &TaskRecord) -> TaskRowDto {
    TaskRowDto {
        task_id: rec.task_id.clone(),
        task_type: rec.task_type.clone(),
        status: lower_task_status(&rec.status),
        description: rec.description.clone(),
        // The reduced task record intentionally keeps no script/checkpoint
        // paths. The concrete mobile command performs the stronger metadata
        // validation before launching; this flag is only an affordance hint.
        can_resume: rec.task_type == "local_workflow" && rec.status == "paused",
        started_at_ms: rec.started_at_ms,
    }
}

/// Lower a `platform_api::team_registry::WorkerInfo` (the POD projection of the
/// coordinator's `WorkerAgent`) to a [`CoordinatorWorkerDto`] (T18).
///
/// The mapping is 1:1 — `WorkerInfo` is already the simplified roster shape that
/// the DTO and the TUI `WorkerRow` share (`agent_id` / `name` / `agent_type` /
/// `status`, with `status` a plain label `String`). No `WorkerStatus` enum is
/// touched here; the simplification happens in the `coordinator`-side
/// `TeamRegistryHandle` impl (T17).
#[must_use]
pub fn lower_worker_agent(info: &WorkerInfo) -> CoordinatorWorkerDto {
    CoordinatorWorkerDto {
        agent_id: info.agent_id.clone(),
        name: info.name.clone(),
        agent_type: info.agent_type.clone(),
        status: info.status.clone(),
    }
}

/// Lower one [`ConversationMessage`] to a [`MessageDto`] — the resumed-scrollback
/// twin of the per-turn [`crate::turn::synthesize_message`].
///
/// The role is the message's wire role (`"user"` / `"assistant"` / `"system"`);
/// the content blocks are lowered through the SAME
/// [`crate::turn::lower_content_block`] path `MessageComplete` uses, so a resumed
/// message and a live-turn message reproduce an IDENTICAL [`MessageDto`] block set
/// for any given content. Image content is projected to the message-level
/// `images` field because clients render it before the user's text row; document
/// content has no client message projection and is dropped.
///
/// A [`ConversationMessage::System`] carries a flat `content: String` (no blocks),
/// so it lowers to a single [`MessageBlockDto::Text`] — a faithful, lossless
/// scrollback rendering of the system body.
#[must_use]
pub fn lower_conversation_message(message: &ConversationMessage) -> MessageDto {
    lower_conversation_message_with(message, &mut crate::turn::ToolUseIndex::default())
}

/// [`lower_conversation_message`] threading a [`crate::turn::ToolUseIndex`] so a
/// `ToolResult` can be paired with the `ToolUse` from the PREVIOUS message.
///
/// Prefer this whenever more than one message is lowered: a tool call and its
/// result are always in adjacent messages, never the same one, so a per-message
/// index can never pair them.
#[must_use]
pub fn lower_conversation_message_with(
    message: &ConversationMessage,
    index: &mut crate::turn::ToolUseIndex,
) -> MessageDto {
    match message {
        ConversationMessage::User { content, .. } => MessageDto {
            role: "user".to_string(),
            blocks: content
                .iter()
                .filter_map(|block| crate::turn::lower_content_block_with(block, index))
                .collect(),
            images: content.iter().filter_map(lower_message_image).collect(),
        },
        ConversationMessage::Assistant { content, .. } => MessageDto {
            role: "assistant".to_string(),
            blocks: content
                .iter()
                .filter_map(|block| crate::turn::lower_content_block_with(block, index))
                .collect(),
            images: Vec::new(),
        },
        ConversationMessage::System {
            subtype,
            compact_metadata,
            ..
        } if subtype.as_deref() == Some("compact_boundary") => MessageDto {
            role: "system".to_string(),
            blocks: vec![MessageBlockDto::CompactBoundary {
                messages_before: compact_metadata
                    .as_ref()
                    .and_then(|metadata| metadata.messages_summarized)
                    .unwrap_or_default(),
                messages_after: 0,
                summary: String::new(),
            }],
            images: Vec::new(),
        },
        ConversationMessage::System { content, .. } => MessageDto {
            role: "system".to_string(),
            blocks: vec![MessageBlockDto::Text {
                text: content.clone(),
            }],
            images: Vec::new(),
        },
    }
}

/// Keep persisted image bytes renderable across every client. The engine's
/// session history is already durable, so inline data becomes a URL-shaped
/// transcript value; a source that was already a URL remains a URL.
fn lower_message_image(block: &protocol::ContentBlock) -> Option<MessageImageDto> {
    let protocol::ContentBlock::Image { source } = block else {
        return None;
    };
    match source {
        protocol::ImageSource::Base64 { media_type, data } => Some(MessageImageDto {
            media_type: media_type.clone(),
            url: format!("data:{media_type};base64,{data}"),
        }),
        protocol::ImageSource::Url { url } => Some(MessageImageDto {
            media_type: String::new(),
            url: url.clone(),
        }),
    }
}

/// Lower a replayed conversation `history` to the OLDEST-FIRST [`MessageDto`]
/// transcript carried by [`ClientEvent::SessionResumed`](client_protocol::events::ClientEvent::SessionResumed).
///
/// `history` is already in chronological (oldest-first) order — the engine's
/// resume path replays the JSONL in file order — so this preserves that order
/// 1:1. Each message lowers through [`lower_conversation_message`], reusing the
/// same `ContentBlock` → `MessageBlockDto` rules as the live `MessageComplete`
/// path so the resumed scrollback is byte-identical to what a live turn would
/// have produced.
#[must_use]
pub fn lower_transcript(history: &[ConversationMessage]) -> Vec<MessageDto> {
    let mut transcript = Vec::with_capacity(history.len());
    // ONE index for the WHOLE transcript: a `ToolUse` in assistant message N
    // pairs with its `ToolResult` in user message N+1.
    let mut tool_uses = crate::turn::ToolUseIndex::default();
    for message in history {
        match message {
            ConversationMessage::User {
                content,
                is_compact_summary: true,
                ..
            } => {
                let summary = content
                    .iter()
                    .filter_map(|block| match block {
                        protocol::ContentBlock::Text { text } => Some(text.as_str()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                let Some(MessageDto { blocks, .. }) = transcript.last_mut() else {
                    continue;
                };
                let Some(MessageBlockDto::CompactBoundary {
                    summary: accumulated,
                    ..
                }) = blocks.last_mut()
                else {
                    continue;
                };
                if !summary.is_empty() {
                    if !accumulated.is_empty() {
                        accumulated.push('\n');
                    }
                    accumulated.push_str(&summary);
                }
            }
            ConversationMessage::User {
                is_visible_in_transcript_only: true,
                ..
            } => {}
            _ => transcript.push(lower_conversation_message_with(message, &mut tool_uses)),
        }
    }
    transcript
}

/// One lowered task-output chunk: the `(task_id, content, total_lines,
/// truncated)` tuple the `ClientEvent::TaskOutputChunk` variant carries.
///
/// `TaskOutputChunk` has no standalone DTO struct (it is carried inline by the
/// event), so this returns the field tuple the event is built from.
#[must_use]
pub fn lower_task_output_chunk(chunk: &TaskOutputChunk) -> (String, String, u64, bool) {
    (
        chunk.task_id.clone(),
        chunk.content.clone(),
        chunk.total_lines,
        chunk.truncated,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Primitive rules ────────────────────────────────────────────────────

    #[test]
    fn value_lowers_to_json_string() {
        let v = serde_json::json!({"file_path": "/tmp/x", "n": 3});
        let s = value_to_json_string(&v);
        // Round-trips back to the same Value (byte form not asserted — only that
        // it is a faithful JSON String).
        let back: serde_json::Value = serde_json::from_str(&s).unwrap();
        assert_eq!(back, v);
        // A primitive lowers to its bare JSON token.
        assert_eq!(value_to_json_string(&serde_json::json!("hi")), "\"hi\"");
        assert_eq!(value_to_json_string(&serde_json::Value::Null), "null");
    }

    #[test]
    fn system_time_to_rfc3339_matches_session_helper() {
        // 2021-01-01T00:00:00Z = 1_609_459_200 epoch seconds.
        let t = UNIX_EPOCH + Duration::from_secs(1_609_459_200);
        assert_eq!(system_time_to_rfc3339(t), "2021-01-01T00:00:00Z");
        // Parity anchor: identical to the session-picker helper byte-for-byte.
        assert_eq!(
            system_time_to_rfc3339(t),
            session::jsonl::loader::format_rfc3339_seconds(t)
        );
        // The epoch itself.
        assert_eq!(system_time_to_rfc3339(UNIX_EPOCH), "1970-01-01T00:00:00Z");
        // Pre-1970 falls back to the epoch literal (never produced by mtime).
        let pre = UNIX_EPOCH - Duration::from_secs(1);
        assert_eq!(system_time_to_rfc3339(pre), "1970-01-01T00:00:00Z");
    }

    #[test]
    fn duration_lowers_to_whole_secs() {
        assert_eq!(duration_to_secs(Duration::from_secs(90)), 90);
        // Sub-second remainder is truncated (whole seconds).
        assert_eq!(duration_to_secs(Duration::from_millis(1_999)), 1);
        assert_eq!(duration_to_secs(Duration::ZERO), 0);
    }

    #[test]
    fn usize_lowers_to_u32_saturating() {
        assert_eq!(usize_to_u32(0), 0);
        assert_eq!(usize_to_u32(42), 42);
        // Saturates rather than panicking on overflow.
        assert_eq!(usize_to_u32(usize::MAX), u32::MAX);
    }

    #[test]
    fn prompt_default_lowers_to_allow_bool() {
        assert!(prompt_default_to_allow(PromptDefault::AllowByDefault));
        assert!(!prompt_default_to_allow(PromptDefault::DenyByDefault));
    }

    // ── Enum rules ───────────────────────────────────────────────────────────

    #[test]
    fn mcp_error_to_struct_variant() {
        assert_eq!(
            lower_mcp_status(&McpStatus::Connected),
            McpStatusDto::Connected
        );
        assert_eq!(
            lower_mcp_status(&McpStatus::Disconnected),
            McpStatusDto::Disconnected
        );
        // The engine tuple `Error(String)` lowers to the DTO STRUCT variant.
        assert_eq!(
            lower_mcp_status(&McpStatus::Error("boom".to_string())),
            McpStatusDto::Error {
                reason: "boom".to_string()
            }
        );
    }

    #[test]
    fn check_status_lowers_each_variant() {
        assert_eq!(lower_check_status(&CheckStatus::Pass), CheckStatusDto::Pass);
        assert_eq!(lower_check_status(&CheckStatus::Warn), CheckStatusDto::Warn);
        assert_eq!(lower_check_status(&CheckStatus::Fail), CheckStatusDto::Fail);
    }

    #[test]
    fn task_status_wire_lowers_with_killed_to_cancelled() {
        assert_eq!(lower_task_status("pending"), TaskStatusDto::Pending);
        assert_eq!(lower_task_status("running"), TaskStatusDto::Running);
        assert_eq!(lower_task_status("paused"), TaskStatusDto::Paused);
        assert_eq!(lower_task_status("completed"), TaskStatusDto::Completed);
        assert_eq!(lower_task_status("failed"), TaskStatusDto::Failed);
        // The engine's terminal "killed" maps to the DTO's user-stop variant.
        assert_eq!(lower_task_status("killed"), TaskStatusDto::Cancelled);
        // Unknown / future status falls back to the safe non-terminal default.
        assert_eq!(lower_task_status("nope"), TaskStatusDto::Pending);
    }

    // ── Struct rules ─────────────────────────────────────────────────────────

    #[test]
    // The lowered `f64` cost fields are copied verbatim (no arithmetic), so an
    // exact `assert_eq!` is the correct assertion here.
    #[allow(clippy::float_cmp)]
    fn cost_snapshot_to_dto() {
        let cost = CostSnapshot {
            total_usd: 0.0123,
            input_tokens: 100,
            output_tokens: 50,
            api_calls: 3,
            session_duration: Duration::from_secs(125),
            ..Default::default()
        };
        let dto = lower_cost_snapshot(&cost);
        assert_eq!(dto.total_usd, 0.0123);
        assert_eq!(dto.input_tokens, 100);
        assert_eq!(dto.output_tokens, 50);
        assert_eq!(dto.api_calls, 3);
        assert_eq!(dto.session_duration_secs, 125);
        // 4-decimal `"${:.4}"` format, matching the TUI bridge (parity).
        assert_eq!(dto.formatted, "$0.0123");
    }

    #[test]
    fn session_metadata_to_row_maps_path_directly() {
        use std::path::PathBuf;
        let uuid = uuid::Uuid::nil();
        let meta = SessionMetadata {
            uuid,
            title: "First chat".to_string(),
            modified: UNIX_EPOCH + Duration::from_secs(1_609_459_200),
            created: UNIX_EPOCH + Duration::from_secs(1_609_459_200),
            message_count: 7,
            path: PathBuf::from("/home/u/.lingxi/sessions/abc.jsonl"),
            pr_number: None,
            custom_or_ai_title: Some("First chat".to_string()),
        };
        let row = lower_session_metadata(&meta);
        assert_eq!(row.uuid, uuid.to_string());
        assert_eq!(row.title, "First chat");
        assert_eq!(row.modified_rfc3339, "2021-01-01T00:00:00Z");
        assert_eq!(row.message_count, 7);
        // `.path` is mapped DIRECTLY (plan line 152), not synthesized.
        assert_eq!(row.path, "/home/u/.lingxi/sessions/abc.jsonl");
    }

    #[test]
    fn mcp_server_info_to_dto() {
        let info = McpServerInfo {
            name: "fs".to_string(),
            status: McpStatus::Error("handshake failed".to_string()),
            transport: "stdio".to_string(),
        };
        let dto = lower_mcp_server_info(&info);
        assert_eq!(dto.name, "fs");
        assert_eq!(dto.transport, "stdio");
        assert_eq!(
            dto.status,
            McpStatusDto::Error {
                reason: "handshake failed".to_string()
            }
        );
    }

    #[test]
    fn hook_info_to_dto_preserves_optional_matcher() {
        let with = HookInfo {
            name: "guard".to_string(),
            event: "PreToolUse".to_string(),
            matcher: Some("Bash.*".to_string()),
            timeout_ms: 5_000,
            ..HookInfo::default()
        };
        let dto = lower_hook_info(&with);
        assert_eq!(dto.name, "guard");
        assert_eq!(dto.event, "PreToolUse");
        assert_eq!(dto.matcher.as_deref(), Some("Bash.*"));
        assert_eq!(dto.timeout_ms, 5_000);

        let without = HookInfo {
            matcher: None,
            ..with
        };
        assert_eq!(lower_hook_info(&without).matcher, None);
    }

    #[test]
    fn agent_info_to_dto() {
        let info = AgentInfo {
            name: "reviewer".to_string(),
            description: "Reviews code".to_string(),
            tools_allowed: vec!["Read".to_string(), "Grep".to_string()],
            wildcard_tools: false,
            ..AgentInfo::default()
        };
        let dto = lower_agent_info(&info);
        assert_eq!(dto.name, "reviewer");
        assert_eq!(dto.description, "Reviews code");
        assert_eq!(
            dto.tools_allowed,
            vec!["Read".to_string(), "Grep".to_string()]
        );
    }

    #[test]
    // `total_cost_usd` is copied verbatim from the engine struct (no
    // arithmetic), so an exact `assert_eq!` is the correct assertion.
    #[allow(clippy::float_cmp)]
    fn status_snapshot_to_dto_leaves_status_line_none() {
        use std::path::PathBuf;
        let snap = StatusSnapshot {
            session_id: "sess-1".to_string(),
            model: "claude-opus-4-8".to_string(),
            model_profile: None,
            n_messages: 12,
            total_cost_usd: 1.5,
            input_tokens: 1_000,
            output_tokens: 500,
            n_mcp_connected: 2,
            n_mcp_total: 3,
            n_hooks: 4,
            n_agents: 1,
            started_at: "2026-06-02T00:00:00Z".to_string(),
            cwd: PathBuf::from("/work/proj"),
            active_workers: 2,
            setting_sources: Vec::new(),
        };
        let dto = lower_status_snapshot(&snap);
        assert_eq!(dto.session_id, "sess-1");
        assert_eq!(dto.model, "claude-opus-4-8");
        assert_eq!(dto.n_messages, 12);
        assert_eq!(dto.total_cost_usd, 1.5);
        assert_eq!(dto.input_tokens, 1_000);
        assert_eq!(dto.output_tokens, 500);
        assert_eq!(dto.n_mcp_connected, 2);
        assert_eq!(dto.n_mcp_total, 3);
        assert_eq!(dto.n_hooks, 4);
        assert_eq!(dto.n_agents, 1);
        assert_eq!(dto.started_at, "2026-06-02T00:00:00Z");
        assert_eq!(dto.cwd, "/work/proj");
        // The appended status-line field defaults to None on lowering (plan 155).
        assert_eq!(dto.status_line, None);
        // The engine `active_workers` (u32) is carried through as Some(n) (T21).
        assert_eq!(dto.active_workers, Some(2));
    }

    #[test]
    fn doctor_report_to_dto() {
        let report = DoctorReport {
            checks: vec![
                DoctorCheck {
                    name: "config-dir".to_string(),
                    status: CheckStatus::Pass,
                    detail: None,
                },
                DoctorCheck {
                    name: "api-key".to_string(),
                    status: CheckStatus::Fail,
                    detail: Some("missing".to_string()),
                },
            ],
            summary: DoctorSummary {
                passed: 1,
                warnings: 0,
                failed: 1,
            },
        };
        let dto = lower_doctor_report(&report);
        assert_eq!(dto.checks.len(), 2);
        assert_eq!(dto.checks[0].name, "config-dir");
        assert_eq!(dto.checks[0].status, CheckStatusDto::Pass);
        assert_eq!(dto.checks[0].detail, None);
        assert_eq!(dto.checks[1].name, "api-key");
        assert_eq!(dto.checks[1].status, CheckStatusDto::Fail);
        assert_eq!(dto.checks[1].detail.as_deref(), Some("missing"));
        assert_eq!(
            dto.summary,
            DoctorSummaryDto {
                passed: 1,
                warnings: 0,
                failed: 1
            }
        );
    }

    #[test]
    fn task_record_to_row() {
        let rec = TaskRecord {
            task_id: "b3f9zk2xq".to_string(),
            task_type: "local_bash".to_string(),
            status: "killed".to_string(),
            description: "build".to_string(),
            command: None,
            ..Default::default()
        };
        let dto = lower_task_record(&rec);
        assert_eq!(dto.task_id, "b3f9zk2xq");
        assert_eq!(dto.task_type, "local_bash");
        // "killed" wire status → Cancelled DTO variant.
        assert_eq!(dto.status, TaskStatusDto::Cancelled);
        assert_eq!(dto.description, "build");
    }

    #[test]
    fn lower_worker_agent_matches_worker_row_fixture() {
        // The `WorkerInfo` projection (T17) lowers 1:1 onto the roster DTO, which
        // itself mirrors the TUI `WorkerRow {agent_id, name, agent_type, status}`.
        let info = WorkerInfo {
            agent_id: "agent:00000000-0000-0000-0000-000000000001".to_string(),
            agent_type: "explorer".to_string(),
            name: "alpha".to_string(),
            status: "working".to_string(),
        };
        let dto = lower_worker_agent(&info);
        assert_eq!(dto.agent_id, "agent:00000000-0000-0000-0000-000000000001");
        assert_eq!(dto.name, "alpha");
        assert_eq!(dto.agent_type, "explorer");
        assert_eq!(dto.status, "working");
    }

    #[test]
    fn task_output_chunk_lowers_to_event_fields() {
        let chunk = TaskOutputChunk {
            task_id: "b3f9zk2xq".to_string(),
            content: "line1\nline2".to_string(),
            total_lines: 2,
            truncated: true,
            ..Default::default()
        };
        let (id, content, lines, truncated) = lower_task_output_chunk(&chunk);
        assert_eq!(id, "b3f9zk2xq");
        assert_eq!(content, "line1\nline2");
        assert_eq!(lines, 2);
        assert!(truncated);
    }

    // ── Transcript lowering (live ResumeSession) ─────────────────────────────

    #[test]
    fn lower_transcript_preserves_order_role_and_blocks() {
        use protocol::{ContentBlock, MessageId, ToolUseId};

        let tu = ToolUseId::new();
        let history = vec![
            ConversationMessage::User {
                id: MessageId::new(),
                content: vec![ContentBlock::Text {
                    text: "resume me".to_string(),
                }],
                is_meta: false,
                is_compact_summary: false,
                is_visible_in_transcript_only: false,
            },
            ConversationMessage::Assistant {
                id: MessageId::new(),
                content: vec![
                    ContentBlock::Text {
                        text: "on it".to_string(),
                    },
                    ContentBlock::ToolUse {
                        id: tu.clone(),
                        name: "Read".to_string(),
                        input: serde_json::json!({"file_path": "/tmp/x"}),
                        provider_id: None,
                    },
                ],
                stop_reason: Some("tool_use".to_string()),
            },
        ];

        let dtos = lower_transcript(&history);
        // Order preserved oldest-first, one DTO per message.
        assert_eq!(dtos.len(), 2);

        // The user message lowers to role "user" with a single Text block.
        assert_eq!(dtos[0].role, "user");
        assert_eq!(
            dtos[0].blocks,
            vec![MessageBlockDto::Text {
                text: "resume me".to_string()
            }]
        );

        // The assistant message lowers to role "assistant", reusing the SAME
        // ContentBlock -> MessageBlockDto rules as the live MessageComplete path
        // (text + tool_use, id stringified, input lowered to a JSON String).
        assert_eq!(dtos[1].role, "assistant");
        assert_eq!(
            dtos[1].blocks,
            vec![
                MessageBlockDto::Text {
                    text: "on it".to_string()
                },
                MessageBlockDto::ToolUse {
                    id: tu.to_string(),
                    tool: "Read".to_string(),
                    input_json: value_to_json_string(&serde_json::json!({"file_path": "/tmp/x"})),
                    header: Some(crate::tool_display::lower_tool_header(
                        "Read",
                        &serde_json::json!({"file_path": "/tmp/x"}),
                    )),
                },
            ]
        );
    }

    /// REGRESSION: a resumed `ToolResult` used to lower with an EMPTY tool
    /// name and all-`None` diff fields, because `lower_content_block` was
    /// per-block and context-free. That made the iOS client render
    /// `chat_tool_returned %@` as "工具  返回" and left its diff view — which
    /// gates on `old_string`/`new_string`/`file_path` — permanently
    /// unreachable. The call is in assistant message N and the result in user
    /// message N+1, so nothing short of a transcript-wide index can pair them.
    #[test]
    fn lower_transcript_pairs_a_tool_result_with_its_call_across_messages() {
        use protocol::{ContentBlock, MessageId, ToolUseId};
        let tu = ToolUseId::new();
        let input = serde_json::json!({
            "file_path": "/tmp/x.rs",
            "old_string": "fn a() {}\n",
            "new_string": "fn b() {}\n",
        });
        let history = vec![
            ConversationMessage::Assistant {
                id: MessageId::new(),
                content: vec![ContentBlock::ToolUse {
                    id: tu.clone(),
                    name: "Edit".to_string(),
                    input: input.clone(),
                    provider_id: None,
                }],
                stop_reason: Some("tool_use".to_string()),
            },
            ConversationMessage::User {
                id: MessageId::new(),
                content: vec![ContentBlock::ToolResult {
                    tool_use_id: tu.clone(),
                    content: "edited".to_string(),
                    is_error: false,
                    provider_tool_use_id: None,
                    content_blocks: None,
                }],
                is_meta: false,
                is_compact_summary: false,
                is_visible_in_transcript_only: false,
            },
        ];

        let dtos = lower_transcript(&history);
        let MessageBlockDto::ToolResult {
            tool,
            old_string,
            new_string,
            file_path,
            display,
            ..
        } = &dtos[1].blocks[0]
        else {
            panic!("expected a ToolResult block, got {:?}", dtos[1].blocks[0]);
        };
        assert_eq!(tool, "Edit", "the tool name is recovered from the call");
        assert_eq!(old_string.as_deref(), Some("fn a() {}\n"));
        assert_eq!(new_string.as_deref(), Some("fn b() {}\n"));
        assert_eq!(file_path.as_deref(), Some("/tmp/x.rs"));
        let display = display.as_ref().expect("a display block");
        assert_eq!(
            display.headline.as_deref(),
            Some("Added 1 line, removed 1 line")
        );
        let diff = display.diff.as_ref().expect("a structured diff");
        assert_eq!(diff.rows.len(), 2, "one removed row + one added row");
    }

    /// REGRESSION: a resumed transcript persists a tool result's MODEL-FACING
    /// STRING (`ToolCallResult.model_content` — for Bash,
    /// `bash_model_content(stdout, stderr, …)`), never the `{stdout, stderr}`
    /// object the LIVE path passes. The per-tool extractors index the result
    /// as an OBJECT, so every resumed Bash row headlined "(No content)" with
    /// an empty body: the entire command output was gone from scrollback after
    /// a restart. Read lost its content the same way.
    #[test]
    fn a_resumed_bash_or_read_result_keeps_its_output() {
        use protocol::{ContentBlock, MessageId, ToolUseId};

        let call =
            |tu: &ToolUseId, tool: &str, input: serde_json::Value| ConversationMessage::Assistant {
                id: MessageId::new(),
                content: vec![ContentBlock::ToolUse {
                    id: tu.clone(),
                    name: tool.to_string(),
                    input,
                    provider_id: None,
                }],
                stop_reason: Some("tool_use".to_string()),
            };
        let persisted = |tu: &ToolUseId, content: &str| ConversationMessage::User {
            id: MessageId::new(),
            content: vec![ContentBlock::ToolResult {
                tool_use_id: tu.clone(),
                content: content.to_string(),
                is_error: false,
                provider_tool_use_id: None,
                content_blocks: None,
            }],
            is_meta: false,
            is_compact_summary: false,
            is_visible_in_transcript_only: false,
        };
        let display_of = |history: &[ConversationMessage]| {
            let dtos = lower_transcript(history);
            let MessageBlockDto::ToolResult { display, .. } = &dtos[1].blocks[0] else {
                panic!("expected a ToolResult block");
            };
            display.clone().expect("a display block")
        };

        let bash_id = ToolUseId::new();
        let bash = display_of(&[
            call(
                &bash_id,
                "Bash",
                serde_json::json!({"command": "cargo test"}),
            ),
            persisted(&bash_id, "compiling…\nwarning: unused\ndone"),
        ]);
        assert_eq!(bash.headline.as_deref(), Some("compiling…"));
        assert_eq!(
            bash.body.as_deref(),
            Some("compiling…\nwarning: unused\ndone"),
            "the resumed body must be the command's output, not nothing"
        );

        let read_id = ToolUseId::new();
        let read = display_of(&[
            call(
                &read_id,
                "Read",
                serde_json::json!({"file_path": "/tmp/x.rs"}),
            ),
            persisted(&read_id, "     1\tone\n     2\ttwo"),
        ]);
        assert_eq!(read.headline.as_deref(), Some("Read 2 lines"));
        assert_eq!(read.body.as_deref(), Some("     1\tone\n     2\ttwo"));
    }

    /// An orphan result — its call fell outside the resumed window — must
    /// still lower, keeping the historical empty tool name so clients can go
    /// on correlating by `id`.
    #[test]
    fn lower_transcript_tolerates_a_tool_result_with_no_paired_call() {
        use protocol::{ContentBlock, MessageId, ToolUseId};
        let history = vec![ConversationMessage::User {
            id: MessageId::new(),
            content: vec![ContentBlock::ToolResult {
                tool_use_id: ToolUseId::new(),
                content: "orphaned".to_string(),
                is_error: false,
                provider_tool_use_id: None,
                content_blocks: None,
            }],
            is_meta: false,
            is_compact_summary: false,
            is_visible_in_transcript_only: false,
        }];
        let dtos = lower_transcript(&history);
        let MessageBlockDto::ToolResult {
            tool,
            old_string,
            new_string,
            file_path,
            ..
        } = &dtos[0].blocks[0]
        else {
            panic!("expected a ToolResult block");
        };
        assert!(tool.is_empty());
        assert!(old_string.is_none() && new_string.is_none() && file_path.is_none());
    }

    #[test]
    fn lower_transcript_empty_history_is_empty() {
        assert!(lower_transcript(&[]).is_empty());
    }

    #[test]
    fn lower_transcript_pairs_compact_boundary_with_hidden_summary() {
        use protocol::{CompactBoundaryMetadata, CompactTrigger, MessageId};

        let history = vec![
            ConversationMessage::compact_boundary(
                MessageId::new(),
                "Conversation compacted".to_string(),
                CompactBoundaryMetadata {
                    trigger: CompactTrigger::Manual,
                    messages_summarized: Some(6),
                    ..Default::default()
                },
            ),
            ConversationMessage::compact_summary(MessageId::new(), "internal summary".to_string()),
        ];

        let transcript = lower_transcript(&history);
        assert_eq!(transcript.len(), 1);
        assert_eq!(transcript[0].role, "system");
        assert_eq!(
            transcript[0].blocks,
            vec![MessageBlockDto::CompactBoundary {
                messages_before: 6,
                messages_after: 0,
                summary: "internal summary".to_string(),
            }]
        );
    }

    #[test]
    fn lower_conversation_message_system_lowers_to_single_text_block() {
        use protocol::MessageId;
        let msg = ConversationMessage::System {
            id: MessageId::new(),
            content: "you are a helpful assistant".to_string(),
            subtype: None,
            compact_metadata: None,
        };
        let dto = lower_conversation_message(&msg);
        assert_eq!(dto.role, "system");
        assert_eq!(
            dto.blocks,
            vec![MessageBlockDto::Text {
                text: "you are a helpful assistant".to_string()
            }]
        );
    }

    #[test]
    fn lower_conversation_message_projects_image_blocks_to_message_media() {
        use protocol::{ContentBlock, ImageSource, MessageId};
        // An image block has no MessageBlockDto analog, so it is projected to a
        // durable URL-shaped message media entry while text stays in blocks.
        let msg = ConversationMessage::User {
            id: MessageId::new(),
            content: vec![
                ContentBlock::Text {
                    text: "look".to_string(),
                },
                ContentBlock::Image {
                    source: ImageSource::Url {
                        url: "https://example.com/i.png".to_string(),
                    },
                },
            ],
            is_meta: false,
            is_compact_summary: false,
            is_visible_in_transcript_only: false,
        };
        let dto = lower_conversation_message(&msg);
        assert_eq!(
            dto.blocks,
            vec![MessageBlockDto::Text {
                text: "look".to_string()
            }]
        );
        assert_eq!(
            dto.images,
            vec![MessageImageDto {
                media_type: String::new(),
                url: "https://example.com/i.png".to_string(),
            }]
        );
    }
}
