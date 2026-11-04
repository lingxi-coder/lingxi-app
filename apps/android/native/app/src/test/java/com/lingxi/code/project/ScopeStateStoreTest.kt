package com.lingxi.code.project

import com.lingxi.code.shouldPersistActiveSession
import com.lingxi.code.shouldTransferNewDraft
import com.lingxi.code.conversation.ConversationDraftWriter
import com.lingxi.code.conversation.completeHandoffDraft
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
    fun `an observed owner registry deleted while a write is staged stays missing`() = runTest {
        val file = File(temp.root, "scope-state.json")
        assertTrue(store().claimSessionOwner("19587a33-0725-48db-abca-8a2aed345f6b", "global", SessionMode.Chat))
        val deletingWriter = ProjectAtomicWriter { target, text, validate ->
            DefaultProjectAtomicWriter().write(target, text) { staged ->
                validate(staged)
                assertTrue(target.delete())
            }
        }
        val guarded = ScopeStateStore(file, deletingWriter)
        val failure = runCatching { guarded.persistConversationDraft("global", "19587a33-0725-48db-abca-8a2aed345f6b", "input") }
        assertTrue(failure.isFailure)
        assertFalse(file.exists())
    }

    @Test
    fun `a replaced owner registry is preserved when a staged write resumes`() = runTest {
        val file = File(temp.root, "scope-state.json")
        assertTrue(store().claimSessionOwner("19587a33-0725-48db-abca-8a2aed345f6b", "global", SessionMode.Chat))
        val replacement = "external replacement"
        val replacingWriter = ProjectAtomicWriter { target, text, validate ->
            DefaultProjectAtomicWriter().write(target, text) { staged ->
                validate(staged)
                target.writeText(replacement)
            }
        }
        val guarded = ScopeStateStore(file, replacingWriter)
        assertTrue(runCatching { guarded.persistConversationDraft("global", "19587a33-0725-48db-abca-8a2aed345f6b", "input") }.isFailure)
        assertEquals(replacement, file.readText())
    }

    @Test
    fun `a different inode with the same bytes is not replaced by a staged write`() = runTest {
        val file = File(temp.root, "scope-state.json")
        assertTrue(store().claimSessionOwner("19587a33-0725-48db-abca-8a2aed345f6b", "global", SessionMode.Chat))
        val original = file.readBytes()
        val moved = File(temp.root, "preserved-original.json")
        val replacingWriter = ProjectAtomicWriter { target, text, validate ->
            DefaultProjectAtomicWriter().write(target, text) { staged ->
                validate(staged)
                assertTrue(target.renameTo(moved))
                target.writeBytes(original)
            }
        }
        val guarded = ScopeStateStore(file, replacingWriter)
        assertTrue(runCatching { guarded.persistConversationDraft("global", "19587a33-0725-48db-abca-8a2aed345f6b", "input") }.isFailure)
        assertTrue(original.contentEquals(file.readBytes()))
        assertTrue(original.contentEquals(moved.readBytes()))
    }

    @Test
    fun `a concurrently created target is not overwritten by its first staged write`() {
        val target = File(temp.root, "new.json")
        val failure = runCatching {
            DefaultProjectAtomicWriter().write(target, "{}") {
                target.writeText("created concurrently")
            }
        }
        assertTrue(failure.isFailure)
        assertEquals("created concurrently", target.readText())
    }

    @Test
    fun `owner observers refresh only after a durable new claim`() = runTest {
        val first = store()
        val second = store()
        val before = second.ownerChanges.value
        assertTrue(first.claimSessionOwner("19587a33-0725-48db-abca-8a2aed345f6b", "global", SessionMode.Chat))
        assertEquals(before + 1, second.ownerChanges.value)
        assertTrue(first.claimSessionOwner("19587a33-0725-48db-abca-8a2aed345f6b", "global", SessionMode.Chat))
        assertEquals(before + 1, second.ownerChanges.value)
    }

    @Test
    fun `scheduled receipt reserves its UUID before the drawer imports it`() = runTest {
        val historyRoot = File(temp.root, "cron/history")
        val history = com.lingxi.code.cron.CronRunHistoryStore(historyRoot)
        val scope = com.lingxi.code.cron.CronScope("global", null, "Global", temp.root.path, "/guest/scheduled")
        val id = "19587a33-0725-48db-abca-8a2aed345f6b"
        val run = history.enqueue(scope, "task", "prompt", 1L)!!
        history.attachExecution(run.runId, sessionId = id, sessionTranscriptPath = "/guest/scheduled.jsonl",
            sessionMode = "chat", sessionScopeKey = "scheduled")
        val owners = store()
        assertFalse(owners.claimSessionOwner(id, "global", SessionMode.Chat))
        assertTrue(owners.readSessionOwners().isEmpty())
        assertTrue(owners.claimSessionOwner(id, "scheduled", SessionMode.Chat))
    }

    @Test
    fun `short cron reservation fences new owners while existing and confirmed owner continue`() = runTest {
        val owners = store()
        val existing = "19587a33-0725-48db-abca-8a2aed345f6b"
        val scheduled = "29587a33-0725-48db-abca-8a2aed345f6b"
        assertTrue(owners.claimSessionOwner(existing, "global", SessionMode.Chat))
        val token = checkNotNull(com.lingxi.code.conversation.SessionAdmissionGate.beginCronReservation(scheduled, "scheduled", SessionMode.Chat, null))
        try {
            assertTrue(owners.claimSessionOwner(existing, "global", SessionMode.Chat))
            assertTrue(owners.claimSessionOwner("other", "global", SessionMode.Chat))
            assertFalse(owners.claimSessionOwner(scheduled, "global", SessionMode.Chat))
            val receipt = com.lingxi.code.cron.CronRunRecord("reservation", "task", "global", null, "Global", "", 1, 1,
                sessionId = scheduled, sessionTranscriptPath = "/guest/scheduled.jsonl", sessionMode = "chat", sessionScopeKey = "scheduled")
            val deferred = owners.reconcileCronReceipts(listOf(receipt), emptyMap(), emptyMap())
            assertTrue(deferred.any { it.sessionId == scheduled })
            assertTrue(com.lingxi.code.cron.CronRunHistoryStore(File(temp.root, "cron/history")).validatedOwnerReceipts().isEmpty())
            assertTrue(com.lingxi.code.conversation.SessionAdmissionGate.permitsNewOwner("unrelated"))
            owners.reconcileCronReceipts(listOf(receipt), emptyMap(), emptyMap(), reservationToken = token)
            assertTrue(owners.claimCronSessionOwner(scheduled, "scheduled", SessionMode.Chat, token))
            com.lingxi.code.conversation.SessionAdmissionGate.finishCronReservation(token, committed = true)
            assertTrue(com.lingxi.code.conversation.SessionAdmissionGate.needsOwnerScan())
            assertTrue(owners.claimSessionOwner(existing, "global", SessionMode.Chat))
            com.lingxi.code.conversation.SessionAdmissionGate.completeOwnerScan(
                com.lingxi.code.conversation.SessionAdmissionGate.scanEpoch())
            assertTrue(owners.claimSessionOwner("other", "global", SessionMode.Chat))
        } finally {
            com.lingxi.code.conversation.SessionAdmissionGate.finishCronReservation(token, committed = false)
            com.lingxi.code.conversation.SessionAdmissionGate.completeOwnerScan(
                com.lingxi.code.conversation.SessionAdmissionGate.scanEpoch())
        }
    }

    @Test
    fun `damaged scheduled receipt index freezes new claims but leaves existing owner intact`() = runTest {
        val owners = store()
        assertTrue(owners.claimSessionOwner("first", "global", SessionMode.Chat))
        val historyRoot = File(temp.root, "cron/history")
        val history = com.lingxi.code.cron.CronRunHistoryStore(historyRoot)
        val scope = com.lingxi.code.cron.CronScope("global", null, "Global", temp.root.path, "/guest/scheduled")
        val run = history.enqueue(scope, "task", "prompt", 1L)!!
        history.attachExecution(run.runId, sessionId = "29587a33-0725-48db-abca-8a2aed345f6b", sessionTranscriptPath = "/guest/scheduled.jsonl",
            sessionMode = "chat", sessionScopeKey = "scheduled")
        val sidecar = File(historyRoot, "sessions.json")
        sidecar.writeText("damaged")
        assertTrue(owners.claimSessionOwner("first", "global", SessionMode.Chat))
        assertTrue(runCatching { owners.claimSessionOwner("second", "global", SessionMode.Chat) }.isFailure)
        assertEquals("damaged", sidecar.readText())
        assertEquals(setOf("first"), owners.readSessionOwners().keys)
    }

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
        first.persistActiveSelection("global", SessionMode.Code)
        val reopened = store()
        assertEquals(SessionMode.Chat, reopened.readSelectedMode())
        assertEquals(SessionMode.Code, reopened.readActiveMode())
    }

    @Test
    fun `each mode restores its own location and UUID ownership is immutable`() = runTest {
        val first = store()
        first.persistActiveSelection("project.a", SessionMode.Code)
        first.persistLastActiveSession("project.a#code", "code-session")
        first.persistActiveSelection("global", SessionMode.Chat)
        first.persistLastActiveSession("global#chat", "chat-session")
        val id = "19587a33-0725-48db-abca-8a2aed345f6b"
        assertTrue(first.claimSessionOwner("sess:$id", "global", SessionMode.Chat))
        assertFalse(first.claimSessionOwner(id, "project.a", SessionMode.Chat))
        assertFalse(first.claimSessionOwner(id, "global", SessionMode.Code))
        val reopened = store()
        assertEquals("project.a", reopened.readActiveScopeKey(SessionMode.Code))
        assertEquals("global", reopened.readActiveScopeKey(SessionMode.Chat))
        assertEquals("code-session", reopened.read("project.a#code")?.lastActiveSessionId)
        assertEquals(ScopeStateStore.SessionOwner("global", SessionMode.Chat), reopened.readSessionOwners()[id])
    }

    @Test
    fun `closed owner admission preserves existing claims and does not write a new one`() = runTest {
        val file = File(temp.root, "scope-state.json")
        val store = ScopeStateStore(file)
        val first = "19587a33-0725-48db-abca-8a2aed345f6b"
        val second = "4190879f-18f0-4706-9f1c-c0ea8cfaac73"
        assertTrue(store.claimSessionOwner(first, "global", SessionMode.Chat))
        val original = file.readText()
        assertTrue(store.claimSessionOwner(first, "global", SessionMode.Chat) { false })
        assertFalse(store.claimSessionOwner(second, "global", SessionMode.Chat) { false })
        assertEquals(original, file.readText())
    }

    @Test
    fun `storage fault wins against a new claim waiting at admission`() = runTest {
        val file = File(temp.root, "scope-state.json")
        val store = ScopeStateStore(file)
        store.persistSelectedMode(SessionMode.Chat)
        val original = file.readText()
        val allowed = java.util.concurrent.atomic.AtomicBoolean(true)
        val attempted = CountDownLatch(1)
        val claim = synchronized(com.lingxi.code.conversation.SessionAdmissionGate.lock) {
            async(Dispatchers.IO) {
                attempted.countDown()
                store.claimSessionOwner("19587a33-0725-48db-abca-8a2aed345f6b", "global", SessionMode.Chat) { allowed.get() }
            }.also {
                assertTrue(attempted.await(2, TimeUnit.SECONDS))
                allowed.set(false)
            }
        }
        assertFalse(claim.await())
        assertEquals(original, file.readText())
    }

    @Test
    fun `failed navigation admission leaves the selected workspace intact`() = runTest {
        val file = File(temp.root, "scope-state.json")
        val store = ScopeStateStore(file)
        assertTrue(store.persistActiveSelection("global", SessionMode.Chat))
        val original = file.readText()
        assertFalse(store.persistActiveSelection("project.p", SessionMode.Code) { false })
        assertEquals(original, file.readText())
        assertEquals("global", store.readActiveScopeKey(SessionMode.Chat))
        assertTrue(store.persistLastActiveSession("global#chat", "first"))
        val withSession = file.readText()
        assertFalse(store.persistLastActiveSession("global#chat", "superseded") { false })
        assertEquals(withSession, file.readText())
        assertEquals("first", store.read("global#chat")?.lastActiveSessionId)
    }

    @Test
    fun `handoff draft and backlink survive UTF16 units and a new store instance`() = runTest {
        val draft = "before\uD800middle\uDC00 after \uD83D\uDE03"
        val sourceId = "19587a33-0725-48db-abca-8a2aed345f6b"
        val handoff = ScopeStateStore.ConversationHandoff("project.a", "sess:$sourceId", SessionMode.Code,
            SessionMode.Chat, "cutoff", "request")
        store().persistHandoffDraft("global", "target", draft, handoff)
        val reopened = store()
        assertEquals(draft.toList(), reopened.readConversationDraft("global", "target")?.toList())
        assertEquals(handoff.copy(sourceSessionId = sourceId), reopened.readConversationHandoff("global", "target"))
    }

    @Test
    fun `handoff retains input entered during its durable write`() = runTest {
        val store = store()
        val target = "19587a33-0725-48db-abca-8a2aed345f6b"
        val handoff = ScopeStateStore.ConversationHandoff("global", "29587a33-0725-48db-abca-8a2aed345f6b",
            SessionMode.Code, SessionMode.Chat, "cutoff", "request")
        val writer = ConversationDraftWriter(this)
        val key = ConversationDraftWriter.Key("global", target)
        val persist: suspend (ConversationDraftWriter.Key, String) -> Unit = { draftKey, value ->
            store.persistConversationDraft(draftKey.scopeKey, draftKey.sessionId, value)
        }
        val entered = CompletableDeferred<Unit>()
        val release = CompletableDeferred<Unit>()
        var draft = ""
        val completion = async {
            completeHandoffDraft("summary", persistHandoff = { value ->
                entered.complete(Unit)
                release.await()
                store.persistHandoffDraft("global", target, value, handoff)
            }, currentDraft = { draft }, publishDraft = { value ->
                draft = value
                writer.write(key, value, persist) { throw it }
            })
        }
        entered.await()
        draft = "new user input"
        writer.write(key, draft, persist) { throw it }.join()
        release.complete(Unit)
        completion.await()
        writer.await()
        assertEquals("summary\n\nnew user input", draft)
        assertEquals(draft, store().readConversationDraft("global", target))
        assertEquals(handoff, store().readConversationHandoff("global", target))
    }

    @Test
    fun `handoff storage itself preserves a previously durable user edit`() = runTest {
        val store = store()
        val target = "19587a33-0725-48db-abca-8a2aed345f6b"
        val handoff = ScopeStateStore.ConversationHandoff("global", "29587a33-0725-48db-abca-8a2aed345f6b",
            SessionMode.Code, SessionMode.Chat, "cutoff", "request")
        store.persistConversationDraft("global", target, "already saved input")
        store.persistHandoffDraft("global", target, "summary", handoff)
        assertEquals("summary\n\nalready saved input", store().readConversationDraft("global", target))
        assertEquals(handoff, store().readConversationHandoff("global", target))
    }

    @Test
    fun `handoff publishes saved input when the owner has not loaded its draft`() = runTest {
        val store = store()
        val target = "19587a33-0725-48db-abca-8a2aed345f6b"
        val handoff = ScopeStateStore.ConversationHandoff("global", "29587a33-0725-48db-abca-8a2aed345f6b",
            SessionMode.Code, SessionMode.Chat, "cutoff", "request")
        store.persistConversationDraft("global", target, "saved before loading")
        var published = ""
        completeHandoffDraft("summary", { store.persistHandoffDraft("global", target, it, handoff) },
            currentDraft = { null }, publishDraft = { published = it })
        assertEquals("summary\n\nsaved before loading", published)
        assertEquals(published, store().readConversationDraft("global", target))
    }

    @Test
    fun `handoff does not publish to a source replaced during its durable write`() = runTest {
        val store = store()
        val target = "19587a33-0725-48db-abca-8a2aed345f6b"
        val handoff = ScopeStateStore.ConversationHandoff("global", "29587a33-0725-48db-abca-8a2aed345f6b",
            SessionMode.Code, SessionMode.Chat, "cutoff", "request")
        val entered = CompletableDeferred<Unit>()
        val release = CompletableDeferred<Unit>()
        var sourceStillMatches = true
        var published = false
        val completion = async {
            runCatching {
                completeHandoffDraft("summary", persistHandoff = { value ->
                    entered.complete(Unit)
                    release.await()
                    store.persistHandoffDraft("global", target, value, handoff)
                }, currentDraft = {
                    check(sourceStillMatches)
                    ""
                }, publishDraft = { published = true })
            }
        }
        entered.await()
        sourceStillMatches = false
        release.complete(Unit)
        assertTrue(completion.await().isFailure)
        assertFalse(published)
        assertEquals("summary", store().readConversationDraft("global", target))
        assertEquals(handoff, store().readConversationHandoff("global", target))
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
                second.persistActiveSelection("global", SessionMode.Chat)
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
    fun `a removed observed owner file cannot be recreated by any store instance`() = runTest {
        val file = File(temp.root, "scope-state.json")
        val first = ScopeStateStore(file)
        val owner = "19587a33-0725-48db-abca-8a2aed345f6b"
        assertTrue(first.claimSessionOwner(owner, "global", SessionMode.Chat))
        first.persistConversationDraft("global", owner, "retained draft")
        val original = file.readText()
        assertTrue(file.delete())

        val reopened = ScopeStateStore(file)
        assertTrue(runCatching { reopened.persistSelectedMode(SessionMode.Code) }.isFailure)
        assertTrue(runCatching { first.persistConversationDraft("global", owner, "replacement") }.isFailure)
        assertTrue(runCatching { reopened.claimSessionOwner("another", "global", SessionMode.Chat) }.isFailure)
        assertFalse(file.exists())

        file.writeText(original)
        assertEquals("retained draft", reopened.readConversationDraft("global", owner))
        assertEquals(ScopeStateStore.SessionOwner("global", SessionMode.Chat), reopened.readSessionOwners()[owner])
    }

    @Test
    fun `drafts failed during owner file loss are written after repair without new input`() = runTest {
        val file = File(temp.root, "scope-state.json")
        val store = ScopeStateStore(file)
        val first = "19587a33-0725-48db-abca-8a2aed345f6b"
        val second = "4190879f-18f0-4706-9f1c-c0ea8cfaac73"
        assertTrue(store.claimSessionOwner(first, "global", SessionMode.Chat))
        assertTrue(store.claimSessionOwner(second, "global", SessionMode.Code))
        store.persistConversationDraft("global", first, "old first")
        store.persistConversationDraft("global", second, "old second")
        val original = file.readText()
        assertTrue(file.delete())

        val writer = ConversationDraftWriter(this)
        val failures = mutableListOf<Exception>()
        val persist: suspend (ConversationDraftWriter.Key, String) -> Unit = { key, text ->
            store.persistConversationDraft(key.scopeKey, key.sessionId, text)
        }
        val firstKey = ConversationDraftWriter.Key("global", first)
        val secondKey = ConversationDraftWriter.Key("global", second)
        writer.write(firstKey, "intermediate", persist, failures::add).join()
        writer.write(firstKey, "last first", persist, failures::add).join()
        writer.write(secondKey, "last second", persist, failures::add).join()
        assertEquals(3, failures.size)
        assertFalse(file.exists())

        file.writeText(original)
        writer.retry(persist, failures::add).forEach { it.join() }
        val reopened = ScopeStateStore(file)
        assertEquals("last first", reopened.readConversationDraft("global", first))
        assertEquals("last second", reopened.readConversationDraft("global", second))
        assertTrue(writer.retry(persist, failures::add).isEmpty())
    }

    @Test
    fun `new input remains last when an older draft retry is already in flight`() = runTest {
        val writer = ConversationDraftWriter(this)
        val key = ConversationDraftWriter.Key("global", "owner")
        writer.write(key, "older", { _, _ -> error("storage unavailable") }, {}).join()
        val retryStarted = CompletableDeferred<Unit>()
        val finishRetry = CompletableDeferred<Unit>()
        var stored = ""
        val persist: suspend (ConversationDraftWriter.Key, String) -> Unit = { _, value ->
            if (value == "older") {
                retryStarted.complete(Unit)
                finishRetry.await()
            }
            stored = value
        }
        val retry = writer.retry(persist) { throw AssertionError(it) }.single()
        retryStarted.await()
        val latest = writer.write(key, "newer", persist) { throw AssertionError(it) }
        finishRetry.complete(Unit)
        retry.join()
        latest.join()
        assertEquals("newer", stored)
        assertTrue(writer.retry(persist) { throw AssertionError(it) }.isEmpty())
    }

    @Test
    fun `last-active session and draft persist per scope across store instances`() = runTest {
        val first = store()
        first.persistActiveSelection("app.tracker", SessionMode.Chat)
        first.persistLastActiveSession("app.tracker#chat", "session-1")
        first.persistConversationDraft("app.tracker", "session-1", "还没发出去的话")
        first.persistConversationDraft("project.p1", "session-2", "项目草稿")
        first.persistWorkspacePinned("app.tracker", 42L)
        first.persistWorkspaceCollapsed("chat:app.tracker", true)

        // A NEW instance reads the same file — durability, not memory.
        val second = store()
        assertEquals("app.tracker", second.readActiveScopeKey(SessionMode.Chat))
        assertEquals(SessionMode.Chat, second.readActiveMode())
        assertEquals("session-1", second.read("app.tracker#chat")?.lastActiveSessionId)
        assertEquals("还没发出去的话", second.readConversationDraft("app.tracker", "session-1"))
        assertEquals("项目草稿", second.readConversationDraft("project.p1", "session-2"))
        assertEquals(42L, second.readWorkspacePresentation()["app.tracker"]?.pinnedAtEpochMillis)
        assertEquals(true, second.readWorkspacePresentation()["chat:app.tracker"]?.collapsed)
        assertNull(second.read("global"))
    }

    @Test
    fun `conversation drafts stay isolated within a workspace`() = runTest {
        val store = store()
        val scope = ConversationScope.Project("notes").persistenceKey()
        val session = "19587a33-0725-48db-abca-8a2aed345f6b"
        store.persistConversationDraft(scope, "sess:$session", "first draft")
        store.persistConversationDraft(scope, "other", "second draft")
        store.persistConversationDraft(ConversationScope.Global.persistenceKey(), session, "global draft")

        val reopened = store()
        assertEquals("first draft", reopened.readConversationDraft(scope, session))
        assertEquals("second draft", reopened.readConversationDraft(scope, "other"))
        assertEquals("global draft", reopened.readConversationDraft("global", session))
        assertNull(reopened.readConversationDraft(scope, "never-seen"))
        reopened.persistConversationDraft(scope, session, "")
        assertEquals("", store().readConversationDraft(scope, session))
    }

    @Test
    fun `old scope format is rejected without overwriting its file`() = runTest {
        val file = File(temp.root, "scope-state.json")
        val old = """{"version":1,"scopes":{"global#code":{"lastActiveSessionId":"old","draft":"old draft"}}}"""
        file.writeText(old)
        assertTrue(runCatching { store().readConversationDraft("global", "old") }.isFailure)
        assertTrue(runCatching { store().persistConversationDraft("global", "new-owner", "new") }.isFailure)
        assertEquals(old, file.readText())
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
    fun `a corrupt file is rejected and retained`() = runTest {
        val file = File(temp.root, "scope-state.json")
        file.writeText("{ not json")
        val store = ScopeStateStore(file)
        assertTrue(runCatching { store.readActiveScopeKey(SessionMode.Code) }.isFailure)
        assertTrue(runCatching { store.persistWorkspaceCollapsed("code:global", true) }.isFailure)
        assertEquals("{ not json", file.readText())
    }

    @Test
    fun `a state path that is a directory is rejected and retained`() = runTest {
        val path = File(temp.root, "scope-state.json")
        assertTrue(path.mkdir())
        val marker = File(path, "keep")
        marker.writeText("retained")
        val store = ScopeStateStore(path)
        assertTrue(runCatching { store.validate() }.isFailure)
        assertTrue(runCatching { store.persistSelectedMode(SessionMode.Chat) }.isFailure)
        assertEquals("retained", marker.readText())
    }

    @Test
    fun `a broken state symlink is not replaced with a new snapshot`() = runTest {
        val path = File(temp.root, "scope-state.json")
        java.nio.file.Files.createSymbolicLink(path.toPath(), File(temp.root, "missing").toPath())
        val store = ScopeStateStore(path)
        assertTrue(runCatching { store.validate() }.isFailure)
        assertTrue(runCatching { store.persistSelectedMode(SessionMode.Chat) }.isFailure)
        assertTrue(java.nio.file.Files.isSymbolicLink(path.toPath()))
    }

    @Test
    fun `noncanonical or invalid owner records are rejected without rewriting`() = runTest {
        val file = File(temp.root, "scope-state.json")
        val store = ScopeStateStore(file)
        store.persistSelectedMode(SessionMode.Chat)
        val valid = org.json.JSONObject(file.readText())
        val uuid = "19587a33-0725-48db-abca-8a2aed345f6b"
        for ((id, scope) in listOf("sess:$uuid" to "global", uuid to "app.old")) {
            val invalid = org.json.JSONObject(valid.toString()).apply {
                getJSONObject("sessionOwners").put(id, org.json.JSONObject().put("scope", scope).put("mode", "chat"))
            }.toString()
            file.writeText(invalid)
            assertTrue(runCatching { store.validate() }.isFailure)
            assertTrue(runCatching { store.persistSelectedMode(SessionMode.Code) }.isFailure)
            assertEquals(invalid, file.readText())
        }
    }

    @Test
    fun `invalid pin data cannot be discarded by an unrelated session write`() = runTest {
        val file = File(temp.root, "scope-state.json")
        val store = ScopeStateStore(file)
        store.persistSessionPinned("session-a", 123L)
        val invalid = org.json.JSONObject(file.readText()).apply {
            getJSONObject("pinnedSessions").put("session-a", "invalid")
        }.toString()
        file.writeText(invalid)
        assertTrue(runCatching { store.persistActiveSelection("global", SessionMode.Chat) }.isFailure)
        assertEquals(invalid, file.readText())
    }
}
