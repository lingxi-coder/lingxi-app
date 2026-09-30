package com.lingxi.code.shell

import org.junit.Assert.assertEquals
import org.junit.Assert.assertSame
import org.junit.Assert.assertTrue
import org.junit.Test

class ShellStateTest {
    private val initial = ShellTaskState(
        sessionId = "session-a",
        taskId = "task-a",
        command = "echo hi",
        cwd = "/workspace",
    )

    @Test
    fun stdoutAndStderrRemainStrictlySeparated() {
        val withStdout = reduceShellEvent(
            initial,
            event(sequence = 0, stream = ShellStream.STDOUT, text = "out"),
        )
        val withBoth = reduceShellEvent(
            withStdout,
            event(sequence = 1, stream = ShellStream.STDERR, text = "err"),
        )

        assertEquals("out", withBoth.stdout)
        assertEquals("err", withBoth.stderr)
    }

    @Test
    fun staleForeignAndPostTerminalEventsAreIgnoredByIdentity() {
        val first = reduceShellEvent(initial, event(sequence = 4, text = "new"))
        assertSame(first, reduceShellEvent(first, event(sequence = 3, text = "stale")))
        assertSame(
            first,
            reduceShellEvent(
                first,
                event(sequence = 5, text = "foreign", sessionId = "session-b"),
            ),
        )

        val terminal = reduceShellEvent(
            first,
            event(
                sequence = 5,
                status = ShellTaskStatus.SUCCEEDED,
                exitCode = 0,
                durationMs = 12,
            ),
        )
        assertSame(terminal, reduceShellEvent(terminal, event(sequence = 6, text = "late")))
        assertEquals(ShellTaskStatus.SUCCEEDED, terminal.status)
    }

    @Test
    fun outputIsTailBoundedAndMarksTruncation() {
        val state = reduceShellEvent(
            initial,
            event(sequence = 0, text = "123456"),
            maxCharsPerStream = 4,
        )

        assertEquals("3456", state.stdout)
        assertTrue(state.outputTruncated)
    }

    private fun event(
        sequence: Long,
        stream: ShellStream = ShellStream.STDOUT,
        text: String? = null,
        status: ShellTaskStatus = ShellTaskStatus.RUNNING,
        exitCode: Int? = null,
        durationMs: Long? = null,
        sessionId: String = "session-a",
    ) = ShellStreamEvent(
        sessionId = sessionId,
        taskId = "task-a",
        sequence = sequence,
        stream = stream,
        text = text,
        status = status,
        exitCode = exitCode,
        durationMs = durationMs,
    )
}
