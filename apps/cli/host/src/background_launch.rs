//! Durable launch context for PTY-backed background sessions.
//!
//! `state.json` intentionally remains the small, user-visible job row.  The
//! complete (and potentially secret-bearing) launch context lives beside it in
//! `launch.json`, written atomically with owner-only permissions.  The daemon
//! roster contains only a [`LaunchSpecRef`], so credentials and large prompt /
//! plugin settings are not copied into the fleet-wide roster.

use crate::agents_registry;
use crate::argv::Argv;
use crate::daemon_roster::{self, Dispatch, Launch};
use platform_api::rooted_fs::{self, AtomicWriteOptions};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::{Error, ErrorKind};
use std::path::{Path, PathBuf};

/// Current on-disk `launch.json` schema.
pub const LAUNCH_SPEC_VERSION: u32 = 1;
/// Filename under `jobs/<short>/`.
pub const LAUNCH_SPEC_FILE: &str = "launch.json";
/// Owner-only runtime identity for orphan process-tree cleanup.
pub const PTY_RUNTIME_FILE: &str = "pty.json";
/// Recent PTY output retained for the `logs <id>` control command.
pub const OUTPUT_LOG_FILE: &str = "output.log";
/// Hard bound for a background session's retained raw PTY output.
pub const OUTPUT_LOG_MAX_BYTES: u64 = 4 * 1024 * 1024;
/// Size restored after compaction, leaving headroom before the next rewrite.
pub const OUTPUT_LOG_RETAIN_BYTES: u64 = 3 * 1024 * 1024;
/// Refuse unexpectedly large files before parsing them.
const MAX_LAUNCH_SPEC_BYTES: u64 = 2 * 1024 * 1024;

/// The small non-secret reference stored in a roster dispatch.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LaunchSpecRef {
    /// Launch schema version expected at `path`.
    #[serde(rename = "schemaVersion")]
    pub schema_version: u32,
    /// Absolute path to the owner-only launch file.
    pub path: String,
}

/// Whether the child starts a new session or opens an existing transcript.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BackgroundLaunchKind {
    /// New session with an optional initial prompt.
    Fresh,
    /// Continue an existing session in place.
    Resume,
    /// Resume a copied transcript under its new session id.
    Fork,
}

/// Initial PTY dimensions captured at dispatch time.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct TerminalSize {
    pub cols: u16,
    pub rows: u16,
}

/// Live PTY child identity persisted by the worker. The daemon checks the
/// recorded process start time before using the numeric PID/PGID, preventing a
/// stale file from killing an unrelated process after PID reuse.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BackgroundPtyRuntime {
    /// Runtime-record schema version.
    pub schema_version: u32,
    /// Owning background job id.
    pub short: String,
    /// Supervisor worker process id.
    pub worker_pid: i32,
    /// Direct TUI child process id.
    pub child_pid: u32,
    /// Stable child start-time identity, where the platform exposes it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub child_proc_start: Option<String>,
    /// Unix process group id. Windows uses its kill-on-close Job Object.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub process_group_id: Option<u32>,
}

impl Default for TerminalSize {
    fn default() -> Self {
        Self { cols: 80, rows: 24 }
    }
}

/// CLI/runtime settings that must survive daemonisation.
///
/// SDK stream formatting and top-level routing flags are deliberately absent:
/// the PTY child is always an interactive, prompt-less, non-background TUI.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct BackgroundLaunchOptions {
    pub model: Option<String>,
    pub fallback_model: Option<String>,
    pub max_turns: Option<u32>,
    pub plan_mode_instructions: Option<String>,
    pub max_budget_usd: Option<f64>,
    pub no_stream: bool,
    pub system_prompt: Option<String>,
    pub append_system_prompt: Option<String>,
    pub system_prompt_file: Option<PathBuf>,
    pub append_system_prompt_file: Option<PathBuf>,
    pub allowed_tools: Option<Vec<String>>,
    pub disallowed_tools: Option<Vec<String>>,
    pub tools: Option<Vec<String>>,
    pub add_dir: Option<Vec<PathBuf>>,
    pub settings: Option<String>,
    pub mcp_config: Option<Vec<String>>,
    pub verbose: bool,
    pub bare: bool,
    pub safe_mode: bool,
    pub agents: Option<String>,
    pub agent: Option<String>,
    pub plugin_dir: Vec<PathBuf>,
    pub plugin_url: Vec<String>,
    pub effort: Option<String>,
    pub betas: Option<Vec<String>>,
    pub debug_file: Option<PathBuf>,
    pub thinking: Option<String>,
    pub thinking_display: Option<String>,
    pub max_thinking_tokens: Option<u32>,
    pub debug: Option<String>,
    pub dangerously_skip_permissions: bool,
    pub permission_mode: Option<String>,
    pub name: Option<String>,
    pub setting_sources: Option<String>,
    pub strict_mcp_config: bool,
    pub exclude_dynamic_system_prompt_sections: bool,
    pub mcp_debug: bool,
    pub ide: bool,
    pub permission_prompt_tool: Option<String>,
    pub allow_dangerously_skip_permissions: bool,
    pub disable_slash_commands: bool,
    pub chrome: bool,
    pub no_chrome: bool,
    pub ax_screen_reader: bool,
    pub file: Option<Vec<String>>,
    pub worktree: Option<String>,
    #[serde(default)]
    pub tmux: Option<String>,
    pub brief: bool,
}

impl BackgroundLaunchOptions {
    #[must_use]
    pub fn from_argv(argv: &Argv) -> Self {
        Self {
            model: argv.model.clone(),
            fallback_model: argv.fallback_model.clone(),
            max_turns: argv.max_turns,
            plan_mode_instructions: argv.plan_mode_instructions.clone(),
            max_budget_usd: argv.max_budget_usd,
            no_stream: argv.no_stream,
            system_prompt: argv.system_prompt.clone(),
            append_system_prompt: argv.append_system_prompt.clone(),
            system_prompt_file: argv.system_prompt_file.clone(),
            append_system_prompt_file: argv.append_system_prompt_file.clone(),
            allowed_tools: argv.allowed_tools.clone(),
            disallowed_tools: argv.disallowed_tools.clone(),
            tools: argv.tools.clone(),
            add_dir: argv.add_dir.clone(),
            settings: argv.settings.clone(),
            mcp_config: argv.mcp_config.clone(),
            verbose: argv.verbose,
            bare: argv.bare,
            safe_mode: argv.safe_mode,
            agents: argv.agents.clone(),
            agent: argv.agent.clone(),
            plugin_dir: argv.plugin_dir.clone(),
            plugin_url: argv.plugin_url.clone(),
            effort: argv.effort.clone(),
            betas: argv.betas.clone(),
            debug_file: argv.debug_file.clone(),
            thinking: argv.thinking.clone(),
            thinking_display: argv.thinking_display.clone(),
            max_thinking_tokens: argv.max_thinking_tokens,
            debug: argv.debug.clone(),
            dangerously_skip_permissions: argv.dangerously_skip_permissions,
            permission_mode: argv.permission_mode.clone(),
            name: argv.name.clone(),
            setting_sources: argv.setting_sources.clone(),
            strict_mcp_config: argv.strict_mcp_config,
            exclude_dynamic_system_prompt_sections: argv.exclude_dynamic_system_prompt_sections,
            mcp_debug: argv.mcp_debug,
            ide: argv.ide,
            permission_prompt_tool: argv.permission_prompt_tool.clone(),
            allow_dangerously_skip_permissions: argv.allow_dangerously_skip_permissions,
            disable_slash_commands: argv.disable_slash_commands,
            chrome: argv.chrome,
            no_chrome: argv.no_chrome,
            ax_screen_reader: argv.ax_screen_reader,
            file: argv.file.clone(),
            worktree: argv.worktree.clone(),
            tmux: argv.tmux.clone(),
            brief: argv.brief,
        }
    }

