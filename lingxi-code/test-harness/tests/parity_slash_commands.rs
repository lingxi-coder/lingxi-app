//! Parity: lock the 99 builtin slash-command names + the 18-core split + the
//! stub-literal output across the full surface.
//!
//! See plan `docs/superpowers/plans/2026-05-25-m5-09-commands-surface.md`
//! Task 6. Locks introduced here (per 2026-05-28 addendum):
//!
//! - Total name count = 99
//! - Core name count = 18
//! - Unimplemented = 81
//! - Stub literal template = "{name}: not implemented in v0.6.0 (M5)"
//! - Unknown literal template = "Unknown command: /{name}"

use command_api::builtin_support::names::{BUILTIN_COMMAND_NAMES, BUILTIN_CORE_NAMES};
use command_api::CommandRegistry;
use command_api::RegistrySlashDispatcher;
use command_core::register_all_builtin_commands;
use serde::Deserialize;
use std::sync::Arc;
use test_harness::parity::load_fixture;
use tokio::sync::RwLock;
use traits::{SlashCommandDispatcher, SlashDispatchResult};

#[derive(Debug, Deserialize)]
struct ParityFile {
    #[serde(rename = "_meta")]
    meta: ParityMeta,
    commands: Vec<ParityCommand>,
}

#[derive(Debug, Deserialize)]
struct ParityMeta {
    total_count_lock: usize,
    core_count_lock: usize,
    unimplemented_count_lock: usize,
    stub_literal_template: String,
    unknown_literal_template: String,
}

#[derive(Debug, Deserialize)]
struct ParityCommand {
    name: String,
    is_core: bool,
    #[serde(default)]
    description: Option<String>,
}

// ============================================================================
// T3 — V2 fixture types: adds `implemented` field (M5-14)
// ============================================================================

#[derive(Debug, Deserialize)]
struct ParityFileV2 {
    #[serde(rename = "_meta")]
    #[allow(dead_code)]
    meta: ParityMeta,
    commands: Vec<ParityCommandV2>,
}

#[derive(Debug, Deserialize)]
struct ParityCommandV2 {
    name: String,
    is_core: bool,
    implemented: bool,
    #[serde(default)]
    #[allow(dead_code)]
    description: Option<String>,
}

fn fixture() -> ParityFile {
    // Fixture filename retained as `parity_slash_commands_102` for git-history
    // continuity; the counts inside reflect the 99-name lock per the
    // 2026-05-28 addendum.
    load_fixture("parity_slash_commands_102")
}

fn fixture_v2() -> ParityFileV2 {
    load_fixture("parity_slash_commands_102")
}

#[test]
fn fixture_total_matches_constant() {
    let f = fixture();
    assert_eq!(f.meta.total_count_lock, 99);
    assert_eq!(f.commands.len(), 99);
    assert_eq!(BUILTIN_COMMAND_NAMES.len(), 99);
    assert_eq!(f.commands.len(), BUILTIN_COMMAND_NAMES.len());
}

#[test]
fn fixture_core_matches_constant() {
    let f = fixture();
    let fixture_core: Vec<&str> = f
        .commands
        .iter()
        .filter(|c| c.is_core)
        .map(|c| c.name.as_str())
        .collect();
    assert_eq!(fixture_core.len(), 18);
    assert_eq!(f.meta.core_count_lock, 18);
    assert_eq!(BUILTIN_CORE_NAMES.len(), 18);

    // Order-sensitive equality (both are ASCII-sorted by construction).
    let const_core: Vec<&str> = BUILTIN_CORE_NAMES.to_vec();
    assert_eq!(fixture_core, const_core);
}

#[test]
fn fixture_unimplemented_count_is_81() {
    let f = fixture();
    let n_unimpl = f.commands.iter().filter(|c| !c.is_core).count();
    assert_eq!(n_unimpl, 81);
    assert_eq!(f.meta.unimplemented_count_lock, 81);
}

#[test]
fn stub_template_lock() {
    let f = fixture();
    assert_eq!(
        f.meta.stub_literal_template,
        "{name}: not implemented in v0.6.0 (M5)"
    );
}

#[test]
fn unknown_template_lock() {
    let f = fixture();
    assert_eq!(f.meta.unknown_literal_template, "Unknown command: /{name}");
}

