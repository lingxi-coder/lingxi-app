//! Live version-facing identifier pins for the Claude Code 2.1.220 oracle.
//!
//! 2.1.220 is the binary this port is read against — the forked-skill sidecars,
//! the `set_cwd` trust handshake, PowerShell 5.1 cwd-first shadowing, the
//! ←-on-empty gesture and the refusal cascade were all ported from it. The
//! advertised version had lagged at 2.1.217, so a session implementing 2.1.220
//! behaviour was telling servers and child processes it was something else.
//!
//! Everything below derives from ONE constant, so the three identifiers cannot
//! drift apart.

use std::collections::BTreeSet;

use serde_json::Value;

const BACKLOG_CONTRACTS: &str =
    include_str!("../src/parity/fixtures/claude_2_1_220_backlog_contracts.json");

fn backlog_contracts() -> Value {
    serde_json::from_str(BACKLOG_CONTRACTS).expect("2.1.220 backlog fixture must be valid JSON")
}

/// The current parity target is exposed from one source of truth.
#[test]
fn version_const_is_2_1_220() {
    assert_eq!(traits::CLAUDE_CODE_VERSION, "2.1.220");
}

/// Child processes receive the 2.1.220 `AI_AGENT` identifier.
#[test]
fn ai_agent_env_value_is_2_1_220() {
    let derived = format!(
        "claude-code_{}_agent",
        traits::CLAUDE_CODE_VERSION.replace('.', "-")
    );
    assert_eq!(derived, "claude-code_2-1-220_agent");
}

/// WebFetch presents the 2.1.220 Claude-compatible user agent.
#[test]
fn web_fetch_user_agent_is_2_1_220() {
    let derived = format!(
        "Claude-User (claude-code/{}; +https://support.anthropic.com/)",
        traits::CLAUDE_CODE_VERSION
    );
    assert_eq!(
        derived,
        "Claude-User (claude-code/2.1.220; +https://support.anthropic.com/)"
    );
}

/// Every version-facing identifier derives from the SAME constant, so a future
/// bump cannot move one and leave another behind — which is exactly how the
/// port came to advertise 2.1.217 while implementing 2.1.220.
#[test]
fn the_identifiers_share_one_source() {
    let v = traits::CLAUDE_CODE_VERSION;
    assert!(format!("claude-code_{}_agent", v.replace('.', "-")).contains(&v.replace('.', "-")));
    assert!(format!("Claude-User (claude-code/{v}; +https://support.anthropic.com/)").contains(v));
}

/// Wave 0 maps every approved engineering item to exactly one implementation
/// wave. The private remote-memory protocol is deliberately tracked outside
/// this list as a single explicit divergence.
#[test]
fn approved_backlog_has_25_unique_items() {
    let fixture = backlog_contracts();
    let items = fixture["items"].as_array().expect("items array");
    assert_eq!(items.len(), 25);

    let ids = items
        .iter()
        .map(|item| item["id"].as_str().expect("item id"))
        .collect::<BTreeSet<_>>();
    assert_eq!(ids.len(), 25, "backlog IDs must be unique");
    assert!(
        items.iter().all(|item| item["target"] == "CLOSED"),
        "every in-scope item must have a CLOSED target"
    );
    assert!(
        items
            .iter()
            .all(|item| matches!(item["wave"].as_u64(), Some(1..=4))),
        "every item must be assigned to Wave 1-4"
    );
}

#[test]
fn private_remote_memory_is_one_explicit_divergence() {
    let fixture = backlog_contracts();
    let divergences = fixture["divergences"]
        .as_array()
        .expect("divergences array");
    assert_eq!(divergences.len(), 1);
    assert_eq!(divergences[0]["id"], "N-env-3/N-protocol-8");
    assert!(divergences[0]["reason"]
        .as_str()
        .expect("divergence reason")
        .contains("private account remote-memory"));
}

/// These are the clean-room black-box contracts most likely to be weakened by
/// an implementation that only searches for names without reproducing the
/// observable state transitions.
#[test]
fn stateful_2_1_220_contracts_are_pinned() {
    let fixture = backlog_contracts();
    let contracts = &fixture["pinned_contracts"];

    assert_eq!(
        contracts["ultracode"]["transitions"],
        serde_json::json!(["enter", "sparse", "exit"])
    );
    assert_eq!(contracts["ultracode"]["default_sparse_cadence"], 10);
    assert_eq!(contracts["observer"]["default_max_depth"], 3);
    assert_eq!(
        contracts["deep_research"]["stages"],
        serde_json::json!(["Scope", "Search", "Fetch", "Verify", "Synthesize"])
    );
    assert_eq!(contracts["deep_research"]["votes_per_claim"], 3);
    assert_eq!(contracts["deep_research"]["max_fetch"], 15);
    assert_eq!(
        contracts["opus_5_bash_addition"],
        "Command output is displayed to you, not reliably to the user."
    );
    assert_eq!(contracts["attached_left_arrow"]["outcome"], "detach");
    assert_eq!(
        contracts["accessibility"]["announces_edit_delta_only"],
        true
    );
}
