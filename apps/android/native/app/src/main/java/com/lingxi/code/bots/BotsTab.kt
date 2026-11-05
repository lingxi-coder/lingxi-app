package com.lingxi.code.bots

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.systemBars
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.windowInsetsPadding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.LazyListState
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.outlined.Add
import androidx.compose.material.icons.outlined.Computer
import androidx.compose.material.icons.outlined.ErrorOutline
import androidx.compose.material.icons.outlined.Info
import androidx.compose.material.icons.outlined.KeyboardArrowDown
import androidx.compose.material3.Button
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.VerticalDivider
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.key
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.produceState
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.runtime.snapshots.SnapshotStateList
import androidx.compose.runtime.snapshots.SnapshotStateMap
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateMapOf
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.rotate
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.LifecycleEventObserver
import androidx.lifecycle.compose.LocalLifecycleOwner
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.repeatOnLifecycle
import com.lingxi.code.R
import com.lingxi.code.bindings.android.RemoteBotStatusFfi
import com.lingxi.code.bindings.android.RemoteConnectionPhaseFfi
import com.lingxi.code.bindings.android.RemoteHostFfi
import com.lingxi.code.bindings.android.RemotePendingActionFfi
import com.lingxi.code.bindings.android.RemoteSessionStatusFfi
import com.lingxi.code.connectivity.rememberOnlineState
import com.lingxi.code.remote.RemoteConnectionUi
import com.lingxi.code.remote.RemoteControl
import com.lingxi.code.remote.RemoteControlManager
import com.lingxi.code.remote.RemoteHostScreen
import com.lingxi.code.remote.RemotePage
import com.lingxi.code.remote.RemoteRoute
import com.lingxi.code.remote.RemoteSessionScreen
import com.lingxi.code.remote.RemoteSignedOut
import com.lingxi.code.remote.botCreateErrorText
import com.lingxi.code.remote.botStatusText
import com.lingxi.code.remote.formatPairingCode
import com.lingxi.code.remote.nameOf
import com.lingxi.code.remote.pendingActionText
import com.lingxi.code.remote.platformLabel
import com.lingxi.code.remote.rememberCountdown
import com.lingxi.code.remote.sessionStatusText
import com.lingxi.code.remote.summaryOf
import com.lingxi.code.settings.SettingsRoutes
import com.lingxi.code.tabs.AppTab
import com.lingxi.code.tabs.BotsRequest
import com.lingxi.code.tabs.LocalTabHost
import com.lingxi.code.tabs.LocalWideLayout
import com.lingxi.code.tabs.OnTabReselected
import com.lingxi.code.tabs.TabLogic
import com.lingxi.code.teams.listTimeText
import com.lingxi.code.teams.rememberNowMs
import com.lingxi.code.theme.BotAvatar
import com.lingxi.code.theme.LocalPalette
import java.time.Instant
import java.time.ZoneId
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.NonCancellable
import kotlinx.coroutines.awaitCancellation
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import kotlinx.coroutines.withTimeoutOrNull

/** Test tags of the Bots tab (this slice owns them; the shell only owns the four tab tags). */
internal object BotsTags {
    const val ROOT = "bots.root"
    const val SIGNED_OUT = "bots.signedOut"
    const val LOADING = "bots.loading"
    const val HOSTS_ERROR = "bots.hostsError"
    const val NO_COMPUTERS = "bots.noComputers"
    const val TREE = "bots.tree"
    const val DETAIL_EMPTY = "bots.detailEmpty"
    const val BOT_RUNNING = "bots.botRunning"
    fun host(hostId: String) = "bots.host.$hostId"
    fun info(hostId: String) = "bots.info.$hostId"
    fun connection(hostId: String) = "bots.connection.$hostId"
    fun note(hostId: String, note: BodyNote) = "bots.note.$hostId.${note.key}"
    fun state(hostId: String) = "bots.state.$hostId"
    fun connect(hostId: String) = "bots.connect.$hostId"
    fun bot(hostId: String, botId: String) = "bots.bot.$hostId.$botId"
    fun newChat(hostId: String, botId: String) = "bots.newChat.$hostId.$botId"
    fun botState(hostId: String, botId: String) = "bots.botState.$hostId.$botId"
    fun chat(hostId: String, botId: String, sessionId: String) = "bots.chat.$hostId.$botId.$sessionId"
    fun showMore(hostId: String, botId: String) = "bots.showMore.$hostId.$botId"
    fun showLess(hostId: String, botId: String) = "bots.showLess.$hostId.$botId"
    fun chatError(hostId: String, botId: String) = "bots.chatError.$hostId.$botId"
}

/** What the rows of the tree can ask for; the tab implements them, a test can record them. */
internal class BotsActions(
    val onSignIn: () -> Unit = {},
    val onRetryHosts: () -> Unit = {},
    val onToggleHost: (hostId: String, collapsed: Boolean) -> Unit = { _, _ -> },
    val onOpenHost: (hostId: String) -> Unit = {},
    val onToggleBot: (row: TreeRow.Bot) -> Unit = {},
    val onOpenChat: (row: TreeRow.Chat) -> Unit = {},
    val onNewChat: (row: TreeRow.NewChat) -> Unit = {},
    val onShowMore: (hostId: String, botId: String) -> Unit = { _, _ -> },
    val onShowLess: (hostId: String, botId: String) -> Unit = { _, _ -> },
    val onConnect: (hostId: String) -> Unit = {},
    val onRetryChats: (hostId: String, botId: String) -> Unit = { _, _ -> },
    val onRetryConnection: () -> Unit = {},
    val onRepair: (hostId: String) -> Unit = {},
)

