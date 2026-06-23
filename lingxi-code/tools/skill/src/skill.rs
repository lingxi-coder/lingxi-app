//! `SkillTool` — resolves a slash-command skill via injected `SkillLoader`.
//!
//! Wire identifiers locked in spec §7:
//! - Tool name: `Skill`.
//! - Descriptor cap: 1024 chars.
//! - Telemetry: `SKILL_STARTED` / `SKILL_COMPLETED` / `SKILL_FAILED`.
//!
//! Contract (parity batch MISC.10, TS `tools/SkillTool/SkillTool.ts`):
//! - Input `{skill, args?}` (TS `:291-298`). `skill` is the slash-command name;
//!   `args` is accepted and echoed but **not** expanded into a prompt.
//! - `validateInput`/`call` trim `skill`, strip a single leading `/`
//!   (TS `:366-372`), then resolve by normalized name.
//! - The three rejection strings are byte-faithful with TS `:406`, `:412-416`,
//!   `:421-427`:
//!   - `Unknown skill: <name>`
//!   - `Skill <name> cannot be used with Skill tool due to disable-model-invocation`
//!   - `Skill <name> is not a prompt-based skill`
//! - Output (inline path) mirrors the TS inline output union (TS `:301-326`):
//!   `{success:true, commandName, allowedTools?, model?, status:"inline"}`.
//!
//! ## Biggest non-faithful surface
//! The entire **forked-agent execution** path (TS `executeForkedSkill` →
//! `runAgent`, `prepareForkedCommandContext`, progress streaming,
//! `createAgentId`), MCP-skill discovery (`getAllCommands` merging
//! `mcp.commands`), remote canonical skills (`EXPERIMENTAL_SKILL_SEARCH`), and
//! frontmatter parsing have **no Rust substrate**. This tool performs *inline
//! resolution + metadata surfacing only*: it loads a descriptor, enforces the
//! locked rejections, and echoes the skill's `model`/`allowedTools`. `args` is
//! accepted but never expanded into a prompt. The descriptor `body` rides along
//! as an extra field (Rust's pragmatic substitute for actually forking).
//!
//! Hermetic by default: the `EmptySkillLoader` always returns "skill not
//! found" (→ `Unknown skill:`). Production hosts inject a loader backed by the
//! real command/skill registry.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use once_cell::sync::Lazy;
use permission::result::PermissionMetadata;
use permission::{PermissionDecisionReason, PermissionResult};
use serde_json::{json, Value};
use telemetry::pii::{PiiTagged, Verified};
use telemetry::sink::{AnalyticsValue, LogEventMetadata};
use telemetry::tengu::tool::{SKILL_COMPLETED, SKILL_FAILED, SKILL_INVOKED, SKILL_STARTED};
use telemetry::AnalyticsBus;

use tool_api::context::ToolUseContext;
use tool_api::progress::ToolProgressSender;
use tool_api::tool_trait::{
    DescriptionOptions, InterruptBehavior, PromptOptions, Tool, ToolCallResult, ToolError,
    ToolStaticContext, ValidationError,
};

/// Tool name byte-lock.
pub const SKILL_TOOL_NAME: &str = "Skill";
/// Descriptor (description) char cap (spec §7).
pub const MAX_SKILL_DESCRIPTOR_LEN: usize = 1024;
/// Maximum skill name length (defensive cap; matches team name lock).
pub const MAX_SKILL_NAME_LEN: usize = 128;

/// Command kind for a resolved skill. Mirrors the TS `Command.type` discriminant
/// — only `prompt`-typed commands may be invoked via the Skill tool
/// (TS `:421-427`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkillCommandType {
    /// A prompt-based slash command (the only model-invocable kind).
    Prompt,
    /// Any other command kind (local/jsx/etc.) — rejected with the locked
    /// "is not a prompt-based skill" error.
    Other,
}

/// Skill descriptor returned by a [`SkillLoader`]. Subset of the TS
/// `PromptCommand` shape — the fields the Skill tool actually surfaces.
#[derive(Debug, Clone)]
pub struct SkillDescriptor {
    /// Canonical name (matches the normalized input field).
    pub name: String,
    /// Short description (capped at [`MAX_SKILL_DESCRIPTOR_LEN`]).
    pub description: String,
    /// Body content (markdown sans frontmatter). Rides along as an extra
    /// output field — Rust's stand-in for forked execution.
    pub body: String,
    /// Whether model invocation is disabled (TS `disableModelInvocation`).
    /// `true` → rejected with the locked "disable-model-invocation" error.
    pub disable_model_invocation: bool,
    /// The command kind (TS `Command.type`). Only [`SkillCommandType::Prompt`]
    /// is model-invocable.
    pub command_type: SkillCommandType,
    /// Optional model override surfaced in the result (TS `command.model`).
    pub model: Option<String>,
    /// Tools this skill allows, surfaced in the result (TS `allowedTools`).
    pub allowed_tools: Vec<String>,
    /// Declared positional argument names (markdown frontmatter `arguments`).
    /// Threaded into [`command_api::substitute_arguments_faithful`] so a named
    /// placeholder like `$ticket` in the skill body resolves to the matching
    /// positional argument. Empty → only `$ARGUMENTS` / `$N` placeholders expand
    /// (TS `processPromptSlashCommand` passes the command's `argNames`).
    pub argument_names: Vec<String>,
    /// Shell to route embedded `!command` expansion through (the markdown
    /// frontmatter `shell` selector). `None` -> bash (the TS default). Forwarded
    /// to [`command_api::execute_shell_commands_in_prompt`] as the `shell` arg —
    /// 1:1 with TS `getPromptForCommand(..., shell)` (`loadSkillsDir.ts:394`).
    pub shell: Option<command_api::FrontmatterShell>,
    /// Skip embedded `!command` shell expansion for this skill. Mirrors the TS
    /// `loadedFrom !== 'mcp'` gate (`loadSkillsDir.ts:374`): MCP skills are
    /// remote/untrusted, so their markdown body is NEVER shell-expanded. `false`
    /// (the default) -> expansion runs (the on-disk / plugin markdown case).
    pub skip_shell_expansion: bool,
    /// The skill's own base directory (TS `baseDir`). `Some(dir)` for file-based
    /// skills (the SKILL.md / command markdown's parent directory); `None` for
    /// non-file skills (e.g. MCP-sourced, which have no `baseDir`). When `Some`,
    /// `${CLAUDE_SKILL_DIR}` in the body is replaced with this path so embedded
    /// `!command` blocks can reference bundled scripts
    /// (`loadSkillsDir.ts:359-363`).
    pub skill_root: Option<std::path::PathBuf>,
    /// The current session id, surfaced to substitute `${CLAUDE_SESSION_ID}` in
    /// the body (TS `getSessionId()`, `loadSkillsDir.ts:366-369`). `None` for
    /// hermetic/loaderless construction (the token is then left as-is); the
    /// production loader stamps the engine's per-session id here. Carried on the
    /// descriptor rather than the frozen `BuiltinToolContext` (whose mobile
    /// construction site cannot be extended).
    pub session_id: Option<String>,
}

impl Default for SkillDescriptor {
    fn default() -> Self {
        Self {
            name: String::new(),
            description: String::new(),
            body: String::new(),
            disable_model_invocation: false,
            command_type: SkillCommandType::Prompt,
            model: None,
            allowed_tools: Vec::new(),
            argument_names: Vec::new(),
            shell: None,
            skip_shell_expansion: false,
            skill_root: None,
            session_id: None,
        }
    }
}

/// Loader trait — production wraps the real command/skill registry; tests inject
/// a fixed descriptor.
#[async_trait]
pub trait SkillLoader: Send + Sync {
    /// Load a skill by its normalized name (leading slash already stripped).
    /// Returns `None` if no such skill is registered → `Unknown skill:`.
    async fn load(&self, name: &str) -> Result<Option<SkillDescriptor>, ToolError>;
}

/// Default hermetic loader — always reports "not found" (→ `Unknown skill:`).
pub struct EmptySkillLoader;

#[async_trait]
impl SkillLoader for EmptySkillLoader {
    async fn load(&self, _name: &str) -> Result<Option<SkillDescriptor>, ToolError> {
        Ok(None)
    }
}

