package com.lingxi.code.connectivity

import android.content.Context
import android.net.ConnectivityManager
import android.net.Network
import android.net.NetworkCapabilities
import android.net.NetworkRequest
import androidx.compose.runtime.Composable
import androidx.compose.runtime.State
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.remember
import androidx.compose.ui.platform.LocalContext
import kotlinx.coroutines.channels.awaitClose
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.callbackFlow
import kotlinx.coroutines.flow.distinctUntilChanged

/**
 * Observes device connectivity via [ConnectivityManager.NetworkCallback] and
 * exposes it as a cold [Flow] of "is online" booleans.
 *
 * "Online" means at least one network is currently available AND validated
 * (`NET_CAPABILITY_VALIDATED`) — so a captive-portal / no-internet Wi-Fi reads
 * as offline, matching what the user actually experiences. The callback is
 * registered when the flow is collected and unregistered when collection stops
 * ([awaitClose]); the flow re-emits the deduped state so the UI only recomposes
 * on an actual online↔offline transition.
 *
 * This NEVER hard-blocks the conversation — it only drives the dismissible
 * [OfflineBanner]. The pure availability bookkeeping lives in [NetworkPresence]
 * so the "any validated network present" reduction is unit-testable on the JVM.
 */
class ConnectivityObserver(context: Context) {

    private val cm = context.applicationContext
        .getSystemService(Context.CONNECTIVITY_SERVICE) as ConnectivityManager

    /** Cold flow: `true` while a validated network is available, else `false`. */
    val isOnline: Flow<Boolean> = callbackFlow {
        val presence = NetworkPresence<Network>()

        // Seed from the current default network so the first emission reflects
        // the real state at subscription time (not an optimistic "online").
        val current = cm.activeNetwork
        val seedValidated = current != null &&
            cm.getNetworkCapabilities(current).isValidatedInternet()
        if (current != null && seedValidated) presence.onAvailable(current)
        trySend(presence.isOnline)

        val callback = object : ConnectivityManager.NetworkCallback() {
            override fun onAvailable(network: Network) {
                // Treated as a candidate; confirmed once capabilities validate.
            }

            override fun onCapabilitiesChanged(network: Network, caps: NetworkCapabilities) {
                if (caps.isValidatedInternet()) presence.onAvailable(network)
                else presence.onLost(network)
                trySend(presence.isOnline)
            }

            override fun onLost(network: Network) {
                presence.onLost(network)
                trySend(presence.isOnline)
            }
        }

        val request = NetworkRequest.Builder()
            .addCapability(NetworkCapabilities.NET_CAPABILITY_INTERNET)
            .build()
        cm.registerNetworkCallback(request, callback)

        awaitClose { cm.unregisterNetworkCallback(callback) }
    }.distinctUntilChanged()
}

/** True when these capabilities describe a validated, internet-capable network. */
private fun NetworkCapabilities?.isValidatedInternet(): Boolean =
    this != null &&
        hasCapability(NetworkCapabilities.NET_CAPABILITY_INTERNET) &&
        hasCapability(NetworkCapabilities.NET_CAPABILITY_VALIDATED)

/**
 * Pure bookkeeping of which validated networks are currently present. The device
 * is "online" iff at least one network is tracked. Generic over the network key
 * [T] (the Android plumbing uses [Network]) so the add/remove → online reduction
 * is unit-testable without Android — the `NetworkCallback` only feeds it handles.
 */
class NetworkPresence<T> {
    private val available = HashSet<T>()

    val isOnline: Boolean get() = available.isNotEmpty()

    fun onAvailable(network: T) {
        available.add(network)
    }

    fun onLost(network: T) {
        available.remove(network)
    }
}

/**
 * Remember an "is online" [State] driven by a [ConnectivityObserver] scoped to
 * the current [LocalContext]. Defaults to `true` (online) until the first
 * callback emission so the banner never flashes on a cold start.
 */
@Composable
fun rememberOnlineState(): State<Boolean> {
    val context = LocalContext.current
    val observer = remember(context) { ConnectivityObserver(context) }
    return observer.isOnline.collectAsState(initial = true)
}
