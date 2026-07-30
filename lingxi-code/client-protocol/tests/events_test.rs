//! F1-03 — live-turn event DTO round-trip tests.
//!
//! Freezes the per-turn streaming events the `client-adapter` emits (plan
//! F1-03). Each variant gets a serialize → assert-tag → deserialize → assert-eq
//! round-trip so the wire shape is locked before the F1-08 snapshot golden is
//! generated. The engine sources are noted per variant in `events.rs`.
//!
//! `serde_json` is a DEV-ONLY dep — the contract crate itself never depends on
//! `serde_json::Value` (governing decision §0.4): every tool payload here is a
//! JSON **String** (`input_json`/`result_json`).

use client_protocol::events::{ClientEvent, CostDto, TurnOutcomeDto};
use client_protocol::local_apps::{
    AppCheckpointDto, AppCheckpointKindDto, AppDesignPatchDto, AppDesignPatchOpDto,
    AppErrorCodeDto, AppRecordDto, AppRuntimeStateDto, AppTemplateKindDto, AppWorkflowStateDto,
    DesignValueDto,
};
use client_protocol::message::{MessageBlockDto, MessageDto};
use std::collections::HashMap;

#[test]
fn ask_user_question_resolved_round_trips() {
    let ev = ClientEvent::AskUserQuestionResolved { request_id: 9 };
    let json = serde_json::to_value(&ev).expect("serialize AskUserQuestionResolved");
    assert_eq!(json["type"], "ask_user_question_resolved");
    assert_eq!(json["request_id"], 9);
    let back: ClientEvent =
        serde_json::from_value(json).expect("deserialize AskUserQuestionResolved");
    assert_eq!(back, ev);
}

/// `TextDelta` — 1:1 `OutputStream::emit_text`. Carries plain assistant text.
#[test]
fn text_delta_round_trips() {
    let ev = ClientEvent::TextDelta {
        text: "hello world".to_string(),
    };
    let json = serde_json::to_value(&ev).expect("serialize TextDelta");
    assert_eq!(json["type"], "text_delta");
    assert_eq!(json["text"], "hello world");
    let back: ClientEvent = serde_json::from_value(json).expect("deserialize TextDelta");
    assert_eq!(back, ev);
}

/// `ToolUseStarted` — 1:1 `emit_tool_call`; the `serde_json::Value` input is
/// lowered to a JSON **String** (`input_json`) per §0.4.
#[test]
fn tool_use_started_round_trips() {
    let ev = ClientEvent::ToolUseStarted {
        id: "tu_01".to_string(),
        tool: "Read".to_string(),
        input_json: r#"{"file_path":"/tmp/x"}"#.to_string(),
    };
    let json = serde_json::to_value(&ev).expect("serialize ToolUseStarted");
    assert_eq!(json["type"], "tool_use_started");
    assert_eq!(json["id"], "tu_01");
    assert_eq!(json["tool"], "Read");
    // The payload is a JSON String on the wire, NOT a nested object.
    assert!(
        json["input_json"].is_string(),
        "input_json must be a String"
    );
    let back: ClientEvent = serde_json::from_value(json).expect("deserialize ToolUseStarted");
    assert_eq!(back, ev);
}

/// `ToolUseResult` — 1:1 `emit_tool_result`; fires in COMPLETION order (clients
/// key by id). `result_json` is a JSON String; `is_error` flags tool failure.
#[test]
fn tool_use_result_round_trips() {
    let ev = ClientEvent::ToolUseResult {
        id: "tu_01".to_string(),
        tool: "Read".to_string(),
        result_json: r#"{"content":"ok"}"#.to_string(),
        is_error: false,
    };
    let json = serde_json::to_value(&ev).expect("serialize ToolUseResult");
    assert_eq!(json["type"], "tool_use_result");
    assert_eq!(json["id"], "tu_01");
    assert_eq!(json["tool"], "Read");
    assert!(
        json["result_json"].is_string(),
        "result_json must be a String"
    );
    assert_eq!(json["is_error"], false);
    let back: ClientEvent = serde_json::from_value(json).expect("deserialize ToolUseResult");
    assert_eq!(back, ev);
}

