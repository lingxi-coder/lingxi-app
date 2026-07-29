/*
 * Copyright (C) 2026 LingXi contributors
 * SPDX-License-Identifier: GPL-3.0-only
 */
package com.lingxi.code.shell

enum class ShellStream {
    STDOUT,
    STDERR,
    PTY,
    SYSTEM,
}

enum class ShellTaskStatus {
    RUNNING,
    SUCCEEDED,
    FAILED,
    TIMED_OUT,
    CANCELLED,
    ;

    val isTerminal: Boolean
        get() = this != RUNNING
}

/**
 * One ordered shell event. Sequence numbers are scoped to (sessionId, taskId).
 * Raw bytes are retained for PTY consumers; text is the host-decoded display
 * form and may be absent for incomplete multibyte chunks.
 */
class ShellStreamEvent(
    val sessionId: String,
    val taskId: String,
    val sequence: Long,
    val stream: ShellStream,
    bytes: ByteArray = byteArrayOf(),
    val text: String? = null,
    val status: ShellTaskStatus = ShellTaskStatus.RUNNING,
    val exitCode: Int? = null,
    val durationMs: Long? = null,
) {
    val bytes: ByteArray = bytes.copyOf()

    init {
        require(sessionId.isNotBlank()) { "sessionId must not be blank" }
        require(taskId.isNotBlank()) { "taskId must not be blank" }
        require(sequence >= 0) { "sequence must be non-negative" }
        require(durationMs == null || durationMs >= 0) { "duration must be non-negative" }
        require(!status.isTerminal || durationMs != null) {
            "terminal events must carry durationMs"
        }
        require(
            status != ShellTaskStatus.SUCCEEDED &&
                status != ShellTaskStatus.FAILED ||
                exitCode != null,
        ) {
            "successful and failed process events must carry exitCode"
        }
    }
}

data class ShellTaskState(
    val sessionId: String,
    val taskId: String,
    val command: String,
    val cwd: String,
    val stdout: String = "",
    val stderr: String = "",
    val ptyText: String = "",
    val status: ShellTaskStatus = ShellTaskStatus.RUNNING,
    val exitCode: Int? = null,
    val durationMs: Long? = null,
    val lastSequence: Long = -1,
    val outputTruncated: Boolean = false,
)

/**
 * Folds only current, strictly ordered events. Once terminal, a task is
 * immutable, preventing late output or a second terminal event from leaking
 * into the next command card.
 */
fun reduceShellEvent(
    previous: ShellTaskState,
    event: ShellStreamEvent,
    maxCharsPerStream: Int = 256 * 1_024,
): ShellTaskState {
    require(maxCharsPerStream > 0) { "maxCharsPerStream must be positive" }
    if (event.sessionId != previous.sessionId || event.taskId != previous.taskId) return previous
    if (previous.status.isTerminal || event.sequence <= previous.lastSequence) return previous

    var truncated = previous.outputTruncated
    fun appendBounded(current: String, addition: String?): String {
        if (addition.isNullOrEmpty()) return current
        val combined = current + addition
        if (combined.length <= maxCharsPerStream) return combined
        truncated = true
        return combined.takeLast(maxCharsPerStream)
    }

    return previous.copy(
        stdout = if (event.stream == ShellStream.STDOUT) {
            appendBounded(previous.stdout, event.text)
        } else {
            previous.stdout
        },
        stderr = if (event.stream == ShellStream.STDERR) {
            appendBounded(previous.stderr, event.text)
        } else {
            previous.stderr
        },
        ptyText = if (event.stream == ShellStream.PTY) {
            appendBounded(previous.ptyText, event.text)
        } else {
            previous.ptyText
        },
        status = event.status,
        exitCode = event.exitCode ?: previous.exitCode,
        durationMs = event.durationMs ?: previous.durationMs,
        lastSequence = event.sequence,
        outputTruncated = truncated,
    )
}
