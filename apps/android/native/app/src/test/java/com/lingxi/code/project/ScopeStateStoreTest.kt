package com.lingxi.code.project

import com.lingxi.code.shouldPersistActiveSession
import com.lingxi.code.shouldTransferNewDraft
import com.lingxi.code.model.ConversationScope
import com.lingxi.code.model.SessionMode
import com.lingxi.code.model.conversationScopeFromKey
import com.lingxi.code.model.persistenceKey
import com.lingxi.code.model.sessionStateKey
import kotlinx.coroutines.test.runTest
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.async
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Rule
import org.junit.Test
import org.junit.rules.TemporaryFolder
import java.io.File
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit

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
    fun `recreated store instances serialize changes to the same file`() = runTest {
        val file = File(temp.root, "scope-state.json")
        val firstWriteStarted = CompletableDeferred<Unit>()
        val releaseFirstWrite = CountDownLatch(1)
        val first = ScopeStateStore(file, ProjectAtomicWriter { target, text, validate ->
            firstWriteStarted.complete(Unit)
            check(releaseFirstWrite.await(5, TimeUnit.SECONDS))
            validate(text)
            target.writeText(text)
        })
        val second = ScopeStateStore(file)
        val preference = async { first.persistSelectedMode(SessionMode.Chat) }
        try {
            firstWriteStarted.await()
            val secondStarted = CompletableDeferred<Unit>()
            val active = async(Dispatchers.IO) {
                secondStarted.complete(Unit)
                second.persistActiveMode(SessionMode.Chat)
            }
            secondStarted.await()
            releaseFirstWrite.countDown()
            preference.await()
            active.await()
            assertEquals(SessionMode.Chat, second.readSelectedMode())
            assertEquals(SessionMode.Chat, second.readActiveMode())
        } finally {
            releaseFirstWrite.countDown()
        }
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
    fun `drafts follow their session across mode changes and stay isolated within a workspace`() = runTest {
        val store = store()
        val scope = ConversationScope.Project("notes").persistenceKey()
        val session = "19587a33-0725-48db-abca-8a2aed345f6b"
        store.persistConversationDraft(scope, "sess:$session", "first draft")
        store.persistConversationDraft(scope, "other", "second draft")
        store.persistConversationDraft(ConversationScope.Global.persistenceKey(), session, "global draft")

        val reopened = store()
        assertEquals("first draft", reopened.readConversationDraft(scope, session, SessionMode.Code))
        assertEquals("first draft", reopened.readConversationDraft(scope, session, SessionMode.Chat))
        assertEquals("second draft", reopened.readConversationDraft(scope, "other", SessionMode.Code))
        assertEquals("global draft", reopened.readConversationDraft("global", session, SessionMode.Code))
        assertNull(reopened.readConversationDraft(scope, "never-seen", SessionMode.Code))
        reopened.persistConversationDraft(scope, session, "")
        assertEquals("", store().readConversationDraft(scope, session, SessionMode.Chat))
    }

    @Test
    fun `legacy draft remains with its recorded owner when the active session changes`() = runTest {
        File(temp.root, "scope-state.json").writeText(
            """{"version":1,"scopes":{"global#code":{"lastActiveSessionId":"old","draft":"old draft"}}}""",
        )
        val store = store()
        assertEquals("old draft", store.readConversationDraft("global", "old", SessionMode.Code))
        store.persistLastActiveSession("global#code", "new-owner")
        assertNull(store.readConversationDraft("global", "new-owner", SessionMode.Code))
        assertEquals("old draft", store.readConversationDraft("global", "old", SessionMode.Chat))
        store.persistConversationDraft("global", "new-owner", "new draft")
        assertEquals("new draft", store().readConversationDraft("global", "new-owner", SessionMode.Code))
        assertEquals("old draft", store().readConversationDraft("global", "old", SessionMode.Code))
    }

    @Test
    fun `a temporary new-chat draft only transfers to its started conversation`() {
        val global = ConversationScope.Global
        val project = ConversationScope.Project("notes")
        assertTrue(shouldTransferNewDraft(global, "new", global, "started", false, true, global))
        assertFalse(shouldTransferNewDraft(global, "new", global, "resumed-empty", false, true, null))
        assertFalse(shouldTransferNewDraft(global, "new", global, "resumed", true, true, global))
        assertFalse(shouldTransferNewDraft(global, "new", project, "started", false, true, global))
        assertFalse(shouldTransferNewDraft(global, "existing", global, "started", false, true, global))
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
