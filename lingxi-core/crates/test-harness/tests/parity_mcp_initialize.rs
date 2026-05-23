//! Parity fixture: MCP `initialize` request wire identity.
//!
//! Locks the literal `clientInfo` and `capabilities` shape emitted by
//! `lingxi_mcp::InitializeParams::default()` against claude-code's
//! reference (`src/services/mcp/client.ts:985-1002`):
//!
//! - `clientInfo.name = "claude-code"` (literal),
//! - `clientInfo.title = "Claude Code"` (literal),
//! - `clientInfo.websiteUrl = "https://claude.com/claude-code"` (literal),
//! - `clientInfo.version = env!("CARGO_PKG_VERSION")` of `lingxi-mcp`,
//! - `capabilities.roots` and `capabilities.elicitation` are the LITERAL
//!   empty objects `{}` (NOT null, NOT missing). The Java MCP SDK rejects
//!   `{form:{},url:{}}` for elicitation, so we must emit a bare `{}`.

use lingxi_mcp::initialize_params::InitializeParams;
use lingxi_test_harness::parity::load_fixture;
use serde::Deserialize;
use serde_json::Value;

#[derive(Deserialize)]
#[allow(clippy::struct_field_names)] // mirrors the JSON fixture's `expected_*` keys
struct Fixture {
    expected_protocol_version: String,
    expected_client_info: Value,
    expected_capabilities_shape: Value,
}

#[test]
fn mcp_initialize_request_matches_claude_code_identity() {
    let fx: Fixture = load_fixture("mcp_initialize_request");
    let params = InitializeParams::default();
    let got = serde_json::to_value(&params).expect("params serializes");

    // protocolVersion is the literal MCP date claude-code locks against.
    assert_eq!(
        got["protocolVersion"], fx.expected_protocol_version,
        "protocolVersion must match claude-code reference",
    );

    // Identity literals.
    let want_info = &fx.expected_client_info;
    assert_eq!(got["clientInfo"]["name"], want_info["name"]);
    assert_eq!(got["clientInfo"]["title"], want_info["title"]);
    assert_eq!(got["clientInfo"]["websiteUrl"], want_info["websiteUrl"]);

    // Version: must equal the lingxi-mcp crate's CARGO_PKG_VERSION at
    // compile time. We don't bake the number into the fixture — it
    // changes every release — but we verify the field is present and
    // tracks the upstream crate.
    let got_version = got["clientInfo"]["version"]
        .as_str()
        .expect("version present");
    assert!(
        got_version.split('.').count() >= 3,
        "version must look semver-like, got {got_version:?}",
    );
    // Cross-check against the embedded constant. Drift here would mean
    // someone bypassed `Default::default()` and hand-rolled a struct.
    assert_eq!(
        got_version,
        lingxi_mcp::CLIENT_VERSION,
        "version must come from CARGO_PKG_VERSION via CLIENT_VERSION",
    );

    // Capabilities: roots and elicitation are LITERALLY empty objects.
    let caps = &got["capabilities"];
    assert!(caps.is_object(), "capabilities must be an object");
    assert_eq!(
        caps["roots"], fx.expected_capabilities_shape["roots"],
        "capabilities.roots must be the literal empty object (not null, not missing)",
    );
    assert_eq!(
        caps["elicitation"],
        fx.expected_capabilities_shape["elicitation"],
        "capabilities.elicitation must be the literal empty object (Java SDK rejects {{form:{{}},url:{{}}}})",
    );

    // Sanity: no extra unexpected capability keys.
    let cap_keys: Vec<&String> = caps.as_object().unwrap().keys().collect();
    assert_eq!(
        cap_keys.len(),
        2,
        "capabilities must have exactly 2 keys (roots, elicitation), got {cap_keys:?}",
    );
}

#[test]
fn mcp_initialize_wire_bytes_contain_claude_code_marker() {
    // Lock the BYTES of the outgoing JSON payload — every MCP server
    // receives this exact string for `clientInfo.name`. Catches any
    // accidental rename or serde rename-all-snake-case slip.
    let params = InitializeParams::default();
    let bytes = serde_json::to_vec(&params).expect("serialize");
    let s = std::str::from_utf8(&bytes).expect("utf8");
    assert!(
        s.contains(r#""name":"claude-code""#),
        "wire bytes must contain literal \"name\":\"claude-code\", got: {s}",
    );
    assert!(
        s.contains(r#""websiteUrl":"https://claude.com/claude-code""#),
        "wire bytes must contain literal websiteUrl, got: {s}",
    );
}
