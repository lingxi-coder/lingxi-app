use std::collections::{BTreeMap, VecDeque};
use std::sync::Arc;
use std::time::Duration;

use crate::{
    validate_capabilities, ApiKeyAuthenticator, AuthStrategy, Authenticator, BearerAuthenticator,
    ClientConfig, CopilotAuthenticator, Credential, CredentialConfig, CredentialProvider, CredentialScope,
    EnvCredentialProvider, FrameStream, LlmError, LlmEvent, LlmRequest, LlmResponse,
    ModelListing, ModelRegistry, ProtocolFamily, ProviderId, ProviderRequest, ProviderResponse,
    Route, StreamDecoder, StreamingResponse, Transport, WireCodec,
};
use crate::sigv4;

/// Anthropic Messages API version sent by codecs this client constructs.
const ANTHROPIC_VERSION: &str = "2023-06-01";

#[derive(Debug)]
pub struct DefaultLlmClient {
    registry: ModelRegistry,
    routes: BTreeMap<String, RouteEntry>,
    credentials: Option<Arc<dyn CredentialProvider>>,
}

#[derive(Debug)]
struct RouteEntry {
    codec: Box<dyn WireCodec>,
    protocol: ProtocolFamily,
    provider_id: ProviderId,
    auth: AuthStrategy,
    credential: CredentialConfig,
    base_url: String,
    signing: Option<crate::SigningConfig>,
}

/// Polling knobs for [`DefaultLlmClient::wait_for_file_active`]. The defaults
/// (2s interval, 300s budget) are this crate's own convenience choice — there
/// is no claude-code/codex counterpart to pin against; tune per call site.
#[derive(Debug, Clone, Copy)]
pub struct FileActivationPoll {
    /// Delay between consecutive status requests.
    pub interval: Duration,
    /// Total budget before giving up with [`LlmError::Transport`].
    pub max_wait: Duration,
}

impl Default for FileActivationPoll {
    fn default() -> Self {
        Self {
            interval: Duration::from_secs(2),
            max_wait: Duration::from_secs(300),
        }
    }
}

impl DefaultLlmClient {
    pub fn from_config(config: ClientConfig) -> Result<Self, LlmError> {
        let registry = ModelRegistry::from_config(config.clone())?;
        let mut routes = BTreeMap::new();

        for provider in config.providers {
            if routes.contains_key(&provider.profile_name) {
                return Err(LlmError::InvalidRequest {
                    message: format!("duplicate provider profile_name: {}", provider.profile_name),
                });
            }
            let codec = build_codec(&provider)?;
            let base_url = provider.base_url.clone();
            let signing = provider.signing.clone();
            routes.insert(
                provider.profile_name,
                RouteEntry {
                    codec,
                    protocol: provider.protocol,
                    provider_id: provider.provider_id,
                    auth: provider.auth,
                    credential: provider.credential,
                    base_url,
                    signing,
                },
            );
        }

        Ok(Self {
            registry,
            routes,
            credentials: None,
        })
    }

    /// Attach a host-managed credential store consulted for
    /// `CredentialConfig::Static` and `CredentialConfig::HostManaged` ids.
    #[must_use]
    pub fn with_credential_provider(mut self, provider: Arc<dyn CredentialProvider>) -> Self {
        self.credentials = Some(provider);
        self
    }

    #[must_use]
    pub fn available_models(&self) -> Vec<ModelListing> {
        self.registry.available_models()
    }

    /// Resolve, validate, encode, and authenticate a request.
    ///
    /// The returned provider request may carry credentials in its headers;
    /// redact with [`crate::Redactor`] before logging.
    pub async fn prepare(&self, request: &LlmRequest) -> Result<PreparedLlmCall, LlmError> {
        use std::time::SystemTime;
        self.prepare_at(request, SystemTime::now()).await
    }

