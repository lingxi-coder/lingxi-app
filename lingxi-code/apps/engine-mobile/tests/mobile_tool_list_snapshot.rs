//! M8-P11 — lock the mobile tool set assembled by the composition root, and
//! assert it omits the desktop-only + device-control tools (the composition
//! roots must not drift into registering the same set).

#![allow(clippy::unwrap_used)]

use engine_mobile::mobile_tool_registry;
use traits::process::ProcessOutput;

#[test]
fn mobile_tool_list_snapshot() {
    let ctx = tool_api::test_support::shell_test_ctx(ProcessOutput {
        stdout: String::new(),
        stderr: String::new(),
        exit_code: 0,
        timed_out: false,
    });
    let reg = mobile_tool_registry(ctx);

    let mut names = reg.all_names();
    names.sort();

    // Desktop-only + device-control tools must NOT be in the mobile set.
    for forbidden in [
        "Bash",
        "PowerShell",
        "REPL",
        "MCP",
        "LSP",
        "Agent",
        "TeamCreate",
        "TeamDelete",
        "EnterWorktree",
        "ExitWorktree",
        "computer",
        "android_use",
        "ios_use",
        // Monitor's Bash/process-substitution contract is not portable to the
        // restricted mobile Shell runtime.
        "Monitor",
    ] {
        assert!(
            !names.iter().any(|n| n == forbidden),
            "mobile tool set must not include `{forbidden}`"
        );
    }
    // Mobile-exclusive tools must be present.
    for required in ["camera", "voice", "share"] {
        assert!(
            names.iter().any(|n| n == required),
            "mobile tool set must include `{required}`"
        );
    }

    insta::assert_yaml_snapshot!("mobile_tool_list", names);
}