/**
 * The Bots tab (SPEC-mobile-team-bot-ui section 4): the user's computers with their Bots and the Bots' chats as one tree,
 * Remote Control being the transport underneath. A computer's header collapses it and its (i) opens the Computer page;
 * a Bot opens to its chats and a chat opens the remote session. [request] is a computer or session a link or a
 * notification named; it is consumed once through [onRequestHandled].
 *
 * Phone: the tree is the root; the Computer page and a session are pushed above it (the tab bar hides while one is
 * open). Wide (>= 840 dp): the tree and page sit side by side; depth still describes what Back can pop.
 *
 * Connection (section 4.4): the list of computers is watched while the tab is shown and the app is in front; when
 * nothing is connected the last-used computer is dialled once it is known to be online and paired; leaving the tab
 * starts a [RemoteControlManager.LEAVE_GRACE_MS] wait after which the connection ends, and coming back calls it off.
 */
@Composable
fun BotsTab(request: BotsRequest?, onRequestHandled: (Long) -> Unit, modifier: Modifier = Modifier) {
    val context = LocalContext.current
    val manager = remember(context) { RemoteControl.manager(context) }
    val stores = remember(context) { BotsStores.get(context) }
    val host = LocalTabHost.current
    val wide = LocalWideLayout.current
    val scope = rememberCoroutineScope()
    val uid by manager.uid.collectAsStateWithLifecycle()
    val hostsState by manager.hosts.collectAsStateWithLifecycle()
    val connection by manager.connection.collectAsStateWithLifecycle()
    val sessions by manager.sessions.collectAsStateWithLifecycle()
    val botsState by manager.bots.collectAsStateWithLifecycle()
    val botChats by manager.botChats.collectAsStateWithLifecycle()
    val lastHostId by manager.lastHostId.collectAsStateWithLifecycle()
    val snapshots by produceState(emptyMap<String, BotsSnapshot>(), uid) {
        value = emptyMap()
        uid?.let { stores.snapshots.snapshots(it).collect { found -> value = found } }
    }
    val prefs by produceState<BotsPrefs?>(null, uid) {
        value = null
        uid?.let { stores.prefs.prefs(it).collect { found -> value = found } }
    }

    // The pages pushed above the tree, newest last; saved as text so the stack survives leaving the tab and process death.
    var stack by rememberSaveable(uid, stateSaver = RemoteRoute.ownedStackSaver(uid)) { mutableStateOf(emptyList<RemoteRoute>()) }
    val showAll = remember { mutableStateOf(emptySet<String>()) }
    val creating = remember { mutableStateListOf<String>() }
    val errors = remember { mutableStateMapOf<String, Int>() }
    val listState = rememberLazyListState()
    var scrollTop by remember { mutableIntStateOf(0) }
    fun push(route: RemoteRoute) { stack = if (wide) listOf(route) else stack + route }

    // The tab is shown: a teardown that was waiting is called off. The tab is left: one starts.
    val sessionOpen by rememberUpdatedState(uid != null && stack.lastOrNull() is RemoteRoute.Session)
    DisposableEffect(manager) {
        manager.cancelScheduledClose()
        onDispose { if (!sessionOpen) manager.scheduleCloseConnection() }
    }

    // The app came back to the front, or the network came back: skip the rest of a backoff.
    val lifecycle = LocalLifecycleOwner.current.lifecycle
    DisposableEffect(lifecycle, manager) {
        val observer = LifecycleEventObserver { _, event -> if (event == Lifecycle.Event.ON_RESUME) manager.nudge() }
        lifecycle.addObserver(observer)
        onDispose { lifecycle.removeObserver(observer) }
    }
    val online by rememberOnlineState()
    LaunchedEffect(online) { if (online) manager.nudge() }

    // Watch the computers while the app is in front; the first time they are listed, dial the last-used one.
    LaunchedEffect(manager, uid) {
        if (uid == null) return@LaunchedEffect
        lifecycle.repeatOnLifecycle(Lifecycle.State.STARTED) {
            launch {
                val listed = manager.hosts.first { it.loaded }
                val attempting = manager.connection.value?.takeUnless { it.phase == RemoteConnectionPhaseFfi.IDLE }?.hostId
                val target = BotsLogic.autoConnectTarget(true, attempting, manager.lastHostId.value, listed.hosts)
                if (target != null && stack.isEmpty() && request == null) runCatching { manager.connect(target) }
            }
            runCatching { manager.watchHosts() }
            try {
                awaitCancellation()
            } finally {
                withContext(NonCancellable) { runCatching { manager.stopWatchingHosts() } }
            }
        }
    }

    // A link or a notification names a session (back lands on the tree) or a computer.
    LaunchedEffect(request?.id, uid) {
        val asked = request ?: return@LaunchedEffect
        // Keep the link pending through the account jump. Following a session requires an
        // account, and a restored wide pane must never create that source while signed out.
        if (uid == null) return@LaunchedEffect
        stack = RemoteRoute.stackFor(asked.sessionId?.let { RemoteRoute.Session(asked.hostId, it) } ?: RemoteRoute.Host(asked.hostId))
        if (asked.sessionId == null) runCatching { manager.connect(asked.hostId) }
        onRequestHandled(asked.id)
    }

    val depth = if (uid == null) 0 else stack.size
    LaunchedEffect(depth) { host.reportDepth(AppTab.Bots, depth) }
    BackHandler(enabled = uid != null && stack.isNotEmpty()) { stack = RemoteRoute.afterBack(stack) }
    // Re-tapping the tab pops to the tree, then scrolls it to the top.
    OnTabReselected(AppTab.Bots) { atDepth -> if (atDepth > 0) stack = emptyList() else scrollTop++ }
    LaunchedEffect(scrollTop) { if (scrollTop > 0) listState.animateScrollToItem(0) }

    val rootState = BotsLogic.rootState(signedIn = uid != null, hosts = hostsState)
    val loadedPrefs = prefs
    val rows = remember(rootState, hostsState.hosts, connection, botsState, botChats, snapshots, loadedPrefs, showAll.value) {
        if (rootState != RootState.Tree || loadedPrefs == null) emptyList()
        else buildTree(TreeInput(hostsState.hosts, connection, botsState, botChats, snapshots, loadedPrefs.collapsedHosts, loadedPrefs.expandedBots, showAll.value))
    }
    val connectedId = BotsLogic.connectedHostId(connection)

    // The expanded Bots of the connected computer keep their chats current for as long as they are shown.
    for (row in rows) {
        if (row is TreeRow.Bot && row.expanded && row.facts.connected) {
            key(row.key) { LaunchedEffect(manager, row.facts.hostId, row.bot.botId) { runCatching { manager.showBotChats(row.facts.hostId, row.bot.botId) } } }
        }
    }

    fun connect(hostId: String) { scope.launch { runCatching { manager.connect(hostId) } } }
    fun openSession(hostId: String, sessionId: String) = push(RemoteRoute.Session(hostId, sessionId))

    val actions = BotsActions(
        onSignIn = { host.openSettings(SettingsRoutes.ACCOUNT, origin = AppTab.Bots) },
        onRetryHosts = { scope.launch { runCatching { manager.watchHosts() } } },
        onToggleHost = { hostId, collapsed -> uid?.let { scope.launch { stores.prefs.setCollapsed(it, hostId, collapsed) } } },
        onOpenHost = { push(RemoteRoute.Host(it)) },
        onToggleBot = { row ->
            val key = BotsLogic.botKey(row.facts.hostId, row.bot.botId)
            val plan = BotsLogic.botTapPlan(row.facts, row.expanded)
            val current = loadedPrefs?.expandedBots ?: BotsLogic.defaultExpanded(connectedId, botsState)
            uid?.let { scope.launch { stores.prefs.setExpandedBots(it, if (plan.expanded) current + key else current - key) } }
            // A Bot that closes forgets that it showed every chat, and any complaint about it.
            if (!plan.expanded) { showAll.value = showAll.value - key; errors.remove(key) }
            if (plan.connect) connect(row.facts.hostId)
        },
        onOpenChat = { row ->
            val key = BotsLogic.botKey(row.facts.hostId, row.botId)
            errors.remove(key)
            when (BotsLogic.openChatPlan(row.facts)) {
                // The session page dials the computer itself when it is not the connected one.
                OpenChatPlan.OpenNow, OpenChatPlan.ConnectThenOpen -> openSession(row.facts.hostId, row.chat.sessionId)
                OpenChatPlan.NoAnswer -> errors[key] = R.string.remote_fail_no_answer
                OpenChatPlan.Pair -> connect(row.facts.hostId)
            }
        },
        onNewChat = { row ->
            val hostId = row.facts.hostId
            val key = BotsLogic.botKey(hostId, row.botId)
            val plan = BotsLogic.newChatPlan(row.facts)
            if (plan != NewChatPlan.Disabled && key !in creating) {
                creating += key
                errors.remove(key)
                scope.launch {
                    try {
                        if (plan == NewChatPlan.ConnectThenCreate) {
                            manager.connect(hostId)
                            check(manager.awaitReady(hostId)) { "remote computer is not reachable" }
                        }
                        openSession(hostId, manager.createBotThread(hostId, row.botId))
                    } catch (cancelled: CancellationException) {
                        throw cancelled
                    } catch (error: Exception) {
                        errors[key] = botCreateErrorText(error)
                    } finally {
                        creating -= key
                    }
                }
            }
        },
        onShowMore = { hostId, botId -> showAll.value = showAll.value + BotsLogic.botKey(hostId, botId) },
        onShowLess = { hostId, botId -> showAll.value = showAll.value - BotsLogic.botKey(hostId, botId) },
        onConnect = ::connect,
        onRetryChats = { hostId, botId -> scope.launch { runCatching { manager.retryBotChats(hostId, botId) } } },
        onRetryConnection = { scope.launch { runCatching { manager.retry() } } },
        onRepair = { hostId -> scope.launch { runCatching { manager.repair(hostId) } } },
    )

    fun hostName(hostId: String) =
        connection?.takeIf { it.hostId == hostId }?.host?.name ?: hostsState.hosts.firstOrNull { it.hostId == hostId }?.name ?: hostId
    val detail: @Composable (Modifier) -> Unit = { m ->
        val top = stack.lastOrNull()
        Box(m) {
            when (top) {
                is RemoteRoute.Host -> RemotePage(hostName(top.hostId), onBack = { stack = RemoteRoute.afterBack(stack) }) {
                    RemoteHostScreen(
                        manager = manager,
                        hostId = top.hostId,
                        hostName = hostName(top.hostId),
                        host = hostsState.hosts.firstOrNull { it.hostId == top.hostId },
                        onOpenSession = { openSession(top.hostId, it.sessionId) },
                        onLeave = { stack = RemoteRoute.afterBack(stack) },
                    )
                }
                is RemoteRoute.Session -> key(top.hostId, top.sessionId) {
                    // A Bot's chat is among its Bot's chats, not the sessions'; the connection knows which Bot once that list was shown.
                    val botId = remember(top, botChats) { manager.botOfThread(top.hostId, top.sessionId) ?: savedBotOf(snapshots[top.hostId], top.sessionId) }
                    val untitled = stringResource(R.string.remote_untitled)
                    RemoteSessionScreen(
                        manager = manager,
                        hostId = top.hostId,
                        hostName = hostName(top.hostId),
                        sessionId = top.sessionId,
                        title = sessions.sessions?.firstOrNull { it.sessionId == top.sessionId }?.title
                            ?: botChats.summaryOf(top.hostId, top.sessionId)?.title?.ifBlank { null }
                            ?: savedChatTitle(snapshots[top.hostId], top.sessionId)
                            ?: botId?.let { untitled },
                        botName = botId?.let { botsState.nameOf(top.hostId, it) ?: snapshots[top.hostId]?.bots?.firstOrNull { bot -> bot.botId == it }?.name },
                        isDark = LocalPalette.current.isDark,
                        onBack = { stack = RemoteRoute.afterBack(stack) },
                    )
                }
                else -> Box(Modifier.fillMaxSize().testTag(BotsTags.DETAIL_EMPTY), contentAlignment = Alignment.Center) {
                    BotsMessage(stringResource(R.string.remote_bots), icon = { Icon(Icons.Outlined.Computer, null, Modifier.size(40.dp), tint = MaterialTheme.colorScheme.onSurfaceVariant) })
                }
            }
        }
    }
    val root: @Composable (Modifier) -> Unit = { m ->
        BotsRootContent(
            state = rootState,
            loading = rootState == RootState.Tree && loadedPrefs == null,
            hostsError = hostsState.error != null && rootState == RootState.Tree,
            rows = rows,
            creating = creating,
            errors = errors,
            actions = actions,
            listState = listState,
            modifier = m,
        )
    }

    Box(
        modifier.fillMaxSize().background(MaterialTheme.colorScheme.background).windowInsetsPadding(WindowInsets.systemBars).imePadding().testTag(BotsTags.ROOT),
    ) {
        key(uid) {
            if (wide && uid != null) {
                Row(Modifier.fillMaxSize()) {
                    root(Modifier.width(360.dp).fillMaxHeight())
                    VerticalDivider()
                    detail(Modifier.weight(1f).fillMaxHeight())
                }
            } else if (stack.isEmpty() || uid == null) {
                root(Modifier.fillMaxSize())
            } else {
                detail(Modifier.fillMaxSize())
            }
        }
    }
}