    /// Replace authority-bearing raw flags with the mode that the foreground
    /// process actually resolved and safety-checked. An explicit canonical
    /// mode has higher precedence than settings when the hidden child mounts;
    /// clearing the skip flag prevents it from independently re-requesting a
    /// more permissive mode after dispatch.
    pub(crate) fn freeze_permission_mode(&mut self, mode: permission::PermissionMode) {
        self.permission_mode = Some(mode.wire_str().to_string());
        self.dangerously_skip_permissions = false;
    }

    pub(crate) fn clear_boot_session_options(&mut self) {
        self.worktree = None;
        self.tmux = None;
    }

    fn apply(&self, argv: &mut Argv) {
        argv.model.clone_from(&self.model);
        argv.fallback_model.clone_from(&self.fallback_model);
        argv.max_turns = self.max_turns;
        argv.plan_mode_instructions
            .clone_from(&self.plan_mode_instructions);
        argv.max_budget_usd = self.max_budget_usd;
        argv.no_stream = self.no_stream;
        argv.system_prompt.clone_from(&self.system_prompt);
        argv.append_system_prompt
            .clone_from(&self.append_system_prompt);
        argv.system_prompt_file.clone_from(&self.system_prompt_file);
        argv.append_system_prompt_file
            .clone_from(&self.append_system_prompt_file);
        argv.allowed_tools.clone_from(&self.allowed_tools);
        argv.disallowed_tools.clone_from(&self.disallowed_tools);
        argv.tools.clone_from(&self.tools);
        argv.add_dir.clone_from(&self.add_dir);
        argv.settings.clone_from(&self.settings);
        argv.mcp_config.clone_from(&self.mcp_config);
        argv.verbose = self.verbose;
        argv.bare = self.bare;
        argv.safe_mode = self.safe_mode;
        argv.agents.clone_from(&self.agents);
        argv.agent.clone_from(&self.agent);
        argv.plugin_dir.clone_from(&self.plugin_dir);
        argv.plugin_url.clone_from(&self.plugin_url);
        argv.effort.clone_from(&self.effort);
        argv.betas.clone_from(&self.betas);
        argv.debug_file.clone_from(&self.debug_file);
        argv.thinking.clone_from(&self.thinking);
        argv.thinking_display.clone_from(&self.thinking_display);
        argv.max_thinking_tokens = self.max_thinking_tokens;
        argv.debug.clone_from(&self.debug);
        argv.dangerously_skip_permissions = self.dangerously_skip_permissions;
        argv.permission_mode.clone_from(&self.permission_mode);
        argv.name.clone_from(&self.name);
        argv.setting_sources.clone_from(&self.setting_sources);
        argv.strict_mcp_config = self.strict_mcp_config;
        argv.exclude_dynamic_system_prompt_sections = self.exclude_dynamic_system_prompt_sections;
        argv.mcp_debug = self.mcp_debug;
        argv.ide = self.ide;
        argv.permission_prompt_tool
            .clone_from(&self.permission_prompt_tool);
        argv.allow_dangerously_skip_permissions = self.allow_dangerously_skip_permissions;
        argv.disable_slash_commands = self.disable_slash_commands;
        argv.chrome = self.chrome;
        argv.no_chrome = self.no_chrome;
        argv.ax_screen_reader = self.ax_screen_reader;
        argv.file.clone_from(&self.file);
        argv.worktree.clone_from(&self.worktree);
        argv.tmux.clone_from(&self.tmux);
        argv.brief = self.brief;
    }
}

/// Complete durable context for one background PTY child.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct BackgroundLaunchSpec {
    pub schema_version: u32,
    pub short: String,
    pub created_at: i64,
    /// Set only after the foreground process completed workspace-trust and
    /// dangerous-bypass confirmation. Missing on older files means false.
    #[serde(default)]
    pub preflight_approved: bool,
    pub launch: BackgroundLaunchKind,
    pub session_id: String,
    pub transcript_path: String,
    pub cwd: String,
    pub origin_cwd: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub worktree_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub worktree_ownership_token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub initial_prompt: Option<String>,
    /// Live TUI boundary state for a mid-turn foreground→background handoff.
    /// Older launch specs omit it and resume with an empty composer/queue.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub handoff: Option<platform_api::BackgroundingSnapshot>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub shell_handoff: Vec<platform_api::shell_handoff::ShellTaskHandoff>,
    pub options: BackgroundLaunchOptions,
    /// Allowlisted environment inherited by the PTY child. `launch.json` is
    /// owner-only because this map may contain provider credentials.
    pub env: BTreeMap<String, String>,
    pub terminal: TerminalSize,
}

impl BackgroundLaunchSpec {
    /// Reconstruct the argument context for the hidden TUI child.
    ///
    /// The prompt is intentionally *not* placed in argv: a positional prompt
    /// routes the current CLI to print mode.  The child submits
    /// [`initial_prompt`](Self::initial_prompt) through the TUI after mount.
    #[must_use]
    pub fn tui_argv(&self) -> Argv {
        let mut argv = Argv {
            cwd: Some(PathBuf::from(&self.cwd)),
            ..Argv::default()
        };
        self.options.apply(&mut argv);
        argv.command = None;
        argv.prompt = None;
        argv.print = false;
        argv.background = false;
        argv.no_tui = false;
        match self.launch {
            BackgroundLaunchKind::Fresh => {
                argv.session_id = Some(self.session_id.clone());
                argv.resume = None;
            }
            BackgroundLaunchKind::Resume | BackgroundLaunchKind::Fork => {
                // Fork transcripts are already copied under `session_id` by
                // the caller. Asking the CLI to fork again would mint a third
                // id, so both variants open their recorded transcript in place.
                argv.session_id = None;
                argv.resume = Some(self.session_id.clone());
                // `--worktree`/`--tmux` are boot-only: a resumed hidden child
                // must reopen the recorded session in place rather than
                // attempting to mint a second managed worktree or tmux host.
                argv.worktree = None;
                argv.tmux = None;
            }
        }
        argv.fork_session = false;
        argv
    }

    #[must_use]
    pub fn reference(&self, config_home: &Path) -> LaunchSpecRef {
        let path = launch_spec_path(config_home, &self.short);
        LaunchSpecRef {
            schema_version: self.schema_version,
            path: std::fs::canonicalize(&path)
                .unwrap_or(path)
                .display()
                .to_string(),
        }
    }
}

#[must_use]
pub fn launch_spec_path(config_home: &Path, short: &str) -> PathBuf {
    agents_registry::jobs_dir(config_home)
        .join(short)
        .join(LAUNCH_SPEC_FILE)
}

