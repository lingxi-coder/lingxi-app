package com.lingxi.code.cron

import com.lingxi.code.model.SessionMode
import com.lingxi.code.project.ScopeStateStore
import kotlinx.coroutines.test.runTest
import org.junit.Assert.*
import org.junit.Rule
import org.junit.Test
import org.junit.rules.TemporaryFolder
import java.io.File

class CronSdkReceiptsTest {
    @get:Rule val temp = TemporaryFolder()
    private val scope get() = CronScope("global", null, "Global", temp.root.path, "/guest/scheduled")
    private val id = "19587a33-0725-48db-abca-8a2aed345f6b"

    @Test fun `committed SDK receipt is mirrored before another owner can claim its UUID`() = runTest {
        val raw = """{"runs":[{"id":"pending","status":"running"},
            {"id":"bound","scheduledAt":123,"status":"succeeded","sessionId":"$id",
            "sessionTranscriptPath":"/guest/exact.jsonl","sessionMode":"chat","sessionScopeKey":"scheduled"}]}"""
        val receipts = sdkCronReceipts(scope, "task", "prompt", raw)
        assertEquals(1, receipts.size)
        val owners = ScopeStateStore(File(temp.root, "scope-state.json"))
        assertTrue(owners.claimSessionOwner("39587a33-0725-48db-abca-8a2aed345f6b", "global", SessionMode.Chat))
        owners.reconcileCronReceipts(receipts, emptyMap(), emptyMap())
        assertEquals("/guest/exact.jsonl", CronRunHistoryStore(File(temp.root, "cron/history"))
            .validatedOwnerReceipts().single().sessionTranscriptPath)
        assertFalse(owners.claimSessionOwner(id, "global", SessionMode.Chat))
        assertTrue(owners.claimSessionOwner(id, "scheduled", SessionMode.Chat))
    }

    @Test fun `SDK receipt conflicting with an existing owner leaves both indexes unchanged`() = runTest {
        val owners = ScopeStateStore(File(temp.root, "scope-state.json"))
        assertTrue(owners.claimSessionOwner(id, "global", SessionMode.Chat))
        val before = File(temp.root, "scope-state.json").readText()
        val receipt = CronRunRecord("run", "task", "global", null, "Global", "prompt", 1, 1,
            sessionId = id, sessionTranscriptPath = "/guest/scheduled.jsonl", sessionMode = "chat", sessionScopeKey = "scheduled")
        assertTrue(runCatching { owners.reconcileCronReceipts(listOf(receipt),
            mapOf(id to ScopeStateStore.SessionOwner("global", SessionMode.Chat)), mapOf(id to "/guest/global.jsonl")) }.isFailure)
        assertEquals(before, File(temp.root, "scope-state.json").readText())
        assertFalse(File(temp.root, "cron/history/sessions.json").exists())
    }

    @Test fun `malformed or incomplete committed receipts fail closed`() {
        assertTrue(runCatching { sdkCronReceipts(scope, "task", "prompt", """{"runs":"bad"}""") }.isFailure)
        assertTrue(runCatching { sdkCronReceipts(scope, "task", "prompt", """{"runs":[{"id":"run","sessionId":"$id"}]}""") }.isFailure)
        assertTrue(runCatching { sdkCronReceipts(scope, "task", "prompt", """{"runs":[{"id":"run",
            "sessionTranscriptPath":"/guest/a","sessionMode":"chat","sessionScopeKey":"scheduled"}]}""") }.isFailure)
        assertTrue(runCatching { sdkCronReceipts(scope, "task", "prompt", """{"runs":[{"id":"run","sessionId":"not-a-uuid",
            "sessionTranscriptPath":"/guest/a","sessionMode":"chat","sessionScopeKey":"scheduled"}]}""") }.isFailure)
        assertTrue(runCatching { sdkCronReceipts(scope, "task", "prompt", """{"runs":[{"id":"run","sessionId":"$id",
            "sessionTranscriptPath":"relative.jsonl","sessionMode":"chat","sessionScopeKey":"scheduled"}]}""") }.isFailure)
        assertTrue(runCatching { sdkCronReceipts(scope, "task", "prompt", """{"runs":[{"id":"run","sessionId":"$id",
            "sessionTranscriptPath":"/guest/a","sessionMode":"chat","sessionScopeKey":"project.other"}]}""") }.isFailure)
    }
}
