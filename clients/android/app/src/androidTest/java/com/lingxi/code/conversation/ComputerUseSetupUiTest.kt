package com.lingxi.code.conversation

import androidx.activity.ComponentActivity
import androidx.compose.ui.test.assertIsDisplayed
import androidx.compose.ui.test.junit4.createAndroidComposeRule
import androidx.compose.ui.test.onNodeWithTag
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.performClick
import androidx.test.platform.app.InstrumentationRegistry
import com.lingxi.code.R
import com.lingxi.code.components.UiTags
import com.lingxi.code.theme.LingXiTheme
import org.junit.Assert.assertTrue
import org.junit.Rule
import org.junit.Test

class ComputerUseSetupUiTest {
    @get:Rule
    val rule = createAndroidComposeRule<ComponentActivity>()

    @Test
    fun unavailableComputerUseShowsMissingStepsAndOpensSettings() {
        var opened = false
        var dismissed = false
        rule.setContent {
            LingXiTheme(darkTheme = true) {
                ComputerUseSetupBanner(
                    status = ComputerUseSetupStatus(
                        accessibilityEnabled = false,
                        browserAuthorized = false,
                        sessionActive = false,
                    ),
                    onOpenComputerUseSettings = { opened = true },
                    onDismiss = { dismissed = true },
                )
            }
        }

        rule.onNodeWithTag(UiTags.COMPUTER_USE_SETUP_BANNER).assertIsDisplayed()
        val context = InstrumentationRegistry.getInstrumentation().targetContext
        val steps = listOf(R.string.chat_computer_use_missing_accessibility,
            R.string.chat_computer_use_missing_browser, R.string.settings_cu_start_session)
            .joinToString("、") { context.getString(it) }
        rule.onNodeWithText(context.getString(R.string.chat_computer_use_unavailable_detail, steps))
            .assertIsDisplayed()
        rule.onNodeWithTag(UiTags.COMPUTER_USE_SETUP_ACTION).performClick()
        rule.runOnIdle { assertTrue(opened) }
        rule.onNodeWithTag(UiTags.COMPUTER_USE_SETUP_DISMISS).performClick()
        rule.runOnIdle { assertTrue(dismissed) }
    }
}
