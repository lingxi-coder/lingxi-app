package com.lingxi.code.cron

import java.io.File
import java.util.UUID
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Test

class CronRunHistoryPersistenceTest {
    @Test fun claimRevalidatesWhenAConflictingReceiptCommitsDuringItsFsync() {
        val files = File(root, "app").apply { mkdirs() }
        val ownerFile = File(files, com.lingxi.code.project.ScopeStateStore.SCOPE_STATE_FILE)
        val history = CronRunHistoryStore(File(files, "cron/history"))
        val owners = com.lingxi.code.project.ScopeStateStore(ownerFile)
        val existing = "19587a33-0725-48db-abca-8a2aed345f6b"
        val claimant = "29587a33-0725-48db-abca-8a2aed345f6b"
        assertTrue(kotlinx.coroutines.runBlocking { owners.claimSessionOwner(existing, "global", com.lingxi.code.model.SessionMode.Chat) })
        val scope = CronScope("global", null, "Global", root.path, "/home/lingxi/scheduled")
        val run = checkNotNull(history.enqueue(scope, "task", "prompt", 900L))
        val entered = java.util.concurrent.CountDownLatch(1)
        val release = java.util.concurrent.CountDownLatch(1)
        val slow = java.util.concurrent.atomic.AtomicBoolean(true)
        val writer = object : com.lingxi.code.project.ProjectAtomicWriter {
            override fun write(target: File, text: String, validate: (String) -> Unit) =
                com.lingxi.code.project.DefaultProjectAtomicWriter().write(target, text, validate)
            override fun writeGuarded(target: File, text: String, validate: (String) -> Unit, publish: (() -> Unit) -> Unit) {
                com.lingxi.code.project.DefaultProjectAtomicWriter().writeGuarded(target, text, { staged ->
                    validate(staged)
                    if (slow.compareAndSet(true, false)) {
                        entered.countDown()
                        check(release.await(10, java.util.concurrent.TimeUnit.SECONDS))
                    }
                }, publish)
            }
        }
        val delayed = com.lingxi.code.project.ScopeStateStore(ownerFile, writer)
        val executor = java.util.concurrent.Executors.newSingleThreadExecutor()
        val claiming = executor.submit<Boolean> { kotlinx.coroutines.runBlocking {
            delayed.claimSessionOwner(claimant, "global", com.lingxi.code.model.SessionMode.Chat)
        } }
        try {
            assertTrue(entered.await(5, java.util.concurrent.TimeUnit.SECONDS))
            assertNotNull(history.attachExecution(run.runId, sessionId = existing, sessionTranscriptPath = "/guest/conflict.jsonl",
                sessionMode = "chat", sessionScopeKey = "scheduled"))
            release.countDown()
            assertTrue("Claim must recheck receipts published while it staged", runCatching {
                claiming.get(5, java.util.concurrent.TimeUnit.SECONDS)
            }.isFailure)
            val saved = kotlinx.coroutines.runBlocking { owners.readSessionOwners() }
            assertTrue(existing in saved)
            assertTrue(claimant !in saved)
        } finally {
            release.countDown()
            executor.shutdownNow()
        }
    }