    /// Clock-injectable variant of [`prepare`] used by tests to pin the `SigV4`
    /// timestamp and produce a deterministic `Authorization` header.
    ///
    /// Production code uses [`prepare`] which passes `SystemTime::now()`.
    pub async fn prepare_at(
        &self,
        request: &LlmRequest,
        now: std::time::SystemTime,
    ) -> Result<PreparedLlmCall, LlmError> {
        let resolved_route = self.registry.resolve(&request.model)?;
        validate_capabilities(request, resolved_route.capabilities)?;

        let entry = self
            .routes
            .get(&resolved_route.profile_name)
            .ok_or(LlmError::ModelUnavailable)?;

        let provider_request = if request.model == resolved_route.request_model {
            entry.codec.encode_request(request)?
        } else {
            let mut routed_request = request.clone();
            routed_request.model.clone_from(&resolved_route.request_model);
            entry.codec.encode_request(&routed_request)?
        };
        let provider_request = self
            .authenticate_at(entry, &resolved_route.profile_name, provider_request, now)
            .await?;

        Ok(PreparedLlmCall {
            route: Route {
                resolved_route,
                codec: entry.codec.clone(),
            },
            provider_request,
        })
    }

    /// Execute a non-streaming call over `transport`.
    ///
    /// Prepares (resolve, validate, encode, authenticate), sends, and decodes
    /// the response through the status-aware codec error taxonomy.
    pub async fn execute(
        &self,
        request: &LlmRequest,
        transport: &dyn Transport,
    ) -> Result<LlmResponse, LlmError> {
        let prepared = self.prepare(request).await?;
        let provider_response = transport.execute(&prepared.provider_request).await?;
        prepared.route.codec.decode_response(provider_response)
    }

    /// Execute a streaming call over `transport`.
    ///
    /// The request must set `stream: true`. Error statuses are drained and
    /// routed through the same codec error taxonomy as non-streaming calls.
    pub async fn execute_stream(
        &self,
        request: &LlmRequest,
        transport: &dyn Transport,
    ) -> Result<LlmEventStream, LlmError> {
        if !request.stream {
            return Err(LlmError::InvalidRequest {
                message: "execute_stream requires LlmRequest.stream = true".to_string(),
            });
        }

        let prepared = self.prepare(request).await?;
        let streaming = transport.open_stream(&prepared.provider_request).await?;

        if streaming.status >= 400 {
            return Err(decode_stream_error(&prepared, streaming).await);
        }

        Ok(LlmEventStream::new(
            prepared.route.codec.stream_decoder(),
            streaming.frames,
        ))
    }

    /// Resolve, validate, encode, and authenticate an Anthropic
    /// `count_tokens` call. Errors on non-Anthropic routes.
    pub async fn prepare_count_tokens(
        &self,
        request: &LlmRequest,
    ) -> Result<ProviderRequest, LlmError> {
        let resolved_route = self.registry.resolve(&request.model)?;
        validate_capabilities(request, resolved_route.capabilities)?;

        let entry = self
            .routes
            .get(&resolved_route.profile_name)
            .ok_or(LlmError::ModelUnavailable)?;
        if !matches!(entry.protocol, ProtocolFamily::AnthropicMessages) {
            return Err(LlmError::InvalidRequest {
                message: format!(
                    "count_tokens is only available on AnthropicMessages routes, not {:?}",
                    entry.protocol
                ),
            });
        }

        let codec = crate::AnthropicMessagesCodec::new(&entry.base_url, ANTHROPIC_VERSION);
        let mut routed_request = request.clone();
        routed_request.model.clone_from(&resolved_route.request_model);
        let provider_request = codec.encode_count_tokens_request(&routed_request)?;
        self.authenticate(entry, &resolved_route.profile_name, provider_request)
            .await
    }

