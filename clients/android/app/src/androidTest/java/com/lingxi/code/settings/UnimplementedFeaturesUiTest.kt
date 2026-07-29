package com.lingxi.code.settings

import androidx.compose.ui.test.junit4.createComposeRule
import androidx.compose.ui.test.onNodeWithTag
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.performClick
import androidx.navigation.compose.rememberNavController
import androidx.test.ext.junit.runners.AndroidJUnit4
import com.lingxi.code.components.UiTags
import com.lingxi.code.drawer.DrawerContent
import com.lingxi.code.drawer.DrawerProductionData
import com.lingxi.code.drawer.rememberDrawerUiState
import com.lingxi.code.theme.LingXiTheme
import org.junit.Assert.assertTrue
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith

@RunWith(AndroidJUnit4::class)
class UnimplementedFeaturesUiTest {
    @get:Rule
    val rule = createComposeRule()

    @Test
    fun drawerDoesNotExposeMemoryOrKnowledgePlaceholders() {
        rule.setContent {
            LingXiTheme {
                DrawerContent(
                    ui = rememberDrawerUiState(),
                    onSelectSession = {},
                    onOpenSettings = {},
                    onClose = {},
                    productionData = DrawerProductionData(
                        workspaces = emptyList(),
                        projects = emptyList(),
                        crons = emptyList(),
                    ),
                )
            }
        }

        rule.onNodeWithText("知识库").assertDoesNotExist()
        rule.onNodeWithText("记忆").assertDoesNotExist()
    }

    @Test
    fun drawerTerminalShortcutOpensShell() {
        var terminalOpened = false
        rule.setContent {
            LingXiTheme {
                DrawerContent(
                    ui = rememberDrawerUiState(),
                    onSelectSession = {},
                    onOpenSettings = {},
                    onOpenTerminal = { terminalOpened = true },
                    onClose = {},
                    productionData = DrawerProductionData(
                        workspaces = emptyList(),
                        projects = emptyList(),
                        crons = emptyList(),
                    ),
                )
            }
        }

        rule.onNodeWithTag(UiTags.DRAWER_TERMINAL).performClick()
        rule.runOnIdle { assertTrue(terminalOpened) }
    }

    @Test
    fun settingsDoesNotExposeUnimplementedMemoryKnowledgeOrWorkflowSection() {
        rule.setContent {
            LingXiTheme {
                MainSettingsPage(
                    state = SettingsUiState(),
                    isDark = true,
                    navController = rememberNavController(),
                )
            }
        }

        rule.onNodeWithText("记忆与知识").assertDoesNotExist()
        rule.onNodeWithText("知识库").assertDoesNotExist()
        rule.onNodeWithText("记忆").assertDoesNotExist()
        rule.onNodeWithText("工作流与自动化").assertDoesNotExist()
    }
}