/// Path to the live PTY process identity record.
#[must_use]
pub fn pty_runtime_path(config_home: &Path, short: &str) -> PathBuf {
    agents_registry::jobs_dir(config_home)
        .join(short)
        .join(PTY_RUNTIME_FILE)
}

/// Persist a live PTY identity atomically with owner-only permissions.
pub fn write_pty_runtime(
    config_home: &Path,
    short: &str,
    runtime: &BackgroundPtyRuntime,
) -> std::io::Result<()> {
    validate_short(short)?;
    if runtime.schema_version != 1
        || runtime.short != short
        || runtime.child_pid <= 1
        || runtime
            .child_proc_start
            .as_deref()
            .is_none_or(str::is_empty)
    {
        return Err(Error::new(
            ErrorKind::InvalidInput,
            "background PTY runtime identity is invalid",
        ));
    }
    let body = serde_json::to_vec(runtime).map_err(|e| Error::new(ErrorKind::InvalidData, e))?;
    let options = AtomicWriteOptions {
        create_parents: false,
        ..AtomicWriteOptions::default()
    };
    rooted_fs::atomic_write(
        config_home,
        &background_relative_path(short, PTY_RUNTIME_FILE),
        &body,
        options,
    )
    .map_err(rooted_error_to_io)
}

/// Read a live PTY identity without following symlinks.
pub fn read_pty_runtime(config_home: &Path, short: &str) -> std::io::Result<BackgroundPtyRuntime> {
    validate_short(short)?;
    let relative = background_relative_path(short, PTY_RUNTIME_FILE);
    let body = rooted_fs::read_to_string_limited(config_home, &relative, 16 * 1024)
        .map_err(rooted_error_to_io)?;
    let metadata = validate_private_regular_path(
        &config_home.join(&relative),
        "background PTY runtime identity",
    )?;
    reject_insecure_mode(&metadata, "background PTY runtime identity")?;
    let runtime: BackgroundPtyRuntime =
        serde_json::from_str(&body).map_err(|e| Error::new(ErrorKind::InvalidData, e))?;
    if runtime.schema_version != 1
        || runtime.short != short
        || runtime.child_pid <= 1
        || runtime
            .child_proc_start
            .as_deref()
            .is_none_or(str::is_empty)
    {
        return Err(Error::new(
            ErrorKind::InvalidData,
            "background PTY runtime identity mismatch",
        ));
    }
    Ok(runtime)
}

/// Remove a PTY runtime record after the child tree has exited.
pub fn remove_pty_runtime(config_home: &Path, short: &str) {
    if validate_short(short).is_ok() {
        let _ = rooted_fs::remove_file(
            config_home,
            &background_relative_path(short, PTY_RUNTIME_FILE),
        );
    }
}

/// Atomically persist `launch.json` with directory `0700` and file `0600` on
/// Unix. The temp file uses `create_new` and is renamed only after `sync_all`.
pub fn write_launch_spec(
    config_home: &Path,
    short: &str,
    spec: &BackgroundLaunchSpec,
) -> std::io::Result<()> {
    validate_short(short)?;
    if spec.schema_version != LAUNCH_SPEC_VERSION || spec.short != short {
        return Err(Error::new(
            ErrorKind::InvalidInput,
            "background launch spec identity/version mismatch",
        ));
    }
    let body =
        serde_json::to_vec_pretty(spec).map_err(|e| Error::new(ErrorKind::InvalidData, e))?;
    if body.len() as u64 > MAX_LAUNCH_SPEC_BYTES {
        return Err(Error::new(
            ErrorKind::InvalidData,
            "launch spec is too large",
        ));
    }
    std::fs::create_dir_all(config_home)?;
    rooted_fs::atomic_write(
        config_home,
        &background_relative_path(short, LAUNCH_SPEC_FILE),
        &body,
        AtomicWriteOptions::default(),
    )
    .map_err(rooted_error_to_io)
}

/// Cross-process shell ownership acknowledgement. Each phase has a separate
/// file so a failed prepare cannot overwrite a committed ownership decision.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct ShellHandoffAck {
    pub task_ids: Vec<String>,
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<ShellHandoffSource>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct ShellHandoffSource {
    pub pid: i32,
    pub process_start: String,
}

fn shell_ack_file(phase: &str) -> std::io::Result<String> {
    if !matches!(phase, "intent" | "ready" | "commit" | "adopted" | "abort") {
        return Err(Error::new(
            ErrorKind::InvalidInput,
            "invalid shell handoff phase",
        ));
    }
    Ok(format!("shell-handoff-{phase}.json"))
}

pub(crate) fn write_shell_handoff_ack(
    home: &Path,
    short: &str,
    phase: &str,
    ack: &ShellHandoffAck,
) -> std::io::Result<()> {
    validate_short(short)?;
    let file = shell_ack_file(phase)?;
    let body =
        serde_json::to_vec(ack).map_err(|error| Error::new(ErrorKind::InvalidData, error))?;
    rooted_fs::atomic_write(
        home,
        &background_relative_path(short, &file),
        &body,
        AtomicWriteOptions::default(),
    )
    .map_err(rooted_error_to_io)
}

