package com.lingxi.code.location

import com.lingxi.code.bindings.LocationFfiException
import kotlinx.coroutines.test.runTest
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class AndroidLocationAdapterTest {
    private class FakeLocationClient(
        private val result: Result<DeviceLocationFix>,
    ) : LocationClient {
        override suspend fun currentLocation(): DeviceLocationFix = result.getOrThrow()
    }

    @Test
    fun successMapsEveryField() = runTest {
        val adapter = AndroidLocationAdapter(
            FakeLocationClient(
                Result.success(
                    DeviceLocationFix(
                        latitude = 31.2304,
                        longitude = 121.4737,
                        accuracyM = 12.5,
                        timestampMs = 1_753_000_000_000,
                    ),
                ),
            ),
        )

        val fix = adapter.currentLocation()

        assertEquals(31.2304, fix.latitude, 0.0)
        assertEquals(121.4737, fix.longitude, 0.0)
        assertEquals(12.5, fix.accuracyM ?: error("accuracy"), 0.0)
        assertEquals(1_753_000_000_000uL, fix.timestampMs)
    }

    @Test
    fun failuresMapToStableFfiVariants() = runTest {
        val cases = listOf(
            LocationFailure.PermissionDenied to LocationFfiException.PermissionDenied::class,
            LocationFailure.Unavailable to LocationFfiException.Unavailable::class,
            LocationFailure.Timeout to LocationFfiException.Timeout::class,
            LocationFailure.Other("native failure") to LocationFfiException.Other::class,
        )

        for ((failure, expected) in cases) {
            val adapter = AndroidLocationAdapter(
                FakeLocationClient(Result.failure(LocationException(failure))),
            )
            val error = runCatching { adapter.currentLocation() }.exceptionOrNull()
            assertTrue("$failure must map to ${expected.simpleName}", expected.isInstance(error))
        }
    }

    @Test
    fun arbitraryThrowableMapsToOther() = runTest {
        val adapter = AndroidLocationAdapter(
            FakeLocationClient(Result.failure(IllegalStateException("provider crashed"))),
        )

        val error = runCatching { adapter.currentLocation() }.exceptionOrNull()

        assertTrue(error is LocationFfiException.Other)
        assertEquals("provider crashed", error?.message)
    }
}
