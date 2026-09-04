//! r1-backlog-protocol-mirrors-05 — the positional lock `variant_ordinal_lock_test.rs`
//! does not cover.
//!
//! # What this catches that `variant_ordinal_lock_test.rs` cannot
//!
//! That file freezes `ClientCommand` and `ClientEvent` declaration order. It
//! says nothing about `local_apps.rs`, whose own doc comments make the same
//! claim about their own types:
//!
//! - `AppRecordDto` (a `#[derive(uniffi::Record)]` struct) — `scaffolded`'s
//!   doc comment: "Appended LAST: UniFFI encodes record fields POSITIONALLY,
//!   so a field inserted above `workspace_rel` would be reinterpreted by a
//!   client built against the previous bindings."
//! - `AppEventDto` (a `#[derive(uniffi::Enum)]` enum) — `AppCreated`'s doc
//!   comment: "Appended at the END, and that is load-bearing rather than
//!   tidy: UniFFI encodes this enum POSITIONALLY, so a variant inserted
//!   anywhere above renumbers every variant after it."
//! - `PluginCommandDto` (also `#[derive(uniffi::Enum)]`) — its own doc:
//!   "Future §17.1 additions … extend this enum, not `ClientCommand` again",
//!   i.e. it is a real UniFFI enum subject to the same ordinal rule, just
//!   never given a lock.
//!
//! Before this file, nothing would fail if a field were spliced above
//! `workspace_rel`, or a variant spliced into the middle of `AppEventDto` or
//! `PluginCommandDto`: `version_guard_test.rs` fingerprints an UNORDERED
//! map of `"<type>::<member>" -> "<wire type>"`, so it is as blind to
//! position as the commands/events guard `variant_ordinal_lock_test.rs`
//! describes itself fixing for those two enums.
//!
//! # The rule this enforces
//!
//! The members frozen below are `AppRecordDto`'s fields, and
//! `PluginCommandDto` / `AppEventDto`'s variants, as declared at the
//! `CLIENT_PROTOCOL_VERSION` 12.0.0 bless (commit `0c313e1b9`,
//! `client-protocol/src/local_apps.rs`). Each must still sit at exactly the
//! index recorded here.
//!
//! - Appending AFTER the frozen prefix: allowed, no bump. Do not add it here
//!   — this is the 12.0.0 snapshot, not a mirror of the current type.
//! - Inserting, removing or reordering anything inside the frozen prefix:
//!   this test goes red and names the ordinal. Move it to the end instead.
//!   If it genuinely cannot go last, that is a MAJOR `CLIENT_PROTOCOL_VERSION`
//!   bump plus a re-bless of `snapshots/blessed_major.txt`, and the frozen
//!   lists here are then re-cut from the new major.

const LOCAL_APPS_SRC: &str = include_str!("../src/local_apps.rs");

/// `AppRecordDto`'s fields in declaration order as of the 12.0.0 bless
/// (`0c313e1b9`). 11 entries.
const APP_RECORD_DTO_FIELDS_AT_12_0_0: &[&str] = &[
    "id",
    "name",
    "brief",
    "git_enabled",
    "created_at_ms",
    "updated_at_ms",
    "workflow_state",
    "conversation_id",
    "init_session_id",
    "workspace_rel",
    "scaffolded",
];

/// `PluginCommandDto`'s variants in declaration order as of the 12.0.0 bless
/// (`0c313e1b9`). 10 entries.
const PLUGIN_COMMAND_DTO_ORDINALS_AT_12_0_0: &[&str] = &[
    "SetEnabled",
    "GetStatus",
    "GetInventory",
    "ResolveCreateConfirmation",
    "ResolveMcpProposalApproval",
    "StartLocalAppMcpAuthoring",
    "SetLocalAppMcpEnabled",
    "SetLocalAppMcpToolEnabled",
    "SetLocalAppMcpConversationPinned",
    "GetManagedMcpInventory",
];

/// `AppEventDto`'s variants in declaration order as of the 12.0.0 bless
/// (`0c313e1b9`). 20 entries.
const APP_EVENT_DTO_ORDINALS_AT_12_0_0: &[&str] = &[
    "AppDetailsChanged",
    "AppBridgeResponse",
    "AppUiRequest",
    "AppCapabilityRequested",
    "AppCheckpointsChanged",
    "AppLlmActivityChanged",
    "AppAgentEventPosted",
    "AppBridgeStreamFrame",
    "AppRecordChanged",
    "AppProfileProposal",
    "AppBackgroundTaskChanged",
    "AppCreated",
    "AppDependencyChangeConfirmationRequested",
    "PluginStatusChanged",
    "PluginInventoryChanged",
    "CreateConfirmationRequested",
    "McpProposalApprovalRequested",
    "ManagedMcpInventoryChanged",
    "VerificationSummaryChanged",
    "LocalAppOperationFailed",
];

