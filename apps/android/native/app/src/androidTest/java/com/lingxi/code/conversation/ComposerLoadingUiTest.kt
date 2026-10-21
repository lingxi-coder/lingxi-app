package com.lingxi.code.conversation

import androidx.compose.runtime.mutableStateOf
import androidx.compose.ui.semantics.SemanticsProperties
import androidx.compose.ui.test.SemanticsMatcher
import androidx.compose.ui.test.assert
import androidx.compose.ui.test.assertIsEnabled
import androidx.compose.ui.test.assertIsFocused
import androidx.compose.ui.test.assertIsNotEnabled
import androidx.compose.ui.test.assertTextContains
import androidx.compose.ui.test.hasSetTextAction
import androidx.compose.ui.test.junit4.v2.createComposeRule
import androidx.compose.ui.test.onNodeWithTag
import androidx.compose.ui.test.performClick
import androidx.compose.ui.test.performImeAction
import androidx.compose.ui.test.performTextInput
import androidx.compose.ui.test.performSemanticsAction
import androidx.compose.ui.semantics.SemanticsActions
import androidx.compose.ui.text.TextRange
import androidx.test.ext.junit.runners.AndroidJUnit4
import com.lingxi.code.components.UiTags
import com.lingxi.code.model.EngineModelCatalog
import com.lingxi.code.model.SessionRef
import com.lingxi.code.theme.LingXiTheme
import org.junit.Assert.assertEquals
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith

@RunWith(AndroidJUnit4::class)
class ComposerLoadingUiTest {
    @get:Rule val rule = createComposeRule()

    @Test fun startupAllowsTypingAndKeepsMiddleCaret() {
        assertLoadingKeepsSelection("new", transitioning = false, selection = TextRange(5))
    }

    @Test fun newSessionConfirmationKeepsForwardSelection() {
        assertLoadingKeepsSelection("new", transitioning = true, selection = TextRange(2, 8))
    }

    @Test fun resumedSessionKeepsBackwardSelection() {
        assertLoadingKeepsSelection("session-loaded", transitioning = true, selection = TextRange(8, 2))
    }

    @Test fun multilineDraftKeepsCaretWhenSessionBecomesReady() {
        assertLoadingKeepsSelection("new", transitioning = false,
            selection = TextRange(14), text = "first line\nsecond line")
    }

    private fun assertLoadingKeepsSelection(
        sessionId: String,
        transitioning: Boolean,
        selection: TextRange,
        text: String = "hello world",
    ) {
        val state = mutableStateOf(ChatState(
            session = SessionRef(sessionId, "Loading session"),
            messages = emptyList(),
            model = EngineModelCatalog.pending,
            sessionReady = false,
            sessionTransitioning = transitioning,
        ))
        val draft = mutableStateOf("")
        val sent = mutableListOf<String>()
        rule.setContent {
            LingXiTheme(darkTheme = false) {
                ChatScreen(state = state.value, onSend = { sent += it }, onNewChat = {},
                    onSelectModel = {}, isDark = false,
                    draft = draft.value, onDraftChange = { draft.value = it })
            }
        }

        val input = rule.onNode(hasSetTextAction())
        input.assertIsEnabled().performClick().performTextInput(text)
        // The convenience input-selection action sorts min/max, so it cannot
        // represent a backward handle selection. Preserve the supplied anchor
        // and focus explicitly, then verify them before and after loading.
        input.performSemanticsAction(SemanticsActions.SetSelection) {
            it(selection.start, selection.end, true)
        }
        input.assert(SemanticsMatcher.expectValue(SemanticsProperties.TextSelectionRange, selection))
        rule.onNodeWithTag(UiTags.COMPOSER_SEND).assertIsNotEnabled()
        input.performImeAction()
        rule.runOnIdle {
            assertEquals(emptyList<String>(), sent)
            assertEquals(text, draft.value)
            state.value = state.value.copy(
                session = SessionRef("session-loaded", "Loaded session"),
                sessionReady = true,
                sessionTransitioning = false,
            )
        }

        input.assertIsFocused().assertTextContains(text).assert(
            SemanticsMatcher.expectValue(SemanticsProperties.TextSelectionRange, selection),
        )
        input.performTextInput("!")
        val edited = text.replaceRange(minOf(selection.start, selection.end),
            maxOf(selection.start, selection.end), "!")
        input.assertTextContains(edited)
        rule.onNodeWithTag(UiTags.COMPOSER_SEND).assertIsEnabled().performClick()
        rule.runOnIdle {
            assertEquals(listOf(edited), sent)
            assertEquals("", draft.value)
        }
    }
}