/// Embedded-`!command` shell runner adapter for skill bodies.
///
/// SKILLEXEC.6: the TS `executeShellCommandsInPrompt`
/// (`utils/promptShellExecution.ts:115`) routes each embedded command through
/// `BashTool.call({ command }, context)`. The Rust [`command_api::ShellRunner`]
/// seam is injected here, wrapping the SAME `ProcessRunner` + `Sandbox` seams the
/// Rust `BashTool` spawns through (`tools/shell/src/bash.rs`): we build the
/// foreground `ProcessCommand` like bash's foreground path — resolve the login
/// shell, prepend the BASH.1 extglob-disable guard, run via `-c -l` — then
/// construct the `SandboxedCommand` through `Sandbox::bypass_with_audit` and call
/// `ProcessRunner::run`.
///
/// Sandbox parity: before finalizing, this runner performs the SAME
/// `should_use_sandbox` + `wrap_with_sandbox` decision as `BashTool::call`
/// (`bash.rs`). The sandbox-decision inputs (`sandbox_available`, `workspace`,
/// `sandbox_runtime`, `platform`) are captured from the `SkillTool`'s
/// [`BuiltinToolContext`] at construction (the `command_api::ShellRunner::run`
/// signature stays `(&self, command, shell)` — the inputs ride on the adapter,
/// not the call). A skill `!command` has no per-command `dangerouslyDisableSandbox`
/// flag (it is a Bash-tool *input* field; skill bodies have no such surface), so
/// we pass `false` for that override — 1:1 with claude-code `shouldUseSandbox.ts`,
/// which has NO permission-mode, project-trust, or classifier inputs (only host
/// availability, the `dangerouslyDisableSandbox` override, and the
/// `excludedCommands` config). The `Sandbox::bypass_with_audit` envelope still finalizes
/// the (possibly wrapped) command string for `ProcessRunner::run`, matching the
/// constructor bash uses at its final foreground spawn — the sandboxing is baked
/// into the wrapped command STRING, not the envelope.
///
/// Divergence (documented): the adapter carries no `AnalyticsBus`, so the
/// `sandbox_refused` / `sandbox_wrap_failed` telemetry events bash emits on the
/// refuse/wrap-failure branches are skipped here; the failure is still surfaced
/// as a `command_api::ShellRunError` (the TS `errorMessage(e)` generic path) so
/// the engine formats `[Error]\n…` and the command does NOT run. The Windows-CMD
/// `2>nul` rewrite is omitted (bash refuses on Windows outright); the BASH.4
/// persistent-cwd `pwd -P` readback is omitted (one-shot expansion keeps no
/// shell-cwd state).
struct SkillShellRunner {
    process: Arc<dyn traits::process::ProcessRunner>,
    sandbox: Arc<dyn traits::sandbox::Sandbox>,
    workspace: std::path::PathBuf,
    // ===== Sandbox-decision inputs, captured from the SkillTool's
    // `BuiltinToolContext` (mirrors what `BashTool::call` reads off `self.ctx`).
    // 1:1 with claude-code `shouldUseSandbox.ts`, which has NO permission-mode,
    // project-trust, or classifier inputs — only host availability, the
    // `dangerouslyDisableSandbox` override, and the `excludedCommands` config.
    /// Whether the host has a working sandbox backend (`bash.rs`).
    sandbox_available: bool,
    /// Sandbox policy runtime config — supplies `excludedCommands` to the
    /// decision and drives the wrap (`bash.rs`).
    sandbox_runtime: sandbox::runtime_config::SandboxRuntimeConfig,
    /// Detected platform — selects the wrap branch (`bash.rs:533`).
    platform: sandbox::runtime_config::Platform,
    /// Injected async sandbox seam, threaded from the `SkillTool`'s
    /// `BuiltinToolContext` so embedded `!command` expansion wraps through the
    /// same runner the Bash tool uses (default `LegacyWrapRunner` =
    /// byte-identical to the previous direct `wrap_with_sandbox` call).
    sandbox_runner: Arc<dyn tool_api::SandboxRunner>,
}

/// Resolve the login shell exactly like `bash.rs::resolve_shell_path`
/// (`/bin/zsh` on macOS, `/bin/bash` elsewhere).
fn resolve_skill_shell_path() -> &'static str {
    if cfg!(target_os = "macos") {
        "/bin/zsh"
    } else {
        "/bin/bash"
    }
}

/// BASH.1 extglob-disable guard, 1:1 with `bash.rs::disable_extglob_command`.
fn skill_disable_extglob(shell_path: &str) -> Option<String> {
    if std::env::var("CLAUDE_CODE_SHELL_PREFIX").is_ok_and(|v| !v.is_empty()) {
        return Some(
            "{ shopt -u extglob || setopt NO_EXTENDED_GLOB; } >/dev/null 2>&1 || true".into(),
        );
    }
    if shell_path.contains("bash") {
        Some("shopt -u extglob 2>/dev/null || true".into())
    } else if shell_path.contains("zsh") {
        Some("setopt NO_EXTENDED_GLOB 2>/dev/null || true".into())
    } else {
        None
    }
}

#[async_trait]
impl command_api::ShellRunner for SkillShellRunner {
    async fn run(
        &self,
        command: &str,
        _shell: Option<command_api::FrontmatterShell>,
    ) -> Result<command_api::ShellOut, command_api::ShellRunError> {
        use sandbox::decision::{should_use_sandbox, SandboxDecision};
        use traits::sandbox::ProcessCommand;

        let shell_path = resolve_skill_shell_path();
        // BASH.1: prepend the extglob-disable guard INTO the command so it runs
        // in the same shell that expands the user's globs (mirrors the TS order
        // `disableExtglob && <cmd>`).
        let spawn_cmd = match skill_disable_extglob(shell_path) {
            Some(prefix) => format!("{prefix} && {command}"),
            None => command.to_string(),
        };

        // ===== Sandbox decision (mirror of `BashTool::call`) =====
        // A skill `!command` has NO per-command `dangerouslyDisableSandbox` flag
        // (that is a Bash-tool *input* field; skill bodies have no such surface),
        // so we pass `false` for that override and otherwise feed the same inputs
        // bash does — 1:1 with claude-code `shouldUseSandbox.ts`. `unsandboxed_allowed`
        // is the canonical `are_unsandboxed_commands_allowed()` accessor.
        let decision = should_use_sandbox(
            command,
            self.sandbox_available,
            /* dangerously_disable_sandbox */ false,
            self.sandbox_runtime.are_unsandboxed_commands_allowed(),
            &self.sandbox_runtime,
            self.workspace.clone(),
        );
        let inner = match decision {
            // No sandbox: run the (extglob-guarded) command unchanged.
            SandboxDecision::NoSandbox => spawn_cmd,
            // Wrap the command string for the sandbox; on failure surface a
            // `ShellRunError` (the TS `errorMessage(e)` generic path) so the
            // command does NOT run. Bash splits this into two arms purely to emit
            // distinct telemetry (`Unsupported` -> `sandbox_refused`,
            // `SbplWrite` -> `sandbox_wrap_failed`) and to pick `InvalidInput` vs
            // `Io`; this adapter has no `AnalyticsBus` and a single `ShellRunError`
            // surface, so both `SandboxWrapError` variants collapse to the same
            // generic failure (the inner string is preserved verbatim — the same
            // string bash surfaces). See the struct doc.
            SandboxDecision::Sandbox { policy: _ } => {
                // Wrap through the injected async `SandboxRunner` (same seam the
                // Bash tool uses). The default `LegacyWrapRunner` forwards to the
                // sync `wrap_with_sandbox` (ignoring `bin_shell`/`cwd`), so this
                // is byte-identical to the previous direct call.
                match self
                    .sandbox_runner
                    .wrap(
                        &spawn_cmd,
                        &self.sandbox_runtime,
                        self.platform,
                        Some(shell_path),
                        Some(self.workspace.as_path()),
                    )
                    .await
                {
                    Ok(wrapped) => wrapped,
                    Err(
                        sandbox::wrap::SandboxWrapError::Unsupported(s)
                        | sandbox::wrap::SandboxWrapError::SbplWrite(s),
                    ) => {
                        return Err(command_api::ShellRunError {
                            stdout: String::new(),
                            stderr: String::new(),
                            interrupted: false,
                            generic_message: Some(s),
                        });
                    }
                }
            }
        };

        let pcmd = ProcessCommand {
            command: shell_path.to_string(),
            // BASH.4: login-shell init (`-l` after `-c`), matching the bash
            // foreground spawn (`bashProvider.ts:201-205`, snapshot path deferred).
            args: vec!["-c".into(), "-l".into(), inner],
            cwd: Some(self.workspace.clone()),
            env: HashMap::new(),
            timeout: None,
            stdin: None,
        };
        let sandboxed = self
            .sandbox
            .bypass_with_audit(pcmd, "skill_shell_expansion");
        let run_result = self.process.run(&sandboxed).await;
        // The wrapped command has finished: tear down any per-command sandbox
        // state. No-op for the default `LegacyWrapRunner`.
        self.sandbox_runner.cleanup_after_command().await;
        match run_result {
            // A timeout maps to the TS interrupted `ShellError` path so the engine
            // formats "Shell command interrupted …" (mirrors bash's timeout->error).
            Ok(out) if out.timed_out => Err(command_api::ShellRunError {
                stdout: out.stdout,
                stderr: out.stderr,
                interrupted: true,
                generic_message: None,
            }),
            // Non-zero exit is NOT an error here — TS `BashTool.call` returns
            // stdout/stderr without throwing on a non-zero status; only an
            // interruption throws. So every completed run yields `ShellOut`.
            Ok(out) => Ok(command_api::ShellOut {
                stdout: out.stdout,
                stderr: out.stderr,
                interrupted: false,
            }),
            // Spawn / I/O failure -> the TS `errorMessage(e)` generic path
            // (`[Error]\n{message}`).
            Err(e) => Err(command_api::ShellRunError {
                stdout: String::new(),
                stderr: String::new(),
                interrupted: false,
                generic_message: Some(format!("{e}")),
            }),
        }
    }
}

/// Per-command permission gate for embedded skill `!command`s.
///
/// SKILLEXEC.6: TS calls `hasPermissionsToUseTool(shellTool, { command }, …)`
/// before each command. The Rust `BashTool::check_permissions` is an allow-all
/// stub (`bash.rs:381-389`), so the parity-faithful gate returns `Allow`,
/// matching the current Bash bar. (TS merges the skill's `allowedTools` into the
/// permission context's `alwaysAllowRules.command`; against an allow-all bar that
/// merge is a no-op, so it is intentionally not replicated here.)
struct SkillShellPermissionGate;

impl command_api::ShellPermissionGate for SkillShellPermissionGate {
    fn check(
        &self,
        _command: &str,
        _shell: Option<command_api::FrontmatterShell>,
    ) -> command_api::ShellPermissionDecision {
        command_api::ShellPermissionDecision::Allow
    }
}

/// `SkillTool` — resolves + validates a slash-command skill.
pub struct SkillTool {
    pub(crate) ctx: tool_api::BuiltinToolContext,
    pub(crate) loader: Arc<dyn SkillLoader>,
}

impl SkillTool {
    /// Construct with the default `EmptySkillLoader`.
    #[must_use]
    pub fn new(ctx: tool_api::BuiltinToolContext) -> Self {
        Self {
            ctx,
            loader: Arc::new(EmptySkillLoader),
        }
    }