/** The Bot a saved chat belongs to, when the connection has not said. */
private fun savedBotOf(snapshot: BotsSnapshot?, sessionId: String): String? =
    snapshot?.bots?.firstOrNull { bot -> bot.chats?.any { it.sessionId == sessionId } == true }?.botId

/** The title a saved chat had, for the session page while its computer is still being dialled. */
private fun savedChatTitle(snapshot: BotsSnapshot?, sessionId: String): String? =
    snapshot?.bots?.firstNotNullOfOrNull { bot -> bot.chats?.firstOrNull { it.sessionId == sessionId }?.title?.ifBlank { null } }

/** Waits until the connection to [hostId] is ready; false when it fails or does not come in time. */
private suspend fun RemoteControlManager.awaitReady(hostId: String): Boolean {
    val settled = withTimeoutOrNull(150_000L) {
        connection.first { it?.hostId == hostId && (it.phase == RemoteConnectionPhaseFfi.READY || it.phase == RemoteConnectionPhaseFfi.FAILED) }
    }
    return settled?.phase == RemoteConnectionPhaseFfi.READY
}

// --- the root: gate, spinner, errors, tree -----------------------------------------------------------------------------------

/** The root of the tab for [state] (section 4.5); the tree itself for [RootState.Tree]. */
@Composable
internal fun BotsRootContent(
    state: RootState,
    loading: Boolean,
    hostsError: Boolean,
    rows: List<TreeRow>,
    creating: List<String>,
    errors: Map<String, Int>,
    actions: BotsActions,
    listState: LazyListState,
    modifier: Modifier = Modifier,
) {
    Column(modifier) {
        Text(
            stringResource(R.string.remote_bots),
            style = MaterialTheme.typography.titleLarge,
            fontWeight = FontWeight.SemiBold,
            modifier = Modifier.padding(horizontal = 16.dp, vertical = 12.dp),
        )
        when {
            state == RootState.SignedOut -> Box(Modifier.fillMaxSize().testTag(BotsTags.SIGNED_OUT)) { RemoteSignedOut(actions.onSignIn) }
            state == RootState.Loading || loading -> Box(Modifier.fillMaxSize().testTag(BotsTags.LOADING), contentAlignment = Alignment.Center) {
                Column(horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.spacedBy(12.dp)) {
                    CircularProgressIndicator()
                    Text(stringResource(R.string.remote_loading), style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
                }
            }
            state == RootState.Error -> BotsMessage(
                title = stringResource(R.string.remote_hosts_error),
                modifier = Modifier.testTag(BotsTags.HOSTS_ERROR),
                icon = { Icon(Icons.Outlined.ErrorOutline, null, Modifier.size(40.dp), tint = MaterialTheme.colorScheme.onSurfaceVariant) },
                action = { Button(onClick = actions.onRetryHosts) { Text(stringResource(R.string.common_retry)) } },
            )
            state == RootState.NoComputers -> BotsMessage(
                title = stringResource(R.string.remote_no_computers),
                body = stringResource(R.string.remote_no_computers_help),
                modifier = Modifier.testTag(BotsTags.NO_COMPUTERS),
                icon = { Icon(Icons.Outlined.Computer, null, Modifier.size(40.dp), tint = MaterialTheme.colorScheme.onSurfaceVariant) },
            )
            else -> BotsTree(rows, hostsError, creating, errors, actions, listState, Modifier.fillMaxSize())
        }
    }
}

/** A centred message: an optional icon, a title, an optional body and an optional action. */
@Composable
private fun BotsMessage(
    title: String,
    modifier: Modifier = Modifier,
    body: String? = null,
    icon: (@Composable () -> Unit)? = null,
    action: (@Composable () -> Unit)? = null,
) {
    Column(modifier.fillMaxSize().padding(32.dp), verticalArrangement = Arrangement.Center, horizontalAlignment = Alignment.CenterHorizontally) {
        if (icon != null) { icon(); Spacer(Modifier.height(12.dp)) }
        Text(title, style = MaterialTheme.typography.titleMedium, textAlign = TextAlign.Center)
        if (body != null) {
            Spacer(Modifier.height(6.dp))
            Text(body, style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant, textAlign = TextAlign.Center)
        }
        if (action != null) { Spacer(Modifier.height(16.dp)); action() }
    }
}

// --- the tree --------------------------------------------------------------------------------------------------------------

/** The rows [buildTree] made, drawn. [hostsError] puts the "could not read the computers" line (with a retry) above them. */
@Composable
internal fun BotsTree(
    rows: List<TreeRow>,
    hostsError: Boolean,
    creating: List<String>,
    errors: Map<String, Int>,
    actions: BotsActions,
    listState: LazyListState,
    modifier: Modifier = Modifier,
) {
    val nowMs = rememberNowMs()
    LazyColumn(modifier.testTag(BotsTags.TREE), state = listState) {
        if (hostsError) item(key = "hosts-error") {
            Row(
                Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 4.dp).testTag(BotsTags.HOSTS_ERROR),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Text(stringResource(R.string.remote_hosts_error), Modifier.weight(1f), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.error)
                TextButton(onClick = actions.onRetryHosts) { Text(stringResource(R.string.common_retry)) }
            }
        }
        items(rows, key = { it.key }) { row ->
            when (row) {
                is TreeRow.HostHeader -> {
                    if (rows.first() !== row) HorizontalDivider()
                    HostHeaderRow(row, nowMs, actions)
                }
                is TreeRow.Connection -> ConnectionNote(row, actions)
                is TreeRow.Note -> NoteRow(row, nowMs)
                is TreeRow.State -> HostStateRow(row, actions)
                is TreeRow.Bot -> BotRow(row, nowMs, actions)
                is TreeRow.NewChat -> NewChatRow(row, BotsLogic.botKey(row.facts.hostId, row.botId) in creating, errors[BotsLogic.botKey(row.facts.hostId, row.botId)], actions)
                is TreeRow.BotState -> BotStateRow(row, actions)
                is TreeRow.Chat -> ChatRow(row, actions)
                is TreeRow.ShowMore -> TextRow(
                    stringResource(R.string.mobile_bots_show_more_fmt, row.hidden), BotsTags.showMore(row.hostId, row.botId),
                    onClick = { actions.onShowMore(row.hostId, row.botId) },
                )
                is TreeRow.ShowLess -> TextRow(
                    stringResource(R.string.chat_tool_show_less), BotsTags.showLess(row.hostId, row.botId),
                    onClick = { actions.onShowLess(row.hostId, row.botId) },
                )
            }
        }
    }
}

