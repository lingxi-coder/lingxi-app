//! Exercise the desktop composition root, rather than a mocked model picker.
use async_trait::async_trait;
use client::adapter::PermissionRequestSink;
use client::protocol::permission::PermissionRequest as PermissionRequestDto;
use harness_runtime::desktop::{build, DesktopConfig};
use lingxi_core::host::OrchestratorHandle;
use orchestrator::test_support::MockOutputStream;
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

struct UnusedPermissionSink;

#[async_trait]
impl PermissionRequestSink for UnusedPermissionSink {
    async fn emit_request(&self, _request: PermissionRequestDto) {
        panic!("initializing a model route must not ask for tool permission");
    }
}

fn config(model: &str, duplicate: bool, origin: &str) -> (tempfile::TempDir, DesktopConfig) {
    let tmp = tempfile::tempdir().unwrap();
    let mut providers = BTreeMap::new();
    for profile in if duplicate {
        vec!["deepseek", "deepseek-search"]
    } else {
        vec!["deepseek"]
    } {
        providers.insert(
            profile.to_owned(),
            serde_json::json!({
                "type": "openai",
                "baseUrl": format!("{origin}/{profile}"),
                "models": [{"id": "deepseek-flash"}]
            }),
        );
    }
    let mut cfg = DesktopConfig::default();
    cfg.cwd = tmp.path().to_path_buf();
    cfg.lingxi_home = tmp.path().join("home");
    cfg.default_model = model.to_owned();
    cfg.default_model_explicit = true;
    cfg.provider_profiles = Some(providers);
    cfg.isolated_credential_storage = true;
    cfg.credential_storage_policy = lingxi_core::host::CredentialStoragePolicy::PlainTextFixture;
    cfg.use_noop_permission_gate = true;
    cfg.enable_automation_scheduler = false;
    cfg.session_persistence = false;
    cfg.max_turns = Some(1);
    cfg.mcp_paths = Vec::new();
    cfg.setting_source_scope = (false, false);
    (tmp, cfg)
}

async fn receive_request(listener: tokio::net::TcpListener) -> (String, serde_json::Value) {
    let (mut socket, _) = listener.accept().await.unwrap();
    let mut bytes = Vec::new();
    let header_end = loop {
        let mut buf = [0; 4096];
        let n = socket.read(&mut buf).await.unwrap();
        assert_ne!(n, 0, "HTTP request ended before headers");
        bytes.extend_from_slice(&buf[..n]);
        if let Some(end) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
            break end + 4;
        }
    };
    let headers = std::str::from_utf8(&bytes[..header_end])
        .unwrap()
        .to_owned();
    let length: usize = headers
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse().unwrap())
        })
        .expect("request content length");
    while bytes.len() < header_end + length {
        let mut buf = [0; 4096];
        let n = socket.read(&mut buf).await.unwrap();
        assert_ne!(n, 0, "HTTP request ended before body");
        bytes.extend_from_slice(&buf[..n]);
    }
    let body = serde_json::from_slice(&bytes[header_end..header_end + length]).unwrap();
    let response = serde_json::json!({
        "id": "route-test", "object": "chat.completion", "model": "deepseek-flash",
        "choices": [{"index": 0, "message": {"role": "assistant", "content": "MODEL_ROUTE_OK"}, "finish_reason": "stop"}],
        "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2}
    }).to_string();
    socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response}", response.len()).as_bytes()).await.unwrap();
    (headers.lines().next().unwrap().to_owned(), body)
}

#[tokio::test]
async fn desktop_boot_preserves_the_resolved_model_profile() {
    for (model, duplicate, expected_profile) in [
        ("deepseek/deepseek-flash", true, "deepseek"),
        ("deepseek-search/deepseek-flash", true, "deepseek-search"),
        ("deepseek/deepseek-flash", false, "deepseek"),
    ] {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let (_tmp, cfg) = config(model, duplicate, &origin);
        let rt = build(
            cfg,
            Arc::new(MockOutputStream::new()),
            Arc::new(UnusedPermissionSink),
        )
        .await
        .expect("selected route must initialize");
        let status = rt.orchestrator.get_status_snapshot().await;
        assert_eq!(status.model, "deepseek-flash");
        assert_eq!(status.model_profile.as_deref(), Some(expected_profile));
        {
            let session = rt.orchestrator.session();
            let session = session.lock().await;
            let binding = session.user_model_setting.as_ref().expect("model source");
            assert_eq!(binding.route_anchor.model, "deepseek-flash");
            assert_eq!(
                binding.route_anchor.profile.as_deref(),
                Some(expected_profile)
            );
        }
        assert_eq!(
            rt.orchestrator.current_audio_binding().await.unwrap().1,
            expected_profile,
            "optional host metadata must see the same authoritative route"
        );
        rt.credentials
            .set_provider_key_ephemeral(expected_profile, "fixture-only-key")
            .await;
        let request = tokio::spawn(receive_request(listener));
        tokio::time::timeout(
            Duration::from_secs(30),
            rt.orchestrator
                .run_turn("Reply MODEL_ROUTE_OK without using tools."),
        )
        .await
        .expect("first turn must finish")
        .expect("first turn must use the selected provider");
        let (request_line, body) = tokio::time::timeout(Duration::from_secs(5), request)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            request_line,
            format!("POST /{expected_profile}/chat/completions HTTP/1.1")
        );
        assert_eq!(
            body["model"], "deepseek-flash",
            "wire model stays provider-local"
        );
        let report = rt.session_lifecycle.shutdown_and_drain().await;
        assert!(report.complete, "shutdown: {:?}", report.errors);
    }
}

#[tokio::test]
async fn desktop_boot_still_rejects_an_ambiguous_unqualified_model() {
    let (_tmp, cfg) = config("deepseek-flash", true, "http://127.0.0.1:9");
    let result = build(
        cfg,
        Arc::new(MockOutputStream::new()),
        Arc::new(UnusedPermissionSink),
    )
    .await;
    let error = match result {
        Ok(_) => panic!("a bare duplicate model must require an explicit provider"),
        Err(error) => error.to_string(),
    };
    assert!(
        error.contains("served by multiple profiles")
            && error.contains("deepseek-flash")
            && error.contains("deepseek-search"),
        "unexpected build error: {error}"
    );
}