/// `MessageComplete` — synthesized (no engine message-boundary event). Carries
/// an optional [`MessageDto`] reproducing the assistant message block set.
#[test]
fn message_complete_round_trips() {
    let ev = ClientEvent::MessageComplete {
        stop_reason: Some("end_turn".to_string()),
        message: Some(MessageDto {
            role: "assistant".to_string(),
            blocks: vec![
                MessageBlockDto::Text {
                    text: "done".to_string(),
                },
                MessageBlockDto::ToolUse {
                    id: "tu_01".to_string(),
                    tool: "Read".to_string(),
                    input_json: "{}".to_string(),
                },
            ],
        }),
    };
    let json = serde_json::to_value(&ev).expect("serialize MessageComplete");
    assert_eq!(json["type"], "message_complete");
    assert_eq!(json["stop_reason"], "end_turn");
    assert_eq!(json["message"]["role"], "assistant");
    let back: ClientEvent = serde_json::from_value(json).expect("deserialize MessageComplete");
    assert_eq!(back, ev);
}

/// `MessageComplete` with both optional fields absent skips them from the wire
/// (the `skip_serializing_if = "Option::is_none"` forward-compat convention).
#[test]
fn message_complete_omits_none_fields() {
    let ev = ClientEvent::MessageComplete {
        stop_reason: None,
        message: None,
    };
    let json = serde_json::to_value(&ev).expect("serialize MessageComplete");
    assert_eq!(json["type"], "message_complete");
    assert!(
        json.get("stop_reason").is_none(),
        "None stop_reason must be skipped"
    );
    assert!(
        json.get("message").is_none(),
        "None message must be skipped"
    );
    let back: ClientEvent = serde_json::from_value(json).expect("deserialize MessageComplete");
    assert_eq!(back, ev);
}

/// `TurnStarted` — adapter-synthesized on `SendPrompt` receipt (no engine
/// source). `turn_id` is optional.
#[test]
fn turn_started_round_trips() {
    let ev = ClientEvent::TurnStarted { turn_id: Some(7) };
    let json = serde_json::to_value(&ev).expect("serialize TurnStarted");
    assert_eq!(json["type"], "turn_started");
    assert_eq!(json["turn_id"], 7);
    let back: ClientEvent = serde_json::from_value(json).expect("deserialize TurnStarted");
    assert_eq!(back, ev);

    // None turn_id is skipped.
    let ev_none = ClientEvent::TurnStarted { turn_id: None };
    let json_none = serde_json::to_value(&ev_none).expect("serialize TurnStarted none");
    assert!(json_none.get("turn_id").is_none());
    let back_none: ClientEvent =
        serde_json::from_value(json_none).expect("deserialize TurnStarted none");
    assert_eq!(back_none, ev_none);
}

/// `TurnEnded` — 1:1 `emit_end_turn`. Carries the outcome, stop reason, and the
/// lowered [`CostDto`].
#[test]
fn turn_ended_round_trips() {
    let ev = ClientEvent::TurnEnded {
        outcome: TurnOutcomeDto::EndTurn,
        stop_reason: Some("end_turn".to_string()),
        cost: CostDto {
            total_usd: 0.0123,
            input_tokens: 100,
            output_tokens: 200,
            api_calls: 3,
            session_duration_secs: 42,
            formatted: "$0.0123".to_string(),
        },
    };
    let json = serde_json::to_value(&ev).expect("serialize TurnEnded");
    assert_eq!(json["type"], "turn_ended");
    assert_eq!(json["outcome"]["type"], "end_turn");
    assert_eq!(json["stop_reason"], "end_turn");
    assert_eq!(json["cost"]["input_tokens"], 100);
    assert_eq!(json["cost"]["session_duration_secs"], 42);
    let back: ClientEvent = serde_json::from_value(json).expect("deserialize TurnEnded");
    assert_eq!(back, ev);
}

/// `CostUpdate` — the cumulative cost snapshot lowered (`Duration` → secs).
#[test]
fn cost_update_round_trips() {
    let ev = ClientEvent::CostUpdate {
        total_usd: 1.5,
        input_tokens: 10,
        output_tokens: 20,
        api_calls: 2,
        session_duration_secs: 99,
        formatted: "$1.5000".to_string(),
    };
    let json = serde_json::to_value(&ev).expect("serialize CostUpdate");
    assert_eq!(json["type"], "cost_update");
    assert_eq!(json["total_usd"], 1.5);
    assert_eq!(json["api_calls"], 2);
    assert_eq!(json["session_duration_secs"], 99);
    assert_eq!(json["formatted"], "$1.5000");
    let back: ClientEvent = serde_json::from_value(json).expect("deserialize CostUpdate");
    assert_eq!(back, ev);
}

