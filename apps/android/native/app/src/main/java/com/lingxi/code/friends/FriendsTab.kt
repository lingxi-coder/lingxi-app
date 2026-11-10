package com.lingxi.code.friends

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.systemBars
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.windowInsetsPadding
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.outlined.ChatBubbleOutline
import androidx.compose.material.icons.outlined.PersonOutline
import androidx.compose.material3.Button
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.VerticalDivider
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import com.lingxi.code.R
import com.lingxi.code.settings.SettingsRoutes
import com.lingxi.code.tabs.AppTab
import com.lingxi.code.tabs.FriendsRequest
import com.lingxi.code.tabs.LocalTabHost
import com.lingxi.code.tabs.LocalWideLayout
import com.lingxi.code.tabs.OnTabReselected
import com.lingxi.code.teams.TeamsMessageState

/**
 * The Friends tab (docs/FRIENDS-CLIENTS.md D1-D16): the friends list, Add friend, Profile settings, Blocked people and a friend's chat,
 * over the app-level [FriendsWorkspace]. A [request] (a friend link or a push) waits for sign-in, acts once and is handed back
 * through [onRequestHandled]; it never redeems, accepts or sends anything by itself.
 *
 * Phone: the list is the root; the other screens are pushed above it (the tab bar hides while one is open). Wide (>= 840 dp): the list
 * on the left, whatever is pushed on the right.
 */
