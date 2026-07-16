//! The slash-command registry — the SINGLE source of truth for slash
//! completion metadata, the `/help` command listing, and dispatch (plan
//! Phase 8).
//!
//! [`crate::bottom_pane::completion_view::command_items`] and the `/help`
//! screen's slash-command section
//! ([`crate::bottom_pane::screen_view::help_lines`]) both derive from
//! [`BUILTIN`], so the three surfaces can never drift (test-enforced by
//! `registry_is_the_single_source_for_completion_help_and_dispatch`).
//!
//! [`resolve`] maps a submitted `/command [args]` buffer to its registered
//! [`SlashCommand`]; [`crate::chat_widget::ChatWidget::handle_slash`] then
//! runs the entry's dispatch fn — there is no hard-coded command `match` in
//! the app. Unknown commands resolve to `None` and fall through as a normal
//! prompt (locked behavior).
//!
//! Registry metadata (names, aliases, descriptions, argument hints,
//! visibility) tracks claude-code 2.1.205's command table byte-for-byte
//! (LingXi branding substituted where the reference says "Claude Code"),
//! including its `get description()` dynamic descriptions
//! ([`SlashCommand::dynamic_description`]).

use crate::chat_widget::{ChatOutcome, ChatWidget};

/// How a command treats trailing argument text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArgSpec {
    /// No arguments: trailing text falls through as a normal prompt.
    None,
    /// Arguments required: an empty tail falls through as a normal prompt.
    Required,
    /// Arguments optional: dispatch either way (the handler interprets the
    /// tail, e.g. `/copy [N]`, `/color [name]`, `/export [filename]`).
    Optional,
}

/// One registered slash command: completion/help metadata plus its dispatch.
pub struct SlashCommand {
    /// Canonical `/name` the user types.
    pub name: &'static str,
    /// Alternate names that dispatch identically (`/quit` → `/exit`).
    pub aliases: &'static [&'static str],
    /// One-line description (the completion popup's right column and the
    /// `/help` screen's command listing). Ignored when
    /// [`Self::dynamic_description`] is set — read via [`Self::describe`].
    pub description: &'static str,
    /// claude-code `get description()` parity: commands whose description is
    /// computed from live state (`/fast`, `/sandbox`, `/terminal-setup`).
    /// `None` = static [`Self::description`].
    pub dynamic_description: Option<fn() -> String>,
    /// claude-code `argumentHint` (e.g. `"<path>"`, `"[on|off]"`); `""` when
    /// the command takes no hinted arguments.
    pub hint: &'static str,
    /// How the command treats trailing argument text.
    pub args: ArgSpec,
    /// Whether the command is advertised in the completion popup + `/help`
    /// (claude-code `isHidden: false`). Hidden commands still dispatch, and
    /// surface in the popup when their exact name is typed (claude-code's
    /// `hiddenExact` rule).
    pub advertised: bool,
    /// The handler run on the widget when the command matches. Receives the
    /// trimmed argument tail (`""` when absent).
    pub run: fn(&mut ChatWidget, &str) -> ChatOutcome,
}

impl SlashCommand {
    /// The live one-line description: the `dynamic_description` product when
    /// present (claude-code `get description()`), else the static text.
    #[must_use]
    pub fn describe(&self) -> String {
        self.dynamic_description
            .map_or_else(|| self.description.to_string(), |f| f())
    }
}

