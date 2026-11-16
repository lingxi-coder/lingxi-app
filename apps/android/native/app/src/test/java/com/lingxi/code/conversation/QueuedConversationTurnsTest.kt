package com.lingxi.code.conversation

import android.content.SharedPreferences
import com.lingxi.code.bindings.client.*
import kotlinx.coroutines.*
import kotlinx.coroutines.test.runTest
import org.junit.Assert.*
import org.junit.Test

class QueuedConversationTurnsTest {
    private fun snapshot(id: Long, state: TurnRecoveryStateDto = TurnRecoveryStateDto.RUNNING) =
        TurnRecoverySnapshotDto("session-a", id.toULong(), state, 1uL, 9uL, true, null)
    private fun fact(id: Long, queued: Boolean = false, settled: Boolean = false, active: Boolean = false,
                     owner: Long? = id, state: TurnRecoveryStateDto = TurnRecoveryStateDto.RUNNING) =
        QueuedTurnAdmission(id, true, queued, settled, active, owner?.let { snapshot(it, state) })
    private fun prompt(id: Long) = ClientCommand.SendPrompt("queued-$id".toClientTextDto(), null, emptyList(), id.toULong(), null)

    private inner class Harness(val prefs: SharedPreferences = FakeSharedPreferences()) {
        var session = "session-a"
        val store = DurableConversationTurnClientStore(prefs, "global#chat", initialSessionId = session).apply { begin(session, 41) }
        val facts = linkedMapOf(41L to fact(41, active = true))
        val sent = mutableListOf<Long>()
        var rejected = 0
        var queryFailure: Throwable? = null
        var queries = 0
        val turns = QueuedConversationTurns(store, { session }) { _, ids ->
            queries++
            queryFailure?.let { throw it }
            ids.map { facts[it] ?: QueuedTurnAdmission(it, false, false, false, false, null) }
        }
        suspend fun send(id: Long, block: suspend () -> Unit = { facts[id] = fact(id, queued = true, owner = null) }) {
            turns.submit(prompt(id), 41, { true }, { rejected++ }) {
                sent += id
                assertEquals(41L, store.load()?.turnId)
                assertTrue(id in store.state(session).queuedTurnIds)
                block()
            }
        }
        fun endA(state: TurnRecoveryStateDto = TurnRecoveryStateDto.COMPLETED) { facts[41] = fact(41, settled = true, state = state) }
    }

    @Test fun queuedIdsAreOrderedDeduplicatedAndSurviveActiveTerminalAndColdStore() = runTest {
        val h = Harness()
        h.send(42); h.send(43)
        h.store.enqueue(h.session, 41, 42)
        assertEquals(listOf(42L, 43L), h.store.state(h.session).queuedTurnIds)
        h.store.clear(h.session, 41)
        val cold = DurableConversationTurnClientStore(h.prefs, "global#chat", initialSessionId = h.session)
        assertNull(cold.load())
        assertEquals(listOf(42L, 43L), cold.state(h.session).queuedTurnIds)
        assertEquals(listOf(42L, 43L), h.sent)
    }

    @Test fun midTurnAppendAckRetiresOnlyItsMemberWithoutReplacingA() = runTest {
        val h = Harness(); h.send(42); h.send(43)
        h.facts[42] = fact(42, settled = true, owner = null)
        val restored = h.turns.recover(h.session)
        assertEquals(41L, restored.record?.turnId)
        assertEquals(listOf(43L), h.store.state(h.session).queuedTurnIds)
        assertEquals(0, h.rejected)
    }

    @Test fun independentExecutorAfterCompletedOrCancelledAIsPersistedBeforeItsRunningEvent() = runTest {
        for (terminal in listOf(TurnRecoveryStateDto.COMPLETED, TurnRecoveryStateDto.CANCELLED)) {
            val h = Harness(); h.send(42); h.endA(terminal)
            h.facts[42] = fact(42, active = true)
            val running = ClientEvent.TurnRecoveryState(snapshot(42))
            h.turns.noteIncoming(running)
            assertTrue(h.turns.beforeEvent(running))
            assertEquals(42L, h.store.load()?.turnId)
            assertTrue(h.store.load()!!.queuedOwner)
            assertEquals(42L, h.turns.executorFor(running))
            assertEquals(listOf(42L), h.sent)
        }
    }