@Composable
internal fun FriendsTab(request: FriendsRequest?, onRequestHandled: (Long) -> Unit, modifier: Modifier = Modifier, workspaceOverride: FriendsWorkspace? = null) {
    val context = LocalContext.current
    val workspace = workspaceOverride ?: remember(context) { FriendsWorkspace.get(context.applicationContext) }
    val state by workspace.state.collectAsStateWithLifecycle()
    val link by workspace.link.state.collectAsStateWithLifecycle()
    val search by workspace.search.state.collectAsStateWithLifecycle()
    val host = LocalTabHost.current
    val wide = LocalWideLayout.current
    val listState = rememberLazyListState()
    var scrollTop by remember { mutableIntStateOf(0) }
    var scrollToRequests by remember { mutableIntStateOf(0) }

    // The pushed screens, newest last: "add", "profile", "blocked" or "chat:<uid>". Saved as text so it survives the tab leaving composition.
    var stackText by rememberSaveable { mutableStateOf("") }
    val stack = stackText.split('\n').filter { it.isNotEmpty() }
    fun setStack(next: List<String>) { stackText = next.joinToString("\n") }
    fun openPage(page: String) = setStack(if (wide) listOf(page) else if (page in stack) stack else stack + page)
    // A chat replaces Add friend, which led to it; beside the list (wide) it replaces whatever is in the pane.
    fun openChat(friendUid: String) = setStack(if (wide) listOf("chat:$friendUid") else stack.filterNot { it == "add" || it.startsWith("chat:") } + "chat:$friendUid")
    fun close(page: String) = setStack(stack.filterNot { it == page })

    val uid = state.uid
    LaunchedEffect(uid) {
        if (uid == null) { setStack(emptyList()); workspace.link.clear() }
        else if (state.profile == null) workspace.loadProfile()
    }

    // A link or push waits for sign-in and, for a chat, for the first cards snapshot (D16).
    LaunchedEffect(request?.id, uid, state.cardsLoaded) {
        val pending = request ?: return@LaunchedEffect
        if (uid == null) return@LaunchedEffect
        when (val target = pending.link) {
            is FriendLink.Add -> { setStack(emptyList()); workspace.link.lookup(FriendProof.Code(target.code)) }
            FriendLink.Requests -> { setStack(emptyList()); scrollToRequests++ }
            is FriendLink.Chat -> {
                if (!state.cardsLoaded) return@LaunchedEffect
                if (state.friendCard(target.uid) != null) openChat(target.uid) else { setStack(emptyList()); workspace.chatUnavailable() }
            }
        }
        onRequestHandled(pending.id)
    }
    // Opening a friend link redeems nothing until the dialog is confirmed; a finished friendship opens its chat.
    LaunchedEffect(link.outcome) {
        val outcome = link.outcome
        if (outcome is FriendOutcome.NowFriends) { workspace.link.clear(); openChat(outcome.uid) }
    }

    val depth = stack.size
    LaunchedEffect(depth) { host.reportDepth(AppTab.Friends, depth) }
    BackHandler(enabled = stack.isNotEmpty()) { setStack(stack.dropLast(1)) }
    // Re-tapping the tab pops to the list, then scrolls it to the top.
    OnTabReselected(AppTab.Friends) { if (stack.isNotEmpty()) setStack(emptyList()) else scrollTop++ }
    LaunchedEffect(scrollTop) { if (scrollTop > 0) listState.animateScrollToItem(0) }

    Box(modifier.fillMaxSize().background(MaterialTheme.colorScheme.background).windowInsetsPadding(WindowInsets.systemBars).imePadding().testTag(FriendsTags.ROOT)) {
        if (uid == null) {
            TeamsMessageState(
                stringResource(R.string.friends_title), Modifier.testTag(FriendsTags.SIGN_IN), icon = Icons.Outlined.PersonOutline, body = stringResource(R.string.friends_signin),
                action = { Button(onClick = { host.openSettings(SettingsRoutes.ACCOUNT, origin = AppTab.Friends) }) { Text(stringResource(R.string.remote_sign_in)) } },
            )
        } else {
            val top = stack.lastOrNull().orEmpty()
            val chatUid = stack.lastOrNull { it.startsWith("chat:") }?.removePrefix("chat:").orEmpty()
            val list: @Composable (Modifier) -> Unit = { m ->
                FriendsListScreen(
                    state = state, uid = uid, openChatUid = if (wide) chatUid else "", listState = listState, scrollToRequests = scrollToRequests,
                    onOpenChat = ::openChat, onAdd = { openPage("add") }, onProfile = { openPage("profile") }, onBlocked = { openPage("blocked") },
                    onRetry = workspace::retry, onRetryProfile = workspace::loadProfile,
                    onAccept = workspace::accept, onDecline = workspace::decline, onCancel = workspace::cancel, modifier = m,
                )
            }
            val page: @Composable (String, Modifier) -> Unit = { entry, m ->
                when {
                    entry == "add" -> AddFriendScreen(
                        state, search, showBack = !wide, onClose = { close(entry) }, onOpenChat = ::openChat, onLoadProfile = workspace::loadProfile,
                        onRotateCode = workspace::rotateCode, onRevokeCode = workspace::revokeCode, onLookup = workspace.search::lookup,
                        onConfirm = workspace.search::confirm, onClear = workspace.search::clear, modifier = m,
                    )
                    entry == "profile" -> FriendProfileScreen(
                        state, showBack = !wide, onClose = { close(entry) }, onLoad = workspace::loadProfile, onSave = workspace::saveProfile, modifier = m,
                    )
                    entry == "blocked" -> BlockedPeopleScreen(
                        state, showBack = !wide, onClose = { close(entry) }, onRetry = workspace::retry, onUnblock = workspace::unblock, modifier = m,
                    )
                    entry.startsWith("chat:") -> FriendChatScreen(workspace, state, uid, entry.removePrefix("chat:"), showBack = !wide, onBack = { close(entry) }, modifier = m)
                }
            }
            if (wide) {
                Row(Modifier.fillMaxSize()) {
                    list(Modifier.width(320.dp).fillMaxHeight())
                    VerticalDivider()
                    if (top.isNotEmpty()) page(top, Modifier.weight(1f).fillMaxHeight())
                    else Box(Modifier.weight(1f).fillMaxHeight()) { TeamsMessageState(stringResource(R.string.friends_title), icon = Icons.Outlined.ChatBubbleOutline) }
                }
            } else if (top.isEmpty()) list(Modifier.fillMaxSize()) else page(top, Modifier.fillMaxSize())

            if (link.proof != null) FriendLinkDialog(
                lookup = link,
                onConfirm = { workspace.link.confirm("") },
                onRetry = { link.proof?.let(workspace.link::lookup) },
                onOpenChat = ::openChat,
                onDismiss = workspace.link::clear,
            )
        }
        state.notice?.let { notice -> FriendsNoticeBanner(notice, onDismiss = { workspace.clearNotice(notice) }, modifier = Modifier.align(Alignment.BottomCenter)) }
    }
}
