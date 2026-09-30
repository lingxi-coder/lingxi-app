package com.lingxi.code.localapps

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class LocalAppWebStorageCleanupTest {
    @Test
    fun `cleanup is only confirmed after app disappears from authoritative snapshot`() {
        val queued = LocalAppWebStorageCleanupLedger()
            .remembering("app-a", "http://127.0.0.1:43100")
            .preparing("app-a", explicitOrigin = null)

        val stillLive = queued.confirmingMissing(setOf("app-a"))
        assertFalse(stillLive.pending.single().confirmedDeleted)

        val deleted = stillLive.confirmingMissing(emptySet())
        assertTrue(deleted.pending.single().confirmedDeleted)
        assertEquals("http://127.0.0.1:43100", deleted.pending.single().origin)
    }

    @Test
    fun `completed cleanup removes only the deleted app origin`() {
        val ledger = LocalAppWebStorageCleanupLedger()
            .remembering("app-a", "http://127.0.0.1:43100")
            .remembering("app-b", "http://127.0.0.1:43101")
            .preparing("app-a", null)
            .confirmingMissing(setOf("app-b"))
        val completed = ledger.completed(ledger.pending.single())

        assertEquals(mapOf("app-b" to "http://127.0.0.1:43101"), completed.origins)
        assertTrue(completed.pending.isEmpty())
    }

    @Test
    fun `reassigned origin remains blocked until old cleanup is complete`() {
        val origin = "http://127.0.0.1:43100"
        val oldEntryLedger = LocalAppWebStorageCleanupLedger()
            .remembering("old-app", origin)
            .preparing("old-app", null)
            .confirmingMissing(emptySet())

        assertTrue(oldEntryLedger.hasPendingOrigin(origin))

        val cleaned = oldEntryLedger.completed(oldEntryLedger.pending.single())
        assertFalse(cleaned.hasPendingOrigin(origin))
        assertEquals(
            mapOf("new-app" to origin),
            cleaned.remembering("new-app", origin).origins,
        )
    }

    @Test
    fun `late duplicate completion cannot remove a replacement app origin`() {
        val origin = "http://127.0.0.1:43100"
        val oldLedger = LocalAppWebStorageCleanupLedger()
            .remembering("same-id", origin)
            .preparing("same-id", null)
            .confirmingMissing(emptySet())
        val entry = oldLedger.pending.single()
        val replaced = oldLedger.completed(entry).remembering("same-id", origin)

        assertEquals(replaced, replaced.completed(entry))
    }

    @Test
    fun `same id deletion keeps an older confirmed origin cleanup`() {
        val oldOrigin = "http://127.0.0.1:43100"
        val newOrigin = "http://127.0.0.1:43101"
        val oldConfirmed = LocalAppWebStorageCleanupLedger()
            .remembering("same-id", oldOrigin)
            .preparing("same-id", null)
            .confirmingMissing(emptySet())

        val both = oldConfirmed
            .remembering("same-id", newOrigin)
            .preparing("same-id", null)

        assertEquals(setOf(oldOrigin, newOrigin), both.pending.map { it.origin }.toSet())
        assertTrue(both.pending.single { it.origin == oldOrigin }.confirmedDeleted)
        assertFalse(both.pending.single { it.origin == newOrigin }.confirmedDeleted)
    }

    @Test
    fun `failed delete releases unconfirmed gate but cannot cancel confirmed cleanup`() {
        val origin = "http://127.0.0.1:43100"
        val prepared = LocalAppWebStorageCleanupLedger()
            .remembering("app-a", origin)
            .preparing("app-a", null)

        val cancelled = prepared.cancellingUnconfirmed("app-a")
        assertFalse(cancelled.hasPendingOrigin(origin))
        assertEquals(origin, cancelled.origins["app-a"])

        val confirmed = prepared.confirmingMissing(emptySet())
        assertEquals(confirmed, confirmed.cancellingUnconfirmed("app-a"))
        assertTrue(confirmed.hasPendingOrigin(origin))
    }

    @Test
    fun `unconfirmed deletion does not wedge the same app but still blocks origin reuse`() {
        val origin = "http://127.0.0.1:43100"
        val pending = LocalAppWebStorageCleanupLedger()
            .remembering("app-a", origin)
            .preparing("app-a", null)

        assertFalse(pending.blocksOriginClaim("app-a", origin))
        assertTrue(pending.blocksOriginClaim("app-b", origin))
        assertTrue(
            pending.confirmingMissing(emptySet()).blocksOriginClaim("app-a", origin),
        )
    }

    @Test
    fun `disappearance without explicit preparation never creates cleanup work`() {
        val ledger = LocalAppWebStorageCleanupLedger()
            .remembering("app-a", "http://127.0.0.1:43100")

        assertTrue(ledger.confirmingMissing(emptySet()).pending.isEmpty())
    }

    @Test
    fun `ledger round trip preserves confirmed pending work for launch retry`() {
        val expected = LocalAppWebStorageCleanupLedger()
            .remembering("app-a", "http://localhost:43100")
            .preparing("app-a", null)
            .confirmingMissing(emptySet())

        assertEquals(expected, LocalAppWebStorageCleanupLedger.decode(expected.encode()))
    }

    @Test
    fun `trusted origin canonicalization excludes paths and external hosts`() {
        assertEquals("http://127.0.0.1:43100", trustedLoopbackOrigin("http://127.0.0.1:43100/a?q=1"))
        assertEquals("http://[::1]:43100", trustedLoopbackOrigin("http://[::1]:43100/a"))
        assertEquals(null, trustedLoopbackOrigin("https://example.com/a"))
    }
}
