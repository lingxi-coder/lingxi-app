//! Tests for `argv.rs`, extracted from inline `#[cfg(test)]` blocks. Included via `#[path] mod argv_test;`.

use super::*;

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    /// claude-code 2.1.263 `ZDn(ERe.some(ue))` where
    /// `ue=(un)=>re.includes(un)||F.some((kn)=>Fr(kn).toolName===un)`. The two
    /// arms are NOT symmetric: `--tools` (`re`) is matched BARE, while
    /// `--allowedTools` (`F`) is rule-parsed first.
    #[test]
    fn todo_tools_opt_in_matches_tools_bare_and_allowed_tools_parsed() {
        fn opt_in(args: &[&str]) -> bool {
            let mut argv = vec!["lingxi-cli"];
            argv.extend_from_slice(args);
            Argv::from_iter(argv).unwrap().todo_tools_opt_in()
        }

        // Nothing named ⇒ no opt-in.
        assert!(!opt_in(&[]));
        assert!(!opt_in(&["--tools", "Bash", "Edit"]));
        assert!(!opt_in(&["--allowedTools", "Read", "Bash(git *)"]));

        // Each of the five bare names trips either list.
        for name in platform_api::session_flags::TODO_TOOL_NAMES {
            assert!(opt_in(&["--tools", name]), "--tools {name}");
            assert!(opt_in(&["--allowedTools", name]), "--allowedTools {name}");
        }

        // Comma-separated entries are split, matching the registry's own
        // `--tools` normalizer.
        assert!(opt_in(&["--tools", "Bash,TaskCreate"]));
        assert!(opt_in(&["--allowedTools", "Read,TaskList"]));

        // THE ASYMMETRY. A rule-shaped entry is parsed down to its tool name for
        // `--allowedTools` only; `--tools` compares the raw token.
        assert!(opt_in(&["--allowedTools", "TaskCreate(x)"]));
        assert!(!opt_in(&["--tools", "TaskCreate(x)"]));

        // A near-miss name must not trip it.
        assert!(!opt_in(&["--tools", "TaskStop"]));
        assert!(!opt_in(&["--tools", "TaskOutput"]));
    }

    // parity 2.1.207: `--permission-mode manual` is ACCEPTED (was hard-rejected
    // by the old fixed value_parser list). `manual` is the CLI alias for
    // `default`; both spellings parse.
    #[test]
    fn permission_mode_accepts_manual_alias() {
        let a = Argv::from_iter(["lingxi-cli", "--permission-mode", "manual"]).unwrap();
        assert_eq!(a.permission_mode.as_deref(), Some("manual"));
        // The hidden `default` alias still parses too.
        let b = Argv::from_iter(["lingxi-cli", "--permission-mode", "default"]).unwrap();
        assert_eq!(b.permission_mode.as_deref(), Some("default"));
        // Every other real mode still parses.
        for mode in [
            "acceptEdits",
            "auto",
            "bypassPermissions",
            "dontAsk",
            "plan",
        ] {
            let p = Argv::from_iter(["lingxi-cli", "--permission-mode", mode]).unwrap();
            assert_eq!(p.permission_mode.as_deref(), Some(mode));
        }
        // An out-of-set value is still hard-rejected.
        assert!(Argv::from_iter(["lingxi-cli", "--permission-mode", "banana"]).is_err());
    }

    #[test]
    fn no_args_is_repl_mode() {
        let a = Argv::from_iter(["lingxi-cli"]).unwrap();
        assert!(a.is_repl_mode());
        assert!(a.prompt.is_none());
    }

    #[test]
    fn version_uses_claude_short_flag_and_keeps_uppercase_alias() {
        for flag in ["-v", "-V", "--version"] {
            let error = Argv::from_iter(["lingxi-cli", flag]).unwrap_err();
            assert_eq!(error.kind(), clap::error::ErrorKind::DisplayVersion);
        }

        use clap::CommandFactory;
        let help = Argv::command().render_help().to_string();
        assert!(help.contains("-v, --version"));
        assert!(!help.contains("-V, --version"));
    }

    #[test]
    fn positional_prompt_is_oneshot() {
        let a = Argv::from_iter(["lingxi-cli", "fix the bug"]).unwrap();
        assert_eq!(a.prompt.as_deref(), Some("fix the bug"));
        assert!(!a.is_repl_mode());
    }

    #[test]
    fn print_flag_short_form() {
        let a = Argv::from_iter(["lingxi-cli", "-p", "hi"]).unwrap();
        assert!(a.print);
        assert_eq!(a.prompt.as_deref(), Some("hi"));
    }

    #[test]
    fn print_flag_long_form() {
        let a = Argv::from_iter(["lingxi-cli", "--print", "hi"]).unwrap();
        assert!(a.print);
    }

    #[test]
    fn resume_with_uuid() {
        let a = Argv::from_iter([
            "lingxi-cli",
            "--resume",
            "00000000-0000-0000-0000-000000000001",
        ])
        .unwrap();
        assert_eq!(
            a.resume.as_deref(),
            Some("00000000-0000-0000-0000-000000000001")
        );
        assert!(!a.is_repl_mode());
    }

    #[test]
    fn resume_without_value_enters_picker_mode() {
        let a = Argv::from_iter(["lingxi-cli", "--resume"]).unwrap();
        // Empty sentinel = picker.
        assert_eq!(a.resume.as_deref(), Some(""));
    }

    #[test]
    fn resume_short_alias_with_value() {
        let a =
            Argv::from_iter(["lingxi-cli", "-r", "00000000-0000-0000-0000-000000000001"]).unwrap();
        assert_eq!(
            a.resume.as_deref(),
            Some("00000000-0000-0000-0000-000000000001")
        );
        assert!(!a.is_repl_mode());
    }

    #[test]
    fn resume_short_alias_without_value_enters_picker() {
        // `-r` value is OPTIONAL (`[value]`): bare `-r` → empty picker sentinel.
        let a = Argv::from_iter(["lingxi-cli", "-r"]).unwrap();
        assert_eq!(a.resume.as_deref(), Some(""));
    }

    #[test]
    fn continue_long_flag() {
        let a = Argv::from_iter(["lingxi-cli", "--continue"]).unwrap();
        assert!(a.continue_session);
        // `--continue` reopens the most-recent conversation, not a fresh REPL.
        assert!(!a.is_repl_mode());
    }

    #[test]
    fn continue_short_flag() {
        let a = Argv::from_iter(["lingxi-cli", "-c"]).unwrap();
        assert!(a.continue_session);
    }

    #[test]
    fn continue_default_false() {
        let a = Argv::from_iter(["lingxi-cli", "hi"]).unwrap();
        assert!(!a.continue_session);
    }

    #[test]
    fn fork_session_flag() {
        let a = Argv::from_iter(["lingxi-cli", "--resume", "--fork-session"]).unwrap();
        assert!(a.fork_session);
        assert_eq!(a.resume.as_deref(), Some(""));
    }

    #[test]
    fn fork_session_default_false() {
        let a = Argv::from_iter(["lingxi-cli", "hi"]).unwrap();
        assert!(!a.fork_session);
    }

    #[test]
    fn continue_and_fork_together() {
        let a = Argv::from_iter(["lingxi-cli", "-c", "--fork-session"]).unwrap();
        assert!(a.continue_session && a.fork_session);
        assert!(!a.is_repl_mode());
    }

    #[test]
    fn model_flag() {
        let a = Argv::from_iter(["lingxi-cli", "--model", "claude-sonnet-4-6", "hi"]).unwrap();
        assert_eq!(a.model.as_deref(), Some("claude-sonnet-4-6"));
    }

    #[test]
    fn fallback_model_flag() {
        let a = Argv::from_iter([
            "lingxi-cli",
            "--print",
            "--fallback-model",
            "claude-sonnet-4-6",
            "hi",
        ])
        .unwrap();
        assert_eq!(a.fallback_model.as_deref(), Some("claude-sonnet-4-6"));
    }

    #[test]
    fn fallback_model_default_none() {
        let a = Argv::from_iter(["lingxi-cli", "hi"]).unwrap();
        assert!(a.fallback_model.is_none());
    }

    #[test]
    fn fallback_model_normalizes_ordered_csv_and_rejects_empty_entries() {
        let a = Argv::from_iter([
            "lingxi-cli",
            "--fallback-model",
            " sonnet , haiku,sonnet ",
            "hi",
        ])
        .unwrap();
        assert_eq!(a.fallback_model.as_deref(), Some("sonnet,haiku"));
        assert!(
            Argv::from_iter(["lingxi-cli", "--fallback-model", "sonnet,,haiku", "hi"]).is_err()
        );
    }

    #[test]
    fn fallback_model_accepted_without_print() {
        // Soft restriction (parity with claude-code): the flag PARSES regardless
        // of --print; honoring is deferred to the print/non-interactive consumer.
        let a =
            Argv::from_iter(["lingxi-cli", "--fallback-model", "claude-sonnet-4-6", "hi"]).unwrap();
        assert_eq!(a.fallback_model.as_deref(), Some("claude-sonnet-4-6"));
    }

    #[test]
    fn max_turns_flag_parses() {
        let a = Argv::from_iter(["lingxi-cli", "--print", "--max-turns", "5", "hi"]).unwrap();
        assert_eq!(a.max_turns, Some(5));
    }

    #[test]
    fn max_turns_default_none() {
        let a = Argv::from_iter(["lingxi-cli", "hi"]).unwrap();
        assert!(a.max_turns.is_none());
    }

    #[test]
    fn max_budget_usd_flag_parses() {
        let a =
            Argv::from_iter(["lingxi-cli", "--print", "--max-budget-usd", "2.5", "hi"]).unwrap();
        assert_eq!(a.max_budget_usd, Some(2.5));
    }

    #[test]
    fn max_budget_usd_rejects_zero_and_negative() {
        // Parity with claude-code: the arg parser rejects `amount <= 0`.
        assert!(Argv::from_iter(["lingxi-cli", "--max-budget-usd", "0", "hi"]).is_err());
        assert!(Argv::from_iter(["lingxi-cli", "--max-budget-usd", "-1", "hi"]).is_err());
    }

    #[test]
    fn max_budget_usd_rejects_non_numeric() {
        // A non-numeric value (JS `Number(...)` → `NaN`) is rejected too.
        assert!(Argv::from_iter(["lingxi-cli", "--max-budget-usd", "abc", "hi"]).is_err());
    }

    #[test]
    fn max_budget_usd_rejects_infinity_forms() {
        // Rust's `f64::FromStr` parses these all to `f64::INFINITY`, which is
        // neither NaN nor `<= 0`. Mirror JS: `Number("inf")`/`Number("INF")` are
        // NaN and `Number("1e400")` is Infinity — all rejected by the finiteness
        // guard so the cost cap can never be unbounded.
        assert!(Argv::from_iter(["lingxi-cli", "--max-budget-usd", "inf", "hi"]).is_err());
        assert!(Argv::from_iter(["lingxi-cli", "--max-budget-usd", "INF", "hi"]).is_err());
        assert!(Argv::from_iter(["lingxi-cli", "--max-budget-usd", "Infinity", "hi"]).is_err());
        assert!(Argv::from_iter(["lingxi-cli", "--max-budget-usd", "1e400", "hi"]).is_err());
    }

    #[test]
    fn json_schema_flag_parses() {
        let a = Argv::from_iter([
            "lingxi-cli",
            "--print",
            "--json-schema",
            r#"{"type":"object"}"#,
            "hi",
        ])
        .unwrap();
        assert_eq!(a.json_schema.as_deref(), Some(r#"{"type":"object"}"#));
    }

    #[test]
    fn json_schema_default_none() {
        let a = Argv::from_iter(["lingxi-cli", "hi"]).unwrap();
        assert!(a.json_schema.is_none());
    }

    #[test]
    fn cwd_flag() {
        let a = Argv::from_iter(["lingxi-cli", "--cwd", "/tmp", "hi"]).unwrap();
        assert_eq!(a.cwd, Some(PathBuf::from("/tmp")));
    }

    #[test]
    fn no_stream_flag() {
        let a = Argv::from_iter(["lingxi-cli", "--no-stream", "hi"]).unwrap();
        assert!(a.no_stream);
    }

    #[test]
    fn json_flag() {
        let a = Argv::from_iter(["lingxi-cli", "--json", "hi"]).unwrap();
        assert!(a.json);
    }

    #[test]
    fn debug_flag() {
        // Bare `--debug` with no following token → on, unfiltered (Some("")).
        let bare = Argv::from_iter(["lingxi-cli", "--debug"]).unwrap();
        assert!(bare.debug.is_some());
        assert!(bare.debug_enabled());
        assert_eq!(bare.debug_filter(), None);
        // SPACE form (matches commander `-d, --debug [filter]`): `--debug api,hooks`
        // consumes the next token as the optional FILTER, leaving no prompt — so a
        // user copying `claude --debug api,hooks` gets the same parse here.
        let a = Argv::from_iter(["lingxi-cli", "--debug", "api,hooks"]).unwrap();
        assert_eq!(a.debug_filter(), Some("api,hooks"));
        assert_eq!(
            a.prompt, None,
            "--debug consumes the next token as the filter, not the prompt"
        );
        // The `=` form still binds the value too.
        let b = Argv::from_iter(["lingxi-cli", "--debug=api,hooks"]).unwrap();
        assert_eq!(b.debug.as_deref(), Some("api,hooks"));
        assert_eq!(b.debug_filter(), Some("api,hooks"));
        // Short alias `-d` (space and `=` forms).
        let c = Argv::from_iter(["lingxi-cli", "-d=scope"]).unwrap();
        assert_eq!(c.debug.as_deref(), Some("scope"));
        assert!(c.debug_enabled());
        let c2 = Argv::from_iter(["lingxi-cli", "-d", "scope"]).unwrap();
        assert_eq!(c2.debug_filter(), Some("scope"));
        // Off by default.
        let d = Argv::from_iter(["lingxi-cli", "hi"]).unwrap();
        assert!(d.debug.is_none());
        assert!(!d.debug_enabled());
    }

    #[test]
    fn no_tui_flag() {
        let a = Argv::from_iter(["lingxi-cli", "--no-tui"]).unwrap();
        assert!(a.no_tui);
    }

    #[test]
    fn no_tui_flag_default_false() {
        let a = Argv::from_iter(["lingxi-cli"]).unwrap();
        assert!(!a.no_tui);
    }

    #[test]
    fn dangerously_skip_permissions_flag_parses() {
        let a = Argv::from_iter(["lingxi-cli", "--dangerously-skip-permissions"]).unwrap();
        assert!(a.dangerously_skip_permissions);
    }

    #[test]
    fn permission_mode_flag_parses() {
        let a = Argv::from_iter(["lingxi-cli", "--permission-mode", "plan"]).unwrap();
        assert_eq!(a.permission_mode.as_deref(), Some("plan"));
        let b = Argv::from_iter(["lingxi-cli"]).unwrap();
        assert!(b.permission_mode.is_none());
        assert!(!b.dangerously_skip_permissions);
    }

    #[test]
    fn unknown_flag_errors() {
        let r = Argv::from_iter(["lingxi-cli", "--nonexistent"]);
        assert!(r.is_err());
    }

    #[test]
    fn all_flags_together() {
        let a = Argv::from_iter([
            "lingxi-cli",
            "--print",
            "--no-stream",
            "--json",
            "--debug",
            "--no-tui",
            "--cwd",
            "/r",
            "--model",
            "claude-opus-4-7",
            "--resume",
            "00000000-0000-0000-0000-000000000001",
            "fix it",
        ])
        .unwrap();
        assert!(a.print && a.no_stream && a.json && a.debug.is_some() && a.no_tui);
        assert_eq!(a.cwd, Some(PathBuf::from("/r")));
        assert_eq!(a.model.as_deref(), Some("claude-opus-4-7"));
        assert_eq!(
            a.resume.as_deref(),
            Some("00000000-0000-0000-0000-000000000001")
        );
        assert_eq!(a.prompt.as_deref(), Some("fix it"));
    }

    // ── New flag parse tests (v2.1.186 parity) ───────────────────────────────

    #[test]
    fn output_format_text_parses() {
        let a =
            Argv::from_iter(["lingxi-cli", "--print", "--output-format", "text", "hi"]).unwrap();
        assert_eq!(a.output_format.as_deref(), Some("text"));
        assert!(!a.is_json_output());
    }

    #[test]
    fn output_format_json_parses_and_activates_json_output() {
        let a =
            Argv::from_iter(["lingxi-cli", "--print", "--output-format", "json", "hi"]).unwrap();
        assert_eq!(a.output_format.as_deref(), Some("json"));
        assert!(a.is_json_output());
    }

    #[test]
    fn output_format_stream_json_parses_and_activates_stream_json() {
        let a = Argv::from_iter([
            "lingxi-cli",
            "--print",
            "--output-format",
            "stream-json",
            "hi",
        ])
        .unwrap();
        assert_eq!(a.output_format.as_deref(), Some("stream-json"));
        // stream-json routes through StreamJsonStream, NOT the SinkAdapter/JsonSink path.
        assert!(
            a.is_stream_json(),
            "is_stream_json() must be true for stream-json"
        );
        assert!(
            !a.is_json_output(),
            "is_json_output() must be false for stream-json (it has its own path)"
        );
    }

    #[test]
    fn output_format_invalid_errors() {
        let r = Argv::from_iter(["lingxi-cli", "--output-format", "xml"]);
        assert!(r.is_err());
    }

    #[test]
    fn json_flag_activates_json_output() {
        let a = Argv::from_iter(["lingxi-cli", "--json", "hi"]).unwrap();
        assert!(a.is_json_output());
    }

    #[test]
    fn no_output_format_or_json_is_not_json_output() {
        let a = Argv::from_iter(["lingxi-cli", "hi"]).unwrap();
        assert!(!a.is_json_output());
    }

    #[test]
    fn input_format_text_parses() {
        let a = Argv::from_iter(["lingxi-cli", "--print", "--input-format", "text", "hi"]).unwrap();
        assert_eq!(a.input_format.as_deref(), Some("text"));
    }

    #[test]
    fn input_format_stream_json_parses() {
        let a = Argv::from_iter([
            "lingxi-cli",
            "--print",
            "--input-format",
            "stream-json",
            "hi",
        ])
        .unwrap();
        assert_eq!(a.input_format.as_deref(), Some("stream-json"));
    }

    #[test]
    fn system_prompt_parses() {
        let a = Argv::from_iter([
            "lingxi-cli",
            "--print",
            "--system-prompt",
            "You are helpful.",
            "hi",
        ])
        .unwrap();
        assert_eq!(a.system_prompt.as_deref(), Some("You are helpful."));
    }

    #[test]
    fn append_system_prompt_parses() {
        let a = Argv::from_iter([
            "lingxi-cli",
            "--print",
            "--append-system-prompt",
            "Also be concise.",
            "hi",
        ])
        .unwrap();
        assert_eq!(a.append_system_prompt.as_deref(), Some("Also be concise."));
    }

    #[test]
    fn system_prompt_file_parses() {
        let a = Argv::from_iter([
            "lingxi-cli",
            "--system-prompt-file",
            "/tmp/prompt.txt",
            "hi",
        ])
        .unwrap();
        assert_eq!(a.system_prompt_file, Some(PathBuf::from("/tmp/prompt.txt")));
    }

    #[test]
    fn append_system_prompt_file_parses() {
        let a = Argv::from_iter([
            "lingxi-cli",
            "--append-system-prompt-file",
            "/tmp/append.txt",
            "hi",
        ])
        .unwrap();
        assert_eq!(
            a.append_system_prompt_file,
            Some(PathBuf::from("/tmp/append.txt"))
        );
    }

    #[test]
    fn allowed_tools_parses() {
        // Positional prompt before multi-value flag to avoid greedy consumption.
        let a =
            Argv::from_iter(["lingxi-cli", "fix it", "--allowed-tools", "Bash", "Edit"]).unwrap();
        let tools = a.allowed_tools.unwrap();
        assert_eq!(tools, &["Bash", "Edit"]);
    }

    #[test]
    fn allowed_tools_alias_parses() {
        let a = Argv::from_iter(["lingxi-cli", "fix it", "--allowedTools", "Bash"]).unwrap();
        let tools = a.allowed_tools.unwrap();
        assert_eq!(tools, &["Bash"]);
    }

    #[test]
    fn disallowed_tools_parses() {
        let a = Argv::from_iter(["lingxi-cli", "fix it", "--disallowed-tools", "Bash"]).unwrap();
        let tools = a.disallowed_tools.unwrap();
        assert_eq!(tools, &["Bash"]);
    }

    #[test]
    fn disallowed_tools_alias_parses() {
        let a = Argv::from_iter(["lingxi-cli", "fix it", "--disallowedTools", "Edit"]).unwrap();
        let tools = a.disallowed_tools.unwrap();
        assert_eq!(tools, &["Edit"]);
    }

    #[test]
    fn tools_parses() {
        let a = Argv::from_iter(["lingxi-cli", "fix it", "--tools", "Bash", "Read"]).unwrap();
        let tools = a.tools.unwrap();
        assert_eq!(tools, &["Bash", "Read"]);
    }

    #[test]
    fn restricted_flag_and_help_are_exposed() {
        let a = Argv::from_iter(["lingxi-cli", "--restricted", "fix it"]).unwrap();
        assert!(a.restricted);
        assert!(a.restricted_enabled());
        let help = Argv::command().render_help().to_string();
        assert!(help.contains("Restricted mode: removes the built-in tools"));
        assert!(help.contains("bypassPermissions"));
        assert!(help.contains("tool-configuration files."));
        assert!(help.contains("--restricted"));
    }

    #[test]
    fn add_dir_single_parses() {
        let a = Argv::from_iter(["lingxi-cli", "fix it", "--add-dir", "/extra/docs"]).unwrap();
        let dirs = a.add_dir.unwrap();
        assert_eq!(dirs, &[PathBuf::from("/extra/docs")]);
    }

    #[test]
    fn add_dir_multiple_parses() {
        let a = Argv::from_iter(["lingxi-cli", "fix it", "--add-dir", "/a", "/b"]).unwrap();
        let dirs = a.add_dir.unwrap();
        assert_eq!(dirs, &[PathBuf::from("/a"), PathBuf::from("/b")]);
    }

    #[test]
    fn settings_parses() {
        let a = Argv::from_iter(["lingxi-cli", "--settings", r#"{"verbose":true}"#, "hi"]).unwrap();
        assert_eq!(a.settings.as_deref(), Some(r#"{"verbose":true}"#));
    }

    #[test]
    fn mcp_config_parses() {
        let a =
            Argv::from_iter(["lingxi-cli", "fix it", "--mcp-config", "/path/mcp.json"]).unwrap();
        let cfg = a.mcp_config.unwrap();
        assert_eq!(cfg, &["/path/mcp.json"]);
    }

    #[test]
    fn verbose_flag_parses() {
        let a = Argv::from_iter(["lingxi-cli", "--verbose", "hi"]).unwrap();
        assert!(a.verbose);
    }

    #[test]
    fn bare_flag_parses() {
        let a = Argv::from_iter(["lingxi-cli", "--bare", "hi"]).unwrap();
        assert!(a.bare);
    }

    #[test]
    fn safe_mode_flag_parses() {
        let a = Argv::from_iter(["lingxi-cli", "--safe-mode", "hi"]).unwrap();
        assert!(a.safe_mode);
    }

    #[test]
    fn agents_parses() {
        // claude `--agents <json>` is a SINGLE value (a JSON object string),
        // not a space-separated list.
        let json = r#"{"reviewer":{"description":"Reviews code","prompt":"You are a reviewer"}}"#;
        let a = Argv::from_iter(["lingxi-cli", "fix it", "--agents", json]).unwrap();
        assert_eq!(a.agents.as_deref(), Some(json));
    }

    #[test]
    fn agent_parses() {
        // claude `--agent <agent>` is a SINGLE value.
        let a = Argv::from_iter(["lingxi-cli", "fix it", "--agent", "coder"]).unwrap();
        assert_eq!(a.agent.as_deref(), Some("coder"));
    }

    // ── v2.1.191 parity: new flags parse + choices enforcement ───────────────

    #[test]
    fn session_id_parses() {
        let a = Argv::from_iter([
            "lingxi-cli",
            "--session-id",
            "00000000-0000-0000-0000-000000000001",
            "hi",
        ])
        .unwrap();
        assert_eq!(
            a.session_id.as_deref(),
            Some("00000000-0000-0000-0000-000000000001")
        );
    }

    #[test]
    fn name_short_and_long_parse() {
        let a = Argv::from_iter(["lingxi-cli", "-n", "my session", "hi"]).unwrap();
        assert_eq!(a.name.as_deref(), Some("my session"));
        let b = Argv::from_iter(["lingxi-cli", "--name", "other", "hi"]).unwrap();
        assert_eq!(b.name.as_deref(), Some("other"));
    }

    #[test]
    fn setting_sources_strict_mcp_exclude_dynamic_parse() {
        let a = Argv::from_iter([
            "lingxi-cli",
            "--setting-sources",
            "user,project",
            "--strict-mcp-config",
            "--exclude-dynamic-system-prompt-sections",
            "hi",
        ])
        .unwrap();
        assert_eq!(a.setting_sources.as_deref(), Some("user,project"));
        assert!(a.strict_mcp_config);
        assert!(a.exclude_dynamic_system_prompt_sections);
    }

    #[test]
    fn previously_hard_erroring_flags_now_accepted() {
        // The whole batch that used to hard-error must now parse cleanly.
        // Positional prompt FIRST so the greedy multi-value `--file` (num_args
        // 1..) doesn't swallow it (same convention as allowed_tools_parses).
        let a = Argv::from_iter([
            "lingxi-cli",
            "hi",
            "--mcp-debug",
            "--ide",
            "--allow-dangerously-skip-permissions",
            "--disable-slash-commands",
            "--chrome",
            "--ax-screen-reader",
            "--permission-prompt-tool",
            "mcp__perm__prompt",
            "--file",
            "file_abc:doc.txt",
            "file_def:img.png",
        ])
        .unwrap();
        assert_eq!(a.prompt.as_deref(), Some("hi"));
        assert!(a.mcp_debug && a.ide && a.allow_dangerously_skip_permissions);
        assert!(a.disable_slash_commands && a.chrome && a.ax_screen_reader);
        assert_eq!(
            a.permission_prompt_tool.as_deref(),
            Some("mcp__perm__prompt")
        );
        assert_eq!(a.file.as_deref().map(<[String]>::len), Some(2));
    }

    #[test]
    fn no_chrome_parses() {
        let a = Argv::from_iter(["lingxi-cli", "--no-chrome", "hi"]).unwrap();
        assert!(a.no_chrome);
        assert!(!a.chrome);
    }

    #[test]
    fn chrome_flags_are_mutually_exclusive() {
        assert!(Argv::from_iter(["lingxi-cli", "--chrome", "--no-chrome", "hi"]).is_err());
    }

    #[test]
    fn remote_control_flag_accepts_optional_name() {
        let unnamed = Argv::from_iter(["lingxi-cli", "--remote-control"]).unwrap();
        assert_eq!(unnamed.remote_control.as_deref(), Some(""));
        let named = Argv::from_iter(["lingxi-cli", "--remote-control", "desk"]).unwrap();
        assert_eq!(named.remote_control.as_deref(), Some("desk"));
    }

    #[test]
    fn worktree_optional_value() {
        let a = Argv::from_iter(["lingxi-cli", "--worktree", "feature-x"]).unwrap();
        assert_eq!(a.worktree.as_deref(), Some("feature-x"));
        // Bare `-w` → empty sentinel (auto-named).
        let b = Argv::from_iter(["lingxi-cli", "-w"]).unwrap();
        assert_eq!(b.worktree.as_deref(), Some(""));
    }

    #[test]
    fn tmux_requires_equals_value() {
        // `--tmux` alone → native (empty sentinel).
        let a = Argv::from_iter(["lingxi-cli", "--tmux", "-w", "wt"]).unwrap();
        assert_eq!(a.tmux.as_deref(), Some(""));
        // `--tmux=classic` → classic.
        let b = Argv::from_iter(["lingxi-cli", "--tmux=classic", "-w", "wt"]).unwrap();
        assert_eq!(b.tmux.as_deref(), Some("classic"));
    }

    #[test]
    fn background_and_bg_alias_parse() {
        let a = Argv::from_iter(["lingxi-cli", "--background", "hi"]).unwrap();
        assert!(a.background);
        let b = Argv::from_iter(["lingxi-cli", "--bg", "hi"]).unwrap();
        assert!(b.background);
    }

    // (M-01 cc2.1.215) `--brief` enables the SendUserMessage (Brief) tool. It is
    // a plain bool flag, default-off (absent = tool invisible to the model).
    #[test]
    fn brief_flag_parses_and_defaults_off() {
        let off = Argv::from_iter(["lingxi-cli", "hi"]).unwrap();
        assert!(!off.brief, "--brief defaults off");
        let on = Argv::from_iter(["lingxi-cli", "--brief", "hi"]).unwrap();
        assert!(on.brief);
    }

    #[test]
    fn optional_value_flag_consumes_next_token_like_commander() {
        // Parity (review P1): an optional-value global flag (`--debug`, `--from-pr`,
        // `--resume`) BEFORE a subcommand-name token consumes that token as its
        // OWN value, exactly like commander does in the real binary. Verified
        // side-by-side: `claude --debug auth` does NOT route to the `auth`
        // subcommand — `auth` is eaten as the debug filter (and it then errors
        // "Input must be provided", not a billable turn). So here too the next
        // token binds as the value and `command` stays None.
        let dbg = Argv::from_iter(["lingxi-cli", "--debug", "auth"]).unwrap();
        assert!(
            dbg.command.is_none(),
            "--debug consumes `auth` as the filter, like commander"
        );
        assert_eq!(dbg.debug_filter(), Some("auth"));
        let fp = Argv::from_iter(["lingxi-cli", "--from-pr", "auth"]).unwrap();
        assert!(
            fp.command.is_none(),
            "--from-pr consumes `auth` as its value, like commander"
        );
        assert_eq!(fp.from_pr.as_deref(), Some("auth"));
        // `-r mcp` is likewise "resume, search 'mcp'" → the resume PICKER (never a
        // billable chat turn). command stays None; resume is set.
        let r = Argv::from_iter(["lingxi-cli", "-r", "mcp"]).unwrap();
        assert!(r.command.is_none() && r.resume.as_deref() == Some("mcp"));
        // A subcommand-name token as the FIRST argument still routes to the
        // subcommand (no optional-value flag precedes it to eat it).
        assert!(Argv::from_iter(["lingxi-cli", "auth", "status"])
            .unwrap()
            .command
            .is_some());
        // Controls: a normal prompt stays command=None; the `=` filter works.
        assert!(Argv::from_iter(["lingxi-cli", "fix the bug"])
            .unwrap()
            .command
            .is_none());
        assert_eq!(
            Argv::from_iter(["lingxi-cli", "--debug=api,hooks"])
                .unwrap()
                .debug
                .as_deref(),
            Some("api,hooks"),
            "--debug=<filter> still binds the value"
        );
    }

    #[test]
    fn permission_mode_rejects_unknown_choice() {
        // claude commander `.choices(...)` hard-rejects out-of-list values.
        assert!(Argv::from_iter(["lingxi-cli", "--permission-mode", "bogus", "hi"]).is_err());
        // `auto` is a real choice in 2.1.191.
        let a = Argv::from_iter(["lingxi-cli", "--permission-mode", "auto", "hi"]).unwrap();
        assert_eq!(a.permission_mode.as_deref(), Some("auto"));
    }

    #[test]
    fn prompt_suggestions_rejects_invalid_choice() {
        assert!(Argv::from_iter(["lingxi-cli", "--prompt-suggestions", "banana", "hi"]).is_err());
        // Valid choices + bare preset still work.
        assert_eq!(
            Argv::from_iter(["lingxi-cli", "--prompt-suggestions", "off", "hi"])
                .unwrap()
                .prompt_suggestions
                .as_deref(),
            Some("off")
        );
        assert_eq!(
            Argv::from_iter(["lingxi-cli", "--prompt-suggestions"])
                .unwrap()
                .prompt_suggestions
                .as_deref(),
            Some("true")
        );
    }

    /// (M4 cc2.1.198) `--plugin-dir` is REPEATABLE (commander help: "Load a
    /// plugin from a directory or .zip for this session only (repeatable:
    /// --plugin-dir A --plugin-dir B.zip) (default: [])").
    #[test]
    fn plugin_dir_parses() {
        let a = Argv::from_iter(["lingxi-cli", "--plugin-dir", "/my/plugins", "hi"]).unwrap();
        assert_eq!(a.plugin_dir, vec![PathBuf::from("/my/plugins")]);
        // Repeatable — order preserved; zip paths are plain values here.
        let b = Argv::from_iter([
            "lingxi-cli",
            "--plugin-dir",
            "/a",
            "--plugin-dir",
            "/b.zip",
            "hi",
        ])
        .unwrap();
        assert_eq!(
            b.plugin_dir,
            vec![PathBuf::from("/a"), PathBuf::from("/b.zip")]
        );
        // Default: empty list (commander `(default: [])`).
        assert!(Argv::from_iter(["lingxi-cli", "hi"])
            .unwrap()
            .plugin_dir
            .is_empty());
    }

    /// (M4 cc2.1.198) `--effort` argParser port (`u4i`/`Xat`): trim+lowercase,
    /// `med`→`medium` alias, membership in UR; unknown value → `None` + the
    /// byte-locked stderr warning (note the em dash), binary verified live.
    #[test]
    fn effort_normalizes_and_warns() {
        // Valid levels normalize (trim + lowercase).
        let a = Argv::from_iter(["lingxi-cli", "--effort", "  HIGH ", "hi"]).unwrap();
        assert_eq!(a.normalized_effort(), (Some("high".to_string()), None));
        // Alias `med` → `medium` (`c4i = {med:"medium"}`).
        let b = Argv::from_iter(["lingxi-cli", "--effort", "med", "hi"]).unwrap();
        assert_eq!(b.normalized_effort(), (Some("medium".to_string()), None));
        // Unknown → ignored with the byte-locked warning.
        let c = Argv::from_iter(["lingxi-cli", "--effort", "banana", "hi"]).unwrap();
        let (level, warning) = c.normalized_effort();
        assert_eq!(level, None);
        assert_eq!(
            warning.as_deref(),
            Some("Warning: Unknown --effort value 'banana' \u{2014} ignoring it and using the default effort. Valid values: low, medium, high, xhigh, max.")
        );
        // Absent flag → no level, no warning.
        let d = Argv::from_iter(["lingxi-cli", "hi"]).unwrap();
        assert_eq!(d.normalized_effort(), (None, None));
    }

    /// (M4 cc2.1.198) `--prompt-suggestions` maps to a BOOLEAN like the
    /// binary's argParser (`!Hl(i)`), and a truthy value outside
    /// `--print` + `--output-format=stream-json` trips the byte-locked fatal
    /// (verified live on the 2.1.198 binary: stderr line + exit 1;
    /// `--prompt-suggestions false` passes in any mode).
    #[test]
    fn prompt_suggestions_gate_and_bool_mapping() {
        let locked = "--prompt-suggestions requires --print and --output-format=stream-json (prompt_suggestion messages are only surfaced in stream-json output).";
        // Bare flag → preset "true" → enabled → rejected without print/stream-json.
        // (A following non-dash token would bind as the optional VALUE — same
        // greediness as commander's `[value]` — so the prompt is omitted here.)
        let a = Argv::from_iter(["lingxi-cli", "--prompt-suggestions"]).unwrap();
        assert_eq!(a.prompt_suggestions_enabled(), Some(true));
        assert_eq!(a.validate_prompt_suggestions_args().unwrap_err(), locked);
        // Truthy token + print but text output → still rejected.
        let b = Argv::from_iter(["lingxi-cli", "-p", "--prompt-suggestions", "yes", "hi"]).unwrap();
        assert_eq!(b.validate_prompt_suggestions_args().unwrap_err(), locked);
        // print + stream-json → accepted (prompt BEFORE the optional-value
        // flag so it isn't bound as the flag's value).
        let c = Argv::from_iter([
            "lingxi-cli",
            "hi",
            "-p",
            "--output-format",
            "stream-json",
            "--verbose",
            "--prompt-suggestions",
        ])
        .unwrap();
        assert!(c.validate_prompt_suggestions_args().is_ok());
        // Falsy tokens map to false and pass anywhere (binary `a.promptSuggestions`
        // is boolean false → the Es gate is skipped).
        for tok in ["false", "0", "no", "off"] {
            let d = Argv::from_iter(["lingxi-cli", "--prompt-suggestions", tok, "hi"]).unwrap();
            assert_eq!(d.prompt_suggestions_enabled(), Some(false), "{tok}");
            assert!(d.validate_prompt_suggestions_args().is_ok(), "{tok}");
        }
        // Absent → None, always passes.
        let e = Argv::from_iter(["lingxi-cli", "hi"]).unwrap();
        assert_eq!(e.prompt_suggestions_enabled(), None);
        assert!(e.validate_prompt_suggestions_args().is_ok());
    }

    #[test]
    fn no_session_persistence_flag_parses() {
        let a = Argv::from_iter(["lingxi-cli", "--no-session-persistence", "hi"]).unwrap();
        assert!(a.no_session_persistence);
    }

    /// (M3 cc2.1.198) `--no-session-persistence` requires `--print`. Byte-locked
    /// to the binary's main action @223929381 (`Es("Error: --no-session-
    /// persistence can only be used with --print mode.")`); the method returns
    /// the string sans `Error: ` prefix (the caller prints `Error: {msg}`).
    #[test]
    fn no_session_persistence_requires_print_mode() {
        // Interactive (no --print) ⟶ byte-exact rejection.
        let a = Argv::from_iter(["lingxi-cli", "--no-session-persistence", "hi"]).unwrap();
        assert_eq!(
            a.validate_session_persistence_args().unwrap_err(),
            "--no-session-persistence can only be used with --print mode."
        );
        // With --print ⟶ accepted.
        let b =
            Argv::from_iter(["lingxi-cli", "--print", "--no-session-persistence", "hi"]).unwrap();
        assert!(b.validate_session_persistence_args().is_ok());
        // Flag absent ⟶ always fine, print or not.
        let c = Argv::from_iter(["lingxi-cli", "hi"]).unwrap();
        assert!(c.validate_session_persistence_args().is_ok());
    }

    /// (C5) `--plan-mode-instructions` requires `--print`; mirrors the
    /// `--no-session-persistence` gate (message sans `Error: ` prefix).
    #[test]
    fn plan_mode_instructions_requires_print_mode() {
        // Set without --print ⟶ byte-exact rejection.
        let a = Argv::from_iter(["lingxi-cli", "--plan-mode-instructions", "BODY", "hi"]).unwrap();
        assert_eq!(
            a.validate_plan_mode_instructions_args().unwrap_err(),
            "--plan-mode-instructions can only be used with --print mode."
        );
        // With --print ⟶ accepted.
        let b = Argv::from_iter([
            "lingxi-cli",
            "--print",
            "--plan-mode-instructions",
            "BODY",
            "hi",
        ])
        .unwrap();
        assert!(b.validate_plan_mode_instructions_args().is_ok());
        // Flag absent ⟶ always fine, print or not.
        let c = Argv::from_iter(["lingxi-cli", "hi"]).unwrap();
        assert!(c.validate_plan_mode_instructions_args().is_ok());
    }

    /// (M3 cc2.1.198) `--bg`/`--background` × `--print`/`-p` rejected up front.
    /// Message byte-locked to the bg fast-path validator `pof` @218854391
    /// (stderr line, no `Error:` prefix, exit 1) — note the real em dash.
    #[test]
    fn background_with_print_rejected_up_front() {
        let locked = "--bg and --print conflict: --print never starts the interactive session that `lingxi-cli agents` attaches to, so the job would be unattachable. The prompt is the positional \u{2014} drop --print: `lingxi-cli --bg '<task>'`.";
        // Every spelling pair conflicts: long/alias × long/short.
        for args in [
            ["lingxi-cli", "--bg", "--print", "task"],
            ["lingxi-cli", "--background", "--print", "task"],
            ["lingxi-cli", "--bg", "-p", "task"],
            ["lingxi-cli", "-p", "--background", "task"],
        ] {
            let a = Argv::from_iter(args).unwrap();
            assert_eq!(
                a.validate_background_args().unwrap_err(),
                locked,
                "{args:?} must trip the upfront reject"
            );
        }
        // Either flag alone passes.
        assert!(Argv::from_iter(["lingxi-cli", "--bg", "task"])
            .unwrap()
            .validate_background_args()
            .is_ok());
        assert!(Argv::from_iter(["lingxi-cli", "-p", "task"])
            .unwrap()
            .validate_background_args()
            .is_ok());
    }

    /// (M3 cc2.1.198) `--safe-mode` / `--bare` parse-and-carry (the wiring is
    /// exercised in `init.rs` / engine-desktop tests).
    #[test]
    fn safe_mode_and_bare_flags_parse() {
        let a = Argv::from_iter(["lingxi-cli", "--safe-mode", "hi"]).unwrap();
        assert!(a.safe_mode);
        assert!(!a.bare);
        let b = Argv::from_iter(["lingxi-cli", "--bare", "hi"]).unwrap();
        assert!(b.bare);
        assert!(!b.safe_mode);
    }

    #[test]
    fn from_pr_with_value_parses() {
        // SPACE form (matches commander `--from-pr [value]`): `--from-pr 123`
        // consumes `123` as the value, leaving NO prompt — so `claude --from-pr 123`
        // and `lingxi-cli --from-pr 123` parse identically.
        let a = Argv::from_iter(["lingxi-cli", "--from-pr", "123"]).unwrap();
        assert_eq!(a.from_pr.as_deref(), Some("123"));
        assert_eq!(
            a.prompt, None,
            "--from-pr consumes the next token as the value, not the prompt"
        );
        // The `=` form still binds the value too.
        let b = Argv::from_iter(["lingxi-cli", "--from-pr=123"]).unwrap();
        assert_eq!(b.from_pr.as_deref(), Some("123"));
    }

    #[test]
    fn from_pr_without_value_uses_empty_sentinel() {
        let a = Argv::from_iter(["lingxi-cli", "--from-pr"]).unwrap();
        assert_eq!(a.from_pr.as_deref(), Some(""));
    }

    #[test]
    fn effort_parses() {
        let a = Argv::from_iter(["lingxi-cli", "--effort", "high", "hi"]).unwrap();
        assert_eq!(a.effort.as_deref(), Some("high"));
    }

    #[test]
    fn betas_parses() {
        let a = Argv::from_iter(["lingxi-cli", "fix it", "--betas", "beta1", "beta2"]).unwrap();
        let betas = a.betas.unwrap();
        assert_eq!(betas, &["beta1", "beta2"]);
    }

    #[test]
    fn debug_file_parses() {
        let a = Argv::from_iter(["lingxi-cli", "--debug-file", "/tmp/debug.log", "hi"]).unwrap();
        assert_eq!(a.debug_file, Some(PathBuf::from("/tmp/debug.log")));
    }

    #[test]
    fn include_partial_messages_parses() {
        let a = Argv::from_iter(["lingxi-cli", "--include-partial-messages", "hi"]).unwrap();
        assert!(a.include_partial_messages);
    }

    #[test]
    fn include_hook_events_parses() {
        let a = Argv::from_iter(["lingxi-cli", "--include-hook-events", "hi"]).unwrap();
        assert!(a.include_hook_events);
    }

    #[test]
    fn forward_subagent_text_parses() {
        let a = Argv::from_iter(["lingxi-cli", "--forward-subagent-text", "hi"]).unwrap();
        assert!(a.forward_subagent_text);
        // Absent by default.
        let b = Argv::from_iter(["lingxi-cli", "hi"]).unwrap();
        assert!(!b.forward_subagent_text);
    }

    #[test]
    fn forward_subagent_text_effective_flag_or_env() {
        // Flag alone → effective true (env absent).
        let a = Argv::from_iter(["lingxi-cli", "--forward-subagent-text", "hi"]).unwrap();
        assert!(a.forward_subagent_text_effective());

        // No flag, no env → effective false. Save/restore the process env so a
        // stray value from another test doesn't flake this assertion.
        let b = Argv::from_iter(["lingxi-cli", "hi"]).unwrap();
        let prev = std::env::var("CLAUDE_CODE_FORWARD_SUBAGENT_TEXT").ok();
        std::env::remove_var("CLAUDE_CODE_FORWARD_SUBAGENT_TEXT");
        assert!(!b.forward_subagent_text_effective());

        // No flag, canonical truthy env → effective true (binary `xe = k || env`).
        std::env::set_var("CLAUDE_CODE_FORWARD_SUBAGENT_TEXT", "1");
        assert!(b.forward_subagent_text_effective());

        // No flag, NON-canonical non-empty env → still effective true. The binary
        // reads this var RAW off `process.env` (`Z.CLAUDE_CODE_FORWARD_SUBAGENT_TEXT`),
        // so plain JS string truthiness applies: `"0"`/`"false"` are non-empty and
        // therefore truthy. (This differs from `--include-partial-messages`, which
        // the binary gates through `isEnvTruthy`.)
        for v in ["0", "false", "off", "no"] {
            std::env::set_var("CLAUDE_CODE_FORWARD_SUBAGENT_TEXT", v);
            assert!(
                b.forward_subagent_text_effective(),
                "{v:?} is non-empty ⇒ raw-truthy per the binary"
            );
        }

        // No flag, empty env → effective false (empty string is JS-falsy).
        std::env::set_var("CLAUDE_CODE_FORWARD_SUBAGENT_TEXT", "");
        assert!(!b.forward_subagent_text_effective());

        // Restore.
        match prev {
            Some(v) => std::env::set_var("CLAUDE_CODE_FORWARD_SUBAGENT_TEXT", v),
            None => std::env::remove_var("CLAUDE_CODE_FORWARD_SUBAGENT_TEXT"),
        }
    }

    #[test]
    fn replay_user_messages_parses() {
        let a = Argv::from_iter(["lingxi-cli", "--replay-user-messages", "hi"]).unwrap();
        assert!(a.replay_user_messages);
    }

    #[test]
    fn thinking_enabled_parses() {
        let a = Argv::from_iter(["lingxi-cli", "--thinking", "enabled", "hi"]).unwrap();
        assert_eq!(a.thinking.as_deref(), Some("enabled"));
    }

    #[test]
    fn thinking_adaptive_parses() {
        let a = Argv::from_iter(["lingxi-cli", "--thinking", "adaptive", "hi"]).unwrap();
        assert_eq!(a.thinking.as_deref(), Some("adaptive"));
    }

    #[test]
    fn thinking_disabled_parses() {
        let a = Argv::from_iter(["lingxi-cli", "--thinking", "disabled", "hi"]).unwrap();
        assert_eq!(a.thinking.as_deref(), Some("disabled"));
    }

    #[test]
    fn thinking_invalid_errors() {
        let r = Argv::from_iter(["lingxi-cli", "--thinking", "full"]);
        assert!(r.is_err());
    }

    #[test]
    fn thinking_display_summarized_parses() {
        let a = Argv::from_iter(["lingxi-cli", "--thinking-display", "summarized", "hi"]).unwrap();
        assert_eq!(a.thinking_display.as_deref(), Some("summarized"));
    }

    #[test]
    fn thinking_display_omitted_parses() {
        let a = Argv::from_iter(["lingxi-cli", "--thinking-display", "omitted", "hi"]).unwrap();
        assert_eq!(a.thinking_display.as_deref(), Some("omitted"));
    }

    #[test]
    fn max_thinking_tokens_parses() {
        let a = Argv::from_iter(["lingxi-cli", "--max-thinking-tokens", "1000", "hi"]).unwrap();
        assert_eq!(a.max_thinking_tokens, Some(1000));
    }

    #[test]
    fn prompt_suggestions_with_value_parses() {
        let a = Argv::from_iter(["lingxi-cli", "--prompt-suggestions", "false", "hi"]).unwrap();
        assert_eq!(a.prompt_suggestions.as_deref(), Some("false"));
    }

    #[test]
    fn prompt_suggestions_without_value_uses_true_sentinel() {
        let a = Argv::from_iter(["lingxi-cli", "--prompt-suggestions"]).unwrap();
        assert_eq!(a.prompt_suggestions.as_deref(), Some("true"));
    }

    #[test]
    fn resolve_system_prompt_from_flag() {
        let a = Argv::from_iter(["lingxi-cli", "--system-prompt", "Be helpful.", "hi"]).unwrap();
        assert_eq!(a.resolve_system_prompt().as_deref(), Some("Be helpful."));
    }

    #[test]
    fn resolve_system_prompt_none_when_not_set() {
        let a = Argv::from_iter(["lingxi-cli", "hi"]).unwrap();
        assert!(a.resolve_system_prompt().is_none());
    }

    #[test]
    fn resolve_append_system_prompt_from_flag() {
        let a =
            Argv::from_iter(["lingxi-cli", "--append-system-prompt", "Be concise.", "hi"]).unwrap();
        assert_eq!(
            a.resolve_append_system_prompt().as_deref(),
            Some("Be concise.")
        );
    }

    #[test]
    fn resolve_append_system_prompt_none_when_not_set() {
        let a = Argv::from_iter(["lingxi-cli", "hi"]).unwrap();
        assert!(a.resolve_append_system_prompt().is_none());
    }

    // ── P3 validation chain ──────────────────────────────────────────────────

    #[test]
    fn input_format_stream_json_without_output_format_stream_json_errors() {
        let a = Argv::from_iter([
            "lingxi-cli",
            "--print",
            "--verbose",
            "--input-format",
            "stream-json",
            "--output-format",
            "json",
            "hi",
        ])
        .unwrap();
        let err = a.validate_stream_json_input_args().unwrap_err();
        assert_eq!(
            err,
            "--input-format=stream-json requires output-format=stream-json."
        );
    }

    #[test]
    fn input_format_stream_json_without_print_errors() {
        let a = Argv::from_iter([
            "lingxi-cli",
            "--verbose",
            "--input-format",
            "stream-json",
            "--output-format",
            "stream-json",
            "hi",
        ])
        .unwrap();
        let err = a.validate_stream_json_input_args().unwrap_err();
        assert_eq!(err, "--input-format=stream-json requires --print.");
    }

    #[test]
    fn replay_user_messages_without_stream_json_input_errors() {
        let a = Argv::from_iter([
            "lingxi-cli",
            "--print",
            "--verbose",
            "--output-format",
            "stream-json",
            "--replay-user-messages",
            "hi",
        ])
        .unwrap();
        let err = a.validate_stream_json_input_args().unwrap_err();
        assert_eq!(
            err,
            "--replay-user-messages requires both --input-format=stream-json and --output-format=stream-json."
        );
    }

    #[test]
    fn replay_user_messages_without_output_format_stream_json_errors() {
        let a = Argv::from_iter([
            "lingxi-cli",
            "--print",
            "--verbose",
            "--input-format",
            "stream-json",
            "--output-format",
            "json",
            "--replay-user-messages",
            "hi",
        ])
        .unwrap();
        // The input-format validation fires first (before replay check).
        let err = a.validate_stream_json_input_args().unwrap_err();
        assert_eq!(
            err,
            "--input-format=stream-json requires output-format=stream-json."
        );
    }

    #[test]
    fn valid_stream_json_input_flags_pass_validation() {
        let a = Argv::from_iter([
            "lingxi-cli",
            "--print",
            "--verbose",
            "--input-format",
            "stream-json",
            "--output-format",
            "stream-json",
            "hi",
        ])
        .unwrap();
        assert!(a.validate_stream_json_input_args().is_ok());
    }

    #[test]
    fn valid_stream_json_input_with_replay_passes_validation() {
        let a = Argv::from_iter([
            "lingxi-cli",
            "--print",
            "--verbose",
            "--input-format",
            "stream-json",
            "--output-format",
            "stream-json",
            "--replay-user-messages",
            "hi",
        ])
        .unwrap();
        assert!(a.validate_stream_json_input_args().is_ok());
    }

    #[test]
    fn no_stream_json_flags_passes_validation() {
        let a = Argv::from_iter(["lingxi-cli", "hi"]).unwrap();
        assert!(a.validate_stream_json_input_args().is_ok());
    }

    #[test]
    fn is_stream_json_input_detects_flag() {
        let a = Argv::from_iter(["lingxi-cli", "--input-format", "stream-json", "hi"]).unwrap();
        assert!(a.is_stream_json_input());
    }

    #[test]
    fn is_stream_json_input_false_for_text() {
        let a = Argv::from_iter(["lingxi-cli", "--input-format", "text", "hi"]).unwrap();
        assert!(!a.is_stream_json_input());
    }

    // CLI-01 (cc 2.1.238): `--autocompact <auto|tokens>` is a NEW *visible*
    // root flag. Its argParser is `DUn` (cc-238.js @222905882) — the acceptance
    // set pinned below is that function's, not a re-derivation:
    // `auto`; `k`/`m` suffixes via `parseFloat` on the SUFFIXED string; a bare
    // 100..=1000 meaning thousands; `By`'s exponent and grouped-thousands
    // spellings; and the `[1e5, 1e6]` clamp (`Lli`/`hRa`).
    #[test]
    fn autocompact_parses_the_oracle_acceptance_set() {
        let auto = Argv::from_iter(["lingxi-cli", "--autocompact", "AUTO"]).unwrap();
        assert_eq!(auto.autocompact, Some(AutocompactWindow::Auto));

        for (raw, tokens) in [
            ("500k", 500_000_u64),
            ("1m", 1_000_000),
            ("0.5m", 500_000),
            ("100k", 100_000),
            // A bare 100..=1000 is shorthand for thousands.
            ("200", 200_000),
            ("1000", 1_000_000),
            // Outside the shorthand band the number is taken literally.
            ("200000", 200_000),
            // `By` = `T8y(t) ?? parseInt(t,10)`: exponent + grouped-thousands.
            ("2e5", 200_000),
            ("2.5e5", 250_000),
            ("200,000", 200_000),
        ] {
            let parsed = Argv::from_iter(["lingxi-cli", "--autocompact", raw]).unwrap();
            assert_eq!(
                parsed.autocompact,
                Some(AutocompactWindow::Tokens(tokens)),
                "--autocompact {raw}"
            );
        }
    }

    // The clamp and the byte-exact rejection copy from the `j3t` throw at
    // cc-238.js @243908809.
    #[test]
    fn autocompact_rejects_out_of_range_and_unparseable_values() {
        for raw in ["99k", "1001", "1.5m", "2m", "abc", "-500k", ""] {
            assert!(
                Argv::from_iter(["lingxi-cli", "--autocompact", raw]).is_err(),
                "--autocompact {raw} should be rejected"
            );
        }
        let error = Argv::from_iter(["lingxi-cli", "--autocompact", "99k"])
            .unwrap_err()
            .to_string();
        assert!(
            error.contains(
                "It must be 'auto', or between 100k and 1M (e.g. 500k, 200000, or 200 as shorthand)"
            ),
            "rejection copy drifted: {error}"
        );
    }

    // `--autocompact` carries no `.hideHelp()` in 2.1.238 (unlike its
    // `--advisor` neighbour), so it must render in root `--help` with the
    // oracle's spec and description. Asserted through clap's `Arg`
    // introspection rather than the rendered screen: `wrap_help` re-flows the
    // help column to the terminal width, which would let an en dash drift to a
    // hyphen without a `contains` ever noticing.
    #[test]
    fn autocompact_is_a_visible_root_flag_with_the_oracle_copy() {
        let command = <Argv as clap::CommandFactory>::command();
        let arg = command
            .get_arguments()
            .find(|arg| arg.get_id() == "autocompact")
            .expect("--autocompact is a root flag");
        assert!(!arg.is_hide_set(), "--autocompact must render in --help");
        assert_eq!(arg.get_long(), Some("autocompact"));
        assert_eq!(
            arg.get_value_names()
                .map(|names| names.iter().map(ToString::to_string).collect::<Vec<_>>()),
            Some(vec!["auto|tokens".to_string()])
        );
        // EN DASH, not a hyphen: cc-238.js @243908809 stores
        // "Auto-compact window size (auto, or 100k\u2013(U+2013)1M tokens)".
        let expected = "Auto-compact window size (auto, or 100k\u{2013}1M tokens)";
        assert_eq!(
            arg.get_help().map(ToString::to_string).as_deref(),
            Some(expected),
            "--autocompact help copy drifted"
        );
    }

    // (CLI-13/16 + SC-09, cc 2.1.238) The four truncating-resume / rewind
    // cross-flag gates, in the oracle's order and with its byte-exact copy.
    // They are the first four statements of `runHeadless` (@307217538), so they
    // apply to PRINT mode only — both flags' own help says "Ignored outside
    // print mode". The `Error: ` prefix is the caller's (`run_cli`), matching
    // every sibling gate in this file.
    #[test]
    fn truncating_resume_gates_fire_in_the_oracle_order() {
        let no_resume = Argv::from_iter(["lingxi-cli", "-p", "--resume-session-at", "m1"]).unwrap();
        assert_eq!(
            no_resume.validate_truncating_resume_args(),
            Err("--resume-session-at requires --resume".to_string())
        );

        let drops_alone = Argv::from_iter([
            "lingxi-cli",
            "-p",
            "--resume",
            "s1",
            "--resume-drops-turn",
            "m1",
        ])
        .unwrap();
        assert_eq!(
            drops_alone.validate_truncating_resume_args(),
            Err("--resume-drops-turn requires --resume-session-at".to_string())
        );

        let rewind_no_resume =
            Argv::from_iter(["lingxi-cli", "-p", "--rewind-files", "m1"]).unwrap();
        assert_eq!(
            rewind_no_resume.validate_truncating_resume_args(),
            Err("--rewind-files requires --resume".to_string())
        );

        // `if(c.rewindFiles&&t)` — `t` is the resolved headless input, so a
        // prompt positional trips it...
        let rewind_with_prompt = Argv::from_iter([
            "lingxi-cli",
            "-p",
            "--resume",
            "s1",
            "--rewind-files",
            "m1",
            "hello",
        ])
        .unwrap();
        assert_eq!(
            rewind_with_prompt.validate_truncating_resume_args(),
            Err(
                "--rewind-files is a standalone operation and cannot be used with a prompt"
                    .to_string()
            )
        );
        // ...and so does the stream-json input iterator, which is a non-null
        // object and therefore truthy in the same check.
        let rewind_streaming = Argv::from_iter([
            "lingxi-cli",
            "-p",
            "--resume",
            "s1",
            "--rewind-files",
            "m1",
            "--input-format",
            "stream-json",
        ])
        .unwrap();
        assert_eq!(
            rewind_streaming.validate_truncating_resume_args(),
            Err(
                "--rewind-files is a standalone operation and cannot be used with a prompt"
                    .to_string()
            )
        );

        // The legal combination passes.
        let ok = Argv::from_iter([
            "lingxi-cli",
            "-p",
            "--resume",
            "s1",
            "--resume-session-at",
            "m1",
            "--resume-drops-turn",
            "m0",
        ])
        .unwrap();
        assert_eq!(ok.validate_truncating_resume_args(), Ok(()));
    }

    // The same argv OUTSIDE print mode is a true no-op: the oracle only reads
    // these flags inside `runHeadless`, and both help strings promise they are
    // "Ignored outside print mode".
    #[test]
    fn truncating_resume_gates_are_print_mode_only() {
        for argv in [
            vec!["lingxi-cli", "--resume-session-at", "m1"],
            vec!["lingxi-cli", "--resume-drops-turn", "m1"],
            vec!["lingxi-cli", "--rewind-files", "m1", "hello"],
        ] {
            let parsed = Argv::from_iter(argv.clone()).unwrap();
            assert_eq!(
                parsed.validate_truncating_resume_args(),
                Ok(()),
                "{argv:?} must be ignored outside --print"
            );
        }
    }

    // (CLI-17) The hidden root-flag surface. Each of these carries `.hideHelp()`
    // in the 2.1.238 registration block, so each must PARSE and must NOT render
    // in `--help`. A flag the oracle accepts must never become a clap
    // "unexpected argument".
    #[test]
    fn hidden_root_flags_parse_and_stay_out_of_help() {
        let valued = [
            ("--task-budget", "4096"),
            ("--workload", "cron"),
            ("--managed-settings", "{}"),
            ("--plugin-dir-no-mcp", "/tmp/p"),
            ("--advisor", "opus"),
            ("--sdk-url", "wss://example.invalid"),
            ("--prefill", "hi"),
            ("--prefill-b64", "aGk"),
            ("--deep-link-repo", "org/repo"),
            ("--deep-link-last-fetch", "1700000000000"),
            ("--deep-link-cwd-b64", "L3RtcA"),
            ("--messaging-socket-path", "/tmp/sock"),
            ("--resume-session-at", "m1"),
            ("--resume-drops-turn", "m0"),
            ("--rewind-files", "m1"),
        ];
        for (flag, value) in valued {
            assert!(
                Argv::from_iter(["lingxi-cli", flag, value]).is_ok(),
                "{flag} must parse"
            );
        }
        for flag in [
            "--debug-to-stderr",
            "--init",
            "--init-only",
            "--maintenance",
            "--session-mirror",
            "--enable-auth-status",
            "--deep-link-origin",
            "--reply-on-resume",
            "--enable-auto-mode",
        ] {
            assert!(
                Argv::from_iter(["lingxi-cli", flag]).is_ok(),
                "{flag} must parse"
            );
        }

        let command = <Argv as clap::CommandFactory>::command();
        for id in [
            "debug_to_stderr",
            "init",
            "init_only",
            "maintenance",
            "session_mirror",
            "task_budget",
            "enable_auth_status",
            "workload",
            "managed_settings",
            "plugin_dir_no_mcp",
            "advisor",
            "channels",
            "dangerously_load_development_channels",
            "sdk_url",
            "prefill",
            "prefill_b64",
            "deep_link_origin",
            "deep_link_repo",
            "deep_link_last_fetch",
            "deep_link_cwd_b64",
            "reply_on_resume",
            "enable_auto_mode",
            "append_subagent_system_prompt",
            "messaging_socket_path",
            "resume_session_at",
            "resume_drops_turn",
            "rewind_files",
        ] {
            let arg = command
                .get_arguments()
                .find(|arg| arg.get_id() == id)
                .unwrap_or_else(|| panic!("{id} is a root flag"));
            assert!(arg.is_hide_set(), "{id} carries .hideHelp() in the oracle");
        }
    }

    // `-d2e, --debug-to-stderr` carries `.implies({debug:!0})` (@307399127), so
    // it is a third alias onto the same switch as `--debug` / `--debug-file`.
    #[test]
    fn debug_to_stderr_implies_debug_mode() {
        let a = Argv::from_iter(["lingxi-cli", "--debug-to-stderr"]).unwrap();
        assert!(a.debug_to_stderr);
        assert!(a.debug_enabled());
    }

    // `--task-budget`'s argParser rejects NaN, <= 0 and non-integers with one
    // byte-exact message (@307403649).
    #[test]
    fn task_budget_rejects_everything_the_oracle_rejects() {
        // (`-1` is not in this list: clap rejects a leading-hyphen VALUE before
        // the parser ever runs, so it would assert commander's message against
        // clap's "unexpected argument" — a different gate.)
        for raw in ["0", "1.5", "abc", ""] {
            let error = Argv::from_iter(["lingxi-cli", "--task-budget", raw])
                .expect_err("value must be rejected")
                .to_string();
            assert!(
                error.contains("--task-budget must be a positive integer"),
                "rejection copy drifted for {raw}: {error}"
            );
        }
        let ok = Argv::from_iter(["lingxi-cli", "--task-budget", "4096"]).unwrap();
        assert_eq!(ok.task_budget, Some(4096));
    }

    // `--prefill-b64` / `--deep-link-cwd-b64` use Node's
    // `Buffer.from(v,"base64url").toString("utf8")`, which never throws: a
    // malformed value decodes leniently instead of becoming an argv error, and
    // `--deep-link-last-fetch`'s `Number.isFinite` argParser DROPS a
    // non-numeric value rather than rejecting it.
    #[test]
    fn deep_link_arg_parsers_are_lenient_like_node() {
        let a = Argv::from_iter(["lingxi-cli", "--prefill-b64", "aGVsbG8"]).unwrap();
        assert_eq!(a.resolve_prefill().as_deref(), Some("hello"));
        // Out-of-alphabet characters are skipped, not rejected.
        let b = Argv::from_iter(["lingxi-cli", "--prefill-b64", "aG!Vsb G8="]).unwrap();
        assert_eq!(b.resolve_prefill().as_deref(), Some("hello"));
        // `--prefill-b64` wins over `--prefill` (only one is ever set upstream).
        let c =
            Argv::from_iter(["lingxi-cli", "--prefill", "raw", "--prefill-b64", "aGk"]).unwrap();
        assert_eq!(c.resolve_prefill().as_deref(), Some("hi"));

        let d = Argv::from_iter(["lingxi-cli", "--deep-link-cwd-b64", "L3RtcA"]).unwrap();
        assert_eq!(d.resolve_deep_link_cwd().as_deref(), Some("/tmp"));

        let e = Argv::from_iter(["lingxi-cli", "--deep-link-last-fetch", "nope"]).unwrap();
        assert_eq!(e.deep_link_last_fetch_ms(), None, "Number('nope') is NaN");
        let f = Argv::from_iter(["lingxi-cli", "--deep-link-last-fetch", "1700000000000"]).unwrap();
        assert!(
            f.deep_link_last_fetch_ms()
                .is_some_and(|ms| (ms - 1_700_000_000_000.0).abs() < 1.0),
            "a finite epoch-ms value survives the argParser"
        );
    }
}