    /// Upload raw media bytes via the Gemini File API resumable protocol.
    ///
    /// Resolves `model_or_alias` exactly like [`prepare`](Self::prepare) and
    /// requires the resolved profile's protocol family to be EXACTLY
    /// [`ProtocolFamily::GeminiGenerateContent`] — Vertex Gemini does not use
    /// the File API (media goes through GCS URIs there), so `VertexGemini`
    /// routes are rejected with [`LlmError::InvalidRequest`].
    ///
    /// Two-leg flow, both legs authenticated through the same path as
    /// `prepare` (`x-goog-api-key` for `ApiKey`-auth Gemini profiles):
    ///
    /// 1. START — `POST {upload_base}/upload/v1beta/files` with metadata; the
    ///    response's `x-goog-upload-url` header is the session URL.
    /// 2. UPLOAD+FINALIZE — `POST` the raw `bytes` (via
    ///    `ProviderRequest::body_bytes`) to that session URL.
    ///
    /// Non-2xx responses on either leg map through the shared Gemini error
    /// taxonomy. The returned [`crate::GeminiFile`] is NOT polled here:
    /// callers must poll
    /// [`crate::providers::gemini_files::file_status_request`] until
    /// `state == "ACTIVE"` for video/PDF uploads (or use the
    /// [`Self::wait_for_file_active`] convenience); images are typically
    /// `ACTIVE` immediately. A `FAILED` state passes through as data, not an
    /// error. The resulting `uri` plugs into
    /// [`crate::ContentBlock::ImageUrl`], which the Gemini codec encodes as a
    /// `file_data.file_uri` part.
    pub async fn upload_file(
        &self,
        model_or_alias: &str,
        bytes: Vec<u8>,
        mime_type: &str,
        display_name: &str,
        transport: &dyn Transport,
    ) -> Result<crate::GeminiFile, LlmError> {
        use crate::providers::gemini_files;

        let resolved_route = self.registry.resolve(model_or_alias)?;
        let entry = self
            .routes
            .get(&resolved_route.profile_name)
            .ok_or(LlmError::ModelUnavailable)?;
        if !matches!(entry.protocol, ProtocolFamily::GeminiGenerateContent) {
            return Err(LlmError::InvalidRequest {
                message: "file upload requires a gemini provider profile".to_string(),
            });
        }

        // Leg 1: START — metadata only; returns the resumable session URL.
        let start = gemini_files::start_upload_request(
            &entry.base_url,
            bytes.len(),
            mime_type,
            display_name,
        );
        let start = self
            .authenticate(entry, &resolved_route.profile_name, start)
            .await?;
        let start_response = transport.execute(&start).await?;
        if start_response.status >= 400 {
            return Err(gemini_files::decode_upload_error(&start_response));
        }
        let upload_url = gemini_files::parse_start_response(&start_response.headers)?;

        // Leg 2: UPLOAD+FINALIZE — raw bytes to the session URL.
        let upload = gemini_files::upload_finalize_request(&upload_url, bytes);
        let upload = self
            .authenticate(entry, &resolved_route.profile_name, upload)
            .await?;
        let upload_response = transport.execute(&upload).await?;
        if upload_response.status >= 400 {
            return Err(gemini_files::decode_upload_error(&upload_response));
        }
        gemini_files::parse_upload_response(&upload_response.body_json)
    }

    /// Poll the Gemini File API until `file_name` leaves `PROCESSING`.
    ///
    /// Convenience layer over [`Self::upload_file`]'s "callers poll
    /// `file_status_request` until `ACTIVE`" contract: same
    /// Gemini-family-only guard, same authenticated request path. Returns
    /// the final [`crate::GeminiFile`] on `ACTIVE`; `FAILED` →
    /// [`LlmError::InvalidRequest`]; budget exhausted →
    /// [`LlmError::Transport`]. State strings are matched verbatim (tolerant
    /// decoder convention — any unknown state keeps polling until the budget
    /// runs out). Cadence comes from [`FileActivationPoll`]; its defaults are
    /// this crate's own choice (no upstream counterpart to pin against).
    pub async fn wait_for_file_active(
        &self,
        model_or_alias: &str,
        file_name: &str,
        transport: &dyn Transport,
        poll: FileActivationPoll,
    ) -> Result<crate::GeminiFile, LlmError> {
        use crate::providers::gemini_files;

        // Same resolution + family guard as `upload_file`.
        let resolved_route = self.registry.resolve(model_or_alias)?;
        let entry = self
            .routes
            .get(&resolved_route.profile_name)
            .ok_or(LlmError::ModelUnavailable)?;
        if !matches!(entry.protocol, ProtocolFamily::GeminiGenerateContent) {
            return Err(LlmError::InvalidRequest {
                message: "file upload requires a gemini provider profile".to_string(),
            });
        }

        let deadline = tokio::time::Instant::now() + poll.max_wait;
        loop {
            let status = gemini_files::file_status_request(&entry.base_url, file_name);
            let status = self
                .authenticate(entry, &resolved_route.profile_name, status)
                .await?;
            let response = transport.execute(&status).await?;
            if response.status >= 400 {
                return Err(gemini_files::decode_upload_error(&response));
            }
            let file = gemini_files::parse_file_status(&response.body_json)?;

            match file.state.as_str() {
                "ACTIVE" => return Ok(file),
                "FAILED" => {
                    return Err(LlmError::InvalidRequest {
                        message: format!("gemini file processing failed: {file_name}"),
                    })
                }
                _ => {}
            }
            if tokio::time::Instant::now() + poll.interval > deadline {
                return Err(LlmError::Transport {
                    message: format!(
                        "gemini file did not become ACTIVE within {}s",
                        poll.max_wait.as_secs()
                    ),
                });
            }
            tokio::time::sleep(poll.interval).await;
        }
    }

