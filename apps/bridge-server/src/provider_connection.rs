//! Provider connection probe used by the Desktop settings surface.
//!
//! The probe stays in the engine process so a credential read from the shared
//! secure store is never returned to Electron. It mirrors the mobile engine's
//! low-cost model-list check: DNS/TLS, authentication, and the selected model
//! are verified without spending inference tokens.

use std::sync::Arc;
use std::time::{Duration, Instant};

use client::protocol::commands::ProviderCredentialSecretDto;
use client::protocol::events::ClientEvent;
use lingxi_core::host::{HttpError, HttpTransport};
use lingxi_core::types::{HttpMethod, HttpRequest, HttpResponse};

const TIMEOUT: Duration = Duration::from_secs(15);

pub(crate) struct ProviderConnectionProbe {
    pub operation_id: u64,
    pub provider_id: String,
    pub api_base: String,
    pub model: String,
    pub credential_override: Option<ProviderCredentialSecretDto>,
}

pub(crate) async fn test(
    credentials: Option<&Arc<secret::CredentialManager>>,
    http: Option<&Arc<dyn HttpTransport>>,
    probe: ProviderConnectionProbe,
) -> ClientEvent {
    let draft = probe
        .credential_override
        .as_ref()
        .map(ProviderCredentialSecretDto::expose_secret)
        .filter(|value| !value.trim().is_empty());
    let used_stored_credential = draft.is_none();
    let credential = if let Some(value) = draft {
        value.to_string()
    } else if let Some(credentials) = credentials {
        match credentials.get_provider_key(&probe.provider_id).await {
            Ok(Some(secret)) => secret.expose_secret().clone(),
            Ok(None) => {
                return failure(
                    &probe,
                    "请先输入或保存 API Key",
                    false,
                    false,
                    None,
                    0,
                    true,
                );
            }
            Err(_) => {
                return failure(
                    &probe,
                    "无法读取本机安全存储中的 API Key",
                    false,
                    false,
                    None,
                    0,
                    true,
                );
            }
        }
    } else {
        return failure(
            &probe,
            "Provider 凭据存储不可用",
            false,
            false,
            None,
            0,
            true,
        );
    };

    if credential.len() > 16_384 || credential.contains('\0') {
        return failure(
            &probe,
            "API Key 格式无效",
            false,
            false,
            None,
            0,
            used_stored_credential,
        );
    }
    let Some(http) = http else {
        return failure(
            &probe,
            "Provider 连接测试不可用",
            false,
            false,
            None,
            0,
            used_stored_credential,
        );
    };
    let endpoint = match models_endpoint(&probe.api_base, &probe.provider_id) {
        Ok(endpoint) => endpoint,
        Err(message) => {
            return failure(
                &probe,
                message,
                false,
                false,
                None,
                0,
                used_stored_credential,
            );
        }
    };
    let request = HttpRequest {
        method: HttpMethod::Get,
        url: endpoint,
        headers: connection_headers(&probe.provider_id, &credential),
        body: None,
        body_bytes: None,
        timeout: Some(TIMEOUT),
    };
    let started = Instant::now();
    let response = http.request(request).await;
    let latency_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    classify(response, &probe, latency_ms, used_stored_credential)
}

fn models_endpoint(api_base: &str, provider_id: &str) -> Result<String, &'static str> {
    let base = api_base.trim().trim_end_matches('/');
    if base.is_empty() {
        return Err("请填写 API 地址");
    }
    if !(base.starts_with("https://") || base.starts_with("http://")) {
        return Err("API 地址必须以 https:// 或 http:// 开头");
    }
    if base
        .chars()
        .any(|ch| ch.is_whitespace() || matches!(ch, '#' | '?'))
        || base.split_once("://").is_some_and(|(_, authority)| {
            authority
                .split('/')
                .next()
                .is_some_and(|host| host.contains('@'))
        })
    {
        return Err("API 地址格式无效");
    }
    if base.ends_with("/models") {
        return Ok(base.to_string());
    }
    if let Some(prefix) = base.strip_suffix("/chat/completions") {
        return Ok(format!("{prefix}/models"));
    }
    if matches!(provider_id, "anthropic" | "glm-coding") && !base.ends_with("/v1") {
        return Ok(format!("{base}/v1/models"));
    }
    Ok(format!("{base}/models"))
}

