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
 * the location request before this system permission is touched.
 */
internal class LocationResultHost(
    val owner: Any,
    val hasPermission: () -> Boolean,
    val isEnabled: () -> Boolean,
    val requestPermission: () -> Unit,
    val requestFix: ((Result<DeviceLocationFix>) -> Unit) -> (() -> Unit),
)

/** Pure ownership protocol shared by the Android facade and coroutine tests. */
internal class LocationResultCoordinator(
    private val post: (() -> Unit) -> Unit,
    private val postTimeout: (Runnable, Long) -> Unit,
    private val removeTimeout: (Runnable) -> Unit,
    private val timeoutMs: Long = LOCATION_TIMEOUT_MS,
) : LocationClient {
    private class Pending(val host: LocationResultHost, val continuation: CancellableContinuation<DeviceLocationFix>) {
        var cancelFix: (() -> Unit)? = null
        var timeout: Runnable? = null
    }
    private val lock = Any()
    private var host: LocationResultHost? = null
    private var pending: Pending? = null
    private var permissionResultOwner: Pending? = null

    fun attach(next: LocationResultHost) {
        val retired = synchronized(lock) {
            if (host?.owner === next.owner) return@synchronized null
            val old = pending.takeIf { it?.host?.owner !== next.owner }
            if (old != null) pending = null
            if (permissionResultOwner?.host?.owner !== next.owner) permissionResultOwner = null
            host = next
            old
        }
        retired?.let { completeFailure(it, LocationFailure.Unavailable) }
    }

    fun detach(owner: Any) {
        val retired = synchronized(lock) {
            if (host?.owner === owner) host = null
            if (permissionResultOwner?.host?.owner === owner) permissionResultOwner = null
            pending?.takeIf { it.host.owner === owner }?.also { pending = null }
        }
        retired?.let { completeFailure(it, LocationFailure.Unavailable) }
    }

    override suspend fun currentLocation(): DeviceLocationFix {
        val current = synchronized(lock) { host } ?: throw LocationException(LocationFailure.Unavailable)
        if (!current.isEnabled()) throw LocationException(LocationFailure.Unavailable)
        return suspendCancellableCoroutine { continuation ->
            val request = Pending(current, continuation)
            val accepted = synchronized(lock) {
                if (host !== current || pending != null || permissionResultOwner != null) false
                else { pending = request; true }
            }
            if (!accepted) {
                continuation.resumeWithException(LocationException(LocationFailure.Other("another location request is already in flight")))
                return@suspendCancellableCoroutine
            }
            continuation.invokeOnCancellation { retire(request) }
            post {
                if (!isCurrent(request)) return@post
                val timeout = Runnable { fail(request, LocationFailure.Timeout) }
                synchronized(lock) { request.timeout = timeout }
                postTimeout(timeout, timeoutMs)
                if (!isCurrent(request)) { removeTimeout(timeout); return@post }
                if (current.hasPermission()) requestFix(request)
                else {
                    val ownsPermission = synchronized(lock) {
                        if (pending !== request || !continuation.isActive) false
                        else { permissionResultOwner = request; true }
                    }
                    if (!ownsPermission) return@post
                    runCatching(current.requestPermission).onFailure {
                        synchronized(lock) { if (permissionResultOwner === request) permissionResultOwner = null }
                        fail(request, LocationFailure.Other(it.message ?: "location permission request failed"))
                    }
                }
            }
        }
    }

    fun onPermissionResult(owner: Any, granted: Boolean) {
        val request = synchronized(lock) {
            permissionResultOwner?.takeIf { it.host.owner === owner }?.also { permissionResultOwner = null }
        } ?: return
        if (!isCurrent(request)) return
        if (!granted || !request.host.hasPermission()) { fail(request, LocationFailure.PermissionDenied); return }
        if (!request.host.isEnabled()) { fail(request, LocationFailure.Unavailable); return }
        requestFix(request)
    }

    private fun requestFix(request: Pending) {
        if (!isCurrent(request)) return
        try {
            val cancel = request.host.requestFix { result ->
                result.onSuccess { succeed(request, it) }.onFailure {
                    fail(request, if (it is LocationException) it.failure else LocationFailure.Other(it.message ?: "location request failed"))
                }
            }
            // A provider may call back synchronously, or cancellation may race
            // startup. In either case close this request's newly returned handle.
            val registered = synchronized(lock) {
                if (pending !== request || !request.continuation.isActive) false
                else { request.cancelFix = cancel; true }
            }
            if (!registered) cancel()
        } catch (error: Throwable) {
            fail(request, if (error is LocationException) error.failure else LocationFailure.Other(error.message ?: "location request failed"))
        }
    }

    private fun isCurrent(request: Pending): Boolean = synchronized(lock) {
        pending === request && host === request.host && request.continuation.isActive
    }

    private fun retire(request: Pending): Boolean {
        val owned = synchronized(lock) { if (pending !== request) false else { pending = null; true } }
        if (owned) cleanup(request)
        return owned
    }
    private fun cleanup(request: Pending) {
        val resources = synchronized(lock) {
            (request.timeout to request.cancelFix).also { request.timeout = null; request.cancelFix = null }
        }
        resources.first?.let { runCatching { removeTimeout(it) } }
        resources.second?.let { runCatching(it) }
    }
    private fun succeed(request: Pending, fix: DeviceLocationFix) {
        if (retire(request) && request.continuation.isActive) request.continuation.resume(fix)
    }
    private fun fail(request: Pending, failure: LocationFailure) {
        if (retire(request)) completeFailure(request, failure)
    }
    private fun completeFailure(request: Pending, failure: LocationFailure) {
        cleanup(request)
        if (request.continuation.isActive) request.continuation.resumeWithException(LocationException(failure))
    }
}

