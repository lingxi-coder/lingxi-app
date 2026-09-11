package com.lingxi.code.conversation

import android.graphics.Bitmap
import androidx.activity.ComponentActivity
import androidx.activity.enableEdgeToEdge
import android.view.WindowManager
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.ime
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.safeDrawingPadding
import androidx.compose.ui.Modifier
import androidx.compose.runtime.SideEffect
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.mutableStateOf
import androidx.compose.ui.graphics.asAndroidBitmap
import androidx.compose.ui.platform.LocalConfiguration
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.test.*
import androidx.compose.ui.test.junit4.createAndroidComposeRule
import androidx.compose.ui.unit.Density
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import com.lingxi.code.R
import com.lingxi.code.components.LXIconName
import com.lingxi.code.model.EngineModelCatalog
import com.lingxi.code.model.Message
import com.lingxi.code.model.Role
import com.lingxi.code.model.SessionRef
import com.lingxi.code.theme.LingXiTheme
import org.junit.Assert.assertEquals
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith
import java.io.File

/** Real native chat layout driven only by immutable fixtures: no engine or credentials. */
@RunWith(AndroidJUnit4::class)
class DesktopConversationParityUiTest {
    @get:Rule val rule = createAndroidComposeRule<ComponentActivity>()

    private val first = ToolCallUi(
        id = "read-one", tool = "Read", status = AgentToolStatus.Completed,
        header = ToolHeaderUi(ToolVerbUi.Read, "ReadFile", "README.md", title = "ReadFile(README.md)"),
        display = ToolResultDisplayUi(body = "Read output", bodyLines = 1),
    )
    private val last = ToolCallUi(
        id = "write-two", tool = "Write", status = AgentToolStatus.Completed,
        header = ToolHeaderUi(ToolVerbUi.Create, "WriteFile", "result.kt", title = "WriteFile(result.kt)"),
        display = ToolResultDisplayUi(body = "Verified tool output", bodyLines = 1),
    )

    private fun fixture(running: Boolean = false) = ChatState(
        session = SessionRef("parity-fixture", "Conversation parity"),
        model = EngineModelCatalog.pending,
        streaming = running,
        agentRun = if (running) AgentRunState(1, tools = listOf(AgentToolRunState(
            last.id, last.tool, status = AgentToolStatus.Running, header = last.header,
        ))) else null,
        messages = listOf(
            Message(Role.User, "Show the implementation.", id = "user"),
            Message(Role.Ai, "", id = "assistant", blocks = listOf(
                MessageContent.Text("## Ready\n**Readable *native* output.**\n\n| File | State |\n| --- | --- |\n| result.kt | Ready |\n\n```kotlin\nval answer = 42\n```"),
                MessageContent.Tool(first),
                MessageContent.Tool(last.copy(status = if (running) AgentToolStatus.Running else AgentToolStatus.Completed)),
            )),
        ),
        sessionAgents = listOf(SessionAgentUi("fixture-verifier-42", "Verifier", "reviewer", "Local model", "completed", "Checks passed")),
    )

    @Composable
    private fun FixtureScreen(state: ChatState, dark: Boolean, onToggle: (String) -> Unit) {
        LingXiTheme(darkTheme = dark) {
            ChatScreen(state = state, onSend = {}, onNewChat = {}, onSelectModel = {},
                isDark = dark, onToggleTheme = {}, onToggleToolCall = onToggle)
        }
    }

    @Test fun phoneLightDarkAndDisclosureRetainLastToolPresentation() {
        val dark = mutableStateOf(false)
        val state = mutableStateOf(fixture())
        rule.setContent { FixtureScreen(state.value, dark.value) { id ->
            val current = state.value.expandedToolCalls
            state.value = state.value.copy(expandedToolCalls = if (id in current) current - id else current + id)
        } }
        assertEquals(LXIconName.Edit, toolIconName(last.header?.verb))
        rule.onNodeWithText("ReadFile(README.md)").assertDoesNotExist()
        rule.onNodeWithText("WriteFile(result.kt)").assertExists()
        rule.onNodeWithText("val answer = 42").assertExists()
        rule.onNodeWithText("Verifier").assertExists()
        for (mode in listOf(false, true)) {
            rule.runOnIdle { dark.value = mode }
            rule.onNode(hasScrollToIndexAction()).performScrollToIndex(0)
            rule.onNodeWithText("Show the implementation.").assertIsDisplayed()
            capture("android-conversation-${if (mode) "dark" else "light"}")
        }
        rule.onNode(hasScrollToIndexAction()).performScrollToNode(hasTestTag("conversation.tool.group"))
        rule.onNodeWithTag("conversation.tool.group").performClick()
        rule.onNodeWithText("ReadFile(README.md)").assertExists()
        rule.onAllNodesWithText("WriteFile(result.kt)").onLast().performClick()
        rule.onNodeWithText("Verified tool output").assertIsDisplayed()
        val wide = rule.activity.resources.configuration.screenWidthDp >= 840
        rule.onAllNodes(isDialog()).assertCountEquals(if (wide) 0 else 1)
        capture("android-conversation-${if (wide) "tablet" else "phone"}-tool-detail", dialog = !wide)
        val context = InstrumentationRegistry.getInstrumentation().targetContext
        rule.onNodeWithContentDescription(context.getString(R.string.chat_run_collapse)).performClick()
        rule.onNode(hasScrollToIndexAction()).performScrollToNode(hasTestTag("conversation.tool.group"))
        rule.onNodeWithTag("conversation.tool.group").performClick()
        rule.onNodeWithText("ReadFile(README.md)").assertDoesNotExist()
        rule.onNodeWithText("Verified tool output").assertDoesNotExist()
    }