/// `CompactionCompleted` — 1:1 `emit_compaction_completed`.
#[test]
fn compaction_completed_round_trips() {
    let ev = ClientEvent::CompactionCompleted {
        messages_before: 50,
        messages_after: 12,
        bytes_saved: 4096,
    };
    let json = serde_json::to_value(&ev).expect("serialize CompactionCompleted");
    assert_eq!(json["type"], "compaction_completed");
    assert_eq!(json["messages_before"], 50);
    assert_eq!(json["messages_after"], 12);
    assert_eq!(json["bytes_saved"], 4096);
    let back: ClientEvent = serde_json::from_value(json).expect("deserialize CompactionCompleted");
    assert_eq!(back, ev);
}

/// `ThinkingDelta` — now LIVE-FED (§0.7 follow-up): `event_router` emits it per
/// `ContentDelta::ThinkingDelta` chunk. The wire shape is unchanged, so the
/// frozen round-trip still holds (the live stream carries `signature: None`).
#[test]
fn thinking_delta_round_trips() {
    let ev = ClientEvent::ThinkingDelta {
        thinking: "let me think".to_string(),
        signature: Some("sig".to_string()),
    };
    let json = serde_json::to_value(&ev).expect("serialize ThinkingDelta");
    assert_eq!(json["type"], "thinking_delta");
    assert_eq!(json["thinking"], "let me think");
    assert_eq!(json["signature"], "sig");
    let back: ClientEvent = serde_json::from_value(json).expect("deserialize ThinkingDelta");
    assert_eq!(back, ev);

    // signature is optional and skipped when None.
    let ev_no_sig = ClientEvent::ThinkingDelta {
        thinking: "x".to_string(),
        signature: None,
    };
    let json_no_sig = serde_json::to_value(&ev_no_sig).expect("serialize ThinkingDelta no-sig");
    assert!(json_no_sig.get("signature").is_none());
    let back_no_sig: ClientEvent =
        serde_json::from_value(json_no_sig).expect("deserialize ThinkingDelta no-sig");
    assert_eq!(back_no_sig, ev_no_sig);
}

/// `UsageUpdate` — now LIVE-FED (§0.7 follow-up): `event_router` emits it from
/// the `MessageStart` / `MessageDelta` usage fields. The wire shape is
/// unchanged, so the frozen round-trip still holds.
#[test]
fn usage_update_round_trips() {
    let ev = ClientEvent::UsageUpdate {
        input_tokens: 11,
        output_tokens: 22,
        cache_read_tokens: 3,
        cache_creation_tokens: 4,
    };
    let json = serde_json::to_value(&ev).expect("serialize UsageUpdate");
    assert_eq!(json["type"], "usage_update");
    assert_eq!(json["input_tokens"], 11);
    assert_eq!(json["output_tokens"], 22);
    assert_eq!(json["cache_read_tokens"], 3);
    assert_eq!(json["cache_creation_tokens"], 4);
    let back: ClientEvent = serde_json::from_value(json).expect("deserialize UsageUpdate");
    assert_eq!(back, ev);
}

/// `AppsChanged` — carries the full local-app record set (bare-string
/// template/state enums; an absent `conversation_id` skipped).
#[test]
fn apps_changed_round_trips() {
    let ev = ClientEvent::AppsChanged {
        apps: vec![AppRecordDto {
            id: "habits-1a2b".to_string(),
            name: "Habits".to_string(),
            template: AppTemplateKindDto::Dashboard,
            created_at_ms: 1_750_000_000_000,
            updated_at_ms: 1_750_000_000_001,
            workflow_state: AppWorkflowStateDto::CollectingSpec,
            conversation_id: None,
            workspace_rel: "apps/habits-1a2b/workspace".to_string(),
        }],
    };
    let json = serde_json::to_value(&ev).expect("serialize AppsChanged");
    assert_eq!(json["type"], "apps_changed");
    assert_eq!(json["apps"][0]["id"], "habits-1a2b");
    assert_eq!(json["apps"][0]["template"], "dashboard");
    assert_eq!(json["apps"][0]["workflow_state"], "collecting_spec");
    assert!(
        json["apps"][0].get("conversation_id").is_none(),
        "None conversation_id must be skipped"
    );
    let back: ClientEvent = serde_json::from_value(json).expect("deserialize AppsChanged");
    assert_eq!(back, ev);
}

