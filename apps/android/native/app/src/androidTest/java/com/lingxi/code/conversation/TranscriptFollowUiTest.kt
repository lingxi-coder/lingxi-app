package com.lingxi.code.conversation

import androidx.compose.foundation.gestures.scrollBy
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.systemBars
import androidx.compose.foundation.layout.windowInsetsPadding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.LazyListState
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.test.*
import androidx.compose.ui.unit.dp
import androidx.core.view.ViewCompat
import androidx.core.view.WindowCompat
import androidx.core.view.WindowInsetsCompat
import androidx.core.view.WindowInsetsControllerCompat
import androidx.test.ext.junit.runners.AndroidJUnit4
import com.lingxi.code.components.UiTags
import com.lingxi.code.model.EngineModelCatalog
import com.lingxi.code.model.Message
import com.lingxi.code.model.Role
import com.lingxi.code.model.SessionRef
import com.lingxi.code.testing.createComponentComposeRule
import com.lingxi.code.theme.LingXiTheme
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith

@RunWith(AndroidJUnit4::class)
class TranscriptFollowUiTest {
    @get:Rule val rule = createComponentComposeRule()

    @Test fun measuredTailBandIncludesOffscreenSentinelAndBottomPadding() {
        lateinit var list: LazyListState
        lateinit var scope: CoroutineScope
        var density = 1f
        rule.setContent {
            list = rememberLazyListState()
            scope = rememberCoroutineScope()
            density = LocalDensity.current.density
            LazyColumn(
                state = list,
                modifier = Modifier.height(160.dp).width(240.dp),
                contentPadding = PaddingValues(top = 18.dp, bottom = 8.dp),
            ) {
                item(key = "final-message") { Spacer(Modifier.height(600.dp)) }
                item(key = "bottom-anchor") { Spacer(Modifier.height(0.dp)) }
            }
            LaunchedEffect(Unit) { list.scrollToItem(1) }
        }
        rule.runOnIdle { scope.launch { list.scrollBy(-23f * density) } }
        rule.waitForIdle()
        rule.runOnIdle {
            val layout = list.layoutInfo
            // At 23 dp from the actual end the anchor is outside the viewport.
            assertEquals(0, layout.visibleItemsInfo.last().index)
            assertTrue(measuredAtBottom(list, (24f * density).toInt()))
        }
        rule.runOnIdle { scope.launch { list.scrollBy(-2f * density) } }
        rule.waitForIdle()
        rule.runOnIdle { assertFalse(measuredAtBottom(list, (24f * density).toInt())) }
    }

    @Test fun sameStreamingRowGrowthAndViewportResizeKeepTheNewestParagraphVisible() {
        val state = mutableStateOf(fixture().copy(streaming = true, streamingMessage = stream(12)))
        val height = mutableStateOf(600.dp)
        rule.setContent {
            LingXiTheme(darkTheme = false) {
                Box(Modifier.height(height.value)) {
                    ChatScreen(state.value, onSend = {}, onNewChat = {}, onSelectModel = {}, isDark = false)
                }
            }
        }
        rule.onNodeWithText("Latest streamed paragraph").assertIsDisplayed()
        rule.onNode(hasSetTextAction()).assertIsNotFocused()
        rule.runOnIdle { state.value = state.value.copy(streamingMessage = stream(45)) }
        rule.onNodeWithText("Latest streamed paragraph").assertIsDisplayed()
        rule.runOnIdle { height.value = 350.dp }
        rule.onNodeWithText("Latest streamed paragraph").assertIsDisplayed()
        rule.runOnIdle { height.value = 600.dp }
        rule.onNodeWithText("Latest streamed paragraph").assertIsDisplayed()
        rule.onNode(hasSetTextAction()).assertIsNotFocused()
        rule.onNodeWithTag(UiTags.CHAT_JUMP_TO_LATEST).assertDoesNotExist()
    }

    @Test fun userHistoryReadingSurvivesIncomingContentUntilExplicitJump() {
        val state = mutableStateOf(fixture())
        rule.setContent {
            LingXiTheme(darkTheme = false) {
                ChatScreen(state.value, onSend = {}, onNewChat = {}, onSelectModel = {}, isDark = false)
            }
        }
        traceStep("history: swiping")
        browseHistory()
        traceStep("history: measuring reading anchor")
        val marker = historyMarker()
        val before = marker.getBoundsInRoot().top
        traceStep("history: updating incoming content")
        rule.runOnIdle {
            state.value = state.value.copy(streaming = true, streamingMessage = stream(45))
        }
        traceStep("history: verifying reading anchor")
        assertEquals(before, marker.assertIsDisplayed().getBoundsInRoot().top)
        traceStep("history: clicking jump")
        rule.onNodeWithTag(UiTags.CHAT_JUMP_TO_LATEST).performClick()
        traceStep("history: checking tail")
        rule.onNodeWithText("Latest streamed paragraph").assertIsDisplayed()
        rule.onNodeWithTag(UiTags.CHAT_JUMP_TO_LATEST).assertDoesNotExist()
    }