/// Every slash command the ratatui backend handles, tracking claude-code
/// 2.1.205's registry. Table order is stable/curated; the completion popup
/// sorts advertised entries alphabetically at render time (claude-code
/// `generateCommandSuggestions` sorts builtins with `localeCompare`), and the
/// `/help` listing keeps table order.
///
/// Commands claude-code 2.1.205 dropped are dropped here too (`/doctor` — now
/// a bundled skill, `/files`, `/commit`, `/init-verifiers`); `/stats` and
/// `/cost` live on as `/usage` aliases, `/vim` as a hidden
/// moved-to-`/config` redirect. `/web`, `/connect`, and `/image` are
/// deliberate LingXi divergences (multi-provider support).
pub const BUILTIN: &[SlashCommand] = &[
    SlashCommand {
        name: "/help",
        aliases: &[],
        description: "Show help and available commands",
        dynamic_description: None,
        hint: "",
        args: ArgSpec::None,
        advertised: true,
        run: ChatWidget::cmd_help,
    },
    SlashCommand {
        name: "/model",
        aliases: &[],
        description: "Set the AI model for LingXi",
        dynamic_description: None,
        hint: "<model>",
        args: ArgSpec::None,
        advertised: true,
        run: ChatWidget::cmd_model,
    },
    SlashCommand {
        name: "/mcp",
        aliases: &[],
        description: "Manage MCP servers",
        dynamic_description: None,
        hint: "[reconnect|enable|disable [<server>|all]]",
        // Optional (NOT None): bare `/mcp` opens the listing; `reconnect
        // [<server>|all]` and any other subcommand must reach the handler
        // (which reconnects or renders the usage line) rather than falling
        // through as an LLM prompt.
        args: ArgSpec::Optional,
        advertised: true,
        run: ChatWidget::cmd_mcp,
    },
    SlashCommand {
        name: "/web",
        aliases: &[],
        description: "Configure web search",
        dynamic_description: None,
        hint: "",
        args: ArgSpec::None,
        advertised: true,
        run: ChatWidget::cmd_web,
    },
    SlashCommand {
        name: "/connect",
        aliases: &[],
        description: "Connect a model provider",
        dynamic_description: None,
        hint: "",
        args: ArgSpec::Optional,
        advertised: true,
        run: ChatWidget::cmd_connect,
    },
    SlashCommand {
        name: "/permissions",
        aliases: &["/allowed-tools"],
        description: "Manage allow and deny tool permission rules",
        dynamic_description: None,
        hint: "",
        args: ArgSpec::None,
        advertised: true,
        run: ChatWidget::cmd_permissions,
    },
    SlashCommand {
        name: "/add-dir",
        aliases: &[],
        description: "Add a new working directory",
        dynamic_description: None,
        hint: "<path>",
        // Optional (NOT Required): a bare `/add-dir` reaches the handler so it
        // renders its own "Usage: /add-dir <path>" line rather than falling
        // through as an LLM prompt (same rationale as `/fork`).
        args: ArgSpec::Optional,
        advertised: true,
        run: ChatWidget::cmd_add_dir,
    },
    SlashCommand {
        // parity 2.1.207 `local-jsx` `name:"cd"`: move this session to a new
        // working directory (confirm dialog → shared `SessionCwd` swap). The
        // description matches `command_api::builtin_support::core_description("cd")`.
        name: "/cd",
        aliases: &[],
        description: "Move this session to a new working directory",
        dynamic_description: None,
        hint: "<path>",
        // Optional (NOT Required): a bare `/cd` reaches the handler so it renders
        // its own "Usage: /cd <path>" line rather than falling through as an LLM
        // prompt (same rationale as `/add-dir`).
        args: ArgSpec::Optional,
        advertised: true,
        run: ChatWidget::cmd_cd,
    },
    SlashCommand {
        name: "/rewind",
        aliases: &["/checkpoint", "/undo"],
        description: "Restore the code and/or conversation to a previous point",
        dynamic_description: None,
        hint: "",
        args: ArgSpec::None,
        advertised: true,
        run: ChatWidget::cmd_rewind,
    },
    SlashCommand {
        name: "/resume",
        aliases: &["/continue"],
        description: "Resume a previous conversation",
        dynamic_description: None,
        hint: "[conversation id or search term]",
        args: ArgSpec::Optional,
        advertised: true,
        run: ChatWidget::cmd_resume,
    },
    SlashCommand {
        name: "/tasks",
        aliases: &["/bashes"],
        description: "View and manage everything running in the background",
        dynamic_description: None,
        hint: "",
        args: ArgSpec::None,
        advertised: true,
        run: ChatWidget::cmd_tasks,
    },
    SlashCommand {
        // 2.1.205 `name:"workflows"` (`local-jsx`, `immediate`). LingXi ships
        // the workflow subsystem always-on (the `allow_workflows` /
        // `tengu_workflows_enabled` / plan gates have no LingXi equivalent — the
        // Workflow tool is registered unconditionally), so this is advertised
        // unconditionally too.
        name: "/workflows",
        aliases: &[],
        description: "Browse running and completed workflows",
        dynamic_description: None,
        hint: "",
        args: ArgSpec::None,
        advertised: true,
        run: ChatWidget::cmd_workflows,
    },
    SlashCommand {
        name: "/hooks",
        aliases: &[],
        description: "View hook configurations for tool events",
        dynamic_description: None,
        hint: "",
        args: ArgSpec::None,
        advertised: true,
        run: ChatWidget::cmd_hooks,
    },
    SlashCommand {
        name: "/agents",
        aliases: &[],
        description: "(removed) Ask LingXi to create/manage subagents, or edit .lingxi/agents/",
        dynamic_description: None,
        hint: "",
        args: ArgSpec::None,
        advertised: true,
        run: ChatWidget::cmd_agents,
    },
    SlashCommand {
        name: "/skills",
        aliases: &[],
        description: "List available skills",
        dynamic_description: None,
        hint: "",
        args: ArgSpec::None,
        advertised: true,
        run: ChatWidget::cmd_skills,
    },
    SlashCommand {
        name: "/memory",
        aliases: &[],
        description: "Open a memory file in your editor",
        dynamic_description: None,
        hint: "",
        args: ArgSpec::None,
        advertised: true,
        run: ChatWidget::cmd_memory,
    },
    SlashCommand {
        name: "/status",
        aliases: &[],
        description: "Show LingXi status including version, model, account, API connectivity, and tool statuses",
        dynamic_description: None,
        hint: "",
        args: ArgSpec::None,
        advertised: true,
        run: ChatWidget::cmd_status,
    },
    SlashCommand {
        name: "/config",
        aliases: &["/settings"],
        description: "Open settings",
        dynamic_description: None,
        hint: "[key=value]",
        // Optional (NOT None): bare `/config` opens the settings screen;
        // `key=value` pairs must reach the handler (which applies them or
        // renders the byte-exact usage/error lines) rather than falling
        // through as an LLM prompt.
        args: ArgSpec::Optional,
        advertised: true,
        run: ChatWidget::cmd_config,
    },
    SlashCommand {
        name: "/diff",
        aliases: &[],
        description: "View uncommitted changes and per-turn diffs",
        dynamic_description: None,
        hint: "",
        args: ArgSpec::None,
        advertised: true,
        run: ChatWidget::cmd_diff,
    },
    SlashCommand {
        name: "/export",
        aliases: &[],
        description: "Export the current conversation to a file or clipboard",
        dynamic_description: None,
        hint: "[filename]",
        args: ArgSpec::Optional,
        advertised: true,
        run: ChatWidget::cmd_export,
    },
    SlashCommand {
        name: "/copy",
        aliases: &[],
        description: "Copy LingXi's last response to clipboard (or /copy N for the Nth-latest)",
        dynamic_description: None,
        hint: "",
        args: ArgSpec::Optional,
        advertised: true,
        run: ChatWidget::cmd_copy,
    },
    SlashCommand {
        name: "/theme",
        aliases: &[],
        description: "Change the theme",
        dynamic_description: None,
        hint: "",
        args: ArgSpec::None,
        advertised: true,
        run: ChatWidget::cmd_theme,
    },
    SlashCommand {
        name: "/color",
        aliases: &[],
        description: "Set the prompt bar color for this session",
        dynamic_description: None,
        hint: "",
        args: ArgSpec::Optional,
        advertised: true,
        run: ChatWidget::cmd_color,
    },
    // claude-code 2.1.205 replaced `/vim` with a hidden moved-to-`/config`
    // redirect (`lBd("vim", "Editor mode")`): unadvertised, and running it is
    // handled by `cmd_vim` (which still toggles until `/config` gains
    // key=value editing — tracked R2 work).
    SlashCommand {
        name: "/vim",
        aliases: &[],
        description: "Editor mode moved to /config",
        dynamic_description: None,
        hint: "",
        args: ArgSpec::None,
        advertised: false,
        run: ChatWidget::cmd_vim,
    },
    SlashCommand {
        name: "/clear",
        aliases: &["/reset", "/new"],
        description: "Start a new session with empty context; previous session stays on disk (resumable with /resume)",
        dynamic_description: None,
        hint: "[name]",
        // Optional: claude-code names the fresh session with the tail; the
        // clear must dispatch either way rather than fall through as a prompt.
        args: ArgSpec::Optional,
        advertised: true,
        run: ChatWidget::cmd_clear,
    },
    SlashCommand {
        name: "/exit",
        aliases: &["/quit"],
        description: "Exit the CLI",
        dynamic_description: None,
        hint: "",
        args: ArgSpec::None,
        advertised: true,
        run: cmd_exit,
    },
    SlashCommand {
        name: "/image",
        aliases: &[],
        description: "Attach an image file by path",
        dynamic_description: None,
        hint: "",
        args: ArgSpec::Required,
        advertised: false,
        run: ChatWidget::cmd_image,
    },
    // ===== `command_core`-backed commands (bridged via
    // `ChatWidget::run_core_command`). Static-template prompt injectors and
    // read-only reports whose handlers construct from bare state. =====
    SlashCommand {
        name: "/init",
        aliases: &[],
        description: "Initialize a new LINGXI.md file with codebase documentation",
        dynamic_description: None,
        hint: "",
        args: ArgSpec::None,
        advertised: true,
        run: ChatWidget::cmd_init,
    },
    SlashCommand {
        name: "/commit-push-pr",
        aliases: &[],
        description: "Commit, push, and open a PR",
        dynamic_description: None,
        hint: "",
        args: ArgSpec::Optional,
        advertised: true,
        run: ChatWidget::cmd_commit_push_pr,
    },
    SlashCommand {
        name: "/review",
        aliases: &[],
        description: "Review a GitHub pull request; for your working diff use /code-review",
        dynamic_description: None,
        hint: "[pr number]",
        args: ArgSpec::Optional,
        advertised: true,
        run: ChatWidget::cmd_review,
    },
    SlashCommand {
        name: "/security-review",
        aliases: &[],
        description: "Complete a security review of the pending changes on the current branch",
        dynamic_description: None,
        hint: "",
        args: ArgSpec::None,
        advertised: true,
        run: ChatWidget::cmd_security_review,
    },
    SlashCommand {
        name: "/statusline",
        aliases: &[],
        description: "Set up LingXi's status line UI",
        dynamic_description: None,
        hint: "",
        args: ArgSpec::Optional,
        advertised: true,
        run: ChatWidget::cmd_statusline,
    },
    SlashCommand {
        name: "/insights",
        aliases: &[],
        description: "Generate a report analyzing your LingXi sessions",
        dynamic_description: None,
        hint: "",
        args: ArgSpec::Optional,
        advertised: true,
        run: ChatWidget::cmd_insights,
    },
    // Hidden in claude-code 2.1.205 (`isHidden: true`): dispatchable, shown
    // in the popup only when fully typed.
    SlashCommand {
        name: "/version",
        aliases: &[],
        description: "Show this session's version (autoupdate may have a newer one)",
        dynamic_description: None,
        hint: "",
        args: ArgSpec::None,
        advertised: false,
        run: ChatWidget::cmd_version,
    },
    SlashCommand {
        name: "/release-notes",
        aliases: &[],
        description: "View release notes",
        dynamic_description: None,
        hint: "",
        args: ArgSpec::None,
        advertised: true,
        run: ChatWidget::cmd_release_notes,
    },
    SlashCommand {
        name: "/stickers",
        aliases: &[],
        description: "Order LingXi stickers",
        dynamic_description: None,
        hint: "",
        args: ArgSpec::None,
        advertised: true,
        run: ChatWidget::cmd_stickers,
    },
    SlashCommand {
        name: "/autocompact",
        aliases: &[],
        description: "Set how full the context gets before auto-summarizing",
        dynamic_description: None,
        hint: "[auto|<tokens>]",
        args: ArgSpec::Optional,
        advertised: true,
        run: ChatWidget::cmd_autocompact,
    },
    SlashCommand {
        name: "/keybindings",
        aliases: &[],
        description: "Open your keyboard shortcuts file",
        dynamic_description: None,
        hint: "",
        args: ArgSpec::None,
        advertised: true,
        run: ChatWidget::cmd_keybindings,
    },
    SlashCommand {
        name: "/terminal-setup",
        aliases: &[],
        description: "Install Shift+Enter key binding for newlines",
        // claude-code 2.1.205 no longer hides this row on CSI-u terminals; the
        // description itself is computed per-terminal (`get description()`).
        dynamic_description: Some(desc_terminal_setup),
        hint: "",
        args: ArgSpec::None,
        advertised: true,
        run: ChatWidget::cmd_terminal_setup,
    },
    SlashCommand {
        name: "/skill-doctor",
        aliases: &[],
        description: "Show which loaded skills are unused and costing context",
        dynamic_description: None,
        hint: "",
        args: ArgSpec::None,
        advertised: true,
        run: ChatWidget::cmd_skill_doctor,
    },
    // ===== OrchestratorHandle-backed commands (need a live engine handle,
    // threaded into `ChatWidget` from the CLI `run_app`; a no-op "unavailable"
    // system line when unwired). =====
    SlashCommand {
        name: "/context",
        aliases: &[],
        description: "Visualize current context usage as a colored grid",
        dynamic_description: None,
        hint: "[all]",
        args: ArgSpec::None,
        advertised: true,
        run: ChatWidget::cmd_context,
    },
    SlashCommand {
        name: "/usage",
        aliases: &["/cost", "/stats"],
        description: "Show session cost, plan usage, and activity stats",
        dynamic_description: None,
        hint: "",
        args: ArgSpec::None,
        advertised: true,
        run: ChatWidget::cmd_usage,
    },
    SlashCommand {
        name: "/effort",
        aliases: &[],
        description: "Set effort level for model usage",
        dynamic_description: None,
        hint: "",
        args: ArgSpec::Optional,
        advertised: true,
        run: ChatWidget::cmd_effort,
    },
    SlashCommand {
        name: "/fast",
        aliases: &[],
        description: "Toggle fast mode (Opus 4.8)",
        dynamic_description: None,
        hint: "[on|off]",
        // Optional: `on`/`off` set the state; a bare `/fast` toggles it. Reaches
        // the handler either way rather than falling through as a prompt.
        args: ArgSpec::Optional,
        advertised: true,
        run: ChatWidget::cmd_fast,
    },
    SlashCommand {
        name: "/plan",
        aliases: &[],
        description: "Enable plan mode or view the current session plan",
        dynamic_description: None,
        hint: "[open|share|<description>]",
        // Optional (NOT Required): a trailing `open`/`<description>` must reach
        // the handler (which enters plan mode) rather than falling through as an
        // LLM prompt. LingXi has no plan store, so the description is not
        // submitted and `open` does not launch an editor (documented gap).
        args: ArgSpec::Optional,
        advertised: true,
        run: ChatWidget::cmd_plan,
    },
    SlashCommand {
        name: "/goal",
        aliases: &[],
        description: "Set a goal LingXi checks before stopping",
        dynamic_description: None,
        hint: "[<condition> | clear]",
        args: ArgSpec::Optional,
        advertised: true,
        run: ChatWidget::cmd_goal,
    },
    SlashCommand {
        name: "/fork",
        aliases: &[],
        description: "Spawn a background agent that inherits the full conversation",
        dynamic_description: None,
        hint: "<directive>",
        // Optional (NOT Required): a bare `/fork` must reach the handler so it
        // renders its own "Usage: /fork <directive>" line rather than falling
        // through as an LLM prompt.
        args: ArgSpec::Optional,
        advertised: true,
        run: ChatWidget::cmd_fork,
    },
    SlashCommand {
        name: "/branch",
        // No `fork` alias: claude gates it on `feature('FORK_SUBAGENT') ? [] :
        // ['fork']`, and LingXi ships /fork as its own command, so the alias set
        // is empty (the FORK_SUBAGENT-enabled branch).
        aliases: &[],
        description: "Create a branch of the current conversation at this point",
        dynamic_description: None,
        hint: "[name]",
        // Optional (NOT Required): a bare `/branch` must reach the handler so it
        // derives the branch name from the first prompt rather than falling
        // through as an LLM prompt.
        args: ArgSpec::Optional,
        advertised: true,
        run: ChatWidget::cmd_branch,
    },
    SlashCommand {
        name: "/recap",
        aliases: &[],
        description: "Generate a one-line session recap now",
        dynamic_description: None,
        hint: "",
        args: ArgSpec::None,
        advertised: true,
        run: ChatWidget::cmd_recap,
    },
    SlashCommand {
        name: "/btw",
        aliases: &[],
        description: "Ask a quick side question without interrupting the main conversation",
        dynamic_description: None,
        hint: "<question>",
        // Optional (NOT Required): a bare `/btw` must reach the handler so it
        // renders "Usage: /btw <your question>" rather than falling through as
        // an LLM prompt (same rationale as `/fork`).
        args: ArgSpec::Optional,
        advertised: true,
        run: ChatWidget::cmd_btw,
    },
    SlashCommand {
        name: "/rename",
        aliases: &["/name"],
        description: "Rename the current conversation",
        dynamic_description: None,
        hint: "[name]",
        // Optional (NOT Required): a bare `/rename` must reach the handler so it
        // renders "Usage: /rename <name>" rather than falling through as an LLM
        // prompt (auto-name generation is deferred).
        args: ArgSpec::Optional,
        advertised: true,
        run: ChatWidget::cmd_rename,
    },
    SlashCommand {
        name: "/reload-skills",
        aliases: &[],
        description: "Pick up skills added or changed on disk during this session",
        dynamic_description: None,
        hint: "",
        args: ArgSpec::None,
        advertised: true,
        run: ChatWidget::cmd_reload_skills,
    },
    SlashCommand {
        name: "/plugin",
        aliases: &["/plugins", "/marketplace"],
        description: "Manage LingXi plugins",
        dynamic_description: None,
        hint: "",
        // None: `/plugin` opens the interactive manager; args are ignored (the
        // arg-driven subcommands live on the `lingxi-cli plugin` CLI surface).
        args: ArgSpec::None,
        advertised: true,
        run: ChatWidget::cmd_plugin,
    },
    SlashCommand {
        name: "/reload-plugins",
        aliases: &[],
        description: "Activate pending plugin changes in the current session",
        dynamic_description: None,
        hint: "[--force]",
        args: ArgSpec::None,
        advertised: true,
        run: ChatWidget::cmd_reload_plugins,
    },
    SlashCommand {
        name: "/sandbox",
        aliases: &[],
        description: "Toggle sandbox mode for bash commands",
        // claude-code renders live sandbox state (glyph + enabled/disabled +
        // "(⏎ to configure)"); hidden entirely when the platform can't sandbox
        // (see `is_runtime_hidden`).
        dynamic_description: Some(desc_sandbox),
        hint: "exclude \"command pattern\"",
        // Optional (NOT None): a bare `/sandbox` toggles; `exclude "..."` and an
        // unknown subcommand must also reach the handler for their echoes.
        args: ArgSpec::Optional,
        advertised: true,
        run: ChatWidget::cmd_sandbox,
    },
    SlashCommand {
        name: "/compact",
        aliases: &[],
        description: "Free up context by summarizing the conversation so far",
        dynamic_description: None,
        hint: "<optional custom summarization instructions>",
        args: ArgSpec::Optional,
        advertised: true,
        run: ChatWidget::cmd_compact,
    },
    // `/stop` is dispatchable but NOT advertised: claude-code gates it on
    // `isEnabled: () => LINGXI_SESSION_KIND === "bg"`, and this ratatui path is
    // always an interactive (never a `bg`) session, so it must never surface in
    // the completion popup / `/help` — matching the reference's hidden-in-TUI
    // behavior without runtime-gating the static table. Typing `/stop` still
    // works (shows "Session stopped." and quits).
    SlashCommand {
        name: "/stop",
        aliases: &[],
        description: "Stop this background session; transcript and worktree are kept",
        dynamic_description: None,
        hint: "",
        args: ArgSpec::None,
        advertised: false,
        run: ChatWidget::cmd_stop,
    },
];