object LocationController : LocationClient {
    private class Launchers(val owner: Any, val context: Context, val requestPermission: ActivityResultLauncher<Array<String>>)
    private val mainHandler = Handler(Looper.getMainLooper())
    private val coordinator = LocationResultCoordinator(
        post = { operation -> mainHandler.post { operation() } },
        postTimeout = { callback, delay -> mainHandler.postDelayed(callback, delay) },
        removeTimeout = mainHandler::removeCallbacks,
    )

    fun makeLaunchers(owner: Any, context: Context, requestPermission: ActivityResultLauncher<Array<String>>): Any =
        Launchers(owner, context.applicationContext, requestPermission)

    fun attach(value: Any) {
        val host = value as Launchers
        coordinator.attach(LocationResultHost(host.owner,
            hasPermission = { hasAnyLocationPermission(host.context) },
            isEnabled = { manager(host.context)?.let(LocationManagerCompat::isLocationEnabled) == true },
            requestPermission = { host.requestPermission.launch(arrayOf(Manifest.permission.ACCESS_FINE_LOCATION, Manifest.permission.ACCESS_COARSE_LOCATION)) },
            requestFix = { callback -> requestFix(host.context, callback) },
        ))
    }
    fun detach(owner: Any) = coordinator.detach(owner)
    override suspend fun currentLocation(): DeviceLocationFix = coordinator.currentLocation()
    fun onLocationPermission(owner: Any, result: Map<String, Boolean>) = coordinator.onPermissionResult(owner,
        result[Manifest.permission.ACCESS_FINE_LOCATION] == true || result[Manifest.permission.ACCESS_COARSE_LOCATION] == true)

    private fun requestFix(context: Context, callback: (Result<DeviceLocationFix>) -> Unit): () -> Unit {
        val manager = manager(context) ?: throw LocationException(LocationFailure.Unavailable)
        val provider = chooseProvider(manager, hasPermission(context, Manifest.permission.ACCESS_FINE_LOCATION))
            ?: throw LocationException(LocationFailure.Unavailable)
        val signal = CancellationSignal()
        try {
            LocationManagerCompat.getCurrentLocation(manager, provider, signal, ContextCompat.getMainExecutor(context)) { location ->
                if (location == null) callback(Result.failure(LocationException(LocationFailure.Unavailable)))
                else callback(Result.success(DeviceLocationFix(location.latitude, location.longitude,
                    location.accuracy.takeIf { location.hasAccuracy() && it >= 0f }?.toDouble(), location.time.coerceAtLeast(0L))))
            }
        } catch (_: SecurityException) { throw LocationException(LocationFailure.PermissionDenied) }
        catch (_: IllegalArgumentException) { throw LocationException(LocationFailure.Unavailable) }
        return { signal.cancel() }
    }
    private fun manager(context: Context): LocationManager? = context.getSystemService(Context.LOCATION_SERVICE) as? LocationManager
    private fun chooseProvider(manager: LocationManager, fine: Boolean): String? {
        val criteria = Criteria().apply {
            accuracy = if (fine) Criteria.ACCURACY_FINE else Criteria.ACCURACY_COARSE
            powerRequirement = Criteria.POWER_LOW
            isCostAllowed = false
        }
        return runCatching { manager.getBestProvider(criteria, true) }.getOrNull()
            ?: runCatching { manager.getProviders(true) }.getOrDefault(emptyList()).firstOrNull { it != LocationManager.PASSIVE_PROVIDER }
    }
    private fun hasAnyLocationPermission(context: Context): Boolean = hasPermission(context, Manifest.permission.ACCESS_FINE_LOCATION) || hasPermission(context, Manifest.permission.ACCESS_COARSE_LOCATION)
    private fun hasPermission(context: Context, permission: String): Boolean = ContextCompat.checkSelfPermission(context, permission) == PackageManager.PERMISSION_GRANTED
}
