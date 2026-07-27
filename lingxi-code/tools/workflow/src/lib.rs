//! `tool-workflow` — the `Workflow` tool: launch a model-authored workflow
//! script as a background task.
//!
//! claude-code's Workflow tool runs the script in the background, returning
//! immediately with `{ status: "async_launched", taskId, taskType:
//! "local_workflow" }`; a `<task-notification>` arrives when it completes. The
//! script orchestrates subagents (`agent()`/`parallel()`/`pipeline()`/…);
//! LingXi runs it on the `workflow` crate's QuickJS runtime via a
//! `LocalWorkflow` background task (`tasks::handlers::local_workflow`).
//!
//! The byte-exact model-facing surface — the tool name, the long-form
//! description ([`Tool::prompt`]), and the input schema — is reproduced from the
//! claude-code v2.1.185 binary. The launch itself goes through the injected
//! [`WorkflowLauncher`] seam, which the composition root wires over the task
//! registry; with no launcher wired the tool serves its surface but `call`
//! reports a clear error.

#![forbid(unsafe_code)]

mod size_guideline;
pub use size_guideline::{prompt_appendix_for, WorkflowSizeGuideline};

use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use once_cell::sync::Lazy;
use permission::result::PermissionMetadata;
use permission::{PermissionDecisionReason, PermissionResult};
use serde_json::{json, Value};
use traits::env::is_env_truthy;

use tool_api::context::ToolUseContext;
use tool_api::progress::ToolProgressSender;
use tool_api::tool_trait::{
    DescriptionOptions, PromptOptions, Tool, ToolCallResult, ToolError, ToolStaticContext,
    ValidationError,
};

/// Maximum script size in bytes (claude-code `P2 = 524288` = 512 KB).
pub const MAX_SCRIPT_BYTES: usize = 524288;

/// Tool name byte-lock.
pub const TOOL_NAME: &str = "Workflow";

const WORKFLOW_EXTENSIONS: [&str; 4] = [".js", ".mjs", ".ts", ""];

/// The long-form tool description (claude-code v2.1.185 `prompt`), reproduced
/// byte-for-byte. A trailing newline (should an editor add one to the data file)
/// is stripped so the API description matches the binary exactly.
static DESCRIPTION: Lazy<String> = Lazy::new(|| {
    include_str!("workflow_description.txt")
        .trim_end_matches('\n')
        .to_string()
});

/// The input schema (claude-code v2.1.185 `inputSchema`), reproduced from the
/// zod `strictObject` definition (source-order properties, `additionalProperties:
/// false`, no required keys — the "at least one of script/name/scriptPath"
/// constraint is a runtime `.refine`, enforced in [`Tool::validate_input`]).
static INPUT_SCHEMA: Lazy<Value> = Lazy::new(|| {
    serde_json::from_str(include_str!("workflow_input_schema.json"))
        .expect("workflow_input_schema.json is valid JSON")
});

/// What a [`WorkflowLauncher`] needs to start a workflow run. Mirrors the
/// Workflow tool's input minus the `title`/`description` fields (which the tool
/// description marks "Ignored").
#[derive(Debug, Clone, Default)]
pub struct WorkflowLaunchSpec {
    /// Inline self-contained script source.
    pub script: Option<String>,
    /// Name of a predefined/saved workflow.
    pub name: Option<String>,
    /// Path to a script file on disk (takes precedence over `script`/`name`).
    pub script_path: Option<String>,
    /// `args` global value, passed verbatim.
    pub args: Option<Value>,
    /// Resume a prior run by its `wf_…` id.
    pub resume_from_run_id: Option<String>,
}

/// The result of a successful launch.
#[derive(Debug, Clone, Default)]
pub struct WorkflowLaunched {
    /// The background task id (claude-code `taskId`).
    pub task_id: String,
    /// The local run id for `resumeFromRunId` (claude-code `runId`). Minted at
    /// launch so the model can pass it back to resume; `None` if unavailable.
    pub run_id: Option<String>,
    /// Path to the persisted workflow script for this invocation (claude-code
    /// `scriptPath`) — editable, and passable back as `scriptPath` to re-run
    /// without resending the script. `None` if the host did not persist it.
    pub script_path: Option<String>,
    /// `meta.name` from the script (claude-code `workflowName`). `None` if not
    /// extracted.
    pub workflow_name: Option<String>,
    /// `meta.description` from the script (claude-code `summary = p`). `None`
    /// if the workflow has no description in its meta block.
    pub summary: Option<String>,
    /// Directory where subagent transcripts are written (claude-code
    /// `transcriptDir = Nte(runId)` → `<sessionProjectDir>/<sessionId>/subagents/workflows/<runId>`).
    /// `None` if the session dir is not available to the launcher.
    pub transcript_dir: Option<String>,
}

/// Error launching a workflow.
#[derive(Debug)]
pub struct WorkflowLaunchError(pub String);

impl std::fmt::Display for WorkflowLaunchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}
impl std::error::Error for WorkflowLaunchError {}

fn user_config_home_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os(branding::CONFIG_DIR_ENV) {
        return Some(PathBuf::from(dir));
    }
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .map(|home| home.join(branding::DOT_DIR))
}

fn saved_workflow_dirs() -> Vec<PathBuf> {
    let project = PathBuf::from(branding::DOT_DIR).join("workflows");
    let mut dirs = vec![project.clone()];
    if let Some(user) = user_config_home_dir().map(|home| home.join("workflows")) {
        if user != project {
            dirs.push(user);
        }
    }
    dirs
}

fn saved_workflow_candidates(name: &str) -> Vec<PathBuf> {
    saved_workflow_dirs()
        .into_iter()
        .flat_map(|dir| {
            WORKFLOW_EXTENSIONS
                .iter()
                .map(move |ext| dir.join(format!("{name}{ext}")))
        })
        .collect()
}

fn path_for_read(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn saved_workflow_dirs_display() -> String {
    saved_workflow_dirs()
        .into_iter()
        .map(|dir| {
            let mut s = dir.to_string_lossy().into_owned();
            if !s.ends_with(std::path::MAIN_SEPARATOR) {
                s.push(std::path::MAIN_SEPARATOR);
            }
            s
        })
        .collect::<Vec<_>>()
        .join(" or ")
}

/// Resolve a launch spec to a script source. Precedence follows claude-code:
/// `scriptPath` over `script` over `name` (the schema marks `scriptPath` as
/// "Takes precedence over `script` and `name`"). `read` loads a file's contents
/// (the host provides real I/O); `name` resolution looks first under the
/// project saved-workflow directory (`.lingxi/workflows/<name>`) and then under
/// the user config directory (`$LINGXI_CONFIG_DIR/workflows/<name>` or
/// `~/.lingxi/workflows/<name>`) with common script extensions. (LingXi ships no
/// built-in workflow library, so a `name` that isn't a saved file is an error.)
pub fn resolve_script<R>(spec: &WorkflowLaunchSpec, read: R) -> Result<String, WorkflowLaunchError>
where
    R: Fn(&str) -> std::io::Result<String>,
{
    let nonempty = |o: &Option<String>| o.as_deref().filter(|s| !s.is_empty()).map(str::to_string);
    if let Some(path) = nonempty(&spec.script_path) {
        return read(&path)
            .map_err(|e| WorkflowLaunchError(format!("cannot read scriptPath '{path}': {e}")));
    }
    if let Some(script) = nonempty(&spec.script) {
        return Ok(script);
    }
    if let Some(name) = nonempty(&spec.name) {
        for candidate in saved_workflow_candidates(&name) {
            if let Ok(src) = read(&path_for_read(&candidate)) {
                return Ok(src);
            }
        }
        return Err(WorkflowLaunchError(format!(
            "no saved workflow named '{name}' under {}",
            saved_workflow_dirs_display()
        )));
    }
    Err(WorkflowLaunchError(
        "Must provide script, name, or scriptPath".into(),
    ))
}

// ── Save dynamic workflow (claude-code `eya` / `uQ_`, dialog mode:"save") ─────

/// Where a saved workflow is written — the oracle's `scope` field of the "Save
/// dynamic workflow" dialog (`iNt`). `Project` writes under the project workflow
/// dir (`<cwd>/.lingxi/workflows`); `User` under the user config dir
/// (`$LINGXI_CONFIG_DIR` / `~/.lingxi`, `+ /workflows`). Defaults to `Project`
/// (oracle `useState("project")`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkflowScope {
    /// Project scope — `<cwd>/.lingxi/workflows` (oracle `.claude/workflows`).
    Project,
    /// User scope — `<userConfigHome>/workflows` (oracle `xDt()`).
    User,
}

