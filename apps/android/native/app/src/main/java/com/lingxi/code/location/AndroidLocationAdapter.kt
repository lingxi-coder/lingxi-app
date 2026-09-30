package com.lingxi.code.location

import com.lingxi.code.bindings.AndroidLocation
import com.lingxi.code.bindings.LocationFfiException
import com.lingxi.code.bindings.LocationFixFfi

/** Maps Android's one-shot location surface onto the generated UniFFI callback. */
class AndroidLocationAdapter(
    private val client: LocationClient = LocationController,
) : AndroidLocation {
    override suspend fun currentLocation(): LocationFixFfi {
        val fix = try {
            client.currentLocation()
        } catch (error: LocationException) {
            throw error.failure.toFfi()
        } catch (error: LocationFfiException) {
            throw error
        } catch (error: Throwable) {
            throw LocationFfiException.Other(error.message ?: "location error")
        }
        return LocationFixFfi(
            latitude = fix.latitude,
            longitude = fix.longitude,
            accuracyM = fix.accuracyM,
            timestampMs = fix.timestampMs.coerceAtLeast(0L).toULong(),
        )
    }
}

private fun LocationFailure.toFfi(): LocationFfiException = when (this) {
    is LocationFailure.PermissionDenied -> LocationFfiException.PermissionDenied()
    is LocationFailure.Unavailable -> LocationFfiException.Unavailable()
    is LocationFailure.Timeout -> LocationFfiException.Timeout()
    is LocationFailure.Other -> LocationFfiException.Other(message)
}
