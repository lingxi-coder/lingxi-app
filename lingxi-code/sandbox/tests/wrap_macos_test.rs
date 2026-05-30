use sandbox::runtime_config::{
    FilesystemRestrictionConfig, NetworkRestrictionConfig, Platform, SandboxRuntimeConfig,
};
use sandbox::wrap::wrap_with_sandbox;

fn cfg(allow_write: Vec<&str>, deny_read: Vec<&str>) -> SandboxRuntimeConfig {
    SandboxRuntimeConfig {
        enabled: true,
        filesystem: FilesystemRestrictionConfig {
            allow_write: allow_write.into_iter().map(String::from).collect(),
            deny_read: deny_read.into_iter().map(String::from).collect(),
            ..Default::default()
        },
        network: NetworkRestrictionConfig::default(),
        ..Default::default()
    }
}

#[test]
fn macos_wrap_starts_with_sandbox_exec_dash_f() {
    let wrapped =
        wrap_with_sandbox("ls", &cfg(vec!["/tmp/work"], vec![]), Platform::Mac).expect("wrap ok");
    assert!(wrapped.starts_with("sandbox-exec -f "), "got: {wrapped}");
    assert!(wrapped.contains("/bin/sh -c"), "got: {wrapped}");
}

#[test]
fn macos_wrap_emits_profile_at_real_path() {
    let wrapped = wrap_with_sandbox("ls", &cfg(vec![], vec![]), Platform::Mac).expect("wrap ok");
    // Extract the profile path from `sandbox-exec -f <path> ...`.
    let tail = wrapped.strip_prefix("sandbox-exec -f ").unwrap();
    let mut parts = tail.splitn(2, ' ');
    let path = parts.next().expect("profile path");
    let content = std::fs::read_to_string(path).expect("read profile");
    assert!(content.starts_with("(version 1)\n"), "got: {content}");
    assert!(content.contains("(deny default)"), "got: {content}");
    assert!(content.contains("(allow file-read*)"), "got: {content}");
    let _ = std::fs::remove_file(path);
}

#[test]
fn macos_sbpl_includes_allow_write_entries() {
    let wrapped = wrap_with_sandbox(
        "ls",
        &cfg(vec!["/Users/u/repo", "/tmp/work"], vec![]),
        Platform::Mac,
    )
    .expect("wrap ok");
    let path = wrapped
        .strip_prefix("sandbox-exec -f ")
        .unwrap()
        .split(' ')
        .next()
        .unwrap();
    let content = std::fs::read_to_string(path).unwrap();
    assert!(
        content.contains(r#"(allow file-write* (regex "^/Users/u/repo"))"#),
        "missing first allow_write entry: {content}"
    );
    assert!(
        content.contains(r#"(allow file-write* (regex "^/tmp/work"))"#),
        "missing second allow_write entry: {content}"
    );
    let _ = std::fs::remove_file(path);
}

#[test]
fn macos_sbpl_includes_deny_read_entries() {
    let wrapped = wrap_with_sandbox("ls", &cfg(vec![], vec!["/private/etc"]), Platform::Mac)
        .expect("wrap ok");
    let path = wrapped
        .strip_prefix("sandbox-exec -f ")
        .unwrap()
        .split(' ')
        .next()
        .unwrap();
    let content = std::fs::read_to_string(path).unwrap();
    assert!(
        content.contains(r#"(deny file-read* (regex "^/private/etc"))"#),
        "missing deny_read entry: {content}"
    );
    let _ = std::fs::remove_file(path);
}

#[test]
fn macos_sbpl_no_network_unless_requested() {
    let wrapped = wrap_with_sandbox("ls", &cfg(vec![], vec![]), Platform::Mac).expect("wrap ok");
    let path = wrapped
        .strip_prefix("sandbox-exec -f ")
        .unwrap()
        .split(' ')
        .next()
        .unwrap();
    let content = std::fs::read_to_string(path).unwrap();
    assert!(
        !content.contains("(allow network*)"),
        "network should be denied by default: {content}"
    );
    let _ = std::fs::remove_file(path);
}

#[test]
fn macos_sbpl_network_when_domains_listed() {
    let mut policy = cfg(vec![], vec![]);
    policy.network.allowed_domains = vec!["api.example.com".to_string()];
    let wrapped = wrap_with_sandbox("curl", &policy, Platform::Mac).expect("wrap ok");
    let path = wrapped
        .strip_prefix("sandbox-exec -f ")
        .unwrap()
        .split(' ')
        .next()
        .unwrap();
    let content = std::fs::read_to_string(path).unwrap();
    assert!(
        content.contains("(allow network*)"),
        "expected network allow line: {content}"
    );
    let _ = std::fs::remove_file(path);
}
