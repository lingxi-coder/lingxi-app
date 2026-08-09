package com.lingxi.code.location

import android.Manifest
import android.content.Context
import android.content.pm.PackageManager
import android.location.Criteria
import android.location.LocationManager
import android.os.CancellationSignal
import android.os.Handler
import android.os.Looper
import androidx.activity.result.ActivityResultLauncher
import androidx.core.content.ContextCompat
import androidx.core.location.LocationManagerCompat
import kotlinx.coroutines.CancellableContinuation
import kotlinx.coroutines.suspendCancellableCoroutine
import kotlin.coroutines.resume
import kotlin.coroutines.resumeWithException

private const val LOCATION_TIMEOUT_MS = 20_000L

data class DeviceLocationFix(
    val latitude: Double,
    val longitude: Double,
    val accuracyM: Double?,
    val timestampMs: Long,
)

sealed interface LocationFailure {
    data object PermissionDenied : LocationFailure
    data object Unavailable : LocationFailure
    data object Timeout : LocationFailure
    data class Other(val message: String) : LocationFailure
}

class LocationException(val failure: LocationFailure) : Exception(
    when (failure) {
        is LocationFailure.Other -> failure.message
        else -> failure::class.simpleName ?: "location error"
    },
)

/** Injectable one-shot location seam used by the generated UniFFI adapter. */
interface LocationClient {
    suspend fun currentLocation(): DeviceLocationFix
}

/**
 * Process-global bridge from the Rust-driven location callback to Android's
 * runtime-permission UI and [LocationManager]. The engine has already approved
 * the local app's declared capability before this system permission is touched.
 */
object LocationController : LocationClient {
    private class Launchers(
        val context: Context,
        val requestPermission: ActivityResultLauncher<Array<String>>,
    )

    private data class Pending(
        val continuation: CancellableContinuation<DeviceLocationFix>,
        var cancellationSignal: CancellationSignal? = null,
    )

    private val stateLock = Any()
    private val mainHandler = Handler(Looper.getMainLooper())

    @Volatile
    private var launchers: Launchers? = null

    private var pending: Pending? = null
    private var awaitingPermission = false

    private val timeout = Runnable {
        failCurrent(LocationFailure.Timeout)
    }

    fun attach(value: Any) {
        launchers = value as Launchers
    }

    fun makeLaunchers(
        context: Context,
        requestPermission: ActivityResultLauncher<Array<String>>,
    ): Any = Launchers(context.applicationContext, requestPermission)

    fun detach() {
        launchers = null
        failCurrent(LocationFailure.Unavailable)
    }

    override suspend fun currentLocation(): DeviceLocationFix {
        val current = launchers ?: throw LocationException(LocationFailure.Unavailable)
        val manager = current.context.getSystemService(Context.LOCATION_SERVICE) as? LocationManager
            ?: throw LocationException(LocationFailure.Unavailable)
        if (!LocationManagerCompat.isLocationEnabled(manager)) {
            throw LocationException(LocationFailure.Unavailable)
        }

        return suspendCancellableCoroutine { continuation ->
            val accepted = synchronized(stateLock) {
                if (pending != null) {
                    false
                } else {
                    pending = Pending(continuation)
                    true
                }
            }
            if (!accepted) {
                continuation.resumeWithException(
                    LocationException(LocationFailure.Other("another location request is already in flight")),
                )
                return@suspendCancellableCoroutine
            }

            continuation.invokeOnCancellation {
                cancelIfCurrent(continuation)
            }
            mainHandler.postDelayed(timeout, LOCATION_TIMEOUT_MS)

            if (hasAnyLocationPermission(current.context)) {
                requestFix(current, manager)
            } else {
                synchronized(stateLock) { awaitingPermission = true }
                mainHandler.post {
                    runCatching {
                        current.requestPermission.launch(
                            arrayOf(
                                Manifest.permission.ACCESS_FINE_LOCATION,
                                Manifest.permission.ACCESS_COARSE_LOCATION,
                            ),
                        )
                    }.onFailure {
                        failCurrent(LocationFailure.Other(it.message ?: "location permission request failed"))
                    }
                }
            }
        }
    }

