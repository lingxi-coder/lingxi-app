package com.lingxi.code.teams

import androidx.annotation.StringRes
import com.lingxi.code.R

/**
 * A team-management confirmation (docs/TEAMS-UI-POLISH.md U8): the title names the thing ([subject] fills its `%1$s`, null for a title with no
 * name in it), the body says what changes, the button is a verb, and a [destructive] one is drawn in the error colour. [command] is what the
 * confirm button sends. Every one has a Cancel; the old "Confirm this membership change?" and the title-only "Leave this team?" are gone.
 */
internal data class TeamConfirmation(
    @StringRes val title: Int, val subject: String?, @StringRes val body: Int, @StringRes val button: Int,
    val destructive: Boolean, val command: Map<String, Any>,
) {
    /** Leaving or deleting a team closes its details page once the command is done. */
    val closesDetails: Boolean get() = command["action"] in setOf("leave", "delete")
}

internal object TeamConfirmations {
    fun leave(team: TeamSummary) = TeamConfirmation(
        R.string.teams_confirm_leave_title_fmt, team.name, R.string.teams_confirm_leave_body, R.string.teams_confirm_leave_button, true,
        mapOf("action" to "leave", "teamId" to team.id, "generation" to team.generation),
    )

    fun delete(team: TeamSummary) = TeamConfirmation(
        R.string.teams_confirm_delete_title_fmt, team.name, R.string.teams_confirm_delete_body, R.string.teams_confirm_delete_button, true,
        mapOf("action" to "delete", "teamId" to team.id),
    )

    /** [name] is how the member is shown (the display name, or the word "Member" when there is none). */
    fun remove(team: TeamSummary, member: TeamMember, name: String) = TeamConfirmation(
        R.string.teams_confirm_remove_title_fmt, name, R.string.teams_confirm_remove_body, R.string.teams_confirm_remove_button, true,
        mapOf("action" to "remove", "teamId" to team.id, "uid" to member.uid, "generation" to member.generation),
    )

    /** An admin becomes a member and a member becomes an admin; neither is destructive. */
    fun changeRole(team: TeamSummary, member: TeamMember, name: String): TeamConfirmation {
        val promote = member.role != "admin"
        return TeamConfirmation(
            if (promote) R.string.teams_confirm_promote_title_fmt else R.string.teams_confirm_demote_title_fmt, name,
            if (promote) R.string.teams_confirm_promote_body else R.string.teams_confirm_demote_body,
            if (promote) R.string.teams_confirm_promote_button else R.string.teams_confirm_demote_button, false,
            mapOf("action" to "role", "teamId" to team.id, "uid" to member.uid, "generation" to member.generation, "role" to if (promote) "admin" else "member"),
        )
    }

    /**
     * Handing the team over: the member becomes its owner and the sender stays an admin. The title names the member, the body says what stays,
     * and the button is the verb. It asks once and, like the other role changes, is not drawn as destructive.
     */
    fun makeOwner(team: TeamSummary, member: TeamMember, name: String) = TeamConfirmation(
        R.string.teams_confirm_make_owner_title_fmt, name, R.string.teams_confirm_make_owner, R.string.teams_make_owner, false,
        mapOf("action" to "handOver", "teamId" to team.id, "uid" to member.uid, "generation" to member.generation),
    )

    fun revokeInvite(team: TeamSummary, invite: TeamInvite) = TeamConfirmation(
        R.string.teams_confirm_revoke_title, null, R.string.teams_confirm_revoke_body, R.string.teams_confirm_revoke_button, true,
        mapOf("action" to "revokeInvite", "teamId" to team.id, "inviteId" to invite.id),
    )
}
