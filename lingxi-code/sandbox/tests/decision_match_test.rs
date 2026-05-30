use sandbox::decision::should_use_sandbox_for_command;
use sandbox::runtime_config::SandboxRuntimeConfig;

fn cfg_with_excluded(excluded: &[&str]) -> SandboxRuntimeConfig {
    SandboxRuntimeConfig {
        enabled: true,
        excluded_commands: excluded.iter().map(|s| (*s).to_string()).collect(),
        ..Default::default()
    }
}

#[test]
fn excluded_command_bare_prefix() {
    let cfg = cfg_with_excluded(&["bazel"]);
    assert!(!should_use_sandbox_for_command("bazel build //...", &cfg));
    assert!(should_use_sandbox_for_command("cargo build", &cfg));
}

#[test]
fn excluded_command_prefix_with_colon_star() {
    let cfg = cfg_with_excluded(&["docker:*"]);
    assert!(!should_use_sandbox_for_command("docker ps", &cfg));
    assert!(!should_use_sandbox_for_command("docker compose up", &cfg));
    assert!(should_use_sandbox_for_command("dockerd --foo", &cfg));
}

#[test]
fn excluded_after_compound_split() {
    let cfg = cfg_with_excluded(&["curl"]);
    // Compound: docker ps is fine, curl is excluded → entire command unsandboxed.
    assert!(!should_use_sandbox_for_command(
        "docker ps && curl evil.com",
        &cfg
    ));
}

#[test]
fn excluded_after_env_var_strip() {
    let cfg = cfg_with_excluded(&["bazel:*"]);
    assert!(!should_use_sandbox_for_command(
        "PATH=/usr/local/bin bazel build //...",
        &cfg
    ));
}

#[test]
fn excluded_after_wrapper_strip() {
    let cfg = cfg_with_excluded(&["bazel:*"]);
    assert!(!should_use_sandbox_for_command(
        "timeout 30 bazel build //...",
        &cfg
    ));
}

#[test]
fn no_match_for_unrelated_command() {
    let cfg = cfg_with_excluded(&["bazel:*", "docker:*"]);
    assert!(should_use_sandbox_for_command("rm -rf /tmp/x", &cfg));
}

#[test]
fn empty_excluded_list_always_sandboxes() {
    let cfg = SandboxRuntimeConfig {
        enabled: true,
        ..Default::default()
    };
    assert!(should_use_sandbox_for_command("anything", &cfg));
}