    @Test fun unrelatedDurableOwnerCanBeClaimedWhileReceiptFsyncIsStalled() {
        val files = File(root, "app").apply { mkdirs() }
        val historyRoot = File(files, "cron/history")
        val history = CronRunHistoryStore(historyRoot)
        val scope = CronScope("global", null, "Global", root.path, "/home/lingxi/scheduled")
        val run = checkNotNull(history.enqueue(scope, "task", "prompt", 900L))
        val entered = java.util.concurrent.CountDownLatch(1)
        val release = java.util.concurrent.CountDownLatch(1)
        val slow = java.util.concurrent.atomic.AtomicBoolean(true)
        val writer = object : com.lingxi.code.project.ProjectAtomicWriter {
            override fun write(target: File, text: String, validate: (String) -> Unit) =
                com.lingxi.code.project.DefaultProjectAtomicWriter().write(target, text, validate)
            override fun writeGuarded(target: File, text: String, validate: (String) -> Unit, publish: (() -> Unit) -> Unit) {
                com.lingxi.code.project.DefaultProjectAtomicWriter().writeGuarded(target, text, { staged ->
                    validate(staged)
                    if (target.name == "index.json" && slow.compareAndSet(true, false)) {
                        entered.countDown()
                        check(release.await(10, java.util.concurrent.TimeUnit.SECONDS))
                    }
                }, publish)
            }
        }
        val guarded = CronRunHistoryStore(historyRoot, atomicWriter = writer)
        val owners = com.lingxi.code.project.ScopeStateStore(File(files, com.lingxi.code.project.ScopeStateStore.SCOPE_STATE_FILE))
        val executor = java.util.concurrent.Executors.newFixedThreadPool(3)
        val id = "19587a33-0725-48db-abca-8a2aed345f6b"
        val other = "29587a33-0725-48db-abca-8a2aed345f6b"
        val writing = executor.submit<CronRunRecord?> {
            guarded.attachExecution(run.runId, sessionId = id, sessionTranscriptPath = "/guest/one.jsonl",
                sessionMode = "chat", sessionScopeKey = "scheduled")
        }
        try {
            assertTrue(entered.await(5, java.util.concurrent.TimeUnit.SECONDS))
            val unrelated = executor.submit<Boolean> { kotlinx.coroutines.runBlocking {
                owners.claimSessionOwner(other, "global", com.lingxi.code.model.SessionMode.Chat)
            } }
            val admitted = runCatching { unrelated.get(2, java.util.concurrent.TimeUnit.SECONDS) }.getOrDefault(false)
            release.countDown()
            assertNotNull(writing.get(5, java.util.concurrent.TimeUnit.SECONDS))
            assertTrue("Another UUID waited for an unrelated receipt fsync", admitted)
        } finally {
            release.countDown()
            executor.shutdownNow()
        }
    }

    @Test fun receiptStagingDoesNotBlockNavigationAndFencesOnlyItsUuid() {
        val scope = CronScope("global", null, "Global", root.path, "/home/lingxi/scheduled")
        val run = checkNotNull(store.enqueue(scope, "task", "prompt", 900L))
        val entered = java.util.concurrent.CountDownLatch(1)
        val release = java.util.concurrent.CountDownLatch(1)
        val slow = java.util.concurrent.atomic.AtomicBoolean(true)
        val writer = object : com.lingxi.code.project.ProjectAtomicWriter {
            override fun write(target: File, text: String, validate: (String) -> Unit) =
                com.lingxi.code.project.DefaultProjectAtomicWriter().write(target, text, validate)
            override fun writeGuarded(target: File, text: String, validate: (String) -> Unit, publish: (() -> Unit) -> Unit) {
                com.lingxi.code.project.DefaultProjectAtomicWriter().writeGuarded(target, text, { staged ->
                    validate(staged)
                    if (target.name == "index.json" && slow.compareAndSet(true, false)) {
                        entered.countDown()
                        check(release.await(10, java.util.concurrent.TimeUnit.SECONDS))
                    }
                }, publish)
            }
        }
        val guarded = CronRunHistoryStore(root, atomicWriter = writer)
        val executor = java.util.concurrent.Executors.newFixedThreadPool(2)
        val id = "19587a33-0725-48db-abca-8a2aed345f6b"
        val other = "29587a33-0725-48db-abca-8a2aed345f6b"
        val gate = com.lingxi.code.conversation.SessionAdmissionGate
        val writing = executor.submit<CronRunRecord?> {
            guarded.attachExecution(run.runId, sessionId = id, sessionTranscriptPath = "/guest/one.jsonl",
                sessionMode = "chat", sessionScopeKey = "scheduled")
        }
        try {
            assertTrue(entered.await(5, java.util.concurrent.TimeUnit.SECONDS))
            val navigation = executor.submit<Long> {
                com.lingxi.code.conversation.nextNavigationIntent(java.util.concurrent.atomic.AtomicLong())
            }
            val navigated = runCatching { navigation.get(500, java.util.concurrent.TimeUnit.MILLISECONDS) }.isSuccess
            val same = gate.beginCronReservation(id, "global", com.lingxi.code.model.SessionMode.Chat, "/guest/other.jsonl")
            val unrelated = gate.beginCronReservation(other, "global", com.lingxi.code.model.SessionMode.Chat, "/guest/other.jsonl")
            release.countDown()
            assertNotNull(writing.get(5, java.util.concurrent.TimeUnit.SECONDS))
            unrelated?.let { gate.finishCronReservation(it, committed = true) }
            same?.let { gate.finishCronReservation(it, committed = true) }
            assertTrue("Navigation waited for a scheduled receipt fsync", navigated)
            assertNull(same)
            assertNotNull(unrelated)
        } finally {
            release.countDown()
            executor.shutdownNow()
        }
    }

