package com.lingxi.code.apps

import com.lingxi.code.project.ProjectAtomicWriter
import com.lingxi.code.model.SessionMode
import kotlinx.coroutines.async
import kotlinx.coroutines.awaitAll
import kotlinx.coroutines.runBlocking
import org.junit.Assert.*
import org.junit.Rule
import org.junit.Test
import org.junit.rules.TemporaryFolder
import java.io.File
import java.util.UUID

class AuthoringIntentStoreTest {
    private suspend fun AuthoringIntentStore.saveRequest(request: AuthoringRequest) = saveRequest(request, SessionMode.Code)
    private suspend fun AuthoringIntentStore.savePrepared(intent: AuthoringIntent) = savePrepared(intent, SessionMode.Code)
    @get:Rule val temporary = TemporaryFolder()
    private val writer = ProjectAtomicWriter { file, text, validate -> validate(text); file.writeText(text) }
    private fun prepared() = AuthoringIntent(UUID.randomUUID().toString(), UUID.randomUUID().toString(), UUID.randomUUID().toString(),
        "/home/user/.brand/.apps/example", UUID.randomUUID().toString(), UUID.randomUUID().toString(), null)

    @Test fun persistsExactIndependentOwnersAndRejectsChangedTuple() = runBlocking {
        val directory = File(temporary.root, "intents")
        val first = prepared()
        val second = prepared()
        val storeA = AuthoringIntentStore(directory, writer)
        val storeB = AuthoringIntentStore(directory, writer)
        awaitAll(async { storeA.savePrepared(first) }, async { storeB.savePrepared(second) })
        val sessionA = UUID.randomUUID().toString()
        val sessionB = UUID.randomUUID().toString()
        val attachedA = storeA.attach(first.selectionId, sessionA, "/transcripts/a.jsonl", SessionMode.Code, trackLive = false)
        val attachedB = storeB.attach(second.selectionId, sessionB, "/transcripts/b.jsonl", SessionMode.Code, trackLive = false)
        assertEquals(attachedA, AuthoringIntentStore(directory, writer).read(sessionA))
        assertEquals(attachedB, storeA.read(sessionB))
        val file = File(directory, "source-${first.selectionId}.json")
        val bytes = file.readBytes()
        assertTrue(runCatching { storeA.savePrepared(first.copy(sourceId = UUID.randomUUID().toString())) }.isFailure)
        assertArrayEquals(bytes, file.readBytes())
        assertEquals(setOf(attachedA, attachedB), storeA.staged().toSet())
        storeA.markCatalogued(sessionA)
        assertEquals(listOf(attachedB), storeA.staged())
    }

    @Test fun corruptedOrMissingProofDoesNotDamageAnotherOwner() = runBlocking {
        val directory = File(temporary.root, "intents")
        val store = AuthoringIntentStore(directory, writer)
        val first = prepared()
        val second = prepared()
        assertNull(store.readSelection(first.selectionId))
        store.savePrepared(first)
        store.savePrepared(second)
        val file = File(directory, "source-${first.selectionId}.json")
        file.writeText("{broken")
        val bytes = file.readBytes()
        assertTrue(runCatching { store.savePrepared(first) }.isFailure)
        assertArrayEquals(bytes, file.readBytes())
        assertEquals(second, store.readSelection(second.selectionId))
    }

    @Test fun stagedOwnerBecomesDiscoverableOnlyAfterItsLiveAttemptEnds() = runBlocking {
        val store = AuthoringIntentStore(File(temporary.root, "intents"), writer)
        val item = prepared()
        val session = UUID.randomUUID().toString()
        store.savePrepared(item)
        assertEquals(item, store.readSelection(item.selectionId))
        assertTrue(store.staged().isEmpty())
        val attached = store.attach(item.selectionId, session, "/transcripts/exact.jsonl", SessionMode.Code)
        assertTrue(store.staged().isEmpty())
        store.releaseLive(session)
        assertEquals(listOf(attached), store.staged())
        store.markCatalogued(session)
        assertTrue(store.staged().isEmpty())
        assertEquals(attached.copy(catalogued = true), store.read(session))
    }

    @Test fun requestIdAndPayloadAreDurableBeforeSourceExists() = runBlocking {
        val directory = File(temporary.root, "intents")
        val store = AuthoringIntentStore(directory, writer)
        val request = AuthoringRequest(UUID.randomUUID().toString(), "create", "Counter",
            UUID.randomUUID().toString(), UUID.randomUUID().toString())
        store.saveRequest(request)
        assertEquals(request, AuthoringIntentStore(directory, writer).readRequest(request.selectionId))
        assertNull(store.readSelection(request.selectionId))
        assertTrue(runCatching { store.saveRequest(request.copy(name = "Other")) }.isFailure)
    }

    @Test fun chatAndUnknownModesCannotWriteRequestsOrSourceIntents() = runBlocking {
        val directory = File(temporary.root, "intents")
        val store = AuthoringIntentStore(directory, writer)
        val item = prepared()
        val request = AuthoringRequest(item.selectionId, "create", "Counter", item.projectId, item.workspaceId)
        for (mode in listOf(SessionMode.Chat, null)) {
            val requestFailure = runCatching { store.saveRequest(request, mode) }.exceptionOrNull()
            val sourceFailure = runCatching { store.savePrepared(item, mode) }.exceptionOrNull()
            assertTrue(requestFailure is AppDevelopmentRequiresCodeException)
            assertTrue(sourceFailure is AppDevelopmentRequiresCodeException)
            assertEquals(APP_DEVELOPMENT_REQUIRES_CODE, requestFailure?.message)
            assertFalse(directory.exists())
        }
    }