impl WorkflowScope {
    /// The wire string used in telemetry / persistence (`"project"` / `"user"`).
    #[must_use]
    pub fn wire(self) -> &'static str {
        match self {
            WorkflowScope::Project => "project",
            WorkflowScope::User => "user",
        }
    }
    /// The capitalized UI label (oracle `l3p`): `"Project"` / `"User"`.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            WorkflowScope::Project => "Project",
            WorkflowScope::User => "User",
        }
    }
    /// Tab toggles Project ⇄ User (oracle `IQ_`).
    #[must_use]
    pub fn toggled(self) -> Self {
        match self {
            WorkflowScope::Project => WorkflowScope::User,
            WorkflowScope::User => WorkflowScope::Project,
        }
    }
}

/// Kebab-sanitize a workflow name (oracle `lme`):
/// `toLowerCase()` → collapse each `[^a-z0-9]+` run to a single `-` → trim
/// leading/trailing `-`; an empty result becomes `"workflow"`.
#[must_use]
pub fn sanitize_workflow_name(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut prev_dash = false;
    for ch in name.chars() {
        let lc = ch.to_ascii_lowercase();
        if lc.is_ascii_lowercase() || lc.is_ascii_digit() {
            out.push(lc);
            prev_dash = false;
        } else if !prev_dash {
            out.push('-');
            prev_dash = true;
        }
    }
    let trimmed = out.trim_matches('-');
    if trimmed.is_empty() {
        "workflow".to_string()
    } else {
        trimmed.to_string()
    }
}

/// The directory a workflow of `scope` is saved into (oracle `uQ_(scope, cwd)`).
/// `User` → `<userConfigHome>/workflows`; `Project` → `<cwd>/.lingxi/workflows`.
/// (The oracle joins the git root / cwd with `.claude/workflows`; LingXi uses the
/// `.lingxi` dir name — the accepted branding divergence.) Falls back to the
/// project dir when the user config home can't be resolved.
#[must_use]
pub fn workflow_scope_dir(scope: WorkflowScope, cwd: &Path) -> PathBuf {
    match scope {
        WorkflowScope::User => user_config_home_dir()
            .map(|home| home.join("workflows"))
            .unwrap_or_else(|| cwd.join(branding::DOT_DIR).join("workflows")),
        WorkflowScope::Project => cwd.join(branding::DOT_DIR).join("workflows"),
    }
}

/// A successful [`save_dynamic_workflow`] — the oracle `eya` return
/// `{name, path, scope}` plus the `script_size_chars` telemetry field.
#[derive(Debug, Clone)]
pub struct WorkflowSaved {
    /// Sanitized workflow name (the `<name>` in `<name>.js`).
    pub name: String,
    /// Absolute path the script was written to.
    pub path: PathBuf,
    /// The scope it was saved under.
    pub scope: WorkflowScope,
    /// `script.length` (UTF-16 code units, matching JS) — the oracle
    /// `script_size_chars` telemetry field.
    pub script_size_chars: usize,
}

/// Error saving a dynamic workflow (oracle `eya` throw paths).
#[derive(Debug)]
pub enum WorkflowSaveError {
    /// The target file exists and `overwrite` was not set (oracle EEXIST →
    /// telemetry `"already_exists"`). Message byte-exact with the binary.
    AlreadyExists {
        /// Sanitized name that collided.
        name: String,
        /// The `<name>.js` path that already exists.
        path: PathBuf,
    },
    /// Any other I/O failure (mkdir / write) — oracle `"write_failed"`.
    Io(std::io::Error),
}

impl std::fmt::Display for WorkflowSaveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WorkflowSaveError::AlreadyExists { name, path } => write!(
                f,
                "Dynamic workflow \"{name}\" already exists at {}. Use a different name or overwrite.",
                path.display()
            ),
            WorkflowSaveError::Io(e) => write!(f, "{e}"),
        }
    }
}
impl std::error::Error for WorkflowSaveError {}

/// Save a dynamic workflow to disk (claude-code `eya`). Writes the sanitized
/// `<name>.js` under [`workflow_scope_dir`], creating the dir `0o700` and the
/// file `0o600`. The entire save is protected by a root-confined advisory lock
/// and a same-directory atomic replacement. No path component below the trusted
/// root may be a symlink/reparse point. Without `overwrite`, the final install
/// is an atomic no-clobber operation and yields
/// [`WorkflowSaveError::AlreadyExists`] on collision.
pub fn save_dynamic_workflow(
    name: &str,
    scope: WorkflowScope,
    script: &str,
    overwrite: bool,
    cwd: &Path,
) -> Result<WorkflowSaved, WorkflowSaveError> {
    let sanitized = sanitize_workflow_name(name);
    let dir = workflow_scope_dir(scope, cwd);
    let path = dir.join(format!("{sanitized}.js"));

    // Project saves are anchored at cwd so `.lingxi` itself cannot be swapped
    // for a symlink. User saves anchor one level above the configured home for
    // the same reason; a relative configured home remains cwd-relative.
    let (root, relative_dir) = match scope {
        WorkflowScope::Project => (
            cwd.to_path_buf(),
            PathBuf::from(branding::DOT_DIR).join("workflows"),
        ),
        WorkflowScope::User if dir.is_absolute() => {
            let config_home = dir.parent().unwrap_or(&dir);
            match (config_home.parent(), config_home.file_name()) {
                (Some(parent), Some(name)) => {
                    (parent.to_path_buf(), PathBuf::from(name).join("workflows"))
                }
                _ => (config_home.to_path_buf(), PathBuf::from("workflows")),
            }
        }
        WorkflowScope::User => (
            std::env::current_dir().unwrap_or_else(|_| cwd.to_path_buf()),
            dir.clone(),
        ),
    };
    let relative = relative_dir.join(format!("{sanitized}.js"));
    let lock_relative = relative_dir.join(".save.lock");
    let io_error =
        |error: traits::FsError| WorkflowSaveError::Io(std::io::Error::other(error.to_string()));
    let _lock = traits::rooted_fs::lock_exclusive(
        &root,
        &lock_relative,
        traits::rooted_fs::PRIVATE_DIR_MODE,
        traits::rooted_fs::PRIVATE_FILE_MODE,
    )
    .map_err(io_error)?;
    let options = traits::AtomicWriteOptions {
        overwrite,
        ..traits::AtomicWriteOptions::default()
    };
    if let Err(error) =
        traits::rooted_fs::atomic_write(&root, &relative, script.as_bytes(), options)
    {
        if matches!(error, traits::FsError::AlreadyExists(_)) {
            return Err(WorkflowSaveError::AlreadyExists {
                name: sanitized,
                path,
            });
        }
        return Err(io_error(error));
    }

    Ok(WorkflowSaved {
        name: sanitized,
        path,
        scope,
        // JS `.length` counts UTF-16 code units.
        script_size_chars: script.encode_utf16().count(),
    })
}

/// The success feedback string shown after a save (oracle `iNt` `lun(...)`),
/// byte-exact: `Dynamic workflow saved to <path>. Invoke as /<name> or
/// Workflow({name: "<name>"}) in future sessions.`
#[must_use]
pub fn saved_feedback(name: &str, path: &Path) -> String {
    format!(
        "Dynamic workflow saved to {}. Invoke as /{name} or Workflow({{name: \"{name}\"}}) in future sessions.",
        path.display()
    )
}

/// Seam that spawns a `LocalWorkflow` background task and returns its id. The
/// composition root wires this over the task registry (keeping this crate
/// decoupled from `tasks`); tests inject a mock.
#[async_trait]
pub trait WorkflowLauncher: Send + Sync {
    /// Launch the workflow described by `spec`, returning the new task id.
    async fn launch(
        &self,
        spec: WorkflowLaunchSpec,
    ) -> Result<WorkflowLaunched, WorkflowLaunchError>;
}

/// `WorkflowTool` — launch a workflow script as a background task.
#[derive(Clone)]
pub struct WorkflowTool {
    launcher: Option<Arc<dyn WorkflowLauncher>>,
    /// The session-frozen `workflowSizeGuideline` `/config` value. Binary
    /// `St().workflowSizeGuideline`, frozen for the session via `Jvd`'s cache;
    /// here the composition root reads the persisted setting once and hands it
    /// in, so the freeze is structural. Drives the [`Tool::prompt`] appendix
    /// (`qAs + VAs(size)`). Defaults to [`WorkflowSizeGuideline::Medium`]
    /// (the oracle's `_Td`) when unset — NOT `Unrestricted`.
    size_guideline: WorkflowSizeGuideline,
    /// MANAGED-settings `disableWorkflows` (binary `fbn()`'s second arm).
    /// Threaded at registration because `ToolStaticContext` carries only
    /// feature flags.
    managed_disable_workflows: bool,
}

