package com.lingxi.code.project

import com.lingxi.code.model.ConversationScope
import com.lingxi.code.model.conversationScopeFromKey
import com.lingxi.code.model.persistenceKey
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
        assertEquals("app.tracker", ConversationScope.LocalApp("tracker").persistenceKey())

        assertEquals(ConversationScope.Global, conversationScopeFromKey("global"))
        assertEquals(ConversationScope.Project("p1"), conversationScopeFromKey("project.p1"))
        assertEquals(ConversationScope.LocalApp("tracker"), conversationScopeFromKey("app.tracker"))
        assertNull(conversationScopeFromKey(null))
        assertNull(conversationScopeFromKey("app."))
        assertNull(conversationScopeFromKey("weird"))
    }

    @Test
    fun `last-active session and draft persist per scope across store instances`() = runTest {
        val first = store()
        first.persistActiveScope("app.tracker")
        first.persistLastActiveSession("app.tracker", "session-1")
        first.persistDraft("app.tracker", "还没发出去的话")
        first.persistDraft("project.p1", "项目草稿")

        // A NEW instance reads the same file — durability, not memory.
        val second = store()
        assertEquals("app.tracker", second.readActiveScopeKey())
        assertEquals("session-1", second.read("app.tracker")?.lastActiveSessionId)
        assertEquals("还没发出去的话", second.read("app.tracker")?.draft)
        assertEquals("项目草稿", second.read("project.p1")?.draft)
        assertNull(second.read("global"))
    }

    @Test
    fun `a corrupt file reads as empty instead of crashing scope restore`() = runTest {
        val file = File(temp.root, "scope-state.json")
        file.writeText("{ not json")
        val store = ScopeStateStore(file)
        assertNull(store.readActiveScopeKey())
        store.persistActiveScope("global")
        assertEquals("global", store.readActiveScopeKey())
    }
}
