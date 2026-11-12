package com.lingxi.code.theme

import androidx.activity.ComponentActivity
import androidx.compose.ui.test.assertIsDisplayed
import androidx.compose.ui.test.assertIsNotEnabled
import androidx.compose.ui.test.hasAnyAncestor
import androidx.compose.ui.test.hasClickAction
import androidx.compose.ui.test.hasText
import androidx.compose.ui.test.isDialog
import androidx.compose.ui.test.junit4.v2.createAndroidComposeRule
import androidx.compose.ui.test.onNodeWithTag
import androidx.compose.ui.test.performClick
import androidx.test.ext.junit.runners.AndroidJUnit4
import com.lingxi.code.R
import com.lingxi.code.testing.finishComposeFixture
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith

/**
 * The one confirmation of docs/TEAMS-UI-POLISH.md U8: a title that names the thing, a body only when it adds something, a verb on the button, and a
 * Cancel that is always there, whatever the action.
 */
@RunWith(AndroidJUnit4::class)
class ConfirmDialogUiTest {
    @After fun closeFixture() = finishComposeFixture(rule.activity)

    @get:Rule
    val rule = createAndroidComposeRule<ComponentActivity>()

    private val events = mutableListOf<String>()
    private fun cancel() = rule.activity.getString(R.string.common_cancel)
    private fun inDialog(label: String) = hasText(label) and hasAnyAncestor(isDialog())

    private fun show(body: String? = "It cannot be undone.", destructive: Boolean = true, enabled: Boolean = true) = rule.setContent {
        LingXiTheme(darkTheme = false) {
            ConfirmDialog(
                title = "Delete “Crew”?", body = body, confirm = "Delete", destructive = destructive, enabled = enabled, confirmTag = "confirm",
                onConfirm = { events += "confirm" }, onDismiss = { events += "dismiss" },
            )
        }
    }

    @Test fun theTitleNamesTheThingTheBodyAddsAndTheButtonIsAVerbNextToACancel() {
        show()
        rule.onNode(inDialog("Delete “Crew”?")).assertIsDisplayed()
        rule.onNode(inDialog("It cannot be undone.")).assertIsDisplayed()
        rule.onNode(inDialog("Delete") and hasClickAction()).assertIsDisplayed()
        rule.onNode(inDialog(cancel()) and hasClickAction()).assertIsDisplayed()
    }

    @Test fun withoutABodyThereIsStillACancel() {
        show(body = null)
        rule.onNode(inDialog("Delete “Crew”?")).assertIsDisplayed()
        rule.onNode(inDialog(cancel()) and hasClickAction()).assertIsDisplayed()
    }

    @Test fun confirmActsOnceAndCancelActsAsDismissWithoutConfirming() {
        show()
        rule.onNode(inDialog(cancel()) and hasClickAction()).performClick()
        assertEquals(listOf("dismiss"), events)
        events.clear()
        rule.onNodeWithTag("confirm").performClick()
        assertEquals(listOf("confirm"), events)
    }

    @Test fun aBusyActionDisablesTheVerbButNotTheCancel() {
        show(enabled = false)
        rule.onNodeWithTag("confirm").assertIsNotEnabled()
        rule.onNode(inDialog(cancel()) and hasClickAction()).performClick()
        assertEquals(listOf("dismiss"), events)
    }
}
