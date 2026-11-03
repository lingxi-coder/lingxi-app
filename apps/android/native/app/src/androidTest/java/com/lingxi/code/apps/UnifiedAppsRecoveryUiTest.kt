package com.lingxi.code.apps

import androidx.compose.ui.test.hasAnyAncestor
import androidx.compose.ui.test.hasTestTag
import androidx.compose.ui.test.hasText
import androidx.compose.ui.test.assertIsDisplayed
import androidx.compose.ui.test.junit4.v2.createComposeRule
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.performClick
import androidx.compose.ui.graphics.Color
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import com.lingxi.code.R
import com.lingxi.code.theme.LingXiTheme
import com.lingxi.code.model.Project
import com.lingxi.code.project.ProjectPage
import org.junit.Assert.assertEquals
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith

@RunWith(AndroidJUnit4::class)
class UnifiedAppsRecoveryUiTest {
    @get:Rule val rule = createComposeRule()

    @Test fun recoveryButtonUsesTheExactDisplayedProjectAndSelection() {
        val context = InstrumentationRegistry.getInstrumentation().targetContext
        val first = PendingAuthoringRecovery("project-a", "selection-a", "First app")
        val second = PendingAuthoringRecovery("project-b", "selection-b", "Second app")
        var opened: PendingAuthoringRecovery? = null
        rule.setContent {
            LingXiTheme {
                UnifiedAppsPage(UnifiedLocalApps.get(context), {}, {}, {}, { _, _, _ -> }, {},
                    pendingAuthoring = listOf(first, second), onRecoverAuthoring = { opened = it }, canStartAuthoring = { true })
            }
        }
        val button = context.getString(R.string.local_app_authoring_recover)
        rule.onNode(hasText(button) and hasAnyAncestor(hasTestTag("pending-authoring:${second.selectionId}")))
            .performClick()
        rule.runOnIdle { assertEquals(second, opened) }
    }

    @Test fun chatRecoveryShowsCodePromptWithoutStartingRecovery() {
        val context = InstrumentationRegistry.getInstrumentation().targetContext
        val pending = PendingAuthoringRecovery("project-a", "selection-a", "App to recover")
        var opened: PendingAuthoringRecovery? = null
        rule.setContent {
            LingXiTheme {
                UnifiedAppsPage(UnifiedLocalApps.get(context), {}, {}, {}, { _, _, _ -> }, {},
                    pendingAuthoring = listOf(pending), onRecoverAuthoring = { opened = it }, canStartAuthoring = { false })
            }
        }
        rule.onNodeWithText(context.getString(R.string.local_app_authoring_recover)).performClick()
        rule.onNodeWithText(context.getString(R.string.local_app_development_requires_code)).assertIsDisplayed()
        rule.runOnIdle { assertEquals(null, opened) }
    }

    @Test fun projectRecoveryInChatShowsThePromptOnTheVisiblePage() {
        val context = InstrumentationRegistry.getInstrumentation().targetContext
        var opened = false
        val project = Project("project-a", "local", "App project", "◇", Color.Blue, "", emptyList())
        rule.setContent {
            LingXiTheme {
                ProjectPage(project, emptyList(), "", {}, {}, {}, {}, {}, {}, {},
                    onRecoverAuthoring = { opened = true }, canStartAuthoring = { false })
            }
        }
        rule.onNodeWithText(context.getString(R.string.local_app_authoring_recover)).performClick()
        rule.onNodeWithText(context.getString(R.string.local_app_development_requires_code)).assertIsDisplayed()
        rule.runOnIdle { assertEquals(false, opened) }
    }

    @Test fun chatCreateAndSourceImportNeverStartAuthoring() {
        val context = InstrumentationRegistry.getInstrumentation().targetContext
        var created = false
        var captured = 0
        rule.setContent {
            LingXiTheme {
                UnifiedAppsPage(UnifiedLocalApps.get(context), {}, {}, {}, { _, _, _ -> created = true }, {},
                    canStartAuthoring = { false }, captureImportTicket = { captured++; null })
            }
        }
        val prompt = context.getString(R.string.local_app_development_requires_code)
        rule.onNodeWithText(context.getString(R.string.local_app_v2_create)).performClick()
        rule.onNodeWithText(prompt).assertIsDisplayed()
        rule.onNodeWithText(context.getString(R.string.local_app_v2_cancel)).performClick()
        rule.onNodeWithText(context.getString(R.string.local_app_v2_import_source)).performClick()
        rule.onNodeWithText(prompt).assertIsDisplayed()
        rule.runOnIdle { assertEquals(false, created); assertEquals(0, captured) }
    }
}