/// `AppDesignerRequested` — delivers the pending designer `interaction_id`
/// (the ONLY path the id reaches the client, gating `ConfirmAppDesign`).
#[test]
fn app_designer_requested_round_trips() {
    let ev = ClientEvent::AppDesignerRequested {
        app_id: "habits-1a2b".to_string(),
        interaction_id: "int-designer-9".to_string(),
        revision: 5,
    };
    let json = serde_json::to_value(&ev).expect("serialize AppDesignerRequested");
    assert_eq!(json["type"], "app_designer_requested");
    assert_eq!(json["app_id"], "habits-1a2b");
    assert_eq!(json["interaction_id"], "int-designer-9");
    assert_eq!(json["revision"], 5);
    let back: ClientEvent = serde_json::from_value(json).expect("deserialize AppDesignerRequested");
    assert_eq!(back, ev);
}

/// `AppDesignDraftChanged` — carries the new revision + the full field map
/// ({field_id → {kind, value}}).
#[test]
fn app_design_draft_changed_round_trips() {
    let mut fields = HashMap::new();
    fields.insert(
        "title".to_string(),
        DesignValueDto::ShortText {
            value: "Habit Tracker".to_string(),
        },
    );
    fields.insert(
        "screens".to_string(),
        DesignValueDto::ScreenList {
            value: vec!["home".to_string(), "stats".to_string()],
        },
    );
    let ev = ClientEvent::AppDesignDraftChanged {
        app_id: "habits-1a2b".to_string(),
        revision: 6,
        fields,
    };
    let json = serde_json::to_value(&ev).expect("serialize AppDesignDraftChanged");
    assert_eq!(json["type"], "app_design_draft_changed");
    assert_eq!(json["app_id"], "habits-1a2b");
    assert_eq!(json["revision"], 6);
    assert_eq!(json["fields"]["title"]["kind"], "short_text");
    assert_eq!(json["fields"]["title"]["value"], "Habit Tracker");
    assert_eq!(json["fields"]["screens"]["kind"], "screen_list");
    assert_eq!(json["fields"]["screens"]["value"][1], "stats");
    let back: ClientEvent =
        serde_json::from_value(json).expect("deserialize AppDesignDraftChanged");
    assert_eq!(back, ev);
}

/// `AppDesignSuggestionAvailable` — the agent's pending patch, applied only
/// via an explicit `ApplyAgentDesignSuggestion` echo of `suggestion_id`.
#[test]
fn app_design_suggestion_available_round_trips() {
    let ev = ClientEvent::AppDesignSuggestionAvailable {
        app_id: "habits-1a2b".to_string(),
        suggestion_id: "sugg-77".to_string(),
        based_on_revision: 6,
        patch: AppDesignPatchDto {
            ops: vec![AppDesignPatchOpDto::Set {
                field_id: "accent".to_string(),
                value: DesignValueDto::Color {
                    value: "#3366ff".to_string(),
                },
            }],
            note: Some("bolder accent".to_string()),
        },
    };
    let json = serde_json::to_value(&ev).expect("serialize AppDesignSuggestionAvailable");
    assert_eq!(json["type"], "app_design_suggestion_available");
    assert_eq!(json["suggestion_id"], "sugg-77");
    assert_eq!(json["based_on_revision"], 6);
    assert_eq!(json["patch"]["ops"][0]["op"], "set");
    assert_eq!(json["patch"]["ops"][0]["value"]["kind"], "color");
    let back: ClientEvent =
        serde_json::from_value(json).expect("deserialize AppDesignSuggestionAvailable");
    assert_eq!(back, ev);
}

/// `AppDesignConflict` — a stale `expected_revision` was rejected; both
/// revisions surface so the client can re-pull and re-apply.
#[test]
fn app_design_conflict_round_trips() {
    let ev = ClientEvent::AppDesignConflict {
        app_id: "habits-1a2b".to_string(),
        expected_revision: 4,
        actual_revision: 6,
    };
    let json = serde_json::to_value(&ev).expect("serialize AppDesignConflict");
    assert_eq!(json["type"], "app_design_conflict");
    assert_eq!(json["expected_revision"], 4);
    assert_eq!(json["actual_revision"], 6);
    let back: ClientEvent = serde_json::from_value(json).expect("deserialize AppDesignConflict");
    assert_eq!(back, ev);
}