    @Test fun linkedMessageWinsInitialTailAndNewSessionResetsFollowing() {
        val state = mutableStateOf(fixture())
        val link = mutableStateOf<String?>("uuid-0")
        rule.setContent {
            LingXiTheme(darkTheme = false) {
                ChatScreen(state.value, onSend = {}, onNewChat = {}, onSelectModel = {}, isDark = false,
                    linkedMessageUuid = link.value, onMessageLinkHandled = { link.value = null })
            }
        }
        rule.onNodeWithText("History 0").assertIsDisplayed()
        rule.runOnIdle { state.value = state.value.copy(streaming = true, streamingMessage = stream(20)) }
        rule.onNodeWithText("History 0").assertIsDisplayed()
        rule.runOnIdle { state.value = state.value.copy(session = SessionRef("other", "Other chat")) }
        rule.onNodeWithText("Latest streamed paragraph").assertIsDisplayed()
    }

    @Test fun realKeyboardOpenCloseKeepsStreamingTailAndUnsentDraftFocused() {
        val state = mutableStateOf(fixture().copy(streaming = true, streamingMessage = stream(12)))
        val draft = mutableStateOf("")
        rule.runOnUiThread {
            WindowCompat.setDecorFitsSystemWindows(rule.activity.window, false)
            rule.activity.window.setSoftInputMode(android.view.WindowManager.LayoutParams.SOFT_INPUT_ADJUST_RESIZE)
        }
        rule.setContent {
            LingXiTheme(darkTheme = false) {
                Box(Modifier.fillMaxSize().windowInsetsPadding(WindowInsets.systemBars).imePadding()) {
                    ChatScreen(state.value, onSend = {}, onNewChat = {}, onSelectModel = {}, isDark = false,
                        draft = draft.value, onDraftChange = { draft.value = it })
                }
            }
        }
        val input = rule.onNode(hasSetTextAction())
        input.performClick().performTextInput("Unsent scroll draft")
        rule.waitUntil(5_000) { imeVisible() }
        rule.runOnIdle { state.value = state.value.copy(streamingMessage = stream(45)) }
        rule.onNodeWithText("Latest streamed paragraph").assertIsDisplayed()
        input.assertIsFocused().assertTextContains("Unsent scroll draft")
        rule.runOnUiThread {
            WindowInsetsControllerCompat(rule.activity.window, rule.activity.window.decorView)
                .hide(WindowInsetsCompat.Type.ime())
        }
        rule.waitUntil(5_000) { !imeVisible() }
        rule.runOnIdle { state.value = state.value.copy(streamingMessage = stream(50)) }
        rule.onNodeWithText("Latest streamed paragraph").assertIsDisplayed()
        rule.onNodeWithTag(UiTags.CHAT_JUMP_TO_LATEST).assertDoesNotExist()
        input.assertTextContains("Unsent scroll draft")
    }

