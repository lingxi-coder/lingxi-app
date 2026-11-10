package com.lingxi.code.friends

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.LazyListState
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.outlined.Close
import androidx.compose.material.icons.outlined.ErrorOutline
import androidx.compose.material.icons.outlined.MoreVert
import androidx.compose.material.icons.outlined.PersonAdd
import androidx.compose.material.icons.outlined.PersonOutline
import androidx.compose.material.icons.outlined.Search
import androidx.compose.material3.Button
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalConfiguration
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import com.lingxi.code.R
import com.lingxi.code.teams.TeamsListLogic
import com.lingxi.code.teams.TeamsMessageState
import com.lingxi.code.teams.UnreadBadge
import com.lingxi.code.teams.listTimeText
import com.lingxi.code.teams.rememberNowMs
import com.lingxi.code.teams.rememberTeamsListStrings
import com.lingxi.code.theme.PersonAvatar

/**
 * The Friends root (docs/FRIENDS-CLIENTS.md D3, D4, D5): header, the name banner, the Requests section, the filter and the friends.
 * The phone's root, or the left pane when wide. [openChatUid] is the friend whose chat is open beside it (wide), highlighted and read.
 * A change of [scrollToRequests] brings the Requests section into view (the `friend/requests` link and push).
 */
@Composable
internal fun FriendsListScreen(
    state: FriendsState,
    uid: String,
    openChatUid: String,
    listState: LazyListState,
    scrollToRequests: Int,
    onOpenChat: (String) -> Unit,
    onAdd: () -> Unit,
    onProfile: () -> Unit,
    onBlocked: () -> Unit,
    onRetry: () -> Unit,
    onRetryProfile: () -> Unit,
    onAccept: (String) -> Unit,
    onDecline: (String) -> Unit,
    onCancel: (String) -> Unit,
    modifier: Modifier = Modifier,
) {
    val locale = LocalConfiguration.current.locales[0]
    val collator = remember(locale) { FriendsLogic.friendCollator(locale) }
    val strings = rememberTeamsListStrings()
    val now = rememberNowMs()
    var searching by rememberSaveable { mutableStateOf(false) }
    var query by rememberSaveable { mutableStateOf("") }
    var more by remember { mutableStateOf(false) }
    var expanded by rememberSaveable { mutableStateOf("") }

    val arranged = remember(state.cards, state.names, collator) { state.resolvedCards().sortedWith(FriendsLogic.compareCards(collator)) }
    val incoming = arranged.filter { it.state == FriendState.Incoming }
    val outgoing = arranged.filter { it.state == FriendState.Outgoing }
    val friends = arranged.filter { it.state == FriendState.Friends }
    val shown = remember(friends, query) { friends.filter { FriendsLogic.matchesFilter(it, query) } }
    val showNameBanner = FriendsLogic.needsName(state.profile)
    val showProfileError = state.profile == null && state.profileOp.error != null
    val showLoadError = state.cardsFailed && state.cardsLoaded
    val leading = listOf(showNameBanner, showProfileError, showLoadError).count { it }

    LaunchedEffect(scrollToRequests) { if (scrollToRequests > 0) listState.animateScrollToItem(leading) }

    Column(modifier.fillMaxSize().testTag(FriendsTags.LIST)) {
        Row(Modifier.fillMaxWidth().height(56.dp).padding(horizontal = 8.dp), verticalAlignment = Alignment.CenterVertically) {
            if (searching) {
                val focus = remember { FocusRequester() }
                LaunchedEffect(Unit) { focus.requestFocus() }
                OutlinedTextField(
                    value = query, onValueChange = { query = it }, singleLine = true,
                    placeholder = { Text(stringResource(R.string.friends_filter_placeholder)) },
                    keyboardOptions = KeyboardOptions(imeAction = ImeAction.Search), keyboardActions = KeyboardActions(onSearch = { }),
                    modifier = Modifier.weight(1f).focusRequester(focus).testTag(FriendsTags.FILTER_FIELD),
                )
                IconButton(onClick = { searching = false; query = "" }) { Icon(Icons.Outlined.Close, stringResource(R.string.common_close)) }
            } else {
                Text(stringResource(R.string.friends_title), Modifier.weight(1f).padding(start = 8.dp), style = MaterialTheme.typography.titleLarge, fontWeight = FontWeight.SemiBold, maxLines = 1, overflow = TextOverflow.Ellipsis)
                IconButton(onClick = { searching = true }, modifier = Modifier.testTag(FriendsTags.SEARCH)) { Icon(Icons.Outlined.Search, stringResource(R.string.friends_filter_placeholder)) }
                IconButton(onClick = onAdd, modifier = Modifier.testTag(FriendsTags.ADD)) { Icon(Icons.Outlined.PersonAdd, stringResource(R.string.friends_add)) }
                Box {
                    IconButton(onClick = { more = true }, modifier = Modifier.testTag(FriendsTags.MORE)) { Icon(Icons.Outlined.MoreVert, stringResource(R.string.composer_more)) }
                    DropdownMenu(expanded = more, onDismissRequest = { more = false }) {
                        DropdownMenuItem(text = { Text(stringResource(R.string.friends_profile_title)) }, onClick = { more = false; onProfile() })
                        DropdownMenuItem(text = { Text(stringResource(R.string.friends_blocked_title)) }, onClick = { more = false; onBlocked() })
                    }
                }
            }
        }

        Box(Modifier.weight(1f).fillMaxWidth()) {
            when {
                !state.cardsLoaded && state.cardsFailed -> TeamsMessageState(
                    stringResource(R.string.friends_load_error), Modifier.testTag(FriendsTags.LOAD_FAILED), icon = Icons.Outlined.ErrorOutline,
                    action = { Button(onClick = onRetry) { Text(stringResource(R.string.common_retry)) } },
                )
                !state.cardsLoaded -> Column(Modifier.fillMaxSize().testTag(FriendsTags.LOADING), verticalArrangement = Arrangement.Center, horizontalAlignment = Alignment.CenterHorizontally) {
                    CircularProgressIndicator(Modifier.size(28.dp))
                    Spacer(Modifier.height(12.dp))
                    Text(stringResource(R.string.friends_loading), style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
                }
                arranged.isEmpty() -> TeamsMessageState(
                    stringResource(R.string.friends_empty_title), Modifier.testTag(FriendsTags.EMPTY), icon = Icons.Outlined.PersonOutline, body = stringResource(R.string.friends_empty_body),
                    action = { Button(onClick = onAdd) { Text(stringResource(R.string.friends_add)) } },
                )
                else -> LazyColumn(Modifier.fillMaxSize(), state = listState) {
                    if (showLoadError) item(key = "load-error") {
                        Surface(Modifier.fillMaxWidth(), color = MaterialTheme.colorScheme.errorContainer) {
                            Row(Modifier.padding(horizontal = 16.dp, vertical = 8.dp), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                                Text(stringResource(R.string.friends_load_error), Modifier.weight(1f), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onErrorContainer)
                                TextButton(onClick = onRetry) { Text(stringResource(R.string.common_retry)) }
                            }
                        }
                    }
                    if (showNameBanner) item(key = "name-banner") {
                        Surface(Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 8.dp).testTag(FriendsTags.NAME_BANNER), shape = MaterialTheme.shapes.medium, color = MaterialTheme.colorScheme.secondaryContainer) {
                            Row(Modifier.padding(start = 16.dp, end = 8.dp, top = 8.dp, bottom = 8.dp), verticalAlignment = Alignment.CenterVertically) {
                                Text(stringResource(R.string.friends_name_prompt), Modifier.weight(1f), style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSecondaryContainer)
                                TextButton(onClick = onProfile) { Text(stringResource(R.string.friends_name_prompt_action)) }
                            }
                        }
                    }
                    if (showProfileError) item(key = "profile-error") {
                        Row(Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 4.dp).testTag(FriendsTags.PROFILE_ERROR), verticalAlignment = Alignment.CenterVertically) {
                            FriendsErrorText(state.profileOp.error, Modifier.weight(1f))
                            TextButton(onClick = onRetryProfile) { Text(stringResource(R.string.common_retry)) }
                        }
                    }
                    if (incoming.isNotEmpty() || outgoing.isNotEmpty()) {
                        item(key = "requests") {
                            Text(stringResource(R.string.friends_section_requests), Modifier.padding(start = 16.dp, end = 16.dp, top = 12.dp, bottom = 4.dp).testTag(FriendsTags.REQUESTS), style = MaterialTheme.typography.titleSmall)
                        }
                        requestGroup("received", R.string.friends_requests_received, incoming, expanded, { expanded = it }, state, onAccept, onDecline, onCancel)
                        requestGroup("sent", R.string.friends_requests_sent, outgoing, expanded, { expanded = it }, state, onAccept, onDecline, onCancel)
                        item(key = "requests-end") { HorizontalDivider(Modifier.padding(top = 8.dp)) }
                    }
                    when {
                        friends.isNotEmpty() && shown.isEmpty() -> item(key = "filter-empty") {
                            Text(stringResource(R.string.friends_filter_empty), Modifier.fillMaxWidth().padding(24.dp).testTag(FriendsTags.FILTER_EMPTY), style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
                        }
                        else -> items(shown, key = { "friend-${it.uid}" }) { card ->
                            val unread = FriendsLogic.unreadShown(card, state.openChatUid)
                            FriendRow(
                                card, TeamsListLogic.pointerPreview(card.lastMessage, uid, strings), listTimeText(card.lastAtMs, now), unread,
                                selected = card.uid == openChatUid, onClick = { onOpenChat(card.uid) },
                            )
                        }
                    }
                }
            }
        }
    }
}

