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
                "models": [{"id": "deepseek-flash", "metadata": {"contextWindowTokens": 1000000, "maxInputTokens": 1000000, "maxOutputTokens": 8192}}]
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

#[tokio::test]
async fn weather_usage_is_refreshed_after_turn_resume_and_clear() {
    use lingxi_core::types::utf16_json::Utf16JsonProjection as Json;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let (tmp, cfg) = config("deepseek/deepseek-flash", false, &origin);
    let rt = build(
        cfg,
        Arc::new(MockOutputStream::new()),
        Arc::new(UnusedPermissionSink),
    )
    .await
    .unwrap();
    let module = tmp.path().join("weather-probe.mjs");
    std::fs::write(&module, r#"
      let readings = [];
      export function register(on) {
        on('session.start', async ($, e, next) => { const result = await next(e); readings = [await $.session.usage()]; return result; });
        on('turn.complete', async ($, e, next) => { const result = await next(e); if (!e.agentId) { const usage = await $.session.usage(); readings.push(usage); } return result; });
        on('tool.call', () => ({ result: { readings: readings.slice() } }));
      }
    "#).unwrap();
    let host = hooks::mods::ModHost::start(None).await.unwrap();
    host.load(
        "weather-probe",
        tmp.path(),
        &module,
        Json::parse("{}").unwrap(),
    )
    .await
    .unwrap();
    rt.hook_registry.write().await.set_mod_host(host.clone());
    tokio::time::timeout(
        Duration::from_secs(10),
        rt.orchestrator.fire_session_start("startup"),
    )
    .await
    .expect("session.start probe timed out");
    rt.credentials
        .set_provider_key_ephemeral("deepseek", "fixture-only-key")
        .await;
    let request = tokio::spawn(receive_request(listener));
    tokio::time::timeout(
        Duration::from_secs(30),
        rt.orchestrator
            .run_turn("Reply MODEL_ROUTE_OK without using tools."),
    )
    .await
    .unwrap()
    .unwrap();
    tokio::time::timeout(Duration::from_secs(5), request)
        .await
        .expect("model request did not reach fixture")
        .unwrap();
    let direct = hooks::mods::ModSessionContext::usage(rt.orchestrator.as_ref(), None)
        .await
        .unwrap();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&direct.to_json_string().unwrap()).unwrap()
            ["context"]["tokens"],
        1
    );
    let observations = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let result = host
                .dispatch_with_log_at_session(
                    "tool.call",
                    Json::parse(r#"{"tool":"WeatherProbe"}"#).unwrap(),
                    rt.orchestrator.as_ref(),
                    |_| async { panic!("probe handles call") },
                    |_, _| async {},
                )
                .await
                .unwrap();
            let value: serde_json::Value =
                serde_json::from_str(&result.to_json_string().unwrap()).unwrap();
            if value["result"]["readings"]
                .as_array()
                .is_some_and(|rows| rows.len() >= 2)
            {
                break value;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("turn.complete must refresh the reading");
    assert_eq!(
        observations["result"]["readings"][1]["context"]["tokens"], 1,
        "{observations}"
    );
    rt.orchestrator
        .resume_session(
            lingxi_core::types::SessionId::new(),
            vec![],
            None,
            None,
            lingxi_core::host::ResumeRuntimeSnapshot {
                current_usage: Some(lingxi_core::host::CurrentUsageSnapshot {
                    input_tokens: 800,
                    cache_read_input_tokens: 23200,
                    ..Default::default()
                }),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let resumed = host
        .dispatch_with_log_at_session(
            "tool.call",
            Json::parse(r#"{"tool":"WeatherProbe"}"#).unwrap(),
            rt.orchestrator.as_ref(),
            |_| async { panic!("probe handles call") },
            |_, _| async {},
        )
        .await
        .unwrap();
    let resumed: serde_json::Value =
        serde_json::from_str(&resumed.to_json_string().unwrap()).unwrap();
    assert_eq!(
        resumed["result"]["readings"].as_array().unwrap().len(),
        1,
        "resume must reset the Mod reading history via session.start"
    );
    assert_eq!(
        resumed["result"]["readings"][0]["context"]["tokens"], 24000,
        "session.start must see restored provider usage including cached tokens"
    );
    rt.orchestrator.clear_session().await.unwrap();
    let cleared = host
        .dispatch_with_log_at_session(
            "tool.call",
            Json::parse(r#"{"tool":"WeatherProbe"}"#).unwrap(),
            rt.orchestrator.as_ref(),
            |_| async { panic!("probe handles call") },
            |_, _| async {},
        )
        .await
        .unwrap();
    let cleared: serde_json::Value =
        serde_json::from_str(&cleared.to_json_string().unwrap()).unwrap();
    assert_eq!(cleared["result"]["readings"].as_array().unwrap().len(), 1);
    assert!(
        cleared["result"]["readings"][0]["context"]
            .get("tokens")
            .is_none(),
        "clear must not retain the resumed token reading"
    );
    let report = rt.session_lifecycle.shutdown_and_drain().await;
    assert!(report.complete, "shutdown: {:?}", report.errors);
}
