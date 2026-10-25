package com.lingxi.code.remote

import androidx.annotation.StringRes
import com.lingxi.code.R
import com.lingxi.code.bindings.android.RemoteConnectionPhaseFfi
import com.lingxi.code.bindings.android.RemoteConnectionStateFfi
import com.lingxi.code.bindings.android.RemoteFailureFfi
import com.lingxi.code.bindings.android.RemoteFailureKindFfi
import com.lingxi.code.bindings.android.RemoteTransportFfi

/** What the screen of one computer shows for its connection. */
sealed interface RemoteConnectionUi {
    /** Nothing dialled (yet, or stopped). */
    data object Idle : RemoteConnectionUi

    /** Dialling: offer, answer, channel, handshake. */
    data class Connecting(val everReady: Boolean) : RemoteConnectionUi

    /** The user has to allow this device on the computer; [code] is what its dialog shows. */
    data class Pairing(val code: String) : RemoteConnectionUi

    /** Waiting out a backoff, or dialling again, after the link dropped. */
    data class Reconnecting(@StringRes val reason: Int?, val retryAtMs: Long?) : RemoteConnectionUi

    data class Ready(val transport: RemoteTransportFfi?) : RemoteConnectionUi

    /** The client stopped trying. [repair] pairs again with a new key; [forget] drops the pin. */
    data class Failed(
        @StringRes val message: Int,
        val kind: RemoteFailureKindFfi,
        val canRetry: Boolean,
        val repair: Boolean,
        val forget: Boolean,
    ) : RemoteConnectionUi
}

/** Maps the client's connection state to what is drawn; the rules mirror the web viewer. */
fun remoteConnectionUi(state: RemoteConnectionStateFfi?): RemoteConnectionUi {
    if (state == null) return RemoteConnectionUi.Idle
    return when (state.phase) {
        RemoteConnectionPhaseFfi.IDLE -> RemoteConnectionUi.Idle
        RemoteConnectionPhaseFfi.CONNECTING -> RemoteConnectionUi.Connecting(state.everReady)
        RemoteConnectionPhaseFfi.PAIRING ->
            state.pairingCode?.let { RemoteConnectionUi.Pairing(it) } ?: RemoteConnectionUi.Connecting(state.everReady)
        RemoteConnectionPhaseFfi.READY -> RemoteConnectionUi.Ready(state.transport)
        RemoteConnectionPhaseFfi.RECONNECTING ->
            RemoteConnectionUi.Reconnecting(state.failure?.let(::remoteFailureText), state.retryAtMs?.toLong())
        RemoteConnectionPhaseFfi.FAILED -> {
            val failure = state.failure
            val kind = failure?.kind ?: RemoteFailureKindFfi.CLOSED
            RemoteConnectionUi.Failed(
                message = failure?.let(::remoteFailureText) ?: R.string.remote_fail_closed,
                kind = kind,
                canRetry = kind !in NO_RETRY,
                repair = kind == RemoteFailureKindFfi.REVOKED,
                forget = kind == RemoteFailureKindFfi.VERIFICATION || kind == RemoteFailureKindFfi.PAIRING_CODE_MISMATCH,
            )
        }
    }
}

/** Failures a retry cannot fix: an update, a setting, a new pairing, or a refusal to trust. */
private val NO_RETRY = setOf(
    RemoteFailureKindFfi.UNSUPPORTED,
    RemoteFailureKindFfi.UPGRADE_REQUIRED,
    RemoteFailureKindFfi.DISABLED,
    RemoteFailureKindFfi.REVOKED,
    RemoteFailureKindFfi.VERIFICATION,
    RemoteFailureKindFfi.PAIRING_CODE_MISMATCH,
)

/** The string that explains [failure] to the user. */
@StringRes
fun remoteFailureText(failure: RemoteFailureFfi): Int {
    if (failure.kind == RemoteFailureKindFfi.NETWORK && (failure.detail == "401" || failure.detail == "403")) {
        return R.string.remote_fail_network_denied
    }
    if (failure.kind == RemoteFailureKindFfi.DENIED) {
        when (failure.detail) {
            "rate_limited" -> return R.string.remote_fail_denied_rate_limited
            "busy" -> return R.string.remote_fail_denied_busy
            "timeout" -> return R.string.remote_fail_denied_timeout
        }
    }
    return when (failure.kind) {
        RemoteFailureKindFfi.UNSUPPORTED -> R.string.remote_fail_unsupported
        RemoteFailureKindFfi.UNKNOWN_HOST -> R.string.remote_fail_unknown_host
        RemoteFailureKindFfi.NETWORK -> R.string.remote_fail_network
        RemoteFailureKindFfi.NO_ANSWER -> R.string.remote_fail_no_answer
        RemoteFailureKindFfi.CLOSED -> R.string.remote_fail_closed
        RemoteFailureKindFfi.CAPACITY -> R.string.remote_fail_capacity
        RemoteFailureKindFfi.BUSY -> R.string.remote_fail_busy
        RemoteFailureKindFfi.REVOKED -> R.string.remote_fail_revoked
        RemoteFailureKindFfi.DISABLED -> R.string.remote_fail_disabled
        RemoteFailureKindFfi.UPGRADE_REQUIRED -> R.string.remote_fail_upgrade
        RemoteFailureKindFfi.DENIED -> R.string.remote_fail_denied
        RemoteFailureKindFfi.EXPIRED -> R.string.remote_fail_expired
        RemoteFailureKindFfi.REJECTED -> R.string.remote_fail_rejected
        RemoteFailureKindFfi.VERIFICATION -> R.string.remote_fail_verification
        RemoteFailureKindFfi.PAIRING_CODE_MISMATCH -> R.string.remote_fail_pairing_mismatch
        RemoteFailureKindFfi.PROTOCOL -> R.string.remote_fail_protocol
    }
}

/** The pairing code as the desktop dialog shows it: `123 456`. */
fun formatPairingCode(code: String): String =
    if (code.length == 6) "${code.substring(0, 3)} ${code.substring(3)}" else code
