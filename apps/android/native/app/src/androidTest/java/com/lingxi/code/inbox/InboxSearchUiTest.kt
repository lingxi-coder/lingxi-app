package com.lingxi.code.inbox

import androidx.activity.ComponentActivity
import androidx.compose.ui.test.assertIsDisplayed
import androidx.compose.ui.test.assertIsFocused
import androidx.compose.ui.test.junit4.createAndroidComposeRule
import androidx.compose.ui.test.onNodeWithContentDescription
import androidx.compose.ui.test.onNodeWithTag
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.performClick
import androidx.compose.ui.test.performTextInput
import androidx.test.ext.junit.runners.AndroidJUnit4
import com.lingxi.code.R
import com.lingxi.code.theme.LingXiTheme
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith

@RunWith(AndroidJUnit4::class)
class InboxSearchUiTest {
    @get:Rule val compose = createAndroidComposeRule<ComponentActivity>()

    @Test fun searchOpensOnTapAndClosingRestoresAllMessages() {
        val activity = compose.activity
        val controller = InboxController.get(activity)
        val state = InboxState(rows = listOf(
            InboxItem(InboxRef("team", "qa", "alpha", 1), category = "team", title = "Alpha review"),
            InboxItem(InboxRef("team", "qa", "beta", 1), category = "team", title = "Beta planning"),
        ))
        compose.setContent {
            LingXiTheme(darkTheme = false) {
                InboxScreenContent(state, controller, onOpen = {}, onOpenProjects = {})
            }
        }

        val search = compose.onNodeWithTag("inbox.search")
        val alpha = compose.onNodeWithText("Alpha review")
        val beta = compose.onNodeWithText("Beta planning")
        search.assertDoesNotExist()
        alpha.assertIsDisplayed()
        beta.assertIsDisplayed()

        compose.onNodeWithContentDescription(activity.getString(R.string.inbox_search)).performClick()
        search.assertIsFocused().performTextInput("Alpha")
        alpha.assertIsDisplayed()
        beta.assertDoesNotExist()

        compose.onNodeWithContentDescription(activity.getString(R.string.common_close)).performClick()
        search.assertDoesNotExist()
        alpha.assertIsDisplayed()
        beta.assertIsDisplayed()
    }
}