/// Strip `//`-comments so brace counting is not thrown off by prose.
fn strip_line_comment(line: &str) -> &str {
    match line.find("//") {
        Some(at) => &line[..at],
        None => line,
    }
}

/// Extract the top-level variant identifiers of `enum <name>` from Rust
/// source, in declaration order. Identical approach to
/// `variant_ordinal_lock_test.rs::variants_in_declaration_order` (kept as an
/// independent copy rather than a shared `#[path]` include, so a bug fixed
/// in one does not silently start passing the other without review).
fn variants_in_declaration_order(src: &str, enum_name: &str) -> Vec<String> {
    let header = format!("pub enum {enum_name} {{");
    let start = src
        .find(&header)
        .unwrap_or_else(|| panic!("`{header}` not found in the source"))
        + header.len();

    let mut depth: i32 = 1;
    let mut out = Vec::new();
    for line in src[start..].lines() {
        let code = strip_line_comment(line);

        if depth == 1 {
            let trimmed = code.trim_start();
            let ident: String = trimmed
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                .collect();
            if !ident.is_empty() && ident.starts_with(|c: char| c.is_ascii_uppercase()) {
                let rest = trimmed[ident.len()..].trim_start();
                if rest.starts_with('{')
                    || rest.starts_with('(')
                    || rest.starts_with(',')
                    || rest.is_empty()
                {
                    out.push(ident);
                }
            }
        }

        for c in code.chars() {
            match c {
                '{' | '(' | '[' => depth += 1,
                '}' | ')' | ']' => depth -= 1,
                _ => {}
            }
        }
        if depth <= 0 {
            break;
        }
    }
    out
}

/// Extract the top-level field identifiers of `struct <name>` from Rust
/// source, in declaration order. `pub struct` bodies here hold only
/// `pub <field>: <Type>,` lines (plus attributes/doc comments, which never
/// start a line with a lowercase identifier followed by `:`), so — unlike
/// the enum parser — depth tracking only needs to stop at the matching
/// close-brace; nested `{`/`}` inside a field's own type (e.g.
/// `Vec<AppDataFieldDto>`) never occurs because Rust field types don't use
/// braces.
fn fields_in_declaration_order(src: &str, struct_name: &str) -> Vec<String> {
    let header = format!("pub struct {struct_name} {{");
    let start = src
        .find(&header)
        .unwrap_or_else(|| panic!("`{header}` not found in the source"))
        + header.len();

    let mut depth: i32 = 1;
    let mut out = Vec::new();
    for line in src[start..].lines() {
        let code = strip_line_comment(line);
        let trimmed = code.trim_start();

        if depth == 1 {
            if let Some(rest) = trimmed.strip_prefix("pub ") {
                let ident: String = rest
                    .chars()
                    .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                    .collect();
                if !ident.is_empty() && rest[ident.len()..].trim_start().starts_with(':') {
                    out.push(ident);
                }
            }
        }

        for c in code.chars() {
            match c {
                '{' | '(' | '[' => depth += 1,
                '}' | ')' | ']' => depth -= 1,
                _ => {}
            }
        }
        if depth <= 0 {
            break;
        }
    }
    out
}

/// The shared assertion, mirroring
/// `variant_ordinal_lock_test.rs::assert_frozen_prefix`: reports the first
/// divergence by ordinal and name.
fn assert_frozen_prefix(type_name: &str, actual: &[String], frozen: &[&str]) {
    assert!(
        actual.len() >= frozen.len(),
        "`{type_name}` (client-protocol/src/local_apps.rs) now has {} members, fewer than the \
         {} frozen at the 12.0.0 bless. A REMOVED member shifts every later ordinal on the \
         positional UniFFI mobile bindings and is a breaking change: it needs a MAJOR \
         CLIENT_PROTOCOL_VERSION bump and a re-bless of snapshots/blessed_major.txt.\n\
         Current order: {actual:?}",
        actual.len(),
        frozen.len(),
    );

    for (ordinal, expected) in frozen.iter().enumerate() {
        let found = actual[ordinal].as_str();
        assert_eq!(
            found,
            *expected,
            "`{type_name}` ordinal {ordinal} (UniFFI ordinal {uniffi}) is `{found}`, but \
             `{expected}` was frozen there at the 12.0.0 bless.\n\
             \n\
             UniFFI encodes a variant/field as its DECLARATION INDEX, so a member inserted, \
             removed or reordered ahead of ordinal {ordinal} shifts this and every later member. \
             An old mobile client and a new host would both still advertise \
             CLIENT_PROTOCOL_VERSION 12.0.0, the handshake would accept, and `{expected}` would \
             decode as `{found}`.\n\
             \n\
             FIX: move the new member to the END of `{type_name}` in \
             client-protocol/src/local_apps.rs. Appending is additive and needs no bump. Only \
             if it truly cannot go last is this a MAJOR CLIENT_PROTOCOL_VERSION bump plus a \
             re-bless of snapshots/blessed_major.txt — and then re-cut the frozen list in this \
             file from the new major.\n\
             \n\
             Full current order: {actual:?}",
            uniffi = ordinal + 1,
        );
    }
}

