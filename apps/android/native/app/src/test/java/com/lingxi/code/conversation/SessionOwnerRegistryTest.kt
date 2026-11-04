package com.lingxi.code.conversation

import com.lingxi.code.model.SessionMode
import kotlinx.coroutines.async
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest
import org.junit.Assert.*
import org.junit.Test
import java.util.concurrent.atomic.AtomicLong

class SessionOwnerRegistryTest {
    @OptIn(kotlinx.coroutines.ExperimentalCoroutinesApi::class)
    @Test fun publishedOwnerIndexWriteDoesNotHoldNavigationCommit() = runTest {
        val mutex = Mutex()
        val entered = CompletableDeferred<Unit>()
        val finish = CompletableDeferred<Unit>()
        val first = async {
            withPreparedNavigation(mutex, { true }, acquire = { Unit }, prepare = { it },
                commit = { _, _ -> true }, afterCommit = { _, _ -> entered.complete(Unit); finish.await() }, release = {})
        }
        entered.await()
        val second = async {
            withPreparedNavigation(mutex, { true }, acquire = { Unit }, prepare = { it },
                commit = { _, _ -> true }, release = {})
        }
        runCurrent()
        assertTrue(second.isCompleted)
        assertTrue(second.await())
        finish.complete(Unit)
        assertTrue(first.await())
    }
    @Test fun notificationWaitsForBothOwnerCatalogsBeforeResolvingAPath() {
        assertFalse(canResolveConversationNotification(projectsLoading = true, scheduledLoading = false))
        assertFalse(canResolveConversationNotification(projectsLoading = false, scheduledLoading = true))
        assertFalse(canResolveConversationNotification(projectsLoading = true, scheduledLoading = true))
        assertTrue(canResolveConversationNotification(projectsLoading = false, scheduledLoading = false))
    }
    @OptIn(kotlinx.coroutines.ExperimentalCoroutinesApi::class)
    @Test fun waitingForCronOwnerDoesNotBlockNewerNavigation() = runTest {
        val id = "b2dd7a33-0725-48db-abca-8a2aed345f6b"
        val cron = checkNotNull(SessionAdmissionGate.beginCronReservation(id, "global", SessionMode.Chat, "/guest/source.jsonl"))
        val mutex = Mutex()
        val sequence = AtomicLong(1)
        var selected = "original"
        try {
            val old = async {
                withPreparedNavigation(mutex, { sequence.get() == 1L },
                    acquire = { SessionAdmissionGate.awaitSourcePreparation(id) },
                    prepare = { it },
                    commit = { _, _ -> selected = "old"; true },
                    release = { SessionAdmissionGate.finishSourcePreparation(it) })
            }
            runCurrent()
            assertFalse(old.isCompleted)
            sequence.incrementAndGet()
            val latest = async {
                withPreparedNavigation(mutex, { sequence.get() == 2L },
                    acquire = { Unit }, prepare = { it },
                    commit = { _, _ -> selected = "retained"; true }, release = {})
            }
            runCurrent()
            assertTrue(latest.isCompleted)
            assertTrue(latest.await())
            assertEquals("retained", selected)
            SessionAdmissionGate.finishCronReservation(cron, committed = true)
            assertFalse(old.await())
            assertEquals("retained", selected)
        } finally { SessionAdmissionGate.finishCronReservation(cron, committed = true) }
    }
    @Test fun deferredReceiptRescanWaitsForOtherInFlightCronAdmissions() {
        val first = "c9587a33-0725-48db-abca-8a2aed345f6b"
        val second = "b9587a33-0725-48db-abca-8a2aed345f6b"
        val firstToken = checkNotNull(SessionAdmissionGate.beginCronReservation(first, "scheduled", SessionMode.Chat, null))
        val secondToken = checkNotNull(SessionAdmissionGate.beginCronReservation(second, "scheduled", SessionMode.Chat, null))
        try {
            assertTrue(SessionAdmissionGate.deferReceiptForOtherCron(
                SessionAdmissionGate.ReceiptIdentity(first, "scheduled", SessionMode.Chat, "/guest/source.jsonl"), ownToken = null))
            SessionAdmissionGate.finishCronReservation(firstToken, committed = true)
            assertTrue(SessionAdmissionGate.permitsNewOwner("unrelated"))
            assertTrue(SessionAdmissionGate.permitsNewOwner(second, secondToken))
            SessionAdmissionGate.finishCronReservation(secondToken, committed = true)
            assertTrue(SessionAdmissionGate.needsOwnerScan())
        } finally {
            SessionAdmissionGate.finishCronReservation(firstToken, committed = true)
            SessionAdmissionGate.finishCronReservation(secondToken, committed = true)
            SessionAdmissionGate.completeOwnerScan(SessionAdmissionGate.scanEpoch())
        }
    }