    @Test fun observedCronIndexDeletedDuringStagingStaysMissing() {
        val scope = CronScope("global", null, "Global", root.path, "/home/lingxi/scheduled")
        val run = checkNotNull(store.enqueue(scope, "task", "prompt", 900L))
        val index = File(root, "index.json")
        val sessions = File(root, "sessions.json")
        val backup = File(root, "index.last-good.json")
        val previousSessions = sessions.readBytes()
        val previousBackup = backup.readBytes()
        val deletingWriter = com.lingxi.code.project.ProjectAtomicWriter { target, text, validate ->
            com.lingxi.code.project.DefaultProjectAtomicWriter().write(target, text) { staged ->
                validate(staged)
                if (target.name == index.name) assertTrue(target.delete())
            }
        }
        val guarded = CronRunHistoryStore(root, atomicWriter = deletingWriter)
        assertTrue(runCatching { guarded.markRunning(run.runId, 1) }.isFailure)
        org.junit.Assert.assertFalse(index.exists())
        assertTrue(previousSessions.contentEquals(sessions.readBytes()))
        assertTrue(previousBackup.contentEquals(backup.readBytes()))
        assertTrue(runCatching { guarded.validatedOwnerReceipts() }.isFailure)
    }

    @Test fun unknownOutcomeSurvivesReloadAndCannotBeRequeuedForTheSameOccurrence() {
        val store = CronRunHistoryStore(root)
        val scope = CronScope("global", null, "None", root.path, "/home/lingxi/lingxi/global")
        val run = checkNotNull(store.enqueue(scope, "task", "prompt", 123L))
        store.markRunning(run.runId, 1)
        store.markTerminal(run.runId, CronRunStatus.OutcomeUnknown, errorMessage = "Result was not confirmed")
        val reloaded = CronRunHistoryStore(root)
        assertEquals(CronRunStatus.OutcomeUnknown, reloaded.record(run.runId)?.status)
        org.junit.Assert.assertNull(reloaded.enqueue(scope, "task", "prompt", 123L))
    }
    private lateinit var root: File
    private lateinit var store: CronRunHistoryStore
    private var clock = 1_000L
    private var sequence = 0

    @Before
    fun setUp() {
        root = java.nio.file.Files.createTempDirectory("cron-history-").toFile()
        store = CronRunHistoryStore(
            historyRoot = root,
            now = { clock++ },
            newId = { "run-${sequence++}" },
        )
    }

    @After
    fun tearDown() {
        root.deleteRecursively()
    }

    @Test
    fun executionReceiptRetainsExactLogicalTranscriptAndOwnerAcrossReload() {
        val scope = CronScope("global", null, "Global", root.path, "/home/lingxi/scheduled")
        val run = store.enqueue(scope, "task", "prompt", 900L)!!
        store.attachExecution(run.runId, sessionId = "19587a33-0725-48db-abca-8a2aed345f6b", sessionTranscriptPath = "/guest/original.jsonl",
            sessionMode = "chat", sessionScopeKey = "global")
        store.markTerminal(run.runId, CronRunStatus.Succeeded)
        val saved = CronRunHistoryStore(root).generatedSessions().single()
        assertEquals("/guest/original.jsonl", saved.sessionTranscriptPath)
        assertEquals("chat", saved.sessionMode)
        assertEquals("global", saved.sessionScopeKey)
    }

    @Test
    fun conflictingSessionReceiptsRemainVisibleForFailClosedAdmission() {
        val scope = CronScope("global", null, "Global", root.path, "/home/lingxi/scheduled")
        val first = store.enqueue(scope, "task", "prompt", 900L)!!
        store.attachExecution(first.runId, sessionId = "19587a33-0725-48db-abca-8a2aed345f6b", sessionTranscriptPath = "/guest/one.jsonl",
            sessionMode = "chat", sessionScopeKey = "scheduled")
        store.markTerminal(first.runId, CronRunStatus.Succeeded)
        val second = store.enqueue(scope, "task", "prompt", 1800L)!!
        store.attachExecution(second.runId, sessionId = "19587a33-0725-48db-abca-8a2aed345f6b", sessionTranscriptPath = "/guest/two.jsonl",
            sessionMode = "chat", sessionScopeKey = "scheduled")
        store.markTerminal(second.runId, CronRunStatus.Succeeded)
        assertEquals(setOf("/guest/one.jsonl", "/guest/two.jsonl"),
            CronRunHistoryStore(root).generatedSessions().map { it.sessionTranscriptPath }.toSet())
    }

