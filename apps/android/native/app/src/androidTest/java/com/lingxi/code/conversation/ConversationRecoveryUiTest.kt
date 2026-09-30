package com.lingxi.code.conversation

import androidx.compose.ui.test.assertIsDisplayed
import androidx.compose.ui.test.junit4.createComposeRule
import androidx.compose.ui.test.onNodeWithTag
import androidx.compose.ui.test.performClick
import androidx.test.ext.junit.runners.AndroidJUnit4
import com.lingxi.code.components.UiTags
import com.lingxi.code.model.EngineModelCatalog
import com.lingxi.code.model.SessionRef
import com.lingxi.code.theme.LingXiTheme
import java.util.concurrent.atomic.AtomicBoolean
import org.junit.Assert.assertTrue
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith

@RunWith(AndroidJUnit4::class)
class ConversationRecoveryUiTest {

    @get:Rule
    val rule = createComposeRule()

    @Test
    fun waitingRecoveredCheckpointExposesDiscardActionInComposer() {
        val discarded = AtomicBoolean(false)
        rule.setContent {
            LingXiTheme(darkTheme = false) {
                ChatScreen(
                    state = ChatState(
                        session = SessionRef(id = "session-a", title = "会话"),
                        messages = emptyList(),
                        sessionReady = true,
                        model = EngineModelCatalog.pending,
                        durableRecoveryBlocked = true,
                    ),
                    onSend = {},
                    onNewChat = {},
                    onSelectModel = {},
                    isDark = false,
                    onToggleTheme = {},
                    onDiscardRecoveredTurn = { discarded.set(true) },
                )
            }
        }

        rule.onNodeWithTag(UiTags.COMPOSER_DISCARD).assertIsDisplayed().performClick()

        assertTrue(discarded.get())
    }
}
