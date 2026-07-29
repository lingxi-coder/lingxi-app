/*
 * Copyright (C) 2026 OpenMinis contributors
 * Copyright (C) 2026 LingXi contributors
 * SPDX-License-Identifier: GPL-3.0-only
 *
 * Adapted from OpenMinis commit 9cf3a855.
 */
package com.lingxi.code.shell

import android.content.Context
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.withContext
import java.net.HttpURLConnection
import java.net.URL

fun interface EpochClock {
    fun nowMs(): Long
}

fun interface BashNetworkProbe {
    suspend fun isReachable(): Boolean
}

fun interface GuestCommandExecutor {
    suspend fun run(command: String, timeoutMs: Long): Int
}

interface BashFailureStore {
    fun failureCount(): Int
    fun lastFailureAtMs(): Long
    fun recordFailure(atMs: Long)
    fun clearFailures()
}

data class OnDemandBashConfig(
    val maxStrikes: Int = 3,
    val failureBackoffMs: Long = 24L * 60 * 60 * 1_000,
    val unavailableTtlMs: Long = 10L * 60 * 1_000,
    val probeTimeoutMs: Long = 15_000,
    val installTimeoutMs: Long = 60_000,
)

/**
 * Serial, self-healing Bash availability state machine.
 *
 * The host-side network probe runs before `apk add`, failures persist across
 * launches, and a process launch performs at most one failed installation.
 */
class OnDemandBash(
    private val executor: GuestCommandExecutor,
    private val networkProbe: BashNetworkProbe,
    private val failures: BashFailureStore,
    private val clock: EpochClock = EpochClock(System::currentTimeMillis),
    private val config: OnDemandBashConfig = OnDemandBashConfig(),
) {
    sealed interface Outcome {
        data object Available : Outcome
        data class Unavailable(val reason: String) : Outcome
    }

    sealed interface Availability {
        data object Unknown : Availability
        data object Available : Availability
        data class Unavailable(
            val untilMs: Long,
            val reason: String,
        ) : Availability
    }

    private val lock = Mutex()
    private var availability: Availability = Availability.Unknown
    private var attemptedInstallThisLaunch = false

    suspend fun ensureBash(): Outcome = lock.withLock {
        when (val cached = availability) {
            Availability.Available -> return Outcome.Available
            is Availability.Unavailable -> {
                if (clock.nowMs() < cached.untilMs) return Outcome.Unavailable(cached.reason)
                availability = Availability.Unknown
            }
            Availability.Unknown -> Unit
        }

        if (runGuest(PROBE_COMMAND, config.probeTimeoutMs) == 0) {
            availability = Availability.Available
            return Outcome.Available
        }
        backoffReason()?.let { return unavailable(it) }
        if (attemptedInstallThisLaunch) {
            return unavailable("Bash installation was already attempted this app launch")
        }

        attemptedInstallThisLaunch = true
        val reachable = try {
            networkProbe.isReachable()
        } catch (cancelled: CancellationException) {
            throw cancelled
        } catch (_: Throwable) {
            false
        }
        if (!reachable) {
            failures.recordFailure(clock.nowMs())
            return unavailable("network or apk mirror is unreachable")
        }

        val installCode = runGuest(INSTALL_COMMAND, config.installTimeoutMs)
        val verified = installCode == 0 &&
            runGuest(PROBE_COMMAND, config.probeTimeoutMs) == 0
        if (verified) {
            failures.clearFailures()
            availability = Availability.Available
            // Permit one reinstall if Bash is later removed in this launch.
            attemptedInstallThisLaunch = false
            return Outcome.Available
        }

        failures.recordFailure(clock.nowMs())
        return unavailable("apk add bash failed (exit $installCode)")
    }

    suspend fun markDisappeared() = lock.withLock {
        availability = Availability.Unknown
    }

    suspend fun snapshot(): Availability = lock.withLock { availability }

    private suspend fun runGuest(command: String, timeoutMs: Long): Int = try {
        executor.run(command, timeoutMs)
    } catch (cancelled: CancellationException) {
        throw cancelled
    } catch (_: Throwable) {
        EXECUTOR_FAILURE_EXIT
    }

    private fun unavailable(reason: String): Outcome.Unavailable {
        availability = Availability.Unavailable(
            untilMs = clock.nowMs() + config.unavailableTtlMs,
            reason = reason,
        )
        return Outcome.Unavailable(reason)
    }

    private fun backoffReason(): String? {
        val count = failures.failureCount()
        if (count >= config.maxStrikes) {
            return "automatic Bash installation disabled after ${config.maxStrikes} failures; run `apk add bash` manually"
        }
        val lastFailure = failures.lastFailureAtMs()
        if (lastFailure > 0 && clock.nowMs() - lastFailure < config.failureBackoffMs) {
            return "automatic Bash installation is backing off after a recent failure"
        }
        return null
    }

    companion object {
        private const val PROBE_COMMAND = "command -v bash >/dev/null 2>&1"
        private const val INSTALL_COMMAND = "apk add bash"
        private const val EXECUTOR_FAILURE_EXIT = -1
        private const val APK_PROBE_URL = "https://dl-cdn.alpinelinux.org/alpine/"

        fun create(
            context: Context,
            executor: GuestCommandExecutor,
        ): OnDemandBash = OnDemandBash(
            executor = executor,
            networkProbe = HttpBashNetworkProbe(APK_PROBE_URL),
            failures = SharedPreferencesBashFailureStore(context.applicationContext),
        )
    }
}

private class SharedPreferencesBashFailureStore(context: Context) : BashFailureStore {
    private val preferences = context.getSharedPreferences(PREFS, Context.MODE_PRIVATE)

    override fun failureCount(): Int = preferences.getInt(KEY_FAILURE_COUNT, 0)

    override fun lastFailureAtMs(): Long = preferences.getLong(KEY_LAST_FAILURE, 0)

    override fun recordFailure(atMs: Long) {
        preferences.edit()
            .putInt(KEY_FAILURE_COUNT, failureCount() + 1)
            .putLong(KEY_LAST_FAILURE, atMs)
            .apply()
    }

    override fun clearFailures() {
        preferences.edit()
            .remove(KEY_FAILURE_COUNT)
            .remove(KEY_LAST_FAILURE)
            .apply()
    }

    private companion object {
        const val PREFS = "lingxi_bash_install"
        const val KEY_FAILURE_COUNT = "failureCount"
        const val KEY_LAST_FAILURE = "lastFailureAtMs"
    }
}

private class HttpBashNetworkProbe(
    private val endpoint: String,
) : BashNetworkProbe {
    override suspend fun isReachable(): Boolean = withContext(Dispatchers.IO) {
        val connection = try {
            URL(endpoint).openConnection() as HttpURLConnection
        } catch (_: Throwable) {
            return@withContext false
        }
        try {
            connection.requestMethod = "HEAD"
            connection.connectTimeout = 5_000
            connection.readTimeout = 5_000
            connection.responseCode in 200..499
        } catch (_: Throwable) {
            false
        } finally {
            connection.disconnect()
        }
    }
}
