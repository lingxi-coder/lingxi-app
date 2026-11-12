package com.lingxi.code.teams

import com.lingxi.code.R
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/** The team-management confirmations (U8): a title that names the thing, a body, a verb, and the command each one sends. */
class TeamConfirmationsTest {
    private val team = TeamSummary("t1", "Design", 4, "owner", 3)
    private val member = TeamMember("u2", "Mia", "member", 5)
    private val admin = member.copy(role = "admin")
    private val invite = TeamInvite("i1", "member", "2026-11-19T12:00:00.000Z", 0, 5, false)

    @Test fun leavingNamesTheTeamAndSendsTheLeaveCommandThenClosesTheDetails() {
        val c = TeamConfirmations.leave(team)
        assertEquals(R.string.teams_confirm_leave_title_fmt, c.title)
        assertEquals("Design", c.subject)
        assertEquals(R.string.teams_confirm_leave_body, c.body)
        assertEquals(R.string.teams_confirm_leave_button, c.button)
        assertTrue(c.destructive)
        assertTrue(c.closesDetails)
        assertEquals(mapOf("action" to "leave", "teamId" to "t1", "generation" to 3L), c.command)
    }

    @Test fun deletingNamesTheTeamAndIsDestructive() {
        val c = TeamConfirmations.delete(team)
        assertEquals(R.string.teams_confirm_delete_title_fmt, c.title)
        assertEquals("Design", c.subject)
        assertEquals(R.string.teams_confirm_delete_body, c.body)
        assertEquals(R.string.teams_confirm_delete_button, c.button)
        assertTrue(c.destructive)
        assertTrue(c.closesDetails)
        assertEquals(mapOf("action" to "delete", "teamId" to "t1"), c.command)
    }

    @Test fun removingAMemberNamesThemAndKeepsTheDetailsOpen() {
        val c = TeamConfirmations.remove(team, member, "Mia")
        assertEquals(R.string.teams_confirm_remove_title_fmt, c.title)
        assertEquals("Mia", c.subject)
        assertEquals(R.string.teams_confirm_remove_body, c.body)
        assertEquals(R.string.teams_confirm_remove_button, c.button)
        assertTrue(c.destructive)
        assertFalse(c.closesDetails)
        assertEquals(mapOf("action" to "remove", "teamId" to "t1", "uid" to "u2", "generation" to 5L), c.command)
    }

    @Test fun aMemberBecomesAnAdminAndAnAdminBecomesAMemberWithoutAlarm() {
        val promote = TeamConfirmations.changeRole(team, member, "Mia")
        assertEquals(R.string.teams_confirm_promote_title_fmt, promote.title)
        assertEquals(R.string.teams_confirm_promote_body, promote.body)
        assertEquals(R.string.teams_confirm_promote_button, promote.button)
        assertFalse(promote.destructive)
        assertEquals(mapOf("action" to "role", "teamId" to "t1", "uid" to "u2", "generation" to 5L, "role" to "admin"), promote.command)

        val demote = TeamConfirmations.changeRole(team, admin, "Mia")
        assertEquals(R.string.teams_confirm_demote_title_fmt, demote.title)
        assertEquals(R.string.teams_confirm_demote_body, demote.body)
        assertEquals(R.string.teams_confirm_demote_button, demote.button)
        assertFalse(demote.destructive)
        assertEquals("member", demote.command["role"])
    }

    @Test fun revokingAnInviteHasATitleWithNoNameInIt() {
        val c = TeamConfirmations.revokeInvite(team, invite)
        assertEquals(R.string.teams_confirm_revoke_title, c.title)
        assertNull(c.subject)
        assertEquals(R.string.teams_confirm_revoke_body, c.body)
        assertEquals(R.string.teams_confirm_revoke_button, c.button)
        assertTrue(c.destructive)
        assertFalse(c.closesDetails)
        assertEquals(mapOf("action" to "revokeInvite", "teamId" to "t1", "inviteId" to "i1"), c.command)
    }

    @Test fun noConfirmationIsTitleOnlyOrSaysConfirm() {
        val all = listOf(
            TeamConfirmations.leave(team), TeamConfirmations.delete(team), TeamConfirmations.remove(team, member, "Mia"),
            TeamConfirmations.changeRole(team, member, "Mia"), TeamConfirmations.changeRole(team, admin, "Mia"), TeamConfirmations.revokeInvite(team, invite),
        )
        val retired = setOf(R.string.teams_confirm, R.string.teams_confirm_change, R.string.teams_confirm_leave, R.string.teams_confirm_delete)
        for (c in all) {
            assertTrue(c.title !in retired)
            assertTrue(c.button !in retired)
            assertTrue(c.body != 0)
        }
        // Each verb is its own: the button says what it does.
        assertEquals(all.size, all.map { it.button }.toSet().size)
    }
}