private val BotIndent = 16.dp
private val ChatIndent = 68.dp

@Composable
private fun HostHeaderRow(row: TreeRow.HostHeader, nowMs: Long, actions: BotsActions) {
    val host = row.host
    val t = LocalPalette.current
    val toggle = stringResource(if (row.collapsed) R.string.chat_run_expand else R.string.chat_run_collapse)
    Row(
        Modifier.fillMaxWidth()
            .clickable(onClickLabel = toggle) { actions.onToggleHost(host.hostId, !row.collapsed) }
            .padding(start = 16.dp, end = 4.dp, top = 6.dp, bottom = 6.dp)
            .testTag(BotsTags.host(host.hostId)),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(10.dp),
    ) {
        Icon(
            Icons.Outlined.KeyboardArrowDown, toggle,
            Modifier.size(20.dp).rotate(if (row.collapsed) -90f else 0f), tint = MaterialTheme.colorScheme.onSurfaceVariant,
        )
        Icon(Icons.Outlined.Computer, null, Modifier.size(24.dp), tint = MaterialTheme.colorScheme.onSurfaceVariant)
        Column(Modifier.weight(1f)) {
            Text(host.name, style = MaterialTheme.typography.titleSmall, fontWeight = FontWeight.SemiBold, maxLines = 1, overflow = TextOverflow.Ellipsis)
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                Box(Modifier.size(8.dp).clip(CircleShape).background(if (row.connected) t.statusConnected else if (host.online) t.ok else t.statusIdle))
                Text(
                    hostStatusLine(host, row.connected, nowMs),
                    style = MaterialTheme.typography.labelMedium, color = MaterialTheme.colorScheme.onSurfaceVariant, maxLines = 1, overflow = TextOverflow.Ellipsis,
                )
            }
        }
        IconButton(onClick = { actions.onOpenHost(host.hostId) }, modifier = Modifier.testTag(BotsTags.info(host.hostId))) {
            Icon(Icons.Outlined.Info, stringResource(R.string.mobile_bots_computer_details), tint = MaterialTheme.colorScheme.onSurfaceVariant)
        }
    }
}

