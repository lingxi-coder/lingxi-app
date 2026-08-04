//! Local-apps DTO round-trip + exact-wire-shape tests.
//!
//! Mirrors `tests/computer_access_test.rs`: each type gets a serialize →
//! assert-shape → deserialize → assert-eq round-trip. The fieldless enums ride
//! as bare wire STRINGS (the `AccessTierDto` precedent — byte-identical to the
//! `local-apps` core enums' canonical `as_str()` values), and the two
//! payload-carrying enums use the spec-§A discriminators (`kind` / `op`), so
//! several tests assert BYTE-IDENTICAL serialized JSON, not just structural
//! equivalence.

use client_protocol::local_apps::{
    builtin_app_templates, AppCheckpointDto, AppCheckpointKindDto, AppCreateOriginDto,
    AppDataFieldTypeDto, AppDesignFieldTypeDto, AppDesignPatchDto, AppDesignPatchOpDto,
    AppErrorCodeDto, AppRecordDto, AppRuntimeStateDto, AppTemplateKindDto, AppWorkflowStateDto,
    DensityLevelDto, DesignValueDto,
};

/// `AppTemplateKindDto` is a bare wire STRING, byte-identical to the core
/// `AppTemplateKind::as_str()` values.
#[test]
fn template_kind_serializes_as_bare_string() {
    let cases = [
        (AppTemplateKindDto::Dashboard, "\"dashboard\""),
        (AppTemplateKindDto::CrudTracker, "\"crud_tracker\""),
        (AppTemplateKindDto::ContentShowcase, "\"content_showcase\""),
        (AppTemplateKindDto::FormUtility, "\"form_utility\""),
    ];
    for (kind, expected) in cases {
        let json = serde_json::to_string(&kind).expect("serialize AppTemplateKindDto");
        assert_eq!(json, expected, "AppTemplateKindDto::{kind:?} wire form");
        let back: AppTemplateKindDto =
            serde_json::from_str(&json).expect("deserialize AppTemplateKindDto");
        assert_eq!(back, kind);
    }
}

/// `AppWorkflowStateDto` — every spec-§B state round-trips as its bare
/// `snake_case` string.
#[test]
fn workflow_state_serializes_as_bare_string() {
    let cases = [
        (AppWorkflowStateDto::CollectingSpec, "\"collecting_spec\""),
        (
            AppWorkflowStateDto::AwaitingSpecConfirmation,
            "\"awaiting_spec_confirmation\"",
        ),
        (AppWorkflowStateDto::Generating, "\"generating\""),
        (AppWorkflowStateDto::Validating, "\"validating\""),
        (
            AppWorkflowStateDto::AwaitingPreviewConfirmation,
            "\"awaiting_preview_confirmation\"",
        ),
        (AppWorkflowStateDto::Revising, "\"revising\""),
        (AppWorkflowStateDto::Ready, "\"ready\""),
        (
            AppWorkflowStateDto::GenerationFailed,
            "\"generation_failed\"",
        ),
        (
            AppWorkflowStateDto::ValidationFailed,
            "\"validation_failed\"",
        ),
    ];
    for (state, expected) in cases {
        let json = serde_json::to_string(&state).expect("serialize AppWorkflowStateDto");
        assert_eq!(json, expected, "AppWorkflowStateDto::{state:?} wire form");
        let back: AppWorkflowStateDto =
            serde_json::from_str(&json).expect("deserialize AppWorkflowStateDto");
        assert_eq!(back, state);
    }
}

/// `AppRuntimeStateDto` — every spec-§C state round-trips as its bare string.
#[test]
fn runtime_state_serializes_as_bare_string() {
    let cases = [
        (AppRuntimeStateDto::Stopped, "\"stopped\""),
        (AppRuntimeStateDto::Starting, "\"starting\""),
        (AppRuntimeStateDto::Running, "\"running\""),
        (AppRuntimeStateDto::Stopping, "\"stopping\""),
        (AppRuntimeStateDto::Failed, "\"failed\""),
    ];
    for (state, expected) in cases {
        let json = serde_json::to_string(&state).expect("serialize AppRuntimeStateDto");
        assert_eq!(json, expected, "AppRuntimeStateDto::{state:?} wire form");
        let back: AppRuntimeStateDto =
            serde_json::from_str(&json).expect("deserialize AppRuntimeStateDto");
        assert_eq!(back, state);
    }
}

