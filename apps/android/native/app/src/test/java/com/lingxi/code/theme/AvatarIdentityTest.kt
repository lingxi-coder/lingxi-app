package com.lingxi.code.theme

import com.lingxi.code.tabs.SharedVectors
import com.lingxi.code.tabs.objects
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

/** SPEC-mobile-team-bot-ui section 8.5: people's colours and initials, and the Bot avatar id of a team Bot key. */
class AvatarIdentityTest {
    private val avatars get() = SharedVectors.root.getJSONObject("avatars")

    @Test fun theColourListIsTheTwelveColoursOfTheDesktop() {
        val want = avatars.getJSONArray("colorList")
        assertEquals(12, want.length())
        assertEquals((0 until want.length()).map { want.getString(it) }, AvatarIdentity.AVATAR_COLORS.map { "#%06x".format(it and 0xFFFFFF) })
        assertTrue(AvatarIdentity.AVATAR_COLORS.all { it ushr 24 == 0xFFL })
    }

    @Test fun everyColourKeepsWhiteInitialsReadable() {
        fun linear(channel: Long): Double = (channel / 255.0).let { if (it <= 0.03928) it / 12.92 else Math.pow((it + 0.055) / 1.055, 2.4) }
        for (argb in AvatarIdentity.AVATAR_COLORS) {
            val luminance = 0.2126 * linear((argb shr 16) and 0xFF) + 0.7152 * linear((argb shr 8) and 0xFF) + 0.0722 * linear(argb and 0xFF)
            assertTrue("#%06x".format(argb and 0xFFFFFF), 1.05 / (luminance + 0.05) >= 4.5)
        }
    }

    @Test fun theHashIndexAndColourOfAnIdMatchTheDesktop() {
        val cases = avatars.getJSONArray("color").objects()
        assertEquals(6, cases.size)
        cases.forEach { case ->
            val id = case.getString("id")
            assertEquals(id, case.getLong("fnv1a"), AvatarIdentity.fnv1a(id))
            assertEquals(id, case.getInt("index"), AvatarIdentity.avatarColorIndex(id))
            assertEquals(id, case.getString("color"), AvatarIdentity.avatarColorHex(id))
            assertEquals(id, AvatarIdentity.AVATAR_COLORS[case.getInt("index")], AvatarIdentity.avatarColorArgb(id))
        }
    }

    @Test fun initialsAreTwoLettersOrOneCjkCharacterOrAQuestionMark() {
        val cases = avatars.getJSONArray("initials").objects()
        assertEquals(10, cases.size)
        cases.forEach { assertEquals(it.getString("name"), it.getString("expect"), AvatarIdentity.initials(it.getString("name"))) }
    }

    @Test fun aTeamTileTakesTheFirstCharacterOfTheName() {
        assertEquals("D", AvatarIdentity.teamInitial("design"))
        assertEquals("D", AvatarIdentity.teamInitial("  Design crew "))
        assertEquals("É", AvatarIdentity.teamInitial("élan"))
        assertEquals("🧠", AvatarIdentity.teamInitial("🧠 lab"))
        assertEquals("研", AvatarIdentity.teamInitial("研究组"))
        assertEquals("?", AvatarIdentity.teamInitial(" "))
    }

    @Test fun aTeamBotIsDrawnFromItsBotIdTheLastSegmentOfItsKey() {
        val cases = avatars.getJSONArray("teamBotAvatarId")
        assertEquals(3, cases.length())
        for (i in 0 until cases.length()) { val case = cases.getJSONArray(i); assertEquals(case.getString(0), case.getString(1), AvatarIdentity.teamBotAvatarId(case.getString(0))) }
    }
}