/// The advertised registry entries in table order (callers that need
/// claude-code's alphabetical popup order sort at render time).
pub fn advertised() -> impl Iterator<Item = &'static SlashCommand> {
    BUILTIN
        .iter()
        .filter(|command| command.advertised && !is_runtime_hidden(command.name))
}

/// Runtime `isHidden` gate for statically-`advertised` rows whose palette /
/// `/help` visibility depends on the environment, mirroring claude-code's
/// per-command `isHidden` predicate. Currently only `/sandbox`, which
/// claude-code hides when the platform cannot sandbox
/// (`!Mo.isSupportedPlatform()` — sandboxing is macOS seatbelt + Linux bwrap).
/// Deliberately build-static (NOT keyed on [`register_sandbox_toggle`]) so
/// popup/help listings are deterministic. `command_items` / `help_lines`
/// derive from `advertised()`, so both surfaces honor this gate uniformly.
#[must_use]
pub fn is_runtime_hidden(name: &str) -> bool {
    name == "/sandbox" && !cfg!(any(target_os = "macos", target_os = "linux"))
}

/// The live sandbox on/off state for the popup's `/sandbox` row, read from the
/// currently-registered toggle. `false` when sandboxing is unsupported/unwired.
fn sandbox_enabled() -> bool {
    SANDBOX_TOGGLE
        .lock()
        .ok()
        .and_then(|slot| {
            slot.as_ref()
                .map(|t| t.load(std::sync::atomic::Ordering::Relaxed))
        })
        .unwrap_or(false)
}

