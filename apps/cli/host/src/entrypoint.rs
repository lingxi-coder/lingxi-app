use super::argv;
use super::ax_screen_reader;
use super::bypass_env;
use super::command_line_output::commander_error;
use super::command_line_output::commander_help;
use super::commands;
use super::control_plane;
use super::cwd;
use super::exit_codes;
use super::init;
use super::logging;
use super::mode;
use super::output;
use super::output_adapter;
use super::permission_prompt_notify;
use super::permission_settings::resolve_permission_mode;
use super::process_wrapper;
use super::run;
use super::startup::ghosting_terminal_notice;
use super::startup::run_config_startup;
use super::startup::startup_deprecation_notice;
use super::startup_resources;
use super::startup_trace;
use super::stream_json;
use crate::argv::Argv;
use clap::error::ErrorKind;
use std::ffi::OsString;
use std::path::Path;
use std::sync::Arc;

/// Check for an occupied transcript id anywhere under the current config
/// home's `projects/` store. Fresh `--session-id` launches/forks must fail
/// before runtime construction instead of appending to an existing JSONL.
///
/// A missing store is an empty store. Every other I/O failure is surfaced so
/// the caller fails closed instead of treating an unreadable store as proof
/// that the id is unused.
pub(super) async fn session_id_exists_in_store(
    config_home: &Path,
    session_id: uuid::Uuid,
) -> std::io::Result<bool> {
    let projects_root = config_home.join("projects");
    let mut entries = match tokio::fs::read_dir(&projects_root).await {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => {
            return Err(std::io::Error::new(
                error.kind(),
                format!("failed to read {}: {error}", projects_root.display()),
            ));
        }
    };
    let file_name = format!("{session_id}.jsonl");
    loop {
        let Some(entry) = entries.next_entry().await.map_err(|error| {
            std::io::Error::new(
                error.kind(),
                format!("failed to scan {}: {error}", projects_root.display()),
            )
        })?
        else {
            return Ok(false);
        };
        if !entry.file_type().await?.is_dir() {
            continue;
        }
        if tokio::fs::try_exists(entry.path().join(&file_name)).await? {
            return Ok(true);
        }
    }
}

