use std::collections::BTreeMap;
use std::sync::Arc;

use crate::{
    validate_capabilities, ApiKeyAuthenticator, AuthStrategy, Authenticator, BearerAuthenticator,
    ClientConfig, Credential, CredentialConfig, CredentialProvider, CredentialScope,
    EnvCredentialProvider, LlmError, LlmRequest, LlmResponse, ModelListing, ModelRegistry,
    ProtocolFamily, ProviderId, ProviderRequest, Route, Transport, WireCodec,
};

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
            routes.insert(
                provider.profile_name,
                RouteEntry {
                    codec,
                    protocol: provider.protocol,
                    provider_id: provider.provider_id,
                    auth: provider.auth,
                    credential: provider.credential,
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
    pub fn prepare(&self, request: &LlmRequest) -> Result<PreparedLlmCall, LlmError> {
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
        let provider_request =
            self.authenticate(entry, &resolved_route.profile_name, provider_request)?;

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
        let prepared = self.prepare(request)?;
        let provider_response = transport.execute(&prepared.provider_request).await?;
        prepared.route.codec.decode_response(provider_response)
    }

    fn authenticate(
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
                let Some(secret) = self.load_secret(entry, profile_name)? else {
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

    fn load_secret(
        &self,
        entry: &RouteEntry,
        profile_name: &str,
    ) -> Result<Option<String>, LlmError> {
        let credential = match &entry.credential {
            CredentialConfig::None => return Ok(None),
            CredentialConfig::Env { var } => EnvCredentialProvider::new(var.clone())
                .load(&CredentialScope::new(entry.provider_id.clone(), profile_name))?,
            CredentialConfig::Static { id } | CredentialConfig::HostManaged { id } => self
                .credentials
                .as_ref()
                .ok_or(LlmError::Authentication)?
                .load(
                    &CredentialScope::new(entry.provider_id.clone(), profile_name)
                        .with_credential_id(id.clone()),
                )?,
        };

        let (Credential::ApiKey(secret) | Credential::BearerToken(secret)) = credential;
        Ok(Some(secret))
    }
}

fn build_codec(provider: &crate::ProviderProfile) -> Result<Box<dyn WireCodec>, LlmError> {
    match &provider.protocol {
        crate::ProtocolFamily::AnthropicMessages => Ok(Box::new(crate::AnthropicMessagesCodec::new(
            provider.base_url.clone(),
            "2023-06-01",
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
