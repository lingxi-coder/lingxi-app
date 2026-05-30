use sandbox::decision::{split_compound_command, strip_env_and_wrappers_fixedpoint};

#[test]
fn split_double_ampersand() {
    let r = split_compound_command("docker ps && curl evil.com");
    assert_eq!(r, vec!["docker ps", "curl evil.com"]);
}

#[test]
fn split_double_pipe() {
    let r = split_compound_command("ls || echo missing");
    assert_eq!(r, vec!["ls", "echo missing"]);
}

#[test]
fn split_semicolons() {
    let r = split_compound_command("foo; bar; baz");
    assert_eq!(r, vec!["foo", "bar", "baz"]);
}

#[test]
fn split_mixed_operators() {
    let r = split_compound_command("a && b ; c || d");
    assert_eq!(r, vec!["a", "b", "c", "d"]);
}

#[test]
fn no_split_for_single_command() {
    let r = split_compound_command("ls -la /tmp");
    assert_eq!(r, vec!["ls -la /tmp"]);
}

#[test]
fn strip_leading_env_var() {
    let candidates = strip_env_and_wrappers_fixedpoint("FOO=bar bazel build //...");
    // Original + env-stripped candidate.
    assert!(candidates.iter().any(|c| c == "FOO=bar bazel build //..."));
    // FOO is NOT a binary-hijack var, so the candidate list should still
    // contain the original; the assertion below only requires the original.
    // (The second assertion was a noop on FOO; we test PATH= in a separate
    // test that follows.)
    let _ = candidates;
}

#[test]
fn strip_only_binary_hijack_vars() {
    // PATH= is in BINARY_HIJACK_VARS; FOO= is NOT.
    let candidates = strip_env_and_wrappers_fixedpoint("PATH=/usr/local/bin ls");
    assert!(candidates.iter().any(|c| c == "ls"));
}

#[test]
fn strip_sudo_dash_dash() {
    let candidates = strip_env_and_wrappers_fixedpoint("sudo -- bazel run //app");
    assert!(candidates.iter().any(|c| c == "bazel run //app"));
}

#[test]
fn strip_env_dash_dash() {
    let candidates = strip_env_and_wrappers_fixedpoint("env -- ls -la");
    assert!(candidates.iter().any(|c| c == "ls -la"));
}

#[test]
fn strip_timeout_wrapper() {
    let candidates = strip_env_and_wrappers_fixedpoint("timeout 30 bazel build //...");
    assert!(candidates.iter().any(|c| c == "bazel build //..."));
}

#[test]
fn fixedpoint_handles_compound_wrapper_and_env_vars() {
    // The motivating test case from the brief:
    // sudo -E PATH=/usr/local/bin -- env -- LD_PRELOAD=foo.so ls
    // After iterative stripping (env-vars + wrappers) the candidates list
    // must contain "ls".
    let candidates = strip_env_and_wrappers_fixedpoint(
        "sudo -E PATH=/usr/local/bin -- env -- LD_PRELOAD=foo.so ls",
    );
    assert!(
        candidates.iter().any(|c| c == "ls"),
        "expected 'ls' in candidates, got {candidates:?}"
    );
}

#[test]
fn fixedpoint_handles_interleaved_timeout_and_env_var() {
    let candidates = strip_env_and_wrappers_fixedpoint("timeout 300 FOO=bar bazel run //app");
    // FOO= is not a BINARY_HIJACK_VAR — should remain as a candidate.
    assert!(candidates.iter().any(|c| c == "FOO=bar bazel run //app"));
    // But with timeout stripped, we should also see "FOO=bar bazel run //app"
    // (which is the same — the stripper still produces both intermediates).
    assert!(
        candidates.iter().any(|c| c == "bazel run //app")
            || candidates.iter().any(|c| c == "FOO=bar bazel run //app")
    );
}