/** "Connected", "Online", or "Offline · Last seen 2 days ago", then "· Not paired" for a computer this phone has not paired with. */
@Composable
private fun hostStatusLine(host: RemoteHostFfi, connected: Boolean, nowMs: Long): String {
    val state = when {
        connected -> stringResource(R.string.mobile_bots_connected)
        host.online -> stringResource(R.string.remote_online)
        else -> {
            val offline = stringResource(R.string.remote_offline)
            val seen = host.lastSeenMs.toLong().takeIf { it > 0 }
            if (seen == null) offline else offline + " · " + stringResource(R.string.mobile_bots_last_seen_fmt, ageText(seen, nowMs))
        }
    }
    return if (host.paired) state else state + " · " + stringResource(R.string.remote_not_paired)
}

/** A moment as "5 min ago", "Yesterday", "3 days ago" or a date, in the steps the session lists use. */
@Composable
internal fun ageText(atMs: Long, nowMs: Long): String = when (val age = ageOf(atMs, nowMs)) {
    Age.JustNow -> stringResource(R.string.project_relative_time_just_now)
    is Age.Minutes -> stringResource(R.string.session_relative_minutes_ago_fmt, age.n)
    is Age.Hours -> stringResource(R.string.session_relative_hours_ago_fmt, age.n)
    Age.Yesterday -> stringResource(R.string.session_relative_yesterday)
    is Age.Days -> stringResource(R.string.session_relative_days_ago_fmt, age.n)
    Age.Date -> {
        val date = Instant.ofEpochMilli(atMs).atZone(ZoneId.systemDefault())
        stringResource(R.string.session_relative_month_day_fmt, date.monthValue, date.dayOfMonth)
    }
}