/// The live sandbox on/off cell, shared with the widget the CLI wires via
/// `ChatWidget::set_sandbox_toggle`. Held in a `Mutex` (not a `OnceLock`) so
/// [`register_sandbox_toggle`] can REPLACE it: the CLI builds a fresh runtime
/// (and a fresh toggle `Arc`) on every in-process session switch (`/resume`,
/// `/branch`, `/rewind`), so a first-registration-wins cell would freeze the
/// popup's `/sandbox` state to session A while `cmd_sandbox` flips session B's.
static SANDBOX_TOGGLE: std::sync::Mutex<Option<std::sync::Arc<std::sync::atomic::AtomicBool>>> =
    std::sync::Mutex::new(None);

/// Register (REPLACING any prior) the live sandbox toggle for `/sandbox`'s
/// dynamic description. Every call overwrites, so the popup row always tracks
/// the CURRENT session's toggle across in-process session switches.
pub fn register_sandbox_toggle(toggle: std::sync::Arc<std::sync::atomic::AtomicBool>) {
    if let Ok(mut slot) = SANDBOX_TOGGLE.lock() {
        *slot = Some(toggle);
    }
}

/// The static (session-config-derived) inputs to the `/sandbox` description
/// beyond the live on/off toggle: claude-code's `isAutoAllowBashIfSandboxedEnabled`
/// (`t`), `areUnsandboxedCommandsAllowed` (`r`), the policy-lock (`n` =
/// `areSandboxSettingsLockedByPolicy || areUnsandboxedCommandsForbiddenByPolicy`),
/// and `checkDependencies().errors.length === 0` (`o`). Registered once per
/// session by the CLI mount; the [`Default`] leaves every flag off (and
/// `deps_ok`) so an unregistered build renders exactly the old
/// `${glyph} sandbox enabled|disabled (⏎ to configure)` string.
#[derive(Debug, Clone, Copy)]
pub struct SandboxDescFlags {
    /// `isAutoAllowBashIfSandboxedEnabled` → "sandbox enabled (auto-allow)".
    pub auto_allow: bool,
    /// `areUnsandboxedCommandsAllowed` → ", fallback allowed".
    pub fallback_allowed: bool,
    /// policy-locked → " (managed)".
    pub managed: bool,
    /// `checkDependencies().errors.length === 0`; `false` → the warning glyph.
    pub deps_ok: bool,
}