    @Test fun conflictingReceiptAgainstPendingCandidateFreezesNewClaims() {
        val id = "a9587a33-0725-48db-abca-8a2aed345f6b"
        val token = checkNotNull(SessionAdmissionGate.beginCronReservation(id, "global", SessionMode.Chat, "/guest/exact.jsonl"))
        try {
            assertTrue(runCatching {
                SessionAdmissionGate.deferReceiptForOtherCron(
                    SessionAdmissionGate.ReceiptIdentity(id, "global", SessionMode.Chat, "/guest/other.jsonl"), ownToken = null)
            }.isFailure)
            assertFalse(SessionAdmissionGate.permitsNewOwner("unrelated"))
        } finally {
            SessionAdmissionGate.finishCronReservation(token, committed = true)
            SessionAdmissionGate.completeOwnerScan(SessionAdmissionGate.scanEpoch())
        }
    }

    @Test fun onlyTheOwningCronCallbackMayMirrorItsPendingUuid() {
        val id = "d9587a33-0725-48db-abca-8a2aed345f6b"
        val identity = SessionAdmissionGate.ReceiptIdentity(id, "scheduled", SessionMode.Chat, "/guest/source.jsonl")
        val cron = checkNotNull(SessionAdmissionGate.beginCronReservation(id, "scheduled", SessionMode.Chat, null))
        try {
            assertNull(SessionAdmissionGate.beginReceiptWrite(setOf(identity)))
            assertNull(SessionAdmissionGate.beginReceiptWrite(setOf(identity), cron + 1))
            val mirror = checkNotNull(SessionAdmissionGate.beginReceiptWrite(setOf(identity), cron))
            try {
                assertTrue(SessionAdmissionGate.permitsReceiptWrite(mirror))
                assertTrue(SessionAdmissionGate.permitsNewOwner("unrelated"))
                assertNull(SessionAdmissionGate.beginReceiptWrite(setOf(identity)))
            } finally { SessionAdmissionGate.finishReceiptWrite(mirror) }
        } finally { SessionAdmissionGate.finishCronReservation(cron, committed = true) }
    }

    @Test fun stagedOwnerClaimAllowsItsMatchingLiveOwnerButFencesCompetingUuidUse() {
        val id = "f9587a33-0725-48db-abca-8a2aed345f6b"
        val owner = Any()
        val competing = Any()
        val token = checkNotNull(SessionAdmissionGate.beginOwnerWrite(id, "global", SessionMode.Chat, null))
        try {
            assertTrue(SessionAdmissionGate.mayRegisterRuntimeOwner(id, "global", SessionMode.Chat, owner))
            assertFalse(SessionAdmissionGate.mayRegisterRuntimeOwner(id, "project.other", SessionMode.Code, competing))
            assertTrue(SessionAdmissionGate.mayRegisterRuntimeOwner("unrelated", "global", SessionMode.Chat, competing))
            SessionAdmissionGate.recordRuntimeOwner(id, "global", SessionMode.Chat, owner) { "/guest/exact.jsonl" }
            assertTrue(SessionAdmissionGate.permitsOwnerWrite(token, id, null))
            assertNull(SessionAdmissionGate.beginCronReservation(id, "global", SessionMode.Chat, "/guest/exact.jsonl"))
        } finally {
            SessionAdmissionGate.removeRuntimeOwner(owner)
            SessionAdmissionGate.finishOwnerWrite(token)
        }
    }

    @Test fun ownersKeepTheirStateAndModeAcrossNavigation() {
        data class Owner(var draft: String, var running: Boolean)
        val registry = SessionOwnerRegistry<Owner>()
        val first = Owner("first draft", true)
        val second = Owner("second draft", false)
        assertTrue(registry.register("global", "a", SessionMode.Chat, first))
        assertTrue(registry.register("global", "b", SessionMode.Code, second))
        assertSame(second, registry.find("global", "b", SessionMode.Code))
        assertSame(first, registry.find("global", "a", SessionMode.Chat))
        assertTrue(first.running)
        assertEquals("first draft", first.draft)
        assertNull(registry.find("global", "a", SessionMode.Code))
        assertNull(registry.find("project.one", "a", SessionMode.Chat))
        assertFalse(registry.register("global", "a", SessionMode.Chat, second))
        assertFalse(registry.register("project.one", "a", SessionMode.Chat, second))
        assertTrue(registry.contains("a"))
        assertSame(first, registry.find("global", "a", SessionMode.Chat))
    }