    /// Construct with a caller-supplied loader.
    #[must_use]
    pub fn with_loader(ctx: tool_api::BuiltinToolContext, loader: Arc<dyn SkillLoader>) -> Self {
        Self { ctx, loader }
    }
}

static SCHEMA: Lazy<Value> = Lazy::new(|| {
    json!({
        "type": "object",
        "properties": {
            "skill": {
                "type": "string",
                "description": "The name of a skill from the available-skills list. Do not guess names."
            },
            "args": {
                "type": "string",
                "description": "Optional arguments for the skill"
            }
        },
        "required": ["skill"]
    })
});

fn pii_str(s: &str) -> AnalyticsValue {
    AnalyticsValue::String(PiiTagged::assert_pii_tagged_column(s.to_string()).into_inner())
}

fn verified_str(s: &str) -> AnalyticsValue {
    AnalyticsValue::String(Verified::assert_safe(s.to_string()).into_inner())
}

async fn emit_failed(bus: &Arc<AnalyticsBus>, kind: &str, duration_ms: u64) {
    let mut md: LogEventMetadata = HashMap::new();
    md.insert("error_kind".into(), verified_str(kind));
    md.insert(
        "duration_ms".into(),
        AnalyticsValue::Int(duration_ms as i64),
    );
    bus.log_event(SKILL_FAILED, md).await;
}

/// Trim `skill` and strip a single leading `/` (TS `:356`, `:366-372`).
/// Returns the normalized command name (may be empty if input was blank).
fn normalize_skill_name(skill: &str) -> String {
    let trimmed = skill.trim();
    trimmed.strip_prefix('/').unwrap_or(trimmed).to_string()
}

/// Sanitize a normalized skill name for the `SKILL_INVOKED` telemetry
/// `skill_name` dimension (a `Verified`/whitelisted, non-PII field). Builtin
/// command names pass through; any non-builtin collapses to `"custom"` — TS
/// `command_name = NOT_CODE_OR_FILEPATHS` over `builtInCommandNames`
/// (`commands.ts:350-353`). The descriptor carries no bundled/official-source
/// flag, so bundled/official skills are indistinguishable here and all
/// non-builtins map to `"custom"`.
fn skill_name_dimension(command_name: &str) -> String {
    if command_api::builtin_support::names::BUILTIN_COMMAND_NAMES.contains(&command_name) {
        command_name.to_string()
    } else {
        "custom".to_string()
    }
}

#[async_trait]
impl Tool for SkillTool {
    fn name(&self) -> &str {
        SKILL_TOOL_NAME
    }
    fn input_schema(&self) -> &Value {
        &SCHEMA
    }
    fn is_enabled(&self, _: &ToolStaticContext) -> bool {
        true
    }
    fn max_result_size_chars(&self) -> usize {
        tool_api::util::output_truncation::MAX_TOOL_OUTPUT_LENGTH
    }
    fn is_concurrency_safe(&self, _: &Value) -> bool {
        true
    }
    fn is_read_only(&self, _: &Value) -> bool {
        true
    }
    fn is_destructive(&self, _: &Value) -> bool {
        false
    }
    fn is_open_world(&self, _: &Value) -> bool {
        false
    }
    fn interrupt_behavior(&self, _: &Value) -> InterruptBehavior {
        InterruptBehavior::Block
    }

    async fn check_permissions(&self, _: &Value, _: &ToolUseContext) -> PermissionResult {
        PermissionResult::Allow {
            reason: PermissionDecisionReason::Other {
                reason: "Skill loads a registered skill descriptor (read-only)".into(),
            },
            updated_input: None,
            update_destination: None,
            metadata: PermissionMetadata::default(),
        }
    }

    async fn description(&self, input: &Value, _: &DescriptionOptions) -> String {
        // TS `:342`: `Execute skill: ${skill}`.
        match input.get("skill").and_then(Value::as_str) {
            Some(s) => format!("Execute skill: {s}"),
            None => "Execute a slash-command skill.".into(),
        }
    }

    async fn prompt(&self, _: &PromptOptions) -> String {
        "Skill: invoke a slash-command skill by name.".into()
    }

    async fn validate_input(
        &self,
        input: &Value,
        ctx: &ToolUseContext,
    ) -> Result<(), ValidationError> {
        let skill = input
            .get("skill")
            .and_then(Value::as_str)
            .ok_or_else(|| ValidationError("Skill: missing or non-string skill".into()))?;

        // Trim; reject blank (TS `:356-363` → "Invalid skill format").
        let trimmed = skill.trim();
        if trimmed.is_empty() {
            return Err(ValidationError(format!("Invalid skill format: {skill}")));
        }

        // Strip a single leading slash (TS `:366-372`).
        let normalized = normalize_skill_name(skill);
        if normalized.chars().count() > MAX_SKILL_NAME_LEN {
            return Err(ValidationError(format!(
                "Skill: name length {} exceeds max {}",
                normalized.chars().count(),
                MAX_SKILL_NAME_LEN
            )));
        }

        // Resolve + apply the three locked rejections (TS `:399-427`).
        match self.loader.load(&normalized).await {
            Ok(Some(desc)) => {
                if desc.disable_model_invocation {
                    return Err(ValidationError(format!(
                        "Skill {normalized} cannot be used with {SKILL_TOOL_NAME} tool due to disable-model-invocation"
                    )));
                }
                if desc.command_type != SkillCommandType::Prompt {
                    return Err(ValidationError(format!(
                        "Skill {normalized} is not a prompt-based skill"
                    )));
                }
                Ok(())
            }
            Ok(None) => Err(ValidationError(format!("Unknown skill: {normalized}"))),
            // Loader I/O failure — surface verbatim; not one of the locked
            // contract strings.
            Err(e) => {
                let _ = ctx;
                Err(ValidationError(format!("{e}")))
            }
        }
    }

