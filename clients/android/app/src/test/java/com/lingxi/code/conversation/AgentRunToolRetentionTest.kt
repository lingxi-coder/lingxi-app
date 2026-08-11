package com.lingxi.code.conversation

import com.lingxi.code.R
import com.lingxi.code.model.EngineModelCatalog
import com.lingxi.code.model.Message
import com.lingxi.code.model.Role
import com.lingxi.code.model.SessionRef
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * The live run trace is the ONLY record of a turn's tool calls until
 * [ChatState.settleTurn] moves them into the transcript — the engine keeps
 * `ToolUse` out of the assistant message, which is precisely why `settleTurn`
 * exists. Anything the reducer drops is therefore deleted from the transcript
 * permanently, so the 50-row budget has to be a RENDER window and nothing else.
 *
 * Pure JVM: these only build data classes and call the pure reducer/settle
 * functions, so no Compose runtime and no native `.so` is involved.
 */
class AgentRunToolRetentionTest {

    private fun header(tool: String) = ToolHeaderUi(
        verb = ToolVerbUi.Read,
        label = "Read",
        primary = "src/$tool.rs",
        title = "Read(src/$tool.rs)",
    )

    private fun started(index: Int) = ReplyEvent.ToolActivity(
        label = "Read",
        id = "t$index",
        tool = "Read",
        status = AgentToolStatus.Running,
        header = header("t$index"),
    )

    private fun finished(index: Int) = ReplyEvent.ToolActivity(
        label = "Read",
        id = "t$index",
        tool = "Read",
        status = AgentToolStatus.Completed,
        display = ToolResultDisplayUi(headline = "Read $index lines"),
    )

    /** A run whose [count] calls all started and all returned. */
    private fun runOf(count: Int): AgentRunState {
        var run = AgentRunState(turnId = 1L)
        for (index in 1..count) run = run.reduceTool(started(index))
        for (index in 1..count) run = run.reduceTool(finished(index))
        return run
    }

    private fun chatState(run: AgentRunState) = ChatState(
        session = SessionRef("s", "标题"),
        messages = emptyList(),
        model = EngineModelCatalog.pending,
        agentRun = run,
    )

    // --- D1: the cap must never delete transcript content ------------------

    @Test
    fun `a sixty tool turn settles with every one of its rows`() {
        val run = runOf(60)
        assertEquals("the reducer must keep every row", 60, run.tools.size)

        val settled = chatState(run).settleTurn(
            run = run.finish(AgentRunOutcome.Completed),
            settling = Message(role = Role.Ai, text = "done"),
        )
        val absorbed = settled.messages.last().blocks.filterIsInstance<MessageContent.Tool>()
        assertEquals("all 60 calls must reach the transcript", 60, absorbed.size)
        // Including the OLDEST — a `takeLast` cap silently deleted calls 1-10.
        assertEquals("t1", absorbed.first().call.id)
        assertEquals("t60", absorbed.last().call.id)
        assertEquals(60, absorbed.map { it.call.id }.toSet().size)
    }

    @Test
    fun `a late result for a row outside the display window updates it in place`() {
        // The evicting cap made this APPEND a second, header-less row at the
        // bottom: a bare tool name, out of chronological order, that also
        // evicted one MORE of the oldest rows.
        var run = AgentRunState(turnId = 1L)
        for (index in 1..60) run = run.reduceTool(started(index))
        run = run.reduceTool(finished(1))

        assertEquals("no row may be appended for an id already present", 60, run.tools.size)
        val first = run.tools.first()
        assertEquals("t1", first.id)
        assertEquals(AgentToolStatus.Completed, first.status)
        assertNotNull("the call's header must survive the result event", first.header)
        assertNotNull(first.display)
    }

    // --- D1: the 50-row budget is a display window, and only that ----------

    @Test
    fun `the display window caps what is drawn without capping what is kept`() {
        val run = runOf(60)
        assertEquals(60, run.tools.size)
        assertEquals(MAX_TOOL_ROWS, run.toolDisplayWindow().size)
        assertEquals(60 - MAX_TOOL_ROWS, run.hiddenToolCount())
        // The window shows the NEWEST rows; the hidden ones are the oldest.
        assertEquals("t11", run.toolDisplayWindow().first().id)
        assertEquals("t60", run.toolDisplayWindow().last().id)
    }

    @Test
    fun `a short turn is drawn whole and announces nothing hidden`() {
        val run = runOf(3)
        assertEquals(3, run.toolDisplayWindow().size)
        assertEquals(0, run.hiddenToolCount())
        assertTrue(run.toolDisplayWindow() === run.tools)
    }

    // --- D2: an engine label override must survive localization -----------

    @Test
    fun `an engine label override is rendered verbatim, never localized away`() {
        // `WebSearch` and `WebFetch` share the Fetch verb and are told apart by
        // NOTHING but the label override.
        assertNull(
            "Web Search must not be replaced by the Fetch catalog entry",
            toolVerbLabelRes(
                ToolHeaderUi(verb = ToolVerbUi.Fetch, label = "Web Search", title = "Web Search"),
            ),
        )
        assertEquals(
            R.string.chat_tool_verb_fetch,
            toolVerbLabelRes(ToolHeaderUi(verb = ToolVerbUi.Fetch, label = "Fetch", title = "Fetch")),
        )

        // A Task's label is the subagent type — losing it loses which agent ran.
        assertNull(
            toolVerbLabelRes(
                ToolHeaderUi(verb = ToolVerbUi.Task, label = "code-reviewer", title = "code-reviewer"),
            ),
        )
        assertEquals(
            R.string.chat_tool_verb_task,
            toolVerbLabelRes(ToolHeaderUi(verb = ToolVerbUi.Task, label = "Task", title = "Task")),
        )
    }

    @Test
    fun `an uncounted shell verb shows its label instead of fabricating a count`() {
        // `REPL` is verb=Shell with NO count. `count ?: 1` used to render
        // "Running 1 shell command…" for a call that ran no shell command.
        assertNull(
            toolVerbLabelRes(
                ToolHeaderUi(verb = ToolVerbUi.Shell, label = "REPL", count = null, title = "REPL"),
            ),
        )
        assertEquals(
            R.string.chat_tool_verb_shell_label,
            toolVerbLabelRes(
                ToolHeaderUi(
                    verb = ToolVerbUi.Shell,
                    label = "Running 1 shell command…",
                    count = 1,
                    title = "Running 1 shell command…",
                ),
            ),
        )
    }

    @Test
    fun `a generic verb always renders the raw tool name`() {
        assertNull(
            toolVerbLabelRes(
                ToolHeaderUi(verb = ToolVerbUi.Generic, label = "search_issues", title = "search_issues"),
            ),
        )
    }

    @Test
    fun `the canonical english table mirrors the engine's ToolVerb english`() {
        // Hand-copied from `tui-core/src/tool_display/header.rs`. The two that
        // are NOT the enum name are exactly where a drifted copy would show up.
        assertEquals("Write", canonicalVerbEnglish(ToolVerbUi.Create))
        assertEquals("Update Todos", canonicalVerbEnglish(ToolVerbUi.Todo))
        assertNull(canonicalVerbEnglish(ToolVerbUi.Generic))
        assertNull(canonicalVerbEnglish(ToolVerbUi.Shell))
        // Every other verb has one, or a header could never be localized at all.
        val missing = ToolVerbUi.values()
            .filterNot { it == ToolVerbUi.Generic || it == ToolVerbUi.Shell }
            .filter { canonicalVerbEnglish(it) == null }
        assertEquals(emptyList<ToolVerbUi>(), missing)
    }
}
