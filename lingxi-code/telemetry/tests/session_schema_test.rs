use lingxi_telemetry::tengu::session;

#[test]
fn all_20_session_event_names_are_locked() {
    let names: &[&str] = &[
        session::STARTED,
        session::RESUMED,
        session::COMPLETED,
        session::ABORTED,
        session::PERSISTED,
        session::LOAD_FAILED,
        session::ID_GENERATED,
        session::CLEAR_REQUESTED,
        session::CLEAR_COMPLETED,
        session::EXPORT_STARTED,
        session::EXPORT_COMPLETED,
        session::EXPORT_FAILED,
        session::IMPORT_STARTED,
        session::IMPORT_COMPLETED,
        session::IMPORT_FAILED,
        session::APPENDED,
        session::ROTATED,
        session::CORRUPTED,
        session::RESUME_STARTED,
        session::RESUME_COMPLETED,
    ];
    assert_eq!(
        names.len(),
        20,
        "session category must declare exactly 20 events"
    );
    for n in names {
        assert!(
            n.starts_with("tengu_session_"),
            "{n} must start with tengu_session_"
        );
    }
    assert_eq!(session::STARTED, "tengu_session_started");
    // M5-07 jsonl-persistence triplet.
    assert_eq!(session::APPENDED, "tengu_session_appended");
    assert_eq!(session::ROTATED, "tengu_session_rotated");
    assert_eq!(session::CORRUPTED, "tengu_session_corrupted");
    // M5-08 resume pair.
    assert_eq!(session::RESUME_STARTED, "tengu_session_resume_started");
    assert_eq!(session::RESUME_COMPLETED, "tengu_session_resume_completed");
}

#[test]
fn session_started_payload_round_trips() {
    use lingxi_telemetry::Verified;
    let p = session::StartedPayload {
        session_id: Verified::assert_safe("sess-uuid".into()),
        resumed_from: None,
    };
    let json = serde_json::to_string(&p).expect("serialize");
    let _: session::StartedPayload = serde_json::from_str(&json).expect("round-trip");
}

// ---------- M5-07: appended / rotated / corrupted payload round-trips ----------

#[test]
fn appended_payload_round_trips() {
    use lingxi_telemetry::Verified;
    let p = session::AppendedPayload {
        session_id: Verified::assert_safe("11111111-2222-3333-4444-555555555555".into()),
        message_uuid: Verified::assert_safe("aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa".into()),
    };
    let s = serde_json::to_string(&p).expect("ser");
    let back: session::AppendedPayload = serde_json::from_str(&s).expect("de");
    assert_eq!(back.session_id.as_str(), p.session_id.as_str());
    assert_eq!(back.message_uuid.as_str(), p.message_uuid.as_str());
}

#[test]
fn rotated_payload_round_trips() {
    use lingxi_telemetry::Verified;
    let p = session::RotatedPayload {
        session_id: Verified::assert_safe("11111111-2222-3333-4444-555555555555".into()),
        bytes_before_rotation: 50_000_000,
    };
    let s = serde_json::to_string(&p).expect("ser");
    let back: session::RotatedPayload = serde_json::from_str(&s).expect("de");
    assert_eq!(back.session_id.as_str(), p.session_id.as_str());
    assert_eq!(back.bytes_before_rotation, p.bytes_before_rotation);
}

#[test]
fn corrupted_payload_round_trips() {
    use lingxi_telemetry::Verified;
    let p = session::CorruptedPayload {
        session_id: Verified::assert_safe("11111111-2222-3333-4444-555555555555".into()),
        error: Verified::assert_safe("io_error".into()),
    };
    let s = serde_json::to_string(&p).expect("ser");
    let back: session::CorruptedPayload = serde_json::from_str(&s).expect("de");
    assert_eq!(back.error.as_str(), "io_error");
}

#[test]
fn three_new_names_have_correct_prefixes() {
    assert_eq!(session::APPENDED, "tengu_session_appended");
    assert_eq!(session::ROTATED, "tengu_session_rotated");
    assert_eq!(session::CORRUPTED, "tengu_session_corrupted");
}

#[test]
fn resume_started_payload_round_trips() {
    use lingxi_telemetry::Verified;
    let p = session::ResumeStartedPayload {
        session_id: Verified::assert_safe("11111111-2222-3333-4444-555555555555".into()),
    };
    let s = serde_json::to_string(&p).expect("ser");
    let back: session::ResumeStartedPayload = serde_json::from_str(&s).expect("de");
    assert_eq!(back.session_id.as_str(), p.session_id.as_str());
}

#[test]
fn resume_completed_payload_round_trips() {
    use lingxi_telemetry::Verified;
    let p = session::ResumeCompletedPayload {
        session_id: Verified::assert_safe("11111111-2222-3333-4444-555555555555".into()),
        message_count: 7,
    };
    let s = serde_json::to_string(&p).expect("ser");
    let back: session::ResumeCompletedPayload = serde_json::from_str(&s).expect("de");
    assert_eq!(back.session_id.as_str(), p.session_id.as_str());
    assert_eq!(back.message_count, 7);
}

#[test]
fn two_resume_names_have_correct_prefixes() {
    assert_eq!(session::RESUME_STARTED, "tengu_session_resume_started");
    assert_eq!(session::RESUME_COMPLETED, "tengu_session_resume_completed");
}
