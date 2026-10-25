package com.lingxi.code.remote

import com.lingxi.code.R
import com.lingxi.code.bindings.android.RemoteConnectionPhaseFfi
import com.lingxi.code.bindings.android.RemoteConnectionStateFfi
import com.lingxi.code.bindings.android.RemoteFailureFfi
import com.lingxi.code.bindings.android.RemoteFailureKindFfi
import com.lingxi.code.bindings.android.RemoteTransportFfi
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class RemoteConnectionUiTest {
    private fun state(
        phase: RemoteConnectionPhaseFfi,
        pairingCode: String? = null,
        failure: RemoteFailureFfi? = null,
        retryAtMs: ULong? = null,
        everReady: Boolean = false,
    ) = RemoteConnectionStateFfi(
        hostId = "h", phase = phase, transport = RemoteTransportFfi.TUNNEL, pairingCode = pairingCode,
        failure = failure, attempt = 0u, retryAtMs = retryAtMs, host = null, everReady = everReady,
    )

    private fun failed(kind: RemoteFailureKindFfi, detail: String? = null) =
        remoteConnectionUi(state(RemoteConnectionPhaseFfi.FAILED, failure = RemoteFailureFfi(kind, detail, false)))
            as RemoteConnectionUi.Failed

    @Test fun nothingDialledIsIdle() {
        assertEquals(RemoteConnectionUi.Idle, remoteConnectionUi(null))
        assertEquals(RemoteConnectionUi.Idle, remoteConnectionUi(state(RemoteConnectionPhaseFfi.IDLE)))
    }

    @Test fun pairingShowsTheCodeAndWaitsForTheDesktop() {
        assertEquals(RemoteConnectionUi.Pairing("123456"), remoteConnectionUi(state(RemoteConnectionPhaseFfi.PAIRING, pairingCode = "123456")))
        assertEquals("123 456", formatPairingCode("123456"))
        assertEquals("12", formatPairingCode("12"))
        // No code yet: still dialling.
        assertTrue(remoteConnectionUi(state(RemoteConnectionPhaseFfi.PAIRING)) is RemoteConnectionUi.Connecting)
    }

    @Test fun readyCarriesTheTransport() {
        assertEquals(RemoteConnectionUi.Ready(RemoteTransportFfi.TUNNEL), remoteConnectionUi(state(RemoteConnectionPhaseFfi.READY)))
    }

    @Test fun reconnectingExplainsWhyAndWhen() {
        val ui = remoteConnectionUi(
            state(RemoteConnectionPhaseFfi.RECONNECTING, failure = RemoteFailureFfi(RemoteFailureKindFfi.NETWORK, null, true), retryAtMs = 5000uL),
        ) as RemoteConnectionUi.Reconnecting
        assertEquals(R.string.remote_fail_network, ui.reason)
        assertEquals(5000L, ui.retryAtMs)
    }

    @Test fun deniedFailuresExplainTheirReasonAndCanBeRetried() {
        val denied = failed(RemoteFailureKindFfi.DENIED)
        assertEquals(R.string.remote_fail_denied, denied.message)
        assertTrue(denied.canRetry)
        assertFalse(denied.repair)
        assertEquals(R.string.remote_fail_denied_rate_limited, failed(RemoteFailureKindFfi.DENIED, "rate_limited").message)
        assertEquals(R.string.remote_fail_denied_busy, failed(RemoteFailureKindFfi.DENIED, "busy").message)
        assertEquals(R.string.remote_fail_denied_timeout, failed(RemoteFailureKindFfi.DENIED, "timeout").message)
    }

    @Test fun revokedOffersToPairAgainButNotToRetry() {
        val revoked = failed(RemoteFailureKindFfi.REVOKED)
        assertEquals(R.string.remote_fail_revoked, revoked.message)
        assertTrue(revoked.repair)
        assertFalse(revoked.canRetry)
        assertFalse(revoked.forget)
    }

    @Test fun aKeyOrCodeMismatchOffersForgettingAndNoRetry() {
        for (kind in listOf(RemoteFailureKindFfi.VERIFICATION, RemoteFailureKindFfi.PAIRING_CODE_MISMATCH)) {
            val ui = failed(kind)
            assertTrue(kind.name, ui.forget)
            assertFalse(kind.name, ui.canRetry)
            assertFalse(kind.name, ui.repair)
        }
    }

    @Test fun updatesAndSettingsCannotBeFixedByRetrying() {
        for (kind in listOf(RemoteFailureKindFfi.UNSUPPORTED, RemoteFailureKindFfi.UPGRADE_REQUIRED, RemoteFailureKindFfi.DISABLED)) {
            assertFalse(kind.name, failed(kind).canRetry)
        }
        for (kind in listOf(RemoteFailureKindFfi.NETWORK, RemoteFailureKindFfi.NO_ANSWER, RemoteFailureKindFfi.CLOSED, RemoteFailureKindFfi.BUSY)) {
            assertTrue(kind.name, failed(kind).canRetry)
        }
    }

    @Test fun anExpiredTokenOnTheDatabaseIsToldApart() {
        assertEquals(R.string.remote_fail_network_denied, failed(RemoteFailureKindFfi.NETWORK, "401").message)
        assertEquals(R.string.remote_fail_network_denied, failed(RemoteFailureKindFfi.NETWORK, "403").message)
        assertEquals(R.string.remote_fail_network, failed(RemoteFailureKindFfi.NETWORK, "500").message)
    }

    @Test fun everyFailureKindHasAMessage() {
        for (kind in RemoteFailureKindFfi.values()) failed(kind)
    }
}
