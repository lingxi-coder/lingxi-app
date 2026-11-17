package com.lingxi.code.teams

import com.lingxi.code.R
import org.json.JSONObject
import org.junit.Assert.*
import org.junit.Test
import java.io.File

class TeamBotEntryTest {
    private val unknown = TeamBotValue.Words(R.string.chat_status_unknown)
    private val supported = TeamBotValue.Words(R.string.teams_bot_supported)
    private val notSupported = TeamBotValue.Words(R.string.teams_bot_not_supported)
    private fun card(extra: String = "") = TeamBotCard.parse(JSONObject("""{"sharedBotId":"sharedbot_01","name":"Helper","description":"","maintainer":{"uid":"owner","generation":1},"revision":1,"state":"published","permitted":true,"ready":true,"onlineUntil":0,"usePolicy":{"mode":"all"},"quotas":{"team":100,"member":10,"bot":50}$extra}"""))

    @Test fun missingProjectionNeverInventsStatusCapabilityOrZeroRemaining() {
        val bot = card()
        assertTrue(bot.examples.isEmpty())
        assertNull(bot.capabilities)
        assertNull(bot.availability)
        assertNull(bot.dailyRemaining)
        assertNull(bot.quotaResetsAt)
        assertEquals(R.string.teams_bot_avail_unverified, TeamBotEntry.availabilityRes(bot))
        assertEquals("a Bot that has not reported what it can do is Unknown, not unsupported", listOf(unknown, unknown), TeamBotEntry.capabilities(bot).map { it.value })
        // The remaining requests and the reset are Unknown, never the 100 / 10 / 50 limits of the quotas the card also carries.
        assertEquals(TeamBotItem(R.string.teams_bot_remaining_today, unknown), TeamBotEntry.remaining(bot))
        assertEquals(TeamBotItem(R.string.teams_bot_resets, unknown), TeamBotEntry.resets(bot))
    }

    @Test fun projectionParsesExamplesActualCapabilitiesAndExplicitZero() {
        val bot = card(""", "examples":["First","Second","Third","Fourth"],"capabilities":{"durableWait":true,"writeOperations":false},"availability":"quota_exhausted","dailyRemaining":0,"quotaResetsAt":1794873600000""")
        assertEquals(listOf("First", "Second", "Third"), bot.examples)
        assertEquals(TeamBotCapabilities(true, false), bot.capabilities)
        assertEquals(0L, bot.dailyRemaining)
        assertEquals(R.string.teams_bot_avail_quota, TeamBotEntry.availabilityRes(bot))
        assertEquals(listOf(TeamBotItem(R.string.teams_bot_wait_resume, supported), TeamBotItem(R.string.teams_bot_write_ops, notSupported)), TeamBotEntry.capabilities(bot))
        assertEquals("zero is a figure, not Unknown", TeamBotItem(R.string.teams_bot_remaining_today, TeamBotValue.Figure("0")), TeamBotEntry.remaining(bot))
        assertEquals(TeamBotItem(R.string.teams_bot_resets, TeamBotValue.Figure("2026-11-17 00:00 UTC")), TeamBotEntry.resets(bot))
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
        assertEquals(R.string.teams_bot_avail_maintenance, TeamBotEntry.availabilityRes(bot))
        assertEquals(listOf(notSupported, supported), TeamBotEntry.capabilities(bot).map { it.value })
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
        assertEquals(R.string.teams_bot_avail_offline, TeamBotEntry.availabilityRes(card().copy(availability = "offline")))
    }

    @Test fun memberDirectoryKeepsAuthorizedPausedCardsWithoutMakingThemSelectable() {
        val paused = card().copy(state = "paused", permitted = false, availability = "paused")
        val published = card()
        val archived = card().copy(state = "retired")
        assertEquals(listOf(paused, published), TeamBotEntry.memberDirectory(listOf(paused, published, archived)))
        assertFalse(paused.selectable)
        assertEquals("a Bot whose sharing stopped reads Sharing stopped (U1), never Paused", R.string.teams_bot_disabled, TeamBotEntry.availabilityRes(paused))
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
        assertEquals(R.string.teams_bot_avail_unverified, TeamBotEntry.availabilityRes(expired))
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

    /** The member text is the catalog's: the keys Desktop and Web read (`packages/teams/BotMemberDetails.tsx`), each with its own words. */
    @Test fun everyAvailabilityNamesItsOwnSharedWordsAndAStateThisBuildDoesNotKnowReadsUnverified() {
        val expected = mapOf(
            "ready" to R.string.teams_bot_avail_ready, "offline" to R.string.teams_bot_avail_offline, "maintenance" to R.string.teams_bot_avail_maintenance,
            "quota_exhausted" to R.string.teams_bot_avail_quota, "unverified" to R.string.teams_bot_avail_unverified,
        )
        for ((state, id) in expected) assertEquals(state, id, TeamBotEntry.availabilityRes(card().copy(availability = state)))
        assertEquals(expected.size, expected.values.toSet().size)
        // Whatever its host reports, a Bot whose sharing stopped reads so; a missing or newer state reads unverified.
        for (state in listOf("ready", "offline", null)) assertEquals(R.string.teams_bot_disabled, TeamBotEntry.availabilityRes(card().copy(state = "paused", availability = state)))
        assertEquals(R.string.teams_bot_disabled, TeamBotEntry.availabilityRes(card().copy(availability = "paused")))
        for (state in listOf(null, "from-a-newer-service", "")) assertEquals(R.string.teams_bot_avail_unverified, TeamBotEntry.availabilityRes(card().copy(availability = state)))
    }

    @Test fun theMemberTextHasItsWordsInAllFiveLanguages() {
        val keys = listOf(
            "teams_bot_avail_ready", "teams_bot_avail_offline", "teams_bot_avail_maintenance", "teams_bot_avail_quota", "teams_bot_avail_unverified", "teams_bot_disabled",
            "teams_bot_supported", "teams_bot_not_supported", "teams_bot_wait_resume", "teams_bot_write_ops", "teams_bot_remaining_today", "teams_bot_resets",
            "teams_bot_examples_hint", "chat_status_unknown",
        )
        for (directory in listOf("values", "values-zh-rTW", "values-en", "values-ja", "values-ko")) {
            val file = listOf("src/main/res/$directory/strings.xml", "app/src/main/res/$directory/strings.xml", "apps/android/native/app/src/main/res/$directory/strings.xml").map(::File).first { it.exists() }
            // Plain text on purpose: the Android XML classes on the unit-test class path are stubs.
            val names = Regex("<string\\s+name=\"([^\"]+)\"[^>]*>(.+?)</string>", RegexOption.DOT_MATCHES_ALL).findAll(file.readText()).associate { it.groupValues[1] to it.groupValues[2] }
            for (key in keys) assertFalse("$directory has no text for $key", names[key].isNullOrBlank())
        }
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
