//! Process IO, signals and product callbacks for the embedded headless runner.

use crate::argv::Argv;
use harness_runtime::headless::host::HeadlessHostServices;
use harness_runtime::headless::io::Output;
use lingxi_core::host::OrchestratorHandle;
use std::collections::HashMap;
use std::io::IsTerminal;
use std::path::Path;
use std::sync::Arc;

pub(crate) fn process_stdout() -> Output {
    Output::with_terminal(tokio::io::stdout(), std::io::stdout().is_terminal())
}

pub(crate) fn process_stderr() -> Output {
    Output::with_terminal(tokio::io::stderr(), std::io::stderr().is_terminal())
}

struct CliHeadlessServices {
    background_forker: Arc<crate::bg_session_forker::CliBgSessionForker>,
    auto_connect_ide: bool,
}

#[async_trait::async_trait]
impl HeadlessHostServices for CliHeadlessServices {
    fn mcp_local_staging_root(&self, session_cwd: &Path) -> Option<std::path::PathBuf> {
        // This process adapter owns a local workspace. Bind each call to the
        // session's resolved cwd, including an accepted directory relocation.
        Some(session_cwd.to_path_buf())
    }

    fn request_identity(&self) -> Option<harness_runtime::desktop::DesktopRequestIdentity> {
        Some(harness_runtime::desktop::DesktopRequestIdentity {
            device_id: migrations::global_config::get_or_create_user_id(),
            account_uuid: String::new(),
            extra_metadata: None,
        })
    }

    async fn runtime_ready(
        &self,
        runtime: &harness_runtime::desktop::DesktopRuntime,
    ) -> Result<(), String> {
        self.background_forker
            .bind_runtime(&runtime.orchestrator, runtime.session_cwd.clone());
        if self.auto_connect_ide {
            if let Err(error) = runtime.orchestrator.ide_auto_connect_if_single().await {
                eprintln!("Warning: could not connect to local IDE: {error}");
            }
        }
        Ok(())
    }

    fn environment_variable(&self, name: &str) -> Option<String> {
        std::env::var(name).ok()
    }

    fn update_environment_variables(
        &self,
        variables: HashMap<String, String>,
    ) -> Result<(), String> {
        for (name, value) in variables {
            std::env::set_var(name, value);
        }
        Ok(())
    }

    fn refresh_launch_identity(&self, cwd: &Path, transcript: &Path) -> Result<(), String> {
        crate::background_launch::refresh_current_background_launch_identity(cwd, transcript)
            .map(|_| ())
            .map_err(|error| error.to_string())
    }

    fn launch_identity_rollback_failed(&self, detail: &str) {
        crate::mode::fail_current_background_job_after_cd_failure(detail);
    }

    fn remember_permission_mode(&self, mode: &str) {
        crate::permission_mode_preference::remember(mode);
    }

    fn trust_config_path(&self) -> Option<std::path::PathBuf> {
        migrations::global_config::global_config_path()
    }

    fn account_metadata(&self) -> serde_json::Value {
        serde_json::json!({
            "email": "",
            "organization": "",
            "subscriptionType": "Claude Max",
            "apiProvider": "firstParty"
        })
    }
}