/** One group of the Requests section: a sub-heading, at most [FriendsLogic.SECTION_ROWS] rows and a final "Show all (n)" row (D4). */
private fun androidx.compose.foundation.lazy.LazyListScope.requestGroup(
    id: String, @androidx.annotation.StringRes title: Int, cards: List<FriendCard>, expanded: String, expand: (String) -> Unit, state: FriendsState,
    onAccept: (String) -> Unit, onDecline: (String) -> Unit, onCancel: (String) -> Unit,
) {
    if (cards.isEmpty()) return
    val all = id in expanded.split(',')
    item(key = "requests-$id") {
        Text(stringResource(title), Modifier.padding(horizontal = 16.dp, vertical = 4.dp), style = MaterialTheme.typography.labelLarge, color = MaterialTheme.colorScheme.onSurfaceVariant)
    }
    items(if (all) cards else cards.take(FriendsLogic.SECTION_ROWS), key = { "request-${it.uid}" }) { card ->
        RequestRow(
            card, state.rowOps[card.uid],
            onAccept = { onAccept(card.uid) }, onDecline = { onDecline(card.uid) }, onCancel = { onCancel(card.uid) },
        )
    }
    if (!all && cards.size > FriendsLogic.SECTION_ROWS) item(key = "requests-$id-all") {
        TextButton(onClick = { expand(listOf(expanded, id).filter { it.isNotEmpty() }.joinToString(",")) }, Modifier.padding(horizontal = 8.dp)) {
            Text(stringResource(R.string.friends_show_all_fmt, cards.size))
        }
    }
}