    // Each auth strategy is a self-contained arm; the length is necessary.
    #[allow(clippy::too_many_lines)]
    async fn authenticate(
        &self,
        entry: &RouteEntry,
        profile_name: &str,
        request: ProviderRequest,
    ) -> Result<ProviderRequest, LlmError> {
        use std::time::SystemTime;
        self.authenticate_at(entry, profile_name, request, SystemTime::now()).await
    }

    /// Injectable-clock variant used by tests to pin the `SigV4` timestamp.
    ///
    /// Production code calls `authenticate` which passes `SystemTime::now()`.
    /// Called from `prepare_at` (public) so that integration tests can use it
    /// without needing access to the private `RouteEntry` type.
    #[allow(clippy::too_many_lines)]
    async fn authenticate_at(
        &self,
        entry: &RouteEntry,
        profile_name: &str,
        mut request: ProviderRequest,
        now: std::time::SystemTime,
    ) -> Result<ProviderRequest, LlmError> {
        match entry.auth {
            AuthStrategy::None => return Ok(request),

            // ── AWS SigV4 ────────────────────────────────────────────────────
            // Credential::AwsSigV4 must be loaded via StaticCredentialProvider
            // or a host-managed store; the single-env-var Env path cannot
            // express three fields (access key, secret key, session token).
            AuthStrategy::AwsSigV4 => {
                // Require signing config (region + service).
                let signing = entry.signing.as_ref().ok_or_else(|| LlmError::InvalidRequest {
                    message: format!(
                        "provider profile '{profile_name}' uses AwsSigV4 but has no signing config; \
                         set ProviderProfile.signing = Some(SigningConfig {{ region, service }})"
                    ),
                })?;

                // Load Credential::AwsSigV4 from the credential store.
                let credential = self.load_credential(entry, profile_name).await?;
                let (access_key_id, secret_access_key, session_token) = match credential {
                    Some(Credential::AwsSigV4 { access_key_id, secret_access_key, session_token }) => {
                        (access_key_id, secret_access_key, session_token)
                    }
                    Some(other) => {
                        return Err(LlmError::InvalidRequest {
                            message: format!(
                                "provider profile '{profile_name}': AwsSigV4 auth requires \
                                 Credential::AwsSigV4 but got {other:?}",
                            ),
                        });
                    }
                    None => {
                        // CredentialConfig::None — host opted out of signing.
                        return Ok(request);
                    }
                };

                // Body bytes: sign exactly what LlmTransportBridge sends on the wire.
                // LlmTransportBridge calls `body_json.to_string()` unconditionally —
                // Value::Null serialises to the 4-byte string "null", not an empty body.
                // Signing empty bytes for Null would diverge from the wire payload → 403.
                let body_bytes = request.body_json.to_string().into_bytes();

                // Use the injected clock (now) for the timestamp.
                // Production code passes SystemTime::now(); tests pass a fixed instant.
                let datetime = {
                    use std::time::UNIX_EPOCH;
                    let secs = now
                        .duration_since(UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs();
                    // Format as YYYYMMDDTHHMMSSZ from Unix seconds.
                    let (year, month, day, hour, min, sec) = secs_to_ymdhms(secs);
                    format!("{year:04}{month:02}{day:02}T{hour:02}{min:02}{sec:02}Z")
                };

                let signed = sigv4::sign_request(
                    &request.method,
                    &request.url,
                    &request.headers,
                    &body_bytes,
                    &access_key_id,
                    &secret_access_key,
                    session_token.as_deref(),
                    &signing.region,
                    &signing.service,
                    &datetime,
                )
                .map_err(|e| LlmError::InvalidRequest { message: e })?;

                request.headers.insert("x-amz-date".to_string(), signed.x_amz_date);
                request.headers.insert("x-amz-content-sha256".to_string(), signed.x_amz_content_sha256);
                if let Some(token) = signed.x_amz_security_token {
                    request.headers.insert("x-amz-security-token".to_string(), token);
                }
                request.headers.insert("Authorization".to_string(), signed.authorization);
                return Ok(request);
            }

            // ── GCP Token (Bearer token) ─────────────────────────────────────
            // Vertex AI / GCP services use OAuth2 bearer tokens.
            // Reuse BearerAuthenticator — same wire shape as OpenAI bearer.
            AuthStrategy::GcpToken => {
                let Some(secret) = self.load_secret(entry, profile_name).await? else {
                    return Ok(request);
                };
                let authenticator = BearerAuthenticator::new(secret);
                return authenticator.apply(request);
            }

            // ── Azure Token (api-key header) ─────────────────────────────────
            // Azure OpenAI uses `api-key: <key>` rather than `Authorization:
            // Bearer ...`. The key may come from Credential::ApiKey or
            // Credential::BearerToken (host-managed stores may use either).
            //
            // Reference:
            // https://learn.microsoft.com/en-us/azure/ai-services/openai/reference
            AuthStrategy::AzureToken => {
                let Some(secret) = self.load_secret(entry, profile_name).await? else {
                    return Ok(request);
                };
                let authenticator = ApiKeyAuthenticator::with_header_name("api-key", secret);
                return authenticator.apply(request);
            }

            // ── Standard key / bearer auth ───────────────────────────────────
            AuthStrategy::ApiKey
            | AuthStrategy::Bearer
            | AuthStrategy::OAuthBearer
            | AuthStrategy::CopilotBearer => {
                let Some(secret) = self.load_secret(entry, profile_name).await? else {
                    // CredentialConfig::None: the host opted out of
                    // client-applied authentication for this profile.
                    return Ok(request);
                };
                let authenticator: Box<dyn Authenticator> = match (&entry.auth, &entry.protocol) {
                    // GitHub Copilot: GitHub OAuth token used directly as the
                    // bearer plus the Copilot header set (also strips x-api-key).
                    (AuthStrategy::CopilotBearer, _) => {
                        Box::new(CopilotAuthenticator::new(secret))
                    }
                    (AuthStrategy::ApiKey, ProtocolFamily::AnthropicMessages) => {
                        Box::new(ApiKeyAuthenticator::new(secret))
                    }
                    (AuthStrategy::ApiKey, ProtocolFamily::GeminiGenerateContent) => {
                        Box::new(ApiKeyAuthenticator::with_header_name("x-goog-api-key", secret))
                    }
                    // OpenAI-style APIs send api keys as bearer tokens.
                    _ => Box::new(BearerAuthenticator::new(secret)),
                };
                request = authenticator.apply(request)?;
            }
        }

        // Anthropic accepts OAuth bearer tokens only with the oauth beta flag.
        //
        // Parity: `constants/oauth.ts:36` OAUTH_BETA_HEADER = 'oauth-2025-04-20';
        // `utils/betas.ts:251-252` appends it via push() (list element, not clobber).
        // Use append_beta so that any betas already present (e.g., set by the
        // orchestrator's betas assembler before authenticate runs) are preserved.
        if matches!(entry.auth, AuthStrategy::OAuthBearer)
            && matches!(entry.protocol, ProtocolFamily::AnthropicMessages)
        {
            let existing = request.headers.get("anthropic-beta").map(String::as_str);
            let value = append_beta(existing, "oauth-2025-04-20");
            request
                .headers
                .insert("anthropic-beta".to_string(), value);
        }
        Ok(request)
    }

    /// Load credential material for `entry`, returning `None` when the
    /// credential config is `None` (host opted out of client auth).
    async fn load_credential(
        &self,
        entry: &RouteEntry,
        profile_name: &str,
    ) -> Result<Option<Credential>, LlmError> {
        let credential = match &entry.credential {
            CredentialConfig::None => return Ok(None),
            CredentialConfig::Env { var } => EnvCredentialProvider::new(var.clone())
                .load(&CredentialScope::new(entry.provider_id.clone(), profile_name))
                .await?,
            CredentialConfig::Static { id } | CredentialConfig::HostManaged { id } => {
                self.credentials
                    .as_ref()
                    .ok_or(LlmError::Authentication)?
                    .load(
                        &CredentialScope::new(entry.provider_id.clone(), profile_name)
                            .with_credential_id(id.clone()),
                    )
                    .await?
            }
        };
        Ok(Some(credential))
    }

    /// Load a plain string secret for key/bearer auth strategies.
    ///
    /// Returns `None` when `CredentialConfig::None` was set (host opted out).
    /// Returns `Err` when the credential is `AwsSigV4` (use `load_credential`
    /// instead for that strategy).
    async fn load_secret(
        &self,
        entry: &RouteEntry,
        profile_name: &str,
    ) -> Result<Option<String>, LlmError> {
        let Some(credential) = self.load_credential(entry, profile_name).await? else {
            return Ok(None);
        };

        match credential {
            Credential::ApiKey(secret) | Credential::BearerToken(secret) => Ok(Some(secret)),
            // AwsSigV4 creds are three-field structs; callers that need them
            // must use load_credential() directly.
            Credential::AwsSigV4 { .. } => Err(LlmError::InvalidRequest {
                message: format!(
                    "provider profile '{profile_name}': AwsSigV4 credentials must be loaded \
                     via load_credential(), not load_secret()"
                ),
            }),
            // ChatGptOAuth creds are multi-field structs; callers that need them
            // must use load_credential() directly.
            Credential::ChatGptOAuth { .. } => Err(LlmError::InvalidRequest {
                message: format!(
                    "provider profile '{profile_name}': ChatGptOAuth credentials must be loaded \
                     via load_credential(), not load_secret()"
                ),
            }),
        }
    }
}

/// Convert Unix epoch seconds to `(year, month, day, hour, minute, second)`.
///
/// Used to build the `YYYYMMDDTHHMMSSZ` timestamp for `SigV4` signing without
/// depending on `chrono` or `time` crates.
///
/// Algorithm: Howard Hinnant's `civil_from_days`
/// (<http://howardhinnant.github.io/date_algorithms.html>).
// The algorithm uses single-char variable names from the reference paper, large
// integer constants, signed/unsigned conversions, and boolean-to-int patterns
// that are idiomatic there but trigger multiple clippy lints.  Suppress them at
// the function level to keep the code aligned with the reference.
#[allow(
    clippy::many_single_char_names,
    clippy::similar_names,
    clippy::unreadable_literal,
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss,
    clippy::bool_to_int_with_if
)]
pub(crate) fn secs_to_ymdhms(secs: u64) -> (u32, u32, u32, u32, u32, u32) {
    let days = secs / 86_400;
    let time = secs % 86_400;
    let hour = (time / 3600) as u32;
    let min = ((time % 3600) / 60) as u32;
    let sec = (time % 60) as u32;

    let z = days as i64 + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let yr = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = (yr + if month <= 2 { 1 } else { 0 }) as u32;
    (year, month, day, hour, min, sec)
}

