package com.lingxi.code.teams

import org.json.JSONObject
import org.junit.Assert.*
import org.junit.Test

class TeamBotEntryTest {
    private fun card(extra: String = "") = TeamBotCard.parse(JSONObject("""{"sharedBotId":"sharedbot_01","name":"Helper","description":"","maintainer":{"uid":"owner","generation":1},"revision":1,"state":"published","permitted":true,"ready":true,"onlineUntil":0,"usePolicy":{"mode":"all"},"quotas":{"team":100,"member":10,"bot":50}$extra}"""))

    @Test fun missingProjectionNeverInventsStatusCapabilityOrZeroRemaining() {
        val bot = card()
        assertTrue(bot.examples.isEmpty())
        assertNull(bot.capabilities)
        assertNull(bot.availability)
        assertNull(bot.dailyRemaining)
        assertNull(bot.quotaResetsAt)
        assertEquals("Status unverified", TeamBotEntry.availability(bot, false))
        assertEquals("Capabilities unverified", TeamBotEntry.capabilities(bot, false))
        assertTrue(TeamBotEntry.quota(bot, false).contains("unverified"))
        assertFalse(TeamBotEntry.quota(bot, false).contains("100"))
    }

    @Test fun projectionParsesExamplesActualCapabilitiesAndExplicitZero() {
        val bot = card(""", "examples":["First","Second","Third","Fourth"],"capabilities":{"durableWait":true,"writeOperations":false},"availability":"quota_exhausted","dailyRemaining":0,"quotaResetsAt":1794873600000""")
        assertEquals(listOf("First", "Second", "Third"), bot.examples)
        assertEquals(TeamBotCapabilities(true, false), bot.capabilities)
        assertEquals(0L, bot.dailyRemaining)
        assertEquals("Daily quota exhausted", TeamBotEntry.availability(bot, false))
        assertTrue(TeamBotEntry.capabilities(bot, false).contains("Waits"))
        assertFalse(TeamBotEntry.capabilities(bot, false).contains("Writes"))
        assertEquals("Your remaining uses today: 0 · Resets 2026-11-17 00:00 UTC", TeamBotEntry.quota(bot, false))
    }

