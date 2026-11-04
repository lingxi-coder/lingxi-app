package com.lingxi.code.conversation

import com.lingxi.code.bindings.client.slashCommandArgumentCount

/** Presentation metadata stays independent from native FFI loading in the editor. */
data class ConversationSlashCommand(
    val name: String,
    val aliases: List<String> = emptyList(),
    val argumentHint: String? = null,
    val argumentNames: List<String> = emptyList(),
    val description: String = "",
    val source: String = "builtin",
    val hidden: Boolean = false,

)

/** Claude Code's suffix is a display hint; it is never part of the sent draft. */
internal fun slashArgumentHint(
    draft: String,
    cursorOffset: Int,
    commands: List<ConversationSlashCommand>,
    argumentCount: (String) -> Int? = { slashCommandArgumentCount(it)?.toInt() },
): String? {
    if (cursorOffset <= 0 || !draft.startsWith('/') || !draft.endsWith(' ')) return null
    val separator = draft.indexOf(' ')
    if (separator < 0) return null
    val token = draft.substring(1, separator)
    if (!token.all { it in 'a'..'z' || it in 'A'..'Z' || it in '0'..'9' || it in ".:_-" }) return null
    val command = commands.firstOrNull { it.name == token }
        ?: commands.firstOrNull { token in it.aliases }
        ?: return null
    if (separator == draft.lastIndex && !command.argumentHint.isNullOrEmpty()) return command.argumentHint
    if (command.argumentNames.isEmpty()) return null
    val count = argumentCount(draft.substring(separator + 1)) ?: return null
    return command.argumentNames.drop(count).joinToString(" ") { "[$it]" }.takeIf { it.isNotEmpty() }
}

data class ConversationCommandOutput(val command: String, val text: String, val isError: Boolean)

/** Echo task controls immediately; unknown commands wait for TurnStarted. */
internal fun slashCommandEchoesRequest(raw: String): Boolean {
    val parts = raw.trim().split(Regex("\\s+"), limit = 2)
    val name = parts.firstOrNull()?.lowercase()
    val args = parts.getOrNull(1).orEmpty().trim()
    return name in setOf("/compact", "/goal", "/fusion") ||
        (name == "/plan" && args.isNotEmpty() && args !in setOf("open", "share"))
}

internal enum class SlashMessagePlacement { Messages, Temporary, Conditional }
internal fun ConversationSlashCommand.messagePlacement(): SlashMessagePlacement {
    if (source != "builtin") return SlashMessagePlacement.Conditional
    return when (name.lowercase()) {
        "compact", "goal", "fusion", "init", "insights", "security-review", "team-onboarding" -> SlashMessagePlacement.Messages
        "help", "status", "usage", "cost", "stats", "recap", "context", "doctor", "model", "permissions", "effort", "fast", "theme", "config", "copy", "clear", "resume", "agents", "mcp", "skills", "tasks", "hooks", "files", "workflows", "export", "rename", "reload-skills", "reload-plugins", "autocompact", "brief", "diff", "output-style", "powerup", "stop" -> SlashMessagePlacement.Temporary
        else -> SlashMessagePlacement.Conditional
    }
}

internal fun slashCommandSuggestions(raw: String, commands: List<ConversationSlashCommand>): List<ConversationSlashCommand> {
    if (!raw.startsWith('/') || raw.drop(1).any { it.isWhitespace() || it == '/' }) return emptyList()
    val query = raw.drop(1)
    return commands.filter { command ->
        val exact = command.name.equals(query, true) || command.aliases.any { it.equals(query, true) }
        if (command.hidden) exact
        else exact || command.name.contains(query, true) || command.aliases.any { it.contains(query, true) } || command.description.contains(query, true)
    }.sortedWith(compareBy<ConversationSlashCommand> { !it.name.startsWith(query, true) }.thenBy { it.name }).take(100)
}