/// `AppCreateOriginDto` — `"chat"` / `"library"` bare strings.
#[test]
fn create_origin_serializes_as_bare_string() {
    let cases = [
        (AppCreateOriginDto::Chat, "\"chat\""),
        (AppCreateOriginDto::Library, "\"library\""),
    ];
    for (origin, expected) in cases {
        let json = serde_json::to_string(&origin).expect("serialize AppCreateOriginDto");
        assert_eq!(json, expected, "AppCreateOriginDto::{origin:?} wire form");
        let back: AppCreateOriginDto =
            serde_json::from_str(&json).expect("deserialize AppCreateOriginDto");
        assert_eq!(back, origin);
    }
}

/// `AppErrorCodeDto` — every extensible failure code rides as its bare string.
#[test]
fn error_code_serializes_as_bare_string() {
    let cases = [
        (AppErrorCodeDto::NotFound, "\"not_found\""),
        (AppErrorCodeDto::RevisionConflict, "\"revision_conflict\""),
        (
            AppErrorCodeDto::InteractionInvalid,
            "\"interaction_invalid\"",
        ),
        (
            AppErrorCodeDto::WorkflowStateInvalid,
            "\"workflow_state_invalid\"",
        ),
        (AppErrorCodeDto::RuntimeBusy, "\"runtime_busy\""),
        (AppErrorCodeDto::NotYetAvailable, "\"not_yet_available\""),
        (AppErrorCodeDto::StorageCorrupt, "\"storage_corrupt\""),
        (AppErrorCodeDto::InvalidRequest, "\"invalid_request\""),
        (AppErrorCodeDto::Io, "\"io\""),
    ];
    for (code, expected) in cases {
        let json = serde_json::to_string(&code).expect("serialize AppErrorCodeDto");
        assert_eq!(json, expected, "AppErrorCodeDto::{code:?} wire form");
        let back: AppErrorCodeDto =
            serde_json::from_str(&json).expect("deserialize AppErrorCodeDto");
        assert_eq!(back, code);
    }
}

/// `AppCheckpointKindDto` — every checkpoint kind rides as its bare string.
#[test]
fn checkpoint_kind_serializes_as_bare_string() {
    let cases = [
        (
            AppCheckpointKindDto::ScaffoldCreated,
            "\"scaffold_created\"",
        ),
        (
            AppCheckpointKindDto::GenerationValidated,
            "\"generation_validated\"",
        ),
        (
            AppCheckpointKindDto::PreviewApproved,
            "\"preview_approved\"",
        ),
        (AppCheckpointKindDto::UserApproved, "\"user_approved\""),
        (AppCheckpointKindDto::PreRestore, "\"pre_restore\""),
    ];
    for (kind, expected) in cases {
        let json = serde_json::to_string(&kind).expect("serialize AppCheckpointKindDto");
        assert_eq!(json, expected, "AppCheckpointKindDto::{kind:?} wire form");
        let back: AppCheckpointKindDto =
            serde_json::from_str(&json).expect("deserialize AppCheckpointKindDto");
        assert_eq!(back, kind);
    }
}

