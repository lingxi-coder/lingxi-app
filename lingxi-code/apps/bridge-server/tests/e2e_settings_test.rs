//! Task 21a — automating the ENGINE half of the manual settings QA checklist.
//!
//! Follows the `e2e_serve_test.rs` harness exactly: `bridge_server::boot::assemble`
//! builds a real `engine_desktop::DesktopRuntime` from a deterministic
//! `DesktopConfig` rooted at a sandbox temp dir, and the resulting connection is
//! served by the same `McpEndpoint` the production binary uses. Where
//! `e2e_serve_test.rs` proves the turn/handshake path, this file proves the
//! SETTINGS path: `RefreshListings{Settings}`, `UpdateSettings`, and
//! `UpdatePermissionRules` reaching REAL files on disk, over the REAL
//! WebSocket transport — not a direct `router.route()` call (see
//! `router_test.rs`, which already covers the decode/dispatch logic
//! in-process) and not a mocked bridge.
//!
//! `sandbox_config` deliberately roots `lingxi_home` and `cwd` at TWO SEPARATE
//! directories (unlike `e2e_serve_test.rs`'s single shared tempdir, which never
//! touches settings and so never notices): `SettingsPaths::project_dir` is
//! `cwd`, and the Project/Local layer files live at
//! `<cwd>/<DOT_DIR>/settings*.json`, while the User layer lives at
//! `<lingxi_home>/settings.json`. If `lingxi_home` were `cwd.join(DOT_DIR)` (as
//! `e2e_serve_test.rs`'s helper sets it), the User and Project layers would
//! resolve to the identical file and every precedence assertion below would
//! pass by accident.

#![allow(clippy::unwrap_used)]

use std::sync::Arc;
use std::time::Duration;

use bridge::wire::Frame;
use bridge::{BridgeRequest, Capabilities, ClientHello, McpEndpoint, BRIDGE_PROTOCOL_VERSION};
use bridge_server::boot;
use client_protocol::commands::{
    ClientCommand, ListingKindDto, PermissionBehaviorDto, SettingsDestinationDto,
};
use client_protocol::events::{ClientEvent, ErrorKindDto};
use engine_desktop::DesktopConfig;
use futures_util::{SinkExt, StreamExt};
use serde_json::Value;
use tokio_tungstenite::tungstenite::handshake::client::generate_key;
use tokio_tungstenite::tungstenite::Message;

/// `boot::assemble` → `initialize_live_session` writes to PROCESS-GLOBAL
/// statics (`PROCESS_DIR` / `PROCESS_SESSION` / `PROCESS_NAME` in
/// `platform_api::live_sessions`) shared by every test in this binary. Rust's
/// default harness runs a file's `#[tokio::test]` fns CONCURRENTLY, so
/// without this these five tests race each other on that global state — this
/// file's own version of the guard `boot.rs`'s `LOOP_KA_TEST_SERIAL` documents
/// ("keep the entire guard lifetime serialized so another test cannot ...
/// overwrite its globals"). `bridge_server::driver::LOOP_KA_TEST_SERIAL` is
/// `pub(crate)` and unreachable from this external test crate, hence a
/// file-local mutex rather than reusing it. Held for the WHOLE test body (the
/// guard binding lives to the end of the function), not just around
/// `boot::assemble`, because the served connection keeps touching the live
/// session for the test's duration.
static PROCESS_GLOBALS_SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn serialize_process_globals() -> std::sync::MutexGuard<'static, ()> {
    PROCESS_GLOBALS_SERIAL
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

const TEST_TOKEN: &str = "settings-e2e-token-32chars000000000";

/// The sandbox's stand-in for the machine's managed (policy) settings root —
/// see `sandbox_config` for why this must never be the real one.
fn managed_dir(tmp: &tempfile::TempDir) -> std::path::PathBuf {
    tmp.path().join("managed")
}