    @Test
    fun unfinishedOccurrenceDeduplicatesAndTerminalRunAllowsNextOccurrence() {
        val scope = CronScope("global", null, "全局", root.path, "/home/lingxi/lingxi/global")
        val first = store.enqueue(scope, "task", "prompt", 900L)

        assertNotNull(first)
        assertNull(store.enqueue(scope, "task", "prompt", 900L))

        store.markTerminal(first!!.runId, CronRunStatus.Succeeded, resultText = "ok")
        assertNotNull(store.enqueue(scope, "task", "prompt", 1_800L))
    }

    @Test
    fun atomicIndexRoundTripsAndBoundsResultBytes() {
        val scope = CronScope("global", null, "全局", root.path, "/home/lingxi/lingxi/global")
        val run = store.enqueue(scope, "task", "prompt", 900L)!!
        store.markTerminal(
            run.runId,
            CronRunStatus.Succeeded,
            resultText = "🙂".repeat(MAX_CRON_RESULT_BYTES),
        )

        val reloaded = CronRunHistoryStore(root).record(run.runId)

        assertNotNull(reloaded)
        assertTrue(reloaded!!.resultText!!.toByteArray(Charsets.UTF_8).size <= MAX_CRON_RESULT_BYTES)
        assertEquals(CronRunStatus.Succeeded, reloaded.status)
    }

    @Test
    fun checksumFailurePreservesBothFilesAndRejectsOwnerValidation() {
        val scope = CronScope("global", null, "全局", root.path, "/home/lingxi/lingxi/global")
        val run = store.enqueue(scope, "task", "prompt", 900L)!!
        store.markTerminal(run.runId, CronRunStatus.Succeeded, resultText = "durable")
        File(root, "index.json").writeText(
            """{"version":2,"payload":"{}","sha256":"tampered"}""",
            Charsets.UTF_8,
        )

        val original = File(root, "index.json").readText()
        val backup = File(root, "index.last-good.json").readText()
        assertTrue(runCatching { CronRunHistoryStore(root).validatedOwnerReceipts() }.isFailure)
        assertEquals(original, File(root, "index.json").readText())
        assertEquals(backup, File(root, "index.last-good.json").readText())
        assertTrue(root.listFiles().orEmpty().none { it.name.startsWith("index.corrupt.") })
    }

    @Test
    fun missingOrDamagedSessionIndexRejectsAdmissionWithoutReplacingIt() {
        val scope = CronScope("global", null, "Global", root.path, "/guest/scheduled")
        val run = store.enqueue(scope, "task", "prompt", 900L)!!
        store.attachExecution(run.runId, sessionId = "19587a33-0725-48db-abca-8a2aed345f6b", sessionTranscriptPath = "/guest/a.jsonl",
            sessionMode = "chat", sessionScopeKey = "scheduled")
        val sidecar = File(root, "sessions.json")
        sidecar.writeText("damaged")
        assertTrue(runCatching { CronRunHistoryStore(root).validatedOwnerReceipts() }.isFailure)
        assertEquals("damaged", sidecar.readText())
        sidecar.delete()
        assertTrue(runCatching { CronRunHistoryStore(root).validatedOwnerReceipts() }.isFailure)
        assertTrue(!sidecar.exists())
    }