/// `AppWorkflowChanged` — bare-string state + an optional detail (skipped when
/// `None`).
#[test]
fn app_workflow_changed_round_trips() {
    let ev = ClientEvent::AppWorkflowChanged {
        app_id: "habits-1a2b".to_string(),
        state: AppWorkflowStateDto::GenerationFailed,
        detail: Some("npm install failed".to_string()),
    };
    let json = serde_json::to_value(&ev).expect("serialize AppWorkflowChanged");
    assert_eq!(json["type"], "app_workflow_changed");
    assert_eq!(json["state"], "generation_failed");
    assert_eq!(json["detail"], "npm install failed");
    let back: ClientEvent = serde_json::from_value(json).expect("deserialize AppWorkflowChanged");
    assert_eq!(back, ev);

    let ev_min = ClientEvent::AppWorkflowChanged {
        app_id: "habits-1a2b".to_string(),
        state: AppWorkflowStateDto::Generating,
        detail: None,
    };
    let json_min = serde_json::to_value(&ev_min).expect("serialize minimal AppWorkflowChanged");
    assert!(
        json_min.get("detail").is_none(),
        "None detail must be skipped"
    );
    let back_min: ClientEvent =
        serde_json::from_value(json_min).expect("deserialize minimal AppWorkflowChanged");
    assert_eq!(back_min, ev_min);
}

/// `AppGenerationProgress` — stage + optional percent/detail (skipped when
/// `None`).
#[test]
fn app_generation_progress_round_trips() {
    let ev = ClientEvent::AppGenerationProgress {
        app_id: "habits-1a2b".to_string(),
        stage: "scaffold".to_string(),
        percent: Some(40),
        detail: Some("writing pages".to_string()),
    };
    let json = serde_json::to_value(&ev).expect("serialize AppGenerationProgress");
    assert_eq!(json["type"], "app_generation_progress");
    assert_eq!(json["stage"], "scaffold");
    assert_eq!(json["percent"], 40);
    assert_eq!(json["detail"], "writing pages");
    let back: ClientEvent =
        serde_json::from_value(json).expect("deserialize AppGenerationProgress");
    assert_eq!(back, ev);

    let ev_min = ClientEvent::AppGenerationProgress {
        app_id: "habits-1a2b".to_string(),
        stage: "validate".to_string(),
        percent: None,
        detail: None,
    };
    let json_min = serde_json::to_value(&ev_min).expect("serialize minimal AppGenerationProgress");
    assert!(
        json_min.get("percent").is_none(),
        "None percent must be skipped"
    );
    assert!(
        json_min.get("detail").is_none(),
        "None detail must be skipped"
    );
    let back_min: ClientEvent =
        serde_json::from_value(json_min).expect("deserialize minimal AppGenerationProgress");
    assert_eq!(back_min, ev_min);
}

/// `AppRuntimeChanged` — bare-string runtime state + optional last_error.
#[test]
fn app_runtime_changed_round_trips() {
    let ev = ClientEvent::AppRuntimeChanged {
        app_id: "habits-1a2b".to_string(),
        state: AppRuntimeStateDto::Failed,
        last_error: Some("port already in use".to_string()),
    };
    let json = serde_json::to_value(&ev).expect("serialize AppRuntimeChanged");
    assert_eq!(json["type"], "app_runtime_changed");
    assert_eq!(json["state"], "failed");
    assert_eq!(json["last_error"], "port already in use");
    let back: ClientEvent = serde_json::from_value(json).expect("deserialize AppRuntimeChanged");
    assert_eq!(back, ev);

    let ev_min = ClientEvent::AppRuntimeChanged {
        app_id: "habits-1a2b".to_string(),
        state: AppRuntimeStateDto::Stopped,
        last_error: None,
    };
    let json_min = serde_json::to_value(&ev_min).expect("serialize minimal AppRuntimeChanged");
    assert!(
        json_min.get("last_error").is_none(),
        "None last_error must be skipped"
    );
    let back_min: ClientEvent =
        serde_json::from_value(json_min).expect("deserialize minimal AppRuntimeChanged");
    assert_eq!(back_min, ev_min);
}