/// A deterministic, env-free `DesktopConfig` with `lingxi_home` and `cwd`
/// rooted at two SEPARATE sandbox directories (see module doc for why this
/// must not share a root the way `e2e_serve_test.rs`'s helper does). The api
/// key is empty and credential storage is isolated, exactly like
/// `e2e_serve_test.rs::sandbox_config` — these tests never send a turn, but
/// `boot::assemble` still builds the full runtime, and a stray provider key in
/// the developer's real keychain must not make this test non-hermetic.
fn sandbox_config() -> (tempfile::TempDir, DesktopConfig) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let cwd = tmp.path().join("repo");
    let lingxi_home = tmp.path().join("home");
    std::fs::create_dir_all(&cwd).expect("create sandbox repo dir");
    std::fs::create_dir_all(&lingxi_home).expect("create sandbox home dir");
    std::fs::create_dir_all(managed_dir(&tmp)).expect("create sandbox managed dir");
    // The FOURTH root this harness has to isolate, and the one `cwd` /
    // `lingxi_home` / `isolated_credential_storage` do NOT cover:
    // `boot::assemble` folds `engine_desktop::managed_settings_overlay()` on
    // top of the file layers, and that resolves through
    // `settings_watch::managed_settings_dir()`, which falls back to the
    // machine's REAL policy directory (`/Library/Application
    // Support/LingXi/…` on macOS) unless `LINGXI_MANAGED_DIR` is set. On a
    // machine with an installed managed policy, that policy's keys would
    // show up in every `locked`/`effective` assertion below — the suite would
    // pass or fail depending on whose laptop ran it.
    //
    // `set_var` is process-global, so read carefully before copying this
    // pattern. What makes it safe here is NOT "integration tests are one
    // binary per file" — that only bounds who the readers are. It is that
    // every test binds `serialize_process_globals()` to a NAMED guard at the
    // top of its body, so the guard lives until the whole async fn returns,
    // `endpoint.shutdown().await` included. No second test can be between
    // `set_var` and its read. Bind it to a bare `_` instead and the guard
    // drops immediately, serializing nothing while looking identical here.
    //
    // Second load-bearing fact: `MANAGED_DIR_ENV` is read exactly once,
    // synchronously, inside `boot::assemble` (see `boot.rs`, which caches
    // the resolved `managed` value rather than re-reading per request). It
    // is worth stating why that matters, because `McpEndpoint::shutdown`
    // documents that it does NOT await in-flight per-connection tasks — a
    // prior test's orphaned connection task can still be alive when the next
    // test calls `set_var`. That window is inert only because nothing on the
    // live request path reads this variable. If any request-path code ever
    // starts reading env vars, this guard stops being sufficient.
    std::env::set_var(
        engine_desktop::settings_watch::MANAGED_DIR_ENV,
        managed_dir(&tmp),
    );
    let cfg = DesktopConfig {
        enable_automation_scheduler: true,
        host_workspace_trusted: None,
        api_base: "https://api.anthropic.com".to_string(),
        api_key: String::new(),
        api_key_helper: None,
        managed_oauth_only: false,
        anthropic_key_fd_present: false,
        cwd: cwd.clone(),
        lingxi_home,
        default_model: "claude-sonnet-4-20250514".to_string(),
        default_model_explicit: true,
        // Same hazard as `e2e_serve_test.rs`: without this, any provider key
        // in the developer's real login keychain would make this test
        // machine-dependent.
        isolated_credential_storage: true,
        credential_storage_policy: platform_api::CredentialStoragePolicy::NativePreferred,
        recent_models: Vec::new(),
        fallback_model: None,
        custom_betas: Vec::new(),
        flag_settings: None,
        provider_profiles: None,
        routing: None,
        mcp_paths: vec![cwd.join(".mcp.json")],
        use_noop_permission_gate: false,
        deny_unresolved_ask: false,
        is_tty: false,
        initial_teammate_team_name: None,
        injected_permission_gate: None,
        injected_plugin_secrets: Default::default(),
        ask_user_question_tx: None,
        computer_access_tx: None,
        session_agent_observer: None,
        // `assemble` fills this with the connection's own `AudioBridge`.
        audio: None,
        session_started_as_coordinator: false,
        memory_provider: None,
        permission_mode: permission::PermissionMode::Default,
        permission_mode_cli: None,
        permission_mode_preference: None,
        permission_mode_cli_explicit: false,
        allow_dangerously_skip_permissions: false,
        connect_prompt: None,
        max_turns: None,
        plan_mode_instructions: None,
        plans_directory: None,
        max_budget_usd: None,
        json_schema: None,
        system_prompt_override: None,
        append_system_prompt: None,
        session_id_override: None,
        session_writer_lease: None,
        disable_slash_commands: false,
        add_dir: Vec::new(),
        cli_mcp_servers: Vec::new(),
        strict_mcp_config: false,
        restricted: false,
        restricted_tools: None,
        exclude_dynamic_system_prompt_sections: false,
        setting_source_scope: (true, true),
        customization_gates: engine_desktop::CustomizationGates::default(),
        session_persistence: true,
        cli_agents_json: None,
        cli_agent: None,
        cli_plugin_dirs: Vec::new(),
        initial_effort: None,
        default_model_env_pinned: false,
        session_thinking: Default::default(),
        parent_session_id: None,
        bg_session_forker: None,
        worktree_launch: None,
        tmux_launch: None,
    };
    (tmp, cfg)
}