    @Test fun nativeFastFlingAtHistoryEdgeKeepsKeyboardClosedAndComposerResponsive() {
        rule.runOnUiThread {
            WindowCompat.setDecorFitsSystemWindows(rule.activity.window, false)
            rule.activity.window.setSoftInputMode(android.view.WindowManager.LayoutParams.SOFT_INPUT_ADJUST_RESIZE)
        }
        rule.setContent {
            LingXiTheme(darkTheme = false) {
                Box(Modifier.fillMaxSize().windowInsetsPadding(WindowInsets.systemBars).imePadding()) {
                    ChatScreen(fixture(), onSend = {}, onNewChat = {}, onSelectModel = {}, isDark = false)
                }
            }
        }
        val viewport = rule.onNode(hasScrollToIndexAction()).getBoundsInRoot()
        val input = rule.onNode(hasSetTextAction()).assertIsNotFocused().getBoundsInRoot()
        val density = rule.activity.resources.displayMetrics.density
        fun px(value: androidx.compose.ui.unit.Dp) = (value.value * density).toInt()
        val centerX = px((viewport.left + viewport.right) / 2)
        val height = viewport.bottom - viewport.top
        val ui = androidx.test.platform.app.InstrumentationRegistry.getInstrumentation().uiAutomation
        fun nativeInput(command: String) {
            // Real platform event times let the system IME finish its animation
            // independently of Compose's automatically advanced test clock.
            android.os.ParcelFileDescriptor.AutoCloseInputStream(ui.executeShellCommand(command)).use { it.readBytes() }
        }
        traceStep("native history: fast flings to oldest edge")
        repeat(5) {
            nativeInput("input swipe $centerX ${px(viewport.top + height / 8)} $centerX ${px(viewport.bottom - height / 8)} 200")
            rule.mainClock.advanceTimeBy(500)
            android.os.SystemClock.sleep(250)
        }
        rule.onNodeWithText("History 0").assertIsDisplayed()
        rule.onNode(hasSetTextAction()).assertIsNotFocused()
        assertFalse("Browsing history must not open the keyboard", imeVisible())
        traceStep("native history: opening composer after flings")
        nativeInput("input tap ${px(input.left + 20.dp)} ${px(input.top + 20.dp)}")
        rule.waitUntil(5_000) { imeVisible() }
        rule.onNode(hasSetTextAction()).assertIsFocused()
        rule.runOnUiThread {
            WindowInsetsControllerCompat(rule.activity.window, rule.activity.window.decorView)
                .hide(WindowInsetsCompat.Type.ime())
        }
        rule.waitUntil(5_000) { !imeVisible() }
    }

    @Test fun historyReadingSurvivesRealKeyboardAndQueuedSendReturnsToTail() {
        val state = mutableStateOf(fixture())
        val draft = mutableStateOf("")
        rule.runOnUiThread {
            WindowCompat.setDecorFitsSystemWindows(rule.activity.window, false)
            rule.activity.window.setSoftInputMode(android.view.WindowManager.LayoutParams.SOFT_INPUT_ADJUST_RESIZE)
        }
        rule.setContent {
            LingXiTheme(darkTheme = false) {
                Box(Modifier.fillMaxSize().windowInsetsPadding(WindowInsets.systemBars).imePadding()) {
                    ChatScreen(state.value,
                        onSend = { text -> state.value = state.value.copy(messages = state.value.messages + Message(Role.User, text)) },
                        onNewChat = {}, onSelectModel = {}, isDark = false,
                        draft = draft.value, onDraftChange = { draft.value = it })
                }
            }
        }
        traceStep("keyboard history: browsing")
        browseHistory()
        traceStep("keyboard history: measuring anchor")
        val marker = historyMarker()
        val before = marker.getBoundsInRoot().top
        traceStep("keyboard history: receiving stream")
        rule.runOnIdle { state.value = state.value.copy(streaming = true, streamingMessage = stream(45)) }
        val input = rule.onNode(hasSetTextAction())
        traceStep("keyboard history: opening keyboard and typing")
        input.performClick().performTextInput("Queued scroll prompt")
        rule.waitUntil(5_000) { imeVisible() }
        traceStep("keyboard history: checking anchor and sending")
        assertEquals(before, marker.assertIsDisplayed().getBoundsInRoot().top)
        rule.onNodeWithTag(UiTags.CHAT_JUMP_TO_LATEST).assertExists()
        rule.onNodeWithTag(UiTags.COMPOSER_SEND).performClick()
        rule.onNodeWithText("Latest streamed paragraph").assertIsDisplayed()
        rule.onNodeWithTag(UiTags.CHAT_JUMP_TO_LATEST).assertDoesNotExist()
        input.assertIsFocused()
        assertTrue(imeVisible())
    }

    @Test fun streamedReplyRemainsExpandedOnCompletionAndUserCanCollapseIt() {
        val live = stream(45).copy(streamed = true)
        val state = mutableStateOf(fixture().copy(streaming = true, streamingMessage = live))
        rule.setContent {
            LingXiTheme(darkTheme = false) {
                ChatScreen(state.value, onSend = {}, onNewChat = {}, onSelectModel = {}, isDark = false)
            }
        }
        rule.onNodeWithText("Latest streamed paragraph").assertIsDisplayed()
        rule.runOnIdle { state.value = state.value.copy(messages = state.value.messages + live, streaming = false, streamingMessage = null) }
        rule.onNodeWithText("Latest streamed paragraph").assertIsDisplayed()
        rule.onNodeWithTag("conversation.message.assistant.toggle").performClick()
        rule.onNodeWithText("Latest streamed paragraph").assertIsNotDisplayed()
    }

