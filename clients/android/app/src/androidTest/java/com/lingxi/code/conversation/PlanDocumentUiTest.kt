package com.lingxi.code.conversation

import androidx.compose.ui.test.assertIsDisplayed
import androidx.compose.ui.test.junit4.createComposeRule
import androidx.compose.ui.test.onNodeWithTag
import androidx.compose.ui.test.performClick
import androidx.test.ext.junit.runners.AndroidJUnit4
import com.lingxi.code.theme.LingXiTheme
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith

@RunWith(AndroidJUnit4::class)
class PlanDocumentUiTest {
    @get:Rule val rule = createComposeRule()

    @Test fun previewOpensCompleteNativeDocument() {
        rule.setContent {
            LingXiTheme(darkTheme = false) {
                PlanDocumentCard("# Plan title\n\n## Approach\n\n" + "- Preserve sessions\n".repeat(40), false)
            }
        }
        rule.onNodeWithTag("conversation.plan.preview").assertIsDisplayed().performClick()
        rule.onNodeWithTag("conversation.plan.detail").assertIsDisplayed()
    }
}
