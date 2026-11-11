package com.lingxi.code.friends

import android.content.Intent
import androidx.annotation.StringRes
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardCapitalization
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import com.lingxi.code.R
import com.lingxi.code.teams.TeamsSheet
import com.lingxi.code.teams.rememberCopyToClipboard
import com.lingxi.code.theme.PersonAvatar
import kotlinx.coroutines.delay

/**
 * Add friend (docs/FRIENDS-CLIENTS.md D6): my friend link, then one field that finds somebody by username, email address or
 * another person's friend link. Asking for the profile on every open also creates the first friend code (D2).
 */
@Composable
internal fun AddFriendScreen(
    state: FriendsState,
    lookup: LookupState,
    showBack: Boolean,
    onClose: () -> Unit,
    onOpenChat: (String) -> Unit,
    onLoadProfile: () -> Unit,
    onRotateCode: () -> Unit,
    onRevokeCode: () -> Unit,
    onLookup: (FriendProof) -> Unit,
    onConfirm: (note: String) -> Unit,
    onClear: () -> Unit,
    modifier: Modifier = Modifier,
) {
    LaunchedEffect(Unit) { onLoadProfile() }
    // Leaving the screen drops the card, the code and any request id (D8, D18).
    DisposableEffect(Unit) { onDispose { onClear() } }
    var query by rememberSaveable { mutableStateOf("") }
    // A friendship made here (redeem, accept, a mutual request) opens the chat (D7).
    LaunchedEffect(lookup.outcome) {
        val outcome = lookup.outcome
        if (outcome is FriendOutcome.NowFriends) { onClear(); onOpenChat(outcome.uid) }
    }

    Column(modifier.fillMaxSize().testTag(FriendsTags.ADD_SCREEN)) {
        FriendsPageHeader(stringResource(R.string.friends_add), showBack, onClose)
        Column(
            Modifier.weight(1f).verticalScroll(rememberScrollState()).padding(horizontal = 16.dp).padding(bottom = 24.dp),
            verticalArrangement = Arrangement.spacedBy(16.dp),
        ) {
            MyFriendLink(state, onRetry = onLoadProfile, onRotate = onRotateCode, onRevoke = onRevokeCode)
            FindSomeone(lookup = lookup, query = query, onQuery = { query = it; onClear() }, onLookup = onLookup, onConfirm = onConfirm, onOpenChat = onOpenChat)
        }
    }
}