#[test]
fn app_record_dto_keeps_every_12_0_0_field_at_its_ordinal() {
    let actual = fields_in_declaration_order(LOCAL_APPS_SRC, "AppRecordDto");
    assert_frozen_prefix("AppRecordDto", &actual, APP_RECORD_DTO_FIELDS_AT_12_0_0);
}

#[test]
fn plugin_command_dto_keeps_every_12_0_0_variant_at_its_ordinal() {
    let actual = variants_in_declaration_order(LOCAL_APPS_SRC, "PluginCommandDto");
    assert_frozen_prefix(
        "PluginCommandDto",
        &actual,
        PLUGIN_COMMAND_DTO_ORDINALS_AT_12_0_0,
    );
}

#[test]
fn app_event_dto_keeps_every_12_0_0_variant_at_its_ordinal() {
    let actual = variants_in_declaration_order(LOCAL_APPS_SRC, "AppEventDto");
    assert_frozen_prefix("AppEventDto", &actual, APP_EVENT_DTO_ORDINALS_AT_12_0_0);
}

/// Proof this gate can go red: feed the field parser a synthetic struct with
/// a field spliced into the middle and confirm the assertion fires and names
/// the ordinal.
#[test]
fn a_mid_struct_field_insertion_is_detected() {
    const FROZEN: &[&str] = &["alpha", "beta", "gamma"];

    let good = "pub struct Synthetic {\n    pub alpha: String,\n    /// a brace in prose: {x, y}\n    pub beta: u64,\n    pub gamma: bool,\n    pub delta: Vec<Other>,\n}\n";
    let parsed = fields_in_declaration_order(good, "Synthetic");
    assert_eq!(
        parsed,
        vec!["alpha", "beta", "gamma", "delta"],
        "appending `delta` must parse as ordinal 3"
    );
    assert_frozen_prefix("Synthetic", &parsed, FROZEN);

    let spliced = "pub struct Synthetic {\n    pub alpha: String,\n    pub inserted: u64,\n    pub beta: u64,\n    pub gamma: bool,\n}\n";
    let parsed = fields_in_declaration_order(spliced, "Synthetic");
    assert_eq!(parsed, vec!["alpha", "inserted", "beta", "gamma"]);

    let previous_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let failure = std::panic::catch_unwind(|| {
        assert_frozen_prefix("Synthetic", &parsed, FROZEN);
    });
    std::panic::set_hook(previous_hook);
    let failure = failure.expect_err("a mid-struct field insertion must fail assert_frozen_prefix");
    let message = failure
        .downcast_ref::<String>()
        .map(String::as_str)
        .or_else(|| failure.downcast_ref::<&str>().copied())
        .unwrap_or("");
    assert!(
        message.contains("ordinal 1") && message.contains("inserted") && message.contains("beta"),
        "the failure must name the ordinal and both fields; got: {message}"
    );
}

/// Same proof for the enum-variant parser, reusing the shape already
/// validated in `variant_ordinal_lock_test.rs::a_mid_enum_insertion_is_detected`
/// but against this file's own copy of the parser so a regression here is
/// caught locally rather than only in the sibling file.
#[test]
fn a_mid_enum_variant_insertion_is_detected() {
    const FROZEN: &[&str] = &["Alpha", "Beta", "Gamma"];

    let spliced = "pub enum Synthetic {\n    Alpha { a: u64 },\n\n    Inserted { i: u64 },\n\n    Beta {\n        b: u64,\n    },\n\n    Gamma,\n}\n";
    let parsed = variants_in_declaration_order(spliced, "Synthetic");
    assert_eq!(parsed, vec!["Alpha", "Inserted", "Beta", "Gamma"]);

    let previous_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let failure = std::panic::catch_unwind(|| {
        assert_frozen_prefix("Synthetic", &parsed, FROZEN);
    });
    std::panic::set_hook(previous_hook);
    let failure = failure.expect_err("a mid-enum insertion must fail assert_frozen_prefix");
    let message = failure
        .downcast_ref::<String>()
        .map(String::as_str)
        .or_else(|| failure.downcast_ref::<&str>().copied())
        .unwrap_or("");
    assert!(
        message.contains("ordinal 1") && message.contains("Inserted") && message.contains("Beta"),
        "the failure must name the ordinal and both variants; got: {message}"
    );
}
