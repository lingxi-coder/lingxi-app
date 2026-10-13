package com.lingxi.code.project

import androidx.compose.ui.graphics.Color
import com.lingxi.code.drawer.RecentsFilter
import com.lingxi.code.model.Project
import com.lingxi.code.model.ProjectSession
import com.lingxi.code.model.SessionMode
import org.junit.Assert.assertEquals
import org.junit.Test

class ProjectPagesTest {

    private fun session(id: String, mode: SessionMode, updatedAtEpochMillis: Long) = ProjectSession(
        id = id,
        title = id,
        activity = "",
        preview = "",
        msgs = 1,
        mode = mode,
        updatedAtEpochMillis = updatedAtEpochMillis,
    )

    private fun project(
        id: String,
        name: String = id,
        updatedAtEpochMillis: Long = 0L,
        sessions: List<ProjectSession> = emptyList(),
    ) = Project(
        id = id,
        wsId = "ws",
        name = name,
        icon = "",
        color = Color.Unspecified,
        desc = "",
        sessions = sessions,
        updatedAtEpochMillis = updatedAtEpochMillis,
    )

    @Test
    fun `pinned projects lead and the latest pin is first`() {
        val projects = listOf(
            project("plain", updatedAtEpochMillis = 900),
            project("old-pin", updatedAtEpochMillis = 1),
            project("new-pin", updatedAtEpochMillis = 2),
        )
        val pins = mapOf("project.old-pin" to 10L, "project.new-pin" to 20L)

        assertEquals(
            listOf("new-pin", "old-pin", "plain"),
            sortProjectsForList(projects) { pins[it] }.map { it.id },
        )
    }

    @Test
    fun `unpinned projects order by newest session or own update time then name`() {
        val projects = listOf(
            project("b-name", name = "Bravo", updatedAtEpochMillis = 100),
            project("a-name", name = "Alpha", updatedAtEpochMillis = 100),
            project(
                "has-session",
                updatedAtEpochMillis = 50,
                sessions = listOf(session("s", SessionMode.Code, 500)),
            ),
            project("recent-edit", updatedAtEpochMillis = 300),
        )

        assertEquals(
            listOf("has-session", "recent-edit", "a-name", "b-name"),
            sortProjectsForList(projects) { null }.map { it.id },
        )
    }

    @Test
    fun `project sessions are filtered by chip and newest first`() {
        val project = project(
            "p",
            sessions = listOf(
                session("old-code", SessionMode.Code, 1),
                session("chat", SessionMode.Chat, 5),
                session("new-code", SessionMode.Code, 9),
            ),
        )

        assertEquals(
            listOf("new-code", "chat", "old-code"),
            projectSessionsFor(project, RecentsFilter.All).map { it.id },
        )
        assertEquals(listOf("chat"), projectSessionsFor(project, RecentsFilter.Chat).map { it.id })
        assertEquals(
            listOf("new-code", "old-code"),
            projectSessionsFor(project, RecentsFilter.Code).map { it.id },
        )
    }

    @Test
    fun `workspace pin keys match the drawer's project workspace key`() {
        assertEquals("project.abc", projectWorkspaceKey("abc"))
    }
}
