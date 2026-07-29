package com.lingxi.code.shell

import kotlinx.coroutines.async
import kotlinx.coroutines.awaitAll
import kotlinx.coroutines.test.runTest
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class OnDemandBashTest {
    private class FakeClock(var now: Long = 1_000) : EpochClock {
        override fun nowMs(): Long = now
    }

    private class FakeFailureStore(
        var count: Int = 0,
        var last: Long = 0,
    ) : BashFailureStore {
        override fun failureCount(): Int = count
        override fun lastFailureAtMs(): Long = last
        override fun recordFailure(atMs: Long) {
            count++
            last = atMs
        }
        override fun clearFailures() {
            count = 0
            last = 0
        }
    }

    @Test
    fun existingBashSkipsNetworkAndInstall() = runTest {
        var networkCalls = 0
        val commands = mutableListOf<String>()
        val manager = OnDemandBash(
            executor = GuestCommandExecutor { command, _ ->
                commands += command
                0
            },
            networkProbe = BashNetworkProbe {
                networkCalls++
                true
            },
            failures = FakeFailureStore(),
        )

        assertEquals(OnDemandBash.Outcome.Available, manager.ensureBash())
        assertEquals(OnDemandBash.Outcome.Available, manager.ensureBash())
        assertEquals(listOf("command -v bash >/dev/null 2>&1"), commands)
        assertEquals(0, networkCalls)
    }

    @Test
    fun missingBashInstallsThenVerifiesAndClearsFailures() = runTest {
        val commands = mutableListOf<Pair<String, Long>>()
        var probeCount = 0
        val store = FakeFailureStore(count = 2, last = 0)
        val manager = OnDemandBash(
            executor = GuestCommandExecutor { command, timeout ->
                commands += command to timeout
                if (command.startsWith("command -v")) {
                    if (probeCount++ == 0) 127 else 0
                } else {
                    0
                }
            },
            networkProbe = BashNetworkProbe { true },
            failures = store,
        )

        assertEquals(OnDemandBash.Outcome.Available, manager.ensureBash())
        assertEquals(
            listOf(
                "command -v bash >/dev/null 2>&1",
                "apk add bash",
                "command -v bash >/dev/null 2>&1",
            ),
            commands.map { it.first },
        )
        assertEquals(60_000L, commands[1].second)
        assertEquals(0, store.count)
    }

    @Test
    fun offlineFailureIsCachedAndPersistsBackoff() = runTest {
        val clock = FakeClock()
        val store = FakeFailureStore()
        var networkCalls = 0
        val manager = OnDemandBash(
            executor = GuestCommandExecutor { _, _ -> 127 },
            networkProbe = BashNetworkProbe {
                networkCalls++
                false
            },
            failures = store,
            clock = clock,
            config = OnDemandBashConfig(unavailableTtlMs = 100, failureBackoffMs = 1_000),
        )

        val first = manager.ensureBash() as OnDemandBash.Outcome.Unavailable
        val cached = manager.ensureBash() as OnDemandBash.Outcome.Unavailable
        assertEquals(first.reason, cached.reason)
        assertEquals(1, networkCalls)
        assertEquals(1, store.count)

        clock.now += 101
        val backedOff = manager.ensureBash() as OnDemandBash.Outcome.Unavailable
        assertTrue(backedOff.reason.contains("backing off"))
        assertEquals(1, networkCalls)
    }

    @Test
    fun strikeLimitPreventsNetworkOrInstall() = runTest {
        val store = FakeFailureStore(count = 3)
        var networkCalls = 0
        val commands = mutableListOf<String>()
        val manager = OnDemandBash(
            executor = GuestCommandExecutor { command, _ ->
                commands += command
                127
            },
            networkProbe = BashNetworkProbe {
                networkCalls++
                true
            },
            failures = store,
        )

        val outcome = manager.ensureBash() as OnDemandBash.Outcome.Unavailable

        assertTrue(outcome.reason.contains("disabled after 3 failures"))
        assertEquals(0, networkCalls)
        assertEquals(listOf("command -v bash >/dev/null 2>&1"), commands)
    }

    @Test
    fun concurrentCallersShareOneInstallation() = runTest {
        var installed = false
        var installCalls = 0
        val manager = OnDemandBash(
            executor = GuestCommandExecutor { command, _ ->
                when {
                    command == "apk add bash" -> {
                        installCalls++
                        installed = true
                        0
                    }
                    installed -> 0
                    else -> 127
                }
            },
            networkProbe = BashNetworkProbe { true },
            failures = FakeFailureStore(),
        )

        val outcomes = List(8) { async { manager.ensureBash() } }.awaitAll()

        assertTrue(outcomes.all { it == OnDemandBash.Outcome.Available })
        assertEquals(1, installCalls)
    }

    @Test
    fun markDisappearedReprobesCachedAvailability() = runTest {
        var bashExists = true
        var probes = 0
        val manager = OnDemandBash(
            executor = GuestCommandExecutor { command, _ ->
                if (command.startsWith("command -v")) {
                    probes++
                    if (bashExists) 0 else 127
                } else {
                    bashExists = true
                    0
                }
            },
            networkProbe = BashNetworkProbe { true },
            failures = FakeFailureStore(),
        )
        assertEquals(OnDemandBash.Outcome.Available, manager.ensureBash())

        bashExists = false
        manager.markDisappeared()

        assertEquals(OnDemandBash.Outcome.Available, manager.ensureBash())
        assertEquals(3, probes)
    }
}