    @Test
    fun partialReceiptIsRejectedOnWriteAndOnReadWithoutChangingFiles() {
        val scope = CronScope("global", null, "Global", root.path, "/guest/scheduled")
        val run = store.enqueue(scope, "task", "prompt", 900L)!!
        val index = File(root, "index.json")
        val before = index.readText()
        assertTrue(runCatching { store.attachExecution(run.runId, sessionTranscriptPath = "/guest/a.jsonl") }.isFailure)
        assertEquals(before, index.readText())
        store.attachExecution(run.runId, sessionId = "19587a33-0725-48db-abca-8a2aed345f6b",
            sessionTranscriptPath = "/guest/a.jsonl", sessionMode = "chat", sessionScopeKey = "scheduled")
        val complete = index.readText()
        assertTrue(runCatching { store.attachExecution(run.runId, sessionTranscriptPath = "/guest/replacement.jsonl") }.isFailure)
        assertEquals(complete, index.readText())
        val envelope = org.json.JSONObject(index.readText())
        val payload = org.json.JSONObject(envelope.getString("payload"))
        payload.getJSONArray("runs").getJSONObject(0).remove("sessionId")
        val text = payload.toString()
        val hash = java.security.MessageDigest.getInstance("SHA-256").digest(text.toByteArray(Charsets.UTF_8))
            .joinToString("") { "%02x".format(it) }
        index.writeText(envelope.put("payload", text).put("sha256", hash).toString())
        val damaged = index.readText()
        assertTrue(runCatching { store.validatedOwnerReceipts() }.isFailure)
        assertEquals(damaged, index.readText())
    }
    @Test
    fun executionIdentityAndNotificationClaimSurviveReload() {
        val scope = CronScope("global", null, "None", root.path, "/home/lingxi/lingxi/global")
        val run = store.enqueue(scope, "task", "prompt", 900L)!!
        store.attachExecution(run.runId, "19587a33-0725-48db-abca-8a2aed345f6b", "provider/model", "failed",
            sessionTranscriptPath = "/guest/session.jsonl", sessionMode = "chat", sessionScopeKey = "scheduled")
        store.markTerminal(run.runId, CronRunStatus.Failed, errorMessage = "failed")
        assertTrue(store.claimNotification(run.runId))
        val reloaded = CronRunHistoryStore(root)
        org.junit.Assert.assertFalse(reloaded.claimNotification(run.runId))
        assertEquals("19587a33-0725-48db-abca-8a2aed345f6b", reloaded.record(run.runId)?.sessionId)
        assertEquals("provider/model", reloaded.record(run.runId)?.model)
        assertEquals("failed", reloaded.record(run.runId)?.notificationPolicy)
    }

    @Test
    fun restartRecoversSavedTerminalNotificationsOnceUsingPersistedPolicy() {
        val scope = CronScope("global", null, "None", root.path, "/home/lingxi/lingxi/global")
        fun terminal(task: String, policy: String, status: CronRunStatus): String {
            val run = store.enqueue(scope, task, "saved prompt", 900L)!!
            store.attachExecution(run.runId, notificationPolicy = policy)
            store.markTerminal(run.runId, status, resultText = "saved result")
            return run.runId
        }
        val success = terminal("all", "all", CronRunStatus.Succeeded)
        val failure = terminal("failed", "failed", CronRunStatus.Failed)
        terminal("off", "none", CronRunStatus.Failed)
        terminal("success-filtered", "failed", CronRunStatus.Succeeded)
        val queued = store.enqueue(scope, "queued", "prompt", 900L)!!
        val running = store.enqueue(scope, "running", "prompt", 900L)!!
        store.markRunning(running.runId, 1)

        val posted = mutableListOf<CronRunRecord>()
        // No task or Engine is required to deliver the already persisted results.
        CronRunHistoryStore(root).postPendingNotifications(post = posted::add)
        CronRunHistoryStore(root).postPendingNotifications(post = posted::add)

        assertEquals(setOf(success, failure), posted.map { it.runId }.toSet())
        assertEquals(2, posted.size)
        assertTrue(posted.all { it.resultText == "saved result" })
        assertEquals(CronRunStatus.Queued, store.record(queued.runId)?.status)
        assertEquals(CronRunStatus.Running, store.record(running.runId)?.status)
    }

    @Test
    fun failedNotificationClaimWriteRemainsRecoverable() {
        val scope = CronScope("global", null, "None", root.path, "/home/lingxi/lingxi/global")
        val run = store.enqueue(scope, "task", "prompt", 900L)!!
        store.markTerminal(run.runId, CronRunStatus.Succeeded)
        // Block the write the CLAIM depends on, which lives in index.json.
        //
        // Two ways of blocking it that look right do not work. Replacing
        // sessions.json with a directory fails a write `writeLocked` swallows
        // ON PURPOSE — the sidecar is derived, and its failure must not strand
        // a run as Running forever — so the claim still lands and the
        // notification still posts. Replacing index.json with a directory is
        // undone by `readLocked`, which quarantines a primary it cannot read
        // and then writes a fresh one. Making the history root unwritable is
        // what actually fails the claim write while leaving index.json
        // readable, so the record can still be inspected below.
        var posts = 0
        try {
            assertTrue(root.setWritable(false, false))
            assertTrue(runCatching { store.postPendingNotifications { posts++ } }.isFailure)
            assertEquals(0, posts)
            org.junit.Assert.assertFalse(store.record(run.runId)!!.notificationDelivered)
        } finally {
            assertTrue(root.setWritable(true, true))
        }
        CronRunHistoryStore(root).postPendingNotifications { posts++ }
        CronRunHistoryStore(root).postPendingNotifications { posts++ }
        assertEquals(1, posts)
    }