fn build_codec(provider: &crate::ProviderProfile) -> Result<Box<dyn WireCodec>, LlmError> {
    match &provider.protocol {
        crate::ProtocolFamily::AnthropicMessages => Ok(Box::new(crate::AnthropicMessagesCodec::new(
            provider.base_url.clone(),
            ANTHROPIC_VERSION,
        ))),
        crate::ProtocolFamily::OpenAiChat => {
            Ok(Box::new(crate::OpenAiChatCodec::new(provider.base_url.clone())))
        }
        crate::ProtocolFamily::GeminiGenerateContent => {
            Ok(Box::new(crate::GeminiCodec::new(provider.base_url.clone())))
        }
        crate::ProtocolFamily::AzureOpenAi => {
            // Require the azure config block (api_version).
            let azure = provider.azure.as_ref().ok_or_else(|| LlmError::InvalidRequest {
                message: format!(
                    "provider profile '{}' uses AzureOpenAi but has no azure config; \
                     set ProviderProfile.azure = Some(AzureConfig {{ api_version: \"...\" }})",
                    provider.profile_name
                ),
            })?;
            Ok(Box::new(crate::AzureOpenAiCodec::new(
                provider.base_url.clone(),
                azure.api_version.clone(),
            )))
        }
        crate::ProtocolFamily::VertexClaude => {
            Ok(Box::new(crate::VertexClaudeCodec::new(provider.base_url.clone())))
        }
        crate::ProtocolFamily::VertexGemini => {
            Ok(Box::new(crate::VertexGeminiCodec::new(provider.base_url.clone())))
        }
        crate::ProtocolFamily::BedrockClaude => {
            Ok(Box::new(crate::BedrockClaudeCodec::new(provider.base_url.clone())))
        }
        crate::ProtocolFamily::OpenAiResponses => {
            Ok(Box::new(crate::OpenAiResponsesCodec::new(provider.base_url.clone())))
        }
    }
}