/// Open an authenticated WS connection to `port` and complete the opening
/// `hello` handshake, so each test starts from a ready-to-submit connection.
async fn connect(
    port: u16,
) -> tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>> {
    let url = format!("ws://127.0.0.1:{port}/mcp");
    let req = http::Request::builder()
        .method("GET")
        .uri(&url)
        .header("host", format!("127.0.0.1:{port}"))
        .header("connection", "upgrade")
        .header("upgrade", "websocket")
        .header("sec-websocket-version", "13")
        .header("sec-websocket-key", generate_key())
        .header("sec-websocket-protocol", "mcp")
        .header("x-lingxi-ide-authorization", TEST_TOKEN)
        .body(())
        .unwrap();
    let (mut ws, response) = tokio_tungstenite::connect_async(req)
        .await
        .expect("ws upgrade must succeed");
    assert_eq!(response.status(), 101, "upgrade must return 101");

    let hello = Frame::Request(BridgeRequest {
        id: 0,
        method: "hello".into(),
        params: serde_json::to_value(ClientHello {
            protocol_version: BRIDGE_PROTOCOL_VERSION.into(),
            client_name: "settings-e2e-test".into(),
            capabilities: Capabilities::default(),
        })
        .unwrap(),
    });
    send_frame(&mut ws, &hello).await;
    match next_frame(&mut ws).await {
        Frame::Response(resp) => assert!(resp.error.is_none(), "hello must not error"),
        other => panic!("expected ServerHello response, got {other:?}"),
    }
    ws
}

async fn next_frame<S>(ws: &mut S) -> Frame
where
    S: StreamExt<Item = Result<Message, tokio_tungstenite::tungstenite::Error>> + Unpin,
{
    loop {
        let msg = tokio::time::timeout(Duration::from_secs(10), ws.next())
            .await
            .expect("a frame must arrive within the timeout")
            .expect("stream must yield a message")
            .expect("message must not be a ws error");
        match msg {
            Message::Text(t) => return serde_json::from_str(&t).expect("decode Frame"),
            _ => continue,
        }
    }
}

async fn send_frame<S>(ws: &mut S, frame: &Frame)
where
    S: SinkExt<Message, Error = tokio_tungstenite::tungstenite::Error> + Unpin,
{
    let text = serde_json::to_string(frame).expect("serialize frame");
    ws.send(Message::Text(text)).await.expect("send frame");
}

fn submit(command: &ClientCommand) -> Frame {
    Frame::Request(BridgeRequest {
        id: 1,
        method: "submit".into(),
        params: serde_json::to_value(command).expect("serialize command"),
    })
}