/// Compose the process adapter and hand complete session ownership to Harness.
pub(crate) async fn run(argv: &Argv, mut permission_mode: permission::PermissionMode) -> i32 {
    use harness_runtime::headless::config::{
        HeadlessConfig, HeadlessOptions, InputFormat, OutputFormat, SessionStart,
    };
    #[cfg(not(unix))]
    use harness_runtime::headless::HeadlessIo;
    use harness_runtime::headless::{HeadlessSignal, HeadlessSignals};

    let mut desktop = crate::init::resolve_desktop_config(argv, permission_mode);
    let session_start = match select_session_start(argv, &mut desktop).await {
        Ok(start) => start,
        Err(error) => {
            eprintln!("{error}");
            if argv.is_stream_json() {
                let session_id = argv
                    .resume
                    .as_deref()
                    .and_then(|arg| lingxi_core::types::SessionId::parse_prefixed(arg.trim()))
                    .map(|id| id.as_uuid())
                    .unwrap_or_else(uuid::Uuid::new_v4);
                let frame = session_selection_error_frame(&session_id.to_string(), &error).await;
                let output = process_stdout();
                let line = harness_runtime::headless::stream_json::serialize_ndjson_line(&frame);
                let _ = output.write_all(line.as_bytes()).await;
                let _ = output.flush().await;
            }
            return crate::exit_codes::RUNTIME_ERROR;
        }
    };
    if let SessionStart::Resume {
        session_id, fork, ..
    } = &session_start
    {
        let Some(path) = desktop.session_transcript_path.clone() else {
            eprintln!("lingxi-cli: selected resume source is unavailable");
            return crate::exit_codes::RUNTIME_ERROR;
        };
        let captured = match crate::init::capture_selected_cli_source(
            &desktop.lingxi_home,
            *session_id,
            &path,
            Some(lingxi_core::host::session_context::ConversationMode::Code),
        ) {
            Ok(captured) => captured,
            Err(error) => {
                eprintln!("lingxi-cli: resume source capture failed: {error}");
                return crate::exit_codes::RUNTIME_ERROR;
            }
        };
        if !*fork {
            let cwd = match crate::init::selected_resume_workspace(&captured) {
                Ok(cwd) => cwd,
                Err(error) => {
                    eprintln!("lingxi-cli: resume workspace changed: {error}");
                    return crate::exit_codes::RUNTIME_ERROR;
                }
            };
            let (target_mode, notice) =
                crate::permission_settings::resolve_permission_mode_at(argv, &cwd);
            if let Err(error) =
                crate::permission_settings::enforce_permission_mode_safety(argv, target_mode).await
            {
                eprintln!("{error}");
                return crate::exit_codes::RUNTIME_ERROR;
            }
            if let Some(notice) = notice {
                eprintln!("{notice}");
            }
            permission_mode = target_mode;
        }
        desktop = match configure_selected_resume(
            argv,
            permission_mode,
            desktop,
            *session_id,
            *fork,
            captured,
        ) {
            Ok(desktop) => desktop,
            Err(error) => {
                eprintln!("lingxi-cli: resume source changed: {error}");
                return crate::exit_codes::RUNTIME_ERROR;
            }
        };
    }
    let background_forker = Arc::new(crate::bg_session_forker::CliBgSessionForker::new(
        desktop.lingxi_home.clone(),
        desktop.lingxi_home.clone(),
        crate::background_launch::BackgroundLaunchOptions::from_argv(argv),
        permission_mode,
    ));
    desktop.bg_session_forker = Some(background_forker.clone());
    let is_slash_prompt = argv
        .prompt
        .as_deref()
        .is_some_and(|p| p.trim_start().starts_with('/'));
    let options = HeadlessOptions {
        prompt: argv.prompt.clone(),
        output_format: if argv.is_stream_json() {
            OutputFormat::StreamJson
        } else if argv.json && !argv.print && is_slash_prompt {
            OutputFormat::Ndjson
        } else if argv.is_json_output() {
            OutputFormat::Json
        } else {
            OutputFormat::Text
        },
        input_format: if argv.is_stream_json_input() {
            InputFormat::StreamJson
        } else {
            InputFormat::Text
        },
        no_stream: argv.no_stream,
        verbose: argv.verbose,
        explicit_print: argv.print,
        replay_user_messages: argv.replay_user_messages,
        include_partial_messages: argv.include_partial_messages,
        include_hook_events: argv.include_hook_events,
        forward_subagent_text: argv.forward_subagent_text_effective()
            && argv.print
            && argv.is_stream_json(),
        thinking_display: argv.thinking_display.clone(),
        settings: argv.settings.clone(),
        betas: argv.betas.clone(),
        max_budget_usd: argv.max_budget_usd,
        json_schema: argv.json_schema.as_deref().and_then(|schema| {
            lingxi_core::types::utf16_json::Utf16JsonProjection::parse(schema).ok()
        }),
        prompt_suggestions: argv.prompt_suggestions_enabled() == Some(true),
        max_structured_output_retries:
            harness_runtime::headless::structured_output::resolve_max_retries(
                std::env::var("MAX_STRUCTURED_OUTPUT_RETRIES")
                    .ok()
                    .as_deref(),
            ),
        permission_prompts_none: argv.permission_prompts_none(),
        rewind_files: argv.rewind_files.clone(),
        ..HeadlessOptions::default()
    };
    let services = Arc::new(CliHeadlessServices {
        background_forker,
        auto_connect_ide: argv.ide,
    });
    #[cfg(unix)]
    let io = match crate::headless_process_io::open() {
        Ok(io) => io,
        Err(error) => {
            eprintln!("lingxi-cli: could not open headless process IO: {error}");
            return crate::exit_codes::RUNTIME_ERROR;
        }
    };
    #[cfg(not(unix))]
    // Windows has no cancellable standard-handle adapter here yet. Its Tokio
    // stdio fallback is outside the Unix process-cancellation acceptance tests.
    let io = HeadlessIo {
        input: Box::pin(tokio::io::stdin()),
        input_is_terminal: std::io::IsTerminal::is_terminal(&std::io::stdin()),
        stdout: process_stdout(),
        stderr: process_stderr(),
    };
    let (signal_tx, signal_rx) = tokio::sync::mpsc::unbounded_channel();
    let signal_listener = tokio::spawn(async move {
        #[cfg(unix)]
        let mut terminate =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).ok();
        loop {
            #[cfg(unix)]
            {
                tokio::select! {
                    result = tokio::signal::ctrl_c() => {
                        if result.is_err() || signal_tx.send(HeadlessSignal::Interrupt).is_err() { break; }
                    }
                    _ = async {
                        match terminate.as_mut() {
                            Some(signal) => { signal.recv().await; }
                            None => std::future::pending::<()>().await,
                        }
                    } => {
                        if signal_tx.send(HeadlessSignal::Terminate { signal: 15 }).is_err() { break; }
                    }
                }
            }
            #[cfg(not(unix))]
            {
                if tokio::signal::ctrl_c().await.is_err()
                    || signal_tx.send(HeadlessSignal::Interrupt).is_err()
                {
                    break;
                }
            }
        }
    });
    let result = harness_runtime::headless::run(
        HeadlessConfig {
            desktop,
            options,
            session_start,
        },
        services,
        io,
        HeadlessSignals::new(signal_rx),
    )
    .await;
    signal_listener.abort();
    let _ = signal_listener.await;
    result.code
}