    @Test fun coldRestoreAdoptsTheSameClaimedOrLiveOwnerWithoutSendingAnotherPrompt() = runTest {
        for (live in listOf(false, true)) {
            val h = Harness(); h.send(42); h.endA()
            h.facts[42] = fact(42, active = live)
            val cold = DurableConversationTurnClientStore(h.prefs, "global#chat", initialSessionId = h.session)
            val restored = QueuedConversationTurns(cold, { h.session }) { _, ids -> ids.map { h.facts.getValue(it) } }.recover(h.session)
            assertEquals(42L, restored.record?.turnId)
            assertEquals(live, restored.executorActive)
            assertEquals(listOf(42L), h.sent)
        }
    }

    @Test fun privateColdPendingInputRepairsItsMarkerRatherThanInventingAnActiveOwner() = runTest {
        val h = Harness(); h.send(42); h.endA()
        val result = h.turns.recover(h.session)
        assertNull(result.record)
        assertEquals(42L, result.repairPendingId)
        assertEquals(listOf(42L), h.store.state(h.session).queuedTurnIds)
        assertEquals(listOf(42L), h.sent)
    }

    @Test fun coalescedMemberProvesActualOwnerAndTerminalKeepsLaterUnrelatedInput() = runTest {
        val h = Harness(); h.send(43); h.send(44); h.endA()
        h.facts[43] = fact(43, active = true, owner = 42)
        val plan = h.turns.recover(h.session)
        assertEquals(42L, plan.record?.turnId)
        assertEquals(listOf(43L, 44L), h.store.state(h.session).queuedTurnIds)
        h.facts[42] = fact(42, settled = true, state = TurnRecoveryStateDto.CANCELLED)
        h.facts[43] = fact(43, settled = true, owner = 42, state = TurnRecoveryStateDto.CANCELLED)
        assertTrue(h.turns.beforeEvent(ClientEvent.TurnRecoveryState(snapshot(42, TurnRecoveryStateDto.CANCELLED))))
        assertNull(h.store.load())
        assertEquals(listOf(44L), h.store.state(h.session).queuedTurnIds)
    }

    @Test fun staleMemberTerminalCannotRetireItsRunningExecutor() = runTest {
        val h = Harness(); h.send(43); h.endA(); h.facts[43] = fact(43, active = true, owner = 42)
        h.turns.recover(h.session)
        h.facts[42] = fact(42, active = true)
        h.facts[43] = fact(43, settled = true, owner = null)
        h.turns.beforeEvent(ClientEvent.TurnRecoveryState(snapshot(43, TurnRecoveryStateDto.CANCELLED)))
        assertEquals(42L, h.store.load()?.turnId)
    }

    @Test fun pausedPredecessorRemainsAuthoritativeUntilNativeProvesItSettled() = runTest {
        val h = Harness(); h.send(42)
        h.facts[41] = fact(41, state = TurnRecoveryStateDto.PAUSED_RECOVERABLE)
        h.facts[42] = fact(42, active = true)
        val running = ClientEvent.TurnRecoveryState(snapshot(42))
        assertFalse(h.turns.beforeEvent(running))
        assertEquals(41L, h.store.load()?.turnId)
        assertNull(h.turns.executorFor(running))
    }

    @Test fun definitiveEarlyAndLateRejectionRestoresOnlyTheUnadmittedId() = runTest {
        for (late in listOf(false, true)) {
            val h = Harness()
            runCatching { h.send(42) {
                if (late) { h.endA(); h.store.clear(h.session, 41) }
                throw ClientException.Rejected("turn_cancelled_before_admission")
            } }
            assertEquals(1, h.rejected)
            assertTrue(h.store.state(h.session).queuedTurnIds.isEmpty())
            assertEquals(if (late) null else 41L, h.store.load()?.turnId)
        }
    }

