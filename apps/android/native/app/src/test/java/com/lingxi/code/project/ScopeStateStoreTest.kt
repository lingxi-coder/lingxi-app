package com.lingxi.code.project

import com.lingxi.code.draftForModeChange
import com.lingxi.code.sameDraftOwner
import com.lingxi.code.shouldPersistActiveSession
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
    fun `restoring a session mode does not replace the selected mode for new chats`() = runTest {
        val first = store()
        first.persistSelectedMode(SessionMode.Chat)
        first.persistActiveMode(SessionMode.Code)
        val reopened = store()
        assertEquals(SessionMode.Chat, reopened.readSelectedMode())
        assertEquals(SessionMode.Code, reopened.readActiveMode())
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
    fun `resuming another session loads its mode draft while switching the same owner carries it`() = runTest {
        val store = store()
        val scope = ConversationScope.Project("notes")
        val code = scope.sessionStateKey(SessionMode.Code)
        val chat = scope.sessionStateKey(SessionMode.Chat)
        store.persistDraft(code, "Code draft")
        store.persistDraft(chat, "Chat draft")

        val otherSession = sameDraftOwner(scope, "session-code", scope, "session-chat")
        assertEquals("Chat draft", draftForModeChange(store, chat, "Code draft", otherSession))
        assertEquals("Code draft", store.read(code)?.draft)
        assertEquals("Chat draft", store.read(chat)?.draft)

        val sameSession = sameDraftOwner(scope, "sess:19587a33-0725-48db-abca-8a2aed345f6b", scope,
            "19587a33-0725-48db-abca-8a2aed345f6b")
        assertEquals("Code draft", draftForModeChange(store, chat, "Code draft", sameSession))
        assertEquals("Code draft", store.read(chat)?.draft)
        assertEquals(false, sameDraftOwner(scope, "session-code", ConversationScope.Global, "session-code"))
    }

    @Test
    fun `provisional session cannot replace saved owner while catalog restore is unresolved`() = runTest {
        val store = store()
        val key = ConversationScope.Global.sessionStateKey(SessionMode.Chat)
        store.persistLastActiveSession(key, "saved-owner")
        if (shouldPersistActiveSession(true, "provisional", SessionMode.Chat, SessionMode.Chat, key, null)) {
            store.persistLastActiveSession(key, "provisional")
        }
        assertEquals("saved-owner", store.read(key)?.lastActiveSessionId)
        if (shouldPersistActiveSession(true, "saved-owner", SessionMode.Chat, SessionMode.Chat, key, key)) {
            store.persistLastActiveSession(key, "saved-owner")
        }
        assertEquals("saved-owner", store.read(key)?.lastActiveSessionId)
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
        assertNull(store.readSelectedMode())
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