pub(crate) fn read_shell_handoff_ack(
    home: &Path,
    short: &str,
    phase: &str,
) -> std::io::Result<Option<ShellHandoffAck>> {
    validate_short(short)?;
    let file = shell_ack_file(phase)?;
    let relative = background_relative_path(short, &file);
    let body = match rooted_fs::read_to_string_limited(home, &relative, MAX_LAUNCH_SPEC_BYTES)
        .map_err(rooted_error_to_io)
    {
        Ok(body) => body,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let metadata =
        validate_private_regular_path(&home.join(&relative), "shell handoff acknowledgement")?;
    reject_insecure_mode(&metadata, "shell handoff acknowledgement")?;
    serde_json::from_str(&body)
        .map(Some)
        .map_err(|error| Error::new(ErrorKind::InvalidData, error))
}

pub(crate) async fn wait_shell_handoff_ack(
    home: &Path,
    short: &str,
    phase: &str,
) -> std::io::Result<ShellHandoffAck> {
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        if let Some(abort) = read_shell_handoff_ack(home, short, "abort")? {
            return Err(Error::new(
                ErrorKind::Interrupted,
                abort
                    .error
                    .unwrap_or_else(|| "shell handoff aborted".into()),
            ));
        }
        if let Some(ack) = read_shell_handoff_ack(home, short, phase)? {
            return Ok(ack);
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(Error::new(
                ErrorKind::TimedOut,
                format!("shell handoff {phase} acknowledgement timed out"),
            ));
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
}

/// Read and validate an owner-controlled launch spec. Symlinks and
/// non-regular files are rejected rather than followed.
pub fn read_launch_spec(config_home: &Path, short: &str) -> std::io::Result<BackgroundLaunchSpec> {
    validate_short(short)?;
    let relative = background_relative_path(short, LAUNCH_SPEC_FILE);
    let body = rooted_fs::read_to_string_limited(config_home, &relative, MAX_LAUNCH_SPEC_BYTES)
        .map_err(rooted_error_to_io)?;
    let metadata =
        validate_private_regular_path(&config_home.join(&relative), "background launch spec")?;
    reject_insecure_mode(&metadata, "background launch spec")?;
    let spec: BackgroundLaunchSpec =
        serde_json::from_str(&body).map_err(|e| Error::new(ErrorKind::InvalidData, e))?;
    if spec.schema_version != LAUNCH_SPEC_VERSION || spec.short != short {
        return Err(Error::new(
            ErrorKind::InvalidData,
            "background launch spec identity/version mismatch",
        ));
    }
    Ok(spec)
}

fn refresh_current_background_launch_identity_inner(
    cwd: &Path,
    transcript_path: &Path,
    after_launch_write: impl FnOnce(&Path, &str, &str) -> std::io::Result<()>,
) -> std::io::Result<bool> {
    refresh_current_background_launch_identity_with(
        cwd,
        transcript_path,
        write_launch_spec,
        after_launch_write,
    )
}

/// Refresh a live background session's durable cwd/transcript identity.
///
/// The writer is injected so tests can force a launch-spec failure after the
/// transcript has been moved and verify callers roll the move back. The
/// launch spec is authoritative; a best-effort state-row mirror failure is
/// logged but does not make the already-published launch identity stale.
fn refresh_current_background_launch_identity_with<W, A>(
    cwd: &Path,
    transcript_path: &Path,
    write_launch: W,
    after_launch_write: A,
) -> std::io::Result<bool>
where
    W: FnOnce(&Path, &str, &BackgroundLaunchSpec) -> std::io::Result<()>,
    A: FnOnce(&Path, &str, &str) -> std::io::Result<()>,
{
    let Ok(job_dir_raw) = std::env::var("LINGXI_JOB_DIR") else {
        return Ok(false);
    };
    let job_dir = PathBuf::from(job_dir_raw);
    let short = job_dir
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| Error::new(ErrorKind::InvalidInput, "background job dir is malformed"))?;
    validate_short(short)?;
    let config_home = job_dir
        .parent()
        .and_then(Path::parent)
        .ok_or_else(|| Error::new(ErrorKind::InvalidInput, "background job dir is malformed"))?;
    let _job_lock = agents_registry::lock_job_state(config_home, short)?;
    let mut spec = read_launch_spec(config_home, short)?;
    let expected_name = format!("{}.jsonl", spec.session_id);
    let projects_root = config_home.join("projects");
    let transcript_path = if let Ok(metadata) = std::fs::symlink_metadata(transcript_path) {
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(Error::new(
                ErrorKind::InvalidInput,
                "transcript is not a regular file",
            ));
        }
        let canonical_transcript = std::fs::canonicalize(transcript_path)?;
        let canonical_projects_root = std::fs::canonicalize(&projects_root)?;
        if !canonical_transcript.starts_with(&canonical_projects_root)
            || canonical_transcript
                .file_name()
                .and_then(|name| name.to_str())
                != Some(expected_name.as_str())
        {
            return Err(Error::new(
                ErrorKind::InvalidInput,
                "transcript path does not match the recorded session",
            ));
        }
        canonical_transcript
    } else {
        if !transcript_path.is_absolute()
            || transcript_path.file_name().and_then(|name| name.to_str())
                != Some(expected_name.as_str())
            || transcript_path.strip_prefix(&projects_root).is_err()
        {
            return Err(Error::new(
                ErrorKind::InvalidInput,
                "transcript path does not match the recorded session",
            ));
        }
        transcript_path.to_path_buf()
    };
    spec.cwd = cwd.display().to_string();
    spec.transcript_path = transcript_path.display().to_string();
    write_launch(config_home, short, &spec)?;
    if let Err(error) = after_launch_write(config_home, short, &spec.cwd) {
        tracing::warn!(
            short,
            %error,
            "failed to mirror background launch identity into job state; launch spec remains authoritative"
        );
    }
    Ok(true)
}

pub fn refresh_current_background_launch_identity(
    cwd: &Path,
    transcript_path: &Path,
) -> std::io::Result<bool> {
    refresh_current_background_launch_identity_inner(
        cwd,
        transcript_path,
        |config_home, short, cwd| {
            agents_registry::update_job_cwd_with_lock_held(config_home, short, cwd)
        },
    )
}

pub fn reconcile_job_cwd_from_launch_spec(
    config_home: &Path,
    short: &str,
) -> std::io::Result<bool> {
    let _job_lock = agents_registry::lock_job_state(config_home, short)?;
    let spec = read_launch_spec(config_home, short)?;
    let Some(job) = agents_registry::read_job(config_home, short) else {
        return Ok(false);
    };
    if job.cwd.as_deref() == Some(spec.cwd.as_str()) {
        return Ok(false);
    }
    agents_registry::update_job_cwd_with_lock_held(config_home, short, &spec.cwd)?;
    Ok(true)
}

/// Load `launch.json`, or convert a protocol-v1 job/roster record once.
///
/// `Ok(None)` means neither a launch file nor enough legacy state exists. A
/// successfully migrated roster is rewritten with a non-secret launch ref and
/// its old environment map cleared.
pub fn load_or_migrate_launch_spec(
    config_home: &Path,
    runtime_dir: &Path,
    short: &str,
) -> std::io::Result<Option<BackgroundLaunchSpec>> {
    match read_launch_spec(config_home, short) {
        Ok(spec) => {
            let _ = reconcile_job_cwd_from_launch_spec(config_home, short);
            return Ok(Some(spec));
        }
        Err(e) if e.kind() == ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    let Some(job) = agents_registry::read_job(config_home, short) else {
        return Ok(None);
    };
    let roster = daemon_roster::read_roster(runtime_dir, 0, false).into_roster();
    let Some(record) = roster.workers.get(short) else {
        return Ok(None);
    };
    let spec = from_legacy_dispatch(config_home, short, &job, &record.dispatch);
    write_launch_spec(config_home, short, &spec)?;
    Ok(Some(spec))
}

fn from_legacy_dispatch(
    config_home: &Path,
    short: &str,
    job: &agents_registry::JobState,
    dispatch: &Dispatch,
) -> BackgroundLaunchSpec {
    let (launch, transcript_path, raw_args) = match &dispatch.launch {
        Launch::Prompt { args } => (BackgroundLaunchKind::Fresh, None, args.clone()),
        Launch::Resume {
            transcript_path,
            fork,
            flag_args,
            ..
        } => (
            if *fork {
                BackgroundLaunchKind::Fork
            } else {
                BackgroundLaunchKind::Resume
            },
            transcript_path.clone(),
            flag_args.clone(),
        ),
        Launch::Exec { .. } => (BackgroundLaunchKind::Fresh, None, Vec::new()),
    };
    let mut options = argv_from_legacy_args(&raw_args)
        .as_ref()
        .map(BackgroundLaunchOptions::from_argv)
        .unwrap_or_default();
    if matches!(
        launch,
        BackgroundLaunchKind::Resume | BackgroundLaunchKind::Fork
    ) {
        options.clear_boot_session_options();
    }
    let cwd = job.cwd.clone().unwrap_or_else(|| dispatch.cwd.clone());
    let session_id = job
        .session_id
        .clone()
        .unwrap_or_else(|| dispatch.session_id.clone());
    let transcript_path = transcript_path.unwrap_or_else(|| {
        session::jsonl::path::session_path(config_home, &cwd, &session_id)
            .display()
            .to_string()
    });
    BackgroundLaunchSpec {
        schema_version: LAUNCH_SPEC_VERSION,
        short: short.to_string(),
        created_at: dispatch.created_at,
        // Protocol-v1 records predate foreground trust/bypass preflight. They
        // are migrated for inspection but deliberately cannot launch a hidden
        // TUI until a foreground path re-authorizes them.
        preflight_approved: false,
        launch,
        session_id,
        transcript_path,
        cwd: cwd.clone(),
        origin_cwd: job.origin_cwd.clone().unwrap_or(cwd),
        worktree_path: dispatch
            .worktree
            .as_ref()
            .map(|worktree| worktree.path.clone()),
        worktree_ownership_token: dispatch
            .worktree
            .as_ref()
            .map(|worktree| worktree.ownership_token.clone()),
        initial_prompt: job
            .initial_prompt
            .clone()
            .filter(|prompt| !prompt.trim().is_empty()),
        shell_handoff: Vec::new(),
        handoff: None,
        options,
        env: dispatch.env.clone(),
        terminal: TerminalSize {
            cols: u16::try_from(dispatch.cols.unwrap_or(80)).unwrap_or(u16::MAX),
            rows: u16::try_from(dispatch.rows.unwrap_or(24)).unwrap_or(u16::MAX),
        },
    }
}

fn argv_from_legacy_args(args: &[String]) -> Option<Argv> {
    let mut tokens = Vec::with_capacity(args.len() + 1);
    tokens.push("lingxi-cli".to_string());
    tokens.extend(args.iter().cloned());
    Argv::from_iter(tokens).ok()
}

fn validate_short(short: &str) -> std::io::Result<()> {
    if short.len() == 8
        && short
            .as_bytes()
            .iter()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(byte))
    {
        Ok(())
    } else {
        Err(Error::new(
            ErrorKind::InvalidInput,
            "invalid background job id",
        ))
    }
}

