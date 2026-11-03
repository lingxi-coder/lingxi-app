package com.lingxi.code.apps

import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.async
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.yield
import org.junit.Assert.*
import org.junit.Test
import java.util.UUID
import com.lingxi.code.model.SessionMode
import com.lingxi.code.project.SessionWorkspaceState

class AuthoringRegistrationTest {
    private class NotFound : Exception()

    @Test fun chatIsRejectedBeforeSourceWorkAndAcceptedCodeWorkSurvivesNavigation() = runBlocking {
        assertFalse(authoringEntryAllowed(SessionMode.Chat, SessionMode.Chat, SessionMode.Chat, true))
        assertFalse(authoringEntryAllowed(SessionMode.Code, SessionMode.Chat, SessionMode.Code, true))
        assertFalse(authoringEntryAllowed(SessionMode.Code, null, SessionMode.Code, true))
        assertFalse(authoringEntryAllowed(SessionMode.Code, SessionMode.Code, null, true))
        assertFalse(authoringEntryAllowed(SessionMode.Code, SessionMode.Code, SessionMode.Code, false))
        assertTrue(authoringEntryAllowed(SessionMode.Code, SessionMode.Code, SessionMode.Code, true))
        assertFalse(isRuntimeAppRequest("publish"))
        assertFalse(isRuntimeAppRequest("import"))
        assertFalse(isRuntimeAgentRequest("createAuthoring"))
        assertTrue(isRuntimeAppRequest("start"))
        assertTrue(isRuntimeAgentRequest("tasks.cancel"))
        var writes = 0
        for (mode in listOf(SessionMode.Chat, null)) {
            val failure = runCatching { requireCodeAuthoring(mode); writes++ }.exceptionOrNull()
            assertTrue(failure is AppDevelopmentRequiresCodeException)
            assertEquals("local_app_development_requires_code", failure?.message)
        }
        assertEquals(0, writes)
        val started = CompletableDeferred<Unit>()
        val release = CompletableDeferred<Unit>()
        var current = true
        val work = async {
            requireCodeAuthoring(SessionMode.Code)
            awaitAdmittedAuthoringSource(submit = { started.complete(Unit); release.await(); writes++; "source-a" },
                navigationIsCurrent = { current })
        }
        started.await()
        current = false
        release.complete(Unit)
        assertNull(work.await())
        assertEquals(1, writes)
    }

    @Test fun lostAcknowledgementUsesReceiptWithoutSubmittingTwice() = runBlocking {
        var submissions = 0
        val receipt = registerAuthoringWithReceipt(
            submit = { submissions++; error("acknowledgement lost") }, lookup = { "owner-a" },
            isNotFound = { it is NotFound }, matches = { it == "owner-a" },
        )
        assertEquals("owner-a", receipt)
        assertEquals(1, submissions)
    }

    @Test fun onlyMissingReceiptPermitsOneRetry() = runBlocking {
        var submissions = 0
        var reads = 0
        val result = registerAuthoringWithReceipt(
            submit = { if (++submissions == 1) error("unknown submission") },
            lookup = { if (++reads == 1) throw NotFound(); "owner-a" },
            isNotFound = { it is NotFound }, matches = { it == "owner-a" },
        )
        assertEquals("owner-a", result)
        assertEquals(2, submissions)
        assertEquals(2, reads)
    }

    @Test fun conflictOrUnknownLookupNeverRetries() = runBlocking {
        var submissions = 0
        assertTrue(runCatching {
            registerAuthoringWithReceipt(
                submit = { submissions++; error("unknown submission") }, lookup = { "different-owner" },
                isNotFound = { it is NotFound }, matches = { it == "owner-a" },
            )
        }.isFailure)
        assertEquals(1, submissions)
        assertTrue(runCatching {
            registerAuthoringWithReceipt<Unit>(
                submit = { submissions++; error("unknown submission") }, lookup = { error("lookup unavailable") },
                isNotFound = { it is NotFound }, matches = { true },
            )
        }.isFailure)
        assertEquals(2, submissions)
    }

    @Test fun explicitRetryChecksForAnEarlierReceiptBeforeAnotherSubmission() = runBlocking {
        var submissions = 0
        val result = registerAuthoringWithReceipt(
            submit = { submissions++ }, lookup = { "owner-a" }, isNotFound = { it is NotFound },
            matches = { it == "owner-a" }, lookupBeforeSubmit = true,
        )
        assertEquals("owner-a", result)
        assertEquals(0, submissions)
    }

    @Test fun oldOwnerReceiptCannotAcknowledgeANewAppBindingRequest() = runBlocking {
        var submissions = 0
        var reads = 0
        assertTrue(runCatching {
            registerAuthoringWithReceipt(
                submit = { submissions++; error("binding outcome unknown") },
                lookup = { reads++; "owner-a" }, isNotFound = { it is NotFound },
                matches = { it == "owner-a" }, lookupBeforeSubmit = true, receiptProvesSubmission = false,
            )
        }.isFailure)
        assertEquals(1, submissions)
        assertEquals(0, reads)
        val result = registerAuthoringWithReceipt(
            submit = { submissions++ }, lookup = { reads++; "owner-a" }, isNotFound = { it is NotFound },
            matches = { it == "owner-a" }, lookupBeforeSubmit = true, receiptProvesSubmission = false,
        )
        assertEquals("owner-a", result)
        assertEquals(2, submissions)
        assertEquals(1, reads)
    }

