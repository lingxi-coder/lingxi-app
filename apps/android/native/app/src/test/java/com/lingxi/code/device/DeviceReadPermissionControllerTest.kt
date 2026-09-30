package com.lingxi.code.device

import com.lingxi.code.bindings.android.DeviceControlFfiException
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.async
import kotlinx.coroutines.cancelAndJoin
import kotlinx.coroutines.test.StandardTestDispatcher
import kotlinx.coroutines.test.TestScope
import kotlinx.coroutines.test.advanceTimeBy
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

@OptIn(ExperimentalCoroutinesApi::class)
class DeviceReadPermissionControllerTest {
    private class Fixture(scope: TestScope) {
        val grants = mutableSetOf<String>()
        val launches = mutableListOf<String>()
        var foreground = true
        var launchFailure = false
        val controller = DeviceReadPermissionController(
            isGranted = { it in grants },
            isForeground = { foreground },
            requestPermission = {
                if (launchFailure) throw IllegalStateException("Activity stopped")
                launches.add(it)
            },
            dispatcher = StandardTestDispatcher(scope.testScheduler),
            timeoutMs = 1_000,
        )
    }

    @Test
    fun alreadyGrantedReadDoesNotRequireForegroundOrPrompt() = runTest {
        val fixture = Fixture(this)
        fixture.grants.add("calendar")
        fixture.foreground = false

        fixture.controller.ensurePermission("calendar", "calendar")

        assertTrue(fixture.launches.isEmpty())
    }

    @Test
    fun ungrantedBackgroundReadFailsWithoutPrompt() = runTest {
        val fixture = Fixture(this)
        fixture.foreground = false

        val error = runCatching { fixture.controller.ensurePermission("contacts", "contacts") }
            .exceptionOrNull()

        assertTrue(error is DeviceControlFfiException.Unavailable)
        assertTrue(fixture.launches.isEmpty())
    }

    @Test
    fun readWaitsForActualPermissionGrant() = runTest {
        val fixture = Fixture(this)
        val read = async { fixture.controller.ensurePermission("calendar", "calendar") }
        runCurrent()
        assertEquals(listOf("calendar"), fixture.launches)
        assertFalse(read.isCompleted)

        fixture.grants.add("calendar")
        fixture.controller.onPermissionResult(true)
        read.await()
    }

    @Test
    fun denialAndGrantRevocationKeepStableRejectedError() = runTest {
        for (reportedGrant in listOf(false, true)) {
            val fixture = Fixture(this)
            val read = async {
                runCatching { fixture.controller.ensurePermission("contacts", "contacts") }
            }
            runCurrent()
            fixture.controller.onPermissionResult(reportedGrant)

            val error = read.await().exceptionOrNull()
            assertTrue(error is DeviceControlFfiException.Rejected)
            assertEquals("contacts permission denied", error?.message)
        }
    }

    @Test
    fun concurrentPermissionRequestCannotReplaceCurrentRequest() = runTest {
        val fixture = Fixture(this)
        val first = async { fixture.controller.ensurePermission("calendar", "calendar") }
        runCurrent()

        val error = runCatching { fixture.controller.ensurePermission("contacts", "contacts") }
            .exceptionOrNull()
        assertTrue(error is DeviceControlFfiException.Other)
        assertEquals(listOf("calendar"), fixture.launches)

        fixture.grants.add("calendar")
        fixture.controller.onPermissionResult(true)
        first.await()
    }

    @Test
    fun cancelledRequestRetainsSlotUntilItsOwnResultArrives() = runTest {
        val fixture = Fixture(this)
        val cancelled = async { fixture.controller.ensurePermission("calendar", "calendar") }
        runCurrent()
        cancelled.cancelAndJoin()

        val blocked = runCatching { fixture.controller.ensurePermission("contacts", "contacts") }
            .exceptionOrNull()
        assertTrue(blocked is DeviceControlFfiException.Other)
        assertEquals(listOf("calendar"), fixture.launches)

        fixture.controller.onPermissionResult(false)
        val next = async { fixture.controller.ensurePermission("contacts", "contacts") }
        runCurrent()
        assertEquals(listOf("calendar", "contacts"), fixture.launches)
        assertFalse(next.isCompleted)
        fixture.grants.add("contacts")
        fixture.controller.onPermissionResult(true)
        next.await()
    }

    @Test
    fun unansweredPromptTimesOutWithoutReusingItsPendingResult() = runTest {
        val fixture = Fixture(this)
        val read = async {
            runCatching { fixture.controller.ensurePermission("calendar", "calendar") }
        }
        runCurrent()
        advanceTimeBy(1_000)
        runCurrent()

        val error = read.await().exceptionOrNull()
        assertTrue(error is DeviceControlFfiException.Other)
        assertEquals("calendar permission request timed out", error?.message)
        val blocked = runCatching { fixture.controller.ensurePermission("contacts", "contacts") }
            .exceptionOrNull()
        assertTrue(blocked is DeviceControlFfiException.Other)
        assertEquals(listOf("calendar"), fixture.launches)
        fixture.controller.onPermissionResult(false)
    }

    @Test
    fun destroyedActivityTerminatesPendingAndFutureRequests() = runTest {
        val fixture = Fixture(this)
        val read = async {
            runCatching { fixture.controller.ensurePermission("calendar", "calendar") }
        }
        runCurrent()
        fixture.controller.detach()
        fixture.controller.onPermissionResult(true)
        assertTrue(read.await().exceptionOrNull() is DeviceControlFfiException.Unavailable)

        fixture.grants.add("contacts")
        val error = runCatching { fixture.controller.ensurePermission("contacts", "contacts") }
            .exceptionOrNull()
        assertTrue(error is DeviceControlFfiException.Unavailable)
    }

    @Test
    fun failedLauncherDoesNotLeavePendingRequest() = runTest {
        val fixture = Fixture(this)
        fixture.launchFailure = true
        val error = runCatching { fixture.controller.ensurePermission("calendar", "calendar") }
            .exceptionOrNull()
        assertTrue(error is DeviceControlFfiException.Unavailable)

        fixture.launchFailure = false
        val retry = async { fixture.controller.ensurePermission("calendar", "calendar") }
        runCurrent()
        assertEquals(listOf("calendar"), fixture.launches)
        fixture.grants.add("calendar")
        fixture.controller.onPermissionResult(true)
        retry.await()
    }
}
