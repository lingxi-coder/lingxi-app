package com.lingxi.code.friends

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material3.Button
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.rememberModalBottomSheetState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalConfiguration
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import com.lingxi.code.R
import com.lingxi.code.theme.PersonAvatar

/**
 * Invite friends (docs/FRIENDS-CLIENTS.md D26): a bottom sheet over the Teams tab, titled with the team, over the app-wide Friends workspace's
 * cards (no listener of its own). It lists the friends who are not in [memberUids], in the Friends order, narrowed by the filter; requests never
 * appear. Invite sends one offer through [onInvite] and the row stays disabled while it runs; once it went out the row reads "Invited" until the
 * sheet closes, and a failure shows under the row. A friend whose invite answered "already a member" leaves the list at once, as the refreshed
 * member list will make them. [onDismiss] closes the sheet, and the caller tells the workspace ([FriendsWorkspace.closeInvites]).
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
internal fun InviteFriendsSheet(
    teamName: String,
    state: FriendsState,
    memberUids: Set<String>,
    onInvite: (String) -> Unit,
    onDismiss: () -> Unit,
) {
    val locale = LocalConfiguration.current.locales[0]
    val collator = remember(locale) { FriendsLogic.friendCollator(locale) }
    var query by rememberSaveable { mutableStateOf("") }
    val joined = remember(state.invites) { state.invites.filterValues { it.error == FriendsError.AlreadyMember }.keys }
    val cards = remember(state.cards, state.names) { state.resolvedCards() }
    val everyone = remember(cards, memberUids, joined, collator) { FriendsLogic.invitePicker(cards, memberUids + joined, "", collator) }
    val people = remember(everyone, query) { everyone.filter { FriendsLogic.matchesFilter(it, query) } }

    ModalBottomSheet(onDismissRequest = onDismiss, sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true), modifier = Modifier.testTag(FriendsTags.INVITE_SHEET)) {
        Column(Modifier.fillMaxWidth().imePadding()) {
            Row(Modifier.fillMaxWidth().padding(start = 24.dp, end = 12.dp), verticalAlignment = Alignment.CenterVertically) {
                Text(stringResource(R.string.friends_invite_title_fmt, teamName), Modifier.weight(1f), style = MaterialTheme.typography.titleLarge)
                TextButton(onClick = onDismiss) { Text(stringResource(R.string.common_close)) }
            }
            OutlinedTextField(
                value = query, onValueChange = { query = it }, singleLine = true,
                placeholder = { Text(stringResource(R.string.friends_filter_placeholder)) },
                keyboardOptions = KeyboardOptions(imeAction = ImeAction.Search), keyboardActions = KeyboardActions(onSearch = { }),
                modifier = Modifier.fillMaxWidth().padding(horizontal = 24.dp, vertical = 8.dp).testTag(FriendsTags.INVITE_FILTER),
            )
            when {
                people.isNotEmpty() -> LazyColumn(Modifier.weight(1f, fill = false).fillMaxWidth(), verticalArrangement = Arrangement.spacedBy(4.dp)) {
                    items(people, key = { "invite-${it.uid}" }) { card -> InvitePersonRow(card, state.invites[card.uid], onInvite = { onInvite(card.uid) }) }
                }
                everyone.isEmpty() -> Text(
                    stringResource(R.string.friends_invite_empty), Modifier.fillMaxWidth().padding(horizontal = 24.dp, vertical = 16.dp).testTag(FriendsTags.INVITE_EMPTY),
                    style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
                else -> Text(
                    stringResource(R.string.friends_filter_empty), Modifier.fillMaxWidth().padding(horizontal = 24.dp, vertical = 16.dp).testTag(FriendsTags.INVITE_FILTER_EMPTY),
                    style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
            Text(
                stringResource(R.string.friends_invite_disclosure), Modifier.fillMaxWidth().padding(horizontal = 24.dp, vertical = 12.dp).testTag(FriendsTags.INVITE_DISCLOSURE),
                style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
        }
    }
}

/** A friend of the picker: avatar, name, username if any and Invite, which reads "Invited" (disabled) once the offer went out. */
@Composable
private fun InvitePersonRow(card: FriendCard, row: InviteRow?, onInvite: () -> Unit) {
    Row(Modifier.fillMaxWidth().padding(horizontal = 24.dp, vertical = 6.dp).testTag(FriendsTags.invite(card.uid)), verticalAlignment = Alignment.Top, horizontalArrangement = Arrangement.spacedBy(12.dp)) {
        PersonAvatar(card.uid, card.displayName, 40.dp)
        Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
            Text(card.displayName.ifEmpty { stringResource(R.string.teams_member) }, style = MaterialTheme.typography.bodyLarge, fontWeight = FontWeight.Medium, maxLines = 1, overflow = TextOverflow.Ellipsis)
            card.handle?.let { Text("@$it", style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant, maxLines = 1, overflow = TextOverflow.Ellipsis) }
            row?.error?.let { FriendsErrorText(it) }
        }
        val sent = row?.sentTo != null
        Button(onClick = onInvite, enabled = row?.busy != true && !sent, modifier = Modifier.testTag(FriendsTags.inviteSend(card.uid))) {
            Text(stringResource(if (sent) R.string.friends_invite_sent else R.string.friends_invite_send))
        }
    }
}