impl Default for SandboxDescFlags {
    fn default() -> Self {
        Self {
            auto_allow: false,
            fallback_allowed: false,
            managed: false,
            deps_ok: true,
        }
    }
}

static SANDBOX_DESC_FLAGS: std::sync::Mutex<SandboxDescFlags> =
    std::sync::Mutex::new(SandboxDescFlags {
        auto_allow: false,
        fallback_allowed: false,
        managed: false,
        deps_ok: true,
    });

/// Register (REPLACING any prior) the static `/sandbox` description flags for the
/// current session — the CLI mount derives these from the resolved
/// `SandboxRuntimeConfig`. Mirrors [`register_sandbox_toggle`]'s replace-on-mount
/// discipline so the popup row tracks the CURRENT session's config.
pub fn register_sandbox_desc_flags(flags: SandboxDescFlags) {
    if let Ok(mut slot) = SANDBOX_DESC_FLAGS.lock() {
        *slot = flags;
    }
}

fn sandbox_desc_flags() -> SandboxDescFlags {
    SANDBOX_DESC_FLAGS.lock().map(|f| *f).unwrap_or_default()
}

/// `/sandbox` dynamic description — byte-exact port of claude-code 2.1.206's
/// `get description()`:
/// ```text
/// i = !o ? warning : (e ? tick : circle)
/// s = e ? (t ? "sandbox enabled (auto-allow)" : "sandbox enabled") + (r ? ", fallback allowed" : "")
///       : "sandbox disabled"
/// s += n ? " (managed)" : ""
/// `${i} ${s} (⏎ to configure)`
/// ```
/// where `e` = live toggle ([`sandbox_enabled`]) and `t`/`r`/`n`/`o` come from
/// the registered [`SandboxDescFlags`]. Figures: `warning` U+26A0, `tick` U+2714,
/// `circle` U+25EF (unicode forms, matching the port's existing glyphs).
fn desc_sandbox() -> String {
    render_sandbox_desc(sandbox_enabled(), sandbox_desc_flags())
}

