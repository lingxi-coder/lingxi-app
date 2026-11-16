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

    @Test fun spokenWordsAreAddedToTheBoxAndNeverReplaceIt() {
        assertEquals("hello there", TeamsComposeLogic.appendingTranscript("hello there", ""))
        assertEquals("hello there", TeamsComposeLogic.appendingTranscript("  hello there\n", "  \n"))
        assertEquals("typed and more", TeamsComposeLogic.appendingTranscript("and more", "typed"))
        assertEquals("typed and more", TeamsComposeLogic.appendingTranscript("and more", "typed "))
        assertEquals("第一句\n第二句", TeamsComposeLogic.appendingTranscript("第二句", "第一句\n"))
        // Nothing heard (silence, a cancelled recognition) leaves the box exactly as it was.
        assertEquals("typed ", TeamsComposeLogic.appendingTranscript("", "typed "))
        assertEquals("", TeamsComposeLogic.appendingTranscript(" \n ", ""))
    }

    @Test fun wordsSpokenForOneBoxNeverReachAnother() {
        val box = "uid1:team1:3:conv1:"
        assertTrue(TeamVoiceLogic.delivers(box, box))
        // Another conversation, another account, a new membership generation, a message being changed: all are other boxes.
        assertFalse(TeamVoiceLogic.delivers(box, "uid1:team1:3:conv2:"))
        assertFalse(TeamVoiceLogic.delivers(box, "uid2:team1:3:conv1:"))
        assertFalse(TeamVoiceLogic.delivers(box, "uid1:team1:4:conv1:"))
        assertFalse(TeamVoiceLogic.delivers(box, "uid1:team1:3:conv1:msg1"))
        // Nothing was being spoken (it was cancelled): words that still arrive are dropped.
        assertFalse(TeamVoiceLogic.delivers(null, box))
    }

    @Test fun theMicrophonesOwnEndIsWordsAnExplanationOrNothing() {
        assertEquals(TeamVoiceLogic.End.Heard, TeamVoiceLogic.end(com.lingxi.code.voice.VoiceCapturePhase.Completed))
        assertEquals(TeamVoiceLogic.End.Failed, TeamVoiceLogic.end(com.lingxi.code.voice.VoiceCapturePhase.Failed))
        assertEquals(TeamVoiceLogic.End.Quiet, TeamVoiceLogic.end(com.lingxi.code.voice.VoiceCapturePhase.PermissionRequired))
        assertEquals(TeamVoiceLogic.End.Quiet, TeamVoiceLogic.end(com.lingxi.code.voice.VoiceCapturePhase.Cancelled))
        for (phase in listOf(com.lingxi.code.voice.VoiceCapturePhase.Idle, com.lingxi.code.voice.VoiceCapturePhase.Starting, com.lingxi.code.voice.VoiceCapturePhase.Listening, com.lingxi.code.voice.VoiceCapturePhase.Stopping)) {
            assertEquals(null, TeamVoiceLogic.end(phase))
            assertFalse(TeamVoiceLogic.abandoned(phase))
        }
        assertTrue(TeamVoiceLogic.abandoned(com.lingxi.code.voice.VoiceCapturePhase.Cancelled))
        assertTrue(TeamVoiceLogic.abandoned(com.lingxi.code.voice.VoiceCapturePhase.Failed))
        assertFalse(TeamVoiceLogic.abandoned(com.lingxi.code.voice.VoiceCapturePhase.Completed))
    }
}
