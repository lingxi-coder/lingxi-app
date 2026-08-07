//! F1-06 — `ClientCommand` full-set round-trip + session-lifecycle lock tests.
//!
//! Freezes every inbound command and ENCODES governing decision §0.5:
//! `session_id` is a CONNECTION ATTRIBUTE (carried in `SessionStarted` /
//! `SessionResumed`), NOT a per-live-command param. The ONE allowed occurrence
//! is [`ClientCommand::ResumeSession`], which NAMES a target to resume.
//!
//! Each command gets a serialize → assert-tag → deserialize → assert-eq
//! round-trip so the wire shape is locked before the F1-08 snapshot golden is
//! generated. The conventions inherited from the F1-01 shell (decision §0.1):
//! internally tagged on `type`, `snake_case`; `#[non_exhaustive]`; every
//! optional field `#[serde(default, skip_serializing_if = "Option::is_none")]`.
//!
//! `serde_json` is a DEV-ONLY dep — the contract crate itself never depends on
//! `serde_json::Value` (decision §0.4): image bytes are base64 Strings, tool
//! input lives in JSON Strings elsewhere.

use client_protocol::commands::{
    ClientCommand, CommandResultDto, ImageRefDto, ListingKindDto, PromptModeDto,
    ProviderCredentialSecretDto,
};
use client_protocol::listings::TaskStatusDto;
use client_protocol::local_apps::{
    AppAuthorizationDecisionDto, AppBridgeOperationDto, AppBridgeRequestDto, AppCreateOriginDto,
    AppDesignPatchDto, AppDesignPatchOpDto, DesignValueDto,
};
use client_protocol::permission::PermissionResponseDto;

/// `SendPrompt` — the core inbound command. Carries the text, an optional
/// prompt mode, inline image bytes (decision §0.8), and an optional client
/// turn correlator. Notably it carries NO `session_id` (decision §0.5).
#[test]
fn send_prompt_round_trips() {
    let cmd = ClientCommand::SendPrompt {
        text: "summarize this repo".to_string(),
        prompt_mode: Some(PromptModeDto::Plan),
        images: vec![ImageRefDto {
            media_type: "image/png".to_string(),
            base64: "iVBORw0KGgo=".to_string(),
        }],
        turn_id: Some(7),
    };
    let json = serde_json::to_value(&cmd).expect("serialize SendPrompt");
    assert_eq!(json["type"], "send_prompt");
    assert_eq!(json["text"], "summarize this repo");
    assert_eq!(json["prompt_mode"]["type"], "plan");
    assert_eq!(json["images"][0]["media_type"], "image/png");
    assert_eq!(json["images"][0]["base64"], "iVBORw0KGgo=");
    assert_eq!(json["turn_id"], 7);
    let back: ClientCommand = serde_json::from_value(json).expect("deserialize SendPrompt");
    assert_eq!(back, cmd);
}

