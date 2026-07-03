//! Keyboard-hint modifier labels with Mac-over-SSH detection (cc 2.1.198).
//!
//! Claude Code 2.1.198 shows `opt`/`cmd` (instead of `alt`/`super`) in key
//! hints when the *client* terminal is a Mac, even when the session runs on a
//! non-Mac host over SSH. The binary's platform probe (`Pct()` in the 2.1.198
//! bundle) is:
//!
//! ```js
//! function Pct(){let e=Wt();if(e==="macos")return e;
//!   if(Oe.LC_TERMINAL==="iTerm2"||Oe.TERM_PROGRAM==="Apple_Terminal"||
//!      Oe.TERM_PROGRAM==="iTerm.app")return"macos";return e}
//! ```
//!
//! i.e. local macOS wins; otherwise the client is treated as a Mac when the
//! terminal forwards `LC_TERMINAL=iTerm2` (iTerm2's SSH `SendEnv`) or
//! `TERM_PROGRAM` is `Apple_Terminal` / `iTerm.app`.
//!
//! The modifier display table (`nop` in the bundle) maps:
//! `alt → lower:"opt"/"alt", title:"Opt"/"Alt"` and
//! `super → lower:"cmd"/"super", title:"Cmd"/"Super"` depending on that
//! platform.

/// Pure form of the binary's `Pct()` platform probe: is the *client* terminal
/// a Mac? `local_is_macos` is the compile-target platform; the env values are
/// the forwarded terminal identity (`LC_TERMINAL`, `TERM_PROGRAM`).
#[must_use]
pub fn is_mac_like(
    local_is_macos: bool,
    lc_terminal: Option<&str>,
    term_program: Option<&str>,
) -> bool {
    if local_is_macos {
        return true;
    }
    lc_terminal == Some("iTerm2")
        || term_program == Some("Apple_Terminal")
        || term_program == Some("iTerm.app")
}

/// [`is_mac_like`] over the current process environment + compile target.
#[must_use]
pub fn detect_mac_like() -> bool {
    is_mac_like(
        cfg!(target_os = "macos"),
        std::env::var("LC_TERMINAL").ok().as_deref(),
        std::env::var("TERM_PROGRAM").ok().as_deref(),
    )
}

/// Title-case label for the Alt modifier (`Opt` on Mac-like clients).
/// (binary `nop.alt.title`)
#[must_use]
pub fn alt_label_title(mac_like: bool) -> &'static str {
    if mac_like {
        "Opt"
    } else {
        "Alt"
    }
}

/// Lower-case label for the Alt modifier (`opt` on Mac-like clients).
/// (binary `nop.alt.lower`)
#[must_use]
pub fn alt_label_lower(mac_like: bool) -> &'static str {
    if mac_like {
        "opt"
    } else {
        "alt"
    }
}

/// Title-case label for the Super modifier (`Cmd` on Mac-like clients).
/// (binary `nop.super.title`)
#[must_use]
pub fn super_label_title(mac_like: bool) -> &'static str {
    if mac_like {
        "Cmd"
    } else {
        "Super"
    }
}

/// Lower-case label for the Super modifier (`cmd` on Mac-like clients).
/// (binary `nop.super.lower`)
#[must_use]
pub fn super_label_lower(mac_like: bool) -> &'static str {
    if mac_like {
        "cmd"
    } else {
        "super"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_macos_is_mac_like_regardless_of_env() {
        assert!(is_mac_like(true, None, None));
        assert!(is_mac_like(true, Some("konsole"), Some("WezTerm")));
    }

    #[test]
    fn mac_over_ssh_detected_via_forwarded_terminal_identity() {
        // iTerm2 forwards LC_TERMINAL over SSH (binary: LC_TERMINAL==="iTerm2").
        assert!(is_mac_like(false, Some("iTerm2"), None));
        // Apple Terminal / iTerm via TERM_PROGRAM.
        assert!(is_mac_like(false, None, Some("Apple_Terminal")));
        assert!(is_mac_like(false, None, Some("iTerm.app")));
    }

    #[test]
    fn non_mac_client_is_not_mac_like() {
        assert!(!is_mac_like(false, None, None));
        assert!(!is_mac_like(false, Some("konsole"), Some("WezTerm")));
        // Substring/case must not match (binary uses strict equality).
        assert!(!is_mac_like(false, Some("iterm2"), Some("apple_terminal")));
    }

    #[test]
    fn modifier_labels_swap_on_mac_like() {
        assert_eq!(alt_label_title(true), "Opt");
        assert_eq!(alt_label_title(false), "Alt");
        assert_eq!(alt_label_lower(true), "opt");
        assert_eq!(alt_label_lower(false), "alt");
        assert_eq!(super_label_title(true), "Cmd");
        assert_eq!(super_label_title(false), "Super");
        assert_eq!(super_label_lower(true), "cmd");
        assert_eq!(super_label_lower(false), "super");
    }
}
