package com.lingxi.code.conversation

import androidx.compose.ui.test.assertIsDisplayed
import androidx.compose.ui.test.junit4.createComposeRule
import androidx.compose.ui.test.onNodeWithTag
import androidx.test.ext.junit.runners.AndroidJUnit4
import com.lingxi.code.bindings.AutoModePromptDto
import com.lingxi.code.components.UiTags
import com.lingxi.code.theme.LingXiTheme
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith

/** Compose-level coverage for the persistent-rule suppression affordance. */
@RunWith(AndroidJUnit4::class)
class PermissionPromptDialogUiTest {

    @get:Rule
    val rule = createComposeRule()

    private val state = PermissionPromptState(
        requestId = 17uL,
        title = "允许 Bash？",
        detail = "pwd",
    )

    @Test
    fun suppressedRequest_hidesAllowAlways_butKeepsAllowOnce() {
        rule.setContent {
            LingXiTheme(darkTheme = true) {
                PermissionPromptDialog(
                    state = state.copy(suppressAlwaysAllowRule = true),
                    onApprove = { _, _ -> },
                    onDeny = {},
                )
            }
        }

        rule.onNodeWithTag(UiTags.PERMISSION_ALLOW_ALWAYS).assertDoesNotExist()
        rule.onNodeWithTag(UiTags.PERMISSION_ALLOW_ONCE).assertIsDisplayed()
    }

    @Test
    fun ordinaryRequest_keepsAllowAlways() {
        rule.setContent {
            LingXiTheme(darkTheme = true) {
                PermissionPromptDialog(
                    state = state,
                    onApprove = { _, _ -> },
                    onDeny = {},
                )
            }
        }

        rule.onNodeWithTag(UiTags.PERMISSION_ALLOW_ALWAYS).assertIsDisplayed()
    }

    @Test
    fun autoModeRequest_replacesAllowAlways() {
        rule.setContent {
            LingXiTheme(darkTheme = true) {
                PermissionPromptDialog(
                    state = state.copy(autoModePrompt = AutoModePromptDto.WORKFLOW_BASH),
                    onApprove = { _, _ -> },
                    onDeny = {},
                )
            }
        }

        rule.onNodeWithTag(UiTags.PERMISSION_ALLOW_ALWAYS).assertDoesNotExist()
        rule.onNodeWithTag(UiTags.PERMISSION_ALLOW_AUTO).assertIsDisplayed()
        rule.onNodeWithTag(UiTags.PERMISSION_ALLOW_ONCE).assertIsDisplayed()
    }
}
