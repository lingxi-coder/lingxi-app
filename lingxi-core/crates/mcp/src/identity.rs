//! claude-code identity constants — DO NOT change without updating
//! `2026-05-23-m2-02b-mcp-client.md` §"Critical 1:1 fidelity items".

use serde::Serialize;

/// Wire-literal `clientInfo.name` value sent by claude-code in the MCP
/// `initialize` request (TS `services/mcp/client.ts:987`).
pub const CLIENT_NAME: &str = "claude-code";
/// Wire-literal `clientInfo.title` value sent by claude-code (TS line 988).
pub const CLIENT_TITLE: &str = "Claude Code";
/// Wire-literal `clientInfo.version` — sourced from this crate's `Cargo.toml`.
pub const CLIENT_VERSION: &str = env!("CARGO_PKG_VERSION");
/// Public-facing website URL emitted in `clientInfo.websiteUrl` (matches
/// claude-code). TODO: confirm against claude-code source before release —
/// placeholder uses the marketing landing page.
pub const MCP_WEBSITE_URL: &str = "https://claude.com/claude-code";

/// `clientInfo` payload sent during MCP `initialize`.
///
/// Wire shape: `{"name": "...", "title": "...", "version": "...", "websiteUrl": "..."}`.
/// The `websiteUrl` key is camelCase per claude-code; serde rename is
/// applied explicitly on that single field.
#[derive(Debug, Clone, Serialize)]
pub struct ClientInfo {
    /// Logical client name (= [`CLIENT_NAME`]).
    pub name: &'static str,
    /// Human-readable title (= [`CLIENT_TITLE`]).
    pub title: &'static str,
    /// Semver-compatible version string (= [`CLIENT_VERSION`]).
    pub version: &'static str,
    /// Public marketing/landing-page URL (= [`MCP_WEBSITE_URL`]).
    #[serde(rename = "websiteUrl")]
    pub website_url: &'static str,
}

impl Default for ClientInfo {
    fn default() -> Self {
        Self {
            name: CLIENT_NAME,
            title: CLIENT_TITLE,
            version: CLIENT_VERSION,
            website_url: MCP_WEBSITE_URL,
        }
    }
}

/// `const` constructor — kept alongside `Default::default()` so call sites
/// that prefer a `const`-context value (e.g. `static`-bound contexts) can
/// use it without depending on the runtime `Default` impl.
pub const CLIENT_INFO: ClientInfo = ClientInfo {
    name: CLIENT_NAME,
    title: CLIENT_TITLE,
    version: CLIENT_VERSION,
    website_url: MCP_WEBSITE_URL,
};
