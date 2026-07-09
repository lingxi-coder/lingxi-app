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
    /// `/help` screen's command listing).
    pub description: &'static str,
    /// How the command treats trailing argument text.
    pub args: ArgSpec,
    /// Whether the command is advertised in the completion popup + `/help`.
    pub advertised: bool,
    /// The handler run on the widget when the command matches. Receives the
    /// trimmed argument tail (`""` when absent).
    pub run: fn(&mut ChatWidget, &str) -> ChatOutcome,
}

/// Every slash command the ratatui backend handles. Order is the completion
/// popup order and the `/help` listing order.
///
/// Commands the iocraft backend advertised but that have NO data source or
/// core API on this backend are deliberately NOT registered (never advertise
/// "not implemented"): `/tasks` (no background-task feed reaches `run_app`).
/// (`/fork` and `/recap` were previously deferred here; the engine
/// `fork_conversation` override + `generate_recap` seam now exist, so both are
/// registered below.)
pub const BUILTIN: &[SlashCommand] = &[
    SlashCommand {
        name: "/help",
        aliases: &[],
        description: "Show shortcuts and commands",
        args: ArgSpec::None,
        advertised: true,
        run: ChatWidget::cmd_help,
    },
    SlashCommand {
        name: "/model",
        aliases: &[],
        description: "Switch the active model",
        args: ArgSpec::None,
        advertised: true,
        run: ChatWidget::cmd_model,
    },
    SlashCommand {
        name: "/doctor",
        aliases: &[],
        description: "Show diagnostics",
        args: ArgSpec::None,
        advertised: true,
        run: ChatWidget::cmd_doctor,
    },
    SlashCommand {
        name: "/mcp",
        aliases: &[],
        description: "List MCP servers",
        args: ArgSpec::None,
        advertised: true,
        run: ChatWidget::cmd_mcp,
    },
    SlashCommand {
        name: "/web",
        aliases: &[],
        description: "Configure web search",
        args: ArgSpec::None,
        advertised: true,
        run: ChatWidget::cmd_web,
    },
    SlashCommand {
        name: "/connect",
        aliases: &[],
        description: "Connect a model provider",
        args: ArgSpec::Optional,
        advertised: true,
        run: ChatWidget::cmd_connect,
    },
    SlashCommand {
        name: "/permissions",
        aliases: &["/allowed-tools"],
        description: "Manage allow, ask, and deny tool permission rules",
        args: ArgSpec::None,
        advertised: true,
        run: ChatWidget::cmd_permissions,
    },
    SlashCommand {
        name: "/add-dir",
        aliases: &[],
        description: "Add a new working directory",
        // Optional (NOT Required): a bare `/add-dir` reaches the handler so it
        // renders its own "Usage: /add-dir <path>" line rather than falling
        // through as an LLM prompt (same rationale as `/fork`).
        args: ArgSpec::Optional,
        advertised: true,
        run: ChatWidget::cmd_add_dir,
    },
    SlashCommand {
        name: "/resume",
        aliases: &["/continue"],
        description: "Resume a previous conversation",
        args: ArgSpec::Optional,
        advertised: true,
        run: ChatWidget::cmd_resume,
    },
    SlashCommand {
        name: "/hooks",
        aliases: &[],
        description: "List hooks",
        args: ArgSpec::None,
        advertised: true,
        run: ChatWidget::cmd_hooks,
    },
    SlashCommand {
        name: "/agents",
        aliases: &[],
        description: "List agents",
        args: ArgSpec::None,
        advertised: true,
        run: ChatWidget::cmd_agents,
    },
    SlashCommand {
        name: "/skills",
        aliases: &[],
        description: "List available skills",
        args: ArgSpec::None,
        advertised: true,
        run: ChatWidget::cmd_skills,
    },
    SlashCommand {
        name: "/memory",
        aliases: &[],
        description: "Show LINGXI.md memory files",
        args: ArgSpec::None,
        advertised: true,
        run: ChatWidget::cmd_memory,
    },
    SlashCommand {
        name: "/status",
        aliases: &[],
        description: "Show the session status",
        args: ArgSpec::None,
        advertised: true,
        run: ChatWidget::cmd_status,
    },
    SlashCommand {
        name: "/config",
        aliases: &[],
        description: "Show settings (read-only)",
        args: ArgSpec::None,
        advertised: true,
        run: ChatWidget::cmd_config,
    },
    SlashCommand {
        name: "/stats",
        aliases: &[],
        description: "Show session statistics",
        args: ArgSpec::None,
        advertised: true,
        run: ChatWidget::cmd_stats,
    },
    SlashCommand {
        name: "/diff",
        aliases: &[],
        description: "View uncommitted changes and per-turn diffs",
        args: ArgSpec::None,
        advertised: true,
        run: ChatWidget::cmd_diff,
    },
    SlashCommand {
        name: "/export",
        aliases: &[],
        description: "Export the conversation to a file",
        args: ArgSpec::Optional,
        advertised: true,
        run: ChatWidget::cmd_export,
    },
    SlashCommand {
        name: "/copy",
        aliases: &[],
        description: "Copy the last response to the clipboard",
        args: ArgSpec::Optional,
        advertised: true,
        run: ChatWidget::cmd_copy,
    },
    SlashCommand {
        name: "/theme",
        aliases: &[],
        description: "Change the color theme",
        args: ArgSpec::None,
        advertised: true,
        run: ChatWidget::cmd_theme,
    },
    SlashCommand {
        name: "/color",
        aliases: &[],
        description: "Set the session accent color",
        args: ArgSpec::Optional,
        advertised: true,
        run: ChatWidget::cmd_color,
    },
    SlashCommand {
        name: "/vim",
        aliases: &[],
        description: "Toggle vim editing mode",
        args: ArgSpec::None,
        advertised: true,
        run: ChatWidget::cmd_vim,
    },
    SlashCommand {
        name: "/clear",
        aliases: &[],
        description: "Clear the conversation",
        args: ArgSpec::None,
        advertised: true,
        run: ChatWidget::cmd_clear,
    },
    SlashCommand {
        name: "/exit",
        aliases: &["/quit"],
        description: "Exit LingXi",
        args: ArgSpec::None,
        advertised: true,
        run: cmd_exit,
    },
    SlashCommand {
        name: "/image",
        aliases: &[],
        description: "Attach an image file by path",
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
        args: ArgSpec::None,
        advertised: true,
        run: ChatWidget::cmd_init,
    },
    SlashCommand {
        name: "/init-verifiers",
        aliases: &[],
        description: "Create verifier skill(s) for automated verification of code changes",
        args: ArgSpec::None,
        advertised: true,
        run: ChatWidget::cmd_init_verifiers,
    },
    SlashCommand {
        name: "/commit",
        aliases: &[],
        description: "Create a git commit",
        args: ArgSpec::Optional,
        advertised: true,
        run: ChatWidget::cmd_commit,
    },
    SlashCommand {
        name: "/commit-push-pr",
        aliases: &[],
        description: "Commit, push, and open a PR",
        args: ArgSpec::Optional,
        advertised: true,
        run: ChatWidget::cmd_commit_push_pr,
    },
    SlashCommand {
        name: "/review",
        aliases: &[],
        description: "Review a pull request",
        args: ArgSpec::Optional,
        advertised: true,
        run: ChatWidget::cmd_review,
    },
    SlashCommand {
        name: "/security-review",
        aliases: &[],
        description: "Complete a security review of the pending changes on the current branch",
        args: ArgSpec::None,
        advertised: true,
        run: ChatWidget::cmd_security_review,
    },
    SlashCommand {
        name: "/statusline",
        aliases: &[],
        description: "Set up LingXi's status line UI",
        args: ArgSpec::Optional,
        advertised: true,
        run: ChatWidget::cmd_statusline,
    },
    SlashCommand {
        name: "/insights",
        aliases: &[],
        description: "Generate a report analyzing your LingXi sessions",
        args: ArgSpec::Optional,
        advertised: true,
        run: ChatWidget::cmd_insights,
    },
    SlashCommand {
        name: "/version",
        aliases: &[],
        description: "Print version information",
        args: ArgSpec::None,
        advertised: true,
        run: ChatWidget::cmd_version,
    },
    SlashCommand {
        name: "/release-notes",
        aliases: &[],
        description: "View release notes",
        args: ArgSpec::None,
        advertised: true,
        run: ChatWidget::cmd_release_notes,
    },
    SlashCommand {
        name: "/stickers",
        aliases: &[],
        description: "Order LingXi stickers",
        args: ArgSpec::None,
        advertised: true,
        run: ChatWidget::cmd_stickers,
    },
    SlashCommand {
        name: "/autocompact",
        aliases: &[],
        description: "Configure the auto-compact window size",
        args: ArgSpec::Optional,
        advertised: true,
        run: ChatWidget::cmd_autocompact,
    },
    SlashCommand {
        name: "/keybindings",
        aliases: &[],
        description: "Open or create your keybindings configuration file",
        args: ArgSpec::None,
        advertised: true,
        run: ChatWidget::cmd_keybindings,
    },
    SlashCommand {
        name: "/terminal-setup",
        aliases: &[],
        description: "Install Shift+Enter key binding for newlines",
        args: ArgSpec::None,
        // Statically advertised, but RUNTIME-hidden by `is_runtime_hidden`
        // (consulted in `advertised()`) when the active terminal natively
        // supports CSI-u / the Kitty keyboard protocol — mirroring claude-code's
        // `isHidden: env.terminal in NATIVE_CSIU_TERMINALS` (index.ts set:
        // Ghostty/Kitty/iTerm2/WezTerm). Typing it still dispatches.
        advertised: true,
        run: ChatWidget::cmd_terminal_setup,
    },
    SlashCommand {
        name: "/skill-doctor",
        aliases: &[],
        description: "Show which loaded skills are unused and costing context",
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
        description: "Show current context usage",
        args: ArgSpec::None,
        advertised: true,
        run: ChatWidget::cmd_context,
    },
    SlashCommand {
        name: "/files",
        aliases: &[],
        description: "List all files currently in context",
        args: ArgSpec::None,
        advertised: true,
        run: ChatWidget::cmd_files,
    },
    SlashCommand {
        name: "/usage",
        aliases: &[],
        description: "Show current session usage",
        args: ArgSpec::None,
        advertised: true,
        run: ChatWidget::cmd_usage,
    },
    SlashCommand {
        name: "/effort",
        aliases: &[],
        description: "Set effort level for model usage",
        args: ArgSpec::Optional,
        advertised: true,
        run: ChatWidget::cmd_effort,
    },
    SlashCommand {
        name: "/goal",
        aliases: &[],
        description: "Set a goal — keep working until the condition is met",
        args: ArgSpec::Optional,
        advertised: true,
        run: ChatWidget::cmd_goal,
    },
    SlashCommand {
        name: "/fork",
        aliases: &[],
        description: "Fork the conversation into a background agent",
        // Optional (NOT Required): a bare `/fork` must reach the handler so it
        // renders its own "Usage: /fork <directive>" line rather than falling
        // through as an LLM prompt.
        args: ArgSpec::Optional,
        advertised: true,
        run: ChatWidget::cmd_fork,
    },
    SlashCommand {
        name: "/recap",
        aliases: &[],
        description: "Generate a one-line session recap now",
        args: ArgSpec::None,
        advertised: true,
        run: ChatWidget::cmd_recap,
    },
    SlashCommand {
        name: "/btw",
        aliases: &[],
        description: "Ask a quick side question without interrupting the main conversation",
        // Optional (NOT Required): a bare `/btw` must reach the handler so it
        // renders "Usage: /btw <your question>" rather than falling through as
        // an LLM prompt (same rationale as `/fork`).
        args: ArgSpec::Optional,
        advertised: true,
        run: ChatWidget::cmd_btw,
    },
    SlashCommand {
        name: "/rename",
        aliases: &[],
        description: "Rename the current conversation",
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
        args: ArgSpec::None,
        advertised: true,
        run: ChatWidget::cmd_reload_skills,
    },
    SlashCommand {
        name: "/compact",
        aliases: &[],
        description: "Free up context by summarizing the conversation so far",
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
        args: ArgSpec::None,
        advertised: false,
        run: ChatWidget::cmd_stop,
    },
];

