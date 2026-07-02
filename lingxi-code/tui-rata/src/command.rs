//! The slash-command registry (plan Phase 6 step 9 — minimal for now; plan
//! Phase 8 completes it into the single source for completion metadata, the
//! help listing, and dispatch).
//!
//! [`resolve`] maps a submitted `/command [args]` buffer to its registered
//! [`SlashCommand`]; [`crate::chat_widget::ChatWidget::handle_slash`] then
//! runs the entry's dispatch fn — there is no hard-coded command `match` in
//! the app. Unknown commands resolve to `None` and fall through as a normal
//! prompt (locked behavior). Until Phase 8 unifies the sources, [`BUILTIN`]
//! must stay in sync with
//! [`crate::bottom_pane::completion_view::COMMANDS`] (test-enforced below).

use crate::chat_widget::{ChatOutcome, ChatWidget};

/// One registered slash command: completion/help metadata plus its dispatch.
pub struct SlashCommand {
    /// Canonical `/name` the user types.
    pub name: &'static str,
    /// Alternate names that dispatch identically (`/quit` → `/exit`).
    pub aliases: &'static [&'static str],
    /// One-line description (the completion popup's right column).
    pub description: &'static str,
    /// Whether the command requires trailing arguments (`/image <path>`).
    /// Arg-less commands reject trailing text; arg-taking commands reject an
    /// empty tail — either mismatch falls through as a normal prompt.
    pub accepts_args: bool,
    /// Whether the command is advertised in the completion popup.
    pub advertised: bool,
    /// The handler run on the widget when the command matches. Receives the
    /// trimmed argument tail (`""` for arg-less commands).
    pub run: fn(&mut ChatWidget, &str) -> ChatOutcome,
}

/// Every slash command the ratatui backend currently handles. Advertised
/// entries mirror [`crate::bottom_pane::completion_view::COMMANDS`] in order,
/// name, and description (test-enforced until plan Phase 8 makes this
/// registry the single source).
pub const BUILTIN: &[SlashCommand] = &[
    SlashCommand {
        name: "/help",
        aliases: &[],
        description: "Show shortcuts and commands",
        accepts_args: false,
        advertised: true,
        run: ChatWidget::cmd_help,
    },
    SlashCommand {
        name: "/model",
        aliases: &[],
        description: "Switch the active model",
        accepts_args: false,
        advertised: true,
        run: ChatWidget::cmd_model,
    },
    SlashCommand {
        name: "/doctor",
        aliases: &[],
        description: "Show diagnostics",
        accepts_args: false,
        advertised: true,
        run: ChatWidget::cmd_doctor,
    },
    SlashCommand {
        name: "/mcp",
        aliases: &[],
        description: "List MCP servers",
        accepts_args: false,
        advertised: true,
        run: ChatWidget::cmd_mcp,
    },
    SlashCommand {
        name: "/hooks",
        aliases: &[],
        description: "List hooks",
        accepts_args: false,
        advertised: true,
        run: ChatWidget::cmd_hooks,
    },
    SlashCommand {
        name: "/agents",
        aliases: &[],
        description: "List agents",
        accepts_args: false,
        advertised: true,
        run: ChatWidget::cmd_agents,
    },
    SlashCommand {
        name: "/vim",
        aliases: &[],
        description: "Toggle vim editing mode",
        accepts_args: false,
        advertised: true,
        run: ChatWidget::cmd_vim,
    },
    SlashCommand {
        name: "/clear",
        aliases: &[],
        description: "Clear the conversation",
        accepts_args: false,
        advertised: true,
        run: ChatWidget::cmd_clear,
    },
    SlashCommand {
        name: "/exit",
        aliases: &["/quit"],
        description: "Exit LingXi",
        accepts_args: false,
        advertised: true,
        run: cmd_exit,
    },
    SlashCommand {
        name: "/image",
        aliases: &[],
        description: "Attach an image file by path",
        accepts_args: true,
        advertised: false,
        run: ChatWidget::cmd_image,
    },
];

/// `/exit` (alias `/quit`): exit the app. A free function (not a
/// `ChatWidget` method) because it touches no widget state.
fn cmd_exit(_widget: &mut ChatWidget, _args: &str) -> ChatOutcome {
    ChatOutcome::Quit
}

/// Resolve a submitted composer buffer to `(command, trimmed args)`.
///
/// `None` (the buffer falls through as a normal prompt) when the input is not
/// `/`-led, names no registered command or alias, passes arguments to an
/// arg-less command, or omits them for an arg-taking one — all locked
/// pre-registry behavior.
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
    if command.accepts_args && args.is_empty() {
        return None;
    }
    if !command.accepts_args && !args.is_empty() {
        return None;
    }
    Some((command, args))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bottom_pane::completion_view::COMMANDS;

    #[test]
    fn advertised_registry_matches_completion_popup_metadata() {
        let advertised: Vec<(&str, &str)> = BUILTIN
            .iter()
            .filter(|command| command.advertised)
            .map(|command| (command.name, command.description))
            .collect();
        assert_eq!(
            advertised,
            COMMANDS.to_vec(),
            "registry and completion metadata must stay in sync (same order, \
             names, and descriptions) until plan Phase 8 unifies the source"
        );
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