/// Top-level entrypoint. Returns the process exit code.
pub async fn run_cli(args: Vec<OsString>) -> i32 {
    if let Some(code) = commands::plugin_eval_mock::run_from_env().await {
        return code;
    }
    startup_trace::start();
    // Freeze the startup environment BEFORE anything can apply a settings-file
    // `env` to the process. `${VAR}` inside a MANAGED MCP allow/deny matcher
    // expands against this snapshot, so taking it late would let a lower-trust
    // settings tier steer what an enterprise policy matches. The oracle gets
    // this ordering implicitly (`Dut()` calls `NQr()` first); here it is
    // explicit, and this is the earliest point in the process.
    mcp::enterprise_policy::prime_startup_env();
    let mut parsed = match Argv::from_iter(args.clone()) {
        Ok(a) => a,
        Err(e) => {
            // claude-code (commander) renders several argv errors differently
            // from clap — a single/two-line message with no "Usage:"/"For more
            // information" block. Reformat the ones we can match byte-for-byte
            // (missing-arg, unknown-option, unknown-command + suggestion);
            // everything else keeps clap's rendering (the remaining
            // clap-vs-commander help-block layout difference).
            if let Some(msg) = commander_error(&e, &args) {
                eprintln!("{msg}");
                return exit_codes::ARGV_ERROR;
            }
            // Help is normalized to commander's visible-alias presentation;
            // other clap errors/version output retain clap's renderer.
            if e.kind() == ErrorKind::DisplayHelp {
                print!("{}", commander_help(&e));
            } else {
                e.print().ok();
            }
            return match e.kind() {
                ErrorKind::DisplayHelp | ErrorKind::DisplayVersion => exit_codes::SUCCESS,
                _ => exit_codes::ARGV_ERROR,
            };
        }
    };
    startup_trace::init(parsed.debug_filter());
    startup_trace::mark("argv_parse");

    if parsed.restricted_enabled() {
        std::env::set_var("LINGXI_RESTRICTED", "1");
    }

    // `--debug` is now `Option<String>` (optional category filter); collapse to
    // on/off for logging init. `--mcp-debug` (deprecated alias) and `--debug-file`
    // also imply debug mode.
    // The interactive fullscreen TUI owns the terminal — suppress stderr logging
    // there so WARN lines don't corrupt the rendered frame. A subcommand
    // (mcp/auth/…) or print/stdio mode keeps the normal stderr logger.
    let interactive_tui = matches!(
        parsed.command.as_ref(),
        Some(crate::commands::Commands::BgPtySession(_))
    ) || (parsed.command.is_none()
        && matches!(crate::mode::decide_mode(&parsed), crate::mode::Mode::Tui));
    startup_trace::mark("mode_decide_for_logging");
    logging::init(parsed.debug_enabled(), interactive_tui);
    let telemetry_records_session = parsed.command.is_none()
        || matches!(
            parsed.command.as_ref(),
            Some(crate::commands::Commands::BgPtySession(_))
        );
    let managed_otel_overrides = harness_runtime::desktop::managed_otel_env_overrides().await;
    let _telemetry_guard =
        match telemetry::otel::OtelConfig::from_env_with_managed(&managed_otel_overrides) {
            cfg if cfg.enabled => telemetry::otel::install_process_with_config(
                "lingxi-cli",
                telemetry_records_session,
                cfg,
            ),
            _ => telemetry::otel::TelemetryGuard::disabled(),
        };
    tracing::debug!(?parsed, "argv parsed");

    // (M3 cc2.1.198) `--bg`/`--background` × `--print`/`-p` is rejected UP
    // FRONT — before any mode/subcommand dispatch — matching the binary's bg
    // fast path (`handleBgFlag` runs from the top-level dispatcher on any
    // `--bg`/`--background` token and its `pof` validator rejects before a
    // session dir is even minted). Bare message + `\n` on stderr (no `Error:`
    // prefix), exit 1 (`process.exitCode=1`).
    if let Err(msg) = parsed.validate_background_args() {
        eprintln!("{msg}");
        return exit_codes::ARGV_ERROR;
    }

    // Claude Code 2.1.246 validates the entire `--agents` record before any
    // runtime/auth work. Safe mode deliberately ignores the flag without
    // parsing it; bare mode still validates it.
    let safe_mode = parsed.safe_mode
        || lingxi_core::host::env::is_env_truthy(std::env::var("LINGXI_SAFE_MODE").ok().as_deref());
    if !safe_mode {
        if let Some(raw) = parsed.agents.as_deref() {
            if let Err(error) = agent::parse_agents_from_flag_json_checked(raw) {
                eprintln!("Error: Invalid --agents configuration:\n{error}");
                return exit_codes::ARGV_ERROR;
            }
        }
    }

    // (M4 cc2.1.198) `--effort` argParser warning. The binary validates INSIDE
    // commander's argParser (`u4i` — so the warning prints during argv parse,
    // before any dispatch) and continues with the flag ignored:
    // `process.stderr.write(`Warning: ${l}\n`)`. Verified live: stderr
    // `Warning: Unknown --effort value 'banana' — ignoring it and using the
    // default effort. Valid values: low, medium, high, xhigh, max.`, run
    // continues. The normalized level threads via `resolve_desktop_config`.
    if let (_, Some(warning)) = parsed.normalized_effort() {
        eprintln!("{warning}");
    }

    // (M3 cc2.1.198) `--bare` exports `LINGXI_SIMPLE=1` for this process +
    // children (binary top dispatcher @224048363: `if((l===-1?t:t.slice(0,l))
    // .includes("--bare"))process.env.CLAUDE_CODE_SIMPLE="1"` — pre-`--`
    // tokens only, which clap's parse already honors). `xd()` also treats a
    // pre-set truthy env as bare, mirrored in `resolve_desktop_config`.
    if parsed.bare {
        std::env::set_var("LINGXI_SIMPLE", "1");
    }
    // (M3 cc2.1.198) safe mode (`Ql()` = `--safe-mode` OR truthy env) exports
    // `LINGXI_SAFE_MODE=1` + `LINGXI_DISABLE_LINGXI_MDS=1` (binary @223917313:
    // `if(Ql())process.env.CLAUDE_CODE_SAFE_MODE="1",process.env.
    // CLAUDE_CODE_DISABLE_CLAUDE_MDS="1"`); the latter is the orchestrator's
    // existing LINGXI.md kill-switch (`orchestrator::prompt::memory_block`),
    // so subagents/children inherit the disable too.
    if parsed.safe_mode
        || lingxi_core::host::env::is_env_truthy(std::env::var("LINGXI_SAFE_MODE").ok().as_deref())
    {
        std::env::set_var("LINGXI_SAFE_MODE", "1");
        std::env::set_var("LINGXI_DISABLE_LINGXI_MDS", "1");
    }

    // (M-01, cc2.1.215) `--brief` selects Brief-only mode for this process.
    // Keep the value in shared live-session state so `/brief` can toggle it
    // later without relying on a stale process-environment snapshot.
    lingxi_core::host::session_flags::set_brief_mode_enabled(parsed.brief);

    // claude-code `ZDn(ERe.some(ue))` (2.1.263) — naming any of the five
    // todo/task tools in `--tools`/`--allowedTools` opts the session past the
    // `OO()` model gate. Launch-time immutable, so publish it once here beside
    // the other argv-derived session flags.
    lingxi_core::host::session_flags::set_todo_tools_opt_in(parsed.todo_tools_opt_in());

    // (CLI-2) `--system-prompt-snapshot <on|off>` feeds oracle `lje(e)`'s
    // `e.systemPromptSnapshot`. Launch-time immutable like the flags above, and
    // published BEFORE any conversation is built, since the gate is read on the
    // very first request (that is the request whose prompt gets recorded).
    lingxi_core::host::session_flags::set_system_prompt_snapshot(parsed.system_prompt_snapshot);
    // 2.1.270 `oVn`: streaming input or an SDK URL makes print non-single-shot.
    lingxi_core::host::session_flags::set_single_shot_print_session(
        (parsed.print
            || parsed
                .prompt
                .as_deref()
                .is_some_and(|prompt| !prompt.trim().is_empty()))
            && parsed.input_format.as_deref() != Some("stream-json")
            && parsed.sdk_url.as_deref().unwrap_or("").is_empty(),
    );

    // (CLI-12, cc2.1.238) `--messaging-socket-path <path>` (@307414302) pins
    // the cross-session messaging socket instead of the auto-generated path.
    // Recorded here, before ANY code path can bind the inbox
    // (`mode::ensure_live_messaging` is the only binder and runs far later, in
    // the TUI/session mounts).
    if let Some(path) = parsed.messaging_socket_path.as_deref() {
        crate::mode::set_messaging_socket_override(path);
    }

    // (CLI-15) `--append-subagent-system-prompt` carries the oracle's implication
    // `wby(e,t=process.env){if(e)t.CLAUDE_CODE_ENABLE_APPEND_SUBAGENT_PROMPT="1"}`
    // (@306637528) — the flag turns its own gate on.
    //
    // The SPLICE half is now wired too: the value rides
    // `LINGXI_APPEND_SUBAGENT_SYSTEM_PROMPT` and
    // `agent::handle::append_subagent_system_prompt_suffix` folds it onto every
    // subagent's rendered system prompt as the final section, gated on the env
    // flag above (oracle @292360822:
    // `Zt=!C&&!d?.isolatedContext&&Un(process.env.CLAUDE_CODE_ENABLE_APPEND_SUBAGENT_PROMPT)
    //     &&r.options.appendSubagentSystemPrompt?Rm([...Xt,r.options.appendSubagentSystemPrompt]):Xt`).
    //
    // The env pair is the transport because the subagent spawner is reached
    // through `harness-runtime::desktop`'s runtime build, which carries no per-spawn CLI
    // options channel; it is also what gives the oracle's "propagated to nested
    // subagents" for free — a nested spawn is a child of the same process and
    // reads the same variables. Both are set BEFORE any runtime is constructed.
    //
    // (2.1.261) `--append-subagent-system-prompt-file` feeds the same pair from
    // a file. The INLINE flag wins when both are supplied, so adding the file
    // form cannot silently displace an explicit string. An unreadable file is a
    // hard error: silently running without a prompt the user asked for is worse
    // than refusing to start.
    let appended_subagent_prompt = match parsed.append_subagent_system_prompt.clone() {
        Some(text) => Some(text),
        None => match parsed.append_subagent_system_prompt_file.as_deref() {
            Some(path) => match std::fs::read_to_string(path) {
                Ok(text) => Some(text),
                Err(error) => {
                    eprintln!(
                        "Error: --append-subagent-system-prompt-file could not read {path}: {error}"
                    );
                    return exit_codes::ARGV_ERROR;
                }
            },
            None => None,
        },
    };
    if let Some(text) = appended_subagent_prompt.as_deref() {
        std::env::set_var("LINGXI_ENABLE_APPEND_SUBAGENT_PROMPT", "1");
        std::env::set_var("LINGXI_APPEND_SUBAGENT_SYSTEM_PROMPT", text);
    }

    // (CLI-01, cc2.1.238) `--autocompact <auto|tokens>` projects onto
    // `LINGXI_AUTO_COMPACT_WINDOW`, the port's only auto-compact-window pin
    // (`compaction::thresholds::effective_context_window_size` clamps the model
    // context window with it, and `/autocompact` reports it as
    // `WindowSource::Env`). The oracle resolves the flag with
    // `lvp(t.autocompact, Vo().autoCompactWindow)`: `auto` yields `undefined`
    // and therefore DROPS the configured window, so an explicit `auto` clears a
    // pre-set env pin here rather than merely leaving it alone.
    match parsed.autocompact {
        Some(argv::AutocompactWindow::Auto) => std::env::remove_var("LINGXI_AUTO_COMPACT_WINDOW"),
        Some(argv::AutocompactWindow::Tokens(tokens)) => {
            std::env::set_var("LINGXI_AUTO_COMPACT_WINDOW", tokens.to_string());
        }
        None => {}
    }

    // Custom beta headers are an API-key-only Anthropic surface. Validate at
    // startup so OAuth/non-key sessions do not appear to accept an inert flag.
    if let Some(raw_betas) = parsed.betas.as_mut() {
        let mut seen = std::collections::HashSet::new();
        raw_betas.retain(|beta| {
            let trimmed = beta.trim();
            !trimmed.is_empty() && seen.insert(trimmed.to_string())
        });
        for beta in raw_betas.iter_mut() {
            *beta = beta.trim().to_string();
            if beta.contains(',') || beta.chars().any(char::is_control) {
                eprintln!("lingxi-cli: invalid --betas value `{beta}`");
                return exit_codes::ARGV_ERROR;
            }
        }
        if !raw_betas.is_empty() {
            if std::env::var("ANTHROPIC_API_KEY")
                .ok()
                .is_none_or(|key| key.trim().is_empty())
            {
                eprintln!("lingxi-cli: --betas is only available for Anthropic API-key users.");
                return exit_codes::RUNTIME_ERROR;
            }
            if parsed.model.as_deref().is_some_and(|model| {
                let (profile, _) = llm_runtime::split_profile_model(model);
                profile != "anthropic"
            }) {
                eprintln!("lingxi-cli: --betas is only supported by the Anthropic provider.");
                return exit_codes::RUNTIME_ERROR;
            }
        }
    }

    // `--cwd <dir>` must apply BEFORE the subcommand dispatch, not just for
    // session modes: the subcommands resolve their target project from the LIVE
    // process cwd (mcp via `current_dir()` → project key + `<cwd>/.mcp.json`,
    // doctor, and — destructively — `project purge`, whose default target is the
    // current dir). If we chdir only after dispatch, `--cwd /other mcp add …`
    // writes the WRONG project's config and `--cwd /other project purge` deletes
    // the WRONG project's transcripts. `apply_cwd` only validates + `set_current_dir`
    // (no other side effects), so it is safe to run first for all modes.
    if let Err(e) = cwd::apply_cwd(parsed.cwd.as_deref()) {
        eprintln!("lingxi-cli: {e}");
        return exit_codes::RUNTIME_ERROR;
    }

    // These public flags depend on Anthropic-private services/protocols. Never
    // accept them as inert success and never route them to LingXi's unrelated
    // local desktop bridge.
    if parsed.no_chrome {
        // This flag is an explicit negative capability, not an accepted no-op.
        // The setting is inherited by background/session children and is the
        // single gate future optional browser integrations must consult.
        std::env::set_var("LINGXI_DISABLE_CHROME", "1");
    }
    if parsed.chrome {
        eprintln!(
            "lingxi-cli: --chrome requires the unavailable Anthropic Chrome extension protocol."
        );
        return exit_codes::NOT_IMPLEMENTED;
    }
    if parsed.restricted_enabled() && parsed.remote_control.is_some() {
        eprintln!("Cloud sessions cannot be created from a --restricted session");
        return exit_codes::RUNTIME_ERROR;
    }
    if parsed.remote_control.is_some() {
        eprintln!(
            "lingxi-cli: --remote-control requires the unavailable Anthropic relay/auth protocol."
        );
        return exit_codes::NOT_IMPLEMENTED;
    }

    // Managed enterprise startup version gate (parity 2.1.207 H-BIN-09,
    // CC `c1p`/`a1p` on the fast startup path). If a managed (`policySettings`)
    // tier pins `requiredMinimumVersion`/`requiredMaximumVersion` and this
    // binary's version is outside the org-approved range, print the byte-exact
    // instruction message to stderr and exit 1 — BEFORE any subcommand dispatch
    // or session build. `update`/`install`/`doctor` are exempt (a pinned-out
    // user must be able to fix their install); a bare session (no command) is
    // gated. Reads the OS-level managed dir (cwd-independent, already-applied
    // `--cwd` is irrelevant). Fail-open on an unreadable/malformed policy.
    {
        let top_level = parsed
            .command
            .as_ref()
            .map(crate::commands::Commands::top_level_name);
        let managed_tiers =
            harness_runtime::desktop::settings_watch::managed_settings_raw_tiers().await;
        let policy = lingxi_core::settings::enterprise::managed_version_policy(&managed_tiers);
        if let Some(msg) = lingxi_core::settings::enterprise::version_gate(
            env!("CARGO_PKG_VERSION"),
            policy.required_minimum_version.as_deref(),
            policy.required_maximum_version.as_deref(),
            top_level,
            &mut |m| tracing::error!("{m}"),
        ) {
            eprintln!("{msg}");
            return exit_codes::RUNTIME_ERROR;
        }
    }

    // Freeze `processWrapper` before any command can spawn the daemon, a
    // background worker, or another copy of this CLI. The resolver deliberately
    // excludes project/local settings; descendants inherit this immutable argv
    // snapshot instead of re-reading a possibly different working directory.
    let wrapper_cwd = match std::env::current_dir() {
        Ok(cwd) => cwd,
        Err(error) => {
            eprintln!("lingxi-cli: failed to resolve cwd for processWrapper: {error}");
            return exit_codes::RUNTIME_ERROR;
        }
    };
    if let Err(error) = process_wrapper::configure(parsed.settings.as_deref(), &wrapper_cwd).await {
        eprintln!("lingxi-cli: {error}");
        return exit_codes::RUNTIME_ERROR;
    }
    let flag_settings = match crate::init::parse_flag_settings_checked(parsed.settings.as_deref()) {
        Ok(settings) => settings,
        Err(error) => {
            eprintln!("lingxi-cli: {error}");
            return exit_codes::RUNTIME_ERROR;
        }
    };
    if let Some(settings) = flag_settings {
        if let Ok(value) = serde_json::to_value(settings) {
            let _ = mcp::enterprise_policy::install_flag_settings_policy(value);
        }
    }

    run_config_startup(parsed.command.as_ref()).await;

    // (Item B) Resolve the session permission mode from CLI flags + settings,
    // run the bypass safety guards, and capture the startup notice. This must
    // happen AFTER `cwd::apply_cwd` (so the project `.lingxi/settings.json` is
    // read from the effective project dir) and BEFORE `build_runtime` — a
    // refused bypass exits before the runtime is constructed, and the resolved
    // mode threads into `DesktopConfig.permission_mode`.
    let (permission_mode, permission_notice) = resolve_permission_mode(&parsed);
    if parsed.restricted_enabled()
        && (permission_mode == permission::PermissionMode::BypassPermissions
            || parsed.dangerously_skip_permissions
            || parsed.allow_dangerously_skip_permissions)
    {
        eprintln!("bypassPermissions not supported in restricted mode");
        return exit_codes::RUNTIME_ERROR;
    }
    if permission_mode == permission::PermissionMode::BypassPermissions
        || parsed.dangerously_skip_permissions
    {
        if let Err(msg) = permission::enforce_bypass_safety(&bypass_env::RealBypassEnv::new()).await
        {
            eprintln!("{msg}");
            return exit_codes::RUNTIME_ERROR;
        }
    }

    // Top-level subcommand dispatch (mcp/auth/plugin/project/setup-token/agents/
    // install/update/doctor/auto-mode/ultrareview). When clap matched a leading
    // command token, run that family and exit — this is what stops a bare `mcp`/
    // `auth` token from being swallowed as a billable chat prompt.
    if let Some(command) = parsed.command.clone() {
        if let crate::commands::Commands::Plugin(cli) = &command {
            if matches!(
                cli.command.as_ref(),
                Some(crate::commands::plugin::Sub::Install(_))
                    | Some(crate::commands::plugin::Sub::Update(_))
            ) {
                let sink: Arc<dyn output::OutputSink> = if parsed.is_json_output() {
                    Arc::new(output::JsonSink::new(lingxi_core::types::SessionId::new()))
                } else {
                    Arc::new(output::PlainSink::new())
                };
                let adapter: Arc<dyn lingxi_core::host::OutputStream> =
                    Arc::new(output_adapter::SinkAdapter::new(sink));
                let runtime = match init::build_runtime(&parsed, adapter, permission_mode).await {
                    Ok(r) => r,
                    Err(e) => {
                        eprintln!("lingxi-cli: {e}");
                        return exit_codes::RUNTIME_ERROR;
                    }
                };
                return crate::commands::plugin::run_with_shared_analytics_bus(
                    cli,
                    runtime.analytics_bus.clone(),
                )
                .await;
            }
        }
        return command.run().await;
    }

    // Session-only network resources are resolved before any orchestrator/TUI
    // state is constructed. Background sessions defer this into the PTY child:
    // resolving a plugin URL here would serialize a path below this process's
    // temporary guard and delete it as soon as daemonization returned.
    let _startup_resources = if parsed.background {
        None
    } else {
        match startup_resources::prepare(&mut parsed).await {
            Ok(guard) => Some(guard),
            Err(e) => {
                eprintln!("lingxi-cli: {e}");
                return exit_codes::RUNTIME_ERROR;
            }
        }
    };

    // `--session-id <uuid>` validation (claude-code main.tsx:1276-1300), byte-exact
    // messages + exit 1. Runs for every session mode (print/TUI/REPL) before any
    // runtime is built; the validated id then threads into
    // `DesktopConfig.session_id_override` via `resolve_desktop_config`.
    if let Some(sid) = parsed.session_id.as_deref() {
        // (a) cross-flag rule: pairing --session-id with --continue/--resume
        //     requires --fork-session (else the resumed session's own id wins).
        if (parsed.continue_session || parsed.resume.is_some()) && !parsed.fork_session {
            eprintln!(
                "Error: --session-id can only be used with --continue or --resume if --fork-session is also specified."
            );
            return exit_codes::ARGV_ERROR;
        }
        // (b) UUID validation (a bare UUID; `parse_prefixed` also tolerates the
        //     `sess:`-prefixed display form).
        if lingxi_core::types::SessionId::parse_prefixed(sid).is_none() {
            eprintln!("Error: Invalid session ID. Must be a valid UUID.");
            return exit_codes::ARGV_ERROR;
        }
        // (c) A user-supplied fresh session id must not reuse any existing
        //     transcript path, even from another project under the same config
        //     home. Reject before runtime construction so no append/write path
        //     is opened against the occupied JSONL.
        if let Some(parsed_id) = lingxi_core::types::SessionId::parse_prefixed(sid) {
            match session_id_exists_in_store(&run::lingxi_home_dir(), parsed_id.as_uuid()).await {
                Ok(true) => {
                    eprintln!("Error: Session ID {sid} is already in use.");
                    return exit_codes::ARGV_ERROR;
                }
                Ok(false) => {}
                Err(error) => {
                    eprintln!("Error: Unable to verify session ID uniqueness: {error}");
                    return exit_codes::RUNTIME_ERROR;
                }
            }
        }
    }

    // `--setting-sources <user,project,local>` token validation (claude-code
    // main.tsx): each comma-split token must be one of user/project/local
    // (CASE-SENSITIVE, raw token echoed) else hard-error with the byte-exact
    // message + exit 1. An empty string is VALID (treated as no sources).
    if let Some(sources) = parsed.setting_sources.as_deref() {
        for tok in sources.split(',').map(str::trim).filter(|t| !t.is_empty()) {
            if !matches!(tok, "user" | "project" | "local") {
                eprintln!(
                    "Error processing --setting-sources: Invalid setting source: {tok}. Valid options are: user, project, local"
                );
                return exit_codes::ARGV_ERROR;
            }
        }
    }

    // (`cwd::apply_cwd` already ran above, before the subcommand dispatch.)

    // Background dispatch happens only after session/settings validation and
    // permission safety enforcement. The dispatcher then runs the foreground
    // trust/bypass setup UI and records that approval in launch.json before it
    // daemonises; the hidden PTY child never owns unattended setup dialogs.
    if parsed.background {
        return crate::background_dispatch::dispatch_background(&parsed, permission_mode).await;
    }

    if parsed.teammate_launch_file.is_some() {
        return crate::teammate_worker::run(&parsed, permission_mode).await;
    }

    // (CLI-13/16 + SC-09, cc2.1.238) The truncating-resume / rewind cross-flag
    // gates. In the oracle these are the FIRST four statements of `runHeadless`
    // after `after_grove_check` (@307217538), ahead of every other headless
    // gate, and each writes its line to stderr and exits 1:
    //
    // ```js
    // if(c.resumeSessionAt&&!c.resume){…"Error: --resume-session-at requires --resume"}
    // if(c.resumeDropsTurn!==void 0&&!c.resumeSessionAt){…"Error: --resume-drops-turn requires --resume-session-at"}
    // if(c.rewindFiles&&!c.resume){…"Error: --rewind-files requires --resume"}
    // if(c.rewindFiles&&t){…"Error: --rewind-files is a standalone operation and cannot be used with a prompt"}
    // ```
    //
    // They are therefore print-mode-only, exactly as both flags' help text says
    // ("Ignored outside print mode"); `validate_truncating_resume_args` carries
    // that condition.
    if let Err(msg) = parsed.validate_truncating_resume_args() {
        eprintln!("Error: {msg}");
        return exit_codes::ARGV_ERROR;
    }

    // (CLI-13) The truncating resume itself lives in
    // `crate::resume_truncation::apply_truncating_resume`, applied by
    // `run::resume_resolved_session` immediately after the transcript load —
    // the oracle's own position (@307370121). It stops the chain at the named
    // entry and, when `--resume-drops-turn` is supplied, REFUSES when the
    // discarded range holds anything not attributable to the declared turn
    // (the `AEy` classifier, @306799802). This module used to refuse the flag
    // outright; that refusal is gone now that the behaviour exists.

    // P3 cross-flag validation for --input-format=stream-json and
    // --replay-user-messages (§4.1 SPEC-inferred.md, exact error strings).
    if let Err(msg) = parsed.validate_stream_json_input_args() {
        eprintln!("Error: {msg}");
        return exit_codes::ARGV_ERROR;
    }

    // (M4 cc2.1.198) A truthy `--prompt-suggestions` requires --print +
    // --output-format=stream-json. Binary order: this `Es(...)` gate runs
    // IMMEDIATELY BEFORE the include-partial-messages gate (same statement
    // chain in the main action). Byte-exact message + exit 1 (verified live).
    if let Err(msg) = parsed.validate_prompt_suggestions_args() {
        eprintln!("Error: {msg}");
        return exit_codes::ARGV_ERROR;
    }

    // `--include-partial-messages` requires BOTH --print and
    // --output-format=stream-json (claude-code main.tsx:1848-1852). Byte-exact
    // error + exit 1 (ARGV_ERROR). Without this gate lingxi silently accepted
    // the misuse and ran a (billable) turn.
    if parsed.include_partial_messages && !(parsed.print && parsed.is_stream_json()) {
        eprintln!(
            "Error: --include-partial-messages requires --print and --output-format=stream-json."
        );
        return exit_codes::ARGV_ERROR;
    }

    // (2.1.211) `--forward-subagent-text` (binary `xe = k ||
    // CLAUDE_CODE_FORWARD_SUBAGENT_TEXT`) forwards subagent text/thinking blocks
    // as assistant/user messages with a non-null `parent_tool_use_id`. Requires
    // BOTH --print and --output-format=stream-json. The binary places this gate
    // immediately AFTER the include-partial-messages gate: an EXPLICIT flag in
    // the wrong context is a fatal error, while an env-only opt-in silently
    // disables. Byte-exact message + exit 1.
    if parsed.forward_subagent_text && !(parsed.print && parsed.is_stream_json()) {
        eprintln!(
            "Error: --forward-subagent-text requires --print and --output-format=stream-json."
        );
        return exit_codes::ARGV_ERROR;
    }

    // (M3 cc2.1.198) `--no-session-persistence` requires `--print`. Binary
    // order: this gate runs immediately AFTER the include-partial-messages
    // gate (@223929381, next statement). Byte-exact message + exit 1.
    if let Err(msg) = parsed.validate_session_persistence_args() {
        eprintln!("Error: {msg}");
        return exit_codes::ARGV_ERROR;
    }

    // (C5) `--plan-mode-instructions` is `--print`-only; same surfacing so the
    // final stderr is byte-exact `Error: --plan-mode-instructions can only be
    // used with --print mode.`
    if let Err(msg) = parsed.validate_plan_mode_instructions_args() {
        eprintln!("Error: {msg}");
        return exit_codes::ARGV_ERROR;
    }

    // (CLI-16, cc2.1.238) `--rewind-files <user-message-id>` — "Restore files to
    // state at the specified user message and exit (requires --resume)". In the
    // oracle this branch sits inside `runHeadless` right after the transcript
    // loads (@307222364) and ALWAYS exits, so it never reaches the prompt gate
    // or the `--output-format=stream-json requires --verbose` gate below it —
    // hence its position HERE, ahead of both output-format branches, rather
    // than down with the `--continue`/`--resume` dispatch. Its two argv gates
    // fired above; the sink is built inline because `make_sink` is defined
    // after the `&mut parsed` startup-resource pass.
    if parsed.print && parsed.rewind_files.is_some() {
        let sink: Arc<dyn output::OutputSink> = if parsed.is_json_output() {
            Arc::new(output::JsonSink::new(lingxi_core::types::SessionId::new()))
        } else {
            Arc::new(output::PlainSink::new())
        };
        return run::run_rewind_files(&parsed, sink.as_ref()).await;
    }

    // stream-json: `--output-format stream-json --verbose` (print-only, no
    // SinkAdapter/OutputSink layer — the StreamJsonStream IS the OutputStream).
    //
    // Gate: when `--print`/`-p` is combined with `stream-json` the caller MUST
    // also pass `--verbose` (mirrors claude-code's argv validation:
    // `main.tsx printMode && !verbose && outputFormat=="stream-json"` →
    // "When using --print, --output-format=stream-json requires --verbose").
    if parsed.is_stream_json() {
        if parsed.print && !parsed.verbose {
            eprintln!("Error: When using --print, --output-format=stream-json requires --verbose");
            return exit_codes::ARGV_ERROR;
        }
        let stream = Arc::new(stream_json::StreamJsonStream::new_placeholder());
        lingxi_core::host::OutputStream::set_thinking_display(
            stream.as_ref(),
            parsed.thinking_display.as_deref(),
        );
        // P4: wire --include-partial-messages and --include-hook-events flags
        // before build_runtime so the stream is fully configured before any
        // hook or SSE events flow through it.
        stream.set_flags(parsed.include_partial_messages, parsed.include_hook_events);
        // (2.1.211) Carry the effective `--forward-subagent-text` state (flag OR
        // truthy CLAUDE_CODE_FORWARD_SUBAGENT_TEXT), gated to the valid --print +
        // stream-json context — an explicit flag in the wrong context already
        // errored above; an env-only opt-in in the wrong context stays disabled.
        stream.set_forward_subagent_text(
            parsed.forward_subagent_text_effective() && parsed.print && parsed.is_stream_json(),
        );
        let adapter: Arc<dyn lingxi_core::host::OutputStream> = stream.clone();

        // P5 Phase 2: for the bidirectional `--input-format stream-json` path,
        // build the shared control plane BEFORE `build_runtime` (its outbound
        // handle comes from the stream, available now) and inject the
        // `can_use_tool` permission decider as the inner transport. The
        // `PolicyPermissionGate` (enforcement default on) wraps it as the OUTER
        // local pre-check, so only an unresolved `Ask` round-trips over stdio.
        // The output-only print path keeps the headless deny-on-ask default
        // (no stdin reader to answer a control_response).
        // (2.1.259) `--permission-prompts none` removes the prompt surface. When
        // a surface WAS configured, say so rather than letting it look connected:
        // oracle `if(Hn){me.hostAnswersElicitations=!1; if(Rn!==void 0) log(...)}`,
        // where `Rn` is `"stdio"` for the SDK host or the `--permission-prompt-tool`.
        if parsed.permission_prompts_none() {
            if let Some(surface) = if parsed.is_stream_json_input() {
                Some("SDK host")
            } else if parsed.permission_prompt_tool.is_some() {
                Some("--permission-prompt-tool")
            } else {
                None
            } {
                eprintln!(
                    "--permission-prompts none: permission prompts are answered with a local deny; the {surface} is not consulted"
                );
            }
        }

        let control_plane = if parsed.is_stream_json_input() {
            let plane = control_plane::StdioControlPlane::new(stream.outbound_tx());
            // GATE-SYSMSG-01: share the stream's session-id handle so a locally
            // denied tool emits a `permission_denied` system message stamped with
            // the same `session_id` as every data frame.
            plane.set_session_id(stream.session_id_handle());
            Some(plane)
        } else {
            None
        };

        let runtime = if let Some(plane) = &control_plane {
            let mut cfg = init::resolve_desktop_config(&parsed, permission_mode);
            // §2b: persist an ALLOW response's `updatedPermissions` rule updates to
            // the SAME settings tree the engine resolved (claude-code
            // `persistPermissionUpdates`). Paths come from the same `cfg` so a
            // host-allowed rule lands where the next session loads it.
            let gate = Arc::new(
                control_plane::StdioControlPermissionGate::new(plane.clone()).with_persist(
                    permission::PermissionPaths {
                        lingxi_home: cfg.lingxi_home.clone(),
                        cwd: cfg.cwd.clone(),
                    },
                ),
            );
            // SH-02: keep a typed handle so the `permission_prompt` notifier
            // can be attached once the orchestrator exists (the gate itself has
            // to be built FIRST — it is injected into the runtime config).
            let gate_handle = gate.clone();
            cfg.injected_permission_gate = Some(gate as Arc<dyn permission::gate::PermissionGate>);
            let rt = match init::build_cli_runtime_from_config(cfg, adapter, &parsed).await {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("lingxi-cli: {e}");
                    return exit_codes::RUNTIME_ERROR;
                }
            };
            // SH-02 — this is what makes `Cou` reachable: without it the gate's
            // `OnceLock` stays empty and every armed guard is inert.
            gate_handle.set_prompt_notifier(Arc::new(
                permission_prompt_notify::OrchestratorPermissionPromptNotifier::new(
                    rt.orchestrator.clone(),
                ),
            ));
            init::auto_connect_ide_if_requested(&parsed, &rt).await;
            rt
        } else {
            match init::build_runtime(&parsed, adapter, permission_mode).await {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("lingxi-cli: {e}");
                    return exit_codes::RUNTIME_ERROR;
                }
            }
        };
        // OR-1: point the stream at the orchestrator's LIVE permission-denial
        // cell, so the `result` frame reports the run's refused tool calls
        // instead of a hardcoded `[]`. Shared (not snapshotted) so none of the
        // six result-emit sites has to remember to push the list.
        stream.share_permission_denials(runtime.orchestrator.permission_denials_handle());
        if let Some(notice) = startup_deprecation_notice(&parsed) {
            eprintln!("{notice}");
        }
        if let Some(notice) = &permission_notice {
            eprintln!("{notice}");
        }
        // P3: when --input-format=stream-json is also set, use the multi-turn
        // stdin loop instead of the single-prompt one-shot path.
        if let Some(plane) = control_plane {
            return run::run_stream_json_input_loop(
                &parsed,
                &runtime,
                stream,
                permission_mode,
                plane,
            )
            .await;
        }
        return run::run_stream_json_print(&parsed, &runtime, stream, permission_mode).await;
    }

    // `--output-format json` / `--json` in PRINT mode: emit only the final
    // result JSON line (suppress all streaming frames). Only intercept when
    // a non-slash prompt is present or `--print` is active (no slash prompt)
    // — slash commands keep the old JSON event format, interactive/REPL mode
    // with `--json` falls through to the normal dispatch path (repl.rs handles it).
    let is_non_slash_print = parsed
        .prompt
        .as_deref()
        .is_some_and(|p| !p.trim_start().starts_with('/'));
    if parsed.is_json_output() && (is_non_slash_print || parsed.print) {
        let stream = Arc::new(stream_json::StreamJsonStream::new_json_mode_placeholder());
        let adapter: Arc<dyn lingxi_core::host::OutputStream> = stream.clone();
        let runtime = match init::build_runtime(&parsed, adapter, permission_mode).await {
            Ok(r) => r,
            Err(e) => {
                eprintln!("lingxi-cli: {e}");
                return exit_codes::RUNTIME_ERROR;
            }
        };
        if let Some(notice) = startup_deprecation_notice(&parsed) {
            eprintln!("{notice}");
        }
        if let Some(notice) = &permission_notice {
            eprintln!("{notice}");
        }
        return run::run_json_print(&parsed, &runtime, stream, permission_mode).await;
    }

    let chosen = mode::decide_mode(&parsed);
    startup_trace::mark("mode_decide");

    // (2.1.201) Resolve + cache the accessibility screen-reader gate ONCE. This
    // is independent of the engine runtime, so keep it before the mode-specific
    // runtime build instead of forcing fresh TUI/REPL paths to pre-build and
    // discard a generic runtime.
    let ax_config = init::load_settings_ax_screen_reader(&parsed);
    ax_screen_reader::init(parsed.ax_screen_reader, ax_config);
    // `if(!L && process.stdout.isTTY && IO()) console.log("[Accessible screen
    // reader mode: on]")` — `L` == print mode. Emitted for interactive stdout.
    {
        use std::io::IsTerminal as _;
        ax_screen_reader::maybe_announce(parsed.print, std::io::stdout().is_terminal());
    }

    // (W40-follow-up) One-time startup notices, the bounded stand-in for
    // claude-code's startup notification queue (`main.tsx:2872-2896`). TS pushes
    // up to two HIGH-priority notices onto a UI queue rendered above the REPL:
    // the model-deprecation warning and the permission-mode notice. The Rust CLI
    // has no UI notification queue, so — per the brief — we emit the bounded
    // subset as a plain startup line instead of building that abstraction.
    //
    // Surfaced here: the DEPRECATION warning. `startup_deprecation_notice`
    // resolves the same initial model `resolve_desktop_config` threads into the
    // engine (`--model`, else the desktop default — the analog of TS
    // `resolvedInitialModel = parseUserSpecifiedModel(initialMainLoopModel ??
    // getDefaultMainLoopModel())`) and returns `Some` only when that model is
    // deprecated for the active provider. It goes to STDERR so `--json` stdout
    // stays parseable (the same channel discipline the REPL's `"> "` prompt
    // uses). SAFETY: with any current (Claude 4-generation) default model the
    // lookup is `None`, so this prints nothing and startup is byte-identical.
    //
    // The sibling permission-mode notice (TS `permissionModeNotification`,
    // `main.tsx:2882-2887`) is now wired below: `initialPermissionModeFromCLI`
    // (resolved pre-`build_runtime` above) sets `permission_notice` when the
    // bypass killswitch suppressed a requested bypass.
    if let Some(notice) = startup_deprecation_notice(&parsed) {
        eprintln!("{notice}");
    }

    // (T3) Terminal-compatibility notice: the fullscreen alt-screen UI ghosts on
    // Warp (non-standard alt-screen compositing — stacked frames / stray rows).
    // Print ONCE, before the alt-screen is entered, so it lands in Warp's
    // pre-alt-screen scrollback (Warp keeps it). Only for the interactive TUI
    // (print/REPL/stdio don't enter alt-screen, so they never ghost) and only to a
    // tty (no-op when stderr is piped). Standard terminals print nothing.
    if interactive_tui {
        use std::io::IsTerminal;
        if std::io::stderr().is_terminal() {
            if let Some(notice) =
                ghosting_terminal_notice(std::env::var("TERM_PROGRAM").ok().as_deref())
            {
                eprintln!("{notice}");
            }
        }
    }

    // (Item B) Permission-mode startup notice (TS `permissionModeNotification`,
    // `main.tsx:2882-2887`): set only when the bypass killswitch suppressed a
    // requested bypass (`initialPermissionModeFromCLI`). Emitted on the same
    // bounded stderr channel as the deprecation/migration notices (no UI
    // notification-queue substrate); `None` in the common case, so startup is
    // byte-identical when no bypass was disabled.
    if let Some(notice) = &permission_notice {
        eprintln!("{notice}");
    }

    let make_sink = || -> Arc<dyn output::OutputSink> {
        if parsed.is_json_output() {
            Arc::new(output::JsonSink::new(lingxi_core::types::SessionId::new()))
        } else {
            Arc::new(output::PlainSink::new())
        }
    };

    // `-c/--continue` resumes the MOST-RECENT conversation in the current cwd's
    // project dir (claude-code `main.tsx`: `options.continue` →
    // `loadConversationForResume(undefined)`; errors `No conversation found to
    // continue` when none). Dispatched BEFORE `--resume` so the no-id continue
    // path is honored. `--continue --resume <id>` is rejected upstream
    // (lib.rs:315 cross-flag rule), so the two never collide here.
    if parsed.continue_session {
        let mut resumed_argv = parsed.clone();
        if let Some(effort) = run::inherited_resume_effort(&parsed).await {
            resumed_argv.effort = Some(effort);
        }
        let sink = make_sink();
        let adapter: Arc<dyn lingxi_core::host::OutputStream> =
            Arc::new(output_adapter::SinkAdapter::new(sink.clone()));
        let runtime = match init::build_runtime(&resumed_argv, adapter, permission_mode).await {
            Ok(r) => r,
            Err(e) => {
                eprintln!("lingxi-cli: {e}");
                return exit_codes::RUNTIME_ERROR;
            }
        };
        return run::run_continue(&resumed_argv, &runtime, sink.as_ref()).await;
    }

    // --resume routes through run::run_resume, which itself splits (M7-12):
    //   <uuid>            → load by id
    //   (none) + TTY      → iocraft Resume screen
    //   (none) + --no-tui → M5-08 stdio picker (unchanged fallback)
    if parsed.resume.is_some() {
        let mut resumed_argv = parsed.clone();
        if let Some(effort) = run::inherited_resume_effort(&parsed).await {
            resumed_argv.effort = Some(effort);
        }
        let sink = make_sink();
        let adapter: Arc<dyn lingxi_core::host::OutputStream> =
            Arc::new(output_adapter::SinkAdapter::new(sink.clone()));
        let runtime = match init::build_runtime(&resumed_argv, adapter, permission_mode).await {
            Ok(r) => r,
            Err(e) => {
                eprintln!("lingxi-cli: {e}");
                return exit_codes::RUNTIME_ERROR;
            }
        };
        return run::run_resume(&resumed_argv, &runtime, sink.as_ref()).await;
    }

    // (M4 cc2.1.198) `--from-pr [value]` — the binary opens the SAME resume
    // picker with `filterByPr: rt` (bare flag → only PR-linked sessions; a
    // parseable PR number/URL → `prNumber === n`; an unparseable value applies
    // no narrowing). `--resume` wins when both are given (its branch runs
    // first in the binary's session-source resolution too).
    if parsed.from_pr.is_some() {
        let sink = make_sink();
        return run::run_from_pr(&parsed, sink.as_ref()).await;
    }

    match chosen {
        mode::Mode::Tui | mode::Mode::StdioRepl => {
            mode::dispatch_interactive(chosen, &parsed).await
        }
        mode::Mode::Print(_) => {
            let sink = make_sink();
            let adapter: Arc<dyn lingxi_core::host::OutputStream> =
                Arc::new(output_adapter::SinkAdapter::new(sink.clone()));
            let runtime = match init::build_runtime(&parsed, adapter, permission_mode).await {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("lingxi-cli: {e}");
                    return exit_codes::RUNTIME_ERROR;
                }
            };
            mode::dispatch(chosen, &parsed, &runtime, sink).await
        }
    }
}
