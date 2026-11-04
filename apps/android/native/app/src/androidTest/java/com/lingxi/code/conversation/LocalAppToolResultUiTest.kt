package com.lingxi.code.conversation

import androidx.compose.runtime.mutableStateOf
import androidx.compose.ui.test.assertCountEquals
import androidx.compose.ui.test.assertIsDisplayed
import androidx.compose.ui.test.hasAnyAncestor
import androidx.compose.ui.test.hasText
import androidx.compose.ui.test.isDialog
import androidx.compose.ui.test.junit4.v2.createComposeRule
import androidx.compose.ui.test.onAllNodesWithText
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import com.lingxi.code.R
import com.lingxi.code.theme.LingXiTheme
import org.junit.Assert.assertEquals
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith

@RunWith(AndroidJUnit4::class)
class LocalAppToolResultUiTest {
    @get:Rule val rule = createComposeRule()

    @Test fun bodyOnlyRestoredReasonIsLocalizedInCollapsedAndExpandedView() {
        val context = InstrumentationRegistry.getInstrumentation().targetContext
        val reason = "local_app_development_requires_code: switch to Code"
        val call = ToolCallUi("app-1", "LocalApp", display = ToolResultDisplayUi(
            headline = "Failed", headlineKind = HeadlineKindUi.Failed, body = reason, bodyLines = 1, collapsed = true),
            status = AgentToolStatus.Failed)
        val expanded = mutableStateOf(false)
        rule.setContent { LingXiTheme { ToolCallView(call, expanded.value, { expanded.value = !expanded.value }) } }
        val localized = context.getString(R.string.local_app_development_requires_code)
        rule.onAllNodesWithText(localized).assertCountEquals(1)
        rule.runOnIdle { expanded.value = true }
        rule.onNode(hasText(localized) and hasAnyAncestor(isDialog())).assertIsDisplayed()
        assertEquals(reason, call.display?.body)
    }
}