/// The advertised registry entries in popup/help order.
pub fn advertised() -> impl Iterator<Item = &'static SlashCommand> {
    BUILTIN
        .iter()
        .filter(|command| command.advertised && !is_runtime_hidden(command.name))
}

/// Runtime `isHidden` gate for statically-`advertised` rows whose palette /
/// `/help` visibility depends on the live environment, mirroring claude-code's
/// per-command `isHidden` predicate. Currently only `/terminal-setup`, which
/// claude-code hides when the active terminal already parses CSI-u / the Kitty
/// keyboard protocol (Ghostty/Kitty/iTerm2/WezTerm) and so needs no Shift+Enter
/// binding installed. `command_items` / `help_lines` derive from `advertised()`,
/// so both surfaces honor this gate uniformly.
#[must_use]
pub fn is_runtime_hidden(name: &str) -> bool {
    name == "/terminal-setup" && tui_core::terminal_setup::terminal_natively_supports_csiu()
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
                !command.description.trim().is_empty(),
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
        // advertised command in registry order with its description.
        let items = command_items("/");
        assert_eq!(items.len(), advertised().count());
        for (item, command) in items.iter().zip(advertised()) {
            assert_eq!(item.insert, command.name);
            assert_eq!(item.desc, command.description);
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
                assert!(
                    help_text.contains(command.description),
                    "{} description missing from /help",
                    command.name
                );
            } else {
                assert!(
                    !help_text.contains(command.name),
                    "unadvertised {} leaked into /help",
                    command.name
                );
            }
        }
    }

    /// Deliberately dropped commands stay dropped: the iocraft backend's
    /// `/tasks` has no data source on this backend, so it must not be
    /// registered (plan Phase 8 step 4: never advertise "not implemented").
    #[test]
    fn dropped_commands_are_not_registered() {
        assert!(resolve("/tasks").is_none());
        assert!(!BUILTIN.iter().any(|c| c.name == "/tasks"));
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
        assert_eq!(resolve("/permissions").expect("registered").0.name, "/permissions");
        assert_eq!(
            resolve("/allowed-tools").expect("alias").0.name,
            "/permissions"
        );
        // /resume is registered with its claude-code alias.
        assert_eq!(resolve("/resume").expect("registered").0.name, "/resume");
        assert_eq!(resolve("/continue").expect("alias").0.name, "/resume");
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
}