#[derive(Debug)]
pub struct PreparedLlmCall {
    pub route: Route,
    pub provider_request: ProviderRequest,
}

/// Drain an error-status stream and map it through the codec taxonomy.
async fn decode_stream_error(prepared: &PreparedLlmCall, streaming: StreamingResponse) -> LlmError {
    let mut frames = streaming.frames;
    let mut body = Vec::new();
    loop {
        match frames.next_frame().await {
            Ok(Some(frame)) => body.extend_from_slice(&frame.bytes),
            Ok(None) => break,
            Err(error) => return error,
        }
    }

    let body_json = serde_json::from_slice(&body).unwrap_or(serde_json::Value::Null);
    let response = ProviderResponse {
        status: streaming.status,
        headers: streaming.headers,
        body_json,
        request_id: None,
    };
    match prepared.route.codec.decode_response(response) {
        Err(error) => error,
        // decode_response rejects every status >= 400, so this arm is
        // unreachable for the statuses that route here.
        Ok(_) => LlmError::ProviderInternal,
    }
}

/// Pull-based stream of canonical events from a streaming call.
pub struct LlmEventStream {
    decoder: Box<dyn StreamDecoder>,
    frames: Box<dyn FrameStream>,
    queue: VecDeque<LlmEvent>,
    yielded_any: bool,
    finished: bool,
}