    @Test fun unavailableStorageBlocksNewOwnersWithoutDisplacingAnAdmittedTurn() {
        data class Owner(val draft: String, var running: Boolean)
        val registry = SessionOwnerRegistry<Owner>()
        val active = Owner("unsent draft", true)
        val other = Owner("other", false)
        assertTrue(registry.register("global", "a", SessionMode.Chat, active))
        assertTrue(registry.register("global", "a", SessionMode.Chat, active, allowNew = false))
        assertFalse(registry.register("global", "b", SessionMode.Chat, other, allowNew = false))
        assertFalse(registry.register("project.p", "a", SessionMode.Chat, other, allowNew = false))
        assertFalse(shouldBlockForUnavailableStorage(true, hasRegisteredOwner = true))
        assertTrue(shouldBlockForUnavailableStorage(true, hasRegisteredOwner = false))
        assertSame(active, registry.find("global", "a", SessionMode.Chat))
        assertTrue(active.running)
        assertEquals("unsent draft", active.draft)
    }

    @Test fun ownerStateRevalidationKeepsAnAdmittedConversationVisible() {
        for (validation in listOf(null, false)) {
            assertFalse(shouldBlockForOwnerStateValidation(validation, hasRegisteredOwner = true))
            assertTrue(shouldBlockForOwnerStateValidation(validation, hasRegisteredOwner = false))
        }
        assertFalse(shouldBlockForOwnerStateValidation(true, hasRegisteredOwner = false))
    }

    @Test fun failedCronReservationStaysFencedUntilAContemporaneousStrictScan() {
        val stale = SessionAdmissionGate.scanEpoch()
        val id = "19587a33-0725-48db-abca-8a2aed345f6b"
        val token = checkNotNull(SessionAdmissionGate.beginCronReservation(id, "scheduled", SessionMode.Chat, null))
        try {
            assertFalse(SessionAdmissionGate.permitsNewOwner(id))
            assertTrue(SessionAdmissionGate.permitsNewOwner("other"))
            assertTrue(SessionAdmissionGate.permitsNewOwner(id, token))
            assertFalse(SessionAdmissionGate.permitsNewOwner("other", token))
            SessionAdmissionGate.finishCronReservation(token, committed = false)
            assertFalse(SessionAdmissionGate.permitsNewOwner(id))
            assertFalse(SessionAdmissionGate.permitsNewOwner("other"))
            SessionAdmissionGate.completeOwnerScan(stale)
            assertFalse(SessionAdmissionGate.permitsNewOwner(id))
            SessionAdmissionGate.completeOwnerScan(SessionAdmissionGate.scanEpoch())
            assertTrue(SessionAdmissionGate.permitsNewOwner(id))
        } finally {
            SessionAdmissionGate.finishCronReservation(token, committed = false)
            SessionAdmissionGate.completeOwnerScan(SessionAdmissionGate.scanEpoch())
        }
    }

    @Test fun reservationAndRuntimeClaimsUseTheSameUuidFence() {
        val id = "29587a33-0725-48db-abca-8a2aed345f6b"
        val owner = Any()
        val other = Any()
        synchronized(SessionAdmissionGate.lock) {
            assertTrue(SessionAdmissionGate.mayRegisterRuntimeOwner(id, "global", SessionMode.Chat, owner))
            SessionAdmissionGate.recordRuntimeOwner(id, "global", SessionMode.Chat, owner) { "/exact.jsonl" }
        }
        val token = checkNotNull(SessionAdmissionGate.beginCronReservation(id, "global", SessionMode.Chat, "/exact.jsonl"))
        try {
            assertTrue(SessionAdmissionGate.mayRegisterRuntimeOwner(id, "global", SessionMode.Chat, owner))
            assertFalse(SessionAdmissionGate.mayRegisterRuntimeOwner(id, "project.p", SessionMode.Code, other))
            assertEquals(SessionAdmissionGate.RuntimeIdentity("global", SessionMode.Chat, "/exact.jsonl"), SessionAdmissionGate.runtimeIdentity(id))
            assertTrue(SessionAdmissionGate.mayRegisterRuntimeOwner("other", "global", SessionMode.Chat, other))
            assertNull(SessionAdmissionGate.beginCronReservation(id, "global", SessionMode.Chat, "/exact.jsonl"))
        } finally {
            SessionAdmissionGate.removeRuntimeOwner(owner)
            SessionAdmissionGate.finishCronReservation(token, committed = true)
        }
    }

