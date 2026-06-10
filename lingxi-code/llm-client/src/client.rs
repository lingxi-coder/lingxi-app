use std::collections::BTreeMap;

use crate::{
    validate_capabilities, ClientConfig, LlmError, LlmRequest, ModelListing, ModelRegistry,
    ProviderRequest, Route, WireCodec,
};

#[derive(Debug)]
pub struct DefaultLlmClient {
    pub registry: ModelRegistry,
    pub routes: BTreeMap<String, Box<dyn WireCodec>>,
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
            routes.insert(provider.profile_name, codec);
        }

        Ok(Self { registry, routes })
    }

    #[must_use]
    pub fn available_models(&self) -> Vec<ModelListing> {
        self.registry.available_models()
    }

    pub fn prepare(&self, request: &LlmRequest) -> Result<PreparedLlmCall, LlmError> {
        let resolved_route = self.registry.resolve(&request.model)?;
        validate_capabilities(request, resolved_route.capabilities)?;

        let codec = self
            .routes
            .get(&resolved_route.profile_name)
            .ok_or(LlmError::ModelUnavailable)?;

        let provider_request = if request.model == resolved_route.request_model {
            codec.encode_request(request)?
        } else {
            let mut routed_request = request.clone();
            routed_request.model.clone_from(&resolved_route.request_model);
            codec.encode_request(&routed_request)?
        };

        Ok(PreparedLlmCall {
            route: Route {
                resolved_route,
                codec: codec.clone(),
            },
            provider_request,
        })
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