/// (1) `RefreshListings{Settings}` over the real transport returns a real
/// merged snapshot: `effective_json` resolves the way the engine's own
/// precedence says it should (project beats user), `provenance_json` names
/// the layer that actually won, `layers_json` carries each layer's OWN raw
/// value (not the merged view), and `files_json` lists the real on-disk paths.
#[tokio::test]
async fn settings_listing_returns_a_real_merged_snapshot() {
    // Serialized: `boot::assemble` writes process-global live-session
    // statics every test in this file shares (see
    // `PROCESS_GLOBALS_SERIAL`'s doc above).
    let _serial = serialize_process_globals();
    let (_tmp, cfg) = sandbox_config();
    let home = cfg.lingxi_home.clone();
    let project = cfg.cwd.clone();
    std::fs::write(
        home.join("settings.json"),
        r#"{"outputStyle":"from-user","model":"opus"}"#,
    )
    .unwrap();
    let project_dot = project.join(branding::DOT_DIR);
    std::fs::create_dir_all(&project_dot).unwrap();
    std::fs::write(
        project_dot.join("settings.json"),
        r#"{"outputStyle":"from-project"}"#,
    )
    .unwrap();

    let bound = boot::assemble(cfg).await.expect("assemble must succeed");
    let endpoint = McpEndpoint::start_on_ephemeral_port_with_pump(Arc::new(bound.connection))
        .await
        .expect("endpoint must start");
    endpoint.set_auth_token(TEST_TOKEN.to_string());
    let mut ws = connect(endpoint.port()).await;

    send_frame(
        &mut ws,
        &submit(&ClientCommand::RefreshListings {
            which: vec![ListingKindDto::Settings],
        }),
    )
    .await;

    let (effective_json, provenance_json, files_json, layers_json) = match next_frame(&mut ws).await
    {
        Frame::Event(ClientEvent::SettingsSnapshot {
            effective_json,
            provenance_json,
            files_json,
            layers_json,
            ..
        }) => (
            effective_json,
            provenance_json,
            files_json.expect("files_json must be populated"),
            layers_json.expect("layers_json must be populated"),
        ),
        other => panic!("expected a SettingsSnapshot event, got {other:?}"),
    };

    let effective: Value = serde_json::from_str(&effective_json).unwrap();
    assert_eq!(
        effective["outputStyle"], "from-project",
        "project must beat user per the engine's file-layer precedence, got {effective}"
    );
    assert_eq!(
        effective["model"], "opus",
        "a user-only key must still surface in the merge, got {effective}"
    );

    let provenance: Value = serde_json::from_str(&provenance_json).unwrap();
    assert_eq!(
        provenance["outputStyle"], "project",
        "provenance must name the layer that actually won, got {provenance}"
    );

    let layers: Value = serde_json::from_str(&layers_json).unwrap();
    assert_eq!(
        layers["user"]["outputStyle"], "from-user",
        "the user layer's OWN raw value must survive unmerged, got {layers}"
    );
    assert_eq!(
        layers["project"]["outputStyle"], "from-project",
        "the project layer's OWN raw value must survive unmerged, got {layers}"
    );

    let files: Value = serde_json::from_str(&files_json).unwrap();
    let files_arr = files.as_array().expect("files_json must be an array");
    let user_file = files_arr
        .iter()
        .find(|f| f["layer"] == "user")
        .expect("a user file entry must be present");
    assert_eq!(
        user_file["path"],
        home.join("settings.json").to_string_lossy().into_owned(),
        "files_json must list the REAL on-disk user path, got {user_file}"
    );
    assert_eq!(user_file["exists"], true);
    assert_eq!(user_file["parsed"], true);
    let project_file = files_arr
        .iter()
        .find(|f| f["layer"] == "project")
        .expect("a project file entry must be present");
    assert_eq!(
        project_file["path"],
        project_dot
            .join("settings.json")
            .to_string_lossy()
            .into_owned(),
        "files_json must list the REAL on-disk project path, got {project_file}"
    );
    assert_eq!(project_file["exists"], true);

    endpoint.shutdown().await;
}

