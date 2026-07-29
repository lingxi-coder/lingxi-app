package com.lingxi.code.conversation

enum class ShellToolStatus {
    Running,
    Completed,
    Failed,
    TimedOut,
    Cancelled,
}

/**
 * Structured, correlation-safe presentation state for one agent shell call.
 *
 * The engine's stable tool id is the identity. [sessionId] is filled by the
 * reducer (not trusted from tool JSON), preventing a result from an abandoned
 * session being rendered into the newly selected conversation.
 */
data class ShellToolCardState(
    val sessionId: String,
    val taskId: String,
    val command: String,
    val cwd: String? = null,
    val stdout: String = "",
    val stderr: String = "",
    val exitCode: Int? = null,
    val durationMs: Long? = null,
    val status: ShellToolStatus = ShellToolStatus.Running,
    val truncated: Boolean = false,
)

sealed interface ShellToolUpdate {
    val taskId: String

    data class Started(
        override val taskId: String,
        val command: String,
        val cwd: String?,
    ) : ShellToolUpdate

    data class Heartbeat(
        override val taskId: String,
        val elapsedMs: Long,
    ) : ShellToolUpdate

    data class Finished(
        override val taskId: String,
        val stdout: String,
        val stderr: String,
        val exitCode: Int?,
        val elapsedMs: Long?,
        val status: ShellToolStatus,
        val truncated: Boolean,
    ) : ShellToolUpdate
}

internal fun isShellTool(name: String): Boolean {
    val normalized = name.trim().lowercase()
    return normalized == "shell" || normalized == "bash" || normalized == "terminal"
}

internal fun shellStarted(
    id: String,
    inputJson: String,
): ShellToolUpdate.Started {
    return ShellToolUpdate.Started(
        taskId = id,
        command = inputJson.jsonString("command")?.takeIf(String::isNotBlank)
            ?: inputJson.jsonString("cmd")?.takeIf(String::isNotBlank)
            ?: inputJson,
        cwd = inputJson.jsonString("cwd")?.takeIf(String::isNotBlank),
    )
}

internal fun shellFinished(
    id: String,
    resultJson: String,
    isError: Boolean,
): ShellToolUpdate.Finished {
    val result = resultJson.jsonObject("data") ?: resultJson
    val timedOut = result.jsonBoolean("timed_out") == true
    val cancelled = result.jsonBoolean("cancelled") == true ||
        result.jsonBoolean("interrupted") == true
    val exitCode = result.jsonLong("exit_code")?.toInt()
        ?: result.jsonLong("code")?.toInt()
    val status = when {
        timedOut -> ShellToolStatus.TimedOut
        cancelled -> ShellToolStatus.Cancelled
        isError || (exitCode != null && exitCode != 0) -> ShellToolStatus.Failed
        else -> ShellToolStatus.Completed
    }
    return ShellToolUpdate.Finished(
        taskId = id,
        stdout = result.jsonString("stdout").orEmpty(),
        stderr = result.jsonString("stderr").orEmpty()
            .ifBlank { resultJson.jsonString("error").orEmpty() },
        exitCode = exitCode,
        elapsedMs = result.jsonLong("duration_ms")
            ?: result.jsonLong("elapsed_ms"),
        status = status,
        truncated = result.jsonBoolean("truncated") == true,
    )
}

private fun String.jsonObject(key: String): String? {
    val start = Regex("\"${Regex.escape(key)}\"\\s*:\\s*\\{").find(this)?.range?.last ?: return null
    var depth = 1
    var quoted = false
    var escaped = false
    for (index in start + 1 until length) {
        val char = this[index]
        if (quoted) {
            if (escaped) escaped = false
            else if (char == '\\') escaped = true
            else if (char == '"') quoted = false
        } else {
            when (char) {
                '"' -> quoted = true
                '{' -> depth += 1
                '}' -> {
                    depth -= 1
                    if (depth == 0) return substring(start + 1, index)
                }
            }
        }
    }
    return null
}

private fun String.jsonString(key: String): String? {
    val match = Regex("\"${Regex.escape(key)}\"\\s*:\\s*\"((?:\\\\.|[^\"\\\\])*)\"")
        .find(this) ?: return null
    return match.groupValues[1].decodeJsonString()
}

private fun String.jsonLong(key: String): Long? =
    Regex("\"${Regex.escape(key)}\"\\s*:\\s*(-?\\d+)")
        .find(this)
        ?.groupValues
        ?.get(1)
        ?.toLongOrNull()

private fun String.jsonBoolean(key: String): Boolean? =
    Regex("\"${Regex.escape(key)}\"\\s*:\\s*(true|false)", RegexOption.IGNORE_CASE)
        .find(this)
        ?.groupValues
        ?.get(1)
        ?.equals("true", ignoreCase = true)

private fun String.decodeJsonString(): String {
    val output = StringBuilder(length)
    var index = 0
    while (index < length) {
        val char = this[index++]
        if (char != '\\' || index >= length) {
            output.append(char)
            continue
        }
        when (val escaped = this[index++]) {
            '"', '\\', '/' -> output.append(escaped)
            'b' -> output.append('\b')
            'f' -> output.append('\u000C')
            'n' -> output.append('\n')
            'r' -> output.append('\r')
            't' -> output.append('\t')
            'u' -> {
                val end = (index + 4).coerceAtMost(length)
                val decoded = substring(index, end).takeIf { it.length == 4 }?.toIntOrNull(16)
                if (decoded != null) {
                    output.append(decoded.toChar())
                    index = end
                } else {
                    output.append("\\u")
                }
            }
            else -> output.append(escaped)
        }
    }
    return output.toString()
}
