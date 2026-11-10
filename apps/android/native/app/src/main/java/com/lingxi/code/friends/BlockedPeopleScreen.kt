package com.lingxi.code.friends

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.outlined.Block
import androidx.compose.material.icons.outlined.ErrorOutline
import androidx.compose.material3.Button
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import com.lingxi.code.R
import com.lingxi.code.teams.TeamsMessageState
import com.lingxi.code.theme.PersonAvatar

/**
 * Blocked people (docs/FRIENDS-CLIENTS.md D12): the rows of `users/{uid}/blocks`, newest first, each with Unblock (no confirmation).
 * The name is the one the backend wrote when the block was made; nothing is remembered on the device.
 */
@Composable
internal fun BlockedPeopleScreen(
    state: FriendsState, showBack: Boolean, onClose: () -> Unit, onRetry: () -> Unit, onUnblock: (String) -> Unit, modifier: Modifier = Modifier,
) {
    val blocks = remember(state.blocks) { state.blocks.sortedWith(FriendsLogic.compareBlocks) }
    Column(modifier.fillMaxSize().testTag(FriendsTags.BLOCKED_SCREEN)) {
        FriendsPageHeader(stringResource(R.string.friends_blocked_title), showBack, onClose)
        when {
            !state.blocksLoaded && state.blocksFailed -> TeamsMessageState(
                stringResource(R.string.friends_load_error), Modifier.testTag(FriendsTags.LOAD_FAILED), icon = Icons.Outlined.ErrorOutline,
                action = { Button(onClick = onRetry) { Text(stringResource(R.string.common_retry)) } },
            )
            !state.blocksLoaded -> Box(Modifier.fillMaxSize().testTag(FriendsTags.LOADING), contentAlignment = Alignment.Center) { CircularProgressIndicator() }
            blocks.isEmpty() -> TeamsMessageState(stringResource(R.string.friends_blocked_empty), Modifier.testTag(FriendsTags.BLOCKED_EMPTY), icon = Icons.Outlined.Block)
            else -> LazyColumn(Modifier.fillMaxSize()) {
                items(blocks, key = { "block-${it.uid}" }) { entry ->
                    val op = state.rowOps[entry.uid]
                    Row(Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 8.dp), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                        val name = entry.displayName.ifEmpty { stringResource(R.string.teams_member) }
                        PersonAvatar(entry.uid, name, 40.dp)
                        Column(Modifier.weight(1f)) {
                            Text(name, style = MaterialTheme.typography.bodyLarge, fontWeight = FontWeight.Medium, maxLines = 1, overflow = TextOverflow.Ellipsis)
                            entry.handle?.let { Text("@$it", style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant, maxLines = 1, overflow = TextOverflow.Ellipsis) }
                            op?.error?.let { FriendsErrorText(it) }
                        }
                        TextButton(onClick = { onUnblock(entry.uid) }, enabled = op?.busy != true, modifier = Modifier.testTag(FriendsTags.unblock(entry.uid))) {
                            Text(stringResource(R.string.friends_unblock))
                        }
                    }
                }
                item(key = "footer") {
                    Text(stringResource(R.string.friends_unblock_footer), Modifier.padding(16.dp), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                }
            }
        }
    }
}
