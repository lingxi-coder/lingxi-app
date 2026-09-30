use crate::argv::Argv;

pub(super) fn command_runs_config_startup(command: Option<&crate::commands::Commands>) -> bool {
    match command {
        Some(crate::commands::Commands::Project(project)) => project.command.is_some(),
        Some(
            crate::commands::Commands::Sandbox(_)
            | crate::commands::Commands::Attach(_)
            | crate::commands::Commands::RemoteControl(_)
            | crate::commands::Commands::Rm(_)
            | crate::commands::Commands::Logs(_)
            | crate::commands::Commands::Stop(_)
            | crate::commands::Commands::Respawn(_)
            | crate::commands::Commands::Daemon(_)
            | crate::commands::Commands::BgRun(_)
            | crate::commands::Commands::BgPtySession(_),
        ) => false,
        _ => true,
    }
}

pub(super) fn command_initializes_user_id(command: Option<&crate::commands::Commands>) -> bool {
    matches!(
        command,
        Some(
            crate::commands::Commands::Mcp(_)
                | crate::commands::Commands::Doctor(_)
                | crate::commands::Commands::SetupToken(_)
                | crate::commands::Commands::Install(_)
                | crate::commands::Commands::Update(_)
        )
    )
}

/// Materialize first-run configuration and run the versioned startup
/// migrations at Claude's pre-command boundary. Specialized fast paths that
/// bypass this block in the oracle are filtered by
/// [`command_runs_config_startup`].
pub(super) async fn run_config_startup(command: Option<&crate::commands::Commands>) {
    if !command_runs_config_startup(command) {
        return;
    }
    let (Some(global_config_path), Some(lingxi_home)) = (
        migrations::global_config::global_config_path(),
        migrations::global_config::lingxi_config_home(),
    ) else {
        return;
    };

    let migration_pending = migrations::global_config::read_map(&global_config_path)
        .map(|map| {
            map.get("migrationVersion")
                .and_then(serde_json::Value::as_u64)
                != Some(migrations::CURRENT_MIGRATION_VERSION)
        })
        .unwrap_or(false);
    let initializes_device_identity = command.is_none() || command_initializes_user_id(command);

    if migration_pending || initializes_device_identity {
        let first_start_time =
            chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        if let Err(error) = migrations::global_config::ensure_first_start_metadata(
            &global_config_path,
            &first_start_time,
            lingxi_core::host::CLAUDE_CODE_VERSION,
        ) {
            tracing::warn!(%error, "first-start metadata write failed");
        }
    }
    if migration_pending || initializes_device_identity {
        let _ = migrations::global_config::ensure_machine_id(&global_config_path);
    }

    let project_dir = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let env = migrations::MigrationEnv {
        global_config_path,
        lingxi_config_home: lingxi_home,
        project_dir,
        ctx: migrations::MigrationContext::from_env(),
        bus: None,
    };
    // `Lm(e)` in 2.1.245 runs this unversioned migration immediately before
    // the version-13 set; it is intentionally no longer a runner member.
    migrations::migrate_mcp_servers::run(&env).await;
    migrations::run_migrations(&env).await;

    // These command families reach the device identity during their own
    // startup in Claude 2.1.245; keep it after the migration block so the
    // resulting top-level key order matches the oracle.
    if command_initializes_user_id(command) {
        let _ = migrations::global_config::get_or_create_user_id();
    }

    // Async fire-and-forget (TS `.catch(() => {})`): retried next startup.
    tokio::spawn(async move {
        migrations::migrate_changelog_from_config(&env).await;
    });
}

/// Resolve the initial main-loop model the engine will use, then return the
/// model-deprecation startup notice for it (or `None` when current).
///
/// The model resolution is byte-identical to the one
/// `init::resolve_desktop_config` threads into `DesktopConfig.default_model`:
/// the `--model` override if present, else the desktop default
/// (`harness_runtime::desktop::DesktopConfig::default().default_model`). This mirrors
/// claude-code's `resolvedInitialModel = parseUserSpecifiedModel(
/// initialMainLoopModel ?? getDefaultMainLoopModel())` (`main.tsx:2116`), the
/// same value fed to `getModelDeprecationWarning` at `main.tsx:2873`.
///
/// The lookup itself is `harness_runtime::desktop::model_deprecation_warning`
/// (moved from the deleted `providers` crate in Plan 3b); it returns
/// `Some(warning)` only for a deprecated model under the active provider and
/// `None` otherwise — so a current default model yields `None` and the caller
/// prints nothing (byte-identical startup).
pub(super) fn startup_deprecation_notice(argv: &Argv) -> Option<String> {
    let resolved_model = argv
        .model
        .clone()
        .unwrap_or_else(|| harness_runtime::desktop::DesktopConfig::default().default_model);
    harness_runtime::desktop::model_deprecation_warning(Some(&resolved_model))
}

/// (T3) One-line terminal-compatibility notice for terminals known to mis-render
/// the fullscreen alt-screen UI. Currently only Warp (`TERM_PROGRAM=WarpTerminal`)
/// — its non-standard alt-screen compositing ghosts (stacked frames). Standard
/// terminals (iTerm2, Terminal.app, Alacritty, Ghostty, kitty, …) render it
/// correctly, so they get `None`. Pure (env value injected) for unit-testing; the
/// REAL fix for Warp is an inline (non-alt-screen) render loop — a large change
/// the viewport math isn't built for, deferred.
pub(super) fn ghosting_terminal_notice(term_program: Option<&str>) -> Option<&'static str> {
    match term_program {
        Some("WarpTerminal") => Some(
            "\u{26a0} Warp can ghost LingXi's full-screen UI (stacked frames / stray rows). \
             Use iTerm2, Terminal.app, Alacritty, Ghostty, or kitty \u{2014} or try the \
             experimental inline mode: LINGXI_TUI_INLINE=1.",
        ),
        _ => None,
    }
}
