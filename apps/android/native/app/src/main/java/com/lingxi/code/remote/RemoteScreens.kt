package com.lingxi.code.remote

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
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Button
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.lingxi.code.R
import com.lingxi.code.bindings.android.RemoteBotSummaryFfi
import com.lingxi.code.bindings.android.RemoteFailureKindFfi
import com.lingxi.code.bindings.android.RemoteHostFfi
import com.lingxi.code.bindings.android.RemoteLocalAppFfi
import com.lingxi.code.apps.LocalAppBrowser
import com.lingxi.code.bindings.android.RemotePendingActionFfi
import com.lingxi.code.bindings.android.RemoteSessionStatusFfi
import com.lingxi.code.bindings.android.RemoteSessionSummaryFfi
import com.lingxi.code.bindings.android.RemoteTransportFfi
import com.lingxi.code.components.LXIcon
import com.lingxi.code.components.LXIconName
import com.lingxi.code.theme.LingXiTheme
import kotlinx.coroutines.delay
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.launch

/** A page of the Remote Control screens: a back arrow and a title above [content]. */
@Composable
internal fun RemotePage(title: String, onBack: () -> Unit, content: @Composable () -> Unit) {
    val t = LingXiTheme.palette
    Column(Modifier.fillMaxSize().background(t.windowBg)) {
        Row(Modifier.fillMaxWidth().padding(horizontal = 4.dp, vertical = 4.dp), verticalAlignment = Alignment.CenterVertically) {
            TextButton(onClick = onBack) {
                LXIcon(LXIconName.X, size = 20.dp, color = t.text2, contentDescription = stringResource(R.string.remote_back))
            }
            Text(title, color = t.text, fontSize = 18.sp, fontWeight = FontWeight.SemiBold, modifier = Modifier.weight(1f))
        }
        content()
    }
}