    @Test fun runningToolsHideSettledCalls() {
        rule.setContent { FixtureScreen(fixture(running = true), dark = true, onToggle = {}) }
        rule.onNode(hasScrollToIndexAction()).performScrollToNode(hasText("WriteFile(result.kt)"))
        rule.onNodeWithText("WriteFile(result.kt)").assertIsDisplayed()
        rule.onNodeWithText("ReadFile(README.md)").assertDoesNotExist()
        rule.onNodeWithTag("conversation.tool.group").assertDoesNotExist()
        capture("android-conversation-running")
    }

    @Test fun wideToolAndAgentDetailsStayBesideConversation() {
        val initial = fixture().copy(expandedToolCalls = setOf("tool-group:read-one"))
        rule.setContent {
            val density = LocalDensity.current
            // Use actual tablet density when available; phones can still exercise the split path.
            val previewDensity = if (LocalConfiguration.current.screenWidthDp >= 840) density
                else Density(density.density * 0.35f, density.fontScale)
            CompositionLocalProvider(LocalDensity provides previewDensity) {
                FixtureScreen(initial, dark = false, onToggle = {})
            }
        }
        rule.onAllNodesWithText("WriteFile(result.kt)").onLast().performClick()
        rule.onNodeWithText("Verified tool output").assertIsDisplayed()
        rule.onNodeWithText("Conversation parity").assertIsDisplayed()
        rule.onAllNodes(isDialog()).assertCountEquals(0)
        capture("android-conversation-wide-tool-detail")
        rule.onNodeWithText("Verifier").performClick()
        rule.onNodeWithText("fixture-verifier-42", substring = true).assertIsDisplayed()
        rule.onNodeWithText("Conversation parity").assertIsDisplayed()
        rule.onAllNodes(isDialog()).assertCountEquals(0)
        capture("android-conversation-wide-agent-detail")
    }

    @Test fun keyboardDraftAndHistoryAnchorSurviveIncomingMessages() {
        rule.runOnUiThread {
            rule.activity.enableEdgeToEdge()
            rule.activity.window.setSoftInputMode(WindowManager.LayoutParams.SOFT_INPUT_ADJUST_RESIZE)
        }
        val state = mutableStateOf(fixture().copy(
            messages = List(35) { Message(Role.User, "History $it", id = "history-$it") },
            sessionAgents = emptyList(),
        ))
        val draft = mutableStateOf("")
        val sent = mutableListOf<String>()
        val imeVisible = java.util.concurrent.atomic.AtomicBoolean(false)
        rule.setContent {
            LingXiTheme(darkTheme = false) {
                val keyboardBottom = WindowInsets.ime.getBottom(LocalDensity.current)
                SideEffect { imeVisible.set(keyboardBottom > 0) }
                ChatScreen(state.value, onSend = { sent += it }, onNewChat = {}, onSelectModel = {},
                    isDark = false, onToggleTheme = {}, draft = draft.value, modifier = Modifier.safeDrawingPadding().imePadding(),
                    onDraftChange = { draft.value = it })
            }
        }
        rule.onNode(hasSetTextAction()).performClick().performTextInput("Draft survives history")
        rule.waitUntil(timeoutMillis = 10_000) { imeVisible.get() }
        rule.onNode(hasScrollToIndexAction()).performScrollToIndex(0)
        rule.onNodeWithText("History 0").assertIsDisplayed()
        rule.runOnIdle {
            state.value = state.value.copy(messages = state.value.messages + Message(Role.Ai, "New reply", id = "incoming"))
        }
        rule.onNodeWithText("History 0").assertIsDisplayed()
        rule.onNode(hasSetTextAction()).assertTextContains("Draft survives history")
        capture("android-conversation-keyboard-history")
        rule.onNode(hasSetTextAction()).performImeAction()
        rule.runOnIdle {
            assertEquals(listOf("Draft survives history"), sent)
            assertEquals("", draft.value)
        }
    }

    private fun capture(name: String, dialog: Boolean = false) {
        rule.waitForIdle()
        val context = InstrumentationRegistry.getInstrumentation().targetContext
        val directory = requireNotNull(context.getExternalFilesDir("parity-screenshots")).apply { mkdirs() }
        val node = if (dialog) rule.onNode(isDialog()) else rule.onRoot()
        File(directory, "$name.png").outputStream().use {
            node.captureToImage().asAndroidBitmap().compress(Bitmap.CompressFormat.PNG, 100, it)
        }
        if (name.endsWith("keyboard-history")) {
            File(directory, "android-conversation-keyboard-system.png").outputStream().use {
                requireNotNull(InstrumentationRegistry.getInstrumentation().uiAutomation.takeScreenshot())
                    .compress(Bitmap.CompressFormat.PNG, 100, it)
            }
        }
    }
}