    @Test fun sourcePreparationAndCronReservationCompeteOnlyForTheSameUuid() {
        val id = "39587a33-0725-48db-abca-8a2aed345f6b"
        val other = "49587a33-0725-48db-abca-8a2aed345f6b"
        val preparation = checkNotNull(SessionAdmissionGate.beginSourcePreparation(id))
        val otherReservation = checkNotNull(SessionAdmissionGate.beginCronReservation(other, "scheduled", SessionMode.Chat, null))
        try {
            assertNull(SessionAdmissionGate.beginCronReservation(id, "scheduled", SessionMode.Chat, null))
            assertNull(SessionAdmissionGate.beginSourcePreparation(other))
            assertTrue(SessionAdmissionGate.permitsNewOwner("unrelated"))
        } finally {
            SessionAdmissionGate.finishSourcePreparation(preparation)
            SessionAdmissionGate.finishCronReservation(otherReservation, committed = true)
        }
        val after = checkNotNull(SessionAdmissionGate.beginCronReservation(id, "scheduled", SessionMode.Chat, null))
        SessionAdmissionGate.finishCronReservation(after, committed = true)
    }

    @Test fun catalogRequiresTheLiveOwnersScopeModeAndExactSource() {
        val id = "59587a33-0725-48db-abca-8a2aed345f6b"
        val owner = Any()
        var path: String? = null
        SessionAdmissionGate.recordRuntimeOwner(id, "global", SessionMode.Chat, owner) { path }
        try {
            assertFalse(SessionAdmissionGate.permitsCatalogWrite(id, "global", SessionMode.Chat, "/exact.jsonl"))
            path = "/exact.jsonl"
            assertTrue(SessionAdmissionGate.permitsCatalogWrite(id, "global", SessionMode.Chat, path))
            assertFalse(SessionAdmissionGate.permitsCatalogWrite(id, "project.p", SessionMode.Chat, path))
            assertFalse(SessionAdmissionGate.permitsCatalogWrite(id, "global", SessionMode.Code, path))
            assertFalse(SessionAdmissionGate.permitsCatalogWrite(id, "global", SessionMode.Chat, "/copy.jsonl"))
            assertFalse(SessionAdmissionGate.permitsCatalogWrite(id, "global", SessionMode.Chat, null))
            assertNotNull(SessionAdmissionGate.conflictingRuntimeIdentity(id, "project.p", SessionMode.Chat, path))
            assertNotNull(SessionAdmissionGate.conflictingRuntimeIdentity(id, "global", SessionMode.Chat, "/copy.jsonl"))
            assertNull(SessionAdmissionGate.conflictingRuntimeIdentity(id, "global", SessionMode.Chat, null))
            assertTrue(SessionAdmissionGate.permitsCatalogWrite("unrelated", "global", SessionMode.Chat, "/other.jsonl"))
        } finally { SessionAdmissionGate.removeRuntimeOwner(owner) }
    }

    @Test fun catalogWaitsForSourcePreparationAndRetriesAfterRelease() {
        val id = "69587a33-0725-48db-abca-8a2aed345f6b"
        val token = checkNotNull(SessionAdmissionGate.beginSourcePreparation(id))
        val revision = SessionAdmissionGate.revision.value
        try {
            assertFalse(SessionAdmissionGate.permitsCatalogWrite(id, "global", SessionMode.Chat, "/exact.jsonl"))
            assertTrue(SessionAdmissionGate.permitsCatalogWrite("unrelated", "global", SessionMode.Chat, "/other.jsonl"))
        } finally { SessionAdmissionGate.finishSourcePreparation(token) }
        assertTrue(SessionAdmissionGate.revision.value > revision)
        assertTrue(SessionAdmissionGate.permitsCatalogWrite(id, "global", SessionMode.Chat, "/exact.jsonl"))
    }
}
