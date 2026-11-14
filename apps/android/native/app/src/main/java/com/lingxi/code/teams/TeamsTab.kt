package com.lingxi.code.teams

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
import androidx.compose.material.icons.outlined.ErrorOutline
import androidx.compose.material.icons.outlined.Groups
import androidx.compose.material3.Button
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
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
import androidx.compose.ui.platform.LocalConfiguration
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import com.lingxi.code.R
import com.lingxi.code.friends.FriendLink
import com.lingxi.code.friends.FriendMemberSheet
import com.lingxi.code.friends.FriendOutcome
import com.lingxi.code.friends.FriendProof
import com.lingxi.code.friends.FriendsBanner
import com.lingxi.code.friends.FriendsWorkspace
import com.lingxi.code.friends.FriendState
import com.lingxi.code.friends.InviteFriendsSheet
import com.lingxi.code.friends.friendNamesText
import com.lingxi.code.settings.SettingsRoutes
import com.lingxi.code.tabs.AppTab
import com.lingxi.code.tabs.LocalTabHost
import com.lingxi.code.tabs.LocalWideLayout
import com.lingxi.code.tabs.OnTabReselected
import java.util.UUID

/**
 * The Teams tab (SPEC-mobile-team-bot-ui section 6): conversation list, conversation, team details and the new-conversation
 * sheets over the app-level [TeamsWorkspace]. [joinCode] opens the join sheet prefilled (it never joins by itself) and is
 * cleared through [onJoinCodeHandled] once it has been put there.
 *
 * Phone: the list is the root; a conversation or the team details are pushed above it (the tab bar hides while one is open).
 * Wide (>= 840 dp): list | conversation | team details side by side; depth still describes what Back can pop.
 */