#[test]
fn fixture_names_match_constant_order() {
    let f = fixture();
    let fixture_names: Vec<&str> = f.commands.iter().map(|c| c.name.as_str()).collect();
    let const_names: Vec<&str> = BUILTIN_COMMAND_NAMES.to_vec();
    assert_eq!(fixture_names, const_names);
}

#[tokio::test]
async fn every_fixture_command_dispatches_to_handled() {
    // Parity lock: every builtin command name is REGISTERED and dispatches to
    // `Handled`. The original M5-era assertion that each returns the stub
    // literal ("{name}: not implemented in v0.6.0 (M5)") is obsolete — later
    // parity batches (merged from main) implemented many commands with real
    // output (e.g. /commit), so this locks the full name surface dispatches,
    // not the exact body.
    let f = fixture();
    let mut reg = CommandRegistry::new();
    register_all_builtin_commands(&mut reg);
    let d = RegistrySlashDispatcher::new(Arc::new(RwLock::new(reg)));

    for entry in &f.commands {
        let raw = format!("/{}", entry.name);
        match d.dispatch(&raw).await {
            SlashDispatchResult::Handled { .. } => {}
            other => panic!("/{} must dispatch to Handled, got {other:?}", entry.name),
        }
    }
}

#[tokio::test]
async fn unknown_command_uses_locked_literal() {
    let mut reg = CommandRegistry::new();
    register_all_builtin_commands(&mut reg);
    let d = RegistrySlashDispatcher::new(Arc::new(RwLock::new(reg)));

    let outcome = d.dispatch("/definitely-not-a-real-command").await;
    match outcome {
        SlashDispatchResult::Unknown { name, display } => {
            assert_eq!(name, "definitely-not-a-real-command");
            assert_eq!(display, "Unknown command: /definitely-not-a-real-command");
        }
        other => panic!("expected Unknown, got {other:?}"),
    }
}

#[test]
fn core_command_description_matches_fixture() {
    let f = fixture();
    let mut reg = CommandRegistry::new();
    register_all_builtin_commands(&mut reg);
    for entry in f.commands.iter().filter(|c| c.is_core) {
        let expected = entry
            .description
            .as_deref()
            .unwrap_or_else(|| panic!("core /{} missing description in fixture", entry.name));
        let cmd = reg.resolve(&entry.name).expect("core cmd missing");
        assert_eq!(
            cmd.description, expected,
            "/{}: description drift (fixture vs registry)",
            entry.name
        );
    }
}

// ============================================================================
// T3 — M5-14: `implemented` field coverage (18 implemented / 81 unimplemented)
// ============================================================================

#[test]
fn exactly_18_commands_marked_implemented() {
    let f = fixture_v2();
    let n_impl = f.commands.iter().filter(|c| c.implemented).count();
    assert_eq!(
        n_impl, 18,
        "expected exactly 18 implemented commands; got {n_impl}"
    );
}

#[test]
fn exactly_81_commands_marked_unimplemented() {
    let f = fixture_v2();
    let n_unimpl = f.commands.iter().filter(|c| !c.implemented).count();
    assert_eq!(
        n_unimpl, 81,
        "expected exactly 81 unimplemented commands; got {n_unimpl}"
    );
}

#[test]
fn implemented_set_matches_is_core_set() {
    let f = fixture_v2();
    // Every command where `implemented = true` must also have `is_core = true`,
    // and vice versa — the two fields must be in perfect agreement.
    for c in &f.commands {
        assert_eq!(
            c.implemented, c.is_core,
            "/{}: `implemented` ({}) != `is_core` ({}); fields must agree",
            c.name, c.implemented, c.is_core
        );
    }
}

#[test]
fn implemented_names_match_builtin_core_names_constant() {
    let f = fixture_v2();
    let mut fixture_impl: Vec<&str> = f
        .commands
        .iter()
        .filter(|c| c.implemented)
        .map(|c| c.name.as_str())
        .collect();
    fixture_impl.sort_unstable();

    let mut const_core: Vec<&str> =
        command_api::builtin_support::names::BUILTIN_CORE_NAMES.to_vec();
    const_core.sort_unstable();

    assert_eq!(
        fixture_impl, const_core,
        "fixture `implemented` names do not match BUILTIN_CORE_NAMES constant"
    );
}