    async fn call(
        &self,
        input: Value,
        _ctx: ToolUseContext,
        _progress: ToolProgressSender,
    ) -> Result<ToolCallResult, ToolError> {
        let started = Instant::now();
        let bus = self.ctx.bus.clone();

        let skill = match input.get("skill").and_then(Value::as_str) {
            Some(s) => s.to_string(),
            None => {
                emit_failed(&bus, "missing_skill", started.elapsed().as_millis() as u64).await;
                return Err(ToolError::InvalidInput(
                    "Skill: missing or non-string skill".into(),
                ));
            }
        };

        // `args` is accepted and echoed, never expanded into a prompt
        // (substrate gap — see module doc).
        let args = input
            .get("args")
            .and_then(Value::as_str)
            .map(str::to_string);

        // Trim; reject blank (TS `:356-363`).
        if skill.trim().is_empty() {
            emit_failed(&bus, "empty_skill", started.elapsed().as_millis() as u64).await;
            return Err(ToolError::InvalidInput(format!(
                "Invalid skill format: {skill}"
            )));
        }

        // Strip a single leading slash (TS `:597-598`).
        let command_name = normalize_skill_name(&skill);

        let mut md: LogEventMetadata = HashMap::new();
        md.insert("_PROTO_skill_name".into(), pii_str(&command_name));
        bus.log_event(SKILL_STARTED, md).await;

        let loaded = match self.loader.load(&command_name).await {
            Ok(o) => o,
            Err(e) => {
                emit_failed(&bus, "loader_error", started.elapsed().as_millis() as u64).await;
                return Err(e);
            }
        };
        let mut desc = match loaded {
            Some(d) => d,
            None => {
                emit_failed(&bus, "unknown_skill", started.elapsed().as_millis() as u64).await;
                // Locked string (TS `:406`).
                return Err(ToolError::InvalidInput(format!(
                    "Unknown skill: {command_name}"
                )));
            }
        };

        // Locked rejection: disable-model-invocation (TS `:412-416`).
        if desc.disable_model_invocation {
            emit_failed(
                &bus,
                "disable_model_invocation",
                started.elapsed().as_millis() as u64,
            )
            .await;
            return Err(ToolError::InvalidInput(format!(
                "Skill {command_name} cannot be used with {SKILL_TOOL_NAME} tool due to disable-model-invocation"
            )));
        }

        // Locked rejection: non-prompt skill (TS `:421-427`).
        if desc.command_type != SkillCommandType::Prompt {
            emit_failed(&bus, "not_prompt", started.elapsed().as_millis() as u64).await;
            return Err(ToolError::InvalidInput(format!(
                "Skill {command_name} is not a prompt-based skill"
            )));
        }

        // SKILLEXEC.5: fire the registered `SKILL_INVOKED` event on the success
        // path — after validateInput passes (descriptor loaded, prompt-type, not
        // disable-model-invocation) — mirroring TS `SkillTool.ts:654-709`. The
        // `skill_name` dimension is a `Verified` (whitelisted, non-PII) field, so
        // it is sanitized to the builtin command name, or `"custom"` for any
        // non-builtin skill (TS `command_name = NOT_CODE_OR_FILEPATHS` over
        // `builtInCommandNames`, `commands.ts:350-353`). The `SkillDescriptor`
        // carries no bundled/official-source flag, so every non-builtin collapses
        // to `"custom"` — faithful for the hermetic substrate.
        let skill_name_dim = skill_name_dimension(&command_name);
        let mut inv_md: LogEventMetadata = HashMap::new();
        inv_md.insert(
            "invocation_id".into(),
            verified_str(&tool_api::util::ids::ulid_or_uuid()),
        );
        inv_md.insert("skill_name".into(), verified_str(&skill_name_dim));
        bus.log_event(SKILL_INVOKED, inv_md).await;

        // Enforce descriptor cap byte-lock.
        let truncated = if desc.description.chars().count() > MAX_SKILL_DESCRIPTOR_LEN {
            let s: String = desc
                .description
                .chars()
                .take(MAX_SKILL_DESCRIPTOR_LEN)
                .collect();
            desc.description = s;
            true
        } else {
            false
        };

        // SKILLEXEC.3: expand the skill body into the prompt the model must
        // process. Faithful to TS `processPromptSlashCommand`
        // (SkillTool.ts:634-643 → `getPromptForCommand`): `$ARGUMENTS` / `$N` /
        // `$name` substitution over the markdown body with the raw args string
        // (`args || ''`), `appendIfNoPlaceholder = true`, and the command's
        // declared argument names. The result is returned as `new_messages` (a
        // user message) so the turn loop appends it after this tool_result and
        // the model acts on the skill (SkillTool.ts:735-774 `newMessages`).
        //
        // SKILLEXEC.6: the embedded `!command` shell-expansion step (TS
        // `getPromptForCommand` step 4) RUNS here, after argument substitution
        // and before the user message is built — see below. The TS tagging of the
        // message with the parent toolUseID (transient-until-resolved) has no Rust
        // `ConversationMessage` substrate and is scoped out: the expanded prompt
        // enters as a plain user message. The adjacent TS `${CLAUDE_SKILL_DIR}` /
        // `${CLAUDE_SESSION_ID}` token replacements (steps 2-3) run between
        // argument substitution and shell expansion — see just below.
        let args_for_expansion = args.as_deref().unwrap_or("");
        let expanded_prompt = match command_api::substitute_arguments_faithful(
            &desc.body,
            Some(args_for_expansion),
            true,
            &desc.argument_names,
        ) {
            Ok(p) => p,
            Err(e) => {
                emit_failed(
                    &bus,
                    "expansion_error",
                    started.elapsed().as_millis() as u64,
                )
                .await;
                return Err(ToolError::Internal(format!(
                    "Skill {command_name} expansion failed: {e}"
                )));
            }
        };

        // SKILLEXEC: `${CLAUDE_SKILL_DIR}` / `${CLAUDE_SESSION_ID}` token
        // replacement (TS `getPromptForCommand` steps 2-3,
        // `loadSkillsDir.ts:359-369`). Faithful ordering: AFTER argument
        // substitution, BEFORE the embedded `!command` shell expansion below, so
        // a `!command` that references `${CLAUDE_SKILL_DIR}` / `${CLAUDE_SESSION_ID}`
        // sees the substituted value. Both are plain literal-token replacements
        // (the tokens carry no regex metacharacters), so `str::replace` (global by
        // default) matches the TS global-regex `.replace(/.../g, …)` exactly.
        //
        // SAFETY (byte-identical): a body containing NEITHER token is returned
        // unchanged by both `replace` calls (no match → no allocation difference
        // in output), preserving the byte-for-byte invariant for the common case.
        let mut expanded_prompt = expanded_prompt;
        // Step 2: `${CLAUDE_SKILL_DIR}` — only for file-based skills (TS gates on
        // `baseDir`). On Windows, normalize backslashes to forward slashes BEFORE
        // the replace so embedded shell commands don't treat them as escapes
        // (`loadSkillsDir.ts:360-361`).
        if let Some(skill_root) = desc.skill_root.as_ref() {
            let skill_dir = skill_root.to_string_lossy();
            let skill_dir = if cfg!(windows) {
                skill_dir.replace('\\', "/")
            } else {
                skill_dir.into_owned()
            };
            expanded_prompt = expanded_prompt.replace("${CLAUDE_SKILL_DIR}", &skill_dir);
        }
        // Step 3: `${CLAUDE_SESSION_ID}` — always replaced (TS calls
        // `getSessionId()` unconditionally). `None` (hermetic/loaderless) leaves
        // the token untouched rather than substituting an empty string.
        if let Some(session_id) = desc.session_id.as_ref() {
            expanded_prompt = expanded_prompt.replace("${CLAUDE_SESSION_ID}", session_id);
        }

        // SKILLEXEC.6: embedded `!command` shell expansion over the substituted
        // body, faithful to TS `getPromptForCommand` step 4
        // (`loadSkillsDir.ts:374-395` -> `executeShellCommandsInPrompt`). Gated on
        // `!skip_shell_expansion` (the TS `loadedFrom !== 'mcp'` guard): MCP skills
        // are remote/untrusted and never shell-expand their body. The slash-command
        // name arg is `/${command_name}`; the shell arg is the descriptor's
        // frontmatter `shell` selector.
        //
        // SAFETY (byte-identical): when the substituted body contains NO `!command`
        // block, `execute_shell_commands_in_prompt` returns it UNCHANGED (block
        // scan finds nothing; the inline scan is gated behind a `"!`"` substring
        // fast-path), so the common case is byte-identical to today. MCP-sourced
        // skills (`skip_shell_expansion = true`) skip the call entirely.
        let mut expanded_prompt = if desc.skip_shell_expansion {
            expanded_prompt
        } else {
            let shell_ctx = command_api::ShellExpansionCtx {
                runner: Arc::new(SkillShellRunner {
                    process: self.ctx.process.clone(),
                    sandbox: self.ctx.sandbox.clone(),
                    workspace: self.ctx.workspace.clone(),
                    // Sandbox-decision inputs, captured at construction so the
                    // `ShellRunner::run` signature stays unchanged — mirror of
                    // the fields `BashTool::call` reads off `self.ctx`.
                    sandbox_available: self.ctx.sandbox_available,
                    sandbox_runtime: self.ctx.sandbox_runtime.clone(),
                    platform: self.ctx.platform,
                    // Thread the injected runner so the child wraps through the
                    // same seam (default `LegacyWrapRunner` = byte-identical).
                    sandbox_runner: self.ctx.sandbox_runner.clone(),
                }),
                permission_gate: Arc::new(SkillShellPermissionGate),
            };
            match command_api::execute_shell_commands_in_prompt(
                &expanded_prompt,
                &shell_ctx,
                &format!("/{command_name}"),
                desc.shell,
            )
            .await
            {
                Ok(p) => p,
                Err(e) => {
                    emit_failed(
                        &bus,
                        "shell_expansion_error",
                        started.elapsed().as_millis() as u64,
                    )
                    .await;
                    return Err(ToolError::Internal(format!(
                        "Skill {command_name} shell expansion failed: {e}"
                    )));
                }
            }
        };
        if let Some(skill_root) = desc.skill_root.as_ref() {
            let skill_dir = skill_root.to_string_lossy();
            let skill_dir = if cfg!(windows) {
                skill_dir.replace('\\', "/")
            } else {
                skill_dir.into_owned()
            };
            expanded_prompt =
                format!("Base directory for this skill: {skill_dir}\n\n{expanded_prompt}");
        }

        let new_messages = vec![protocol::ConversationMessage::user(
            protocol::MessageId::new(),
            expanded_prompt,
        )];

        let mut md: LogEventMetadata = HashMap::new();
        md.insert(
            "duration_ms".into(),
            AnalyticsValue::Int(started.elapsed().as_millis() as i64),
        );
        md.insert(
            "descriptor_len".into(),
            AnalyticsValue::Int(desc.description.chars().count() as i64),
        );
        md.insert(
            "descriptor_truncated".into(),
            AnalyticsValue::Bool(truncated),
        );
        bus.log_event(SKILL_COMPLETED, md).await;

        // Inline output union (TS `:301-326`). `allowedTools` and `model` are
        // optional — omitted when empty/absent — matching the TS `.optional()`
        // surfacing. `commandName` is the normalized name. `body`/`args`/
        // `descriptor_truncated` ride along as extra fields (Rust substitute
        // for actually forking).
        //
        // `model_content` is the model-facing tool_result string. TS
        // `mapToolResultToToolResultBlockParam` (SkillTool.ts:843-862) makes the
        // model see ONLY a short line, never a JSON dump of the output object
        // (which would leak the full skill `body`). The orchestrator's
        // `tool_result_to_model_text` (turn_loop.rs) prefers `model_content`.
        // This tool only produces the inline path (TS `status:'inline'` default
        // → `Launching skill: ${commandName}`); the forked branch has no Rust
        // substrate (see module doc), so only the inline string is emitted.
        let mut data = json!({
            "success": true,
            "commandName": command_name,
            "status": "inline",
            "body": desc.body,
            "descriptor_truncated": truncated,
            "model_content": format!("Launching skill: {command_name}"),
        });
        // Capture the model override BEFORE `desc.model` is moved into the
        // result JSON below — it feeds the SKILLEXEC.3 `context_modifier`.
        let model_override = desc.model.clone();

        let obj = data.as_object_mut().expect("json object");
        if !desc.allowed_tools.is_empty() {
            obj.insert(
                "allowedTools".into(),
                Value::Array(desc.allowed_tools.into_iter().map(Value::String).collect()),
            );
        }
        if let Some(model) = desc.model {
            obj.insert("model".into(), Value::String(model));
        }
        if let Some(args) = args {
            obj.insert("args".into(), Value::String(args));
        }

        // SKILLEXEC.3 (model scope): when the skill declares `model:` in its
        // frontmatter, return a `context_modifier` that switches the session's
        // main-loop model for the rest of the session — 1:1 with TS
        // `SkillTool.ts:808-821` (`contextModifier` sets `options.mainLoopModel
        // = resolveSkillModelOverride(model, ctx.options.mainLoopModel)`),
        // including the `[1m]`-suffix preservation rule. The turn loop seeds the
        // closure's `ctx` with the live `session.model` (the `currentModel`
        // argument) and folds it POST-BATCH. When `model` is absent the modifier
        // stays `None`, so the no-override path is byte-identical.
        let context_modifier: Option<tool_api::ContextModifier> =
            model_override.map(|model| -> tool_api::ContextModifier {
                Box::new(move |mut ctx: ToolUseContext| {
                    let current = ctx.options.main_loop_model.clone();
                    ctx.options.main_loop_model =
                        crate::model_override::resolve_skill_model_override(&model, &current);
                    ctx
                })
            });

        Ok(ToolCallResult {
            data,
            new_messages,
            context_modifier,
            mcp_meta: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tool_api::test_support::{fresh_ctx, fresh_tx, shell_test_ctx};
    use traits::process::ProcessOutput;

    fn dummy_out() -> ProcessOutput {
        ProcessOutput {
            stdout: String::new(),
            stderr: String::new(),
            exit_code: 0,
            timed_out: false,
        }
    }

    /// A loader that returns a fixed descriptor for any name.
    struct FixedLoader(Option<SkillDescriptor>);
    #[async_trait]
    impl SkillLoader for FixedLoader {
        async fn load(&self, _name: &str) -> Result<Option<SkillDescriptor>, ToolError> {
            Ok(self.0.clone())
        }
    }

    /// A loader that captures the (normalized) name it was asked to load.
    struct CapturingLoader {
        seen: std::sync::Mutex<Option<String>>,
        desc: Option<SkillDescriptor>,
    }
    #[async_trait]
    impl SkillLoader for CapturingLoader {
        async fn load(&self, name: &str) -> Result<Option<SkillDescriptor>, ToolError> {
            *self.seen.lock().unwrap() = Some(name.to_string());
            Ok(self.desc.clone())
        }
    }

    fn prompt_desc(name: &str) -> SkillDescriptor {
        SkillDescriptor {
            name: name.into(),
            description: "a skill".into(),
            body: "body here".into(),
            ..SkillDescriptor::default()
        }
    }

    #[test]
    fn constants_locked() {
        assert_eq!(SKILL_TOOL_NAME, "Skill");
        assert_eq!(MAX_SKILL_DESCRIPTOR_LEN, 1024);
    }

    #[test]
    fn normalize_strips_single_leading_slash_and_trims() {
        assert_eq!(normalize_skill_name("/commit"), "commit");
        assert_eq!(normalize_skill_name("  /commit  "), "commit");
        assert_eq!(normalize_skill_name("commit"), "commit");
        // Only a single leading slash is stripped.
        assert_eq!(normalize_skill_name("//commit"), "/commit");
    }

    #[test]
    fn skill_name_dimension_keeps_builtin_and_sanitizes_custom() {
        // A known builtin command name passes through unchanged...
        let a_builtin = command_api::builtin_support::names::BUILTIN_COMMAND_NAMES[0];
        assert_eq!(skill_name_dimension(a_builtin), a_builtin);
        // ...while any non-builtin (custom) skill collapses to "custom" — the
        // Verified/whitelisted SKILL_INVOKED dimension never leaks a raw PII name.
        assert_eq!(
            skill_name_dimension("definitely-not-a-builtin-skill-xyz"),
            "custom"
        );
    }

    #[test]
    fn schema_uses_skill_and_optional_args() {
        let props = &SCHEMA["properties"];
        assert_eq!(props["skill"]["type"], json!("string"));
        // TS `inputSchema` is `skill: z.string().describe(...)` with NO `.min(1)`
        // (SkillTool.ts:291-298) — the delivered schema must NOT carry a
        // `minLength` constraint. The runtime "Invalid skill format" check
        // (validate_input/call) is what enforces non-blankness, not the schema.
        assert!(
            props["skill"].get("minLength").is_none(),
            "skill schema must not constrain minLength (TS has no .min(1))"
        );
        assert_eq!(props["args"]["type"], json!("string"));
        assert_eq!(SCHEMA["required"], json!(["skill"]));
        // No legacy `name` field remains.
        assert!(props.get("name").is_none());
    }

    #[tokio::test]
    async fn slash_prefixed_skill_is_normalized_before_lookup() {
        let loader = Arc::new(CapturingLoader {
            seen: std::sync::Mutex::new(None),
            desc: Some(prompt_desc("commit")),
        });
        let tool = SkillTool::with_loader(shell_test_ctx(dummy_out()), loader.clone());
        let out = tool
            .call(json!({"skill": "/commit"}), fresh_ctx(), fresh_tx())
            .await
            .expect("ok");
        // Loader saw the slash-stripped name.
        assert_eq!(loader.seen.lock().unwrap().as_deref(), Some("commit"));
        // commandName is the normalized name + inline status.
        assert_eq!(out.data["commandName"], json!("commit"));
        assert_eq!(out.data["status"], json!("inline"));
        assert_eq!(out.data["success"], json!(true));
    }

    #[tokio::test]
    async fn unknown_skill_locked_error() {
        let tool = SkillTool::new(shell_test_ctx(dummy_out()));
        let err = tool
            .call(json!({"skill": "absent"}), fresh_ctx(), fresh_tx())
            .await
            .expect_err("unknown");
        assert!(format!("{err}").contains("Unknown skill: absent"));
    }

    #[tokio::test]
    async fn disable_model_invocation_locked_error() {
        let desc = SkillDescriptor {
            disable_model_invocation: true,
            ..prompt_desc("locked")
        };
        let tool = SkillTool::with_loader(
            shell_test_ctx(dummy_out()),
            Arc::new(FixedLoader(Some(desc))),
        );
        let err = tool
            .call(json!({"skill": "locked"}), fresh_ctx(), fresh_tx())
            .await
            .expect_err("disabled");
        assert!(format!("{err}").contains(
            "Skill locked cannot be used with Skill tool due to disable-model-invocation"
        ));
    }

    #[tokio::test]
    async fn non_prompt_skill_locked_error() {
        let desc = SkillDescriptor {
            command_type: SkillCommandType::Other,
            ..prompt_desc("local-cmd")
        };
        let tool = SkillTool::with_loader(
            shell_test_ctx(dummy_out()),
            Arc::new(FixedLoader(Some(desc))),
        );
        let err = tool
            .call(json!({"skill": "local-cmd"}), fresh_ctx(), fresh_tx())
            .await
            .expect_err("non-prompt");
        assert!(format!("{err}").contains("Skill local-cmd is not a prompt-based skill"));
    }

    #[tokio::test]
    async fn result_surfaces_model_and_allowed_tools_and_status_inline() {
        let desc = SkillDescriptor {
            model: Some("opus".into()),
            allowed_tools: vec!["Bash".into(), "Read".into()],
            ..prompt_desc("rich")
        };
        let tool = SkillTool::with_loader(
            shell_test_ctx(dummy_out()),
            Arc::new(FixedLoader(Some(desc))),
        );
        let out = tool
            .call(json!({"skill": "rich"}), fresh_ctx(), fresh_tx())
            .await
            .expect("ok");
        assert_eq!(out.data["status"], json!("inline"));
        assert_eq!(out.data["model"], json!("opus"));
        assert_eq!(out.data["allowedTools"], json!(["Bash", "Read"]));
        assert_eq!(out.data["commandName"], json!("rich"));
    }

    #[tokio::test]
    async fn model_and_allowed_tools_omitted_when_absent() {
        let tool = SkillTool::with_loader(
            shell_test_ctx(dummy_out()),
            Arc::new(FixedLoader(Some(prompt_desc("plain")))),
        );
        let out = tool
            .call(json!({"skill": "plain"}), fresh_ctx(), fresh_tx())
            .await
            .expect("ok");
        // Optional fields are omitted (matches TS `.optional()` surfacing).
        assert!(out.data.get("model").is_none());
        assert!(out.data.get("allowedTools").is_none());
        assert_eq!(out.data["body"], json!("body here"));
    }

    #[tokio::test]
    async fn model_frontmatter_returns_context_modifier_switching_main_loop_model() {
        // SKILLEXEC.3 (model scope): a skill with `model:` returns a
        // `context_modifier` that switches the turn's main-loop model. Here the
        // seed `ctx` (fresh_ctx → main_loop_model "test", no `[1m]`) has no 1M
        // suffix, so the resolved model is the bare override.
        let desc = SkillDescriptor {
            model: Some("claude-opus-4-6".into()),
            ..prompt_desc("switcher")
        };
        let tool = SkillTool::with_loader(
            shell_test_ctx(dummy_out()),
            Arc::new(FixedLoader(Some(desc))),
        );
        let out = tool
            .call(json!({"skill": "switcher"}), fresh_ctx(), fresh_tx())
            .await
            .expect("ok");
        let modifier = out
            .context_modifier
            .expect("a skill with model: returns a context_modifier");
        let modified = modifier(fresh_ctx());
        assert_eq!(modified.options.main_loop_model, "claude-opus-4-6");
    }

    #[tokio::test]
    async fn model_frontmatter_modifier_preserves_1m_suffix() {
        // When the session is on `[1m]` and the skill's family supports 1M, the
        // suffix is carried over (TS resolveSkillModelOverride).
        let desc = SkillDescriptor {
            model: Some("claude-sonnet-4-6".into()),
            ..prompt_desc("switcher")
        };
        let tool = SkillTool::with_loader(
            shell_test_ctx(dummy_out()),
            Arc::new(FixedLoader(Some(desc))),
        );
        let out = tool
            .call(json!({"skill": "switcher"}), fresh_ctx(), fresh_tx())
            .await
            .expect("ok");
        let modifier = out.context_modifier.expect("context_modifier present");
        let mut seed = fresh_ctx();
        seed.options.main_loop_model = "claude-opus-4-6[1m]".into();
        let modified = modifier(seed);
        assert_eq!(modified.options.main_loop_model, "claude-sonnet-4-6[1m]");
    }

    #[tokio::test]
    async fn no_model_frontmatter_returns_no_context_modifier() {
        // Byte-identical guard: a skill WITHOUT a `model:` frontmatter returns
        // `context_modifier: None`, so the turn loop's no-override path is
        // untouched (session.model never changes).
        let tool = SkillTool::with_loader(
            shell_test_ctx(dummy_out()),
            Arc::new(FixedLoader(Some(prompt_desc("plain")))),
        );
        let out = tool
            .call(json!({"skill": "plain"}), fresh_ctx(), fresh_tx())
            .await
            .expect("ok");
        assert!(
            out.context_modifier.is_none(),
            "no model: frontmatter → no context_modifier (byte-identical)"
        );
    }

    #[tokio::test]
    async fn model_content_is_inline_launching_line_and_omits_body() {
        // TS inline path (SkillTool.ts:856-861): the model's tool_result content
        // is exactly `Launching skill: ${commandName}` — never the JSON dump of
        // the output object (which would leak the full skill `body`).
        let desc = SkillDescriptor {
            body: "SECRET FULL SKILL BODY that must not reach the model".into(),
            ..prompt_desc("commit")
        };
        let tool = SkillTool::with_loader(
            shell_test_ctx(dummy_out()),
            Arc::new(FixedLoader(Some(desc))),
        );
        // Slash-prefixed input: model_content uses the normalized name.
        let out = tool
            .call(json!({"skill": "/commit"}), fresh_ctx(), fresh_tx())
            .await
            .expect("ok");
        let mc = out.data["model_content"]
            .as_str()
            .expect("model_content is a string");
        assert_eq!(mc, "Launching skill: commit");
        // The body still rides along for non-model consumers, but is NOT in the
        // model-facing string.
        assert!(!mc.contains("SECRET FULL SKILL BODY"));
        assert_eq!(
            out.data["body"],
            json!("SECRET FULL SKILL BODY that must not reach the model")
        );
    }

    #[tokio::test]
    async fn args_accepted_optionally_and_echoed() {
        let tool = SkillTool::with_loader(
            shell_test_ctx(dummy_out()),
            Arc::new(FixedLoader(Some(prompt_desc("commit")))),
        );
        // With args.
        let out = tool
            .call(
                json!({"skill": "commit", "args": "--amend"}),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .expect("ok");
        assert_eq!(out.data["args"], json!("--amend"));
        // Without args — no `args` key.
        let out2 = tool
            .call(json!({"skill": "commit"}), fresh_ctx(), fresh_tx())
            .await
            .expect("ok");
        assert!(out2.data.get("args").is_none());
    }

    /// SKILLEXEC.3: a model-invocable Prompt skill expands its body's
    /// `$ARGUMENTS` into a `new_messages` user message the turn loop injects, so
    /// the model acts on the expanded skill prompt.
    #[tokio::test]
    async fn expands_arguments_into_new_messages() {
        let desc = SkillDescriptor {
            body: "Review PR $ARGUMENTS now".into(),
            ..prompt_desc("review-pr")
        };
        let tool = SkillTool::with_loader(
            shell_test_ctx(dummy_out()),
            Arc::new(FixedLoader(Some(desc))),
        );
        let out = tool
            .call(
                json!({"skill": "review-pr", "args": "123"}),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .expect("ok");
        assert_eq!(out.new_messages.len(), 1, "expanded prompt injected");
        match &out.new_messages[0] {
            protocol::ConversationMessage::User { content, .. } => match content.first() {
                Some(protocol::ContentBlock::Text { text }) => {
                    assert_eq!(text, "Review PR 123 now");
                }
                other => panic!("expected leading Text block, got {other:?}"),
            },
            other => panic!("expected injected User message, got {other:?}"),
        }
        // The inline model-facing string is still the launch line — the body is
        // NOT leaked into the tool_result content (SKILLEXEC.1 invariant holds).
        assert_eq!(
            out.data["model_content"],
            json!("Launching skill: review-pr")
        );
    }

    /// Named frontmatter arguments (`$name`) resolve via the descriptor's
    /// `argument_names` (TS `processPromptSlashCommand` passes the command's
    /// `argNames`).
    #[tokio::test]
    async fn expands_named_argument_into_new_messages() {
        let desc = SkillDescriptor {
            body: "Hello $name".into(),
            argument_names: vec!["name".into()],
            ..prompt_desc("greet")
        };
        let tool = SkillTool::with_loader(
            shell_test_ctx(dummy_out()),
            Arc::new(FixedLoader(Some(desc))),
        );
        let out = tool
            .call(
                json!({"skill": "greet", "args": "world"}),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .expect("ok");
        match &out.new_messages[0] {
            protocol::ConversationMessage::User { content, .. } => match content.first() {
                Some(protocol::ContentBlock::Text { text }) => assert_eq!(text, "Hello world"),
                other => panic!("expected leading Text block, got {other:?}"),
            },
            other => panic!("expected injected User message, got {other:?}"),
        }
    }

    /// A skill with no placeholders and empty args injects the body verbatim
    /// (no `$ARGUMENTS` tail append — TS appends only when args is non-empty).
    #[tokio::test]
    async fn no_placeholder_no_args_injects_body_verbatim() {
        let tool = SkillTool::with_loader(
            shell_test_ctx(dummy_out()),
            Arc::new(FixedLoader(Some(prompt_desc("plain")))),
        );
        let out = tool
            .call(json!({"skill": "plain"}), fresh_ctx(), fresh_tx())
            .await
            .expect("ok");
        match &out.new_messages[0] {
            protocol::ConversationMessage::User { content, .. } => match content.first() {
                Some(protocol::ContentBlock::Text { text }) => assert_eq!(text, "body here"),
                other => panic!("expected leading Text block, got {other:?}"),
            },
            other => panic!("expected injected User message, got {other:?}"),
        }
    }

    fn out_with_stdout(stdout: &str) -> ProcessOutput {
        ProcessOutput {
            stdout: stdout.to_string(),
            stderr: String::new(),
            exit_code: 0,
            timed_out: false,
        }
    }

    /// SKILLEXEC.6: a skill body with an embedded inline `!`…`` block runs the
    /// command through the injected shell runner and splices its stdout into the
    /// expanded prompt (the fake `ProcessRunner` returns "hi" for any command).
    #[tokio::test]
    async fn expands_inline_shell_command_into_new_messages() {
        let desc = SkillDescriptor {
            body: "before !`echo hi` after".into(),
            ..prompt_desc("sh")
        };
        // The stub process returns this stdout for the (single) embedded command.
        let tool = SkillTool::with_loader(
            shell_test_ctx(out_with_stdout("hi\n")),
            Arc::new(FixedLoader(Some(desc))),
        );
        let out = tool
            .call(json!({"skill": "sh"}), fresh_ctx(), fresh_tx())
            .await
            .expect("ok");
        match &out.new_messages[0] {
            protocol::ConversationMessage::User { content, .. } => match content.first() {
                Some(protocol::ContentBlock::Text { text }) => {
                    // format_bash_output trims the stdout -> "hi".
                    assert_eq!(text, "before hi after");
                }
                other => panic!("expected leading Text block, got {other:?}"),
            },
            other => panic!("expected injected User message, got {other:?}"),
        }
        // The model-facing line is still the launch string (body not leaked).
        assert_eq!(out.data["model_content"], json!("Launching skill: sh"));
    }

    /// SAFETY: a skill body with NO `!command` is byte-identical to the
    /// argument-substituted text — the shell-expansion engine returns the input
    /// unchanged and the fake process is NEVER invoked (it would error if it were:
    /// the stub is exhausted after one call, but no call happens).
    #[tokio::test]
    async fn no_shell_command_is_byte_identical_to_arg_substituted() {
        let body = "Review PR $ARGUMENTS now (no shell here)";
        let desc = SkillDescriptor {
            body: body.into(),
            ..prompt_desc("nb")
        };
        // Independently compute the pure arg-substitution result.
        let expected =
            command_api::substitute_arguments_faithful(body, Some("123"), true, &[]).unwrap();
        // dummy_out() has empty stdout; if the runner were ever called and then
        // called AGAIN, the stub would error — proving the no-op path.
        let tool = SkillTool::with_loader(
            shell_test_ctx(dummy_out()),
            Arc::new(FixedLoader(Some(desc))),
        );
        let out = tool
            .call(
                json!({"skill": "nb", "args": "123"}),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .expect("ok");
        match &out.new_messages[0] {
            protocol::ConversationMessage::User { content, .. } => match content.first() {
                Some(protocol::ContentBlock::Text { text }) => {
                    assert_eq!(text, &expected);
                    assert_eq!(text, "Review PR 123 now (no shell here)");
                }
                other => panic!("expected leading Text block, got {other:?}"),
            },
            other => panic!("expected injected User message, got {other:?}"),
        }
    }

    /// SAFETY: an MCP-sourced skill (`skip_shell_expansion = true`) is
    /// byte-identical to the arg-substituted text even when its body LOOKS like it
    /// has an embedded `!command` — the expansion call is skipped entirely (TS
    /// `loadedFrom !== 'mcp'` gate).
    #[tokio::test]
    async fn mcp_skip_shell_expansion_is_byte_identical() {
        let desc = SkillDescriptor {
            body: "before !`echo hi` after".into(),
            skip_shell_expansion: true,
            ..prompt_desc("mcpish")
        };
        // dummy_out(): the runner must NOT be called, so its (empty) stdout never
        // matters; a call would not change the body, but skip proves no execution.
        let tool = SkillTool::with_loader(
            shell_test_ctx(dummy_out()),
            Arc::new(FixedLoader(Some(desc))),
        );
        let out = tool
            .call(json!({"skill": "mcpish"}), fresh_ctx(), fresh_tx())
            .await
            .expect("ok");
        match &out.new_messages[0] {
            protocol::ConversationMessage::User { content, .. } => match content.first() {
                Some(protocol::ContentBlock::Text { text }) => {
                    // Body is verbatim — the `!`echo hi`` block is NOT expanded.
                    assert_eq!(text, "before !`echo hi` after");
                }
                other => panic!("expected leading Text block, got {other:?}"),
            },
            other => panic!("expected injected User message, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn descriptor_truncated_at_1024() {
        let desc = SkillDescriptor {
            description: "x".repeat(MAX_SKILL_DESCRIPTOR_LEN + 100),
            ..prompt_desc("huge")
        };
        let tool = SkillTool::with_loader(
            shell_test_ctx(dummy_out()),
            Arc::new(FixedLoader(Some(desc))),
        );
        let out = tool
            .call(json!({"skill": "huge"}), fresh_ctx(), fresh_tx())
            .await
            .expect("ok");
        assert!(out.data["body"].as_str().is_some(), "body present");
        assert_eq!(out.data["descriptor_truncated"], json!(true));
    }

    #[tokio::test]
    async fn blank_skill_rejected() {
        let tool = SkillTool::new(shell_test_ctx(dummy_out()));
        let err = tool
            .call(json!({"skill": "   "}), fresh_ctx(), fresh_tx())
            .await
            .expect_err("blank");
        assert!(format!("{err}").contains("Invalid skill format"));
    }

    #[tokio::test]
    async fn rejects_missing_skill() {
        let tool = SkillTool::new(shell_test_ctx(dummy_out()));
        let err = tool
            .call(json!({}), fresh_ctx(), fresh_tx())
            .await
            .expect_err("missing");
        assert!(format!("{err}").contains("missing or non-string skill"));
    }

    #[tokio::test]
    async fn validate_input_applies_locked_rejections() {
        // Unknown via empty loader.
        let tool = SkillTool::new(shell_test_ctx(dummy_out()));
        let err = tool
            .validate_input(&json!({"skill": "/absent"}), &fresh_ctx())
            .await
            .expect_err("unknown");
        assert!(err.0.contains("Unknown skill: absent"));

        // disable-model-invocation.
        let desc = SkillDescriptor {
            disable_model_invocation: true,
            ..prompt_desc("x")
        };
        let tool = SkillTool::with_loader(
            shell_test_ctx(dummy_out()),
            Arc::new(FixedLoader(Some(desc))),
        );
        let err = tool
            .validate_input(&json!({"skill": "x"}), &fresh_ctx())
            .await
            .expect_err("disabled");
        assert!(err
            .0
            .contains("cannot be used with Skill tool due to disable-model-invocation"));
    }

    // ========================================================================
    // ${CLAUDE_SKILL_DIR} / ${CLAUDE_SESSION_ID} token substitution
    // (TS getPromptForCommand steps 2-3, loadSkillsDir.ts:359-369). The tokens
    // are substituted AFTER argument substitution and BEFORE shell expansion.
    // ========================================================================

    /// Extract the leading Text block of the single injected user message.
    fn injected_text(out: &ToolCallResult) -> String {
        match &out.new_messages[0] {
            protocol::ConversationMessage::User { content, .. } => match content.first() {
                Some(protocol::ContentBlock::Text { text }) => text.clone(),
                other => panic!("expected leading Text block, got {other:?}"),
            },
            other => panic!("expected injected User message, got {other:?}"),
        }
    }

    /// Step 2: `${CLAUDE_SKILL_DIR}` is replaced with `skill_root` when present.
    #[tokio::test]
    async fn skill_dir_token_replaced_when_skill_root_present() {
        let desc = SkillDescriptor {
            body: "scripts live in ${CLAUDE_SKILL_DIR}/bin".into(),
            skill_root: Some(std::path::PathBuf::from("/skills/foo")),
            ..prompt_desc("dir")
        };
        let tool = SkillTool::with_loader(
            shell_test_ctx(dummy_out()),
            Arc::new(FixedLoader(Some(desc))),
        );
        let out = tool
            .call(json!({"skill": "dir"}), fresh_ctx(), fresh_tx())
            .await
            .expect("ok");
        assert_eq!(
            injected_text(&out),
            "Base directory for this skill: /skills/foo\n\nscripts live in /skills/foo/bin"
        );
    }

    #[tokio::test]
    async fn file_backed_skill_prompt_includes_base_directory_prefix() {
        let desc = SkillDescriptor {
            body: "Use the local scripts".into(),
            skill_root: Some(std::path::PathBuf::from("/skills/foo")),
            ..prompt_desc("dir")
        };
        let tool = SkillTool::with_loader(
            shell_test_ctx(dummy_out()),
            Arc::new(FixedLoader(Some(desc))),
        );
        let out = tool
            .call(json!({"skill": "dir"}), fresh_ctx(), fresh_tx())
            .await
            .expect("ok");
        assert_eq!(
            injected_text(&out),
            "Base directory for this skill: /skills/foo\n\nUse the local scripts"
        );
    }

    /// Step 2 (gate): with NO `skill_root` (e.g. MCP / non-file skills), the
    /// `${CLAUDE_SKILL_DIR}` token is left untouched (TS gates on `baseDir`).
    #[tokio::test]
    async fn skill_dir_token_left_as_is_when_skill_root_absent() {
        let desc = SkillDescriptor {
            body: "scripts live in ${CLAUDE_SKILL_DIR}/bin".into(),
            skill_root: None,
            ..prompt_desc("dir")
        };
        let tool = SkillTool::with_loader(
            shell_test_ctx(dummy_out()),
            Arc::new(FixedLoader(Some(desc))),
        );
        let out = tool
            .call(json!({"skill": "dir"}), fresh_ctx(), fresh_tx())
            .await
            .expect("ok");
        assert_eq!(
            injected_text(&out),
            "scripts live in ${CLAUDE_SKILL_DIR}/bin"
        );
    }

    /// Step 2: ALL occurrences of `${CLAUDE_SKILL_DIR}` are replaced (global,
    /// matching the TS `/…/g` regex).
    #[tokio::test]
    async fn skill_dir_token_replaced_globally() {
        let desc = SkillDescriptor {
            body: "${CLAUDE_SKILL_DIR}/a and ${CLAUDE_SKILL_DIR}/b".into(),
            skill_root: Some(std::path::PathBuf::from("/r")),
            ..prompt_desc("dir")
        };
        let tool = SkillTool::with_loader(
            shell_test_ctx(dummy_out()),
            Arc::new(FixedLoader(Some(desc))),
        );
        let out = tool
            .call(json!({"skill": "dir"}), fresh_ctx(), fresh_tx())
            .await
            .expect("ok");
        assert_eq!(
            injected_text(&out),
            "Base directory for this skill: /r\n\n/r/a and /r/b"
        );
    }

    /// Step 3: `${CLAUDE_SESSION_ID}` is replaced with the session id (always,
    /// when one is wired) — including every occurrence.
    #[tokio::test]
    async fn session_id_token_replaced() {
        let desc = SkillDescriptor {
            body: "session ${CLAUDE_SESSION_ID} = ${CLAUDE_SESSION_ID}".into(),
            session_id: Some("sess:abc-123".into()),
            ..prompt_desc("sid")
        };
        let tool = SkillTool::with_loader(
            shell_test_ctx(dummy_out()),
            Arc::new(FixedLoader(Some(desc))),
        );
        let out = tool
            .call(json!({"skill": "sid"}), fresh_ctx(), fresh_tx())
            .await
            .expect("ok");
        assert_eq!(injected_text(&out), "session sess:abc-123 = sess:abc-123");
    }

    /// Step 3 (gate): with NO `session_id` wired (hermetic loader), the token is
    /// left untouched rather than substituting an empty string.
    #[tokio::test]
    async fn session_id_token_left_as_is_when_unset() {
        let desc = SkillDescriptor {
            body: "session ${CLAUDE_SESSION_ID}".into(),
            session_id: None,
            ..prompt_desc("sid")
        };
        let tool = SkillTool::with_loader(
            shell_test_ctx(dummy_out()),
            Arc::new(FixedLoader(Some(desc))),
        );
        let out = tool
            .call(json!({"skill": "sid"}), fresh_ctx(), fresh_tx())
            .await
            .expect("ok");
        assert_eq!(injected_text(&out), "session ${CLAUDE_SESSION_ID}");
    }

    /// A `ProcessRunner` that records the embedded shell command it was handed
    /// (the last `args` element, which `SkillShellRunner` fills with the expanded
    /// `-c` command string) so a test can assert WHAT the shell saw.
    struct CapturingProcess {
        seen: std::sync::Mutex<Vec<String>>,
        stdout: String,
    }
    #[async_trait]
    impl traits::process::ProcessRunner for CapturingProcess {
        async fn run(
            &self,
            cmd: &traits::sandbox::SandboxedCommand,
        ) -> Result<ProcessOutput, traits::process::ProcessError> {
            if let Some(last) = cmd.inner().args.last() {
                self.seen.lock().unwrap().push(last.clone());
            }
            Ok(ProcessOutput {
                stdout: self.stdout.clone(),
                stderr: String::new(),
                exit_code: 0,
                timed_out: false,
            })
        }
        async fn spawn_background(
            &self,
            _cmd: &traits::sandbox::SandboxedCommand,
        ) -> Result<traits::process::ProcessHandle, traits::process::ProcessError> {
            Err(traits::process::ProcessError::Unsupported)
        }
        async fn kill(
            &self,
            _handle: &traits::process::ProcessHandle,
        ) -> Result<(), traits::process::ProcessError> {
            Ok(())
        }
        fn is_available(&self) -> bool {
            true
        }
    }

    /// ORDERING: a `!command` block that references `${CLAUDE_SESSION_ID}` sees
    /// the SUBSTITUTED value — token replacement (step 3) runs BEFORE the embedded
    /// `!command` shell expansion (step 4). We capture the command string the
    /// shell runner is handed and assert the token is already substituted there.
    #[tokio::test]
    async fn token_substitution_precedes_shell_expansion() {
        let capture = Arc::new(CapturingProcess {
            seen: std::sync::Mutex::new(Vec::new()),
            stdout: "OUT\n".into(),
        });
        let mut ctx = shell_test_ctx(dummy_out());
        ctx.process = capture.clone();
        let desc = SkillDescriptor {
            body: "pre !`echo ${CLAUDE_SESSION_ID}` post".into(),
            session_id: Some("sess:zzz".into()),
            ..prompt_desc("ord")
        };
        let tool = SkillTool::with_loader(ctx, Arc::new(FixedLoader(Some(desc))));
        let out = tool
            .call(json!({"skill": "ord"}), fresh_ctx(), fresh_tx())
            .await
            .expect("ok");
        // The expanded prompt splices the (trimmed) stdout.
        assert_eq!(injected_text(&out), "pre OUT post");
        // The command the shell actually ran already had the token substituted.
        let seen = capture.seen.lock().unwrap();
        assert_eq!(seen.len(), 1, "exactly one embedded command ran");
        assert!(
            seen[0].contains("echo sess:zzz"),
            "shell saw substituted session id, got: {}",
            seen[0]
        );
        assert!(
            !seen[0].contains("${CLAUDE_SESSION_ID}"),
            "token must be substituted BEFORE shell expansion, got: {}",
            seen[0]
        );
    }

    /// Records `wrap`/`cleanup_after_command` calls so a test can prove the
    /// skill `!command` expansion routes through `ctx.sandbox_runner`.
    struct WrapCall {
        command: String,
        bin_shell: Option<String>,
        cwd: Option<std::path::PathBuf>,
    }

    #[derive(Default)]
    struct RecordingSandboxRunner {
        wrap_calls: std::sync::Mutex<Vec<WrapCall>>,
        cleanups: std::sync::atomic::AtomicUsize,
    }

    #[async_trait]
    impl tool_api::SandboxRunner for RecordingSandboxRunner {
        async fn wrap(
            &self,
            command: &str,
            _cfg: &sandbox::runtime_config::SandboxRuntimeConfig,
            _platform: sandbox::runtime_config::Platform,
            bin_shell: Option<&str>,
            cwd: Option<&std::path::Path>,
        ) -> Result<String, sandbox::wrap::SandboxWrapError> {
            self.wrap_calls.lock().unwrap().push(WrapCall {
                command: command.to_string(),
                bin_shell: bin_shell.map(ToString::to_string),
                cwd: cwd.map(std::path::Path::to_path_buf),
            });
            Ok(format!("WRAPPED::{command}"))
        }

        async fn cleanup_after_command(&self) {
            self.cleanups
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
    }

    /// The embedded `!command` shell expansion routes through the injected
    /// `ctx.sandbox_runner` (not the sync free fn): the runner's wrapped output
    /// is what gets spawned, it sees the resolved shell + workspace cwd, and
    /// `cleanup_after_command` runs after the command finishes.
    #[tokio::test]
    async fn shell_expansion_routes_through_injected_runner_and_cleans_up() {
        let capture = Arc::new(CapturingProcess {
            seen: std::sync::Mutex::new(Vec::new()),
            stdout: "OUT\n".into(),
        });
        let runner = Arc::new(RecordingSandboxRunner::default());
        let mut ctx = shell_test_ctx(dummy_out());
        ctx.process = capture.clone();
        // Force the Sandbox branch: a non-empty, non-excluded command runs
        // through the wrap whenever the host has a working sandbox backend.
        ctx.sandbox_available = true;
        ctx.workspace = std::path::PathBuf::from("/tmp");
        ctx.sandbox_runner = runner.clone();

        let desc = SkillDescriptor {
            body: "pre !`echo hi` post".into(),
            ..prompt_desc("wrap")
        };
        let tool = SkillTool::with_loader(ctx, Arc::new(FixedLoader(Some(desc))));
        tool.call(json!({"skill": "wrap"}), fresh_ctx(), fresh_tx())
            .await
            .expect("ok");

        let calls = runner.wrap_calls.lock().unwrap();
        assert_eq!(calls.len(), 1, "wrap should be called once");
        let call = &calls[0];
        assert!(
            call.command.contains("echo hi"),
            "runner received the embedded command, got: {}",
            call.command
        );
        assert_eq!(call.bin_shell.as_deref(), Some(resolve_skill_shell_path()));
        assert_eq!(call.cwd.as_deref(), Some(std::path::Path::new("/tmp")));

        // The runner's wrapped output (sentinel) is what actually got spawned.
        let seen = capture.seen.lock().unwrap();
        assert_eq!(seen.len(), 1, "exactly one embedded command ran");
        assert!(
            seen[0].contains("WRAPPED::"),
            "the runner's wrapped command must be spawned, got: {}",
            seen[0]
        );
        drop(seen);

        assert_eq!(
            runner.cleanups.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "cleanup_after_command must be invoked once"
        );
    }

    #[tokio::test]
    async fn file_backed_body_without_tokens_still_gets_base_directory_prefix() {
        let body = "Plain body $ARGUMENTS, no tokens here at all.";
        let desc = SkillDescriptor {
            body: body.into(),
            skill_root: Some(std::path::PathBuf::from("/r")),
            session_id: Some("sess:abc".into()),
            ..prompt_desc("plain")
        };
        let expected =
            command_api::substitute_arguments_faithful(body, Some("X"), true, &[]).unwrap();
        let tool = SkillTool::with_loader(
            shell_test_ctx(dummy_out()),
            Arc::new(FixedLoader(Some(desc))),
        );
        let out = tool
            .call(
                json!({"skill": "plain", "args": "X"}),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .expect("ok");
        assert_eq!(
            injected_text(&out),
            format!("Base directory for this skill: /r\n\n{expected}")
        );
    }

    // ========================================================================
    // SKILLEXEC.6 sandbox parity: the embedded `!command` runner mirrors
    // `BashTool::call`'s `should_use_sandbox` + `wrap_with_sandbox` decision.
    // ========================================================================

    /// The platform wrapper prefix `wrap_with_sandbox` emits, so the assertions
    /// below stay host-agnostic: `sandbox-exec -f` on macOS, `bwrap ` elsewhere.
    fn sandbox_wrap_prefix() -> &'static str {
        if cfg!(target_os = "macos") {
            "sandbox-exec -f"
        } else {
            "bwrap "
        }
    }

    /// With a config that WOULD sandbox (`sandbox_available = true`, default
    /// permission mode, classifier `None` so the trusted-safe shortcut never
    /// fires), the embedded command is run through `wrap_with_sandbox` — the
    /// spawned command string is the WRAPPED form, exactly as `BashTool::call`
    /// would produce. We capture the spawned `-c -l` payload and assert it is the
    /// platform sandbox wrapper, with the original command nested inside.
    #[tokio::test]
    async fn embedded_command_is_sandbox_wrapped_when_decision_says_sandbox() {
        let capture = Arc::new(CapturingProcess {
            seen: std::sync::Mutex::new(Vec::new()),
            stdout: "OUT\n".into(),
        });
        let mut ctx = shell_test_ctx(dummy_out());
        ctx.process = capture.clone();
        // Flip the one input that moves the decision from NoSandbox -> Sandbox:
        // a working sandbox backend. (Default mode + classifier `None` means the
        // trusted-safe shortcut is skipped, so the decision lands on Sandbox.)
        ctx.sandbox_available = true;
        let desc = SkillDescriptor {
            body: "pre !`echo hi` post".into(),
            ..prompt_desc("sbx")
        };
        let tool = SkillTool::with_loader(ctx, Arc::new(FixedLoader(Some(desc))));
        let out = tool
            .call(json!({"skill": "sbx"}), fresh_ctx(), fresh_tx())
            .await
            .expect("ok");
        // The splice still works (the stub stdout is "OUT").
        assert_eq!(injected_text(&out), "pre OUT post");
        let seen = capture.seen.lock().unwrap();
        assert_eq!(seen.len(), 1, "exactly one embedded command ran");
        // The spawned payload is the platform sandbox wrapper — NOT the raw
        // command — proving the should_use_sandbox + wrap_with_sandbox path ran.
        assert!(
            seen[0].starts_with(sandbox_wrap_prefix()),
            "embedded command must be sandbox-wrapped, got: {}",
            seen[0]
        );
        // The original command (plus the BASH.1 extglob guard) is nested inside
        // the wrapper's `/bin/sh -c '…'` payload.
        assert!(
            seen[0].contains("echo hi"),
            "wrapped command should still carry the original command, got: {}",
            seen[0]
        );
    }

    /// With the DEFAULT config (`sandbox_available = false`), the decision is
    /// `NoSandbox`, so the embedded command is run UNWRAPPED — byte-identical to
    /// the pre-sandbox-parity behavior. The spawned payload is the bare
    /// extglob-guarded command, never the platform wrapper.
    #[tokio::test]
    async fn embedded_command_is_unwrapped_when_decision_says_no_sandbox() {
        let capture = Arc::new(CapturingProcess {
            seen: std::sync::Mutex::new(Vec::new()),
            stdout: "OUT\n".into(),
        });
        // shell_test_ctx defaults: sandbox_available = false -> NoSandbox.
        let mut ctx = shell_test_ctx(dummy_out());
        ctx.process = capture.clone();
        let desc = SkillDescriptor {
            body: "pre !`echo hi` post".into(),
            ..prompt_desc("nosbx")
        };
        let tool = SkillTool::with_loader(ctx, Arc::new(FixedLoader(Some(desc))));
        let out = tool
            .call(json!({"skill": "nosbx"}), fresh_ctx(), fresh_tx())
            .await
            .expect("ok");
        assert_eq!(injected_text(&out), "pre OUT post");
        let seen = capture.seen.lock().unwrap();
        assert_eq!(seen.len(), 1, "exactly one embedded command ran");
        // No sandbox wrapper — the raw command runs directly.
        assert!(
            !seen[0].starts_with(sandbox_wrap_prefix()),
            "NoSandbox path must run the command unwrapped, got: {}",
            seen[0]
        );
        assert!(
            seen[0].contains("echo hi"),
            "unwrapped command should be the raw command, got: {}",
            seen[0]
        );
    }
}
