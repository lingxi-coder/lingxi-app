package com.lingxi.code.cron

import com.lingxi.code.model.ConversationScope
import com.lingxi.code.model.SessionMode
import com.lingxi.code.project.*
import com.lingxi.code.conversation.sessionTranscriptPath
import com.lingxi.code.conversation.selectedTranscriptPathMatches
import org.junit.Assert.*
import org.junit.Test

class CronSessionSelectionTest {
    @Test fun selectedPathCannotResolveToAnotherTranscriptAfterCatalogChanges() {
        val selected = "/b/same-id.jsonl"
        assertTrue(selectedTranscriptPathMatches(selected, selected))
        assertFalse(selectedTranscriptPathMatches(selected, "/a/same-id.jsonl"))
        assertFalse(selectedTranscriptPathMatches(null, "/b/same-id.jsonl"))
        assertFalse(selectedTranscriptPathMatches("relative.jsonl", "relative.jsonl"))
    }
    private val a = "19587a33-0725-48db-abca-8a2aed345f6b"
    private val b = "29587a33-0725-48db-abca-8a2aed345f6b"
    private val c = "39587a33-0725-48db-abca-8a2aed345f6b"
    private fun row(id: String, mode: SessionMode, path: String) =
        ProjectSessionSummary(id, id, 1, "", 1, mode, transcriptPath = path)
    private fun state() = ProjectStoreState(loading = false,
        globalSessions = listOf(row(a, SessionMode.Chat, "/guest/global.jsonl"), row(b, SessionMode.Code, "/guest/code.jsonl")),
        projects = listOf(ProjectSnapshot(
            ProjectRecord("project-a", "Project A", ProjectStorageKind.Internal, 1, 1,
                directory = "/old", activeDirectory = "/changed-by-other-session", visibleModes = setOf(SessionMode.Chat)),
            ProjectWorkspace("project-a", "/host", "/changed-by-other-session"),
            listOf(row(c, SessionMode.Chat, "/guest/project-original.jsonl").copy(workspaceDirectory = "/old")),
        )))
    private fun owners() = mapOf(
        a to ScopeStateStore.SessionOwner("global", SessionMode.Chat),
        b to ScopeStateStore.SessionOwner("global", SessionMode.Code),
        c to ScopeStateStore.SessionOwner("project.project-a", SessionMode.Chat),
    )

    @Test fun `picker includes ordinary Global and only captured mode with exact owner and source`() {
        val targets = cronSessionTargets(state(), emptyList(), owners(), SessionMode.Chat)
        assertEquals(setOf(a, c), targets.map { it.sessionId }.toSet())
        val global = targets.single { it.sessionId == a }
        assertEquals("global", global.scopeKey)
        val saved = CronAutomation().withTarget(global)
        assertTrue(saved.matchesTarget(global, GLOBAL_CRON_SCOPE_ID, SessionMode.Chat))
        assertFalse(saved.matchesTarget(global.copy(transcriptPath = "/replacement"), GLOBAL_CRON_SCOPE_ID, SessionMode.Chat))
        assertFalse(saved.matchesTarget(global.copy(scopeKey = "scheduled"), GLOBAL_CRON_SCOPE_ID, SessionMode.Chat))
        assertFalse(saved.matchesTarget(global, GLOBAL_CRON_SCOPE_ID, SessionMode.Code))
        assertEquals("", saved.clearTarget().targetSessionTranscriptPath)
    }

    @Test fun `catalog changes and invalid owners fence selection and submission`() {
        val before = cronSessionTargets(state(), emptyList(), owners(), SessionMode.Chat).first { it.sessionId == a }
        val config = CronAutomation().withTarget(before)
        val replaced = state().copy(globalSessions = listOf(row(a, SessionMode.Chat, "/other.jsonl")))
        val after = cronSessionTargets(replaced, emptyList(), owners(), SessionMode.Chat).single { it.sessionId == a }
        assertFalse(config.matchesTarget(after, GLOBAL_CRON_SCOPE_ID, SessionMode.Chat))
        assertTrue(cronSessionTargets(state().copy(storageUnavailable = true), emptyList(), owners(), SessionMode.Chat).isEmpty())
        assertTrue(cronSessionTargets(state(), emptyList(), emptyMap(), SessionMode.Chat).isEmpty())
    }

    @Test fun `restore selects project identity and exact path independent of current directory`() {
        assertEquals("/guest/project-original.jsonl", sessionTranscriptPath(ConversationScope.Project("project-a"), c,
            SessionMode.Chat, state(), emptyList()))
        assertNull(sessionTranscriptPath(ConversationScope.Global, c, SessionMode.Chat, state(), emptyList()))
        assertNull(sessionTranscriptPath(ConversationScope.Project("project-a"), c, SessionMode.Code, state(), emptyList()))
        val missing = state().copy(projects = state().projects.map { it.copy(sessions = it.sessions.map { row -> row.copy(transcriptPath = null) }) })
        assertNull(sessionTranscriptPath(ConversationScope.Project("project-a"), c, SessionMode.Chat, missing, emptyList()))
    }

    @Test fun `scheduled receipts preserve their owner and reject conflicting transcript paths`() {
        val id = "49587a33-0725-48db-abca-8a2aed345f6b"
        val run = CronRunRecord("run", "task", "global", null, "", "Scheduled", 1, 2,
            sessionId = id, sessionTranscriptPath = "/guest/scheduled.jsonl", sessionMode = "chat", sessionScopeKey = "scheduled")
        val recorded = owners() + (id to ScopeStateStore.SessionOwner("scheduled", SessionMode.Chat))
        val scheduled = cronSessionTargets(state(), listOf(run), recorded, SessionMode.Chat).single { it.sessionId == id }
        assertEquals("scheduled", scheduled.scopeKey)
        assertEquals("/guest/scheduled.jsonl", sessionTranscriptPath(ConversationScope.Scheduled, id, SessionMode.Chat, state(), listOf(run)))
        val conflicting = run.copy(runId = "another", sessionTranscriptPath = "/guest/replacement.jsonl")
        assertNull(sessionTranscriptPath(ConversationScope.Scheduled, id, SessionMode.Chat, state(), listOf(run, conflicting)))
        assertTrue(cronSessionTargets(state(), listOf(run, conflicting), recorded, SessionMode.Chat).none { it.sessionId == id })
        assertTrue(cronSessionTargets(state(), listOf(run.copy(sessionScopeKey = "global")), recorded, SessionMode.Chat).none { it.sessionId == id })
        val conflictingOwner = run.copy(sessionId = a, sessionMode = "code")
        assertNull(sessionTranscriptPath(ConversationScope.Global, a, SessionMode.Chat, state(), listOf(conflictingOwner)))
        assertTrue(cronSessionTargets(state(), listOf(conflictingOwner), owners(), SessionMode.Chat).none { it.sessionId == a })
    }
}