/// Native emits one zero-turn result for a failed stream-json session lookup.
/// Text and JSON lookup failures have no stdout. Reuse the normal result/usage
/// builders and wire serializer without assembling credentials or a runtime.
async fn session_selection_error_frame(session_id: &str, error: &str) -> serde_json::Value {
    use harness_runtime::headless::stream_json::StreamJsonStream;
    let stream = StreamJsonStream::new_placeholder(Output::new(tokio::io::sink()));
    *stream.session_id_handle().lock().await = session_id.to_string();
    let mut source = stream
        .build_result_error_frame(
            "error_during_execution",
            vec![error.to_string()],
            &lingxi_core::host::CostSnapshot::default(),
            "off",
            None,
            &[],
        )
        .await;
    source["duration_ms"] = serde_json::json!(0);
    source["total_cost_usd"] = serde_json::json!(0);
    let source = source
        .as_object_mut()
        .expect("result builder returns an object");
    let mut frame = serde_json::Map::new();
    // Lookup failed before a model turn: only the fields observed in Native's
    // early result belong here, with their original wire order.
    for key in [
        "type",
        "subtype",
        "duration_ms",
        "duration_api_ms",
        "is_error",
        "num_turns",
        "stop_reason",
        "session_id",
        "total_cost_usd",
        "usage",
        "modelUsage",
        "permission_denials",
        "uuid",
        "errors",
        "result_index",
    ] {
        frame.insert(
            key.to_string(),
            source.remove(key).expect("shared result field exists"),
        );
    }
    serde_json::Value::Object(frame)
}

