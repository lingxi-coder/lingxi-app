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

use lingxi_commands::builtin::{BUILTIN_COMMAND_NAMES, BUILTIN_CORE_NAMES};
use lingxi_commands::dispatcher::RegistrySlashDispatcher;
use lingxi_commands::registry::{register_all_builtin_commands, CommandRegistry};
use lingxi_test_harness::parity::load_fixture;
use lingxi_traits::{SlashCommandDispatcher, SlashDispatchResult};
use serde::Deserialize;
use std::sync::Arc;
use tokio::sync::RwLock;

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

fn fixture() -> ParityFile {
    // Fixture filename retained as `parity_slash_commands_102` for git-history
    // continuity; the counts inside reflect the 99-name lock per the
    // 2026-05-28 addendum.
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
async fn every_fixture_command_dispatches_to_expected_literal() {
    let f = fixture();
    let mut reg = CommandRegistry::new();
    register_all_builtin_commands(&mut reg);
    let d = RegistrySlashDispatcher::new(Arc::new(RwLock::new(reg)));

    for entry in &f.commands {
        let raw = format!("/{}", entry.name);
        let outcome = d.dispatch(&raw).await;
        match outcome {
            SlashDispatchResult::Handled { display } => {
                let expected = format!("{}: not implemented in v0.6.0 (M5)", entry.name);
                assert_eq!(display, expected, "wrong output for /{}", entry.name);
            }
            other => panic!("/{}: expected Handled, got {other:?}", entry.name),
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