/** *My friend link* (D6): copy, share, reset and turn off; with no link, create one. The disclosure always sits under the card. */
@Composable
private fun MyFriendLink(state: FriendsState, onRetry: () -> Unit, onRotate: () -> Unit, onRevoke: () -> Unit) {
    val profile = state.profile
    val copy = rememberCopyToClipboard("LingXi friend link")
    val context = LocalContext.current
    val shareMessage = stringResource(R.string.friends_link_share_message)
    val shareTitle = stringResource(R.string.friends_link_share)
    var copied by remember { mutableStateOf(false) }
    var confirm by remember { mutableStateOf("") }
    LaunchedEffect(copied) { if (copied) { delay(2_000); copied = false } }

    Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
        Surface(Modifier.fillMaxWidth().testTag(FriendsTags.LINK_CARD), shape = MaterialTheme.shapes.medium, tonalElevation = 1.dp) {
            Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
                Text(stringResource(R.string.friends_link_title), style = MaterialTheme.typography.titleSmall)
                val code = profile?.friendCode
                when {
                    profile == null && state.profileOp.error == null -> CircularProgressIndicator(Modifier.size(24.dp), strokeWidth = 2.dp)
                    profile == null -> Row(verticalAlignment = Alignment.CenterVertically) {
                        FriendsErrorText(state.profileOp.error!!, Modifier.weight(1f).testTag(FriendsTags.PROFILE_ERROR))
                        TextButton(onClick = onRetry) { Text(stringResource(R.string.common_retry)) }
                    }
                    code == null -> {
                        Text(stringResource(R.string.friends_link_is_off), style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
                        Button(onClick = onRotate, enabled = !state.linkOp.busy, modifier = Modifier.testTag(FriendsTags.LINK_CREATE)) { Text(stringResource(R.string.friends_link_create)) }
                    }
                    else -> {
                        SelectionContainer { Text(code.link, style = MaterialTheme.typography.bodyMedium, maxLines = 1, overflow = TextOverflow.MiddleEllipsis) }
                        Row(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalAlignment = Alignment.CenterVertically) {
                            OutlinedButton(onClick = { copy(code.link); copied = true }, modifier = Modifier.testTag(FriendsTags.LINK_COPY)) { Text(stringResource(R.string.friends_link_copy)) }
                            OutlinedButton(
                                onClick = {
                                    val send = Intent(Intent.ACTION_SEND).setType("text/plain").putExtra(Intent.EXTRA_TEXT, "$shareMessage ${code.link}")
                                    runCatching { context.startActivity(Intent.createChooser(send, shareTitle)) }
                                },
                                modifier = Modifier.testTag(FriendsTags.LINK_SHARE),
                            ) { Text(shareTitle) }
                        }
                        Row(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalAlignment = Alignment.CenterVertically) {
                            TextButton(onClick = { confirm = "reset" }, enabled = !state.linkOp.busy, modifier = Modifier.testTag(FriendsTags.LINK_RESET)) { Text(stringResource(R.string.friends_link_reset)) }
                            TextButton(onClick = { confirm = "off" }, enabled = !state.linkOp.busy, modifier = Modifier.testTag(FriendsTags.LINK_OFF)) { Text(stringResource(R.string.friends_link_off), color = MaterialTheme.colorScheme.error) }
                        }
                        if (copied) Text(stringResource(R.string.friends_link_copied), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.primary)
                    }
                }
                state.linkOp.error?.let { FriendsErrorText(it) }
            }
        }
        Text(stringResource(R.string.friends_link_disclosure), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
    }

    if (confirm.isNotEmpty()) {
        val resetting = confirm == "reset"
        AlertDialog(
            onDismissRequest = { confirm = "" },
            text = { Text(stringResource(if (resetting) R.string.friends_link_reset_confirm else R.string.friends_link_off_confirm)) },
            confirmButton = {
                TextButton(onClick = { if (resetting) onRotate() else onRevoke(); confirm = "" }) {
                    Text(stringResource(if (resetting) R.string.friends_link_reset else R.string.friends_link_off), color = MaterialTheme.colorScheme.error)
                }
            },
            dismissButton = { TextButton(onClick = { confirm = "" }) { Text(stringResource(R.string.common_cancel)) } },
        )
    }
}

/**
 * *Find someone* (D6, D7): one field for a username, an email address or a friend link. Return or the button looks up; nothing is looked up
 * while typing (lookups are rate limited), and editing the field drops the card.
 */
@Composable
private fun FindSomeone(
    lookup: LookupState, query: String, onQuery: (String) -> Unit, onLookup: (FriendProof) -> Unit, onConfirm: (String) -> Unit, onOpenChat: (String) -> Unit,
) {
    val proof = FriendsLogic.classifyQuery(query)
    val invalid = query.isNotBlank() && proof == null
    val busy = lookup.looking || lookup.confirming
    Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
        Text(stringResource(R.string.friends_search_title), style = MaterialTheme.typography.titleSmall)
        OutlinedTextField(
            value = query, onValueChange = onQuery, singleLine = true, enabled = !lookup.confirming, isError = invalid,
            placeholder = { Text(stringResource(R.string.friends_search_placeholder)) },
            supportingText = if (invalid) ({ Text(stringResource(R.string.friends_search_hint)) }) else null,
            keyboardOptions = KeyboardOptions(capitalization = KeyboardCapitalization.None, autoCorrectEnabled = false, keyboardType = KeyboardType.Uri, imeAction = ImeAction.Search),
            keyboardActions = KeyboardActions(onSearch = { if (proof != null && !busy) onLookup(proof) }),
            modifier = Modifier.fillMaxWidth().testTag(FriendsTags.SEARCH_INPUT),
        )
        Button(onClick = { proof?.let(onLookup) }, enabled = proof != null && !busy, modifier = Modifier.testTag(FriendsTags.SEARCH_ACTION)) {
            if (lookup.looking) CircularProgressIndicator(Modifier.size(16.dp), strokeWidth = 2.dp) else Text(stringResource(R.string.friends_search_action))
        }
        if (lookup.result == null) lookup.error?.let { FriendsErrorText(it, tag = FriendsTags.SEARCH_ERROR) }
        FriendResultCard(lookup, onConfirm, onOpenChat)
    }
}

