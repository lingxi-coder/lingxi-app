package com.lingxi.code.project

import com.lingxi.code.model.ConversationScope
import com.lingxi.code.model.SessionMode
import com.lingxi.code.model.conversationScopeFromKey
import com.lingxi.code.model.persistenceKey
import com.lingxi.code.model.sessionStateKey
import kotlinx.coroutines.test.runTest
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Rule
import org.junit.Test
import org.junit.rules.TemporaryFolder
import java.io.File

class ScopeStateStoreTest {

    @get:Rule
    val temp = TemporaryFolder()

    private fun store() = ScopeStateStore(File(temp.root, "scope-state.json"))

    @Test
    fun `scope keys round-trip through their persistence form`() {
        assertEquals("global", ConversationScope.Global.persistenceKey())
        assertEquals("project.p1", ConversationScope.Project("p1").persistenceKey())
        assertEquals("scheduled", ConversationScope.Scheduled.persistenceKey())
        assertEquals("project.p1#code", ConversationScope.Project("p1").sessionStateKey(SessionMode.Code))

        assertEquals(ConversationScope.Global, conversationScopeFromKey("global"))
        assertEquals(ConversationScope.Project("p1"), conversationScopeFromKey("project.p1"))
        assertEquals(ConversationScope.Scheduled, conversationScopeFromKey("scheduled"))
        assertNull(conversationScopeFromKey(null))
        assertNull(conversationScopeFromKey("app.tracker"))
        assertNull(conversationScopeFromKey("project."))
        assertNull(conversationScopeFromKey("weird"))
    }

    @Test
    fun `last-active session and draft persist per scope across store instances`() = runTest {
        val first = store()
        first.persistActiveScope("app.tracker")
        first.persistActiveMode(SessionMode.Chat)
        first.persistLastActiveSession("app.tracker#chat", "session-1")
        first.persistDraft("app.tracker#chat", "还没发出去的话")
        first.persistDraft("project.p1#code", "项目草稿")
        first.persistWorkspacePinned("app.tracker", 42L)
        first.persistWorkspaceCollapsed("chat:app.tracker", true)

        // A NEW instance reads the same file — durability, not memory.
        val second = store()
        assertEquals("app.tracker", second.readActiveScopeKey())
        assertEquals(SessionMode.Chat, second.readActiveMode())
        assertEquals("session-1", second.read("app.tracker#chat")?.lastActiveSessionId)
        assertEquals("还没发出去的话", second.read("app.tracker#chat")?.draft)
        assertEquals("项目草稿", second.read("project.p1#code")?.draft)
        assertEquals(42L, second.readWorkspacePresentation()["app.tracker"]?.pinnedAtEpochMillis)
        assertEquals(true, second.readWorkspacePresentation()["chat:app.tracker"]?.collapsed)
        assertNull(second.read("global"))
    }

    @Test
    fun `session pins and the scheduled-task seen marker persist and unpin cleanly`() = runTest {
        val first = store()
        first.persistSessionPinned("s1", 11L)
        first.persistSessionPinned("s2", 22L)
        first.persistSessionPinned("s1", null)
        first.persistCronResultsSeenAt(1_000L)

        val second = store()
        assertEquals(mapOf("s2" to 22L), second.readPinnedSessions())
        assertEquals(1_000L, second.readCronResultsSeenAt())
    }

    @Test
    fun `a file written before pins existed reads as no pins and never seen`() = runTest {
        File(temp.root, "scope-state.json")
            .writeText("""{"version":1,"activeMode":"chat","scopes":{}}""")
        val store = store()

        assertEquals(emptyMap<String, Long>(), store.readPinnedSessions())
        assertEquals(0L, store.readCronResultsSeenAt())
        assertEquals(SessionMode.Chat, store.readActiveMode())
    }

    @Test
    fun `a corrupt file reads as empty instead of crashing scope restore`() = runTest {
        val file = File(temp.root, "scope-state.json")
        file.writeText("{ not json")
        val store = ScopeStateStore(file)
        assertNull(store.readActiveScopeKey())
        assertEquals(SessionMode.Code, store.readActiveMode())
        store.persistActiveScope("global")
        store.persistWorkspaceCollapsed("code:global", true)
        assertEquals("global", store.readActiveScopeKey())
        assertEquals(true, store.readWorkspacePresentation()["code:global"]?.collapsed)
    }
}