impl WorkflowTool {
    /// Construct. `launcher` is `None` when the host has not wired the workflow
    /// task seam — the model-facing surface is still served, but `call` errors.
    /// The size guideline defaults to `unrestricted`; use
    /// [`Self::with_size_guideline`] to feed the persisted `/config` value.
    #[must_use]
    pub fn new(launcher: Option<Arc<dyn WorkflowLauncher>>) -> Self {
        Self {
            launcher,
            size_guideline: WorkflowSizeGuideline::default(),
            managed_disable_workflows: false,
        }
    }

    /// Set the session-frozen `workflowSizeGuideline` (binary
    /// `St().workflowSizeGuideline`). The composition root resolves the
    /// persisted `/config` value once at startup and passes it here; the value
    /// then flavors the [`Tool::prompt`] appendix for the whole session.
    #[must_use]
    pub fn with_size_guideline(mut self, size: WorkflowSizeGuideline) -> Self {
        self.size_guideline = size;
        self
    }

    /// Set the MANAGED-settings `disableWorkflows` gate (binary `fbn()`'s
    /// second arm, `$H()?.settings.disableWorkflows === true`).
    ///
    /// Threaded at registration like [`Self::with_size_guideline`] rather than
    /// through `ToolStaticContext`, which carries only feature flags. Without
    /// it an organization that set `disableWorkflows: true` still had the tool
    /// advertised and executable — the policy was parsed by nothing.
    #[must_use]
    pub fn with_disable_workflows(mut self, disabled: bool) -> Self {
        self.managed_disable_workflows = disabled;
        self
    }

    /// Is the tool disabled, by env var OR managed setting? Binary `fbn()`.
    fn workflows_disabled(&self) -> bool {
        self.managed_disable_workflows
            || is_env_truthy(std::env::var("LINGXI_DISABLE_WORKFLOWS").ok().as_deref())
    }

    fn spec_from_input(input: &Value) -> WorkflowLaunchSpec {
        let s = |k: &str| input.get(k).and_then(Value::as_str).map(str::to_string);
        WorkflowLaunchSpec {
            script: s("script"),
            name: s("name"),
            script_path: s("scriptPath"),
            args: input.get("args").cloned(),
            resume_from_run_id: s("resumeFromRunId"),
        }
    }

    /// List saved workflow names from project and user workflow directories.
    /// Returns a comma-joined string for the errorCode-1b message. Missing
    /// directories are ignored.
    fn list_available_workflow_names() -> Option<String> {
        let mut saw_dir = false;
        let mut names: Vec<String> = Vec::new();
        for dir in saved_workflow_dirs() {
            let Ok(entries) = std::fs::read_dir(dir) else {
                continue;
            };
            saw_dir = true;
            names.extend(entries.filter_map(|e| {
                let fname = e.ok()?.file_name();
                let fname = fname.to_string_lossy();
                // Strip known extensions to get the bare name.
                for ext in [".js", ".mjs", ".ts"] {
                    if let Some(stem) = fname.strip_suffix(ext) {
                        return Some(stem.to_string());
                    }
                }
                Some(fname.into_owned())
            }));
        }
        if !saw_dir {
            return None;
        }
        names.sort();
        names.dedup();
        Some(names.join(", "))
    }
}

#[async_trait]
impl Tool for WorkflowTool {
    fn name(&self) -> &str {
        TOOL_NAME
    }
    /// 2.1.206 tool-definition `searchHint` (byte-verified).
    fn search_hint(&self) -> Option<&str> {
        Some("orchestrate subagents with deterministic JavaScript workflow")
    }
    fn input_schema(&self) -> &Value {
        &INPUT_SCHEMA
    }
    fn is_enabled(&self, _: &ToolStaticContext) -> bool {
        // Port of `fbn()` + `pA()` from claude-code v2.1.186 (offset 196461282).
        // `fbn()` returns true (= disable) when:
        //   `isEnvTruthy(process.env.LINGXI_DISABLE_WORKFLOWS)` OR
        //   `$H()?.settings.disableWorkflows === true`
        //
        // BOTH arms are implemented. The managed-setting arm is threaded at
        // REGISTRATION (`with_disable_workflows`) rather than through
        // `ToolStaticContext`, which carries only feature flags — the seam that
        // previously blocked it. Before this, an organization setting
        // `disableWorkflows: true` still got the tool advertised and executable.
        //
        // The org/launch (`Xs("allow_workflows")`), GrowthBook
        // (`tengu_workflows_enabled`), and plan-availability gates have no LingXi
        // backing and are treated as permissive (enabled), matching the
        // Max/Team/null-plan default.
        !self.workflows_disabled()
    }
    fn max_result_size_chars(&self) -> usize {
        100000
    }
    fn is_concurrency_safe(&self, _: &Value) -> bool {
        // A workflow fans out many side-effecting agents.
        false
    }
    fn is_read_only(&self, _: &Value) -> bool {
        false
    }

    async fn check_permissions(&self, _: &Value, _: &ToolUseContext) -> PermissionResult {
        // The Workflow tool itself needs no permission gate — the subagents it
        // spawns are individually permissioned (claude-code surfaces no
        // `canUseTool` prompt for Workflow).
        PermissionResult::Allow {
            reason: PermissionDecisionReason::Other {
                reason: "Workflow launch — spawned agents are individually permissioned".into(),
            },
            updated_input: None,
            update_destination: None,
            metadata: PermissionMetadata::default(),
        }
    }

    async fn description(&self, _: &Value, _: &DescriptionOptions) -> String {
        "Running a workflow".into()
    }

    async fn prompt(&self, _: &PromptOptions) -> String {
        // Binary: `async prompt(){ return qAs + VAs(St().workflowSizeGuideline) }`
        // — the base description plus the (possibly-empty) size-guideline
        // appendix for the session-frozen `/config` value.
        format!("{}{}", *DESCRIPTION, self.size_guideline.prompt_appendix())
    }

