use super::*;

#[test]
fn only_warp_gets_the_ghosting_notice() {
    assert!(ghosting_terminal_notice(Some("WarpTerminal")).is_some());
    assert!(ghosting_terminal_notice(Some("iTerm.app")).is_none());
    assert!(ghosting_terminal_notice(Some("Apple_Terminal")).is_none());
    assert!(ghosting_terminal_notice(Some("ghostty")).is_none());
    assert!(ghosting_terminal_notice(None).is_none());
}

fn argv_with_model(model: Option<&str>) -> Argv {
    Argv {
        model: model.map(String::from),
        ..Argv::default()
    }
}

/// SAFETY: the resolved DEFAULT model (no `--model`) is a current Claude 4
/// id, so the deprecation lookup returns `None` and startup prints nothing —
/// byte-identical to before this notice landed. This is the common-case
/// invariant the brief pins.
#[test]
fn default_model_yields_no_notice() {
    // Belt-and-suspenders: assert against the actual desktop default rather
    // than a hardcoded literal so a future default bump can't silently start
    // emitting a notice at every startup.
    let default_model = harness_runtime::desktop::DesktopConfig::default().default_model;
    assert!(
        harness_runtime::desktop::model_deprecation_warning(Some(&default_model)).is_none(),
        "the shipped desktop default model ({default_model}) must not be deprecated, \
         else every startup would emit a notice"
    );
    assert_eq!(startup_deprecation_notice(&argv_with_model(None)), None);
}

/// A `--model` override naming a CURRENT model still yields no notice.
#[test]
fn current_override_model_yields_no_notice() {
    assert_eq!(
        startup_deprecation_notice(&argv_with_model(Some("claude-opus-4-7"))),
        None
    );
}

/// A `--model` override naming a DEPRECATED model surfaces the exact
/// TS-faithful warning line (`deprecation.ts:100` text, byte-locked
/// including the leading `⚠ ` glyph). This is the only condition under which
/// startup emits anything. The provider env is cleared first so the
/// first-party retirement date is the one asserted (the table carries a
/// different date per provider; see `harness_runtime::desktop::model_deprecation_warning`).
#[test]
fn deprecated_override_model_yields_first_party_notice() {
    let prior = [
        (
            "CLAUDE_CODE_USE_BEDROCK",
            std::env::var_os("CLAUDE_CODE_USE_BEDROCK"),
        ),
        (
            "CLAUDE_CODE_USE_VERTEX",
            std::env::var_os("CLAUDE_CODE_USE_VERTEX"),
        ),
        (
            "CLAUDE_CODE_USE_FOUNDRY",
            std::env::var_os("CLAUDE_CODE_USE_FOUNDRY"),
        ),
    ];
    for (k, _) in &prior {
        std::env::remove_var(k);
    }

    let notice = startup_deprecation_notice(&argv_with_model(Some("claude-3-opus-20240229")))
        .expect("a deprecated --model must produce a startup notice");
    assert_eq!(
        notice,
        "⚠ Claude 3 Opus will be retired on January 5, 2026. Consider switching to a newer model."
    );

    // Restore any provider flags this test cleared.
    for (k, v) in prior {
        if let Some(v) = v {
            std::env::set_var(k, v);
        }
    }
}