fn background_relative_path(short: &str, file: &str) -> PathBuf {
    PathBuf::from("jobs").join(short).join(file)
}

fn rooted_error_to_io(error: platform_api::FsError) -> std::io::Error {
    let kind = match error {
        platform_api::FsError::NotFound(_) => ErrorKind::NotFound,
        platform_api::FsError::PermissionDenied(_) => ErrorKind::PermissionDenied,
        platform_api::FsError::AlreadyExists(_) => ErrorKind::AlreadyExists,
        platform_api::FsError::OutsideWorkspace(_)
        | platform_api::FsError::BinaryFile(_)
        | platform_api::FsError::TooLarge { .. } => ErrorKind::InvalidData,
        platform_api::FsError::Io(_) => ErrorKind::Other,
    };
    Error::new(kind, error)
}

fn validate_private_regular_path(path: &Path, label: &str) -> std::io::Result<std::fs::Metadata> {
    let metadata = std::fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        Err(Error::new(
            ErrorKind::InvalidData,
            format!("{label} is not a regular file"),
        ))
    } else {
        Ok(metadata)
    }
}

#[cfg(unix)]
fn reject_insecure_mode(metadata: &std::fs::Metadata, label: &str) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    if metadata.permissions().mode() & 0o077 != 0 {
        Err(Error::new(
            ErrorKind::PermissionDenied,
            format!("{label} is accessible by another user"),
        ))
    } else {
        Ok(())
    }
}