impl LlmEventStream {
    fn new(decoder: Box<dyn StreamDecoder>, frames: Box<dyn FrameStream>) -> Self {
        Self {
            decoder,
            frames,
            queue: VecDeque::new(),
            yielded_any: false,
            finished: false,
        }
    }

    /// Next canonical event; `Ok(None)` after the stream completes or
    /// following a terminal error.
    pub async fn next_event(&mut self) -> Result<Option<LlmEvent>, LlmError> {
        loop {
            if let Some(event) = self.queue.pop_front() {
                self.yielded_any = true;
                return Ok(Some(event));
            }
            if self.finished {
                return Ok(None);
            }

            match self.frames.next_frame().await {
                Ok(Some(frame)) => match self.decoder.decode_frame(frame) {
                    Ok(events) => self.queue.extend(events),
                    Err(error) => {
                        self.finished = true;
                        return Err(error);
                    }
                },
                Ok(None) => {
                    self.finished = true;
                    match self.decoder.finish() {
                        Ok(events) => self.queue.extend(events),
                        Err(error) => return Err(error),
                    }
                }
                Err(error) => {
                    self.finished = true;
                    // A drop after events were delivered is not safely
                    // retryable; upgrade so RetryPolicy will not replay it.
                    return Err(match error {
                        LlmError::Transport { message } if self.yielded_any => {
                            LlmError::StreamInterrupted { message }
                        }
                        other => other,
                    });
                }
            }
        }
    }
}

impl std::fmt::Debug for LlmEventStream {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("LlmEventStream")
            .field("queued_events", &self.queue.len())
            .field("yielded_any", &self.yielded_any)
            .field("finished", &self.finished)
            .finish_non_exhaustive()
    }
}

