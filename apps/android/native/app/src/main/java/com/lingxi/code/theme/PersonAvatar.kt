package com.lingxi.code.theme

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.semantics.clearAndSetSemantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.lingxi.code.teams.TeamBotCard
import com.lingxi.code.teams.TeamIdentity
import com.lingxi.code.teams.TeamMember
import com.lingxi.code.teams.TeamMessageConversation
import com.lingxi.code.teams.TeamsListLogic
import com.lingxi.code.teams.TeamsListStrings

/**
 * Avatars of the Teams surfaces (SPEC-mobile-team-bot-ui section 7). Every one is decorative and hidden from accessibility:
 * the name beside it carries the meaning.
 */
private val AvatarWhite = Color.White

/** A person: initials in white on a colour taken from the uid. */
@Composable
fun PersonAvatar(id: String, name: String, size: Dp = 32.dp, modifier: Modifier = Modifier) {
    Box(
        modifier.size(size).clip(CircleShape).background(Color(AvatarIdentity.avatarColorArgb(id))).clearAndSetSemantics { },
        contentAlignment = Alignment.Center,
    ) {
        Text(AvatarIdentity.initials(name), color = AvatarWhite, fontSize = (size.value * 0.4f).sp, fontWeight = FontWeight.SemiBold, maxLines = 1)
    }
}

/** A Bot: the shipped art, chosen by its `botId` (or, before its card has loaded, the last segment of its key). */
@Composable
fun BotAvatar(botIdOrKey: String, size: Dp = 32.dp, modifier: Modifier = Modifier) {
    Box(modifier.size(size).clearAndSetSemantics { }) { AgentAvatar(AvatarIdentity.teamBotAvatarId(botIdOrKey), Modifier.size(size)) }
}

/** A channel: a rounded square with a `#` glyph, no colour from the id. */
@Composable
fun ChannelAvatar(size: Dp = 40.dp, modifier: Modifier = Modifier) {
    Box(
        modifier.size(size).clip(RoundedCornerShape(size * 0.25f)).background(MaterialTheme.colorScheme.surfaceVariant).clearAndSetSemantics { },
        contentAlignment = Alignment.Center,
    ) {
        Text("#", color = MaterialTheme.colorScheme.onSurfaceVariant, fontSize = (size.value * 0.5f).sp, fontWeight = FontWeight.SemiBold)
    }
}

/** A team: the first letter of its name on a colour taken from the team id. */
@Composable
fun TeamTile(teamId: String, name: String, size: Dp = 36.dp, modifier: Modifier = Modifier) {
    Box(
        modifier.size(size).clip(RoundedCornerShape(size * 0.25f)).background(Color(AvatarIdentity.avatarColorArgb(teamId))).clearAndSetSemantics { },
        contentAlignment = Alignment.Center,
    ) {
        Text(AvatarIdentity.teamInitial(name), color = AvatarWhite, fontSize = (size.value * 0.45f).sp, fontWeight = FontWeight.SemiBold, maxLines = 1)
    }
}

@Composable
internal fun IdentityAvatar(identity: TeamIdentity, size: Dp = 32.dp, modifier: Modifier = Modifier) {
    if (identity.isBot) BotAvatar(identity.id, size, modifier) else PersonAvatar(identity.id, identity.name, size, modifier)
}

/** Up to two other members, overlapped at 0.7 x the lead size inside the lead's own footprint, with a small "+N" for the rest. */
@Composable
internal fun AvatarStack(identities: List<TeamIdentity>, size: Dp, modifier: Modifier = Modifier) {
    val shown = identities.take(2)
    val small = size * 0.7f
    Box(modifier.size(size).clearAndSetSemantics { }) {
        shown.forEachIndexed { index, identity ->
            IdentityAvatar(identity, small, Modifier.offset(x = (size - small) * index, y = (size - small) * index))
        }
        val more = identities.size - shown.size
        if (more > 0) {
            Box(
                Modifier.align(Alignment.BottomEnd).clip(CircleShape).background(MaterialTheme.colorScheme.surface).padding(1.dp).clip(CircleShape).background(MaterialTheme.colorScheme.surfaceVariant),
                contentAlignment = Alignment.Center,
            ) {
                Text("+$more", color = MaterialTheme.colorScheme.onSurfaceVariant, fontSize = (size.value * 0.26f).sp, fontWeight = FontWeight.SemiBold, modifier = Modifier.padding(horizontal = 3.dp))
            }
        }
    }
}

/**
 * The lead of a conversation row or header (section 7.4): `#` for a channel, the other person for a dm, the Bot for a Bot DM,
 * a stack of the other members for a group.
 */
@Composable
internal fun ConversationLead(
    conversation: TeamMessageConversation, members: List<TeamMember>, bots: List<TeamBotCard>, uid: String, strings: TeamsListStrings,
    size: Dp, label: String, modifier: Modifier = Modifier,
) {
    val participants = TeamsListLogic.conversationParticipants(conversation, members, bots, uid, strings)
    when (conversation.kind) {
        "channel" -> ChannelAvatar(size, modifier)
        "bot_dm" -> {
            val bot = participants.firstOrNull { it.isBot }
            if (bot != null) IdentityAvatar(bot, size, modifier) else PersonAvatar(conversation.id, label, size, modifier)
        }
        "dm" -> {
            val other = participants.firstOrNull { !it.self && !it.isBot } ?: participants.firstOrNull { !it.isBot }
            if (other != null) IdentityAvatar(other, size, modifier) else PersonAvatar(conversation.id, label, size, modifier)
        }
        else -> {
            val others = participants.filter { !it.self }.ifEmpty { participants }
            if (others.isEmpty()) PersonAvatar(conversation.id, label, size, modifier) else AvatarStack(others, size, modifier)
        }
    }
}