/** Asks the user to sign in to the account the computers are registered under. */
@Composable
internal fun RemoteSignedOut(onOpenAccount: () -> Unit) {
    val t = LingXiTheme.palette
    Column(Modifier.fillMaxWidth().padding(24.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
        Text(stringResource(R.string.remote_signed_out_title), color = t.text, fontSize = 18.sp, fontWeight = FontWeight.SemiBold)
        Text(stringResource(R.string.remote_signed_out_body), color = t.text2, fontSize = 14.sp)
        Button(onClick = onOpenAccount) { Text(stringResource(R.string.remote_sign_in)) }
    }
}

/** The user's computers: online state, paired marker, connect and forget. */
@Composable
internal fun RemoteHostList(manager: RemoteControlManager, onConnect: (String) -> Unit) {
    val t = LingXiTheme.palette
    val state by manager.hosts.collectAsState()
    val scope = rememberCoroutineScope()
    LaunchedEffect(manager) { runCatching { manager.watchHosts() } }
    DisposableEffect(manager) { onDispose { scope.launch { runCatching { manager.stopWatchingHosts() } } } }
    var forgetting by remember { mutableStateOf<RemoteHostFfi?>(null) }
    Column(Modifier.fillMaxSize().padding(horizontal = 16.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
        Text(stringResource(R.string.remote_computers_intro), color = t.text2, fontSize = 14.sp)
        when {
            state.error != null -> Text(stringResource(R.string.remote_hosts_error), color = t.danger, fontSize = 14.sp)
            !state.loaded -> Text(stringResource(R.string.remote_loading), color = t.text3, fontSize = 14.sp)
            state.hosts.isEmpty() -> Column(verticalArrangement = Arrangement.spacedBy(6.dp)) {
                Text(stringResource(R.string.remote_no_computers), color = t.text, fontWeight = FontWeight.Medium)
                Text(stringResource(R.string.remote_no_computers_help), color = t.text2, fontSize = 14.sp)
            }
        }
        LazyColumn(verticalArrangement = Arrangement.spacedBy(8.dp)) {
            items(state.hosts, key = { it.hostId }) { host ->
                RemoteHostRow(host, onConnect = { onConnect(host.hostId) }, onForget = { forgetting = host })
            }
        }
    }
    forgetting?.let { host ->
        androidx.compose.material3.AlertDialog(
            onDismissRequest = { forgetting = null },
            title = { Text(stringResource(R.string.remote_forget)) },
            text = { Text(stringResource(R.string.remote_forget_help)) },
            confirmButton = {
                TextButton(onClick = { forgetting = null; scope.launch { runCatching { manager.forget(host.hostId) } } }) {
                    Text(stringResource(R.string.remote_forget))
                }
            },
            dismissButton = { TextButton(onClick = { forgetting = null }) { Text(stringResource(R.string.common_cancel)) } },
        )
    }
}

@Composable
private fun RemoteHostRow(host: RemoteHostFfi, onConnect: () -> Unit, onForget: () -> Unit) {
    val t = LingXiTheme.palette
    Column(
        Modifier.fillMaxWidth().clip(RoundedCornerShape(12.dp)).background(t.surface).padding(14.dp),
        verticalArrangement = Arrangement.spacedBy(4.dp),
    ) {
        Text(host.name, color = t.text, fontWeight = FontWeight.SemiBold, fontSize = 16.sp)
        Row(horizontalArrangement = Arrangement.spacedBy(6.dp), verticalAlignment = Alignment.CenterVertically) {
            Text(platformLabel(host.platform), color = t.text3, fontSize = 13.sp)
            Text("·", color = t.text4)
            Text(
                stringResource(if (host.online) R.string.remote_online else R.string.remote_offline),
                color = if (host.online) t.statusConnected else t.text3, fontSize = 13.sp,
            )
            Text("·", color = t.text4)
            Text(stringResource(if (host.paired) R.string.remote_paired else R.string.remote_not_paired), color = t.text3, fontSize = 13.sp)
        }
        if (!host.online) Text(stringResource(R.string.remote_offline_help), color = t.text3, fontSize = 13.sp)
        Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            if (host.online) {
                Button(onClick = onConnect) { Text(stringResource(R.string.remote_connect)) }
            } else {
                OutlinedButton(onClick = onConnect) { Text(stringResource(R.string.remote_try_anyway)) }
            }
            if (host.paired) TextButton(onClick = onForget) { Text(stringResource(R.string.remote_forget)) }
        }
    }
}

private fun platformLabel(platform: String) = when (platform) {
    "darwin" -> "macOS"
    "win32" -> "Windows"
    "linux" -> "Linux"
    else -> platform
}

/**
 * One computer: while it connects, the pairing code or the reason it stopped;
 * once connected, its Bots (when it has any) and its shared sessions.
 */
@Composable
internal fun RemoteHostScreen(
    manager: RemoteControlManager,
    hostId: String,
    hostName: String,
    onOpenBot: (String) -> Unit,
    onOpenSession: (RemoteSessionSummaryFfi) -> Unit,
    onLeave: () -> Unit,
) {
    val connection by manager.connection.collectAsState()
    val sessions by manager.sessions.collectAsState()
    val bots by manager.bots.collectAsState()
    val scope = rememberCoroutineScope()
    LaunchedEffect(hostId) { runCatching { manager.connect(hostId) } }
    val ui = remoteConnectionUi(connection?.takeIf { it.hostId == hostId })
    when (ui) {
        is RemoteConnectionUi.Ready -> RemoteAppsSessionList(
            manager = manager, hostId = hostId, attempt = connection?.attempt,
            transport = ui.transport,
            bots = bots.listFor(hostId),
            sessions = sessions.takeIf { it.hostId == hostId }?.sessions,
            onOpenBot = { onOpenBot(it.botId) },
            onOpenSession = onOpenSession,
        )
        is RemoteConnectionUi.Reconnecting -> RemoteConnecting(
            title = stringResource(R.string.remote_reconnecting),
            reason = ui.reason,
            retryAtMs = ui.retryAtMs,
            onCancel = onLeave,
        )
        is RemoteConnectionUi.Pairing -> RemotePairing(hostName, ui.code, onCancel = onLeave)
        is RemoteConnectionUi.Failed -> RemoteFailed(
            hostName = hostName,
            failure = ui,
            onRetry = { scope.launch { runCatching { manager.retry() } } },
            onRepair = { scope.launch { runCatching { manager.repair(hostId) } } },
            onForget = { scope.launch { runCatching { manager.forget(hostId) }; onLeave() } },
            onLeave = onLeave,
        )
        is RemoteConnectionUi.Connecting, RemoteConnectionUi.Idle -> RemoteConnecting(
            title = if ((ui as? RemoteConnectionUi.Connecting)?.everReady == true) {
                stringResource(R.string.remote_reconnecting)
            } else {
                stringResource(R.string.remote_connecting_fmt, hostName)
            },
            reason = null,
            retryAtMs = null,
            onCancel = onLeave,
        )
    }
}

@Composable
private fun RemoteConnecting(title: String, reason: Int?, retryAtMs: Long?, onCancel: () -> Unit) {
    val t = LingXiTheme.palette
    val seconds = rememberCountdown(retryAtMs)
    Column(Modifier.fillMaxWidth().padding(24.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
        Text(title, color = t.text, fontSize = 18.sp, fontWeight = FontWeight.SemiBold)
        reason?.let { Text(stringResource(it), color = t.text3, fontSize = 14.sp) }
        seconds?.let { Text(stringResource(R.string.remote_retry_in_fmt, it), color = t.text3, fontSize = 14.sp) }
        OutlinedButton(onClick = onCancel) { Text(stringResource(R.string.common_cancel)) }
    }
}

/** Whole seconds until [retryAtMs] (epoch milliseconds), counting down while shown. */
@Composable
private fun rememberCountdown(retryAtMs: Long?): Int? {
    var now by remember(retryAtMs) { mutableStateOf(System.currentTimeMillis()) }
    LaunchedEffect(retryAtMs) {
        if (retryAtMs == null) return@LaunchedEffect
        while (true) {
            now = System.currentTimeMillis()
            delay(500)
        }
    }
    return retryAtMs?.let { maxOf(0L, (it - now + 999) / 1000).toInt() }
}

/** The code the computer's dialog shows: the user compares and allows there. */
@Composable
private fun RemotePairing(hostName: String, code: String, onCancel: () -> Unit) {
    val t = LingXiTheme.palette
    Column(
        Modifier.fillMaxWidth().padding(24.dp),
        horizontalAlignment = Alignment.CenterHorizontally,
        verticalArrangement = Arrangement.spacedBy(10.dp),
    ) {
        Text(stringResource(R.string.remote_pairing_title), color = t.text, fontSize = 18.sp, fontWeight = FontWeight.SemiBold)
        Text(stringResource(R.string.remote_pairing_body_fmt, hostName), color = t.text2, fontSize = 14.sp, textAlign = TextAlign.Center)
        Text(stringResource(R.string.remote_pairing_code), color = t.text3, fontSize = 13.sp)
        Text(
            formatPairingCode(code),
            color = t.text, fontSize = 40.sp, fontWeight = FontWeight.Bold, fontFamily = FontFamily.Monospace,
        )
        Text(stringResource(R.string.remote_pairing_waiting), color = t.text3, fontSize = 14.sp)
        OutlinedButton(onClick = onCancel) { Text(stringResource(R.string.common_cancel)) }
    }
}

@Composable
private fun RemoteFailed(
    hostName: String,
    failure: RemoteConnectionUi.Failed,
    onRetry: () -> Unit,
    onRepair: () -> Unit,
    onForget: () -> Unit,
    onLeave: () -> Unit,
) {
    val t = LingXiTheme.palette
    Column(Modifier.fillMaxWidth().padding(24.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
        Text(hostName, color = t.text, fontSize = 18.sp, fontWeight = FontWeight.SemiBold)
        Text(stringResource(failure.message), color = if (failure.kind == RemoteFailureKindFfi.DENIED) t.text2 else t.danger, fontSize = 14.sp)
        if (failure.repair) Text(stringResource(R.string.remote_pair_again_note), color = t.text3, fontSize = 13.sp)
        Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            when {
                failure.repair -> Button(onClick = onRepair) { Text(stringResource(R.string.remote_pair_again)) }
                failure.forget -> OutlinedButton(onClick = onForget) { Text(stringResource(R.string.remote_forget)) }
                failure.canRetry -> Button(onClick = onRetry) { Text(stringResource(R.string.common_retry)) }
            }
            OutlinedButton(onClick = onLeave) { Text(stringResource(R.string.remote_switch_computer)) }
        }
    }
}

/** Apps owned by the desktop, shown in the same scrolling list as Bots and sessions. */
@Composable
private fun RemoteAppsSessionList(
    manager: RemoteControlManager,
    hostId: String,
    attempt: UInt?,
    transport: RemoteTransportFfi?,
    bots: List<RemoteBotSummaryFfi>,
    sessions: List<RemoteSessionSummaryFfi>?,
    onOpenBot: (RemoteBotSummaryFfi) -> Unit,
    onOpenSession: (RemoteSessionSummaryFfi) -> Unit,
) {
    val t = LingXiTheme.palette
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    var apps by remember(hostId, attempt) { mutableStateOf<List<RemoteLocalAppFfi>?>(null) }
    var appError by remember(hostId, attempt) { mutableStateOf(false) }
    var busy by remember(hostId, attempt) { mutableStateOf(false) }
    var refresh by remember(hostId, attempt) { mutableStateOf(0) }
    LaunchedEffect(hostId, attempt, refresh) {
        apps = null; appError = false
        try { apps = manager.listApps(hostId) }
        catch (cancelled: CancellationException) { throw cancelled }
        catch (_: Exception) { appError = true }
    }
    fun openApp(app: RemoteLocalAppFfi, choose: Boolean) {
        if (busy) return
        scope.launch {
            busy = true; appError = false
            try { LocalAppBrowser.open(context, manager.openApp(hostId, app.appId, app.revision), choose) }
            catch (cancelled: CancellationException) { throw cancelled }
            catch (_: Exception) { appError = true }
            finally { busy = false }
        }
    }
    RemoteSessionList(
        transport, bots, sessions, onOpenBot, onOpenSession,
        sessionsTitle = stringResource(R.string.remote_sessions_title),
        localApps = {
            item {
                Column(verticalArrangement = Arrangement.spacedBy(4.dp)) {
                    Row(verticalAlignment = Alignment.CenterVertically) {
                        Text(stringResource(R.string.remote_apps_title), color = t.text, fontWeight = FontWeight.SemiBold, modifier = Modifier.weight(1f))
                        TextButton(onClick = { refresh++ }, enabled = !busy) { Text(stringResource(R.string.local_app_v2_refresh)) }
                    }
                    Text(stringResource(R.string.remote_apps_help), color = t.text3, fontSize = 13.sp)
                    if (appError) Text(stringResource(R.string.remote_apps_error), color = t.danger, fontSize = 13.sp)
                    else if (apps == null) Text(stringResource(R.string.remote_loading), color = t.text3, fontSize = 13.sp)
                    else if (apps!!.isEmpty()) Text(stringResource(R.string.remote_apps_empty), color = t.text3, fontSize = 13.sp)
                }
            }
            items(apps.orEmpty(), key = { "app:" + it.appId }) { app ->
                Column(Modifier.fillMaxWidth().clip(RoundedCornerShape(12.dp)).background(t.surface).padding(14.dp)) {
                    Text(app.name, color = t.text, fontWeight = FontWeight.SemiBold)
                    if (app.summary.isNotBlank()) Text(app.summary, color = t.text2, fontSize = 13.sp)
                    Text(stringResource(R.string.remote_app_revision, app.revision.toString()), color = t.text3, fontSize = 12.sp)
                    Row {
                        TextButton(enabled = !busy, onClick = { openApp(app, false) }) { Text(stringResource(R.string.local_app_v2_open)) }
                        TextButton(enabled = !busy, onClick = { openApp(app, true) }) { Text(stringResource(R.string.remote_apps_choose)) }
                    }
                }
            }
        },
    )
}

/**
 * The connected computer's Bots, when it has any to show, above its shared sessions with status and
 * what each waits on. A computer that does not know Bots, or has none, looks as it always did.
 */
@Composable
internal fun RemoteSessionList(
    transport: RemoteTransportFfi?,
    bots: List<RemoteBotSummaryFfi>,
    sessions: List<RemoteSessionSummaryFfi>?,
    onOpenBot: (RemoteBotSummaryFfi) -> Unit,
    onOpenSession: (RemoteSessionSummaryFfi) -> Unit,
    localApps: androidx.compose.foundation.lazy.LazyListScope.() -> Unit = {},
    sessionsTitle: String? = null,
) {
    val t = LingXiTheme.palette
    Column(Modifier.fillMaxSize().padding(horizontal = 16.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
        transport?.let {
            Text(
                stringResource(if (it == RemoteTransportFfi.WEBRTC) R.string.remote_direct_help else R.string.remote_tunnel_help),
                color = t.text3, fontSize = 12.sp,
            )
        }
        LazyColumn(verticalArrangement = Arrangement.spacedBy(8.dp)) {
            localApps()
            if (bots.isNotEmpty()) {
                item(key = "bots") {
                    Text(stringResource(R.string.remote_bots), color = t.text3, fontSize = 13.sp, fontWeight = FontWeight.Medium)
                }
                items(bots, key = { "bot/${it.botId}" }) { bot -> RemoteBotRow(bot, onOpenBot) }
                item(key = "bots-end") { Spacer(Modifier.height(4.dp)) }
            }
            if (sessionsTitle != null) {
                item(key = "sessions-title") {
                    Text(sessionsTitle, color = t.text, fontWeight = FontWeight.SemiBold)
                }
            }
            when {
                sessions == null -> item(key = "loading") {
                    Text(stringResource(R.string.remote_loading), color = t.text3, fontSize = 14.sp)
                }
                sessions.isEmpty() -> item(key = "empty") {
                    Column(verticalArrangement = Arrangement.spacedBy(6.dp)) {
                        Text(stringResource(R.string.remote_no_sessions), color = t.text, fontWeight = FontWeight.Medium)
                        Text(stringResource(R.string.remote_no_sessions_help), color = t.text2, fontSize = 14.sp)
                    }
                }
            }
            items(sessions.orEmpty(), key = { it.sessionId }) { session -> RemoteSessionRow(session, onOpenSession) }
        }
    }
}

/** One session, or one chat of a Bot: title, where and with which model, what it waits on, its status. */
@Composable
internal fun RemoteSessionRow(session: RemoteSessionSummaryFfi, onOpen: (RemoteSessionSummaryFfi) -> Unit) {
    val t = LingXiTheme.palette
    Row(
        Modifier.fillMaxWidth().clip(RoundedCornerShape(12.dp)).background(t.surface)
            .clickable { onOpen(session) }.padding(14.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(3.dp)) {
            Text(
                session.title.ifBlank { stringResource(R.string.remote_untitled) },
                color = t.text, fontWeight = FontWeight.SemiBold, fontSize = 16.sp, maxLines = 2,
            )
            Text(
                listOfNotNull(session.projectLabel.ifBlank { null }, session.model).joinToString(" · "),
                color = t.text3, fontSize = 13.sp, maxLines = 1,
            )
            session.pendingAction?.let {
                Text(stringResource(pendingActionText(it)), color = t.accent, fontSize = 13.sp, fontWeight = FontWeight.Medium)
            }
        }
        Text(stringResource(sessionStatusText(session.status)), color = t.text3, fontSize = 13.sp)
    }
}

/** One Bot of the computer: its name, what it is for, how many chats wait on the user, its status. */
@Composable
internal fun RemoteBotRow(bot: RemoteBotSummaryFfi, onOpen: (RemoteBotSummaryFfi) -> Unit) {
    val t = LingXiTheme.palette
    Row(
        Modifier.fillMaxWidth().clip(RoundedCornerShape(12.dp)).background(t.surface)
            .clickable { onOpen(bot) }.padding(14.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(3.dp)) {
            Text(bot.name, color = t.text, fontWeight = FontWeight.SemiBold, fontSize = 16.sp, maxLines = 2)
            if (bot.description.isNotBlank()) {
                Text(bot.description, color = t.text3, fontSize = 13.sp, maxLines = 1, overflow = TextOverflow.Ellipsis)
            }
            if (bot.waitingThreads > 0u) {
                Text(
                    stringResource(R.string.remote_bot_waiting_fmt, bot.waitingThreads.toInt()),
                    color = t.accent, fontSize = 13.sp, fontWeight = FontWeight.Medium,
                )
            }
        }
        Text(stringResource(botStatusText(bot.status)), color = t.text3, fontSize = 13.sp)
    }
}

internal fun pendingActionText(action: RemotePendingActionFfi): Int = when (action) {
    RemotePendingActionFfi.PERMISSION -> R.string.remote_needs_permission
    RemotePendingActionFfi.QUESTION -> R.string.remote_needs_question
    RemotePendingActionFfi.COMPUTER_ACCESS -> R.string.remote_needs_computer_access
}

internal fun sessionStatusText(status: RemoteSessionStatusFfi): Int = when (status) {
    RemoteSessionStatusFfi.RUNNING -> R.string.remote_status_running
    RemoteSessionStatusFfi.REQUIRES_ACTION -> R.string.remote_status_requires_action
    RemoteSessionStatusFfi.IDLE -> R.string.remote_status_idle
    RemoteSessionStatusFfi.DORMANT -> R.string.remote_status_dormant
}