    async fn validate_input(
        &self,
        input: &Value,
        _ctx: &ToolUseContext,
    ) -> Result<(), ValidationError> {
        // ── Schema-level `.refine()` on the `script` field (claude-code 2.1.195) ──
        // The zod input schema is `script: A.string().max(z$).refine(UIe, hYp)`,
        // where `UIe`/`WRa` reject any control char (code < 32 except 9/10, or
        // 127–159). In claude-code this runs at input-parse time, BEFORE the
        // validateInput gates below — so it is the first check here. It applies
        // ONLY to the raw inline `script` field (not scriptPath-read content nor
        // name-resolved scripts). Message byte-exact (`hYp`).
        if let Some(raw_script) = input.get("script").and_then(Value::as_str) {
            if raw_script.chars().any(|c| {
                let code = c as u32;
                if code == 9 || code == 10 {
                    false
                } else {
                    code < 32 || (127..=159).contains(&code)
                }
            }) {
                return Err(ValidationError(
                    "script contains control characters that would be hidden in the approval dialog"
                        .into(),
                ));
            }
        }

        // ── Gate order mirrors claude-code v2.1.186 validateInput (offset 203004507) ──
        //
        // errorCode 7 — abort / input truncated
        // ⚠️ UNREACHABLE: the binary's `yke(t.abortController.signal)` is the
        // HTTP-layer server-retraction signal; LingXi's `ToolUseContext` exposes a
        // local `cancel` (CancellationToken) that fires on sibling errors / user
        // interrupt — a different thing. No server-fallback abort signal is threaded
        // to `validate_input`. Kept as a named constant for documentation; the gate
        // is not wired.
        //
        // errorCode 5 — `disableWorkflows`: env var OR managed setting
        // (binary `fbn()`). Both arms reach here now; the managed value is
        // threaded at registration (`with_disable_workflows`).
        if self.workflows_disabled() {
            return Err(ValidationError(
                "Dynamic workflows are disabled by managed settings (`disableWorkflows`).".into(),
            ));
        }

        // errorCode 6 — session gate (`pA()`)
        // ⚠️ PARTIAL: the binary's `pA()` checks org policy, launch gate, and the
        // user's `/config` "Dynamic workflows" toggle. None of these sources are
        // threaded to validate_input in LingXi's ctx. The gate below is the local-
        // equivalent env-var path; the managed org/launch/config arms are NOT
        // reachable. In practice this gate is always permissive on local builds.
        // Message byte-exact per §8 errorCode 6.
        // (No additional local gate beyond the env-var above — pA() defaults
        // permissive on Max/Team/null-plan; only fires when explicitly disabled.)

        // errorCode 1 — script resolution (sub-errors 1a–1f, byte-exact per §8.1)
        // Reproduces the binary's D7a() resolution logic with exact error strings.
        let s = |k: &str| input.get(k).and_then(Value::as_str).map(str::to_string);
        let script_path = s("scriptPath").filter(|v| !v.is_empty());
        let script = s("script").filter(|v| !v.is_empty());
        let name = s("name").filter(|v| !v.is_empty());

        // Resolved script text (for errorCode 2 and 4 checks below).
        let resolved_script: String;

        if let Some(ref path) = script_path {
            // 1c — UNC path not allowed
            if path.starts_with("\\\\") {
                return Err(ValidationError(format!(
                    "UNC paths are not allowed for workflow scriptPath: {path}"
                )));
            }
            // 1d / 1e / 1f — file read / not found / too large
            match std::fs::read(path) {
                Ok(bytes) => {
                    if bytes.len() > MAX_SCRIPT_BYTES {
                        return Err(ValidationError(format!(
                            "Workflow script file {path} exceeds {MAX_SCRIPT_BYTES} bytes"
                        )));
                    }
                    resolved_script = String::from_utf8_lossy(&bytes).into_owned();
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    return Err(ValidationError(format!(
                        "Workflow script file not found: {path}"
                    )));
                }
                Err(e) => {
                    return Err(ValidationError(format!(
                        "Failed to read workflow script file {path}: {e}"
                    )));
                }
            }
        } else if let Some(ref inline) = script {
            resolved_script = inline.clone();
        } else if let Some(ref wf_name) = name {
            // Try to resolve from saved workflows (project first, then user).
            let mut found: Option<String> = None;
            for candidate in saved_workflow_candidates(wf_name) {
                match std::fs::read_to_string(&candidate) {
                    Ok(src) => {
                        found = Some(src);
                        break;
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                    Err(_) => continue,
                }
            }
            if let Some(src) = found {
                resolved_script = src;
            } else {
                // 1b — workflow name not found; list available names
                let available: String = Self::list_available_workflow_names().unwrap_or_default();
                let list = if available.is_empty() {
                    "(none)".to_string()
                } else {
                    available
                };
                return Err(ValidationError(format!(
                    "Workflow \"{wf_name}\" not found. Available: {list}"
                )));
            }
        } else {
            // 1a — none of script/name/scriptPath provided
            return Err(ValidationError(
                "Must provide script, name, or scriptPath".into(),
            ));
        }

        // errorCode 2 — parse/meta error (`Invalid workflow script: ${error}`)
        // Mirrors binary's `Bw(n.script)` → `validate_meta`.
        if let Err(e) = workflow::validate_meta(&resolved_script) {
            return Err(ValidationError(format!("Invalid workflow script: {e}")));
        }

        // errorCode 4 — determinism violation (inline script only)
        // Binary: `e.script && HKa(r.scriptBody)`. `e.script` is the RAW INLINE
        // `script` field from the input — it is falsy when `name` or `scriptPath`
        // is used. Only inline `script` input is checked; `name`-resolved saved
        // workflows and `scriptPath`-sourced files skip this gate.
        if script.is_some() {
            if let Err(e) = workflow::check_determinism(&resolved_script) {
                // The WorkflowError Display wraps the message; we want the raw
                // NON_DETERMINISTIC_MESSAGE, which lives inside WorkflowError::Script.
                use workflow::WorkflowError;
                let msg = match e {
                    WorkflowError::Script(m) => m,
                    WorkflowError::Engine(m) => m,
                };
                return Err(ValidationError(msg));
            }
        }

        // errorCode 3 — still-running resume target
        // The binary looks up a `local_workflow` task by `workflowRunId ===
        // resumeFromRunId` in the task registry. LingXi's `ToolUseContext` does
        // not carry a task-registry handle, so this gate is NOT enforced here;
        // instead it is enforced at LAUNCH time by `TaskRegistryWorkflowLauncher`
        // (the composition root, which owns the registry — see
        // `find_running_workflow_by_run_id`), which returns the byte-exact
        // "Workflow … is still running … Stop it first with TaskStop(…)" error.
        // Observable behaviour matches: a resume of a still-running workflow is
        // rejected before a second run starts.

        Ok(())
    }

    async fn call(
        &self,
        input: Value,
        _ctx: ToolUseContext,
        _progress: ToolProgressSender,
    ) -> Result<ToolCallResult, ToolError> {
        let launcher = self.launcher.as_ref().ok_or_else(|| {
            ToolError::Internal("Workflow launching is not available in this host".into())
        })?;
        let spec = Self::spec_from_input(&input);
        let launched = launcher
            .launch(spec)
            .await
            .map_err(|e| ToolError::Internal(e.to_string()))?;
        // claude-code result shape (output schema `sUp`, local path): status,
        // taskId, taskType, plus the optional workflowName / runId / scriptPath /
        // summary / transcriptDir. The "remote_launched"/"remote_agent" + sessionUrl
        // variants are the CCR/remote path, out of scope for the single-process
        // build. Optional fields are omitted when the host did not supply them.
        //
        // The model-facing launch text mirrors `mapToolResultToToolResultBlockParam`
        // (binary §1 verbatim, async_launched case):
        //   "Workflow launched in background. Task ID: {taskId}"
        //   + optional "\nSummary: {summary}"
        //   + optional "\nTranscript dir: {transcriptDir}"
        //   + optional "\nScript file: …\n(Edit …)"
        //   + optional "\nRun ID: …\nTo resume …"
        //   + "\n\nYou will be notified when it completes. Use /workflows to watch live progress."
        let task_id = launched.task_id.clone();
        let summary = launched.summary.as_deref();
        let transcript = launched.transcript_dir.as_deref();
        let script_p = launched.script_path.as_deref();
        let run_id_str = launched.run_id.as_deref();

        let n = summary.map_or_else(String::new, |s| format!("\nSummary: {s}"));
        let r = transcript.map_or_else(String::new, |t| format!("\nTranscript dir: {t}"));
        let o = script_p.map_or_else(String::new, |p| {
            format!(
                "\nScript file: {p}\n(Edit this file with Write/Edit and re-invoke Workflow with \
                 {{scriptPath: \"{p}\"}} to iterate without resending the script.)"
            )
        });
        let s = match (script_p, run_id_str) {
            (Some(p), Some(rid)) => format!(
                "\nRun ID: {rid}\nTo resume after editing the script: \
                 Workflow({{scriptPath: \"{p}\", resumeFromRunId: \"{rid}\"}}) \
                 — completed agents return cached results."
            ),
            _ => String::new(),
        };
        let model_content = format!(
            "Workflow launched in background. Task ID: {task_id}{n}{r}{o}{s}\
             \n\nYou will be notified when it completes. Use /workflows to watch live progress."
        );

        let mut data = json!({
            "status": "async_launched",
            "taskId": task_id,
            "taskType": "local_workflow",
            "model_content": model_content,
        });
        let obj = data.as_object_mut().expect("json object");
        if let Some(name) = launched.workflow_name {
            obj.insert("workflowName".into(), Value::String(name));
        }
        if let Some(rid) = launched.run_id {
            obj.insert("runId".into(), Value::String(rid));
        }
        if let Some(path) = launched.script_path {
            obj.insert("scriptPath".into(), Value::String(path));
        }
        if let Some(sum) = launched.summary {
            obj.insert("summary".into(), Value::String(sum));
        }
        if let Some(td) = launched.transcript_dir {
            obj.insert("transcriptDir".into(), Value::String(td));
        }
        Ok(ToolCallResult {
            data,
            model_content: None,
            new_messages: vec![],
            context_modifier: None,
            is_error: false,
            mcp_meta: None,
        })
    }
}

/// Register the `Workflow` tool against `reg`. Without a wired launcher the
/// model-facing surface is served but `call` errors; the composition root
/// constructs [`WorkflowTool::new(Some(launcher))`] directly once the workflow
/// task seam is available.
pub fn register_all(reg: &mut tool_api::ToolRegistry, _ctx: tool_api::BuiltinToolContext) {
    reg.register_builtin(Arc::new(WorkflowTool::new(None)));
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex as StdMutex;

    struct MockLauncher {
        task_id: String,
        seen: StdMutex<Option<WorkflowLaunchSpec>>,
    }
    impl MockLauncher {
        fn new(task_id: &str) -> Arc<Self> {
            Arc::new(Self {
                task_id: task_id.to_string(),
                seen: StdMutex::new(None),
            })
        }
    }
    #[async_trait]
    impl WorkflowLauncher for MockLauncher {
        async fn launch(
            &self,
            spec: WorkflowLaunchSpec,
        ) -> Result<WorkflowLaunched, WorkflowLaunchError> {
            *self.seen.lock().unwrap() = Some(spec);
            Ok(WorkflowLaunched {
                task_id: self.task_id.clone(),
                ..Default::default()
            })
        }
    }

    fn tool(launcher: Option<Arc<dyn WorkflowLauncher>>) -> WorkflowTool {
        WorkflowTool::new(launcher)
    }

    fn unique_temp_path(label: &str) -> std::path::PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "lingxi-workflow-{label}-{}-{nanos}",
            std::process::id()
        ))
    }

    fn with_config_dir_env<T>(path: &std::path::Path, f: impl FnOnce() -> T) -> T {
        let old = std::env::var_os(branding::CONFIG_DIR_ENV);
        std::env::set_var(branding::CONFIG_DIR_ENV, path);
        let out = f();
        match old {
            Some(v) => std::env::set_var(branding::CONFIG_DIR_ENV, v),
            None => std::env::remove_var(branding::CONFIG_DIR_ENV),
        }
        out
    }

    #[test]
    fn name_is_workflow() {
        assert_eq!(tool(None).name(), "Workflow");
    }

    #[test]
    fn max_result_size_is_100000() {
        assert_eq!(tool(None).max_result_size_chars(), 100000);
    }

    #[test]
    fn resolve_script_precedence_and_name_lookup() {
        use std::collections::HashMap;
        let files: HashMap<&str, &str> = HashMap::from([
            ("/abs/wf.js", "FROM_PATH"),
            (".lingxi/workflows/review.js", "FROM_NAME"),
        ]);
        let read = |p: &str| {
            files
                .get(p)
                .map(|s| (*s).to_string())
                .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "nope"))
        };

        // scriptPath wins over an inline script.
        let spec = WorkflowLaunchSpec {
            script: Some("INLINE".into()),
            script_path: Some("/abs/wf.js".into()),
            ..Default::default()
        };
        assert_eq!(resolve_script(&spec, &read).unwrap(), "FROM_PATH");

        // inline script when there is no scriptPath.
        let spec = WorkflowLaunchSpec {
            script: Some("INLINE".into()),
            ..Default::default()
        };
        assert_eq!(resolve_script(&spec, &read).unwrap(), "INLINE");

        // name → .lingxi/workflows/<name>.js.
        let spec = WorkflowLaunchSpec {
            name: Some("review".into()),
            ..Default::default()
        };
        assert_eq!(resolve_script(&spec, &read).unwrap(), "FROM_NAME");

        // unknown name + nothing-provided → errors.
        let spec = WorkflowLaunchSpec {
            name: Some("missing".into()),
            ..Default::default()
        };
        assert!(resolve_script(&spec, &read).is_err());
        assert!(resolve_script(&WorkflowLaunchSpec::default(), &read).is_err());
    }

    #[test]
    fn resolve_script_name_falls_back_to_user_workflows_dir() {
        use std::collections::HashMap;
        let _g = ENV_LOCK.lock().unwrap();
        let config_dir = unique_temp_path("resolve-user");
        let user_workflow = config_dir.join("workflows").join("review.mjs");
        let mut files: HashMap<String, String> = HashMap::new();
        files.insert(
            user_workflow.to_string_lossy().into_owned(),
            "FROM_USER".into(),
        );
        let read = |p: &str| {
            files
                .get(p)
                .cloned()
                .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "nope"))
        };

        let got = with_config_dir_env(&config_dir, || {
            resolve_script(
                &WorkflowLaunchSpec {
                    name: Some("review".into()),
                    ..Default::default()
                },
                &read,
            )
        })
        .unwrap();

        assert_eq!(got, "FROM_USER");
    }

    #[test]
    fn description_matches_the_binary_byte_for_byte() {
        // v2.1.185 runtime length of the Workflow tool description.
        assert_eq!(DESCRIPTION.len(), 18961, "description byte length drifted");
        assert!(DESCRIPTION.starts_with(
            "Execute a workflow script that orchestrates multiple subagents deterministically."
        ));
        assert!(DESCRIPTION.ends_with("hand-author a continuation script."));
        // The ${r1e} interpolation resolved to the ▸ group marker.
        assert!(DESCRIPTION.contains("\"▸ name\" group in /workflows"));
        // No leftover raw escape sequences.
        assert!(!DESCRIPTION.contains("\\u2014"));
    }

    /// Managed `disableWorkflows: true` must disable the tool. Before this was
    /// wired there was no seam for the setting at all, so an organization that
    /// set it still had Workflow advertised AND executable — the policy was
    /// parsed by nothing.
    #[test]
    fn managed_disable_workflows_disables_the_tool() {
        let ctx = ToolStaticContext::default();
        assert!(
            WorkflowTool::new(None).is_enabled(&ctx),
            "enabled by default"
        );
        assert!(
            !WorkflowTool::new(None)
                .with_disable_workflows(true)
                .is_enabled(&ctx),
            "managed disableWorkflows must disable it"
        );
    }

    #[tokio::test]
    async fn managed_disable_workflows_also_rejects_at_validate() {
        // `is_enabled` hides the tool; a caller that invokes it anyway must
        // still be refused, with the byte-exact message.
        let t = WorkflowTool::new(None).with_disable_workflows(true);
        let ctx = tool_api::test_support::fresh_ctx();
        // The env arm must be OFF so this proves the MANAGED arm fired.
        let _g = ENV_LOCK.lock().unwrap();
        std::env::remove_var("LINGXI_DISABLE_WORKFLOWS");
        let err = t
            .validate_input(&serde_json::json!({"script": "x"}), &ctx)
            .await
            .expect_err("must reject");
        assert_eq!(
            err.0,
            "Dynamic workflows are disabled by managed settings (`disableWorkflows`)."
        );
    }

    #[test]
    fn the_default_size_guideline_is_medium_not_unrestricted() {
        assert_eq!(
            WorkflowSizeGuideline::default(),
            WorkflowSizeGuideline::Medium
        );
    }

    #[tokio::test]
    async fn prompt_appends_size_guideline_when_configured() {
        let opts = PromptOptions::default();
        // The DEFAULT is `medium` (oracle `_Td`), not unrestricted, so an
        // unconfigured tool already carries the medium appendix. Shipping
        // `Unrestricted` by default meant shipping NO agent cap where the
        // oracle advises under 15.
        assert_eq!(
            tool(None).prompt(&opts).await,
            format!(
                "{}{}",
                *DESCRIPTION,
                WorkflowSizeGuideline::Medium.prompt_appendix()
            )
        );
        // Each configured size appends its byte-exact `VAs` appendix
        // (`qAs + VAs(size)`), with NO separator between description and appendix
        // (the appendix carries its own leading newline).
        for size in [
            WorkflowSizeGuideline::Small,
            WorkflowSizeGuideline::Medium,
            WorkflowSizeGuideline::Large,
        ] {
            let t = WorkflowTool::new(None).with_size_guideline(size);
            let want = format!("{}{}", *DESCRIPTION, size.prompt_appendix());
            assert_eq!(t.prompt(&opts).await, want, "size={:?}", size);
            // Sanity: the model-visible cap text is present.
            assert!(t
                .prompt(&opts)
                .await
                .contains("The user has configured a workflow size guideline in /config:"));
        }
        // Explicit unrestricted also yields no appendix.
        let u = WorkflowTool::new(None).with_size_guideline(WorkflowSizeGuideline::Unrestricted);
        assert_eq!(u.prompt(&opts).await, *DESCRIPTION);
    }

    #[test]
    fn input_schema_is_byte_exact() {
        let s = &*INPUT_SCHEMA;
        assert_eq!(s["type"], "object");
        assert_eq!(s["additionalProperties"], false);
        assert!(s.get("required").is_none(), "no required keys");
        // Source-order properties (the zod definition order).
        let props = s["properties"].as_object().unwrap();
        let order: Vec<&String> = props.keys().collect();
        assert_eq!(
            order,
            vec![
                "script",
                "name",
                "description",
                "title",
                "args",
                "scriptPath",
                "resumeFromRunId"
            ]
        );
        assert_eq!(props["script"]["maxLength"], 524288);
        assert_eq!(props["resumeFromRunId"]["pattern"], "^wf_[a-z0-9-]{6,}$");
        // E.unknown() → `args` has no `type` constraint.
        assert!(props["args"].get("type").is_none());
        assert!(props["script"]["description"]
            .as_str()
            .unwrap()
            .starts_with("Self-contained workflow script."));
    }

    // A minimal valid workflow script: has a proper meta block, is deterministic.
    const VALID_SCRIPT: &str = concat!(
        "export const meta = { name: 'test', description: 'A test workflow' };\n",
        "await agent('do something');\n",
    );

    #[tokio::test]
    async fn validate_error_1a_must_provide_one_of_three_fields() {
        // errorCode 1a — none of script/name/scriptPath provided.
        let t = tool(None);
        let ctx = tool_api::test_support::fresh_ctx();
        let _g = ENV_LOCK.lock().unwrap();
        std::env::remove_var("LINGXI_DISABLE_WORKFLOWS");

        let err = t.validate_input(&json!({}), &ctx).await.unwrap_err();
        assert_eq!(err.0, "Must provide script, name, or scriptPath");

        let err2 = t
            .validate_input(&json!({ "title": "x" }), &ctx)
            .await
            .unwrap_err();
        assert_eq!(err2.0, "Must provide script, name, or scriptPath");
    }

    #[tokio::test]
    async fn validate_error_2_invalid_workflow_script() {
        // errorCode 2 — script is present but fails meta parse.
        let t = tool(None);
        let ctx = tool_api::test_support::fresh_ctx();
        let _g = ENV_LOCK.lock().unwrap();
        std::env::remove_var("LINGXI_DISABLE_WORKFLOWS");

        // A script with no meta block at all triggers the "must be first statement" error.
        let err = t
            .validate_input(&json!({ "script": "console.log('hello');" }), &ctx)
            .await
            .unwrap_err();
        assert!(
            err.0.starts_with("Invalid workflow script:"),
            "expected errorCode 2 message, got: {:?}",
            err.0
        );
    }

    #[tokio::test]
    async fn validate_error_4_date_now_determinism() {
        // errorCode 4 — inline script uses Date.now().
        let t = tool(None);
        let ctx = tool_api::test_support::fresh_ctx();
        let _g = ENV_LOCK.lock().unwrap();
        std::env::remove_var("LINGXI_DISABLE_WORKFLOWS");

        let script = concat!(
            "export const meta = { name: 'bad', description: 'non-det' };\n",
            "const t = Date.now();\n",
        );
        let err = t
            .validate_input(&json!({ "script": script }), &ctx)
            .await
            .unwrap_err();
        assert_eq!(
            err.0,
            workflow::NON_DETERMINISTIC_MESSAGE,
            "errorCode 4 message must be byte-exact"
        );
    }

    #[tokio::test]
    async fn validate_name_resolved_date_now_passes_determinism_gate() {
        // Binary parity: errorCode-4 is NOT triggered for `name`-resolved saved
        // workflows even when the resolved body contains Date.now(). `e.script`
        // (the raw inline field) is falsy, so the binary skips the determinism
        // check. LingXi must do the same.
        let t = tool(None);
        let ctx = tool_api::test_support::fresh_ctx();
        let _g = ENV_LOCK.lock().unwrap();
        std::env::remove_var("LINGXI_DISABLE_WORKFLOWS");

        // Write a non-deterministic saved workflow to .lingxi/workflows/.
        let dir = std::path::Path::new(".lingxi/workflows");
        std::fs::create_dir_all(dir).unwrap();
        let wf_path = dir.join("nondet-wf.js");
        let nondeterministic_src = concat!(
            "export const meta = { name: 'nondet-wf', description: 'non-det saved' };\n",
            "const t = Date.now();\n",
            "await agent('do something');\n",
        );
        std::fs::write(&wf_path, nondeterministic_src).unwrap();

        let result = t
            .validate_input(&json!({ "name": "nondet-wf" }), &ctx)
            .await;

        // Clean up before asserting so we don't leave stray files.
        let _ = std::fs::remove_file(&wf_path);

        result
            .expect("name-resolved workflow with Date.now() must NOT be rejected for determinism");
    }

    #[tokio::test]
    async fn validate_name_resolves_user_workflow_dir() {
        let t = tool(None);
        let ctx = tool_api::test_support::fresh_ctx();
        let _g = ENV_LOCK.lock().unwrap();
        std::env::remove_var("LINGXI_DISABLE_WORKFLOWS");
        let config_dir = unique_temp_path("validate-user");
        let workflows_dir = config_dir.join("workflows");
        std::fs::create_dir_all(&workflows_dir).unwrap();
        let wf_path = workflows_dir.join("user-only-wf.js");
        std::fs::write(&wf_path, VALID_SCRIPT).unwrap();

        let old_config_dir = std::env::var_os(branding::CONFIG_DIR_ENV);
        std::env::set_var(branding::CONFIG_DIR_ENV, &config_dir);
        let result = t
            .validate_input(&json!({ "name": "user-only-wf" }), &ctx)
            .await;
        match old_config_dir {
            Some(v) => std::env::set_var(branding::CONFIG_DIR_ENV, v),
            None => std::env::remove_var(branding::CONFIG_DIR_ENV),
        }

        let _ = std::fs::remove_file(&wf_path);
        let _ = std::fs::remove_dir_all(&config_dir);
        result.expect("name-resolved workflow must fall back to user workflow dir");
    }

    #[tokio::test]
    async fn validate_error_5_disable_workflows_env() {
        // errorCode 5 — LINGXI_DISABLE_WORKFLOWS=1 fires the managed-settings message.
        let t = tool(None);
        let ctx = tool_api::test_support::fresh_ctx();
        let _g = ENV_LOCK.lock().unwrap();
        std::env::set_var("LINGXI_DISABLE_WORKFLOWS", "1");
        let result = t
            .validate_input(&json!({ "script": VALID_SCRIPT }), &ctx)
            .await;
        std::env::remove_var("LINGXI_DISABLE_WORKFLOWS");

        let err = result.unwrap_err();
        assert_eq!(
            err.0, "Dynamic workflows are disabled by managed settings (`disableWorkflows`).",
            "errorCode 5 message must be byte-exact"
        );
    }

    #[tokio::test]
    async fn validate_script_with_control_char_rejected() {
        // Schema `.refine(UIe, hYp)` — a control char (here NUL) in the raw inline
        // `script` field is rejected byte-exact before any gate runs.
        let t = tool(None);
        let ctx = tool_api::test_support::fresh_ctx();
        let _g = ENV_LOCK.lock().unwrap();
        std::env::remove_var("LINGXI_DISABLE_WORKFLOWS");

        let with_ctrl = format!("{VALID_SCRIPT}\u{0}");
        let err = t
            .validate_input(&json!({ "script": with_ctrl }), &ctx)
            .await
            .unwrap_err();
        assert_eq!(
            err.0, "script contains control characters that would be hidden in the approval dialog",
            "control-char refine message must be byte-exact"
        );
    }

    #[tokio::test]
    async fn validate_script_with_tab_and_newline_allowed() {
        // WRa excludes char codes 9 (tab) and 10 (LF) — a script with those passes.
        let t = tool(None);
        let ctx = tool_api::test_support::fresh_ctx();
        let _g = ENV_LOCK.lock().unwrap();
        std::env::remove_var("LINGXI_DISABLE_WORKFLOWS");

        let with_ws = format!("{VALID_SCRIPT}\n\tlog('ok')\n");
        t.validate_input(&json!({ "script": with_ws }), &ctx)
            .await
            .expect("tab/newline must not trip the control-char refine");
    }

    #[tokio::test]
    async fn validate_valid_script_passes() {
        // A valid script with proper meta + deterministic code → Ok(()).
        let t = tool(None);
        let ctx = tool_api::test_support::fresh_ctx();
        let _g = ENV_LOCK.lock().unwrap();
        std::env::remove_var("LINGXI_DISABLE_WORKFLOWS");

        t.validate_input(&json!({ "script": VALID_SCRIPT }), &ctx)
            .await
            .expect("valid script must pass all gates");
    }

    #[tokio::test]
    async fn call_launches_and_returns_async_launched() {
        let launcher = MockLauncher::new("w_abc123");
        let t = tool(Some(launcher.clone()));
        let res = t
            .call(
                json!({ "script": "return 1;", "args": ["a.ts", "b.ts"] }),
                tool_api::test_support::fresh_ctx(),
                tool_api::test_support::fresh_tx(),
            )
            .await
            .expect("call ok");
        assert_eq!(res.data["status"], "async_launched");
        assert_eq!(res.data["taskId"], "w_abc123");
        assert_eq!(res.data["taskType"], "local_workflow");
        // The launcher saw the parsed spec (script + args).
        let spec = launcher.seen.lock().unwrap().clone().expect("launched");
        assert_eq!(spec.script.as_deref(), Some("return 1;"));
        assert_eq!(spec.args, Some(json!(["a.ts", "b.ts"])));
    }

    #[tokio::test]
    async fn call_without_a_launcher_errors() {
        let t = tool(None);
        let err = t
            .call(
                json!({ "script": "return 1;" }),
                tool_api::test_support::fresh_ctx(),
                tool_api::test_support::fresh_tx(),
            )
            .await
            .unwrap_err();
        assert!(matches!(err, ToolError::Internal(_)));
    }

    // ── Task 5: summary + transcriptDir + byte-exact launch text ─────────────

    /// A launcher that returns a full `WorkflowLaunched` with all optional
    /// fields set, for testing the result JSON shape and model_content text.
    struct RichMockLauncher {
        launched: WorkflowLaunched,
    }
    #[async_trait]
    impl WorkflowLauncher for RichMockLauncher {
        async fn launch(
            &self,
            _spec: WorkflowLaunchSpec,
        ) -> Result<WorkflowLaunched, WorkflowLaunchError> {
            Ok(self.launched.clone())
        }
    }

    /// call() with summary + transcriptDir → result JSON has both fields and the
    /// model-facing text contains the `Summary:` and `Transcript dir:` lines.
    #[tokio::test]
    async fn call_with_summary_and_transcript_dir_in_result() {
        let launcher = Arc::new(RichMockLauncher {
            launched: WorkflowLaunched {
                task_id: "w_t1".into(),
                run_id: Some("wf_abc123def456".into()),
                script_path: Some("/tmp/wf_abc123def456.js".into()),
                workflow_name: Some("my-wf".into()),
                summary: Some("A test workflow".into()),
                transcript_dir: Some(
                    "/home/.lingxi/projects/-Users-me-proj/sess123/subagents/workflows/wf_abc123def456".into()
                ),
            },
        });
        let t = tool(Some(launcher));
        let res = t
            .call(
                json!({ "script": "return 1;" }),
                tool_api::test_support::fresh_ctx(),
                tool_api::test_support::fresh_tx(),
            )
            .await
            .expect("call ok");

        // JSON data shape
        assert_eq!(res.data["status"], "async_launched");
        assert_eq!(res.data["taskId"], "w_t1");
        assert_eq!(res.data["taskType"], "local_workflow");
        assert_eq!(res.data["summary"], "A test workflow");
        assert_eq!(
            res.data["transcriptDir"],
            "/home/.lingxi/projects/-Users-me-proj/sess123/subagents/workflows/wf_abc123def456"
        );
        assert_eq!(res.data["runId"], "wf_abc123def456");
        assert_eq!(res.data["scriptPath"], "/tmp/wf_abc123def456.js");
        assert_eq!(res.data["workflowName"], "my-wf");

        // model_content text — byte-exact per oracle §1 async_launched template
        let mc = res.data["model_content"]
            .as_str()
            .expect("model_content string");
        assert!(
            mc.starts_with("Workflow launched in background. Task ID: w_t1"),
            "must start with task id header: {mc}"
        );
        assert!(
            mc.contains("\nSummary: A test workflow"),
            "must have Summary line: {mc}"
        );
        assert!(
            mc.contains("\nTranscript dir: /home/.lingxi/projects/-Users-me-proj/sess123/subagents/workflows/wf_abc123def456"),
            "must have Transcript dir line: {mc}"
        );
        assert!(
            mc.contains("\nScript file: /tmp/wf_abc123def456.js\n(Edit this file with Write/Edit"),
            "must have Script file line: {mc}"
        );
        assert!(
            mc.contains("\nRun ID: wf_abc123def456\nTo resume after editing the script:"),
            "must have Run ID line: {mc}"
        );
        assert!(
            mc.ends_with("\n\nYou will be notified when it completes. Use /workflows to watch live progress."),
            "must end with footer: {mc}"
        );
    }

    /// call() with no summary/transcriptDir → those fields are absent from JSON and
    /// the model_content text has no `Summary:` or `Transcript dir:` lines.
    #[tokio::test]
    async fn call_without_optional_fields_omitted_from_result() {
        let launcher = Arc::new(RichMockLauncher {
            launched: WorkflowLaunched {
                task_id: "w_t2".into(),
                run_id: None,
                script_path: None,
                workflow_name: None,
                summary: None,
                transcript_dir: None,
            },
        });
        let t = tool(Some(launcher));
        let res = t
            .call(
                json!({ "script": "return 1;" }),
                tool_api::test_support::fresh_ctx(),
                tool_api::test_support::fresh_tx(),
            )
            .await
            .expect("call ok");

        // Optional fields absent from JSON
        assert!(res.data.get("summary").is_none(), "summary must be absent");
        assert!(
            res.data.get("transcriptDir").is_none(),
            "transcriptDir must be absent"
        );
        assert!(
            res.data.get("runId").is_none(),
            "runId must be absent when None"
        );
        assert!(
            res.data.get("scriptPath").is_none(),
            "scriptPath must be absent when None"
        );

        // model_content has no conditional lines
        let mc = res.data["model_content"]
            .as_str()
            .expect("model_content string");
        assert!(
            !mc.contains("Summary:"),
            "no Summary line when absent: {mc}"
        );
        assert!(
            !mc.contains("Transcript dir:"),
            "no Transcript dir line when absent: {mc}"
        );
        assert!(
            !mc.contains("Script file:"),
            "no Script file line when absent: {mc}"
        );
        assert!(!mc.contains("Run ID:"), "no Run ID line when absent: {mc}");
        // Footer always present
        assert!(
            mc.ends_with("\n\nYou will be notified when it completes. Use /workflows to watch live progress."),
            "footer always present: {mc}"
        );
    }

    /// Byte-exact full launch text — verifies the entire model_content string
    /// against the §1 oracle template with all conditional lines present.
    #[tokio::test]
    async fn launch_text_byte_exact_full_template() {
        let launcher = Arc::new(RichMockLauncher {
            launched: WorkflowLaunched {
                task_id: "w_TASKID".into(),
                run_id: Some("wf_RUNID".into()),
                script_path: Some("/path/to/script.js".into()),
                workflow_name: Some("wf-name".into()),
                summary: Some("My workflow summary".into()),
                transcript_dir: Some("/tmp/transcripts/subagents/workflows/wf_RUNID".into()),
            },
        });
        let t = tool(Some(launcher));
        let res = t
            .call(
                json!({ "script": "return 1;" }),
                tool_api::test_support::fresh_ctx(),
                tool_api::test_support::fresh_tx(),
            )
            .await
            .expect("call ok");

        let mc = res.data["model_content"]
            .as_str()
            .expect("model_content string");

        // Verify the EXACT string per §1 oracle template
        let expected = concat!(
            "Workflow launched in background. Task ID: w_TASKID",
            "\nSummary: My workflow summary",
            "\nTranscript dir: /tmp/transcripts/subagents/workflows/wf_RUNID",
            "\nScript file: /path/to/script.js",
            "\n(Edit this file with Write/Edit and re-invoke Workflow with {scriptPath: \"/path/to/script.js\"} to iterate without resending the script.)",
            "\nRun ID: wf_RUNID",
            "\nTo resume after editing the script: Workflow({scriptPath: \"/path/to/script.js\", resumeFromRunId: \"wf_RUNID\"}) — completed agents return cached results.",
            "\n\nYou will be notified when it completes. Use /workflows to watch live progress.",
        );
        assert_eq!(mc, expected, "launch text must be byte-exact per §1 oracle");
    }

    // is_enabled gate tests — port of `fbn()` / `pA()` local-deterministic subset.
    //
    // Env vars are process-global; all tests that touch LINGXI_DISABLE_WORKFLOWS
    // must hold ENV_LOCK so they don't race with each other.
    use std::sync::Mutex;
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    /// Default: no env var set → tool is enabled.
    #[test]
    fn is_enabled_default() {
        let _g = ENV_LOCK.lock().unwrap();
        std::env::remove_var("LINGXI_DISABLE_WORKFLOWS");
        let t = tool(None);
        assert!(
            t.is_enabled(&ToolStaticContext::default()),
            "Workflow must be enabled by default"
        );
    }

    /// LINGXI_DISABLE_WORKFLOWS=1 → tool is disabled.
    #[test]
    fn is_enabled_disabled_by_env_1() {
        let _g = ENV_LOCK.lock().unwrap();
        std::env::set_var("LINGXI_DISABLE_WORKFLOWS", "1");
        let t = tool(None);
        let enabled = t.is_enabled(&ToolStaticContext::default());
        std::env::remove_var("LINGXI_DISABLE_WORKFLOWS");
        assert!(!enabled, "Workflow must be disabled when env var is '1'");
    }

    /// LINGXI_DISABLE_WORKFLOWS=true → tool is disabled.
    #[test]
    fn is_enabled_disabled_by_env_true() {
        let _g = ENV_LOCK.lock().unwrap();
        std::env::set_var("LINGXI_DISABLE_WORKFLOWS", "true");
        let t = tool(None);
        let enabled = t.is_enabled(&ToolStaticContext::default());
        std::env::remove_var("LINGXI_DISABLE_WORKFLOWS");
        assert!(!enabled, "Workflow must be disabled when env var is 'true'");
    }

    /// LINGXI_DISABLE_WORKFLOWS=yes → tool is disabled.
    #[test]
    fn is_enabled_disabled_by_env_yes() {
        let _g = ENV_LOCK.lock().unwrap();
        std::env::set_var("LINGXI_DISABLE_WORKFLOWS", "yes");
        let t = tool(None);
        let enabled = t.is_enabled(&ToolStaticContext::default());
        std::env::remove_var("LINGXI_DISABLE_WORKFLOWS");
        assert!(!enabled, "Workflow must be disabled when env var is 'yes'");
    }

    /// LINGXI_DISABLE_WORKFLOWS=0 (falsy) → tool remains enabled.
    #[test]
    fn is_enabled_falsy_env_value_stays_enabled() {
        let _g = ENV_LOCK.lock().unwrap();
        std::env::set_var("LINGXI_DISABLE_WORKFLOWS", "0");
        let t = tool(None);
        let enabled = t.is_enabled(&ToolStaticContext::default());
        std::env::remove_var("LINGXI_DISABLE_WORKFLOWS");
        assert!(
            enabled,
            "Workflow must stay enabled when env var is '0' (falsy)"
        );
    }

    // ── Save dynamic workflow (`eya`/`uQ_`/`lme`) ─────────────────────────────

    #[test]
    fn sanitize_workflow_name_is_kebab_with_fallback() {
        // Oracle `lme`: lowercase, collapse non-alnum runs to '-', trim, fallback.
        assert_eq!(
            sanitize_workflow_name("My Cool Workflow"),
            "my-cool-workflow"
        );
        assert_eq!(sanitize_workflow_name("  Deploy!! Site  "), "deploy-site");
        assert_eq!(sanitize_workflow_name("a__b--c"), "a-b-c");
        assert_eq!(sanitize_workflow_name("Review123"), "review123");
        assert_eq!(sanitize_workflow_name("***"), "workflow");
        assert_eq!(sanitize_workflow_name(""), "workflow");
    }

    #[test]
    fn scope_helpers_toggle_and_label() {
        assert_eq!(WorkflowScope::Project.wire(), "project");
        assert_eq!(WorkflowScope::User.wire(), "user");
        assert_eq!(WorkflowScope::Project.label(), "Project");
        assert_eq!(WorkflowScope::User.label(), "User");
        assert_eq!(WorkflowScope::Project.toggled(), WorkflowScope::User);
        assert_eq!(WorkflowScope::User.toggled(), WorkflowScope::Project);
    }

    #[test]
    fn scope_dir_project_is_cwd_dotdir_workflows() {
        let cwd = std::path::Path::new("/proj/root");
        let dir = workflow_scope_dir(WorkflowScope::Project, cwd);
        assert_eq!(dir, cwd.join(branding::DOT_DIR).join("workflows"));
    }

    #[test]
    fn scope_dir_user_uses_config_home() {
        let _g = ENV_LOCK.lock().unwrap();
        let config_dir = unique_temp_path("scope-user");
        let dir = with_config_dir_env(&config_dir, || {
            workflow_scope_dir(WorkflowScope::User, std::path::Path::new("/anything"))
        });
        assert_eq!(dir, config_dir.join("workflows"));
    }

    #[test]
    fn save_writes_file_and_reports_path_and_size() {
        let cwd = unique_temp_path("save-project");
        std::fs::create_dir_all(&cwd).unwrap();
        let script = "export const meta = { name: 'x' };\n";
        let saved =
            save_dynamic_workflow("My WF", WorkflowScope::Project, script, false, &cwd).unwrap();
        assert_eq!(saved.name, "my-wf");
        assert_eq!(
            saved.path,
            cwd.join(branding::DOT_DIR)
                .join("workflows")
                .join("my-wf.js")
        );
        assert_eq!(saved.scope, WorkflowScope::Project);
        assert_eq!(saved.script_size_chars, script.encode_utf16().count());
        assert_eq!(std::fs::read_to_string(&saved.path).unwrap(), script);
        let _ = std::fs::remove_dir_all(&cwd);
    }

    #[cfg(unix)]
    #[test]
    fn save_sets_600_file_and_700_dir_modes() {
        use std::os::unix::fs::PermissionsExt;
        let cwd = unique_temp_path("save-modes");
        std::fs::create_dir_all(&cwd).unwrap();
        let saved =
            save_dynamic_workflow("perm", WorkflowScope::Project, "x", false, &cwd).unwrap();
        let file_mode = std::fs::metadata(&saved.path).unwrap().permissions().mode() & 0o777;
        assert_eq!(file_mode, 0o600, "file mode must be 0o600");
        let dir_mode = std::fs::metadata(saved.path.parent().unwrap())
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(dir_mode, 0o700, "dir mode must be 0o700");
        let _ = std::fs::remove_dir_all(&cwd);
    }

    #[test]
    fn save_without_overwrite_rejects_existing_file() {
        let cwd = unique_temp_path("save-eexist");
        std::fs::create_dir_all(&cwd).unwrap();
        save_dynamic_workflow("dup", WorkflowScope::Project, "first", false, &cwd).unwrap();
        let err = save_dynamic_workflow("dup", WorkflowScope::Project, "second", false, &cwd)
            .unwrap_err();
        match &err {
            WorkflowSaveError::AlreadyExists { name, path } => {
                assert_eq!(name, "dup");
                assert!(path.ends_with("dup.js"));
            }
            other => panic!("expected AlreadyExists, got {other:?}"),
        }
        // Byte-exact message.
        let expected_path = cwd.join(branding::DOT_DIR).join("workflows").join("dup.js");
        assert_eq!(
            err.to_string(),
            format!(
                "Dynamic workflow \"dup\" already exists at {}. Use a different name or overwrite.",
                expected_path.display()
            )
        );
        // The original content is untouched (create-new never opened it).
        assert_eq!(std::fs::read_to_string(&expected_path).unwrap(), "first");
        let _ = std::fs::remove_dir_all(&cwd);
    }

    #[test]
    fn save_with_overwrite_replaces_existing_file() {
        let cwd = unique_temp_path("save-overwrite");
        std::fs::create_dir_all(&cwd).unwrap();
        save_dynamic_workflow("dup", WorkflowScope::Project, "first", false, &cwd).unwrap();
        let saved =
            save_dynamic_workflow("dup", WorkflowScope::Project, "second", true, &cwd).unwrap();
        assert_eq!(std::fs::read_to_string(&saved.path).unwrap(), "second");
        let _ = std::fs::remove_dir_all(&cwd);
    }

    #[cfg(unix)]
    #[test]
    fn save_refuses_symlinked_project_state_directory() {
        let cwd = unique_temp_path("save-symlink-root");
        let victim = unique_temp_path("save-symlink-victim");
        std::fs::create_dir_all(&cwd).unwrap();
        std::fs::create_dir_all(&victim).unwrap();
        std::os::unix::fs::symlink(&victim, cwd.join(branding::DOT_DIR)).unwrap();

        let err = save_dynamic_workflow("escaped", WorkflowScope::Project, "secret", true, &cwd)
            .unwrap_err();
        assert!(matches!(err, WorkflowSaveError::Io(_)));
        assert!(
            !victim.join("workflows/escaped.js").exists(),
            "a project-local symlink must never redirect workflow output"
        );
        let _ = std::fs::remove_dir_all(&cwd);
        let _ = std::fs::remove_dir_all(&victim);
    }

    #[test]
    fn saved_feedback_is_byte_exact() {
        let path = std::path::Path::new("/home/u/.lingxi/workflows/deploy.js");
        assert_eq!(
            saved_feedback("deploy", path),
            "Dynamic workflow saved to /home/u/.lingxi/workflows/deploy.js. \
             Invoke as /deploy or Workflow({name: \"deploy\"}) in future sessions."
        );
    }
}