@Composable
private fun FriendRow(card: FriendCard, preview: String, time: String, unread: Int, selected: Boolean, onClick: () -> Unit) {
    Row(
        Modifier.fillMaxWidth()
            .background(if (selected) MaterialTheme.colorScheme.surfaceVariant else Color.Transparent)
            .clickable(onClick = onClick)
            .padding(horizontal = 16.dp, vertical = 10.dp).testTag(FriendsTags.row(card.uid)),
        verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        PersonAvatar(card.uid, card.displayName, 40.dp)
        Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                Text(card.displayName.ifEmpty { stringResource(R.string.teams_member) }, Modifier.weight(1f), style = MaterialTheme.typography.bodyLarge, fontWeight = if (unread > 0) FontWeight.Bold else FontWeight.Medium, maxLines = 1, overflow = TextOverflow.Ellipsis)
                if (time.isNotEmpty()) Text(time, Modifier.padding(start = 8.dp), style = MaterialTheme.typography.labelSmall, color = if (unread > 0) MaterialTheme.colorScheme.primary else MaterialTheme.colorScheme.onSurfaceVariant)
            }
            Row(verticalAlignment = Alignment.CenterVertically) {
                Text(
                    preview, Modifier.weight(1f), style = MaterialTheme.typography.bodyMedium, fontWeight = if (unread > 0) FontWeight.Bold else FontWeight.Normal,
                    color = if (unread > 0) MaterialTheme.colorScheme.onSurface else MaterialTheme.colorScheme.onSurfaceVariant, maxLines = 1, overflow = TextOverflow.Ellipsis,
                )
                if (unread > 0) { Spacer(Modifier.width(8.dp)); UnreadBadge(unread) }
            }
        }
    }
}

/** A received or sent request (D4). The note is plain text, two lines at most; the row is disabled while one of its actions is in flight. */
@Composable
private fun RequestRow(card: FriendCard, op: FriendsOp?, onAccept: () -> Unit, onDecline: () -> Unit, onCancel: () -> Unit) {
    val busy = op?.busy == true
    Row(Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 8.dp).testTag(FriendsTags.row(card.uid)), horizontalArrangement = Arrangement.spacedBy(12.dp)) {
        PersonAvatar(card.uid, card.displayName, 40.dp)
        Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
            Text(card.displayName.ifEmpty { stringResource(R.string.teams_member) }, style = MaterialTheme.typography.bodyLarge, fontWeight = FontWeight.Medium, maxLines = 1, overflow = TextOverflow.Ellipsis)
            card.handle?.let { Text("@$it", style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant, maxLines = 1, overflow = TextOverflow.Ellipsis) }
            card.note?.let { Text(it, style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant, maxLines = 2, overflow = TextOverflow.Ellipsis) }
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                if (card.state == FriendState.Incoming) {
                    Button(onClick = onAccept, enabled = !busy, modifier = Modifier.testTag(FriendsTags.accept(card.uid))) { Text(stringResource(R.string.friends_request_accept)) }
                    TextButton(onClick = onDecline, enabled = !busy, modifier = Modifier.testTag(FriendsTags.decline(card.uid))) { Text(stringResource(R.string.friends_request_decline)) }
                } else {
                    Text(stringResource(R.string.friends_request_sent), style = MaterialTheme.typography.labelLarge, color = MaterialTheme.colorScheme.onSurfaceVariant)
                    TextButton(onClick = onCancel, enabled = !busy, modifier = Modifier.testTag(FriendsTags.cancel(card.uid))) { Text(stringResource(R.string.friends_request_cancel)) }
                }
            }
            op?.error?.let { FriendsErrorText(it) }
        }
    }
}