/**
 * The result card (D6) with the one action its relation allows: add (with an optional note, never for a friend link), accept, a
 * disabled "request sent", or "friends" with Open chat. After a request has gone out the card says so. Shared by the Add friend
 * screen and a Team member's Add friend (D15).
 */
@Composable
internal fun FriendResultCard(lookup: LookupState, onConfirm: (note: String) -> Unit, onOpenChat: (String) -> Unit, modifier: Modifier = Modifier) {
    val result = lookup.result ?: return
    var note by rememberSaveable(result.uid) { mutableStateOf("") }
    val outcome = lookup.outcome
    Surface(modifier.fillMaxWidth().testTag(FriendsTags.RESULT), shape = MaterialTheme.shapes.medium, tonalElevation = 1.dp) {
        Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                PersonAvatar(result.uid, result.displayName, 44.dp)
                Column(Modifier.weight(1f)) {
                    Text(result.displayName.ifEmpty { stringResource(R.string.teams_member) }, style = MaterialTheme.typography.titleMedium, maxLines = 1, overflow = TextOverflow.Ellipsis)
                    result.handle?.let { Text("@$it", style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant, maxLines = 1, overflow = TextOverflow.Ellipsis) }
                }
            }
            if (outcome is FriendOutcome.RequestSent) {
                Text(stringResource(R.string.friends_request_sent_to_fmt, outcome.name), style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.primary)
            } else when (FriendsLogic.relationAction(result.relation)) {
                RelationAction.Add -> {
                    if (lookup.proof !is FriendProof.Code) OutlinedTextField(
                        value = note, onValueChange = { if (FriendsLogic.noteFits(it)) note = it }, enabled = !lookup.confirming,
                        placeholder = { Text(stringResource(R.string.friends_note_placeholder)) }, minLines = 2, maxLines = 4,
                        modifier = Modifier.fillMaxWidth().testTag(FriendsTags.RESULT_NOTE),
                    )
                    ResultButton(R.string.friends_add, lookup.confirming) { onConfirm(note) }
                }
                RelationAction.Accept -> ResultButton(R.string.friends_request_accept, lookup.confirming) { onConfirm("") }
                RelationAction.Sent -> Button(onClick = {}, enabled = false, modifier = Modifier.testTag(FriendsTags.RESULT_ACTION)) { Text(stringResource(R.string.friends_request_sent)) }
                RelationAction.Friends -> {
                    Text(stringResource(R.string.friends_relation_friends), style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.primary)
                    Button(onClick = { onOpenChat(result.uid) }, modifier = Modifier.testTag(FriendsTags.RESULT_ACTION)) { Text(stringResource(R.string.friends_open_chat)) }
                }
            }
            lookup.error?.let { FriendsErrorText(it) }
        }
    }
}