/// (2) `UpdateSettings` lands in the REAL file on disk, and an unrelated key
/// already in that file survives verbatim — the automated form of the
/// checklist's "quit the app and `cat` the file" step.
#[tokio::test]
async fn update_settings_write_lands_in_the_real_file_and_preserves_siblings() {
    // Serialized: `boot::assemble` writes process-global live-session
    // statics every test in this file shares (see
    // `PROCESS_GLOBALS_SERIAL`'s doc above).
    let _serial = serialize_process_globals();
    let (_tmp, cfg) = sandbox_config();
    let home = cfg.lingxi_home.clone();
    let user_settings_path = home.join("settings.json");
    std::fs::write(
        &user_settings_path,
        r#"{"outputStyle":"terse","model":"opus"}"#,
    )
    .unwrap();

    let bound = boot::assemble(cfg).await.expect("assemble must succeed");
    let endpoint = McpEndpoint::start_on_ephemeral_port_with_pump(Arc::new(bound.connection))
        .await
        .expect("endpoint must start");
    endpoint.set_auth_token(TEST_TOKEN.to_string());
    let mut ws = connect(endpoint.port()).await;

    send_frame(
        &mut ws,
        &submit(&ClientCommand::UpdateSettings {
            destination: SettingsDestinationDto::User,
            patch_json: r#"{"outputStyle":"verbose"}"#.to_string(),
        }),
    )
    .await;

    match next_frame(&mut ws).await {
        Frame::Event(ClientEvent::SettingsSnapshot { .. }) => {}
        other => panic!("expected a SettingsSnapshot ack for the write, got {other:?}"),
    }

    let on_disk: Value =
        serde_json::from_str(&std::fs::read_to_string(&user_settings_path).unwrap()).unwrap();
    assert_eq!(
        on_disk["outputStyle"], "verbose",
        "the write must land in the real file on disk, got {on_disk}"
    );
    assert_eq!(
        on_disk["model"], "opus",
        "an unrelated key already in the file must survive verbatim, got {on_disk}"
    );

    endpoint.shutdown().await;
}