/// Pure render of the `/sandbox` description from the live toggle + static flags
/// (claude-code `get description()`). Separated from the process-globals so it is
/// testable in isolation.
fn render_sandbox_desc(enabled: bool, f: SandboxDescFlags) -> String {
    let glyph = if !f.deps_ok {
        "\u{26A0}"
    } else if enabled {
        "\u{2714}"
    } else {
        "\u{25EF}"
    };
    let mut status = if enabled {
        let mut s = if f.auto_allow {
            "sandbox enabled (auto-allow)".to_string()
        } else {
            "sandbox enabled".to_string()
        };
        if f.fallback_allowed {
            s.push_str(", fallback allowed");
        }
        s
    } else {
        "sandbox disabled".to_string()
    };
    if f.managed {
        status.push_str(" (managed)");
    }
    format!("{glyph} {status} (\u{23CE} to configure)")
}

/// `/terminal-setup` dynamic description — claude-code 2.1.205
/// `get description()` (per-terminal branch order preserved).
fn desc_terminal_setup() -> String {
    tui_core::terminal_setup::dynamic_description()
}

/// `/exit` (alias `/quit`): exit the app. A free function (not a
/// `ChatWidget` method) because it touches no widget state.
fn cmd_exit(_widget: &mut ChatWidget, _args: &str) -> ChatOutcome {
    ChatOutcome::Quit
}

