use lingxi_sandbox::policy_convert::{
    convert_settings_to_runtime_config, linux_glob_pattern_warnings,
};
use lingxi_sandbox::runtime_config::{SandboxSettingsJson, SettingsJson, SettingsPermissions};
use std::path::PathBuf;

fn settings(allow: Vec<&str>, deny: Vec<&str>) -> SettingsJson {
    SettingsJson {
        permissions: Some(SettingsPermissions {
            allow: allow.into_iter().map(String::from).collect(),
            deny: deny.into_iter().map(String::from).collect(),
            additional_directories: vec![],
        }),
        sandbox: Some(SandboxSettingsJson {
            enabled: Some(true),
            ..Default::default()
        }),
        settings_dir: Some(PathBuf::from("/home/u/.claude")),
    }
}

#[test]
fn extracts_edit_allow_into_allow_write() {
    let s = settings(vec!["Edit(./src/**)"], vec![]);
    let cfg = convert_settings_to_runtime_config(&s);
    assert!(cfg.filesystem.allow_write.contains(&"./src/**".to_string()));
}

#[test]
fn extracts_edit_deny_into_deny_write() {
    let s = settings(vec![], vec!["Edit(//.git/**)"]);
    let cfg = convert_settings_to_runtime_config(&s);
    // `//.git/**` resolves to `/.git/**` (one leading slash stripped).
    assert!(cfg.filesystem.deny_write.contains(&"/.git/**".to_string()));
}

#[test]
fn extracts_read_allow_into_allow_read() {
    let s = settings(vec!["Read(~/.aws/credentials)"], vec![]);
    let cfg = convert_settings_to_runtime_config(&s);
    assert!(cfg
        .filesystem
        .allow_read
        .contains(&"~/.aws/credentials".to_string()));
}

#[test]
fn extracts_read_deny_into_deny_read() {
    let s = settings(vec![], vec!["Read(/secret)"]);
    let cfg = convert_settings_to_runtime_config(&s);
    // `/secret` resolves relative to settings_dir = /home/u/.claude.
    assert!(cfg
        .filesystem
        .deny_read
        .contains(&"/home/u/.claude/secret".to_string()));
}

#[test]
fn extracts_webfetch_domain_into_allowed_domains() {
    let s = settings(vec!["WebFetch(domain:anthropic.com)"], vec![]);
    let cfg = convert_settings_to_runtime_config(&s);
    assert!(cfg
        .network
        .allowed_domains
        .contains(&"anthropic.com".to_string()));
}

#[test]
fn bash_rules_are_ignored_in_this_layer() {
    let s = settings(vec!["Bash(curl:*)"], vec!["Bash(rm:*)"]);
    let cfg = convert_settings_to_runtime_config(&s);
    assert!(cfg.filesystem.allow_write.is_empty());
    assert!(cfg.filesystem.deny_write.is_empty());
}

#[test]
fn passes_through_sandbox_subsection_values() {
    let mut s = settings(vec![], vec![]);
    s.sandbox = Some(SandboxSettingsJson {
        enabled: Some(true),
        fail_if_unavailable: Some(true),
        excluded_commands: Some(vec!["bazel".into(), "make".into()]),
        ..Default::default()
    });
    let cfg = convert_settings_to_runtime_config(&s);
    assert!(cfg.enabled);
    assert!(cfg.fail_if_unavailable);
    assert_eq!(cfg.excluded_commands, vec!["bazel", "make"]);
}

#[test]
fn linux_glob_warning_for_star_in_edit_rule() {
    let s = settings(vec!["Edit(./src/*.rs)"], vec![]);
    let warnings = linux_glob_pattern_warnings(&s);
    assert!(
        warnings.iter().any(|w| w == "Edit(./src/*.rs)"),
        "expected warning for Edit(./src/*.rs), got {warnings:?}"
    );
}

#[test]
fn linux_glob_warning_skips_trailing_double_star() {
    let s = settings(vec!["Edit(./src/**)"], vec![]);
    let warnings = linux_glob_pattern_warnings(&s);
    assert!(
        warnings.is_empty(),
        "expected no warning for trailing /** but got {warnings:?}"
    );
}

#[test]
fn linux_glob_warning_for_brackets() {
    let s = settings(vec![], vec!["Read(./[ab]/file)"]);
    let warnings = linux_glob_pattern_warnings(&s);
    assert!(warnings.iter().any(|w| w == "Read(./[ab]/file)"));
}

#[test]
fn linux_glob_warning_for_question_mark() {
    let s = settings(vec!["Edit(./foo?.txt)"], vec![]);
    let warnings = linux_glob_pattern_warnings(&s);
    assert!(warnings.iter().any(|w| w == "Edit(./foo?.txt)"));
}
