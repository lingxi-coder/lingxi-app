package com.lingxi.code.voice.audio

import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.async
import kotlinx.coroutines.cancelAndJoin
import kotlinx.coroutines.test.StandardTestDispatcher
import kotlinx.coroutines.test.TestScope
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

@OptIn(ExperimentalCoroutinesApi::class)
class AndroidMediaPermissionCoordinatorTest {
    private class Fixture(scope: TestScope) {
        val owner = Any()
        val grants = mutableSetOf<String>()
        val launches = mutableListOf<List<String>>()
        var foreground = true
        var launchFailure = false
        val coordinator = AndroidMediaPermissionCoordinator(
            dispatcher = StandardTestDispatcher(scope.testScheduler),
            timeoutMs = 1_000,
        )

        init {
            coordinator.attach(
                owner = owner,
                isGranted = { it in grants },
                isForeground = { foreground },
                requestPermissions = { permissions ->
                    if (launchFailure) throw IllegalStateException("Activity stopped")
                    launches += permissions.toList()
                },
            )
        }
    }

    @Test
    fun firstOpenRequestsAllMissingPermissionsAndWaitsForOsGrant() = runTest {
        val fixture = Fixture(this)
        val open = async {
            fixture.coordinator.request(setOf("android.permission.RECORD_AUDIO", "android.permission.CAMERA"))
        }
        runCurrent()

        assertEquals(
            listOf(listOf("android.permission.CAMERA", "android.permission.RECORD_AUDIO")),
            fixture.launches,
        )
        assertFalse(open.isCompleted)

        fixture.grants.addAll(fixture.launches.single())
        fixture.coordinator.onPermissionResult(
            fixture.owner,
            fixture.launches.single().associateWith { true },
        )
        assertEquals(AndroidMediaPermissionOutcome.Granted, open.await())
    }

    @Test
    fun existingGrantsAndBackgroundPromptAvailabilityAreSeparated() = runTest {
        val fixture = Fixture(this)
        fixture.grants.add("android.permission.RECORD_AUDIO")
        fixture.foreground = false

        assertEquals(
            AndroidMediaPermissionOutcome.Granted,
            fixture.coordinator.request(setOf("android.permission.RECORD_AUDIO")),
        )
        assertEquals(
            AndroidMediaPermissionOutcome.Unavailable,
            fixture.coordinator.request(setOf("android.permission.CAMERA")),
        )
        assertTrue(fixture.launches.isEmpty())
    }

    @Test
    fun denialAndGrantRevocationAreReportedAsDenied() = runTest {
        val fixture = Fixture(this)
        val open = async { fixture.coordinator.request(setOf("android.permission.CAMERA")) }
        runCurrent()
        fixture.coordinator.onPermissionResult(fixture.owner, mapOf("android.permission.CAMERA" to true))

        // The ActivityResult boolean alone is not trusted; checkPackageManager
        // is represented by the fixture's grant set and remains false here.
        assertEquals(AndroidMediaPermissionOutcome.Denied, open.await())
    }

    @Test
    fun cancelledPromptRetainsItsSlotUntilItsOwnResultArrives() = runTest {
        val fixture = Fixture(this)
        val cancelled = async { fixture.coordinator.request(setOf("android.permission.CAMERA")) }
        runCurrent()
        cancelled.cancelAndJoin()

        assertEquals(
            AndroidMediaPermissionOutcome.Busy,
            fixture.coordinator.request(setOf("android.permission.RECORD_AUDIO")),
        )

        fixture.grants.add("android.permission.CAMERA")
        fixture.coordinator.onPermissionResult(fixture.owner, mapOf("android.permission.CAMERA" to true))
        assertEquals(
            AndroidMediaPermissionOutcome.Granted,
            fixture.coordinator.request(setOf("android.permission.CAMERA")),
        )
    }

    @Test
    fun activityReplacementAndLauncherFailureFailClosed() = runTest {
        val fixture = Fixture(this)
        val open = async { fixture.coordinator.request(setOf("android.permission.CAMERA")) }
        runCurrent()
        fixture.coordinator.detach(fixture.owner)
        assertEquals(AndroidMediaPermissionOutcome.Unavailable, open.await())

        val replacementOwner = Any()
        val replacementGrants = mutableSetOf<String>()
        var replacementLaunchCount = 0
        fixture.coordinator.attach(
            owner = replacementOwner,
            isGranted = { it in replacementGrants },
            isForeground = { true },
            requestPermissions = { replacementLaunchCount++ },
        )
        val replacementOpen = async {
            fixture.coordinator.request(setOf("android.permission.CAMERA"))
        }
        runCurrent()
        fixture.coordinator.onPermissionResult(
            fixture.owner,
            mapOf("android.permission.CAMERA" to true),
        )
        assertEquals(1, replacementLaunchCount)
        assertFalse(replacementOpen.isCompleted)
        replacementGrants.add("android.permission.CAMERA")
        fixture.coordinator.onPermissionResult(
            replacementOwner,
            mapOf("android.permission.CAMERA" to true),
        )
        assertEquals(AndroidMediaPermissionOutcome.Granted, replacementOpen.await())

        val failed = Fixture(this)
        failed.launchFailure = true
        assertEquals(
            AndroidMediaPermissionOutcome.Unavailable,
            failed.coordinator.request(setOf("android.permission.CAMERA")),
        )
    }
}