/** The connection to this computer is not ready: dialling, waiting for its approval, dropped, or failed. */
@Composable
private fun ConnectionNote(row: TreeRow.Connection, actions: BotsActions) {
    val t = LocalPalette.current
    Column(
        Modifier.fillMaxWidth().padding(start = 50.dp, end = 16.dp, top = 4.dp, bottom = 8.dp).testTag(BotsTags.connection(row.hostId)),
        verticalArrangement = Arrangement.spacedBy(4.dp),
    ) {
        when (val ui = row.ui) {
            is RemoteConnectionUi.Connecting -> NoteText(
                if (ui.everReady) stringResource(R.string.remote_reconnecting) else stringResource(R.string.remote_connecting_fmt, row.hostName),
            )
            is RemoteConnectionUi.Pairing -> {
                Text(stringResource(R.string.remote_pairing_title), style = MaterialTheme.typography.labelLarge, fontWeight = FontWeight.SemiBold)
                NoteText(stringResource(R.string.remote_pairing_body_fmt, row.hostName))
                Text(formatPairingCode(ui.code), style = MaterialTheme.typography.headlineSmall, fontFamily = FontFamily.Monospace, fontWeight = FontWeight.Bold)
                NoteText(stringResource(R.string.remote_pairing_waiting))
            }
            is RemoteConnectionUi.Reconnecting -> {
                NoteText(stringResource(R.string.remote_reconnecting))
                ui.reason?.let { NoteText(stringResource(it)) }
                rememberCountdown(ui.retryAtMs)?.let { NoteText(stringResource(R.string.remote_retry_in_fmt, it)) }
            }
            is RemoteConnectionUi.Failed -> {
                Text(stringResource(ui.message), style = MaterialTheme.typography.bodySmall, color = t.danger)
                Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    if (ui.repair) OutlinedButton(onClick = { actions.onRepair(row.hostId) }) { Text(stringResource(R.string.remote_pair_again)) }
                    else if (ui.canRetry) OutlinedButton(onClick = actions.onRetryConnection) { Text(stringResource(R.string.common_retry)) }
                }
            }
            RemoteConnectionUi.Idle, is RemoteConnectionUi.Ready -> Unit
        }
    }
}

