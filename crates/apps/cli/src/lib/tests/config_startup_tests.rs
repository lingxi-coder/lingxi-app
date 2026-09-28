use super::{command_initializes_user_id, command_runs_config_startup};
use crate::argv::Argv;

fn parsed(args: &[&str]) -> Argv {
    Argv::from_iter(std::iter::once("lingxi-cli").chain(args.iter().copied())).unwrap()
}

#[test]
fn standard_commands_run_startup_while_special_fast_paths_do_not() {
    for args in [
        &["auth", "status"][..],
        &["auto-mode", "defaults"][..],
        &["mcp", "list"][..],
        &["plugin", "list"][..],
        &["doctor"][..],
    ] {
        let argv = parsed(args);
        assert!(
            command_runs_config_startup(argv.command.as_ref()),
            "{args:?}"
        );
    }
    for args in [
        &["project"][..],
        &["attach", "missing"][..],
        &["remote-control"][..],
    ] {
        let argv = parsed(args);
        assert!(
            !command_runs_config_startup(argv.command.as_ref()),
            "{args:?}"
        );
    }
    let project_purge = parsed(&["project", "purge", "--dry-run", "--yes"]);
    assert!(command_runs_config_startup(project_purge.command.as_ref()));
    assert!(command_runs_config_startup(None));
}

#[test]
fn only_device_identity_command_families_eagerly_create_user_id() {
    for args in [
        &["mcp", "list"][..],
        &["doctor"][..],
        &["setup-token"][..],
        &["install"][..],
        &["update"][..],
    ] {
        let argv = parsed(args);
        assert!(
            command_initializes_user_id(argv.command.as_ref()),
            "{args:?}"
        );
    }
    let argv = parsed(&["auth", "status"]);
    assert!(!command_initializes_user_id(argv.command.as_ref()));
}
