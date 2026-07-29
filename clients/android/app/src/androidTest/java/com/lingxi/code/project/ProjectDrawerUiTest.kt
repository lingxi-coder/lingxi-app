package com.lingxi.code.project

import androidx.compose.ui.graphics.Color
import androidx.compose.ui.test.assertIsDisplayed
import androidx.compose.ui.test.junit4.createComposeRule
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.performClick
import androidx.test.ext.junit.runners.AndroidJUnit4
import com.lingxi.code.drawer.ProjectsSection
import com.lingxi.code.model.Project
import com.lingxi.code.model.ProjectSession
import com.lingxi.code.theme.LingXiTheme
import org.junit.Assert.assertEquals
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith

@RunWith(AndroidJUnit4::class)
class ProjectDrawerUiTest {
    @get:Rule
    val rule = createComposeRule()

    @Test
    fun projectExpandsAndRoutesRealSessionAndNewSessionCallbacks() {
        var selected = ""
        var newSessionProject = ""
        val project = Project(
            id = "10000000-0000-4000-8000-000000000001",
            wsId = "android-local",
            name = "真实项目",
            icon = "◇",
            color = Color.Blue,
            desc = "本机",
            sessions = listOf(
                ProjectSession("session-1", "真实会话", "刚刚", "", msgs = 3),
            ),
        )
        rule.setContent {
            LingXiTheme {
                ProjectsSection(
                    projects = listOf(project),
                    activeSession = "",
                    openProjects = setOf(project.id),
                    onToggleProject = {},
                    onSelectSession = { projectId, ref -> selected = "$projectId:${ref.id}" },
                    onNewSession = { newSessionProject = it },
                    onCreateProject = {},
                    onReimportProject = {},
                    onExportProject = {},
                    onReauthorizeProject = {},
                )
            }
        }

        rule.onNodeWithText("真实会话").assertIsDisplayed().performClick()
        assertEquals("${project.id}:session-1", selected)
        rule.onNodeWithText("新会话").performClick()
        assertEquals(project.id, newSessionProject)
    }
}