@Composable
private fun NoteText(text: String) {
    Text(text, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
}

/** The note under a computer that shows what was saved: when it was, and that its chats cannot be opened while it is offline. */
@Composable
private fun NoteRow(row: TreeRow.Note, nowMs: Long) {
    val text = when (row.note) {
        BodyNote.SavedNote -> row.savedAtMs?.let { stringResource(R.string.mobile_bots_saved_note_fmt, ageText(it, nowMs)) }
        BodyNote.OfflineChatsNote -> stringResource(R.string.mobile_bots_offline_chats_note)
    } ?: return
    Box(Modifier.padding(start = 50.dp, end = 16.dp, top = 2.dp, bottom = 6.dp).testTag(BotsTags.note(row.hostId, row.note))) { NoteText(text) }
}

/** A section with no Bots to list: why, and the way to get some. */
@Composable
private fun HostStateRow(row: TreeRow.State, actions: BotsActions) {
    val title: String
    var body: String? = null
    var connect = false
    when (row.kind) {
        BodyKind.Loading -> title = stringResource(R.string.remote_loading)
        BodyKind.Unsupported -> title = stringResource(R.string.mobile_bots_unsupported)
        BodyKind.NoBots -> title = stringResource(R.string.mobile_bots_none)
        BodyKind.ConnectToLoad -> { title = stringResource(R.string.mobile_bots_connect_to_load); connect = true }
        BodyKind.OfflineNoSnapshot -> { title = stringResource(R.string.remote_offline); body = stringResource(R.string.remote_offline_help) }
        BodyKind.NotPaired -> { title = stringResource(R.string.remote_not_paired); connect = true }
        BodyKind.Bots -> return
    }
    Column(
        Modifier.fillMaxWidth().padding(start = 50.dp, end = 16.dp, top = 4.dp, bottom = 10.dp).testTag(BotsTags.state(row.hostId)),
        verticalArrangement = Arrangement.spacedBy(6.dp),
    ) {
        Text(title, style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
        body?.let { Text(it, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant) }
        if (connect) Button(onClick = { actions.onConnect(row.hostId) }, modifier = Modifier.testTag(BotsTags.connect(row.hostId))) { Text(stringResource(R.string.remote_connect)) }
    }
}

@Composable
private fun BotRow(row: TreeRow.Bot, nowMs: Long, actions: BotsActions) {
    val bot = row.bot
    val toggle = stringResource(if (row.expanded) R.string.chat_run_collapse else R.string.chat_run_expand)
    val waitingText = if (bot.waitingThreads > 0) stringResource(R.string.remote_bot_waiting_fmt, bot.waitingThreads) else null
    Row(
        Modifier.fillMaxWidth()
            .alpha(if (row.dimmed) DIMMED else 1f)
            .clickable(onClickLabel = toggle) { actions.onToggleBot(row) }
            .padding(start = BotIndent, end = 8.dp, top = 8.dp, bottom = 8.dp)
            .testTag(BotsTags.bot(row.facts.hostId, bot.botId)),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        BotAvatar(bot.botId, 40.dp)
        Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                Text(bot.name, Modifier.weight(1f), style = MaterialTheme.typography.bodyLarge, fontWeight = FontWeight.Medium, maxLines = 1, overflow = TextOverflow.Ellipsis)
                bot.lastActivityAtMs?.let {
                    Text(listTimeText(it, nowMs), Modifier.padding(start = 8.dp), style = MaterialTheme.typography.labelSmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                }
            }
            Row(verticalAlignment = Alignment.CenterVertically) {
                Text(
                    bot.description, Modifier.weight(1f), style = MaterialTheme.typography.bodyMedium,
                    color = MaterialTheme.colorScheme.onSurfaceVariant, maxLines = 1, overflow = TextOverflow.Ellipsis,
                )
                BotStatus(bot.status)
            }
        }
        if (bot.waitingThreads > 0) WaitingBadge(bot.waitingThreads, waitingText.orEmpty())
        Icon(
            Icons.Outlined.KeyboardArrowDown, toggle,
            Modifier.size(20.dp).rotate(if (row.expanded) 0f else -90f), tint = MaterialTheme.colorScheme.onSurfaceVariant,
        )
    }
}

private const val DIMMED = 0.6f

/** Running with a spinner, "needs you" in the accent colour, idle quietly. */
@Composable
private fun BotStatus(status: RemoteBotStatusFfi) {
    val label = stringResource(botStatusText(status))
    when (status) {
        RemoteBotStatusFfi.RUNNING -> Row(
            Modifier.padding(start = 8.dp).testTag(BotsTags.BOT_RUNNING),
            verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(4.dp),
        ) {
            CircularProgressIndicator(Modifier.size(10.dp), strokeWidth = 1.5.dp)
            Text(label, style = MaterialTheme.typography.labelSmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
        }
        RemoteBotStatusFfi.REQUIRES_ACTION -> Text(
            label, Modifier.padding(start = 8.dp),
            style = MaterialTheme.typography.labelSmall, color = MaterialTheme.colorScheme.primary, fontWeight = FontWeight.Medium,
        )
        RemoteBotStatusFfi.IDLE -> Text(
            label, Modifier.padding(start = 8.dp),
            style = MaterialTheme.typography.labelSmall, color = MaterialTheme.colorScheme.onSurfaceVariant,
        )
    }
}

/** How many chats of the Bot wait for the user; read out as "N waiting for you". */
@Composable
private fun WaitingBadge(count: Int, description: String) {
    val text = TabLogic.tabBadgeText(count) ?: return
    Box(
        Modifier.size(width = 22.dp, height = 22.dp).clip(CircleShape).background(MaterialTheme.colorScheme.primary).semantics { contentDescription = description },
        contentAlignment = Alignment.Center,
    ) { Text(text, style = MaterialTheme.typography.labelSmall, color = MaterialTheme.colorScheme.onPrimary, fontWeight = FontWeight.SemiBold, maxLines = 1) }
}

@Composable
private fun NewChatRow(row: TreeRow.NewChat, creating: Boolean, error: Int?, actions: BotsActions) {
    val hostId = row.facts.hostId
    Column(Modifier.fillMaxWidth()) {
        Row(
            Modifier.fillMaxWidth()
                .alpha(if (row.enabled) 1f else DIMMED)
                .clickable(enabled = row.enabled && !creating) { actions.onNewChat(row) }
                .padding(start = ChatIndent, end = 16.dp, top = 8.dp, bottom = 8.dp)
                .testTag(BotsTags.newChat(hostId, row.botId)),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            if (creating) CircularProgressIndicator(Modifier.size(18.dp), strokeWidth = 2.dp)
            else Icon(Icons.Outlined.Add, null, Modifier.size(18.dp), tint = MaterialTheme.colorScheme.primary)
            Text(stringResource(R.string.remote_bot_new_chat), style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.primary, fontWeight = FontWeight.Medium)
        }
        error?.let {
            Text(
                stringResource(it), Modifier.padding(start = ChatIndent, end = 16.dp, bottom = 6.dp).testTag(BotsTags.chatError(hostId, row.botId)),
                style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.error,
            )
        }
    }
}

@Composable
private fun BotStateRow(row: TreeRow.BotState, actions: BotsActions) {
    Row(
        Modifier.fillMaxWidth().padding(start = ChatIndent, end = 16.dp, top = 4.dp, bottom = 6.dp).testTag(BotsTags.botState(row.hostId, row.botId)),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        val text = when (row.state) {
            BotBodyState.ChatsLoading -> R.string.mobile_bots_chats_loading
            BotBodyState.ChatsFailed -> R.string.mobile_bots_chats_failed
            BotBodyState.NoChats -> R.string.remote_bot_no_chats
            BotBodyState.Gone -> R.string.remote_bot_gone
        }
        Text(
            stringResource(text), Modifier.weight(1f), style = MaterialTheme.typography.bodySmall,
            color = if (row.state == BotBodyState.ChatsFailed) MaterialTheme.colorScheme.error else MaterialTheme.colorScheme.onSurfaceVariant,
        )
        if (row.state == BotBodyState.ChatsFailed) TextButton(onClick = { actions.onRetryChats(row.hostId, row.botId) }) { Text(stringResource(R.string.common_retry)) }
    }
}

@Composable
private fun ChatRow(row: TreeRow.Chat, actions: BotsActions) {
    val chat = row.chat
    Row(
        Modifier.fillMaxWidth()
            .alpha(if (row.dimmed) DIMMED else 1f)
            .clickable { actions.onOpenChat(row) }
            .padding(start = ChatIndent, end = 16.dp, top = 8.dp, bottom = 8.dp)
            .testTag(BotsTags.chat(row.facts.hostId, row.botId, chat.sessionId)),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        Text(
            chat.title.ifBlank { stringResource(R.string.remote_untitled) }, Modifier.weight(1f),
            style = MaterialTheme.typography.bodyMedium, maxLines = 1, overflow = TextOverflow.Ellipsis,
        )
        when {
            chat.pendingAction != null -> Text(
                stringResource(pendingActionText(chat.pendingAction)),
                style = MaterialTheme.typography.labelSmall, color = MaterialTheme.colorScheme.primary, fontWeight = FontWeight.Medium,
            )
            chat.status == RemoteSessionStatusFfi.RUNNING -> Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                CircularProgressIndicator(Modifier.size(10.dp), strokeWidth = 1.5.dp)
                Text(stringResource(sessionStatusText(chat.status)), style = MaterialTheme.typography.labelSmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
            }
            chat.status == RemoteSessionStatusFfi.REQUIRES_ACTION -> Text(
                stringResource(sessionStatusText(chat.status)),
                style = MaterialTheme.typography.labelSmall, color = MaterialTheme.colorScheme.primary, fontWeight = FontWeight.Medium,
            )
            else -> Unit
        }
    }
}

@Composable
private fun TextRow(text: String, tag: String, onClick: () -> Unit) {
    Text(
        text,
        Modifier.fillMaxWidth().clickable(onClick = onClick).padding(start = ChatIndent, end = 16.dp, top = 8.dp, bottom = 8.dp).testTag(tag),
        style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.primary,
    )
}