fn connection_headers(provider_id: &str, credential: &str) -> Vec<(String, String)> {
    let mut headers = vec![("accept".to_string(), "application/json".to_string())];
    match provider_id {
        "anthropic" | "glm-coding" => {
            headers.push(("x-api-key".to_string(), credential.to_string()));
            headers.push(("anthropic-version".to_string(), "2023-06-01".to_string()));
        }
        "gemini" => headers.push(("x-goog-api-key".to_string(), credential.to_string())),
        _ => headers.push(("authorization".to_string(), format!("Bearer {credential}"))),
    }
    headers
}

fn failure(
    probe: &ProviderConnectionProbe,
    message: impl Into<String>,
    reachable: bool,
    authenticated: bool,
    http_status: Option<u16>,
    latency_ms: u64,
    used_stored_credential: bool,
) -> ClientEvent {
    ClientEvent::ProviderConnectionTested {
        operation_id: probe.operation_id,
        provider_id: probe.provider_id.clone(),
        connected: false,
        reachable,
        authenticated,
        model_available: false,
        http_status,
        latency_ms,
        message: message.into(),
        used_stored_credential,
    }
}

fn model_ids(body: &str) -> Option<Vec<String>> {
    let value: serde_json::Value = serde_json::from_str(body).ok()?;
    let entries = value
        .get("data")
        .or_else(|| value.get("models"))?
        .as_array()?;
    Some(
        entries
            .iter()
            .filter_map(|entry| {
                entry
                    .get("id")
                    .or_else(|| entry.get("name"))
                    .and_then(serde_json::Value::as_str)
                    .map(|id| id.strip_prefix("models/").unwrap_or(id).to_string())
            })
            .collect(),
    )
}

fn classify(
    response: Result<HttpResponse, HttpError>,
    probe: &ProviderConnectionProbe,
    latency_ms: u64,
    used_stored_credential: bool,
) -> ClientEvent {
    match response {
        Ok(response) if (200..300).contains(&response.status) => {
            let ids = model_ids(&response.body);
            let model_available = probe.model.trim().is_empty()
                || ids
                    .as_ref()
                    .is_some_and(|models| models.iter().any(|id| id == probe.model.trim()));
            match ids {
                Some(_) if !model_available => failure(
                    probe,
                    format!(
                        "连接与认证成功，但模型 `{}` 不在可用列表中",
                        probe.model.trim()
                    ),
                    true,
                    true,
                    Some(response.status),
                    latency_ms,
                    used_stored_credential,
                ),
                Some(_) => ClientEvent::ProviderConnectionTested {
                    operation_id: probe.operation_id,
                    provider_id: probe.provider_id.clone(),
                    connected: true,
                    reachable: true,
                    authenticated: true,
                    model_available: true,
                    http_status: Some(response.status),
                    latency_ms,
                    message: format!("连接成功 · {latency_ms} ms"),
                    used_stored_credential,
                },
                None => ClientEvent::ProviderConnectionTested {
                    operation_id: probe.operation_id,
                    provider_id: probe.provider_id.clone(),
                    connected: true,
                    reachable: true,
                    authenticated: true,
                    model_available: false,
                    http_status: Some(response.status),
                    latency_ms,
                    message: format!("连接与认证成功 · {latency_ms} ms（未能校验模型列表）"),
                    used_stored_credential,
                },
            }
        }
        Ok(response) => status_failure(probe, response.status, latency_ms, used_stored_credential),
        Err(HttpError::Status { status, .. }) => {
            status_failure(probe, status, latency_ms, used_stored_credential)
        }
        Err(HttpError::Timeout(_)) => failure(
            probe,
            "连接超时，请检查网络或 API 地址",
            false,
            false,
            None,
            latency_ms,
            used_stored_credential,
        ),
        Err(HttpError::Connection(_)) => failure(
            probe,
            "无法连接服务，请检查网络、DNS、TLS 或 API 地址",
            false,
            false,
            None,
            latency_ms,
            used_stored_credential,
        ),
        Err(HttpError::InvalidRequest(_)) => failure(
            probe,
            "API 地址或请求配置无效",
            false,
            false,
            None,
            latency_ms,
            used_stored_credential,
        ),
        Err(HttpError::InvalidResponse(_)) => failure(
            probe,
            "服务响应格式无效",
            true,
            false,
            None,
            latency_ms,
            used_stored_credential,
        ),
        Err(HttpError::Cancelled) => failure(
            probe,
            "连接测试已取消",
            false,
            false,
            None,
            latency_ms,
            used_stored_credential,
        ),
    }
}