    @Test
    fun workerTerminalRedeliveryAndRecoveryShareDurableClaim() {
        val scope = CronScope("global", null, "None", root.path, "/home/lingxi/lingxi/global")
        val run = store.enqueue(scope, "task", "prompt", 900L)!!
        store.markTerminal(run.runId, CronRunStatus.Succeeded)
        var posts = 0
        store.postPendingNotifications(run.runId) { posts++ }
        CronRunHistoryStore(root).postPendingNotifications { posts++ }
        assertEquals(1, posts)
    }

    @Test
    fun pauseCancelsQueuedButPreservesRunning() {
        val scope = CronScope("global", null, "None", root.path, "/home/lingxi/lingxi/global")
        val queued = store.enqueue(scope, "queued-task", "prompt", 900L)!!
        val running = store.enqueue(scope, "running-task", "prompt", 900L)!!
        store.markRunning(running.runId, 1)
        store.cancelQueued(scope.scopeId, "queued-task")
        store.cancelQueued(scope.scopeId, "running-task")
        assertEquals(CronRunStatus.Cancelled, store.record(queued.runId)?.status)
        assertEquals(CronRunStatus.Running, store.record(running.runId)?.status)
    }

    @Test
    fun automationEditsPreserveUnknownMetadataAndCopyResetsSession() {
        val original = CronAutomation("""{"version":2,"futureField":"keep","status":"completed","runMode":"task_session","ownedSessionId":"old"}""")
        val copy = original.copied().change("model", "provider/model")
        assertEquals("keep", org.json.JSONObject(copy.json).getString("futureField"))
        assertEquals("active", copy.status)
        assertEquals("new_session", copy.runMode)
        org.junit.Assert.assertFalse(org.json.JSONObject(copy.json).has("ownedSessionId"))
    }

    @Test
    fun runningTaskCoalescesOnlyOnePendingOccurrence() {
        val scope = CronScope("global", null, "None", root.path, "/home/lingxi/lingxi/global")
        val running = store.enqueue(scope, "task", "prompt", 900L)!!
        store.markRunning(running.runId, 1)
        assertNotNull(store.enqueue(scope, "task", "prompt", 1_800L))
        assertNull(store.enqueue(scope, "task", "prompt", 2_700L))
        assertEquals(2, store.records().count { !it.status.isTerminal })
    }

    @Test
    fun interruptedOccurrenceCannotBeDispatchedAgain() {
        val scope = CronScope("global", null, "None", root.path, "/home/lingxi/lingxi/global")
        val run = store.enqueue(scope, "task", "prompt", 900L)!!
        store.markTerminal(run.runId, CronRunStatus.Interrupted, errorMessage = "process interrupted")
        assertNull(store.enqueue(scope, "task", "prompt", 900L))
        assertNotNull(store.enqueue(scope, "task", "prompt", 1_800L))
    }

    @Test
    fun generatedSessionIndexOutlivesRunRetention() {
        val scope = CronScope("global", null, "None", root.path, "/home/lingxi/lingxi/global")
        repeat(25) { index ->
            val run = store.enqueue(scope, "task", "prompt", index.toLong())!!
            val id = UUID.nameUUIDFromBytes("session-$index".toByteArray()).toString()
            store.attachExecution(run.runId, sessionId = id, sessionTranscriptPath = "/guest/$id.jsonl",
                sessionMode = "chat", sessionScopeKey = "scheduled")
            store.markTerminal(run.runId, CronRunStatus.Succeeded)
        }
        val reloaded = CronRunHistoryStore(root)
        assertEquals(20, reloaded.records().size)
        assertEquals(25, reloaded.generatedSessions().size)
        assertTrue(reloaded.generatedSessions().any { it.sessionId == UUID.nameUUIDFromBytes("session-0".toByteArray()).toString() })
    }

}