    @Test fun malformedOptionalValuesRemainUnknown() {
        val bot = card(""", "examples":[null,42," ","Valid","Valid"],"capabilities":{"durableWait":true},"availability":"runtime secret","dailyRemaining":-1,"quotaResetsAt":"0"""")
        assertEquals(listOf("Valid"), bot.examples)
        assertNull(bot.capabilities)
        assertNull(bot.availability)
        assertNull(bot.dailyRemaining)
        assertNull(bot.quotaResetsAt)
        assertNull(TeamBotEntry.nonnegativeInteger(0.5))
        assertNull(TeamBotEntry.nonnegativeInteger(Double.NaN))
        assertNull(TeamBotEntry.nonnegativeInteger(true))
        assertNull(TeamBotEntry.nonnegativeInteger("0"))
    }

    @Test fun maintenanceNeverShowsDetailedHostReason() {
        val bot = card(""", "availability":"maintenance","healthReason":"model","capabilities":{"durableWait":false,"writeOperations":true}""")
        assertEquals("Under maintenance", TeamBotEntry.availability(bot, false))
        assertEquals("维护中", TeamBotEntry.availability(bot, true))
        assertFalse(TeamBotEntry.capabilities(bot, false).contains("Waits"))
        assertTrue(TeamBotEntry.capabilities(bot, false).contains("Writes"))
    }

    @Test fun examplesOnlyFillAnEmptyDraftAndNeverAppendOrSend() {
        assertTrue(TeamBotEntry.canSeed(ComposerDraft(), false, "Try this"))
        assertFalse(TeamBotEntry.canSeed(ComposerDraft("Existing"), false, "Try this"))
        assertFalse(TeamBotEntry.canSeed(ComposerDraft(" "), false, "Try this"))
        assertFalse(TeamBotEntry.canSeed(ComposerDraft(replyToId = "message"), false, "Try this"))
        assertFalse(TeamBotEntry.canSeed(ComposerDraft(editingId = "message"), false, "Try this"))
        assertFalse(TeamBotEntry.canSeed(ComposerDraft(targets = setOf("other")), false, "Try this"))
        assertFalse(TeamBotEntry.canSeed(ComposerDraft(), true, "Try this"))
        assertFalse(TeamBotEntry.canSeed(ComposerDraft(), false, " "))
        assertFalse(TeamBotEntry.canSeed(ComposerDraft(), false, "x".repeat(8193)))
    }

    @Test fun explicitHostAdmissionBlocksOnlyNewBotRequests() {
        val targets = setOf(card().sharedBotId)
        for (status in listOf("paused", "offline", "maintenance", "quota_exhausted")) {
            val bot = card().copy(availability = status)
            assertEquals(listOf(bot), TeamBotEntry.blockedRequests(targets, listOf(bot), false))
            // Existing wait replies and message edits do not request another run.
            assertTrue(TeamBotEntry.blockedRequests(targets, listOf(bot), true).isEmpty())
            // Human-only messages have no Bot target, regardless of host or network state.
            assertTrue(TeamBotEntry.blockedRequests(emptySet(), listOf(bot), false).isEmpty())
        }
        for (status in listOf(null, "ready", "unverified"))
            assertTrue(TeamBotEntry.blockedRequests(targets, listOf(card().copy(availability = status)), false).isEmpty())
        assertEquals("Host offline", TeamBotEntry.availability(card().copy(availability = "offline"), false))
    }

    @Test fun memberDirectoryKeepsAuthorizedPausedCardsWithoutMakingThemSelectable() {
        val paused = card().copy(state = "paused", permitted = false, availability = "paused")
        val published = card()
        val archived = card().copy(state = "retired")
        assertEquals(listOf(paused, published), TeamBotEntry.memberDirectory(listOf(paused, published, archived)))
        assertFalse(paused.selectable)
        assertEquals("Paused", TeamBotEntry.availability(paused, false))
        assertEquals("已暂停", TeamBotEntry.availability(paused, true))
        assertEquals(setOf(paused.sharedBotId), staleTeamBotSelections(setOf(paused.sharedBotId), listOf(paused)))
    }

    @Test fun maintenanceTimerExpiresAtNinetySecondsWithoutAnotherSignal() {
        var clock = 1_000_000L
        val bot = card(""", "availability":"maintenance","statusExpiresAt":1090000""").copy(ready = false, onlineUntil = 1_180_000)
        assertEquals(1_090_000L, bot.statusExpiresAt)
        val deadline = TeamBotEntry.nextStatusDeadline(listOf(bot), clock)!!
        assertEquals(90_000L, deadline - clock)
        clock += 89_999
        assertEquals("maintenance", TeamBotEntry.expireStatuses(listOf(bot), clock).single().availability)
        clock++
        val expired = TeamBotEntry.expireStatuses(listOf(bot), clock).single()
        assertEquals("unverified", expired.availability)
        assertFalse(expired.ready)
        assertNull(TeamBotEntry.nextStatusDeadline(listOf(expired), clock))
        assertEquals("Status unverified", TeamBotEntry.availability(expired, false))
        // A failed reread keeps the server-authorized card, but never restores the expired maintenance claim.
        assertEquals(expired, TeamBotEntry.unverified(expired))
    }

    @Test fun statusExpiryPrefersProjectionAndFallsBackOnlyForLegacyReadyCards() {
        val now = 1_000L
        val legacyReady = card().copy(onlineUntil = 91_000)
        val legacyNotReady = legacyReady.copy(ready = false)
        val projectedNotReady = legacyNotReady.copy(availability = "maintenance", statusExpiresAt = 46_000)
        assertEquals(91_000L, TeamBotEntry.nextStatusDeadline(listOf(legacyReady), now))
        assertNull(TeamBotEntry.nextStatusDeadline(listOf(legacyNotReady), now))
        assertEquals(46_000L, TeamBotEntry.nextStatusDeadline(listOf(legacyReady, projectedNotReady), now))
        assertEquals("unverified", TeamBotEntry.expireStatuses(listOf(legacyReady), 91_000).single().availability)
        assertNull(card(""", "statusExpiresAt":"123"""").statusExpiresAt)
        assertNull(card(""", "statusExpiresAt":-1""").statusExpiresAt)
    }

    @Test fun expiredKnownBlocksSurvivePendingFailedAndUnknownRefreshUntilFreshReady() {
        val now = 90_000L
        for (status in listOf("maintenance", "quota_exhausted")) {
            val known = card().copy(ready = false, availability = status, statusExpiresAt = now)
            val pending = TeamBotEntry.expireStatuses(listOf(known), now).single()
            assertTrue(pending.permitted)
            assertEquals("published", pending.state)
            assertEquals("unverified", pending.availability)
            assertTrue(pending.admissionBlockedPendingRefresh)
            val failed = TeamBotEntry.unverified(pending)
            val unknown = TeamBotEntry.acceptSnapshot(listOf(failed), listOf(card().copy(availability = "unverified", ready = false)), now).single()
            for (bot in listOf(pending, failed, unknown)) {
                assertEquals(listOf(bot), TeamBotEntry.blockedRequests(setOf(bot.sharedBotId), listOf(bot), false))
                assertTrue(TeamBotEntry.blockedRequests(setOf(bot.sharedBotId), listOf(bot), true).isEmpty())
                assertTrue(TeamBotEntry.blockedRequests(emptySet(), listOf(bot), false).isEmpty())
                try { TeamBotEntry.requireNewRequest(setOf(bot.sharedBotId), listOf(bot)); fail("Must stop before a new submit is journaled") }
                catch (error: TeamMessageFailure) { assertEquals("BOT_STATUS_UNVERIFIED", error.code) }
            }
            val staleReady = card().copy(availability = "ready", statusExpiresAt = now)
            assertTrue(TeamBotEntry.acceptSnapshot(listOf(unknown), listOf(staleReady), now).single().admissionBlockedPendingRefresh)
            val ready = TeamBotEntry.acceptSnapshot(listOf(unknown), listOf(card().copy(availability = "ready", statusExpiresAt = now + 90_000)), now).single()
            assertFalse(ready.admissionBlockedPendingRefresh)
            TeamBotEntry.requireNewRequest(setOf(ready.sharedBotId), listOf(ready))
        }
        // Legacy/missing projections do not acquire a new local block merely from a read failure.
        val legacyUnknown = TeamBotEntry.unverified(card())
        assertFalse(legacyUnknown.admissionBlockedPendingRefresh)
        TeamBotEntry.requireNewRequest(setOf(legacyUnknown.sharedBotId), listOf(legacyUnknown))
    }

    @Test fun contextIncludesAccountTeamMembershipAndLifecycle() {
        val original = TeamBotEntryContext("alice", "team1", 1, 1)
        assertNotEquals(original, original.copy(uid = "bob"))
        assertNotEquals(original, original.copy(teamId = "team2"))
        assertNotEquals(original, original.copy(generation = 2))
        assertNotEquals(original, original.copy(epoch = 2))
        val lease = TeamBotEntryLease()
        assertTrue(TeamBotEntry.canOpen(original, original, lease.active))
        assertFalse(TeamBotEntry.canOpen(original, original.copy(uid = "bob"), lease.active))
        assertFalse(TeamBotEntry.canOpen(original, original.copy(teamId = "team2"), lease.active))
        assertFalse(TeamBotEntry.canOpen(original, original.copy(generation = 2), lease.active))
        assertFalse(TeamBotEntry.canOpen(original, original.copy(epoch = 2), lease.active))
        assertFalse(TeamBotEntry.canOpen(original, null, lease.active))
        lease.active = false
        assertFalse(TeamBotEntry.canOpen(original, original, lease.active))
    }
}