/** A Team member's Add friend (D15): the lookup by team and uid, then the same result card, in the Teams tab's sheet. */
@Composable
internal fun FriendMemberSheet(lookup: LookupState, onConfirm: (note: String) -> Unit, onOpenChat: (String) -> Unit, onDismiss: () -> Unit) {
    TeamsSheet(stringResource(R.string.friends_add), onDismiss) {
        when {
            lookup.result != null -> FriendResultCard(lookup, onConfirm, onOpenChat)
            lookup.error != null -> FriendsErrorText(lookup.error, tag = FriendsTags.SEARCH_ERROR)
            else -> CircularProgressIndicator(Modifier.size(24.dp), strokeWidth = 2.dp)
        }
    }
}

@Composable
private fun ResultButton(@StringRes label: Int, busy: Boolean, onClick: () -> Unit) {
    Button(onClick = onClick, enabled = !busy, modifier = Modifier.testTag(FriendsTags.RESULT_ACTION)) {
        if (busy) CircularProgressIndicator(Modifier.size(16.dp), strokeWidth = 2.dp) else Text(stringResource(label))
    }
}

/**
 * Opening a friend link (D8): the link is looked up and the person confirms before anything is redeemed. A friend already shows
 * "friends" with Open chat; a code that finds nobody (revoked, rotated, blocked, your own) says the link is no longer valid.
 */
@Composable
internal fun FriendLinkDialog(lookup: LookupState, onConfirm: () -> Unit, onRetry: () -> Unit, onOpenChat: (String) -> Unit, onDismiss: () -> Unit) {
    val result = lookup.result
    val dialog = Modifier.testTag(FriendsTags.LINK_DIALOG)
    when {
        result != null && result.relation == FriendRelation.Friends -> AlertDialog(
            modifier = dialog, onDismissRequest = onDismiss,
            title = { Text(result.displayName.ifEmpty { stringResource(R.string.teams_member) }) },
            text = { Text(stringResource(R.string.friends_relation_friends)) },
            confirmButton = { TextButton(onClick = { onDismiss(); onOpenChat(result.uid) }) { Text(stringResource(R.string.friends_open_chat)) } },
            dismissButton = { TextButton(onClick = onDismiss) { Text(stringResource(R.string.common_cancel)) } },
        )
        result != null -> AlertDialog(
            modifier = dialog, onDismissRequest = { if (!lookup.confirming) onDismiss() },
            title = { Text(stringResource(R.string.friends_confirm_title_fmt, result.displayName.ifEmpty { stringResource(R.string.teams_member) })) },
            text = {
                Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                    Text(stringResource(R.string.friends_confirm_link_body))
                    lookup.error?.let { FriendsErrorText(it) }
                }
            },
            confirmButton = {
                TextButton(onClick = onConfirm, enabled = !lookup.confirming) {
                    if (lookup.confirming) CircularProgressIndicator(Modifier.size(16.dp), strokeWidth = 2.dp) else Text(stringResource(R.string.friends_add))
                }
            },
            dismissButton = { TextButton(onClick = onDismiss, enabled = !lookup.confirming) { Text(stringResource(R.string.common_cancel)) } },
        )
        lookup.error != null -> AlertDialog(
            modifier = dialog, onDismissRequest = onDismiss,
            text = { Text(stringResource(if (lookup.error == FriendsError.NotFound) R.string.friends_link_invalid else friendsErrorRes(lookup.error))) },
            confirmButton = {
                if (lookup.error.definitive) TextButton(onClick = onDismiss) { Text(stringResource(R.string.common_close)) }
                else TextButton(onClick = onRetry) { Text(stringResource(R.string.common_retry)) }
            },
            dismissButton = if (lookup.error.definitive) null else ({ TextButton(onClick = onDismiss) { Text(stringResource(R.string.common_cancel)) } }),
        )
        else -> AlertDialog(
            modifier = dialog, onDismissRequest = onDismiss,
            text = { Row(horizontalArrangement = Arrangement.Center, modifier = Modifier.fillMaxWidth()) { CircularProgressIndicator() } },
            confirmButton = {},
            dismissButton = { TextButton(onClick = onDismiss) { Text(stringResource(R.string.common_cancel)) } },
        )
    }
}