    @Test fun ambiguousFailureKeepsNativeAdmissionAndMissingProofRestoresExactlyOnce() = runTest {
        val h = Harness()
        runCatching { h.send(42) { error("unknown transport receipt") } }
        assertEquals(listOf(42L), h.store.state(h.session).queuedTurnIds)
        assertEquals(0, h.rejected)
        val missing = h.turns.recover(h.session)
        assertEquals(listOf(42L), missing.rejectedIds)
        h.turns.recover(h.session)
        assertEquals(1, h.rejected)
        assertEquals(listOf(42L), h.sent)
        assertTrue(h.turns.isRejection(checkNotNull(h.turns.rejectionNotice(missing))))
    }

    @Test fun ambiguousButAdmittedFailureNeverResendsOrRestoresThePrompt() = runTest {
        val h = Harness()
        runCatching { h.send(42) { h.facts[42] = fact(42, queued = true, owner = null); error("unknown publication") } }
        h.turns.recover(h.session)
        assertEquals(listOf(42L), h.store.state(h.session).queuedTurnIds)
        assertEquals(0, h.rejected)
        assertEquals(listOf(42L), h.sent)
    }

    @Test fun inFlightMissingQueryCannotRetireAnAdmissionBeforeItsNativeReceipt() = runTest {
        val h = Harness(); val entered = CompletableDeferred<Unit>(); val release = CompletableDeferred<Unit>()
        val job = launch { runCatching { h.send(42) { entered.complete(Unit); release.await(); error("uncertain") } } }
        entered.await()
        h.turns.recover(h.session)
        assertEquals(listOf(42L), h.store.state(h.session).queuedTurnIds)
        assertEquals(0, h.rejected)
        release.complete(Unit); job.join()
        h.turns.recover(h.session)
        assertEquals(1, h.rejected)
    }

    @Test fun cancellingTheForeignFutureReleasesItsReceiptFenceForContinue() = runTest {
        val h = Harness(); val entered = CompletableDeferred<Unit>()
        val job = launch { h.send(42) { entered.complete(Unit); awaitCancellation() } }
        entered.await(); job.cancelAndJoin()
        assertEquals(0, h.rejected)
        h.turns.recover(h.session)
        assertEquals(1, h.rejected)
        assertTrue(h.store.state(h.session).queuedTurnIds.isEmpty())
    }

    @Test fun failedLocalPublicationNeverCallsNativeAndRestoresItsInput() = runTest {
        val delegate = FakeSharedPreferences()
        var fail = false
        val prefs = object : SharedPreferences by delegate {
            override fun edit(): SharedPreferences.Editor {
                val editor = delegate.edit()
                return object : SharedPreferences.Editor by editor {
                    override fun putString(key: String?, value: String?): SharedPreferences.Editor { editor.putString(key, value); return this }
                    override fun commit(): Boolean = editor.commit().let { if (fail) { fail = false; false } else it }
                }
            }
        }
        val h = Harness(prefs); fail = true
        assertTrue(runCatching { h.send(42) }.isFailure)
        assertTrue(h.sent.isEmpty())
        assertEquals(1, h.rejected)
        assertTrue(h.store.state(h.session).queuedTurnIds.isEmpty())
        assertEquals(41L, h.store.load()?.turnId)
    }

    @Test fun attachRunningIsNotProofOfALiveExecutorButResumeRunningIs() = runTest {
        val h = Harness(); h.send(42); h.endA(); h.facts[42] = fact(42)
        h.turns.recover(h.session)
        h.turns.beginAttach(42)
        val attaching = ClientEvent.TurnRecoveryState(snapshot(42)); h.turns.noteIncoming(attaching)
        h.turns.endAttach(42)
        h.turns.beforeEvent(attaching)
        assertNull(h.turns.executorFor(attaching))
        assertEquals(42L, h.turns.ownerFor(attaching))
        h.facts[42] = fact(42, active = true)
        val resumed = ClientEvent.TurnRecoveryState(snapshot(42)); h.turns.noteIncoming(resumed)
        h.turns.beforeEvent(resumed)
        assertEquals(42L, h.turns.executorFor(resumed))
    }