#[cfg(not(unix))]
fn reject_insecure_mode(_metadata: &std::fs::Metadata, _label: &str) -> std::io::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn tmpdir() -> PathBuf {
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let mut path = std::env::temp_dir();
        path.push(format!(
            "lingxi-launch-spec-test-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    fn sample(short: &str, launch: BackgroundLaunchKind) -> BackgroundLaunchSpec {
        BackgroundLaunchSpec {
            schema_version: LAUNCH_SPEC_VERSION,
            short: short.to_string(),
            created_at: 123,
            preflight_approved: true,
            launch,
            session_id: "11111111-2222-3333-4444-555555555555".to_string(),
            transcript_path: "/tmp/session.jsonl".to_string(),
            cwd: "/tmp/project".to_string(),
            origin_cwd: "/tmp/project".to_string(),
            worktree_path: None,
            worktree_ownership_token: None,
            initial_prompt: Some("hello".to_string()),
            shell_handoff: Vec::new(),
            handoff: None,
            options: BackgroundLaunchOptions {
                model: Some("test-model".to_string()),
                allowed_tools: Some(vec!["Read".to_string()]),
                permission_mode: Some("plan".to_string()),
                tmux: Some("classic".to_string()),
                ..BackgroundLaunchOptions::default()
            },
            env: BTreeMap::from([("ANTHROPIC_API_KEY".to_string(), "secret".to_string())]),
            terminal: TerminalSize {
                cols: 132,
                rows: 43,
            },
        }
    }

    fn seed_job(home: &std::path::Path, short: &str, session_id: &str, cwd: &str) {
        let flags: Vec<String> = Vec::new();
        agents_registry::write_job_state(
            home,
            short,
            &agents_registry::JobStateWrite {
                state: "working",
                tempo: Some("active"),
                name: None,
                session_id: Some(session_id),
                cwd: Some(cwd),
                origin_cwd: Some(cwd),
                created_at: Some("2026-07-04T00:00:00.000Z"),
                intent: Some("resume"),
                display_intent: None,
                template: Some("bg"),
                respawn_flags: &flags,
                in_flight: None,
                backend: Some("daemon"),
                initial_prompt: None,
                detail: None,
                worker_pid: None,
                worker_proc_start: None,
                phase: Some("queued"),
                worker_generation: Some("gen-1"),
                claim_token: None,
                claim_owner: None,
                claim_created_at: None,
                claim_lease_ms: None,
            },
        )
        .unwrap();
    }

    #[test]
    fn launch_spec_round_trips_atomically_and_tui_argv_is_promptless() {
        let home = tmpdir();
        let mut spec = sample("abcd1234", BackgroundLaunchKind::Fresh);
        spec.handoff = Some(platform_api::BackgroundingSnapshot::Idle {
            queued_commands: vec!["/compact keep tests".into()],
            draft: "draft 🦀".into(),
            boundary_id: uuid::Uuid::new_v4(),
        });
        write_launch_spec(&home, "abcd1234", &spec).unwrap();
        let read = read_launch_spec(&home, "abcd1234").unwrap();
        assert_eq!(read, spec);
        assert!(!launch_spec_path(&home, "abcd1234")
            .with_extension(format!("tmp.{}", std::process::id()))
            .exists());
        let argv = read.tui_argv();
        assert_eq!(argv.prompt, None);
        assert!(!argv.print);
        assert!(!argv.background);
        assert_eq!(argv.model.as_deref(), Some("test-model"));
        assert_eq!(argv.tmux.as_deref(), Some("classic"));
        assert_eq!(argv.session_id.as_deref(), Some(read.session_id.as_str()));
        assert_eq!(argv.resume, None);

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(launch_spec_path(&home, "abcd1234"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(mode, 0o600);
        }
    }

    #[test]
    fn refresh_current_background_launch_identity_retargets_cwd_and_transcript() {
        let _env_guard = ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let home = tmpdir();
        let short = "cafe0009";
        let session_id = "11111111-2222-3333-4444-555555555555";
        let old_cwd = home.join("old");
        let new_cwd = home.join("new");
        std::fs::create_dir_all(&old_cwd).unwrap();
        std::fs::create_dir_all(&new_cwd).unwrap();
        let transcript = home
            .join("projects")
            .join("workspace")
            .join(format!("{session_id}.jsonl"));
        std::fs::create_dir_all(transcript.parent().unwrap()).unwrap();
        std::fs::write(
            &transcript,
            "{\"type\":\"user\",\"message\":{\"role\":\"user\"}}\n",
        )
        .unwrap();

        let mut spec = sample(short, BackgroundLaunchKind::Fresh);
        spec.session_id = session_id.to_string();
        spec.cwd = old_cwd.display().to_string();
        spec.origin_cwd = old_cwd.display().to_string();
        spec.transcript_path = transcript.display().to_string();
        write_launch_spec(&home, short, &spec).unwrap();
        seed_job(&home, short, session_id, &old_cwd.display().to_string());

        let job_dir = agents_registry::jobs_dir(&home).join(short);
        let prior = std::env::var_os("LINGXI_JOB_DIR");
        std::env::set_var("LINGXI_JOB_DIR", &job_dir);
        let refreshed = refresh_current_background_launch_identity(&new_cwd, &transcript).unwrap();
        match prior {
            Some(value) => std::env::set_var("LINGXI_JOB_DIR", value),
            None => std::env::remove_var("LINGXI_JOB_DIR"),
        }

        assert!(refreshed);
        let updated = read_launch_spec(&home, short).unwrap();
        let new_cwd_s = new_cwd.display().to_string();
        assert_eq!(updated.cwd, new_cwd_s);
        assert_eq!(
            updated.transcript_path,
            std::fs::canonicalize(&transcript)
                .unwrap()
                .display()
                .to_string()
        );
        let job = agents_registry::read_job(&home, short).unwrap();
        assert_eq!(job.cwd.as_deref(), Some(new_cwd_s.as_str()));
    }

    #[test]
    fn refresh_current_background_launch_identity_accepts_future_transcript_path() {
        let _env_guard = ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let home = tmpdir();
        let short = "cafe0010";
        let session_id = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";
        let old_cwd = home.join("old");
        let new_cwd = home.join("new");
        std::fs::create_dir_all(&old_cwd).unwrap();
        std::fs::create_dir_all(&new_cwd).unwrap();
        let future_transcript = home
            .join("projects")
            .join("workspace")
            .join("nested")
            .join(format!("{session_id}.jsonl"));

        let mut spec = sample(short, BackgroundLaunchKind::Fresh);
        spec.session_id = session_id.to_string();
        spec.cwd = old_cwd.display().to_string();
        spec.origin_cwd = old_cwd.display().to_string();
        spec.transcript_path = home
            .join("projects")
            .join("old.jsonl")
            .display()
            .to_string();
        write_launch_spec(&home, short, &spec).unwrap();
        seed_job(&home, short, session_id, &old_cwd.display().to_string());

        let job_dir = agents_registry::jobs_dir(&home).join(short);
        let prior = std::env::var_os("LINGXI_JOB_DIR");
        std::env::set_var("LINGXI_JOB_DIR", &job_dir);
        let refreshed =
            refresh_current_background_launch_identity(&new_cwd, &future_transcript).unwrap();
        match prior {
            Some(value) => std::env::set_var("LINGXI_JOB_DIR", value),
            None => std::env::remove_var("LINGXI_JOB_DIR"),
        }

        assert!(refreshed);
        let updated = read_launch_spec(&home, short).unwrap();
        let new_cwd_s = new_cwd.display().to_string();
        assert_eq!(updated.cwd, new_cwd_s);
        assert_eq!(
            updated.transcript_path,
            future_transcript.display().to_string()
        );
        let job = agents_registry::read_job(&home, short).unwrap();
        assert_eq!(job.cwd.as_deref(), Some(new_cwd_s.as_str()));
    }

    #[test]
    fn job_state_mirror_failure_keeps_launch_spec_authoritative() {
        let _env_guard = ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let home = tmpdir();
        let short = "cafe0011";
        let session_id = "bbbbbbbb-cccc-dddd-eeee-ffffffffffff";
        let old_cwd = home.join("old");
        let new_cwd = home.join("new");
        std::fs::create_dir_all(&old_cwd).unwrap();
        std::fs::create_dir_all(&new_cwd).unwrap();
        let transcript = home
            .join("projects")
            .join("workspace")
            .join(format!("{session_id}.jsonl"));
        std::fs::create_dir_all(transcript.parent().unwrap()).unwrap();
        std::fs::write(
            &transcript,
            "{\"type\":\"user\",\"message\":{\"role\":\"user\"}}\n",
        )
        .unwrap();

        let flags: Vec<String> = Vec::new();
        agents_registry::write_job_state(
            &home,
            short,
            &agents_registry::JobStateWrite {
                state: "working",
                tempo: Some("active"),
                name: None,
                session_id: Some(session_id),
                cwd: Some(&old_cwd.display().to_string()),
                origin_cwd: Some(&old_cwd.display().to_string()),
                created_at: Some("2026-07-04T00:00:00.000Z"),
                intent: Some("resume"),
                display_intent: None,
                template: Some("bg"),
                respawn_flags: &flags,
                in_flight: None,
                backend: Some("daemon"),
                initial_prompt: None,
                detail: None,
                worker_pid: None,
                worker_proc_start: None,
                phase: Some("queued"),
                worker_generation: Some("gen-1"),
                claim_token: None,
                claim_owner: None,
                claim_created_at: None,
                claim_lease_ms: None,
            },
        )
        .unwrap();

        let mut spec = sample(short, BackgroundLaunchKind::Fresh);
        spec.session_id = session_id.to_string();
        spec.cwd = old_cwd.display().to_string();
        spec.origin_cwd = old_cwd.display().to_string();
        spec.transcript_path = transcript.display().to_string();
        write_launch_spec(&home, short, &spec).unwrap();

        let job_dir = agents_registry::jobs_dir(&home).join(short);
        let prior = std::env::var_os("LINGXI_JOB_DIR");
        std::env::set_var("LINGXI_JOB_DIR", &job_dir);
        let refreshed = refresh_current_background_launch_identity_inner(
            &new_cwd,
            &transcript,
            |_config_home, _short, _cwd| Err(Error::other("simulated second write failure")),
        )
        .unwrap();
        match prior {
            Some(value) => std::env::set_var("LINGXI_JOB_DIR", value),
            None => std::env::remove_var("LINGXI_JOB_DIR"),
        }
        assert!(refreshed);

        let launch = read_launch_spec(&home, short).unwrap();
        assert_eq!(launch.cwd, new_cwd.display().to_string());
        let stale_job = agents_registry::read_job(&home, short).unwrap();
        assert_eq!(
            stale_job.cwd.as_deref(),
            Some(old_cwd.display().to_string().as_str())
        );

        assert!(reconcile_job_cwd_from_launch_spec(&home, short).unwrap());
        let repaired_job = agents_registry::read_job(&home, short).unwrap();
        assert_eq!(
            repaired_job.cwd.as_deref(),
            Some(new_cwd.display().to_string().as_str())
        );
    }

    #[test]
    fn injected_launch_write_failure_leaves_old_resumable_identity_untouched() {
        let _env_guard = ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let home = tmpdir();
        let short = "cafe0012";
        let session_id = "cccccccc-dddd-eeee-ffff-000000000000";
        let old_cwd = home.join("old");
        let new_cwd = home.join("new");
        std::fs::create_dir_all(&old_cwd).unwrap();
        std::fs::create_dir_all(&new_cwd).unwrap();
        let transcript = home
            .join("projects")
            .join("workspace")
            .join(format!("{session_id}.jsonl"));
        std::fs::create_dir_all(transcript.parent().unwrap()).unwrap();
        std::fs::write(
            &transcript,
            "{\"type\":\"user\",\"message\":{\"role\":\"user\"}}\n",
        )
        .unwrap();

        let mut spec = sample(short, BackgroundLaunchKind::Resume);
        spec.session_id = session_id.to_string();
        spec.cwd = old_cwd.display().to_string();
        spec.origin_cwd = old_cwd.display().to_string();
        spec.transcript_path = transcript.display().to_string();
        write_launch_spec(&home, short, &spec).unwrap();

        let job_dir = agents_registry::jobs_dir(&home).join(short);
        let prior = std::env::var_os("LINGXI_JOB_DIR");
        std::env::set_var("LINGXI_JOB_DIR", &job_dir);
        let error = refresh_current_background_launch_identity_with(
            &new_cwd,
            &transcript,
            |_config_home, _short, _spec| Err(Error::other("simulated launch write failure")),
            |_config_home, _short, _cwd| panic!("state mirror must not run after launch failure"),
        )
        .unwrap_err();
        match prior {
            Some(value) => std::env::set_var("LINGXI_JOB_DIR", value),
            None => std::env::remove_var("LINGXI_JOB_DIR"),
        }

        assert!(error.to_string().contains("simulated launch write failure"));
        let launch = read_launch_spec(&home, short).unwrap();
        assert_eq!(launch.launch, BackgroundLaunchKind::Resume);
        assert_eq!(launch.cwd, old_cwd.display().to_string());
        assert_eq!(launch.transcript_path, transcript.display().to_string());
    }

    #[test]
    fn resume_and_fork_argv_open_recorded_session_without_forking_again() {
        for launch in [BackgroundLaunchKind::Resume, BackgroundLaunchKind::Fork] {
            let mut spec = sample("abcd1234", launch);
            spec.options.worktree = Some("feature".to_string());
            spec.options.tmux = Some("classic".to_string());
            let argv = spec.tui_argv();
            assert_eq!(argv.resume.as_deref(), Some(spec.session_id.as_str()));
            assert_eq!(argv.session_id, None);
            assert!(!argv.fork_session);
            assert_eq!(argv.worktree, None);
            assert_eq!(argv.tmux, None);
        }
    }

    #[test]
    fn session_download_flags_round_trip_into_pty_child() {
        let mut spec = sample("abcd1234", BackgroundLaunchKind::Fresh);
        spec.options.plugin_url = vec!["https://plugins.example.test/a.zip".to_string()];
        spec.options.file = Some(vec!["file_123:fixtures/input.txt".to_string()]);
        spec.options.betas = Some(vec!["context-1m-2025-08-07".to_string()]);

        let argv = spec.tui_argv();
        assert_eq!(argv.plugin_url, spec.options.plugin_url);
        assert_eq!(argv.file, spec.options.file);
        assert_eq!(argv.betas, spec.options.betas);
    }

    #[test]
    fn resolved_permission_mode_overrides_stale_dangerous_launch_flags() {
        let mut spec = sample("abcd1234", BackgroundLaunchKind::Fresh);
        spec.options.permission_mode = Some("bypassPermissions".to_string());
        spec.options.dangerously_skip_permissions = true;
        spec.options
            .freeze_permission_mode(permission::PermissionMode::Default);

        let argv = spec.tui_argv();
        assert_eq!(argv.permission_mode.as_deref(), Some("default"));
        assert!(!argv.dangerously_skip_permissions);
    }

    #[test]
    fn rejects_identity_mismatch_and_symlink() {
        let home = tmpdir();
        let spec = sample("abcd1234", BackgroundLaunchKind::Fresh);
        assert_eq!(
            write_launch_spec(&home, "ffffffff", &spec)
                .unwrap_err()
                .kind(),
            ErrorKind::InvalidInput
        );

        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;
            let target = home.join("target.json");
            std::fs::write(&target, b"{}").unwrap();
            let path = launch_spec_path(&home, "beef0001");
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            symlink(target, path).unwrap();
            assert_eq!(
                read_launch_spec(&home, "beef0001").unwrap_err().kind(),
                ErrorKind::InvalidData
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn launch_writer_rejects_symlinked_job_directory_without_touching_victim() {
        use std::os::unix::fs::symlink;

        let home = tmpdir();
        let victim = tempfile::tempdir().unwrap();
        let short = "beef0002";
        std::fs::create_dir_all(agents_registry::jobs_dir(&home)).unwrap();
        symlink(victim.path(), agents_registry::jobs_dir(&home).join(short)).unwrap();

        let error = write_launch_spec(&home, short, &sample(short, BackgroundLaunchKind::Fresh))
            .unwrap_err();
        assert_eq!(error.kind(), ErrorKind::InvalidData);
        assert!(!victim.path().join(LAUNCH_SPEC_FILE).exists());
    }

    #[cfg(unix)]
    #[test]
    fn launch_writer_rejects_final_symlink_without_touching_victim() {
        use std::os::unix::fs::symlink;

        let home = tmpdir();
        let short = "beef0003";
        let victim = home.join("victim.json");
        std::fs::write(&victim, "protected").unwrap();
        let launch_path = launch_spec_path(&home, short);
        std::fs::create_dir_all(launch_path.parent().unwrap()).unwrap();
        symlink(&victim, &launch_path).unwrap();

        assert!(
            write_launch_spec(&home, short, &sample(short, BackgroundLaunchKind::Fresh),).is_err()
        );
        assert_eq!(std::fs::read_to_string(victim).unwrap(), "protected");
    }

    #[cfg(unix)]
    #[test]
    fn launch_reader_rejects_group_or_world_access() {
        use std::os::unix::fs::PermissionsExt;

        let home = tmpdir();
        let short = "cafe0004";
        write_launch_spec(&home, short, &sample(short, BackgroundLaunchKind::Fresh)).unwrap();
        let path = launch_spec_path(&home, short);
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o640)).unwrap();
        assert_eq!(
            read_launch_spec(&home, short).unwrap_err().kind(),
            ErrorKind::PermissionDenied
        );
    }

    #[test]
    fn pty_runtime_round_trips_owner_only_and_rejects_unsafe_identity() {
        let home = tmpdir();
        let short = "cafe0002";
        write_launch_spec(&home, short, &sample(short, BackgroundLaunchKind::Fresh)).unwrap();
        let runtime = BackgroundPtyRuntime {
            schema_version: 1,
            short: short.to_string(),
            worker_pid: 4100,
            child_pid: 4200,
            child_proc_start: Some("Mon Jul 21 12:00:00 2026".to_string()),
            process_group_id: Some(4200),
        };
        write_pty_runtime(&home, short, &runtime).unwrap();
        assert_eq!(read_pty_runtime(&home, short).unwrap(), runtime);

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(pty_runtime_path(&home, short))
                .unwrap()
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(mode, 0o600);
        }

        let mut missing_start = runtime.clone();
        missing_start.child_proc_start = None;
        assert_eq!(
            write_pty_runtime(&home, short, &missing_start)
                .unwrap_err()
                .kind(),
            ErrorKind::InvalidInput
        );
    }

    #[cfg(unix)]
    #[test]
    fn pty_runtime_reader_rejects_symlink_substitution() {
        use std::os::unix::fs::symlink;

        let home = tmpdir();
        let short = "cafe0003";
        write_launch_spec(&home, short, &sample(short, BackgroundLaunchKind::Fresh)).unwrap();
        let target = home.join("attacker-pty.json");
        std::fs::write(
            &target,
            br#"{"schema_version":1,"short":"cafe0003","worker_pid":9,"child_pid":10,"child_proc_start":"x"}"#,
        )
        .unwrap();
        symlink(&target, pty_runtime_path(&home, short)).unwrap();
        assert_eq!(
            read_pty_runtime(&home, short).unwrap_err().kind(),
            ErrorKind::InvalidData
        );
    }

    #[test]
    fn legacy_resume_converts_to_versioned_exact_transcript_context() {
        let home = tmpdir();
        let short = "cafe0001";
        let session_id = "11111111-2222-3333-4444-555555555555";
        let flags = Vec::new();
        let job = agents_registry::JobStateWrite {
            state: "working",
            tempo: Some("active"),
            name: None,
            session_id: Some(session_id),
            cwd: Some("/tmp/project"),
            origin_cwd: Some("/tmp/origin"),
            created_at: None,
            intent: None,
            display_intent: None,
            template: Some("bg"),
            respawn_flags: &flags,
            in_flight: None,
            backend: Some("daemon"),
            initial_prompt: Some("continue"),
            detail: None,
            worker_pid: None,
            worker_proc_start: None,
            phase: None,
            worker_generation: None,
            claim_token: None,
            claim_owner: None,
            claim_created_at: None,
            claim_lease_ms: None,
        };
        agents_registry::write_job_state(&home, short, &job).unwrap();
        let job = agents_registry::read_job(&home, short).unwrap();
        let exact = home.join("custom/session-copy.jsonl").display().to_string();
        let dispatch = Dispatch {
            proto: 1,
            short: short.to_string(),
            nonce: None,
            session_id: session_id.to_string(),
            created_at: 77,
            source: daemon_roster::DispatchSource::Shell,
            cwd: "/tmp/project".to_string(),
            launch: Launch::Resume {
                session_id: session_id.to_string(),
                transcript_path: Some(exact.clone()),
                fork: false,
                flag_args: vec![
                    "--worktree".to_string(),
                    "feature".to_string(),
                    "--tmux=classic".to_string(),
                ],
            },
            launch_spec: None,
            env: BTreeMap::from([("ANTHROPIC_API_KEY".to_string(), "secret".to_string())]),
            reattach_env: None,
            worktree: Some(daemon_roster::Worktree {
                path: "/tmp/project/.lingxi/worktrees/feature".to_string(),
                ownership_token: "token-123".to_string(),
            }),
            isolation: daemon_roster::Isolation::None,
            respawn_flags: Vec::new(),
            attach_stall_respawns: None,
            agent: None,
            routine: None,
            seed: None,
            cols: Some(101),
            rows: Some(37),
        };
        let spec = from_legacy_dispatch(&home, short, &job, &dispatch);
        assert!(
            !spec.preflight_approved,
            "legacy migration must fail closed"
        );
        assert_eq!(spec.launch, BackgroundLaunchKind::Resume);
        assert_eq!(spec.transcript_path, exact);
        assert_eq!(spec.origin_cwd, "/tmp/origin");
        assert_eq!(
            spec.worktree_path.as_deref(),
            Some("/tmp/project/.lingxi/worktrees/feature")
        );
        assert_eq!(spec.options.worktree, None);
        assert_eq!(spec.options.tmux, None);
        assert_eq!(
            spec.terminal,
            TerminalSize {
                cols: 101,
                rows: 37
            }
        );
        assert_eq!(
            spec.env.get("ANTHROPIC_API_KEY").map(String::as_str),
            Some("secret")
        );

        let mut roster = daemon_roster::empty_roster(7);
        roster.workers.insert(
            short.to_string(),
            daemon_roster::WorkerRecord {
                pid: 41,
                proc_start: Some("start".to_string()),
                session_id: session_id.to_string(),
                rendezvous_sock: "live-endpoint".to_string(),
                pty_sock: Some("live-endpoint".to_string()),
                messaging_sock: None,
                cli_version: Some("0.11.0".to_string()),
                started_at: 77,
                attempt: 0,
                cwd: "/tmp/project".to_string(),
                worktree_path: None,
                dispatch,
                pending_respawn: None,
                dec_modes: None,
                rv_auth: Some("live-token".to_string()),
                pty_auth: Some("live-token".to_string()),
                extra: serde_json::Map::new(),
            },
        );
        daemon_roster::write_roster(&home, &roster).unwrap();
        let before = daemon_roster::read_roster(&home, 0, false).into_roster();
        let migrated = load_or_migrate_launch_spec(&home, &home, short)
            .unwrap()
            .expect("legacy spec migrated");
        assert!(!migrated.preflight_approved);
        let after = daemon_roster::read_roster(&home, 0, false).into_roster();
        assert_eq!(
            after, before,
            "worker-side migration must not rewrite roster"
        );
    }
    #[tokio::test]
    async fn shell_handoff_acknowledgements_are_phase_scoped_and_abort_wakes_waiter() {
        let home = tempfile::tempdir().unwrap();
        let short = "cafe1234";
        let ready = ShellHandoffAck {
            source: None,
            task_ids: vec!["b12345678".into()],
            error: None,
        };
        write_shell_handoff_ack(home.path(), short, "ready", &ready).unwrap();
        assert_eq!(
            wait_shell_handoff_ack(home.path(), short, "ready")
                .await
                .unwrap(),
            ready
        );
        assert!(read_shell_handoff_ack(home.path(), short, "commit")
            .unwrap()
            .is_none());
        write_shell_handoff_ack(
            home.path(),
            short,
            "abort",
            &ShellHandoffAck {
                source: None,
                task_ids: Vec::new(),
                error: Some("source kept ownership".into()),
            },
        )
        .unwrap();
        let error = wait_shell_handoff_ack(home.path(), short, "commit")
            .await
            .unwrap_err();
        assert_eq!(error.kind(), ErrorKind::Interrupted);
        assert!(write_shell_handoff_ack(home.path(), "../escape", "ready", &ready).is_err());
        assert!(write_shell_handoff_ack(home.path(), short, "../escape", &ready).is_err());
    }
}
