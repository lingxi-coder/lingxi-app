use lingxi_telemetry::tengu::session;

#[test]
fn all_15_session_event_names_are_locked() {
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
    ];
    assert_eq!(names.len(), 15, "session category must declare exactly 15 events");
    for n in names {
        assert!(n.starts_with("tengu_session_"), "{n} must start with tengu_session_");
    }
    assert_eq!(session::STARTED, "tengu_session_started");
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