/// `DesignValueDto` is tagged on `kind` with the payload under `value` —
/// BYTE-IDENTICAL to the core `DesignValue` adjacently-tagged wire form for
/// every field kind (spec §A).
#[test]
fn design_value_matches_exact_wire_shape() {
    let cases = [
        (
            DesignValueDto::ShortText {
                value: "Team Board".to_string(),
            },
            r#"{"kind":"short_text","value":"Team Board"}"#,
        ),
        (
            DesignValueDto::LongText {
                value: "line one\nline two".to_string(),
            },
            r#"{"kind":"long_text","value":"line one\nline two"}"#,
        ),
        (
            DesignValueDto::SingleChoice {
                value: "cards".to_string(),
            },
            r#"{"kind":"single_choice","value":"cards"}"#,
        ),
        (
            DesignValueDto::MultipleChoice {
                value: vec!["tags".to_string(), "search".to_string()],
            },
            r#"{"kind":"multiple_choice","value":["tags","search"]}"#,
        ),
        (
            DesignValueDto::Boolean { value: true },
            r#"{"kind":"boolean","value":true}"#,
        ),
        (
            DesignValueDto::Color {
                value: "#aabbcc".to_string(),
            },
            r##"{"kind":"color","value":"#aabbcc"}"##,
        ),
        (
            DesignValueDto::Density {
                value: DensityLevelDto::Compact,
            },
            r#"{"kind":"density","value":"compact"}"#,
        ),
        (
            DesignValueDto::ScreenList {
                value: vec!["home".to_string(), "detail".to_string()],
            },
            r#"{"kind":"screen_list","value":["home","detail"]}"#,
        ),
        (
            DesignValueDto::FeatureList {
                value: vec!["export".to_string()],
            },
            r#"{"kind":"feature_list","value":["export"]}"#,
        ),
    ];
    for (value, expected) in cases {
        let json = serde_json::to_string(&value).expect("serialize DesignValueDto");
        assert_eq!(json, expected, "DesignValueDto::{value:?} wire form");
        let back: DesignValueDto = serde_json::from_str(&json).expect("deserialize DesignValueDto");
        assert_eq!(back, value);
    }
}

/// `DensityLevelDto` — bare `"compact"` / `"comfortable"` strings.
#[test]
fn density_level_serializes_as_bare_string() {
    let cases = [
        (DensityLevelDto::Compact, "\"compact\""),
        (DensityLevelDto::Comfortable, "\"comfortable\""),
    ];
    for (level, expected) in cases {
        let json = serde_json::to_string(&level).expect("serialize DensityLevelDto");
        assert_eq!(json, expected, "DensityLevelDto::{level:?} wire form");
        let back: DensityLevelDto =
            serde_json::from_str(&json).expect("deserialize DensityLevelDto");
        assert_eq!(back, level);
    }
}

/// `AppDesignPatchDto` / `AppDesignPatchOpDto` — the spec-§A wire shape:
/// `{ "op": "set", "field_id": …, "value": … }` / `{ "op": "remove",
/// "field_id": … }`, with an absent `note` omitted.
#[test]
fn design_patch_matches_exact_wire_shape() {
    let patch = AppDesignPatchDto {
        ops: vec![
            AppDesignPatchOpDto::Set {
                field_id: "title".to_string(),
                value: DesignValueDto::ShortText {
                    value: "Hi".to_string(),
                },
            },
            AppDesignPatchOpDto::Remove {
                field_id: "accent".to_string(),
            },
        ],
        note: None,
    };
    let json = serde_json::to_string(&patch).expect("serialize AppDesignPatchDto");
    assert_eq!(
        json,
        r#"{"ops":[{"op":"set","field_id":"title","value":{"kind":"short_text","value":"Hi"}},{"op":"remove","field_id":"accent"}]}"#
    );
    let back: AppDesignPatchDto =
        serde_json::from_str(&json).expect("deserialize AppDesignPatchDto");
    assert_eq!(back, patch);

    // A present note is carried.
    let with_note = AppDesignPatchDto {
        ops: vec![],
        note: Some("polish the palette".to_string()),
    };
    let json_n = serde_json::to_value(&with_note).expect("serialize noted AppDesignPatchDto");
    assert_eq!(json_n["note"], "polish the palette");
    let back_n: AppDesignPatchDto =
        serde_json::from_value(json_n).expect("deserialize noted AppDesignPatchDto");
    assert_eq!(back_n, with_note);
}