/// Resolve only the selected local source before runtime or credential startup.
/// An empty --continue is a fresh session in Claude Code 2.1.321 print mode.
async fn select_session_start(
    argv: &Argv,
    desktop: &mut harness_runtime::desktop::DesktopConfig,
) -> Result<harness_runtime::headless::config::SessionStart, String> {
    use harness_runtime::headless::config::SessionStart;
    use session::jsonl::loader::LoaderError;

    let load_catalog = || async {
        // The discovery loader maps an absent project bucket to an empty
        // catalog. Do not let an invalid/unreadable store become a fresh
        // conversation merely because --continue now permits an empty store.
        let projects = desktop.lingxi_home.join("projects");
        match tokio::fs::symlink_metadata(&projects).await {
            Ok(metadata) => {
                let metadata = if metadata.is_symlink() {
                    tokio::fs::metadata(&projects)
                        .await
                        .map_err(|error| error.to_string())?
                } else {
                    metadata
                };
                if !metadata.is_dir() {
                    return Err("resume transcript root is not a directory".to_string());
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error.to_string()),
        }
        match crate::run::load_resume_rows_from(&desktop.lingxi_home, &desktop.cwd).await {
            Ok(rows) => Ok(rows),
            Err(LoaderError::EmptyDirectory) => Ok(Vec::new()),
            Err(error) => Err(error.to_string()),
        }
    };
    let selected = if argv.continue_session {
        let rows = load_catalog().await?;
        rows.into_iter().next().map(|row| (row.uuid, row.path))
    } else if let Some(raw) = argv.resume.as_deref() {
        let arg = raw.trim();
        let selected = if let Some(id) = lingxi_core::types::SessionId::parse_prefixed(arg) {
            let id = id.as_uuid();
            let path = crate::init::resolve_unique_cli_resume_path(&desktop.lingxi_home, id)
                .map_err(|error| {
                    if error.kind() == std::io::ErrorKind::NotFound {
                        format!("No conversation found with session ID: {id}")
                    } else {
                        error.to_string()
                    }
                })?;
            Some((id, path))
        } else if std::path::Path::new(arg).is_file() {
            let path = crate::init::absolute_resolved_tui_resume_path(arg.into())
                .map_err(|error| error.to_string())?;
            let id = path
                .file_stem()
                .and_then(|stem| stem.to_str())
                .and_then(|stem| uuid::Uuid::parse_str(stem).ok())
                .ok_or_else(|| "resume transcript has invalid filename".to_string())?;
            // Keep the existing catalog-root, link and ownership checks. The
            // source capture below revalidates and pins the same exact file.
            let path = crate::init::validate_exact_tui_resume_path(&desktop.lingxi_home, id, &path)
                .map_err(|error| error.to_string())?;
            Some((id, path))
        } else {
            crate::run::resolve_resume_title_source_from(load_catalog().await?, arg)?
                .map(|row| (row.uuid, row.path))
        };
        Some(selected.ok_or_else(|| "No conversation found to resume".to_string())?)
    } else {
        None
    };
    Ok(match selected {
        Some((session_id, path)) => {
            let path = crate::init::absolute_resolved_tui_resume_path(path)
                .map_err(|error| error.to_string())?;
            desktop.session_transcript_path = Some(path);
            SessionStart::Resume {
                session_id,
                resume_session_at: (!argv.continue_session)
                    .then(|| argv.resume_session_at.clone())
                    .flatten(),
                resume_drops_turn: (!argv.continue_session)
                    .then(|| argv.resume_drops_turn.clone())
                    .flatten(),
                fork: argv.fork_session,
            }
        }
        None => SessionStart::New,
    })
}

/// Resolve project configuration from the same captured source that the SDK
/// will mount. A fork is created in the launch workspace, so retains its config.
fn configure_selected_resume(
    argv: &Argv,
    permission_mode: permission::PermissionMode,
    launch: harness_runtime::desktop::DesktopConfig,
    session_id: uuid::Uuid,
    fork: bool,
    captured: crate::init::PublishedBranchSource,
) -> Result<harness_runtime::desktop::DesktopConfig, crate::init::InitError> {
    let mut desktop = if fork {
        launch
    } else {
        let cwd = crate::init::selected_resume_workspace(&captured)?;
        let mut resumed =
            crate::init::resolve_resumed_desktop_config_at(argv, permission_mode, cwd);
        // Keep the catalog root used to authenticate this exact source.
        resumed.lingxi_home = launch.lingxi_home;
        resumed
    };
    desktop.session_transcript_path = Some(captured.authority.path());
    crate::init::install_published_branch(&mut desktop, session_id, captured)?;
    Ok(desktop)
}

#[cfg(all(test, unix))]
#[path = "headless_host_mcp_test.rs"]
mod mcp_consumer_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[tokio::test]
    async fn selection_error_result_matches_native_321_bytes_except_generated_uuid() {
        let session_id = "77777777-7777-4777-8777-777777777777";
        let error = format!("No conversation found with session ID: {session_id}");
        let frame = session_selection_error_frame(session_id, &error).await;
        let captured = include_str!("../tests/fixtures/session-321-missing-uuid-result.jsonl");
        let native: serde_json::Value = serde_json::from_str(captured).unwrap();
        let expected = captured.replace(
            native["uuid"].as_str().unwrap(),
            frame["uuid"].as_str().unwrap(),
        );
        assert_eq!(
            harness_runtime::headless::stream_json::serialize_ndjson_line(&frame),
            expected
        );
    }

    #[tokio::test]
    async fn empty_headless_continue_starts_new_but_invalid_store_fails_closed() {
        let root = tempfile::tempdir().unwrap();
        let argv = Argv::try_parse_from(["lingxi-cli", "-p", "--continue", "next"]).unwrap();
        let mut desktop = harness_runtime::desktop::DesktopConfig::default();
        desktop.lingxi_home = root.path().join("home");
        desktop.cwd = root.path().join("work");
        std::fs::create_dir_all(&desktop.cwd).unwrap();
        assert!(matches!(
            select_session_start(&argv, &mut desktop).await.unwrap(),
            harness_runtime::headless::config::SessionStart::New
        ));
        assert!(desktop.session_transcript_path.is_none());
        std::fs::create_dir_all(&desktop.lingxi_home).unwrap();
        std::fs::write(desktop.lingxi_home.join("projects"), "invalid store").unwrap();
        assert!(select_session_start(&argv, &mut desktop).await.is_err());
        #[cfg(unix)]
        {
            std::fs::remove_file(desktop.lingxi_home.join("projects")).unwrap();
            std::os::unix::fs::symlink("missing-root", desktop.lingxi_home.join("projects"))
                .unwrap();
            assert!(select_session_start(&argv, &mut desktop).await.is_err());
        }
    }

    #[tokio::test]
    async fn missing_headless_resume_uuid_reports_session_in_absent_store() {
        let root = tempfile::tempdir().unwrap();
        let id = uuid::Uuid::new_v4();
        let argv = Argv::try_parse_from(["lingxi-cli", "-p", "--resume", &id.to_string(), "next"])
            .unwrap();
        let mut desktop = harness_runtime::desktop::DesktopConfig::default();
        desktop.lingxi_home = root.path().join("home");
        desktop.cwd = root.path().to_path_buf();
        assert_eq!(
            select_session_start(&argv, &mut desktop).await.unwrap_err(),
            format!("No conversation found with session ID: {id}")
        );
        assert!(!desktop.lingxi_home.exists());
    }

    #[tokio::test]
    async fn continue_does_not_apply_explicit_resume_truncation_flags() {
        let root = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(root.path()).unwrap();
        let mut desktop = harness_runtime::desktop::DesktopConfig::default();
        desktop.lingxi_home = root.join("home");
        desktop.cwd = root.join("work");
        std::fs::create_dir_all(&desktop.cwd).unwrap();
        let id = uuid::Uuid::new_v4();
        crate::init::captured_resume_path_tests::transcript(
            &desktop.lingxi_home,
            &desktop.cwd,
            id,
            "continue input",
            "2026-11-09T00:00:00Z",
            "code",
        );
        let argv = Argv::try_parse_from([
            "lingxi-cli",
            "-p",
            "--continue",
            "--resume",
            &id.to_string(),
            "--resume-session-at",
            "resume-boundary",
            "--resume-drops-turn",
            "resume-turn",
            "next",
        ])
        .unwrap();
        assert!(matches!(
            select_session_start(&argv, &mut desktop).await.unwrap(),
            harness_runtime::headless::config::SessionStart::Resume {
                session_id, resume_session_at: None, resume_drops_turn: None, ..
            } if session_id == id
        ));
    }

    #[tokio::test]
    async fn headless_uuid_file_and_title_select_the_same_owned_source() {
        let root = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(root.path()).unwrap();
        let home = root.join("home");
        let cwd = root.join("work");
        std::fs::create_dir_all(&cwd).unwrap();
        let id = uuid::Uuid::new_v4();
        let path = crate::init::captured_resume_path_tests::transcript(
            &home,
            &cwd,
            id,
            "owned input",
            "2026-11-09T00:00:00Z",
            "code",
        );
        use std::io::Write;
        writeln!(std::fs::OpenOptions::new().append(true).open(&path).unwrap(),
            "{}", serde_json::json!({"type":"custom-title", "sessionId":id, "customTitle":"Owned source title"})
        ).unwrap();
        let before = std::fs::read(&path).unwrap();
        for argument in [
            id.to_string(),
            path.to_string_lossy().into_owned(),
            "Owned source title".to_string(),
        ] {
            let argv =
                Argv::try_parse_from(["lingxi-cli", "-p", "--resume", &argument, "next"]).unwrap();
            let mut desktop = harness_runtime::desktop::DesktopConfig::default();
            desktop.lingxi_home = home.clone();
            desktop.cwd = cwd.clone();
            assert!(matches!(
                select_session_start(&argv, &mut desktop).await.unwrap(),
                harness_runtime::headless::config::SessionStart::Resume { session_id, .. } if session_id == id
            ));
            assert_eq!(desktop.session_transcript_path.as_ref(), Some(&path));
        }
        // File argument handling must retain the existing profile root fence.
        let external = root.join(format!("{id}.jsonl"));
        std::fs::write(&external, &before).unwrap();
        let argv = Argv::try_parse_from([
            "lingxi-cli",
            "-p",
            "--resume",
            external.to_str().unwrap(),
            "next",
        ])
        .unwrap();
        let mut desktop = harness_runtime::desktop::DesktopConfig::default();
        desktop.lingxi_home = home;
        desktop.cwd = cwd;
        assert!(select_session_start(&argv, &mut desktop).await.is_err());
        assert_eq!(std::fs::read(path).unwrap(), before);
        assert!(!desktop.lingxi_home.join("sessions").exists());
    }

    #[test]
    fn selected_headless_resume_loads_target_project_but_fork_keeps_launch_project() {
        let root = tempfile::tempdir().unwrap();
        let root_path = std::fs::canonicalize(root.path()).unwrap();
        let launch_cwd = root_path.join("launch");
        let target_cwd = root_path.join("target");
        let home = root_path.join("home");
        std::fs::create_dir_all(&launch_cwd).unwrap();
        std::fs::create_dir_all(&target_cwd).unwrap();
        for (cwd, marker) in [(&launch_cwd, "launch"), (&target_cwd, "target")] {
            std::fs::create_dir_all(cwd.join(branding::DOT_DIR)).unwrap();
            std::fs::write(
                cwd.join(branding::DOT_DIR).join("settings.json"),
                serde_json::json!({
                    "providers": {"resume-provider": {"type":"openai", "baseUrl":format!("https://{marker}.invalid/v1"), "apiKeyEnv":"UNUSED_TEST_KEY"}},
                    "routing": {"projectMarker":marker},
                    "plansDirectory":format!("{marker}-plans")
                }).to_string(),
            ).unwrap();
            std::fs::write(cwd.join(".mcp.json"), "{\"mcpServers\":{}}").unwrap();
        }
        let id = uuid::Uuid::new_v4();
        let path = crate::init::captured_resume_path_tests::transcript(
            &home,
            &target_cwd,
            id,
            "target input",
            "2026-11-02T00:00:00Z",
            "code",
        );
        let argv = Argv::try_parse_from([
            "lingxi-cli",
            "-p",
            "--setting-sources",
            "project",
            "--add-dir",
            "relative",
        ])
        .unwrap();
        for fork in [false, true] {
            let mut launch = crate::init::resolve_desktop_config_at(
                &argv,
                permission::PermissionMode::Default,
                launch_cwd.clone(),
            );
            launch.lingxi_home = home.clone();
            let captured = crate::init::capture_selected_cli_source(
                &home,
                id,
                &path,
                Some(lingxi_core::host::session_context::ConversationMode::Code),
            )
            .unwrap();
            let content = captured.content.clone();
            let cfg = configure_selected_resume(
                &argv,
                permission::PermissionMode::Default,
                launch,
                id,
                fork,
                captured,
            )
            .unwrap();
            let expected = if fork { &launch_cwd } else { &target_cwd };
            assert_eq!(&cfg.cwd, expected);
            assert_eq!(cfg.mcp_paths[0], expected.join(".mcp.json"));
            let marker = if fork { "launch" } else { "target" };
            assert_eq!(
                cfg.provider_profiles.as_ref().unwrap()["resume-provider"]["baseUrl"],
                format!("https://{marker}.invalid/v1")
            );
            assert_eq!(cfg.routing.as_ref().unwrap()["projectMarker"], marker);
            assert_eq!(cfg.plans_directory, Some(format!("{marker}-plans")));
            assert_eq!(cfg.lingxi_home, home);
            assert_eq!(cfg.session_transcript_path.as_ref(), Some(&path));
            assert_eq!(
                cfg.session_transcript_authority
                    .as_ref()
                    .unwrap()
                    .read_checked()
                    .unwrap(),
                content.as_bytes()
            );
            assert_eq!(
                cfg.session_resume_snapshot
                    .as_ref()
                    .unwrap()
                    .content
                    .as_ref(),
                content
            );
            if !fork {
                assert_eq!(
                    cfg.add_dir,
                    vec![std::env::current_dir().unwrap().join("relative")]
                );
            }
        }
    }
}
