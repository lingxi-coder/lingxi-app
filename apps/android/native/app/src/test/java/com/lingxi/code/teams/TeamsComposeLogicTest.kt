package com.lingxi.code.teams

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/** SPEC-mobile-team-bot-ui section 8.3: the composer rules, on the Desktop `compose.ts` vectors. */
class TeamsComposeLogicTest {
    private val compose get() = TeamsVectors.root.getJSONObject("compose")
    private val cards get() = compose.getJSONArray("botCards").objects().map(TeamsVectors::card)
    private fun conversation(kind: String, participants: List<String>) = TeamMessageConversation("c", kind, "", participants, 0, 0, 0, participants.filter { it.startsWith("bot:") })
    private fun lockName(lock: ComposerLock) = lock.name.replaceFirstChar { it.lowercase() }

    @Test fun limitsAreTheServersLimits() {
        assertEquals(compose.getInt("messageBytes"), TeamsComposeLogic.MESSAGE_BYTES)
        assertEquals(compose.getInt("maxTargets"), TeamsComposeLogic.MAX_TARGETS)
    }

    @Test fun canSendTextRejectsBlankAndOversizedText() {
        val cases = compose.getJSONArray("canSendText").objects()
        assertEquals(6, cases.size)
        cases.forEach { assertEquals(it.getString("text"), it.getBoolean("expect"), TeamsComposeLogic.canSendText(TeamsVectors.expand(it.getString("text")))) }
    }

    @Test fun byteLengthCountsUtf8Bytes() {
        val cases = compose.getJSONArray("byteLength")
        assertEquals(3, cases.length())
        for (i in 0 until cases.length()) { val case = cases.getJSONArray(i); assertEquals(case.getString(0), case.getInt(1), TeamsComposeLogic.byteLength(case.getString(0))) }
    }

    @Test fun mentionAtFindsTheWordBeingTypedAfterASpaceOrAnOpeningBracket() {
        val cases = compose.getJSONArray("mentionAt").objects()
        assertEquals(8, cases.size)
        cases.forEach { case ->
            val got = TeamsComposeLogic.mentionAt(case.getString("text"), case.getInt("caret"))
            if (case.isNull("expect")) assertNull(case.getString("text"), got)
            else assertEquals(case.getString("text"), MentionAt(case.getJSONObject("expect").getInt("start"), case.getJSONObject("expect").getString("query")), got)
        }
    }

    @Test fun mentionAtUsesJavaScriptWhitespaceAndTheFortyCharacterQueryLimit() {
        for (space in listOf(" ", "　", "﻿", " ", "\t", "\n")) assertEquals(MentionAt(3, "a"), TeamsComposeLogic.mentionAt("hi$space@a", 5))
        assertEquals(MentionAt(3, "a".repeat(40)), TeamsComposeLogic.mentionAt("hi @" + "a".repeat(40), 3 + 1 + 40))
        assertNull(TeamsComposeLogic.mentionAt("hi @" + "a".repeat(41), 3 + 1 + 41))
        assertNull(TeamsComposeLogic.mentionAt("hi @a @", 0))
        assertEquals(MentionAt(0, ""), TeamsComposeLogic.mentionAt("@", 99))
    }

    @Test fun withoutMentionKeepsOneSpaceBetweenTheNeighbours() {
        val cases = compose.getJSONArray("withoutMention").objects()
        assertEquals(4, cases.size)
        cases.forEach { assertEquals(it.getString("text"), it.getString("expect"), TeamsComposeLogic.withoutMention(it.getString("text"), it.getInt("start"), it.getInt("caret"))) }
    }

    @Test fun snippetFlattensWhitespaceAndCutsByCodePoints() {
        val cases = compose.getJSONArray("snippet").objects()
        assertEquals(4, cases.size)
        cases.forEach { assertEquals(it.getString("text"), TeamsVectors.expand(it.getString("expect")), TeamsComposeLogic.snippet(TeamsVectors.expand(it.getString("text")), it.getInt("length"))) }
        assertEquals("a b", TeamsComposeLogic.snippet("a　 b"))
        assertEquals("", TeamsComposeLogic.snippet("   "))
    }

    @Test fun mentionCandidatesOnlyOfferUsablePresentUnchosenBotsInGroupsAndChannels() {
        val cases = compose.getJSONArray("mentionCandidates").objects()
        assertEquals(5, cases.size)
        cases.forEach { case ->
            val got = TeamsComposeLogic.mentionCandidates(cards, conversation(case.getString("kind"), TeamsVectors.strings(case.getJSONArray("participants"))), TeamsVectors.strings(case.getJSONArray("chosen")), case.getString("query"))
            assertEquals("${case.getString("kind")} / ${case.getString("query")}", TeamsVectors.strings(case.getJSONArray("expect")), got.map { it.sharedBotId })
        }
    }

    @Test fun composerLockExplainsWhyTheComposerIsClosed() {
        val cases = compose.getJSONArray("composerLock").objects()
        assertEquals(9, cases.size)
        cases.forEach { case ->
            val keys = TeamsVectors.strings(case.getJSONArray("bots"))
            val bots = cards.filter { it.sharedBotId in keys }
            val got = TeamsComposeLogic.composerLock(case.getString("uid"), case.getString("kind"), TeamsVectors.strings(case.getJSONArray("participants")), bots, case.getBoolean("botsLoaded"), case.getBoolean("botsUnverified"))
            assertEquals("${case.getString("kind")} ${case.getJSONArray("participants")} ${case.getJSONArray("bots")}", case.getString("expect"), lockName(got))
        }
    }

    @Test fun aBotDmWithAUsableBotNeedsNoCheckAndAMemberlessViewerStillReadsOnly() {
        val usable = cards.filter { it.sharedBotId == "sharedbot_a" }
        assertEquals(ComposerLock.None, TeamsComposeLogic.composerLock("u1", "bot_dm", listOf("user:u1", "shared_bot:sharedbot_a"), usable, botsLoaded = false, botsUnverified = true))
        assertEquals(ComposerLock.OwnerReading, TeamsComposeLogic.composerLock("u1", "group", listOf("user:u2", "user:u3"), usable, true, false))
        assertFalse(TeamsComposeLogic.canSendText("　\n"))
        assertTrue(TeamsComposeLogic.canSendText(" a "))
    }
}
