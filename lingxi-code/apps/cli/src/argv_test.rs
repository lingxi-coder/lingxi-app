//! Tests for `argv.rs`, extracted from inline `#[cfg(test)]` blocks. Included via `#[path] mod argv_test;`.

use super::*;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_args_is_repl_mode() {
        let a = Argv::from_iter(["lingxi-cli"]).unwrap();
        assert!(a.is_repl_mode());
        assert!(a.prompt.is_none());
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

    #[test]
    fn plugin_dir_parses() {
        let a = Argv::from_iter(["lingxi-cli", "--plugin-dir", "/my/plugins", "hi"]).unwrap();
        assert_eq!(a.plugin_dir, Some(PathBuf::from("/my/plugins")));
    }

    #[test]
    fn no_session_persistence_flag_parses() {
        let a = Argv::from_iter(["lingxi-cli", "--no-session-persistence", "hi"]).unwrap();
        assert!(a.no_session_persistence);
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
}