    @Test fun explicitCollapseSurvivesStreamingCompletion() {
        val live = stream(45).copy(streamed = true)
        val state = mutableStateOf(fixture().copy(streaming = true, streamingMessage = live))
        rule.setContent {
            LingXiTheme(darkTheme = false) {
                ChatScreen(state.value, onSend = {}, onNewChat = {}, onSelectModel = {}, isDark = false)
            }
        }
        rule.onNodeWithTag("conversation.message.assistant.toggle").performClick()
        rule.runOnIdle { state.value = state.value.copy(messages = state.value.messages + live, streaming = false, streamingMessage = null) }
        rule.onNodeWithText("Latest streamed paragraph").assertIsNotDisplayed()
        rule.onNodeWithTag("conversation.message.assistant.toggle").performClick()
        rule.onNodeWithText("Latest streamed paragraph").assertIsDisplayed()
    }

    @Test fun actualHistoryGestureInterruptsContinuousSameRowStreaming() {
        val state = mutableStateOf(fixture().copy(streaming = true, streamingMessage = stream(12)))
        rule.setContent {
            LingXiTheme(darkTheme = false) {
                ChatScreen(state.value, onSend = {}, onNewChat = {}, onSelectModel = {}, isDark = false)
            }
        }
        val job = CoroutineScope(Dispatchers.Default).launch {
            repeat(60) { index ->
                delay(100)
                withContext(Dispatchers.Main) { state.value = state.value.copy(streamingMessage = stream(13 + index)) }
            }
        }
        try {
            browseHistory()
            rule.onNodeWithTag(UiTags.CHAT_JUMP_TO_LATEST).assertExists()
            val marker = historyMarker()
            val before = marker.getBoundsInRoot().top
            rule.waitUntil(12_000) { job.isCompleted }
            assertEquals(before, marker.getBoundsInRoot().top)
            rule.onNodeWithTag(UiTags.CHAT_JUMP_TO_LATEST).performClick()
            rule.onNodeWithText("Latest streamed paragraph").assertIsDisplayed()
            rule.onNode(hasSetTextAction()).assertIsNotFocused()
        } finally { job.cancel() }
    }

    private fun imeVisible(): Boolean = rule.runOnIdle {
        ViewCompat.getRootWindowInsets(rule.activity.window.decorView)?.isVisible(WindowInsetsCompat.Type.ime()) == true
    }

    private fun traceStep(step: String) {
        androidx.test.platform.app.InstrumentationRegistry.getInstrumentation().sendStatus(
            2, android.os.Bundle().apply { putString("stream", "$step\n") },
        )
    }

    private fun historyMarker(): SemanticsNodeInteraction {
        val viewport = rule.onNode(hasScrollToIndexAction()).getBoundsInRoot()
        // Measure an actual visible history row without repeatedly overscrolling
        // the oldest edge (which also exercises the OS keyboard inset gesture).
        return (0 until 35).map { rule.onNodeWithText("History $it") }.first { row ->
            row.isDisplayed() && row.getBoundsInRoot().let { bounds ->
                bounds.top >= viewport.top + 8.dp && bounds.bottom <= viewport.bottom
            }
        }
    }

    private fun browseHistory() {
        // Slow drags park on a stable reading position within history.
        repeat(2) {
            rule.onNode(hasScrollToIndexAction()).performTouchInput { swipeDown(durationMillis = 1_000) }
        }
    }

    private fun fixture() = ChatState(
        session = SessionRef("follow-fixture", "Transcript following"),
        model = EngineModelCatalog.pending,
        messages = List(35) { Message(Role.User, "History $it", id = "history-$it", transcriptUuid = "uuid-$it") },
    )

    private fun stream(paragraphs: Int) = Message(
        Role.Ai,
        (0 until paragraphs).joinToString("\n\n") { "Streamed paragraph $it" } + "\n\nLatest streamed paragraph",
        id = "stable-streaming-row",
    )

    private fun measuredAtBottom(list: LazyListState, slackPx: Int): Boolean {
        val layout = list.layoutInfo
        val last = layout.visibleItemsInfo.lastOrNull()
        return TranscriptFollow.isAtBottom(
            totalItemsCount = layout.totalItemsCount,
            lastVisibleIndex = last?.index,
            lastVisibleEndOffset = last?.let { it.offset + it.size },
            viewportEndOffset = layout.viewportEndOffset,
            slackPx = slackPx,
            canScrollForward = list.canScrollForward,
            hasBottomAnchor = true,
            afterContentPadding = layout.afterContentPadding,
        )
    }
}
