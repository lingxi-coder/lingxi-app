package com.lingxi.code.conversation

import com.lingxi.code.bindings.client.slashCommandArgumentCount

/** Presentation metadata stays independent from native FFI loading in the editor. */
data class ConversationSlashCommand(
    val name: String,
    val aliases: List<String> = emptyList(),
    val argumentHint: String? = null,
    val argumentNames: List<String> = emptyList(),
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