/// Resolve a submitted composer buffer to `(command, trimmed args)`.
///
/// `None` (the buffer falls through as a normal prompt) when the input is not
/// `/`-led, names no registered command or alias, passes arguments to an
/// [`ArgSpec::None`] command, or omits them for an [`ArgSpec::Required`] one
/// — all locked pre-registry behavior.
#[must_use]
pub fn resolve(input: &str) -> Option<(&'static SlashCommand, &str)> {
    let trimmed = input.trim();
    if !trimmed.starts_with('/') {
        return None;
    }
    let (name, args) = trimmed
        .split_once(char::is_whitespace)
        .map_or((trimmed, ""), |(name, args)| (name, args.trim()));
    let command = BUILTIN
        .iter()
        .find(|command| command.name == name || command.aliases.contains(&name))?;
    match command.args {
        ArgSpec::None if !args.is_empty() => return None,
        ArgSpec::Required if args.is_empty() => return None,
        _ => {}
    }
    Some((command, args))
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;
    use crate::bottom_pane::completion_view::command_items;
    use crate::bottom_pane::screen_view::help_lines;

    /// Plan Phase 8 step 6: every registry command has completion metadata,
    /// help metadata, and a dispatch path — all derived from ONE registry.
    #[test]
    fn registry_is_the_single_source_for_completion_help_and_dispatch() {
        // Well-formed, unique entries.
        let mut seen = HashSet::new();
        for command in BUILTIN {
            assert!(
                command.name.starts_with('/') && command.name.len() > 1,
                "malformed name {:?}",
                command.name
            );
            assert!(
                !command.describe().trim().is_empty(),
                "{} has no description",
                command.name
            );
            assert!(seen.insert(command.name), "duplicate name {}", command.name);
            for alias in command.aliases {
                assert!(alias.starts_with('/'), "malformed alias {alias:?}");
                assert!(seen.insert(alias), "alias collides: {alias}");
            }
        }

        // Dispatch path: every name and alias resolves back to its entry with
        // an argument shape the command accepts.
        for command in BUILTIN {
            let probe = |name: &str| match command.args {
                ArgSpec::Required => format!("{name} x"),
                ArgSpec::None | ArgSpec::Optional => name.to_string(),
            };
            let (resolved, _) = resolve(&probe(command.name))
                .unwrap_or_else(|| panic!("{} has no dispatch path", command.name));
            assert_eq!(resolved.name, command.name);
            for alias in command.aliases {
                let (resolved, _) = resolve(&probe(alias))
                    .unwrap_or_else(|| panic!("alias {alias} has no dispatch path"));
                assert_eq!(resolved.name, command.name, "alias {alias} mis-routes");
            }
        }

        // Completion metadata derives from the registry: bare "/" lists every
        // advertised command ALPHABETICALLY (claude-code sorts builtin popup
        // rows with localeCompare) with its live description.
        let items = command_items("/");
        assert_eq!(items.len(), advertised().count());
        let mut expected: Vec<_> = advertised().collect();
        expected.sort_by_key(|command| command.name);
        for (item, command) in items.iter().zip(expected) {
            assert_eq!(item.insert, command.name);
            // Dynamic descriptions read live global state that parallel tests
            // mutate (the sandbox toggle) — only static text is compared.
            if command.dynamic_description.is_none() {
                assert_eq!(item.desc, command.describe());
            }
        }

        // Help metadata derives from the registry: every advertised command
        // (name + description) appears in the /help body; unadvertised ones
        // do not.
        let help_text: String = help_lines()
            .iter()
            .flat_map(|line| line.spans.iter())
            .map(|span| span.content.as_ref())
            .collect::<Vec<_>>()
            .join(" ");
        for command in BUILTIN {
            if command.advertised && !is_runtime_hidden(command.name) {
                assert!(
                    help_text.contains(command.name),
                    "{} missing from /help",
                    command.name
                );
                // Dynamic descriptions can change between help_lines() and
                // here (parallel tests mutate the sandbox toggle) — only
                // static text is asserted verbatim.
                if command.dynamic_description.is_none() {
                    assert!(
                        help_text.contains(&command.describe()),
                        "{} description missing from /help",
                        command.name
                    );
                }
            } else {
                assert!(
                    !help_text.contains(&format!("{} ", command.name)),
                    "unadvertised {} leaked into /help",
                    command.name
                );
            }
        }
    }

    /// Deliberately dropped commands stay dropped: claude-code 2.1.205 removed
    /// `/doctor` (now a bundled skill), `/files`, `/commit`, and
    /// `/init-verifiers`; `/stats` and `/cost` survive only as `/usage`
    /// aliases.
    #[test]
    fn commands_dropped_in_2_1_205_are_not_registered() {
        for gone in ["/doctor", "/files", "/commit", "/init-verifiers"] {
            assert!(
                resolve(gone).is_none(),
                "{gone} should not resolve (removed in claude-code 2.1.205)"
            );
        }
        // `/stats` and `/cost` now route to `/usage`.
        assert_eq!(resolve("/stats").expect("alias").0.name, "/usage");
        assert_eq!(resolve("/cost").expect("alias").0.name, "/usage");
    }

    #[test]
    fn tasks_command_is_registered_with_its_alias() {
        assert_eq!(resolve("/tasks").expect("registered").0.name, "/tasks");
        assert_eq!(resolve("/bashes").expect("alias").0.name, "/tasks");
        assert!(BUILTIN.iter().any(|c| c.name == "/tasks"));
    }

    #[test]
    fn resolve_finds_commands_and_aliases() {
        let (help, args) = resolve("/help").expect("registered command");
        assert_eq!(help.name, "/help");
        assert_eq!(args, "");
        // Surrounding whitespace is trimmed before matching.
        assert_eq!(resolve("  /clear  ").expect("trimmed").0.name, "/clear");
        // An alias resolves to its canonical entry.
        assert_eq!(resolve("/quit").expect("alias").0.name, "/exit");
        // /permissions is registered with its claude-code alias.
        assert_eq!(
            resolve("/permissions").expect("registered").0.name,
            "/permissions"
        );
        assert_eq!(
            resolve("/allowed-tools").expect("alias").0.name,
            "/permissions"
        );
        // /resume is registered with its claude-code alias.
        assert_eq!(resolve("/resume").expect("registered").0.name, "/resume");
        assert_eq!(resolve("/continue").expect("alias").0.name, "/resume");
        // 2.1.205 alias additions.
        assert_eq!(resolve("/undo").expect("alias").0.name, "/rewind");
        assert_eq!(resolve("/reset").expect("alias").0.name, "/clear");
        assert_eq!(resolve("/new").expect("alias").0.name, "/clear");
        assert_eq!(resolve("/settings").expect("alias").0.name, "/config");
        assert_eq!(resolve("/name").expect("alias").0.name, "/rename");
    }

    #[test]
    fn resolve_gates_arguments() {
        // Arg-less commands reject trailing text (falls through as a prompt).
        assert!(resolve("/help extra").is_none());
        // Arg-taking commands require a non-empty tail…
        assert!(resolve("/image").is_none());
        assert!(resolve("/image   ").is_none());
        // …and receive it trimmed.
        let (image, args) = resolve("/image  /tmp/pic.png ").expect("args");
        assert_eq!(image.name, "/image");
        assert_eq!(args, "/tmp/pic.png");
    }

    #[test]
    fn resolve_rejects_unknown_and_non_slash_input() {
        assert!(resolve("/frobnicate").is_none());
        assert!(resolve("help").is_none());
        assert!(resolve("").is_none());
    }

    /// Dynamic descriptions (claude-code `get description()`): `/sandbox`
    /// reflects the registered toggle's live state; `/terminal-setup` reflects
    /// the detected terminal.
    #[test]
    fn dynamic_descriptions_reflect_live_state() {
        let sandbox = BUILTIN.iter().find(|c| c.name == "/sandbox").unwrap();
        // Unregistered (or registered-off) reads as disabled...
        let before = sandbox.describe();
        assert!(before.ends_with("(\u{23CE} to configure)"), "{before}");
        // ...and once a toggle is wired the row un-hides and tracks its state.
        // (The registry cell is a process-wide Mutex; `register_sandbox_toggle`
        // now overwrites, so ours wins — but a parallel widget test may register
        // AFTER us, so the exact-state assertions only run while ours is live.)
        let ours = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
        register_sandbox_toggle(ours.clone());
        assert!(!is_runtime_hidden("/sandbox"));
        let is_ours = SANDBOX_TOGGLE
            .lock()
            .ok()
            .and_then(|slot| {
                slot.as_ref()
                    .map(|live| std::sync::Arc::ptr_eq(live, &ours))
            })
            .unwrap_or(false);
        if is_ours {
            ours.store(true, std::sync::atomic::Ordering::Relaxed);
            assert_eq!(
                sandbox.describe(),
                "\u{2714} sandbox enabled (\u{23CE} to configure)"
            );
            ours.store(false, std::sync::atomic::Ordering::Relaxed);
            assert_eq!(
                sandbox.describe(),
                "\u{25EF} sandbox disabled (\u{23CE} to configure)"
            );
        }

        let ts = BUILTIN
            .iter()
            .find(|c| c.name == "/terminal-setup")
            .unwrap();
        assert!(!ts.describe().is_empty());
    }

    #[test]
    fn sandbox_description_renders_all_flag_states() {
        use super::{render_sandbox_desc, SandboxDescFlags};
        let f = |auto_allow, fallback_allowed, managed, deps_ok| SandboxDescFlags {
            auto_allow,
            fallback_allowed,
            managed,
            deps_ok,
        };
        // Baseline (default flags) — unchanged from the pre-fidelity strings.
        assert_eq!(
            render_sandbox_desc(false, SandboxDescFlags::default()),
            "\u{25EF} sandbox disabled (\u{23CE} to configure)"
        );
        assert_eq!(
            render_sandbox_desc(true, SandboxDescFlags::default()),
            "\u{2714} sandbox enabled (\u{23CE} to configure)"
        );
        // auto-allow.
        assert_eq!(
            render_sandbox_desc(true, f(true, false, false, true)),
            "\u{2714} sandbox enabled (auto-allow) (\u{23CE} to configure)"
        );
        // auto-allow + fallback.
        assert_eq!(
            render_sandbox_desc(true, f(true, true, false, true)),
            "\u{2714} sandbox enabled (auto-allow), fallback allowed (\u{23CE} to configure)"
        );
        // fallback without auto-allow.
        assert_eq!(
            render_sandbox_desc(true, f(false, true, false, true)),
            "\u{2714} sandbox enabled, fallback allowed (\u{23CE} to configure)"
        );
        // managed appends to any state (enabled + disabled).
        assert_eq!(
            render_sandbox_desc(true, f(true, true, true, true)),
            "\u{2714} sandbox enabled (auto-allow), fallback allowed (managed) (\u{23CE} to configure)"
        );
        assert_eq!(
            render_sandbox_desc(false, f(false, false, true, true)),
            "\u{25EF} sandbox disabled (managed) (\u{23CE} to configure)"
        );
        // Dependency errors → the warning glyph, overriding tick/circle in BOTH
        // enabled and disabled states.
        assert_eq!(
            render_sandbox_desc(true, f(false, false, false, false)),
            "\u{26A0} sandbox enabled (\u{23CE} to configure)"
        );
        assert_eq!(
            render_sandbox_desc(false, f(false, false, false, false)),
            "\u{26A0} sandbox disabled (\u{23CE} to configure)"
        );
    }
}