/// (3) `UpdatePermissionRules` lands via its own command/writer: the rule
/// appears in `permissions.allow` in the real project settings file, and the
/// rest of the file — an unrelated top-level key — is untouched.
#[tokio::test]
async fn update_permission_rules_lands_in_the_real_project_file() {
    // Serialized: `boot::assemble` writes process-global live-session
    // statics every test in this file shares (see
    // `PROCESS_GLOBALS_SERIAL`'s doc above).
    let _serial = serialize_process_globals();
    let (_tmp, cfg) = sandbox_config();
    let project = cfg.cwd.clone();
    let project_dot = project.join(branding::DOT_DIR);
    std::fs::create_dir_all(&project_dot).unwrap();
    let project_settings_path = project_dot.join("settings.json");
    std::fs::write(&project_settings_path, r#"{"outputStyle":"terse"}"#).unwrap();

    let bound = boot::assemble(cfg).await.expect("assemble must succeed");
    let endpoint = McpEndpoint::start_on_ephemeral_port_with_pump(Arc::new(bound.connection))
        .await
        .expect("endpoint must start");
    endpoint.set_auth_token(TEST_TOKEN.to_string());
    let mut ws = connect(endpoint.port()).await;

    send_frame(
        &mut ws,
        &submit(&ClientCommand::UpdatePermissionRules {
            destination: SettingsDestinationDto::Project,
            behavior: PermissionBehaviorDto::Allow,
            add: vec!["Bash(ls)".to_string()],
            remove: Vec::new(),
        }),
    )
    .await;

    match next_frame(&mut ws).await {
        Frame::Event(ClientEvent::SettingsSnapshot { .. }) => {}
        other => panic!("expected a SettingsSnapshot ack for the rule write, got {other:?}"),
    }

    let on_disk: Value =
        serde_json::from_str(&std::fs::read_to_string(&project_settings_path).unwrap()).unwrap();
    assert_eq!(
        on_disk["permissions"]["allow"][0], "Bash(ls)",
        "the rule must land in permissions.allow on the real file, got {on_disk}"
    );
    assert_eq!(
        on_disk["outputStyle"], "terse",
        "an unrelated top-level key must be untouched, got {on_disk}"
    );

    endpoint.shutdown().await;
}

/// (4) A layer file with invalid JSON is never clobbered: `UpdateSettings`
/// targeting that layer is refused with an `Error`, and the file's bytes are
/// byte-for-byte unchanged. This is the safety property the raw-JSON page's
/// copy now depends on being true.
#[tokio::test]
async fn update_settings_refuses_to_clobber_a_broken_layer_file() {
    // Serialized: `boot::assemble` writes process-global live-session
    // statics every test in this file shares (see
    // `PROCESS_GLOBALS_SERIAL`'s doc above).
    let _serial = serialize_process_globals();
    let (_tmp, cfg) = sandbox_config();
    let project = cfg.cwd.clone();
    let project_dot = project.join(branding::DOT_DIR);
    std::fs::create_dir_all(&project_dot).unwrap();
    let local_settings_path = project_dot.join("settings.local.json");
    let broken_bytes = b"{ this is not valid json".to_vec();
    std::fs::write(&local_settings_path, &broken_bytes).unwrap();

    let bound = boot::assemble(cfg).await.expect("assemble must succeed");
    let endpoint = McpEndpoint::start_on_ephemeral_port_with_pump(Arc::new(bound.connection))
        .await
        .expect("endpoint must start");
    endpoint.set_auth_token(TEST_TOKEN.to_string());
    let mut ws = connect(endpoint.port()).await;

    send_frame(
        &mut ws,
        &submit(&ClientCommand::UpdateSettings {
            destination: SettingsDestinationDto::Local,
            patch_json: r#"{"outputStyle":"x"}"#.to_string(),
        }),
    )
    .await;

    match next_frame(&mut ws).await {
        Frame::Event(ClientEvent::Error { kind, message }) => {
            assert_eq!(
                kind,
                ErrorKindDto::Internal,
                "a broken destination file must be reported as an internal failure, got {message}"
            );
            assert!(
                message.to_lowercase().contains("json"),
                "the error must say what is wrong with the file, got: {message}"
            );
        }
        other => panic!("expected the write to be refused with an Error, got {other:?}"),
    }

    let after = std::fs::read(&local_settings_path).unwrap();
    assert_eq!(
        after, broken_bytes,
        "a broken layer file must NEVER be overwritten, even when the write is refused"
    );

    endpoint.shutdown().await;
}

/// (5) `merged_keys` reflects a real union: a deep-merged key (`hooks`) seeded
/// with different sub-entries in two real layer files must show BOTH entries
/// in `effective_json`, and the key must be named in `merged_keys`.
#[tokio::test]
async fn merged_keys_reflects_a_real_cross_layer_union() {
    // Serialized: `boot::assemble` writes process-global live-session
    // statics every test in this file shares (see
    // `PROCESS_GLOBALS_SERIAL`'s doc above).
    let _serial = serialize_process_globals();
    let (_tmp, cfg) = sandbox_config();
    let home = cfg.lingxi_home.clone();
    std::fs::write(
        home.join("settings.json"),
        r#"{"hooks":{"PreToolUse":{"Bash":"from-user"}}}"#,
    )
    .unwrap();
    let project_dot = cfg.cwd.join(branding::DOT_DIR);
    std::fs::create_dir_all(&project_dot).unwrap();
    std::fs::write(
        project_dot.join("settings.json"),
        r#"{"hooks":{"PostToolUse":{"Read":"from-project"}}}"#,
    )
    .unwrap();

    let bound = boot::assemble(cfg).await.expect("assemble must succeed");
    let endpoint = McpEndpoint::start_on_ephemeral_port_with_pump(Arc::new(bound.connection))
        .await
        .expect("endpoint must start");
    endpoint.set_auth_token(TEST_TOKEN.to_string());
    let mut ws = connect(endpoint.port()).await;

    send_frame(
        &mut ws,
        &submit(&ClientCommand::RefreshListings {
            which: vec![ListingKindDto::Settings],
        }),
    )
    .await;

    match next_frame(&mut ws).await {
        Frame::Event(ClientEvent::SettingsSnapshot {
            effective_json,
            merged_keys,
            ..
        }) => {
            let effective: Value = serde_json::from_str(&effective_json).unwrap();
            assert_eq!(
                effective["hooks"]["PreToolUse"]["Bash"], "from-user",
                "the user layer's hook must survive the real union, got {effective}"
            );
            assert_eq!(
                effective["hooks"]["PostToolUse"]["Read"], "from-project",
                "the project layer's hook must survive the real union, got {effective}"
            );
            let merged = merged_keys.expect("merged_keys must be populated");
            assert!(
                merged.contains(&"hooks".to_string()),
                "hooks resolved from two real layers at once and must be named in \
                 merged_keys, got {merged:?}"
            );
        }
        other => panic!("expected a SettingsSnapshot event, got {other:?}"),
    }

    endpoint.shutdown().await;
}

/// (6) The managed (policy) layer this suite reads is the SANDBOX's, not the
/// machine's.
///
/// This is the A/B for `sandbox_config`'s `LINGXI_MANAGED_DIR` override, not
/// just a feature test for the managed tier: it writes a policy file into the
/// sandbox's managed root and asserts the value reaches `effective_json` and
/// the key reaches `locked`. Without the override,
/// `settings_watch::managed_settings_dir()` resolves to the real OS policy
/// directory, this file is never read, and both assertions fail — which is
/// the same reason the other five tests were previously at the mercy of
/// whatever policy the developer's machine happened to have installed.
#[tokio::test]
async fn the_managed_layer_comes_from_the_sandbox_not_the_machine() {
    // Serialized: `boot::assemble` writes process-global live-session
    // statics every test in this file shares, and `sandbox_config` writes the
    // process-global `LINGXI_MANAGED_DIR` (see `PROCESS_GLOBALS_SERIAL`'s doc
    // above and `sandbox_config`'s own).
    let _serial = serialize_process_globals();
    let (tmp, cfg) = sandbox_config();
    let home = cfg.lingxi_home.clone();
    std::fs::write(home.join("settings.json"), r#"{"outputStyle":"from-user"}"#).unwrap();
    std::fs::write(
        managed_dir(&tmp).join("managed-settings.json"),
        r#"{"outputStyle":"from-sandbox-policy"}"#,
    )
    .unwrap();

    let bound = boot::assemble(cfg).await.expect("assemble must succeed");
    let endpoint = McpEndpoint::start_on_ephemeral_port_with_pump(Arc::new(bound.connection))
        .await
        .expect("endpoint must start");
    endpoint.set_auth_token(TEST_TOKEN.to_string());
    let mut ws = connect(endpoint.port()).await;

    send_frame(
        &mut ws,
        &submit(&ClientCommand::RefreshListings {
            which: vec![ListingKindDto::Settings],
        }),
    )
    .await;

    match next_frame(&mut ws).await {
        Frame::Event(ClientEvent::SettingsSnapshot {
            effective_json,
            provenance_json,
            locked,
            ..
        }) => {
            let effective: Value = serde_json::from_str(&effective_json).unwrap();
            assert_eq!(
                effective["outputStyle"], "from-sandbox-policy",
                "the managed overlay this suite reads must be the sandbox's file, not the \
                 machine's real policy directory, got {effective}"
            );
            let provenance: Value = serde_json::from_str(&provenance_json).unwrap();
            assert_eq!(
                provenance["outputStyle"], "managed",
                "a key the managed overlay supplies must be attributed to `managed`, got \
                 {provenance}"
            );
            let locked = locked.expect("locked must be populated");
            assert!(
                locked.contains(&"outputStyle".to_string()),
                "the sandbox policy's keys are the locked set, got {locked:?}"
            );
        }
        other => panic!("expected a SettingsSnapshot event, got {other:?}"),
    }

    endpoint.shutdown().await;
}