fn status_failure(
    probe: &ProviderConnectionProbe,
    status: u16,
    latency_ms: u64,
    used_stored_credential: bool,
) -> ClientEvent {
    let (message, authenticated) = match status {
        400 | 422 => ("服务可达，但请求格式不受支持", false),
        401 => ("认证失败，请检查 API Key", false),
        402 => ("认证成功，但账户余额不足", true),
        403 => ("服务拒绝访问，请检查 Key 权限", false),
        404 => ("服务可达，但模型列表端点不存在；请检查 API 地址", false),
        429 => ("服务可达，但请求频率已达上限，请稍后重试", false),
        500..=599 => ("Provider 服务暂时不可用，请稍后重试", false),
        _ => ("Provider 返回了无法识别的响应", false),
    };
    failure(
        probe,
        message,
        true,
        authenticated,
        Some(status),
        latency_ms,
        used_stored_credential,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn probe(provider_id: &str, api_base: &str, model: &str) -> ProviderConnectionProbe {
        ProviderConnectionProbe {
            operation_id: 7,
            provider_id: provider_id.to_string(),
            api_base: api_base.to_string(),
            model: model.to_string(),
            credential_override: None,
        }
    }

    #[test]
    fn builds_protocol_specific_model_endpoints() {
        assert_eq!(
            models_endpoint("https://api.anthropic.com", "anthropic"),
            Ok("https://api.anthropic.com/v1/models".to_string())
        );
        assert_eq!(
            models_endpoint("https://generativelanguage.googleapis.com/v1beta", "gemini"),
            Ok("https://generativelanguage.googleapis.com/v1beta/models".to_string())
        );
        assert!(models_endpoint("file:///tmp/provider", "openai").is_err());
    }

    #[test]
    fn recognizes_openai_and_gemini_model_lists() {
        assert_eq!(
            model_ids(r#"{"data":[{"id":"gpt-5"}]}"#),
            Some(vec!["gpt-5".to_string()])
        );
        assert_eq!(
            model_ids(r#"{"models":[{"name":"models/gemini-flash"}]}"#),
            Some(vec!["gemini-flash".to_string()])
        );
    }

    #[test]
    fn successful_catalog_response_reports_model_availability() {
        let event = classify(
            Ok(HttpResponse {
                status: 200,
                headers: Vec::new(),
                body: r#"{"data":[{"id":"deepseek-flash"}]}"#.to_string(),
                body_bytes: Vec::new(),
            }),
            &probe("deepseek", "https://api.deepseek.com", "deepseek-flash"),
            24,
            true,
        );
        assert!(matches!(
            event,
            ClientEvent::ProviderConnectionTested {
                connected: true,
                model_available: true,
                latency_ms: 24,
                used_stored_credential: true,
                ..
            }
        ));
    }

    #[test]
    fn auth_failure_is_log_safe_and_actionable() {
        let event = classify(
            Err(HttpError::Status {
                status: 401,
                body: "secret response".to_string(),
            }),
            &probe("openai", "https://api.openai.com/v1", "gpt-5"),
            9,
            false,
        );
        assert!(matches!(
            event,
            ClientEvent::ProviderConnectionTested {
                connected: false,
                reachable: true,
                authenticated: false,
                http_status: Some(401),
                used_stored_credential: false,
                ..
            }
        ));
        let serialized = serde_json::to_string(&event).expect("serialize event");
        assert!(!serialized.contains("secret response"));
    }
}