    @Test fun sourceNameValidationPrecedesPersistenceAndCountsUnicodeScalars() = runBlocking {
        val directory = File(temporary.root, "intents")
        val store = AuthoringIntentStore(directory, writer)
        fun request(name: String, operation: String = "create") = AuthoringRequest(
            UUID.randomUUID().toString(), operation, name, UUID.randomUUID().toString(), UUID.randomUUID().toString(),
            appId = if (operation == "remix") "app" else null, revision = if (operation == "remix") 1 else null)
        val emoji = "\uD83D\uDE00"
        listOf("a".repeat(80), emoji.repeat(80), "e\u0301".repeat(40), "\u001c").forEach {
            val valid = request(it)
            store.saveRequest(valid)
            assertEquals(valid, store.readRequest(valid.selectionId))
        }
        listOf("a".repeat(81), emoji.repeat(81), "e\u0301".repeat(41), "\u0085\u00a0", "\uD800", "\uDC00").forEach { name ->
            for (operation in listOf("create", "remix")) {
                val invalid = request(name, operation)
                assertTrue(runCatching { store.saveRequest(invalid) }.exceptionOrNull() is InvalidAuthoringNameException)
                assertFalse(File(directory, "request-${invalid.selectionId}.json").exists())
            }
        }
    }

    @Test fun independentRequestsAndAttachedSessionsRemainExact() = runBlocking {
        val directory = File(temporary.root, "intents")
        val store = AuthoringIntentStore(directory, writer)
        val first = prepared()
        val second = prepared()
        val requestA = AuthoringRequest(first.selectionId, "create", "A", first.projectId, first.workspaceId)
        val requestB = AuthoringRequest(second.selectionId, "create", "B", second.projectId, second.workspaceId)
        store.saveRequest(requestA)
        store.saveRequest(requestB)
        store.savePrepared(first)
        store.savePrepared(second)
        val session = UUID.randomUUID().toString()
        assertTrue(runCatching { store.attach(first.selectionId, session, "/a.jsonl", SessionMode.Chat) }.isFailure)
        val attached = store.attach(first.selectionId, session, "/a.jsonl", SessionMode.Code, trackLive = false)
        assertTrue(runCatching { store.attach(first.selectionId, UUID.randomUUID().toString(), "/b.jsonl", SessionMode.Code) }.isFailure)
        assertEquals(attached, AuthoringIntentStore(directory, writer).read(session))
        assertEquals(requestB, store.readRequest(second.selectionId))
        assertEquals(second, store.readSelection(second.selectionId))
    }

    @Test fun appsRecoveryUsesExactProjectPointerUntilReceiptAndRequestAreBothConfirmed() = runBlocking {
        val directory = File(temporary.root, "intents")
        val store = AuthoringIntentStore(directory, writer)
        val item = prepared()
        val request = AuthoringRequest(item.selectionId, "create", "A", item.projectId, item.workspaceId)
        val row = PendingAuthoringRecovery(item.projectId, item.selectionId, "A")
        val other = row.copy(projectId = UUID.randomUUID().toString())
        store.saveRequest(request)
        assertEquals(listOf(row), pendingAuthoringRecoveries(listOf(row, other), store))
        store.savePrepared(item)
        val session = UUID.randomUUID().toString()
        store.attach(item.selectionId, session, "/transcripts/a.jsonl", SessionMode.Code, trackLive = false)
        store.markCatalogued(session)
        assertEquals(listOf(row), pendingAuthoringRecoveries(listOf(row), store))
        store.markBindingConfirmed(item.selectionId, session)
        assertEquals(listOf(row), pendingAuthoringRecoveries(listOf(row), store))
        store.markRequestCompleted(item.selectionId)
        assertTrue(pendingAuthoringRecoveries(listOf(row), AuthoringIntentStore(directory, writer)).isEmpty())
        assertTrue(runCatching { store.saveRequest(request.copy(name = "Other")) }.isFailure)
    }

    @Test fun existingOwnerRebindKeepsNewRequestDiscoverableUntilCompleted() = runBlocking {
        val store = AuthoringIntentStore(File(temporary.root, "intents"), writer)
        val old = prepared()
        store.savePrepared(old)
        val session = UUID.randomUUID().toString()
        store.attach(old.selectionId, session, "/transcripts/a.jsonl", SessionMode.Code, trackLive = false)
        store.markCatalogued(session)
        store.markBindingConfirmed(old.selectionId, session)
        val request = AuthoringRequest(UUID.randomUUID().toString(), "bind", "A", old.projectId, old.workspaceId,
            "app", sourceId = old.sourceId, sourceDirectory = old.sourceDirectory,
            targetSessionId = session, targetTranscriptPath = "/transcripts/a.jsonl")
        store.saveRequest(request)
        val row = PendingAuthoringRecovery(old.projectId, request.selectionId, "A")
        assertEquals(listOf(row), pendingAuthoringRecoveries(listOf(row), store))
        assertEquals(old.selectionId, store.read(session)?.selectionId)
        store.markRequestCompleted(request.selectionId)
        assertTrue(pendingAuthoringRecoveries(listOf(row), store).isEmpty())
    }
}