/// Append `beta` to the comma-joined `anthropic-beta` header value if it is not
/// already present.
///
/// - If `existing` is `None`, returns `beta.to_string()` (first entry).
/// - If `existing` already contains `beta` as a comma-separated segment (trimmed
///   match — handles `"a, b"` spacing that arises when headers are joined with
///   `", "`), the original value is returned unchanged.
/// - Otherwise `", beta"` is appended to `existing`.
///
/// **Passing `Some("")` is not expected and would yield a leading comma.**
///
/// **Parity:** mirrors `claude-code/src/utils/betas.ts:251-252` semantics where
/// `OAUTH_BETA_HEADER` is pushed into the beta list only when
/// `isClaudeAISubscriber()` is true, and the list is later joined — it is never
/// a standalone clobbering insert.
///
/// **`constants/oauth.ts:36`:** `OAUTH_BETA_HEADER = 'oauth-2025-04-20'` is the
/// beta value passed to this function by `authenticate()` for OAuth sessions.
#[must_use]
pub(crate) fn append_beta(existing: Option<&str>, beta: &str) -> String {
    match existing {
        None => beta.to_string(),
        Some(current) => {
            // Check whether `beta` is already a segment (trim to handle ", "-joined lists).
            if current.split(',').any(|seg| seg.trim() == beta) {
                current.to_string()
            } else {
                format!("{current},{beta}")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::append_beta;

    // ---- Task 2: `append_beta` pure-fn unit tests ----
    //
    // Reference: `claude-code/src/utils/betas.ts:251-252`:
    //   if (isClaudeAISubscriber()) { betaHeaders.push(OAUTH_BETA_HEADER) }
    // Reference: `claude-code/src/constants/oauth.ts:36`:
    //   export const OAUTH_BETA_HEADER = 'oauth-2025-04-20' as const

    #[test]
    fn append_beta_to_none_returns_beta_alone() {
        assert_eq!(append_beta(None, "oauth-2025-04-20"), "oauth-2025-04-20");
    }

    #[test]
    fn append_beta_to_existing_single_entry_comma_joins() {
        assert_eq!(
            append_beta(Some("claude-code-20250219"), "oauth-2025-04-20"),
            "claude-code-20250219,oauth-2025-04-20",
        );
    }

    #[test]
    fn append_beta_to_existing_multi_entry_appends_at_end() {
        assert_eq!(
            append_beta(Some("claude-code-20250219,interleaved-thinking-2025-05-14"), "oauth-2025-04-20"),
            "claude-code-20250219,interleaved-thinking-2025-05-14,oauth-2025-04-20",
        );
    }

    #[test]
    fn append_beta_does_not_duplicate_when_already_present() {
        // oauth-2025-04-20 is already in the list — must not be added again.
        assert_eq!(
            append_beta(Some("claude-code-20250219,oauth-2025-04-20"), "oauth-2025-04-20"),
            "claude-code-20250219,oauth-2025-04-20",
        );
    }

    #[test]
    fn append_beta_does_not_duplicate_when_only_entry() {
        assert_eq!(
            append_beta(Some("oauth-2025-04-20"), "oauth-2025-04-20"),
            "oauth-2025-04-20",
        );
    }

    /// Verify the `authenticate()` clobbering bug is exercised via `append_beta`:
    /// a pre-existing multi-beta header must not be overwritten when oauth is added.
    #[test]
    fn oauth_beta_does_not_clobber_existing_betas() {
        let existing = "claude-code-20250219,interleaved-thinking-2025-05-14";
        let result = append_beta(Some(existing), "oauth-2025-04-20");
        // Both pre-existing betas survive.
        assert!(result.split(',').any(|p| p == "claude-code-20250219"),
            "claude-code beta must survive; got: {result}");
        assert!(result.split(',').any(|p| p == "interleaved-thinking-2025-05-14"),
            "interleaved-thinking beta must survive; got: {result}");
        // And oauth is now present.
        assert!(result.split(',').any(|p| p == "oauth-2025-04-20"),
            "oauth beta must be present; got: {result}");
    }

    /// Dedup must work even when segments carry surrounding whitespace from a
    /// `", "`-joined list (e.g., `"a, oauth-2025-04-20"` — the space after
    /// the comma is present).  Passing the same beta again must be a no-op.
    #[test]
    fn append_beta_dedups_with_whitespace() {
        // "a, oauth-2025-04-20" — note the space after the comma.
        let result = append_beta(Some("a, oauth-2025-04-20"), "oauth-2025-04-20");
        assert_eq!(
            result, "a, oauth-2025-04-20",
            "dedup must fire even when the segment has leading whitespace; got: {result}",
        );
    }
}