    @Test fun delayedAcknowledgementStaysWithItsOriginalOwner() = runBlocking {
        val releaseA = CompletableDeferred<Unit>()
        val startedA = CompletableDeferred<Unit>()
        val a = async {
            registerAuthoringWithReceipt(
                submit = { startedA.complete(Unit); releaseA.await() }, lookup = { "owner-a" },
                isNotFound = { it is NotFound }, matches = { it == "owner-a" },
            )
        }
        startedA.await()
        val b = registerAuthoringWithReceipt(
            submit = {}, lookup = { "owner-b" }, isNotFound = { it is NotFound }, matches = { it == "owner-b" },
        )
        assertEquals("owner-b", b)
        assertFalse(a.isCompleted)
        releaseA.complete(Unit)
        yield()
        assertEquals("owner-a", a.await())
    }

    @Test fun coldRecoveryRequiresExactIntentAndFreshNativeSourceProof() = runBlocking {
        val intent = AuthoringIntent(UUID.randomUUID().toString(), UUID.randomUUID().toString(), UUID.randomUUID().toString(), "/home/user/.brand/.apps/example",
            UUID.randomUUID().toString(), UUID.randomUUID().toString(), "app-a", UUID.randomUUID().toString(), "/transcripts/exact.jsonl", SessionMode.Code)
        var exists = false
        var prepared = 0
        var submitted = 0
        suspend fun recover(saved: AuthoringIntent?, acceptProof: Boolean = true) = lookupOrRecoverAuthoringReceipt(
            intent.sourceDirectory, intent.projectId, checkNotNull(intent.sessionId),
            lookup = { if (!exists) throw NotFound(); intent.workspaceId }, isNotFound = { it is NotFound },
            readIntent = { saved }, prepareSource = { candidate ->
                prepared++
                assertEquals(intent.sourceId, candidate.sourceId)
                check(acceptProof)
            }, submitRecovery = { candidate ->
                submitted++
                assertEquals(intent.sessionId, candidate.sessionId)
                exists = true
            }, matches = { receipt, owner -> receipt == owner.workspaceId },
        )
        assertTrue(runCatching { recover(null) }.isFailure)
        assertTrue(runCatching { recover(intent.copy(projectId = UUID.randomUUID().toString())) }.isFailure)
        assertTrue(runCatching { recover(intent, acceptProof = false) }.isFailure)
        assertEquals(0, submitted)
        assertEquals(intent.workspaceId, recover(intent))
        assertEquals(2, prepared)
        assertEquals(1, submitted)
        assertEquals(intent.workspaceId, recover(intent))
        assertEquals(2, prepared)
        assertEquals(1, submitted)
        assertTrue(runCatching { recover(null) }.isFailure)
        assertEquals(1, submitted)
        assertTrue(runCatching { recover(intent.copy(workspaceId = UUID.randomUUID().toString())) }.isFailure)
    }

    @Test fun coldRecoveryDoesNotRegisterBeforeExactNativeResumeAndRechecksAfterSuspension() = runBlocking {
        val session = UUID.randomUUID().toString()
        val intent = AuthoringIntent(UUID.randomUUID().toString(), UUID.randomUUID().toString(), UUID.randomUUID().toString(),
            "/home/user/.brand/.apps/example", UUID.randomUUID().toString(), UUID.randomUUID().toString(), "app",
            session, "/transcripts/a.jsonl", SessionMode.Code)
        val workspace = SessionWorkspaceState(session, intent.sourceDirectory, intent.sourceDirectory, emptyList(), "/transcripts/a.jsonl")
        assertTrue(coldAuthoringResumeMatches(intent, session, SessionMode.Code, true, workspace))
        assertFalse(coldAuthoringResumeMatches(intent, session, SessionMode.Chat, true, workspace))
        assertFalse(coldAuthoringResumeMatches(intent, session, SessionMode.Code, false, workspace))
        assertFalse(coldAuthoringResumeMatches(intent, UUID.randomUUID().toString(), SessionMode.Code, true, workspace))
        assertFalse(coldAuthoringResumeMatches(intent, session, SessionMode.Code, true, workspace.copy(cwd = "/elsewhere")))
        assertFalse(coldAuthoringResumeMatches(intent, session, SessionMode.Code, true, workspace.copy(transcriptPath = "/transcripts/b.jsonl")))

        var current = workspace.copy(transcriptPath = "/transcripts/b.jsonl")
        var registered = 0
        var persisted = 0
        var admitted = 0
        fun matches() = coldAuthoringResumeMatches(intent, session, SessionMode.Code, true, current)
        assertTrue(runCatching { completeColdAuthoringRecovery(::matches, { registered++ }, { persisted++ }, { admitted++; true }) }.isFailure)
        assertEquals(0, registered)
        current = workspace
        val started = CompletableDeferred<Unit>()
        val release = CompletableDeferred<Unit>()
        val attempt = async {
            runCatching { completeColdAuthoringRecovery(::matches, { registered++; started.complete(Unit); release.await() },
                { persisted++ }, { admitted++; true }) }
        }
        started.await()
        current = workspace.copy(cwd = "/changed")
        release.complete(Unit)
        assertTrue(attempt.await().isFailure)
        assertEquals(1, registered)
        assertEquals(0, persisted)
        assertEquals(0, admitted)
    }
}
