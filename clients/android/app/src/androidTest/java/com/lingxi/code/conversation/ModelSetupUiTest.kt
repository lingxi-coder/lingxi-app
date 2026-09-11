package com.lingxi.code.conversation

import androidx.compose.ui.test.assertIsDisplayed
import androidx.compose.ui.test.junit4.createComposeRule
import androidx.compose.ui.test.onNodeWithTag
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.performClick
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import com.lingxi.code.R
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
class ModelSetupUiTest {

    @get:Rule
    val rule = createComposeRule()

    @Test
    fun missingModel_replacesRuntimeErrorWithActionableSetupState() {
        val settingsOpened = AtomicBoolean(false)
        rule.setContent {
            LingXiTheme(darkTheme = false) {
                ChatScreen(
                    state = ChatState(
                        session = SessionRef(id = "new", title = "新对话"),
                        messages = emptyList(),
                        isNew = true,
                        sessionReady = false,
                        model = EngineModelCatalog.pending,
                        error = ChatError("移动端引擎不可用或未正确链接"),
                    ),
                    onSend = {},
                    onNewChat = {},
                    onSelectModel = {},
                    isDark = false,
                    onToggleTheme = {},
                    modelSetupRequired = true,
                    onOpenModelSettings = { settingsOpened.set(true) },
                )
            }
        }

        rule.onNodeWithTag(UiTags.MODEL_SETUP_BANNER).assertIsDisplayed()
        rule.onNodeWithText(InstrumentationRegistry.getInstrumentation().targetContext.getString(R.string.chat_model_not_configured_title)).assertIsDisplayed()
        rule.onNodeWithTag(UiTags.CHAT_ERROR).assertDoesNotExist()
        rule.onNodeWithTag(UiTags.MODEL_SETUP_ACTION).performClick()

        assertTrue(settingsOpened.get())
    }
}
