package com.lingxi.code.remote

import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.compose.ui.test.assertIsDisplayed
import androidx.compose.ui.test.junit4.createComposeRule
import androidx.compose.ui.test.onNodeWithTag
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.performClick
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import com.lingxi.code.R
import com.lingxi.code.bindings.android.RemoteSessionPhaseFfi
import com.lingxi.code.bindings.client.PermissionResponseDto
import com.lingxi.code.components.UiTags
import com.lingxi.code.conversation.PermissionPromptState
import com.lingxi.code.theme.LingXiTheme
import org.junit.Assert.assertEquals
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith

@RunWith(AndroidJUnit4::class)
class TeamApprovalContentUiTest {
    @get:Rule val rule = createComposeRule()

    @Test fun onlyOneTimeApprovalAndDenialAreAvailableWhileLive() {
        var phase by mutableStateOf(RemoteSessionPhaseFfi.LIVE)
        val actions = mutableListOf<String>()
        rule.setContent {
            LingXiTheme(darkTheme = true) {
                TeamApprovalContent(
                    phase = phase,
                    statusLine = null,
                    permission = PermissionPromptState(requestId = 17uL, title = "Run tool?", detail = "pwd"),
                    onApprove = { id, response -> actions += "$id:$response" },
                    onDeny = { id -> actions += "$id:deny" },
                )
            }
        }
        rule.onNodeWithTag(UiTags.PERMISSION_ALLOW_ONCE).assertIsDisplayed().performClick()
        rule.onNodeWithTag(UiTags.PERMISSION_DENY).assertIsDisplayed().performClick()
        rule.onNodeWithTag(UiTags.PERMISSION_ALLOW_ALWAYS).assertDoesNotExist()
        rule.onNodeWithTag(UiTags.PERMISSION_ALLOW_AUTO).assertDoesNotExist()
        rule.onNodeWithTag(UiTags.COMPOSER_SEND).assertDoesNotExist()
        rule.onNodeWithTag(UiTags.COMPOSER_STOP).assertDoesNotExist()
        rule.onNodeWithTag(UiTags.MODEL_PICKER_CHIP).assertDoesNotExist()
        val files = InstrumentationRegistry.getInstrumentation().targetContext.getString(R.string.remote_files)
        rule.onNodeWithText(files).assertDoesNotExist()
        rule.runOnIdle {
            assertEquals(listOf("17:${PermissionResponseDto.ALLOW_ONCE}", "17:deny"), actions)
            phase = RemoteSessionPhaseFfi.OFFLINE
        }
        rule.onNodeWithTag(UiTags.PERMISSION_PROMPT).assertDoesNotExist()
    }
}
