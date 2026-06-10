use std::collections::{BTreeMap, VecDeque};
use std::sync::Arc;

use crate::{
    validate_capabilities, ApiKeyAuthenticator, AuthStrategy, Authenticator, BearerAuthenticator,
    ClientConfig, Credential, CredentialConfig, CredentialProvider, CredentialScope,
    EnvCredentialProvider, FrameStream, LlmError, LlmEvent, LlmRequest, LlmResponse,
    ModelListing, ModelRegistry, ProtocolFamily, ProviderId, ProviderRequest, ProviderResponse,
    Route, StreamDecoder, StreamingResponse, Transport, WireCodec,
};

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
            routes.insert(
                provider.profile_name,
                RouteEntry {
                    codec,
                    protocol: provider.protocol,
                    provider_id: provider.provider_id,
                    auth: provider.auth,
                    credential: provider.credential,
                    base_url,
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
            .authenticate(entry, &resolved_route.profile_name, provider_request)
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

    async fn authenticate(
        &self,
        entry: &RouteEntry,
        profile_name: &str,
        request: ProviderRequest,
    ) -> Result<ProviderRequest, LlmError> {
        let authenticator: Box<dyn Authenticator> = match entry.auth {
            AuthStrategy::None => return Ok(request),
            AuthStrategy::AwsSigV4 | AuthStrategy::GcpToken | AuthStrategy::AzureToken => {
                return Err(LlmError::InvalidRequest {
                    message: format!(
                        "provider profile '{profile_name}' uses auth strategy {:?}, which has no authenticator yet",
                        entry.auth
                    ),
                });
            }
            AuthStrategy::ApiKey | AuthStrategy::Bearer | AuthStrategy::OAuthBearer => {
                let Some(secret) = self.load_secret(entry, profile_name).await? else {
                    // CredentialConfig::None: the host opted out of
                    // client-applied authentication for this profile.
                    return Ok(request);
                };
                match (&entry.auth, &entry.protocol) {
                    (AuthStrategy::ApiKey, ProtocolFamily::AnthropicMessages) => {
                        Box::new(ApiKeyAuthenticator::new(secret))
                    }
                    (AuthStrategy::ApiKey, ProtocolFamily::GeminiGenerateContent) => {
                        Box::new(ApiKeyAuthenticator::with_header_name("x-goog-api-key", secret))
                    }
                    // OpenAI-style APIs send api keys as bearer tokens.
                    _ => Box::new(BearerAuthenticator::new(secret)),
                }
            }
        };

        let mut request = authenticator.apply(request)?;
        // Anthropic accepts OAuth bearer tokens only with the oauth beta flag.
        if matches!(entry.auth, AuthStrategy::OAuthBearer)
            && matches!(entry.protocol, ProtocolFamily::AnthropicMessages)
        {
            request
                .headers
                .insert("anthropic-beta".to_string(), "oauth-2025-04-20".to_string());
        }
        Ok(request)
    }

    async fn load_secret(
        &self,
        entry: &RouteEntry,
        profile_name: &str,
    ) -> Result<Option<String>, LlmError> {
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

        let (Credential::ApiKey(secret) | Credential::BearerToken(secret)) = credential;
        Ok(Some(secret))
    }
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
        crate::ProtocolFamily::OpenAiResponses
        | crate::ProtocolFamily::VertexGemini
        | crate::ProtocolFamily::VertexClaude
        | crate::ProtocolFamily::BedrockClaude
        | crate::ProtocolFamily::AzureOpenAi => Err(LlmError::InvalidRequest {
            message: format!(
                "provider profile '{}' uses protocol family {:?}, which has no codec yet",
                provider.profile_name, provider.protocol
            ),
        }),
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
