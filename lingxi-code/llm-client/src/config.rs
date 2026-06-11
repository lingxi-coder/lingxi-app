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

/// AWS `SigV4` signing region + service for a provider profile.
///
/// Required when [`AuthStrategy::AwsSigV4`] is used. The region and service
/// are needed to build the credential scope string in the `Authorization`
/// header: `<date>/<region>/<service>/aws4_request`.
///
/// Example for Amazon Bedrock in us-east-1:
/// ```json
/// { "region": "us-east-1", "service": "bedrock" }
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SigningConfig {
    /// AWS region (e.g. `"us-east-1"`).
    pub region: String,
    /// AWS service name (e.g. `"bedrock"`, `"execute-api"`).
    pub service: String,
}

/// Azure `OpenAI` API-version configuration.
///
/// Required when [`ProtocolFamily::AzureOpenAi`] is used. The API version is
/// appended as a query parameter (`?api-version=<api_version>`) per the Azure
/// `OpenAI` REST specification:
/// <https://learn.microsoft.com/en-us/azure/ai-services/openai/reference>
///
/// Example:
/// ```json
/// { "apiVersion": "2024-02-01" }
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AzureConfig {
    /// Azure `OpenAI` API version string (e.g. `"2024-02-01"`).
    #[serde(rename = "apiVersion")]
    pub api_version: String,
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
    ///
    /// For `AzureOpenAi` the base URL should be the resource endpoint without
    /// the deployment segment, e.g.
    /// `https://<resource>.openai.azure.com`.  The codec appends
    /// `/openai/deployments/{model}/chat/completions?api-version=...`.
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
    /// AWS `SigV4` signing region + service.
    ///
    /// Required when `auth = AwsSigV4`.  Missing → `InvalidRequest` at auth
    /// time (naming the profile and field).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signing: Option<SigningConfig>,
    /// Azure `OpenAI` API-version configuration.
    ///
    /// Required when `protocol = AzureOpenAi`.  Missing → `InvalidRequest` at
    /// codec-build time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub azure: Option<AzureConfig>,
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
///
/// ## Per-model overrides
///
/// `overrides` is a list of `(model_id, TokenPricing)` pairs, where `model_id`
/// is the **display model** (the `id` key from the `models` array in settings).
/// At the host build step, each override is applied onto the `PricingCatalog`
/// keyed by the model's **billing model** (resolved via the profile's
/// [`ModelProfile`] table).
///
/// Serde round-trips the field; absent → empty vec (existing configs unaffected).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PricingConfig {
    /// Whether missing pricing must fail instead of returning unestimated cost.
    #[serde(default)]
    pub require_priced: bool,
    /// Per-model price overrides declared in the `providers.<name>.pricing` object.
    ///
    /// Each entry is `(display_model_id, TokenPricing)`.  The billing-model
    /// resolution and catalog insertion happen at the host build step, not at
    /// parse time.  Absent → empty (no overrides).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub overrides: Vec<(String, crate::cost::TokenPricing)>,
}