    @Test fun bufferedRunningReceiptSurvivesTerminalRetirementAndNoCacheQueriesOccurWithoutQueuedWork() = runTest {
        val h = Harness()
        h.turns.beforeEvent(ClientEvent.TextDelta("ordinary".toClientTextDto()))
        assertEquals(0, h.queries)
        h.send(42); h.endA(); h.facts[42] = fact(42, active = true)
        val running = ClientEvent.TurnRecoveryState(snapshot(42)); h.turns.beforeEvent(running)
        h.facts[42] = fact(42, settled = true, state = TurnRecoveryStateDto.COMPLETED)
        h.turns.beforeEvent(ClientEvent.TurnRecoveryState(snapshot(42, TurnRecoveryStateDto.COMPLETED)))
        assertNull(h.store.load())
        assertEquals(42L, h.turns.executorFor(running))
        assertFalse(h.turns.blocksNewTurn())
    }

    @Test fun startedRetriesTransientRunningAdmissionFailureWithoutLosingItsOwner() = runTest {
        val h = Harness(); h.send(42); h.endA(); h.facts[42] = fact(42, active = true)
        h.queryFailure = IllegalStateException("temporary query failure")
        assertTrue(runCatching { h.turns.beforeEvent(ClientEvent.TurnRecoveryState(snapshot(42))) }.isFailure)
        assertFalse(h.turns.beforeEvent(ClientEvent.TextDelta("unproven".toClientTextDto())))
        h.queryFailure = null
        val started = ClientEvent.TurnStarted(42uL)
        assertTrue(h.turns.beforeEvent(started))
        assertEquals(42L, h.turns.executorFor(started))
        assertEquals(42L, h.store.load()?.turnId)
    }

    @Test fun healthyTerminalReleasesFailedAdmissionFenceAndRequestsCompleteDisplayRepair() = runTest {
        val h = Harness(); h.send(42); h.endA(); h.facts[42] = fact(42, active = true)
        h.queryFailure = IllegalStateException("temporary query failure")
        runCatching { h.turns.beforeEvent(ClientEvent.TurnRecoveryState(snapshot(42))) }
        runCatching { h.turns.beforeEvent(ClientEvent.TurnStarted(42uL)) }
        h.queryFailure = null
        h.facts[42] = fact(42, settled = true, state = TurnRecoveryStateDto.COMPLETED)
        val terminal = ClientEvent.TurnRecoveryState(snapshot(42, TurnRecoveryStateDto.COMPLETED))
        assertTrue(h.turns.beforeEvent(terminal))
        assertTrue(h.turns.requiresTranscriptRepair(terminal))
        assertEquals(42L, h.turns.ownerFor(terminal))
        assertFalse(h.turns.blocksNewTurn())
        assertNull(h.store.load())
    }

    @Test fun callbackIdentityKeepsEqualRawEventsInTheirActualOwners() = runTest {
        val h = Harness()
        h.turns.noteIncoming(ClientEvent.TurnRecoveryState(snapshot(41)))
        val a = ClientEvent.TextDelta("same".toClientTextDto()); h.turns.noteIncoming(a)
        h.turns.noteIncoming(ClientEvent.TurnRecoveryState(snapshot(42)))
        val b = ClientEvent.TextDelta("same".toClientTextDto()); h.turns.noteIncoming(b)
        assertEquals(a, b)
        assertEquals(41L, h.turns.rawOwnerFor(a))
        assertEquals(42L, h.turns.rawOwnerFor(b))
    }

    @Test fun retiredMetadataDoesNotInventARecoveryModeButPendingBStillDoes() = runTest {
        val h = Harness(); h.send(42)
        assertEquals(com.lingxi.code.model.SessionMode.Chat,
            DurableConversationTurnClientStore.ownerModeForSession(h.prefs, "global", h.session))
        h.store.clear(h.session, 41)
        assertEquals(com.lingxi.code.model.SessionMode.Chat,
            DurableConversationTurnClientStore.ownerModeForSession(h.prefs, "global", h.session))
        h.store.removeQueued(h.session, 42)
        assertNull(DurableConversationTurnClientStore.ownerModeForSession(h.prefs, "global", h.session))
    }
}
