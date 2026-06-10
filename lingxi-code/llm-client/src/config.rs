//! Serde-friendly client configuration.

use crate::ProviderId;
use serde::{Deserialize, Serialize};

/// Raw client configuration supplied by hosts, files, or tests.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ClientConfig {
    /// Configured provider profiles.
    #[serde(default)]
    pub providers: Vec<ProviderProfile>,
}

/// Provider profile used to build one or more routes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProviderProfile {
    /// Explicit provider identity used after route resolution.
    pub provider_id: ProviderId,
    /// Human/config profile name.
    pub profile_name: String,
    /// Base URL for this provider profile.
    ///
    /// The expected shape differs by protocol family: `OpenAiChat` includes
    /// the version segment (`https://api.openai.com/v1`), `AnthropicMessages`
    /// is the bare origin (`https://api.anthropic.com`), and
    /// `GeminiGenerateContent` is the versioned root
    /// (`https://generativelanguage.googleapis.com/v1beta`).
    pub base_url: String,
    /// Wire protocol family used by this route.
    pub protocol: ProtocolFamily,
    /// Auth application strategy.
    pub auth: AuthStrategy,
    /// Credential lookup reference.
    pub credential: CredentialConfig,
    /// Supported models for this profile.
    #[serde(default)]
    pub models: Vec<ModelProfile>,
    /// Pricing behavior for this profile.
    #[serde(default)]
    pub pricing: PricingConfig,
}

/// Wire protocol route family.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProtocolFamily {
    /// Anthropic Messages API.
    AnthropicMessages,
    /// `OpenAI` Responses API.
    OpenAiResponses,
    /// `OpenAI` Chat Completions API.
    OpenAiChat,
    /// Gemini generateContent API.
    GeminiGenerateContent,
    /// Vertex Gemini route family.
    VertexGemini,
    /// Vertex Claude route family.
    VertexClaude,
    /// Bedrock Claude route family.
    BedrockClaude,
    /// Azure `OpenAI` route family.
    AzureOpenAi,
}

/// Authenticator strategy for a resolved route.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthStrategy {
    /// Provider API key header or query auth.
    ApiKey,
    /// Bearer-token auth.
    Bearer,
    /// OAuth bearer-token auth.
    OAuthBearer,
    /// AWS `SigV4` request signing.
    AwsSigV4,
    /// GCP bearer token auth.
    GcpToken,
    /// Azure bearer token auth.
    AzureToken,
    /// No auth.
    None,
}

/// Serializable credential reference.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "type")]
pub enum CredentialConfig {
    /// Load secret material from an environment variable.
    Env {
        /// Environment variable name.
        var: String,
    },
    /// Load static host-supplied secret by id.
    Static {
        /// Host-defined static credential id.
        id: String,
    },
    /// Ask the host-managed secret store for this id.
    HostManaged {
        /// Host-managed secret id.
        id: String,
    },
    /// No credential is loaded; requests for this profile are sent without
    /// client-applied authentication.
    None,
}

/// Model profile declared within a provider profile.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelProfile {
    /// Human-facing model label.
    pub display_model: String,
    /// Provider-local model value sent on the wire.
    pub request_model: String,
    /// Model id used by pricing lookup.
    pub billing_model: String,
    /// Alternate names accepted by registry resolution.
    #[serde(default)]
    pub aliases: Vec<String>,
    /// Model capabilities used for preflight validation.
    #[serde(default)]
    pub capabilities: Capabilities,
}

/// Capabilities advertised by a configured model route.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[allow(clippy::struct_excessive_bools)]
pub struct Capabilities {
    /// Whether streaming is supported.
    pub streaming: bool,
    /// Whether native tool calls are supported.
    pub tools: bool,
    /// Whether image input is supported.
    pub vision: bool,
    /// Whether document input is supported.
    pub documents: bool,
    /// Whether reasoning controls are supported.
    pub reasoning: bool,
    /// Whether structured-output controls are supported.
    pub structured_output: bool,
}

/// Pricing resolution behavior for a profile.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PricingConfig {
    /// Whether missing pricing must fail instead of returning unestimated cost.
    pub require_priced: bool,
}