/// `AppRecordDto` — `snake_case` protocol fields; an absent `conversation_id` is
/// omitted from the wire.
#[test]
fn app_record_round_trips_and_skips_none_conversation() {
    let record = AppRecordDto {
        id: "habits-1a2b".to_string(),
        name: "Habits".to_string(),
        template: AppTemplateKindDto::Dashboard,
        created_at_ms: 1_750_000_000_000,
        updated_at_ms: 1_750_000_000_001,
        workflow_state: AppWorkflowStateDto::CollectingSpec,
        conversation_id: None,
        workspace_rel: "apps/habits-1a2b/workspace".to_string(),
    };
    let json = serde_json::to_value(&record).expect("serialize AppRecordDto");
    assert_eq!(json["id"], "habits-1a2b");
    assert_eq!(json["template"], "dashboard");
    assert_eq!(json["created_at_ms"], 1_750_000_000_000_u64);
    assert_eq!(json["workflow_state"], "collecting_spec");
    assert_eq!(json["workspace_rel"], "apps/habits-1a2b/workspace");
    assert!(
        json.get("conversation_id").is_none(),
        "None conversation_id must be skipped"
    );
    let back: AppRecordDto = serde_json::from_value(json).expect("deserialize AppRecordDto");
    assert_eq!(back, record);

    let chat_born = AppRecordDto {
        conversation_id: Some("conv-42".to_string()),
        ..record
    };
    let json_c = serde_json::to_value(&chat_born).expect("serialize chat-born AppRecordDto");
    assert_eq!(json_c["conversation_id"], "conv-42");
    let back_c: AppRecordDto =
        serde_json::from_value(json_c).expect("deserialize chat-born AppRecordDto");
    assert_eq!(back_c, chat_born);
}

/// `AppCheckpointDto` — `snake_case` protocol fields + bare-string kind.
#[test]
fn app_checkpoint_round_trips() {
    let checkpoint = AppCheckpointDto {
        id: "ckpt-0001".to_string(),
        label: "Scaffold created".to_string(),
        kind: AppCheckpointKindDto::ScaffoldCreated,
        created_at_ms: 1_750_000_000_000,
    };
    let json = serde_json::to_value(&checkpoint).expect("serialize AppCheckpointDto");
    assert_eq!(json["id"], "ckpt-0001");
    assert_eq!(json["label"], "Scaffold created");
    assert_eq!(json["kind"], "scaffold_created");
    assert_eq!(json["created_at_ms"], 1_750_000_000_000_u64);
    let back: AppCheckpointDto =
        serde_json::from_value(json).expect("deserialize AppCheckpointDto");
    assert_eq!(back, checkpoint);
}

#[test]
fn builtin_templates_define_the_same_five_ordered_steps() {
    let templates = builtin_app_templates();
    assert_eq!(templates.len(), 4);
    for template in &templates {
        assert_eq!(template.version, 1);
        assert_eq!(
            template
                .steps
                .iter()
                .map(|step| (step.order, step.id.as_str()))
                .collect::<Vec<_>>(),
            vec![
                (1, "basic"),
                (2, "structure"),
                (3, "data"),
                (4, "appearance"),
                (5, "permissions"),
            ]
        );
        assert_eq!(template.collections.len(), 1);
        assert!(template
            .steps
            .iter()
            .flat_map(|step| &step.fields)
            .any(|field| { field.field_type == AppDesignFieldTypeDto::DataFieldList }));
        assert!(template
            .steps
            .iter()
            .flat_map(|step| &step.fields)
            .any(|field| { field.field_type == AppDesignFieldTypeDto::DomainList }));
    }
    assert_eq!(templates[0].collections[0].id, "records");
    assert_eq!(templates[1].collections[0].id, "items");
    assert_eq!(templates[2].collections[0].id, "entries");
    assert_eq!(templates[3].collections[0].id, "submissions");
    assert!(!templates[3].collections[0].enabled_by_default);
}

#[test]
fn structured_design_values_round_trip_without_untyped_json() {
    let value = DesignValueDto::DataFieldList {
        value: vec![client_protocol::local_apps::AppDataFieldDto {
            id: "priority".to_string(),
            label: "Priority".to_string(),
            field_type: AppDataFieldTypeDto::Enum,
            required: true,
            options: vec!["low".to_string(), "high".to_string()],
        }],
    };
    let json = serde_json::to_value(&value).expect("serialize data field list");
    assert_eq!(json["kind"], "data_field_list");
    assert_eq!(json["value"][0]["field_type"], "enum");
    let back: DesignValueDto = serde_json::from_value(json).expect("deserialize data field list");
    assert_eq!(back, value);

    let domains = DesignValueDto::DomainList {
        value: vec!["api.example.com".to_string()],
    };
    let json = serde_json::to_value(&domains).expect("serialize domain list");
    assert_eq!(json["kind"], "domain_list");
    let back: DesignValueDto = serde_json::from_value(json).expect("deserialize domain list");
    assert_eq!(back, domains);
}
