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
    oauth_descriptor: crate::headless_oauth::OAuthDescriptorCache,
}

#[async_trait::async_trait]
impl HeadlessHostServices for CliHeadlessServices {
    fn oauth_descriptor_credential(
        &self,
    ) -> Option<harness_runtime::headless::host::OAuthDescriptorCredential> {
        self.oauth_descriptor.credential()
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
pub(crate) async fn run(argv: &Argv, permission_mode: permission::PermissionMode) -> i32 {
    use harness_runtime::headless::config::{
        HeadlessConfig, HeadlessOptions, InputFormat, OutputFormat, SessionStart,
    };
    #[cfg(not(unix))]
    use harness_runtime::headless::HeadlessIo;
    use harness_runtime::headless::{HeadlessSignal, HeadlessSignals};

    let mut desktop = crate::init::resolve_desktop_config(argv, permission_mode);
    let background_forker = Arc::new(crate::bg_session_forker::CliBgSessionForker::new(
        desktop.lingxi_home.clone(),
        desktop.lingxi_home.clone(),
        crate::background_launch::BackgroundLaunchOptions::from_argv(argv),
        permission_mode,
    ));
    desktop.bg_session_forker = Some(background_forker.clone());
    let session_start = if argv.continue_session {
        SessionStart::Continue {
            fork: argv.fork_session,
        }
    } else if let Some(resume) = argv.resume.as_deref() {
        let session_id = match lingxi_core::types::SessionId::parse_prefixed(resume.trim()) {
            Some(session_id) => session_id.as_uuid(),
            None => match crate::run::resolve_resume_title(resume.trim()).await {
                Ok(Some(session_id)) => session_id,
                Ok(None) => {
                    eprintln!("lingxi-cli: No conversation found to resume");
                    return crate::exit_codes::RUNTIME_ERROR;
                }
                Err(error) => {
                    eprintln!("lingxi-cli: {error}");
                    return crate::exit_codes::RUNTIME_ERROR;
                }
            },
        };
        SessionStart::Resume {
            session_id,
            resume_session_at: argv.resume_session_at.clone(),
            resume_drops_turn: argv.resume_drops_turn.clone(),
            fork: argv.fork_session,
        }
    } else {
        SessionStart::New
    };
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
        oauth_descriptor: crate::headless_oauth::OAuthDescriptorCache::default(),
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