/// `SendPrompt` optional fields are skipped when absent, and an `images: []`
/// is preserved (a non-`Option` Vec, always present).
#[test]
fn send_prompt_optional_fields_skip_when_none() {
    let cmd = ClientCommand::SendPrompt {
        text: "hi".to_string(),
        prompt_mode: None,
        images: vec![],
        turn_id: None,
    };
    let json = serde_json::to_value(&cmd).expect("serialize minimal SendPrompt");
    assert!(
        json.get("prompt_mode").is_none(),
        "None prompt_mode must be skipped"
    );
    assert!(
        json.get("turn_id").is_none(),
        "None turn_id must be skipped"
    );
    assert!(
        json["images"].is_array(),
        "images Vec is always present (not optional)"
    );
    // A frame omitting the optionals entirely deserializes with defaults.
    let from_minimal: ClientCommand =
        serde_json::from_str(r#"{"type":"send_prompt","text":"hi","images":[]}"#)
            .expect("deserialize minimal SendPrompt");
    assert_eq!(from_minimal, cmd);
}

/// `PromptModeDto` — every prompt-input mode round-trips with its `snake_case`
/// wire tag (`Normal | Bash | Memory | Plan`).
#[test]
fn prompt_mode_variants_round_trip() {
    let cases = [
        (PromptModeDto::Normal, "normal"),
        (PromptModeDto::Bash, "bash"),
        (PromptModeDto::Memory, "memory"),
        (PromptModeDto::Plan, "plan"),
    ];
    for (mode, tag) in cases {
        let json = serde_json::to_value(mode).expect("serialize PromptModeDto");
        assert_eq!(json["type"], tag, "PromptModeDto::{mode:?} tag mismatch");
        let back: PromptModeDto = serde_json::from_value(json).expect("deserialize PromptModeDto");
        assert_eq!(back, mode);
    }
}

/// `ImageRefDto` — uniform inline `{media_type, base64}` (no transport-leaking
/// enum, decision §0.8).
#[test]
fn image_ref_round_trips() {
    let img = ImageRefDto {
        media_type: "image/jpeg".to_string(),
        base64: "/9j/4AAQ=".to_string(),
    };
    let json = serde_json::to_value(&img).expect("serialize ImageRefDto");
    assert_eq!(json["media_type"], "image/jpeg");
    assert_eq!(json["base64"], "/9j/4AAQ=");
    let back: ImageRefDto = serde_json::from_value(json).expect("deserialize ImageRefDto");
    assert_eq!(back, img);
}

/// `Cancel` — carries an optional turn correlator, no `session_id`.
#[test]
fn cancel_round_trips() {
    for turn_id in [Some(3u64), None] {
        let cmd = ClientCommand::Cancel { turn_id };
        let json = serde_json::to_value(&cmd).expect("serialize Cancel");
        assert_eq!(json["type"], "cancel");
        if turn_id.is_none() {
            assert!(
                json.get("turn_id").is_none(),
                "None turn_id must be skipped"
            );
        }
        let back: ClientCommand = serde_json::from_value(json).expect("deserialize Cancel");
        assert_eq!(back, cmd);
    }
}

/// `ApprovePermission` — correlated by `request_id`, carries the response.
#[test]
fn approve_permission_round_trips() {
    let cmd = ClientCommand::ApprovePermission {
        request_id: 42,
        response: PermissionResponseDto::AllowAlways,
    };
    let json = serde_json::to_value(&cmd).expect("serialize ApprovePermission");
    assert_eq!(json["type"], "approve_permission");
    assert_eq!(json["request_id"], 42);
    assert_eq!(json["response"]["type"], "allow_always");
    let back: ClientCommand = serde_json::from_value(json).expect("deserialize ApprovePermission");
    assert_eq!(back, cmd);
}

/// `DenyPermission` — correlated by `request_id` only.
#[test]
fn deny_permission_round_trips() {
    let cmd = ClientCommand::DenyPermission { request_id: 7 };
    let json = serde_json::to_value(&cmd).expect("serialize DenyPermission");
    assert_eq!(json["type"], "deny_permission");
    assert_eq!(json["request_id"], 7);
    let back: ClientCommand = serde_json::from_value(json).expect("deserialize DenyPermission");
    assert_eq!(back, cmd);
}

/// `SetPermissionMode` — names the live mode to apply.
#[test]
fn set_permission_mode_round_trips() {
    let cmd = ClientCommand::SetPermissionMode {
        mode: "acceptEdits".to_string(),
    };
    let json = serde_json::to_value(&cmd).expect("serialize SetPermissionMode");
    assert_eq!(json["type"], "set_permission_mode");
    assert_eq!(json["mode"], "acceptEdits");
    let back: ClientCommand = serde_json::from_value(json).expect("deserialize SetPermissionMode");
    assert_eq!(back, cmd);
}

#[test]
fn provider_credential_command_round_trips_without_debug_leak() {
    let secret = "sk-provider-super-secret";
    let command = ClientCommand::SetProviderCredential {
        operation_id: 9,
        provider_id: "deepseek".to_string(),
        credential: ProviderCredentialSecretDto::new(secret.to_string()),
    };

    let debug = format!("{command:?}");
    assert!(
        !debug.contains(secret),
        "credential leaked through Debug: {debug}"
    );
    assert!(debug.contains("<redacted>"));

    let json = serde_json::to_value(&command).expect("serialize credential command");
    assert_eq!(json["type"], "set_provider_credential");
    assert_eq!(json["credential"], secret);
    let back: ClientCommand = serde_json::from_value(json).expect("deserialize credential command");
    assert_eq!(back, command);
}

/// `SetModel` — names the model to switch to.
#[test]
fn set_model_round_trips() {
    let cmd = ClientCommand::SetModel {
        model: "claude-opus-4-8".to_string(),
    };
    let json = serde_json::to_value(&cmd).expect("serialize SetModel");
    assert_eq!(json["type"], "set_model");
    assert_eq!(json["model"], "claude-opus-4-8");
    let back: ClientCommand = serde_json::from_value(json).expect("deserialize SetModel");
    assert_eq!(back, cmd);
}

/// `ListModels` — a unit-style pull command.
#[test]
fn list_models_round_trips() {
    let cmd = ClientCommand::ListModels;
    let json = serde_json::to_value(&cmd).expect("serialize ListModels");
    assert_eq!(json["type"], "list_models");
    let back: ClientCommand = serde_json::from_value(json).expect("deserialize ListModels");
    assert_eq!(back, cmd);
}

/// `RunSlashCommand` — LOSSY at the dispatcher; the reply is a
/// [`CommandResultDto`]. The command itself carries only the raw input line.
#[test]
fn run_slash_command_round_trips() {
    let cmd = ClientCommand::RunSlashCommand {
        raw: "/model opus".to_string(),
    };
    let json = serde_json::to_value(&cmd).expect("serialize RunSlashCommand");
    assert_eq!(json["type"], "run_slash_command");
    assert_eq!(json["raw"], "/model opus");
    let back: ClientCommand = serde_json::from_value(json).expect("deserialize RunSlashCommand");
    assert_eq!(back, cmd);
}

/// `CommandResultDto` — the reply for `RunSlashCommand` (display text + an
/// optional injected prompt). Round-trips; the optional `injected` is skipped
/// when absent.
#[test]
fn command_result_round_trips() {
    let result = CommandResultDto {
        display: "Switched model to opus".to_string(),
        injected: None,
    };
    let json = serde_json::to_value(&result).expect("serialize CommandResultDto");
    assert_eq!(json["display"], "Switched model to opus");
    assert!(
        json.get("injected").is_none(),
        "None injected must be skipped"
    );
    let back: CommandResultDto =
        serde_json::from_value(json).expect("deserialize CommandResultDto");
    assert_eq!(back, result);

    let with_injection = CommandResultDto {
        display: "Loaded prompt".to_string(),
        injected: Some("Explain the architecture".to_string()),
    };
    let json2 = serde_json::to_value(&with_injection).expect("serialize CommandResultDto injected");
    assert_eq!(json2["injected"], "Explain the architecture");
    let back2: CommandResultDto =
        serde_json::from_value(json2).expect("deserialize CommandResultDto injected");
    assert_eq!(back2, with_injection);
}

/// `RefreshListings` — pulls a set of screen listings by kind.
#[test]
fn refresh_listings_round_trips() {
    let cmd = ClientCommand::RefreshListings {
        which: vec![
            ListingKindDto::Mcp,
            ListingKindDto::Agents,
            ListingKindDto::Sessions,
        ],
    };
    let json = serde_json::to_value(&cmd).expect("serialize RefreshListings");
    assert_eq!(json["type"], "refresh_listings");
    assert_eq!(json["which"][0]["type"], "mcp");
    assert_eq!(json["which"][1]["type"], "agents");
    assert_eq!(json["which"][2]["type"], "sessions");
    let back: ClientCommand = serde_json::from_value(json).expect("deserialize RefreshListings");
    assert_eq!(back, cmd);
}

/// `ListingKindDto` — every screen kind round-trips with its `snake_case` tag.
#[test]
fn listing_kind_variants_round_trip() {
    let cases = [
        (ListingKindDto::Sessions, "sessions"),
        (ListingKindDto::Models, "models"),
        (ListingKindDto::Mcp, "mcp"),
        (ListingKindDto::Hooks, "hooks"),
        (ListingKindDto::Agents, "agents"),
        (ListingKindDto::SlashCommands, "slash_commands"),
        (ListingKindDto::Memory, "memory"),
        (ListingKindDto::Status, "status"),
        (ListingKindDto::Settings, "settings"),
        (ListingKindDto::Auth, "auth"),
        (ListingKindDto::Doctor, "doctor"),
        (ListingKindDto::Tasks, "tasks"),
        (ListingKindDto::Coordinator, "coordinator"),
    ];
    for (kind, tag) in cases {
        let json = serde_json::to_value(kind).expect("serialize ListingKindDto");
        assert_eq!(json["type"], tag, "ListingKindDto::{kind:?} tag mismatch");
        let back: ListingKindDto =
            serde_json::from_value(json).expect("deserialize ListingKindDto");
        assert_eq!(back, kind);
    }
}

/// `ListingKindDto::Coordinator` — the per-worker roster pull kind (T18). A new
/// variant on the `#[non_exhaustive]` enum is additive (no major bump).
#[test]
fn listing_kind_coordinator_roundtrip() {
    let kind = ListingKindDto::Coordinator;
    let json = serde_json::to_value(kind).expect("serialize ListingKindDto::Coordinator");
    assert_eq!(json["type"], "coordinator");
    let back: ListingKindDto =
        serde_json::from_value(json).expect("deserialize ListingKindDto::Coordinator");
    assert_eq!(back, kind);
}

/// `NewSession` — starts a fresh session. Carries optional `cwd`/`model`, but
/// NO `session_id` (the new id is reported back via `SessionStarted`, §0.5).
#[test]
fn new_session_round_trips() {
    let cmd = ClientCommand::NewSession {
        cwd: Some("/home/dev/proj".to_string()),
        model: Some("claude-sonnet-4-6".to_string()),
    };
    let json = serde_json::to_value(&cmd).expect("serialize NewSession");
    assert_eq!(json["type"], "new_session");
    assert_eq!(json["cwd"], "/home/dev/proj");
    assert_eq!(json["model"], "claude-sonnet-4-6");
    assert!(
        json.get("session_id").is_none(),
        "NewSession must NOT carry session_id (§0.5)"
    );
    let back: ClientCommand = serde_json::from_value(json).expect("deserialize NewSession");
    assert_eq!(back, cmd);

    // Both optionals absent ⇒ skipped.
    let minimal = ClientCommand::NewSession {
        cwd: None,
        model: None,
    };
    let json_min = serde_json::to_value(&minimal).expect("serialize minimal NewSession");
    assert!(json_min.get("cwd").is_none(), "None cwd must be skipped");
    assert!(
        json_min.get("model").is_none(),
        "None model must be skipped"
    );
    let back_min: ClientCommand =
        serde_json::from_value(json_min).expect("deserialize minimal NewSession");
    assert_eq!(back_min, minimal);
}

/// `ResumeSession` — the ONE allowed `session_id`-carrying command (decision
/// §0.5): it NAMES a target to resume. Carries an optional `cwd`.
#[test]
fn resume_session_round_trips_and_carries_session_id() {
    let cmd = ClientCommand::ResumeSession {
        session_id: "9f1c2e3a-...".to_string(),
        cwd: None,
    };
    let json = serde_json::to_value(&cmd).expect("serialize ResumeSession");
    assert_eq!(json["type"], "resume_session");
    assert_eq!(
        json["session_id"], "9f1c2e3a-...",
        "ResumeSession is the ONE command that names a session_id target (§0.5)"
    );
    assert!(json.get("cwd").is_none(), "None cwd must be skipped");
    let back: ClientCommand = serde_json::from_value(json).expect("deserialize ResumeSession");
    assert_eq!(back, cmd);
}

/// `ListSessions` — carries an optional row limit.
#[test]
fn list_sessions_round_trips() {
    for limit in [Some(20u32), None] {
        let cmd = ClientCommand::ListSessions { limit };
        let json = serde_json::to_value(&cmd).expect("serialize ListSessions");
        assert_eq!(json["type"], "list_sessions");
        if limit.is_none() {
            assert!(json.get("limit").is_none(), "None limit must be skipped");
        }
        let back: ClientCommand = serde_json::from_value(json).expect("deserialize ListSessions");
        assert_eq!(back, cmd);
    }
}

/// The unit-style session/control commands all round-trip with their
/// `snake_case` tags and carry no payload (hence no `session_id`).
#[test]
fn unit_control_commands_round_trip() {
    let cases = [
        (ClientCommand::Login, "login"),
        (ClientCommand::Logout, "logout"),
        (ClientCommand::ForceCompact, "force_compact"),
        (ClientCommand::ClearSession, "clear_session"),
        (ClientCommand::RequestExit, "request_exit"),
    ];
    for (cmd, tag) in cases {
        let json = serde_json::to_value(&cmd).expect("serialize unit command");
        assert_eq!(json["type"], tag, "{cmd:?} tag mismatch");
        let back: ClientCommand = serde_json::from_value(json).expect("deserialize unit command");
        assert_eq!(back, cmd);
    }
}

/// `TaskList` — carries an optional status filter (reuses `TaskStatusDto`).
#[test]
fn task_list_round_trips() {
    let cmd = ClientCommand::TaskList {
        status_filter: Some(TaskStatusDto::Running),
    };
    let json = serde_json::to_value(&cmd).expect("serialize TaskList");
    assert_eq!(json["type"], "task_list");
    assert_eq!(json["status_filter"]["type"], "running");
    let back: ClientCommand = serde_json::from_value(json).expect("deserialize TaskList");
    assert_eq!(back, cmd);

    let unfiltered = ClientCommand::TaskList {
        status_filter: None,
    };
    let json_u = serde_json::to_value(&unfiltered).expect("serialize unfiltered TaskList");
    assert!(
        json_u.get("status_filter").is_none(),
        "None status_filter must be skipped"
    );
    let back_u: ClientCommand =
        serde_json::from_value(json_u).expect("deserialize unfiltered TaskList");
    assert_eq!(back_u, unfiltered);
}

/// `TaskOutput` — pulls a task's output spool from a byte/line offset.
#[test]
fn task_output_round_trips() {
    let cmd = ClientCommand::TaskOutput {
        task_id: "b12345678".to_string(),
        offset: 1024,
    };
    let json = serde_json::to_value(&cmd).expect("serialize TaskOutput");
    assert_eq!(json["type"], "task_output");
    assert_eq!(json["task_id"], "b12345678");
    assert_eq!(json["offset"], 1024);
    let back: ClientCommand = serde_json::from_value(json).expect("deserialize TaskOutput");
    assert_eq!(back, cmd);
}

/// `TaskStop` — stops a running task by id.
#[test]
fn task_stop_round_trips() {
    let cmd = ClientCommand::TaskStop {
        task_id: "b12345678".to_string(),
    };
    let json = serde_json::to_value(&cmd).expect("serialize TaskStop");
    assert_eq!(json["type"], "task_stop");
    assert_eq!(json["task_id"], "b12345678");
    let back: ClientCommand = serde_json::from_value(json).expect("deserialize TaskStop");
    assert_eq!(back, cmd);
}

/// `ListApps` — a unit-style pull command; the reply is an `AppsChanged` event.
#[test]
fn list_apps_round_trips() {
    let cmd = ClientCommand::ListApps;
    let json = serde_json::to_value(&cmd).expect("serialize ListApps");
    assert_eq!(json["type"], "list_apps");
    let back: ClientCommand = serde_json::from_value(json).expect("deserialize ListApps");
    assert_eq!(back, cmd);
}

#[test]
fn extended_local_app_commands_round_trip() {
    let commands = vec![
        ClientCommand::GetAppDetails {
            app_id: "habits-1a2b".to_string(),
        },
        ClientCommand::RequestAppDesignSuggestion {
            app_id: "habits-1a2b".to_string(),
            expected_revision: 4,
            prompt: Some("make it calmer".to_string()),
        },
        ClientCommand::DismissAppDesignSuggestion {
            app_id: "habits-1a2b".to_string(),
            suggestion_id: "sugg-1".to_string(),
        },
        ClientCommand::RetryAppGeneration {
            app_id: "habits-1a2b".to_string(),
        },
        ClientCommand::ExecuteAppBridgeRequest {
            request: AppBridgeRequestDto {
                request_id: "bridge-1".to_string(),
                app_id: "habits-1a2b".to_string(),
                operation: AppBridgeOperationDto::QueryData,
                payload_json: Some(r#"{"collection":"items"}"#.to_string()),
            },
        },
        ClientCommand::ResolveAppUiRequest {
            request_id: "ui-1".to_string(),
            decision: AppAuthorizationDecisionDto::AllowSession,
            result_json: Some(r#"{"elements":[]}"#.to_string()),
            error: None,
        },
        ClientCommand::ResolveAppCapabilityRequest {
            request_id: "cap-1".to_string(),
            decision: AppAuthorizationDecisionDto::AllowAlways,
        },
        ClientCommand::ResetAppPermissions {
            app_id: "habits-1a2b".to_string(),
        },
    ];

    let expected_types = [
        "get_app_details",
        "request_app_design_suggestion",
        "dismiss_app_design_suggestion",
        "retry_app_generation",
        "execute_app_bridge_request",
        "resolve_app_ui_request",
        "resolve_app_capability_request",
        "reset_app_permissions",
    ];
    for (command, expected_type) in commands.into_iter().zip(expected_types) {
        let json = serde_json::to_value(&command).expect("serialize extended app command");
        assert_eq!(json["type"], expected_type);
        let back: ClientCommand =
            serde_json::from_value(json).expect("deserialize extended app command");
        assert_eq!(back, command);
    }
}

/// `CreateApp` — name + bare-string origin + an optional `conversation_id`
/// (present for `origin: chat`, skipped when `None`). `template` was removed
/// (local-apps#questionnaire, Task 5, coordinator ruling: total removal of
/// the static template catalog).
#[test]
fn create_app_round_trips() {
    let cmd = ClientCommand::CreateApp {
        name: "Habits".to_string(),
        origin: AppCreateOriginDto::Chat,
        conversation_id: Some("conv-42".to_string()),
    };
    let json = serde_json::to_value(&cmd).expect("serialize CreateApp");
    assert_eq!(json["type"], "create_app");
    assert_eq!(json["name"], "Habits");
    assert_eq!(json["origin"], "chat");
    assert_eq!(json["conversation_id"], "conv-42");
    let back: ClientCommand = serde_json::from_value(json).expect("deserialize CreateApp");
    assert_eq!(back, cmd);

    // A library-born app carries no conversation_id — the None is skipped.
    let from_library = ClientCommand::CreateApp {
        name: "Recipes".to_string(),
        origin: AppCreateOriginDto::Library,
        conversation_id: None,
    };
    let json_l = serde_json::to_value(&from_library).expect("serialize library CreateApp");
    assert_eq!(json_l["origin"], "library");
    assert!(
        json_l.get("conversation_id").is_none(),
        "None conversation_id must be skipped"
    );
    let back_l: ClientCommand =
        serde_json::from_value(json_l).expect("deserialize library CreateApp");
    assert_eq!(back_l, from_library);
}

/// `OpenAppDesigner` — opens the design-spec gate for one app.
#[test]
fn open_app_designer_round_trips() {
    let cmd = ClientCommand::OpenAppDesigner {
        app_id: "habits-1a2b".to_string(),
    };
    let json = serde_json::to_value(&cmd).expect("serialize OpenAppDesigner");
    assert_eq!(json["type"], "open_app_designer");
    assert_eq!(json["app_id"], "habits-1a2b");
    let back: ClientCommand = serde_json::from_value(json).expect("deserialize OpenAppDesigner");
    assert_eq!(back, cmd);
}

/// `UpdateAppDesignDraft` — optimistic-concurrency gated draft edit carrying
/// the spec-§A patch shape (`{op, field_id, value}` ops).
#[test]
fn update_app_design_draft_round_trips() {
    let cmd = ClientCommand::UpdateAppDesignDraft {
        app_id: "habits-1a2b".to_string(),
        expected_revision: 3,
        patch: AppDesignPatchDto {
            ops: vec![
                AppDesignPatchOpDto::Set {
                    field_id: "title".to_string(),
                    value: DesignValueDto::ShortText {
                        value: "Habit Tracker".to_string(),
                    },
                },
                AppDesignPatchOpDto::Remove {
                    field_id: "accent".to_string(),
                },
            ],
            note: Some("rename + drop accent".to_string()),
        },
    };
    let json = serde_json::to_value(&cmd).expect("serialize UpdateAppDesignDraft");
    assert_eq!(json["type"], "update_app_design_draft");
    assert_eq!(json["app_id"], "habits-1a2b");
    assert_eq!(json["expected_revision"], 3);
    assert_eq!(json["patch"]["ops"][0]["op"], "set");
    assert_eq!(json["patch"]["ops"][0]["field_id"], "title");
    assert_eq!(json["patch"]["ops"][0]["value"]["kind"], "short_text");
    assert_eq!(json["patch"]["ops"][1]["op"], "remove");
    assert_eq!(json["patch"]["note"], "rename + drop accent");
    let back: ClientCommand =
        serde_json::from_value(json).expect("deserialize UpdateAppDesignDraft");
    assert_eq!(back, cmd);
}

/// `ApplyAgentDesignSuggestion` — echoes the pending suggestion id under the
/// same revision gating as a draft edit.
#[test]
fn apply_agent_design_suggestion_round_trips() {
    let cmd = ClientCommand::ApplyAgentDesignSuggestion {
        app_id: "habits-1a2b".to_string(),
        suggestion_id: "sugg-77".to_string(),
        expected_revision: 4,
    };
    let json = serde_json::to_value(&cmd).expect("serialize ApplyAgentDesignSuggestion");
    assert_eq!(json["type"], "apply_agent_design_suggestion");
    assert_eq!(json["app_id"], "habits-1a2b");
    assert_eq!(json["suggestion_id"], "sugg-77");
    assert_eq!(json["expected_revision"], 4);
    let back: ClientCommand =
        serde_json::from_value(json).expect("deserialize ApplyAgentDesignSuggestion");
    assert_eq!(back, cmd);
}

/// `ConfirmAppDesign` — echoes the pending designer `interaction_id` (only
/// delivered via `AppDesignerRequested`) plus the CURRENT draft revision.
#[test]
fn confirm_app_design_round_trips() {
    let cmd = ClientCommand::ConfirmAppDesign {
        app_id: "habits-1a2b".to_string(),
        revision: 5,
        interaction_id: "int-designer-9".to_string(),
    };
    let json = serde_json::to_value(&cmd).expect("serialize ConfirmAppDesign");
    assert_eq!(json["type"], "confirm_app_design");
    assert_eq!(json["app_id"], "habits-1a2b");
    assert_eq!(json["revision"], 5);
    assert_eq!(json["interaction_id"], "int-designer-9");
    let back: ClientCommand = serde_json::from_value(json).expect("deserialize ConfirmAppDesign");
    assert_eq!(back, cmd);
}

/// `CancelAppDesign` — voids the pending designer gate.
#[test]
fn cancel_app_design_round_trips() {
    let cmd = ClientCommand::CancelAppDesign {
        app_id: "habits-1a2b".to_string(),
    };
    let json = serde_json::to_value(&cmd).expect("serialize CancelAppDesign");
    assert_eq!(json["type"], "cancel_app_design");
    assert_eq!(json["app_id"], "habits-1a2b");
    let back: ClientCommand = serde_json::from_value(json).expect("deserialize CancelAppDesign");
    assert_eq!(back, cmd);
}

/// The runtime trio (`StartApp` / `StopApp` / `RestartApp`) — phase 1 replies
/// with a typed `not_yet_available` failure, but the wire shape is frozen now.
#[test]
fn app_runtime_commands_round_trip() {
    let cases = [
        (
            ClientCommand::StartApp {
                app_id: "habits-1a2b".to_string(),
            },
            "start_app",
        ),
        (
            ClientCommand::StopApp {
                app_id: "habits-1a2b".to_string(),
            },
            "stop_app",
        ),
        (
            ClientCommand::RestartApp {
                app_id: "habits-1a2b".to_string(),
            },
            "restart_app",
        ),
    ];
    for (cmd, tag) in cases {
        let json = serde_json::to_value(&cmd).expect("serialize runtime command");
        assert_eq!(json["type"], tag, "{cmd:?} tag mismatch");
        assert_eq!(json["app_id"], "habits-1a2b");
        let back: ClientCommand =
            serde_json::from_value(json).expect("deserialize runtime command");
        assert_eq!(back, cmd);
    }
}

/// `ConfirmAppPreview` — echoes the pending preview `interaction_id` (only
/// delivered via `AppPreviewReady`) plus the CURRENT draft revision.
#[test]
fn confirm_app_preview_round_trips() {
    let cmd = ClientCommand::ConfirmAppPreview {
        app_id: "habits-1a2b".to_string(),
        revision: 6,
        interaction_id: "int-preview-3".to_string(),
    };
    let json = serde_json::to_value(&cmd).expect("serialize ConfirmAppPreview");
    assert_eq!(json["type"], "confirm_app_preview");
    assert_eq!(json["app_id"], "habits-1a2b");
    assert_eq!(json["revision"], 6);
    assert_eq!(json["interaction_id"], "int-preview-3");
    let back: ClientCommand = serde_json::from_value(json).expect("deserialize ConfirmAppPreview");
    assert_eq!(back, cmd);
}

/// `RequestAppRevision` — carries the user's revision feedback prompt.
#[test]
fn request_app_revision_round_trips() {
    let cmd = ClientCommand::RequestAppRevision {
        app_id: "habits-1a2b".to_string(),
        prompt: "make the chart blue".to_string(),
    };
    let json = serde_json::to_value(&cmd).expect("serialize RequestAppRevision");
    assert_eq!(json["type"], "request_app_revision");
    assert_eq!(json["app_id"], "habits-1a2b");
    assert_eq!(json["prompt"], "make the chart blue");
    let back: ClientCommand = serde_json::from_value(json).expect("deserialize RequestAppRevision");
    assert_eq!(back, cmd);
}

/// `ListAppCheckpoints` — phase 1 replies with an empty list (git is phase 5).
#[test]
fn list_app_checkpoints_round_trips() {
    let cmd = ClientCommand::ListAppCheckpoints {
        app_id: "habits-1a2b".to_string(),
    };
    let json = serde_json::to_value(&cmd).expect("serialize ListAppCheckpoints");
    assert_eq!(json["type"], "list_app_checkpoints");
    assert_eq!(json["app_id"], "habits-1a2b");
    let back: ClientCommand = serde_json::from_value(json).expect("deserialize ListAppCheckpoints");
    assert_eq!(back, cmd);
}

/// `RestoreAppCheckpoint` — names the checkpoint to restore to.
#[test]
fn restore_app_checkpoint_round_trips() {
    let cmd = ClientCommand::RestoreAppCheckpoint {
        app_id: "habits-1a2b".to_string(),
        checkpoint_id: "ckpt-0001".to_string(),
    };
    let json = serde_json::to_value(&cmd).expect("serialize RestoreAppCheckpoint");
    assert_eq!(json["type"], "restore_app_checkpoint");
    assert_eq!(json["app_id"], "habits-1a2b");
    assert_eq!(json["checkpoint_id"], "ckpt-0001");
    let back: ClientCommand =
        serde_json::from_value(json).expect("deserialize RestoreAppCheckpoint");
    assert_eq!(back, cmd);
}

/// `DeleteApp` — deletes the record + workspace; confirmed via `AppsChanged`.
#[test]
fn delete_app_round_trips() {
    let cmd = ClientCommand::DeleteApp {
        app_id: "habits-1a2b".to_string(),
    };
    let json = serde_json::to_value(&cmd).expect("serialize DeleteApp");
    assert_eq!(json["type"], "delete_app");
    assert_eq!(json["app_id"], "habits-1a2b");
    let back: ClientCommand = serde_json::from_value(json).expect("deserialize DeleteApp");
    assert_eq!(back, cmd);
}

/// THE LIFECYCLE LOCK (decision §0.5). For EVERY `ClientCommand` variant except
/// `ResumeSession`, serialize a canonical instance and assert the wire frame
/// carries NO `session_id` key. `ResumeSession` is the ONE allowed occurrence —
/// it names a target to resume — and is asserted to carry `session_id`
/// separately in `resume_session_round_trips_and_carries_session_id`.
///
/// This is a structural assertion over the live wire shape: any future command
/// that smuggles a `session_id` field (other than `ResumeSession`) breaks this
/// test, enforcing that `session_id` stays a CONNECTION ATTRIBUTE.
#[test]
#[allow(clippy::too_many_lines)]
fn no_live_command_carries_session_id() {
    // One canonical instance of every command EXCEPT ResumeSession.
    let commands: Vec<ClientCommand> = vec![
        ClientCommand::SendPrompt {
            text: "x".to_string(),
            prompt_mode: Some(PromptModeDto::Normal),
            images: vec![ImageRefDto {
                media_type: "image/png".to_string(),
                base64: "AA==".to_string(),
            }],
            turn_id: Some(1),
        },
        ClientCommand::Cancel { turn_id: Some(1) },
        ClientCommand::ApprovePermission {
            request_id: 1,
            response: PermissionResponseDto::AllowOnce,
        },
        ClientCommand::DenyPermission { request_id: 1 },
        ClientCommand::SetModel {
            model: "m".to_string(),
        },
        ClientCommand::ListModels,
        ClientCommand::RunSlashCommand {
            raw: "/x".to_string(),
        },
        ClientCommand::RefreshListings {
            which: vec![ListingKindDto::Status],
        },
        ClientCommand::NewSession {
            cwd: Some("/p".to_string()),
            model: Some("m".to_string()),
        },
        ClientCommand::ListSessions { limit: Some(5) },
        ClientCommand::Login,
        ClientCommand::Logout,
        ClientCommand::ForceCompact,
        ClientCommand::ClearSession,
        ClientCommand::TaskList {
            status_filter: Some(TaskStatusDto::Pending),
        },
        ClientCommand::TaskOutput {
            task_id: "b00000000".to_string(),
            offset: 0,
        },
        ClientCommand::TaskStop {
            task_id: "b00000000".to_string(),
        },
        ClientCommand::ListApps,
        ClientCommand::CreateApp {
            name: "Habits".to_string(),
            origin: AppCreateOriginDto::Chat,
            conversation_id: Some("conv-42".to_string()),
        },
        ClientCommand::OpenAppDesigner {
            app_id: "habits-1a2b".to_string(),
        },
        ClientCommand::UpdateAppDesignDraft {
            app_id: "habits-1a2b".to_string(),
            expected_revision: 1,
            patch: AppDesignPatchDto {
                ops: vec![AppDesignPatchOpDto::Set {
                    field_id: "title".to_string(),
                    value: DesignValueDto::ShortText {
                        value: "T".to_string(),
                    },
                }],
                note: None,
            },
        },
        ClientCommand::ApplyAgentDesignSuggestion {
            app_id: "habits-1a2b".to_string(),
            suggestion_id: "sugg-1".to_string(),
            expected_revision: 1,
        },
        ClientCommand::ConfirmAppDesign {
            app_id: "habits-1a2b".to_string(),
            revision: 1,
            interaction_id: "int-1".to_string(),
        },
        ClientCommand::CancelAppDesign {
            app_id: "habits-1a2b".to_string(),
        },
        ClientCommand::StartApp {
            app_id: "habits-1a2b".to_string(),
        },
        ClientCommand::StopApp {
            app_id: "habits-1a2b".to_string(),
        },
        ClientCommand::RestartApp {
            app_id: "habits-1a2b".to_string(),
        },
        ClientCommand::ConfirmAppPreview {
            app_id: "habits-1a2b".to_string(),
            revision: 1,
            interaction_id: "int-2".to_string(),
        },
        ClientCommand::RequestAppRevision {
            app_id: "habits-1a2b".to_string(),
            prompt: "p".to_string(),
        },
        ClientCommand::ListAppCheckpoints {
            app_id: "habits-1a2b".to_string(),
        },
        ClientCommand::RestoreAppCheckpoint {
            app_id: "habits-1a2b".to_string(),
            checkpoint_id: "ckpt-1".to_string(),
        },
        ClientCommand::DeleteApp {
            app_id: "habits-1a2b".to_string(),
        },
        ClientCommand::RequestExit,
    ];

    for cmd in &commands {
        let json = serde_json::to_value(cmd).expect("serialize command");
        let obj = json
            .as_object()
            .expect("command serializes to a JSON object");
        assert!(
            !obj.contains_key("session_id"),
            "live command {cmd:?} must NOT carry session_id (decision §0.5) — \
             session_id is a connection attribute; only ResumeSession names a target"
        );
        // Sanity: it actually round-trips too.
        let back: ClientCommand = serde_json::from_value(json).expect("deserialize command");
        assert_eq!(&back, cmd);
    }

    // The ONE allowed occurrence DOES carry session_id (proves the test isn't
    // vacuously passing because no command ever has the key).
    let resume = ClientCommand::ResumeSession {
        session_id: "target-id".to_string(),
        cwd: None,
    };
    let resume_json = serde_json::to_value(&resume).expect("serialize ResumeSession");
    assert!(
        resume_json
            .as_object()
            .expect("object")
            .contains_key("session_id"),
        "ResumeSession is the ONE command that names a session_id target (§0.5)"
    );
}