/// `AppPreviewReady` — delivers the pending preview `interaction_id`; `url`
/// stays `None` (skipped) until the phase-4 runtime serves the app.
#[test]
fn app_preview_ready_round_trips() {
    let ev = ClientEvent::AppPreviewReady {
        app_id: "habits-1a2b".to_string(),
        interaction_id: "int-preview-3".to_string(),
        revision: 6,
        url: None,
    };
    let json = serde_json::to_value(&ev).expect("serialize AppPreviewReady");
    assert_eq!(json["type"], "app_preview_ready");
    assert_eq!(json["interaction_id"], "int-preview-3");
    assert_eq!(json["revision"], 6);
    assert!(json.get("url").is_none(), "None url must be skipped");
    let back: ClientEvent = serde_json::from_value(json).expect("deserialize AppPreviewReady");
    assert_eq!(back, ev);

    let ev_url = ClientEvent::AppPreviewReady {
        app_id: "habits-1a2b".to_string(),
        interaction_id: "int-preview-4".to_string(),
        revision: 7,
        url: Some("http://localhost:31337".to_string()),
    };
    let json_url = serde_json::to_value(&ev_url).expect("serialize served AppPreviewReady");
    assert_eq!(json_url["url"], "http://localhost:31337");
    let back_url: ClientEvent =
        serde_json::from_value(json_url).expect("deserialize served AppPreviewReady");
    assert_eq!(back_url, ev_url);
}

/// `AppCheckpointCreated` — carries the recorded checkpoint row.
#[test]
fn app_checkpoint_created_round_trips() {
    let ev = ClientEvent::AppCheckpointCreated {
        app_id: "habits-1a2b".to_string(),
        checkpoint: AppCheckpointDto {
            id: "ckpt-0001".to_string(),
            label: "Preview approved".to_string(),
            kind: AppCheckpointKindDto::PreviewApproved,
            created_at_ms: 1_750_000_000_000,
        },
    };
    let json = serde_json::to_value(&ev).expect("serialize AppCheckpointCreated");
    assert_eq!(json["type"], "app_checkpoint_created");
    assert_eq!(json["checkpoint"]["id"], "ckpt-0001");
    assert_eq!(json["checkpoint"]["kind"], "preview_approved");
    let back: ClientEvent =
        serde_json::from_value(json).expect("deserialize AppCheckpointCreated");
    assert_eq!(back, ev);
}

/// `AppOperationFailed` — bare-string typed code; `app_id` is optional
/// (skipped when the failure addressed no specific app).
#[test]
fn app_operation_failed_round_trips() {
    let ev = ClientEvent::AppOperationFailed {
        app_id: Some("habits-1a2b".to_string()),
        code: AppErrorCodeDto::NotYetAvailable,
        message: "app runtime lands in phase 4".to_string(),
    };
    let json = serde_json::to_value(&ev).expect("serialize AppOperationFailed");
    assert_eq!(json["type"], "app_operation_failed");
    assert_eq!(json["app_id"], "habits-1a2b");
    assert_eq!(json["code"], "not_yet_available");
    assert_eq!(json["message"], "app runtime lands in phase 4");
    let back: ClientEvent = serde_json::from_value(json).expect("deserialize AppOperationFailed");
    assert_eq!(back, ev);

    let ev_global = ClientEvent::AppOperationFailed {
        app_id: None,
        code: AppErrorCodeDto::InvalidRequest,
        message: "bad app id".to_string(),
    };
    let json_g = serde_json::to_value(&ev_global).expect("serialize global AppOperationFailed");
    assert!(
        json_g.get("app_id").is_none(),
        "None app_id must be skipped"
    );
    let back_g: ClientEvent =
        serde_json::from_value(json_g).expect("deserialize global AppOperationFailed");
    assert_eq!(back_g, ev_global);
}

/// Enumerate every `TurnOutcomeDto` variant and assert the `snake_case` wire
/// tags plus a byte-stable round-trip (`EndTurn | MaxTurns | Cancelled`).
#[test]
fn end_turn_outcome_variants() {
    let cases = [
        (TurnOutcomeDto::EndTurn, "end_turn"),
        (TurnOutcomeDto::MaxTurns, "max_turns"),
        (TurnOutcomeDto::Cancelled, "cancelled"),
    ];
    for (outcome, tag) in cases {
        let json = serde_json::to_value(&outcome).expect("serialize TurnOutcomeDto");
        assert_eq!(
            json["type"], tag,
            "TurnOutcomeDto::{outcome:?} tag mismatch"
        );
        let back: TurnOutcomeDto =
            serde_json::from_value(json).expect("deserialize TurnOutcomeDto");
        assert_eq!(back, outcome);
    }
}