@Composable
internal fun TeamsTab(joinCode: String?, onJoinCodeHandled: () -> Unit, modifier: Modifier = Modifier, workspaceOverride: TeamsWorkspace? = null) {
    val context = LocalContext.current
    val workspace = workspaceOverride ?: remember(context) { TeamsWorkspace.get(context.applicationContext) }
    val teams by workspace.teams.collectAsStateWithLifecycle()
    val messaging by workspace.messaging.collectAsStateWithLifecycle()
    messaging.artifact?.let { artifact -> TeamArtifactViewer(artifact, workspace.messagingModel, teams.detail?.members.orEmpty(), messaging.bots, workspace.messagingModel::closeArtifact) }
    // The review of a task from the list of recent tasks; a task inside a conversation is reviewed from its own screen.
    var reviewing by remember { mutableStateOf<String?>(null) }
    reviewing?.let { taskId ->
        val viewer = teams.uid?.let { uid -> messaging.cache?.let { TeamMemberRef(uid, it.generation) } }
        TeamOperationsDialog(taskId, workspace.messagingModel, TeamOperationRules.maintains(taskId, messaging.tasks, messaging.bots, viewer), teams.detail?.members.orEmpty()) { reviewing = null }
    }
    val collapsed by workspace.collapsed.collectAsStateWithLifecycle()
    val unreadByTeam by workspace.unreadByTeam.collectAsStateWithLifecycle()
    val host = LocalTabHost.current
    val wide = LocalWideLayout.current
    val listState = rememberLazyListState()
    var scrollTop by remember { mutableIntStateOf(0) }
    // Add friend on a member (D15) is the Friends workspace's own lookup; the friends there are who shows "Friends" instead of the action.
    val friends = remember(context) { FriendsWorkspace.get(context.applicationContext) }
    val friendsState by friends.state.collectAsStateWithLifecycle()
    val memberLookup by friends.member.state.collectAsStateWithLifecycle()
    val friendUids = remember(friendsState.cards) { friendsState.cards.filter { it.state == FriendState.Friends }.map { it.uid }.toSet() }
    // The friends invited from the picker that just closed (D26): said once, here, because only the person who sent them can know.
    var invited by remember { mutableStateOf<List<String>>(emptyList()) }

    // The pushed screens, newest last: "c:<conversationId>" or "details". Saved as text so it survives the tab leaving composition.
    var stackText by rememberSaveable { mutableStateOf("") }
    val stack = stackText.split('\n').filter { it.isNotEmpty() }
    var sheet by rememberSaveable { mutableStateOf("") }
    var joinPrefill by rememberSaveable { mutableStateOf("") }
    fun setStack(next: List<String>) { stackText = next.joinToString("\n") }
    fun openConversation(id: String) = setStack(if (wide) stack.filterNot { it.startsWith("c:") } + "c:$id" else stack + "c:$id")
    fun openDetails() = setStack(if ("details" in stack) stack else stack + "details")

    val uid = teams.uid
    LaunchedEffect(joinCode, uid) {
        if (joinCode != null && uid != null) { joinPrefill = joinCode; sheet = "join"; onJoinCodeHandled() }
    }
    LaunchedEffect(uid) { if (uid == null) { stackText = ""; sheet = "" } }
    // A friendship made from a member row (a mutual request) opens the chat in the Friends tab.
    LaunchedEffect(memberLookup.outcome) {
        val outcome = memberLookup.outcome
        if (outcome is FriendOutcome.NowFriends) { sheet = ""; friends.member.clear(); host.requestFriends(FriendLink.Chat(outcome.uid)) }
    }
    val depth = stack.size
    LaunchedEffect(depth) { host.reportDepth(AppTab.Teams, depth) }
    BackHandler(enabled = stack.isNotEmpty()) { setStack(stack.dropLast(1)) }
    // Re-tapping the tab pops to the list, then scrolls it to the top.
    OnTabReselected(AppTab.Teams) { if (stack.isNotEmpty()) setStack(emptyList()) else scrollTop++ }
    LaunchedEffect(scrollTop) { if (scrollTop > 0) listState.animateScrollToItem(0) }

    Box(modifier.fillMaxSize().background(MaterialTheme.colorScheme.background).windowInsetsPadding(WindowInsets.systemBars).imePadding().testTag(TeamsTags.ROOT)) {
        val team = teams.teams.firstOrNull { it.id == teams.selectedId }
        when {
            uid == null -> SignedOutGate { host.openSettings(SettingsRoutes.ACCOUNT, origin = AppTab.Teams) }
            teams.loadError != null && teams.teams.isEmpty() -> TeamsMessageState(
                stringResource(R.string.teams_load_failed), Modifier.testTag(TeamsTags.LOAD_FAILED), icon = Icons.Outlined.ErrorOutline, body = stringResource(teamsErrorRes(teams.loadError!!)),
                action = { Button(onClick = { workspace.teamsModel.refresh() }) { Text(stringResource(R.string.common_retry)) } },
            )
            !teams.loaded -> Box(Modifier.fillMaxSize().testTag(TeamsTags.LOADING), contentAlignment = Alignment.Center) { CircularProgressIndicator() }
            teams.teams.isEmpty() && teams.loading -> Box(Modifier.fillMaxSize().testTag(TeamsTags.LOADING), contentAlignment = Alignment.Center) { CircularProgressIndicator() }
            teams.teams.isEmpty() -> TeamsMessageState(
                stringResource(R.string.teams_empty), Modifier.testTag(TeamsTags.EMPTY), icon = Icons.Outlined.Groups, body = stringResource(R.string.teams_disclosure),
                action = {
                    Button(onClick = { workspace.teamsModel.clearError(); sheet = "create" }) { Text(stringResource(R.string.teams_create)) }
                    OutlinedButton(onClick = { workspace.teamsModel.clearError(); joinPrefill = ""; sheet = "join" }) { Text(stringResource(R.string.teams_join)) }
                    TextButton(onClick = { sheet = "services" }, modifier = Modifier.testTag("teams.empty.services")) { Text(stringResource(R.string.services_title)) }
                },
            )
            team == null -> TeamsMessageState(
                stringResource(R.string.teams_choose_team), icon = Icons.Outlined.Groups,
                action = { Button(onClick = { sheet = "switcher" }) { Text(stringResource(R.string.teams_switch_team)) } },
            )
            else -> {
                val conversationId = stack.lastOrNull { it.startsWith("c:") }?.removePrefix("c:").orEmpty()
                val list: @Composable (Modifier) -> Unit = { m ->
                    TeamsListScreen(
                        uid = uid, teams = teams, messaging = messaging, collapsed = collapsed, unreadByTeam = unreadByTeam,
                        openConversationId = if (wide) conversationId else "", listState = listState,
                        onOpenConversation = ::openConversation, onSwitchTeam = { sheet = "switcher" }, onNew = { sheet = it },
                        onToggleSection = workspace::toggleSection,
                        onMarkRead = { workspace.messagingModel.markRead(it.id, it.lastSeq) },
                        onMessageBot = { id -> messageBot(workspace, messaging, uid, id) { openConversation(it) } },
                        onArtifact = { workspace.messagingModel.openArtifact(it) },
                        onRefreshTask = { workspace.messagingModel.refreshTask(it) },
                        onCancelTask = { workspace.messagingModel.cancelTask(it) },
                        onReviewTask = { reviewing = it },
                        onDiscardCommand = { workspace.messagingModel.discardCommand(it) },
                        onReconnect = workspace::reconnect, onDiscardCreation = { workspace.messagingModel.discardCreation() }, modifier = m,
                    )
                }
                val conversation: @Composable (String, Modifier) -> Unit = { id, m ->
                    TeamConversationScreen(
                        workspace = workspace, uid = uid, team = team, teams = teams, messaging = messaging, conversationId = id,
                        showBack = !wide, onBack = { setStack(stack.dropLast(1)) }, modifier = m,
                    )
                }
                val details: @Composable (Modifier) -> Unit = { m ->
                    TeamDetailsScreen(
                        workspace = workspace, uid = uid, team = team, teams = teams, messaging = messaging, showBack = !wide,
                        friendUids = friendUids, onAddFriend = { member -> friends.member.lookup(FriendProof.Team(team.id, member.uid)); sheet = "addFriend" },
                        onInviteFriends = { sheet = "inviteFriends" }, onClose = { setStack(stack.filterNot { it == "details" }) },
                        onMessageBot = { key -> messageBot(workspace, messaging, uid, key) { openConversation(it) } }, modifier = m,
                    )
                }
                if (wide) {
                    Row(Modifier.fillMaxSize()) {
                        list(Modifier.width(320.dp).fillMaxHeight())
                        VerticalDivider()
                        if (conversationId.isNotEmpty()) conversation(conversationId, Modifier.weight(1f).fillMaxHeight())
                        else Box(Modifier.weight(1f).fillMaxHeight()) { TeamsMessageState(stringResource(R.string.teams_conversations), icon = Icons.Outlined.ChatBubbleOutline) }
                        if ("details" in stack) { VerticalDivider(); details(Modifier.width(360.dp).fillMaxHeight()) }
                    }
                } else when (val top = stack.lastOrNull()) {
                    null -> list(Modifier.fillMaxSize())
                    "details" -> details(Modifier.fillMaxSize())
                    else -> conversation(top.removePrefix("c:"), Modifier.fillMaxSize())
                }
            }
        }

        val dismiss = { sheet = "" }
        when {
            uid == null -> Unit
            sheet == "switcher" -> TeamSwitcherSheet(
                teams = teams.teams, selectedId = teams.selectedId, unreadByTeam = unreadByTeam,
                onSelect = { id -> sheet = ""; setStack(emptyList()); workspace.selectTeam(id) },
                onCreate = { workspace.teamsModel.clearError(); sheet = "create" },
                onJoin = { workspace.teamsModel.clearError(); joinPrefill = ""; sheet = "join" },
                onDetails = { sheet = ""; openDetails() }, onDismiss = dismiss, onServices = { sheet = "services" },
            )
            // Keyed by the account: another account's services are never shown.
            sheet == "services" -> androidx.compose.runtime.key(uid) { HostedServicesScreen(onClose = dismiss) }
            sheet == "create" -> CreateTeamSheet(
                busy = teams.busy, error = teams.error,
                onSubmit = { name -> workspace.teamsModel.execute(mapOf("action" to "create", "teamId" to UUID.randomUUID().toString(), "name" to name.trim())) { sheet = ""; setStack(emptyList()) } },
                onDismiss = dismiss,
            )
            sheet == "join" -> JoinTeamSheet(
                prefill = joinPrefill, busy = teams.busy, error = teams.error,
                onSubmit = { code -> workspace.teamsModel.execute(mapOf("action" to "join", "code" to code, "requestId" to UUID.randomUUID().toString())) { sheet = ""; joinPrefill = ""; setStack(emptyList()) } },
                onDismiss = dismiss,
            )
            sheet == "addFriend" && memberLookup.proof != null -> FriendMemberSheet(
                lookup = memberLookup, onConfirm = { note -> friends.member.confirm(note) },
                onOpenChat = { friendUid -> sheet = ""; friends.member.clear(); host.requestFriends(FriendLink.Chat(friendUid)) },
                onDismiss = { sheet = ""; friends.member.clear() },
            )
            sheet == "inviteFriends" && team == null -> LaunchedEffect(Unit) { sheet = ""; friends.closeInvites() }
            // Invite friends (D26): the app-wide Friends workspace's cards against this team's members; a person who turns out to be in already
            // (ALREADY_MEMBER) asks for the member list again.
            sheet == "inviteFriends" && team != null -> InviteFriendsSheet(
                teamName = team.name, state = friendsState,
                memberUids = teams.detail?.takeIf { it.team.id == team.id }?.members.orEmpty().map { it.uid }.toSet(),
                onInvite = { friendUid -> friends.offerTeam(team.id, friendUid, onAlreadyMember = { workspace.teamsModel.refresh() }) },
                onDismiss = { sheet = ""; invited = friends.closeInvites() },
            )
            sheet == "newMessage" || sheet == "newChannel" || sheet == "newBot" -> NewConversationSheet(
                mode = sheet, uid = uid, members = teams.detail?.members.orEmpty(), messaging = messaging,
                onOpen = { id -> sheet = ""; openConversation(id) },
                onCreate = { kind, title, parties, sharedBotIds ->
                    val id = UUID.randomUUID().toString()
                    workspace.messagingModel.create(kind, title, parties, id, sharedBotIds) { sheet = ""; openConversation(id) }
                },
                onDismiss = dismiss,
            )
        }
        if (invited.isNotEmpty()) {
            val locale = LocalConfiguration.current.locales[0]
            FriendsBanner(stringResource(R.string.friends_invite_sent_to_fmt, friendNamesText(invited, locale)), invited, onDismiss = { invited = emptyList() }, modifier = Modifier.align(Alignment.BottomCenter))
        }
    }
}

/** Opens the viewer's Bot DM with the Bot or creates it; the same rule as the Bots list on the Desktop. A friend's chat starts one the same way (D22). */
internal fun messageBot(workspace: TeamsWorkspace, messaging: TeamMessagingState, uid: String, sharedBotId: String, open: (String) -> Unit) {
    val existing = TeamsListLogic.findBotDm(messaging.cache?.conversations.orEmpty(), uid, sharedBotId, messaging.bots)
    if (existing != null) { open(existing.id); return }
    val bot = messaging.bots.firstOrNull { it.sharedBotId == sharedBotId } ?: return
    val id = UUID.randomUUID().toString()
    workspace.messagingModel.create("bot_dm", bot.name, emptyList(), id, listOf(sharedBotId)) { open(id) }
}

@Composable
private fun SignedOutGate(onSignIn: () -> Unit) {
    TeamsMessageState(
        stringResource(R.string.teams_title), Modifier.testTag(TeamsTags.SIGN_IN), icon = Icons.Outlined.Groups, body = stringResource(R.string.teams_signin),
        action = { Button(onClick = onSignIn) { Text(stringResource(R.string.remote_sign_in)) } },
    )
}