    /** Activity-result sink registered by [com.lingxi.code.MainActivity]. */
    fun onLocationPermission(result: Map<String, Boolean>) {
        val shouldHandle = synchronized(stateLock) {
            if (!awaitingPermission || pending == null) {
                false
            } else {
                awaitingPermission = false
                true
            }
        }
        if (!shouldHandle) return

        val current = launchers
        if (current == null) {
            failCurrent(LocationFailure.Unavailable)
            return
        }
        val granted = result[Manifest.permission.ACCESS_FINE_LOCATION] == true ||
            result[Manifest.permission.ACCESS_COARSE_LOCATION] == true ||
            hasAnyLocationPermission(current.context)
        if (!granted) {
            failCurrent(LocationFailure.PermissionDenied)
            return
        }
        val manager = current.context.getSystemService(Context.LOCATION_SERVICE) as? LocationManager
        if (manager == null || !LocationManagerCompat.isLocationEnabled(manager)) {
            failCurrent(LocationFailure.Unavailable)
            return
        }
        requestFix(current, manager)
    }

    private fun requestFix(current: Launchers, manager: LocationManager) {
        val fine = hasPermission(current.context, Manifest.permission.ACCESS_FINE_LOCATION)
        val provider = chooseProvider(manager, fine)
        if (provider == null) {
            failCurrent(LocationFailure.Unavailable)
            return
        }
        val signal = CancellationSignal()
        val registered = synchronized(stateLock) {
            val active = pending
            if (active == null) {
                false
            } else {
                active.cancellationSignal = signal
                true
            }
        }
        if (!registered) return

        try {
            LocationManagerCompat.getCurrentLocation(
                manager,
                provider,
                signal,
                ContextCompat.getMainExecutor(current.context),
            ) { location ->
                if (location == null) {
                    failCurrent(LocationFailure.Unavailable)
                } else {
                    succeedCurrent(
                        DeviceLocationFix(
                            latitude = location.latitude,
                            longitude = location.longitude,
                            accuracyM = location.accuracy
                                .takeIf { location.hasAccuracy() && it >= 0f }
                                ?.toDouble(),
                            timestampMs = location.time.coerceAtLeast(0L),
                        ),
                    )
                }
            }
        } catch (_: SecurityException) {
            failCurrent(LocationFailure.PermissionDenied)
        } catch (_: IllegalArgumentException) {
            failCurrent(LocationFailure.Unavailable)
        } catch (error: Throwable) {
            failCurrent(LocationFailure.Other(error.message ?: "location request failed"))
        }
    }

    private fun chooseProvider(manager: LocationManager, fine: Boolean): String? {
        val criteria = Criteria().apply {
            accuracy = if (fine) Criteria.ACCURACY_FINE else Criteria.ACCURACY_COARSE
            powerRequirement = Criteria.POWER_LOW
            isCostAllowed = false
        }
        return runCatching { manager.getBestProvider(criteria, true) }.getOrNull()
            ?: runCatching { manager.getProviders(true) }.getOrDefault(emptyList())
                .firstOrNull { it != LocationManager.PASSIVE_PROVIDER }
    }

    private fun hasAnyLocationPermission(context: Context): Boolean =
        hasPermission(context, Manifest.permission.ACCESS_FINE_LOCATION) ||
            hasPermission(context, Manifest.permission.ACCESS_COARSE_LOCATION)

    private fun hasPermission(context: Context, permission: String): Boolean =
        ContextCompat.checkSelfPermission(context, permission) == PackageManager.PERMISSION_GRANTED

    private fun cancelIfCurrent(continuation: CancellableContinuation<DeviceLocationFix>) {
        val active = synchronized(stateLock) {
            if (pending?.continuation !== continuation) return
            val value = pending
            pending = null
            awaitingPermission = false
            value
        }
        mainHandler.removeCallbacks(timeout)
        active?.cancellationSignal?.cancel()
    }

    private fun succeedCurrent(fix: DeviceLocationFix) {
        val active = takePending() ?: return
        if (active.continuation.isActive) active.continuation.resume(fix)
    }

    private fun failCurrent(failure: LocationFailure) {
        val active = takePending() ?: return
        if (active.continuation.isActive) {
            active.continuation.resumeWithException(LocationException(failure))
        }
    }

    private fun takePending(): Pending? {
        val active = synchronized(stateLock) {
            val value = pending
            pending = null
            awaitingPermission = false
            value
        }
        mainHandler.removeCallbacks(timeout)
        active?.cancellationSignal?.cancel()
        return active
    }
}
