package com.lingxi.code.remote

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.LazyListScope
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Button
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
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
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.lingxi.code.R
import com.lingxi.code.apps.LocalAppBrowser
import com.lingxi.code.bindings.android.RemoteFailureKindFfi
import com.lingxi.code.bindings.android.RemoteHostFfi
import com.lingxi.code.bindings.android.RemoteLocalAppFfi
import com.lingxi.code.bindings.android.RemotePendingActionFfi
import com.lingxi.code.bindings.android.RemoteSessionStatusFfi
import com.lingxi.code.bindings.android.RemoteSessionSummaryFfi
import com.lingxi.code.bindings.android.RemoteTransportFfi
import com.lingxi.code.components.PageTopBar
import com.lingxi.code.theme.LingXiTheme
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch

/** Test tags of the Computer page (this slice owns them). */
internal object RemoteTags {
    const val HOST_PAGE = "remote.hostPage"
    const val CONNECT = "remote.connect"
    const val PAIR_AGAIN = "remote.pairAgain"
    const val FORGET = "remote.forget"
    const val SESSIONS = "remote.sessions"
}

/** A page of the Bots tab above the tree: an app bar with a back arrow and a title over [content]. */
@Composable
internal fun RemotePage(title: String, onBack: () -> Unit, content: @Composable () -> Unit) {
    val t = LingXiTheme.palette
    Column(Modifier.fillMaxSize().background(t.windowBg)) {
        PageTopBar(title = title, onBack = onBack)
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

internal fun platformLabel(platform: String) = when (platform) {
    "darwin" -> "macOS"
    "win32" -> "Windows"
    "linux" -> "Linux"
    else -> platform
}

/**
 * The Computer page (section 4.6): one computer's connection (while it connects the pairing code, if it
 * stopped the reason and a retry), once connected its shared sessions and apps, and the actions on the pairing.
 * The Bots are the tree's. Opening the page connects to nothing; [onConnect] is the user's to press.
 */
@Composable
internal fun RemoteHostScreen(
    manager: RemoteControlManager,
    hostId: String,
    hostName: String,
    host: RemoteHostFfi?,
    onOpenSession: (RemoteSessionSummaryFfi) -> Unit,
    onLeave: () -> Unit,
) {
    val connection by manager.connection.collectAsState()
    val sessions by manager.sessions.collectAsState()
    val scope = rememberCoroutineScope()
    var forgetting by remember { mutableStateOf(false) }
    val ui = remoteConnectionUi(connection?.takeIf { it.hostId == hostId })
    val paired = host?.paired == true
    val repair: () -> Unit = { scope.launch { runCatching { manager.repair(hostId) } } }
    val connect: () -> Unit = { scope.launch { runCatching { manager.connect(hostId) } } }
    val failed = ui as? RemoteConnectionUi.Failed
    val actions: LazyListScope.() -> Unit = {
        remoteHostActions(
            paired = paired,
            showRepair = failed?.repair != true,
            showForget = failed?.forget != true,
            onRepair = repair,
            onForget = { forgetting = true },
        )
    }
    when (ui) {
        is RemoteConnectionUi.Ready -> RemoteAppsSessionList(
            manager = manager, hostId = hostId, attempt = connection?.attempt,
            transport = ui.transport,
            sessions = sessions.takeIf { it.hostId == hostId }?.sessions,
            onOpenSession = onOpenSession,
            footer = actions,
        )
        is RemoteConnectionUi.Reconnecting -> RemoteStatePage(actions) {
            RemoteConnecting(
                title = stringResource(R.string.remote_reconnecting),
                reason = ui.reason,
                retryAtMs = ui.retryAtMs,
                onCancel = onLeave,
            )
        }
        is RemoteConnectionUi.Pairing -> RemoteStatePage(actions) { RemotePairing(hostName, ui.code, onCancel = onLeave) }
        is RemoteConnectionUi.Failed -> RemoteStatePage(actions) {
            RemoteFailed(
                hostName = hostName,
                failure = ui,
                onRetry = { scope.launch { runCatching { manager.retry() } } },
                onRepair = repair,
                onForget = { forgetting = true },
            )
        }
        is RemoteConnectionUi.Connecting -> RemoteStatePage(actions) {
            RemoteConnecting(
                title = if (ui.everReady) stringResource(R.string.remote_reconnecting) else stringResource(R.string.remote_connecting_fmt, hostName),
                reason = null,
                retryAtMs = null,
                onCancel = onLeave,
            )
        }
        RemoteConnectionUi.Idle -> RemoteStatePage(actions) { RemoteNotConnected(hostName, host, onConnect = connect) }
    }
    if (forgetting) {
        androidx.compose.material3.AlertDialog(
            onDismissRequest = { forgetting = false },
            title = { Text(stringResource(R.string.remote_forget)) },
            text = { Text(stringResource(R.string.remote_forget_help)) },
            confirmButton = {
                TextButton(onClick = { forgetting = false; scope.launch { runCatching { manager.forget(hostId) }; onLeave() } }) {
                    Text(stringResource(R.string.remote_forget))
                }
            },
            dismissButton = { TextButton(onClick = { forgetting = false }) { Text(stringResource(R.string.common_cancel)) } },
        )
    }
}

/** A state of the connection above the page's actions, in one scrolling column. */
@Composable
private fun RemoteStatePage(actions: LazyListScope.() -> Unit, state: @Composable () -> Unit) {
    LazyColumn(Modifier.fillMaxSize().padding(horizontal = 16.dp).testTag(RemoteTags.HOST_PAGE), verticalArrangement = Arrangement.spacedBy(8.dp)) {
        item(key = "state") { state() }
        actions()
    }
}

/** The computer is not the connected one: what is known of it, and the way to connect. */
@Composable
private fun RemoteNotConnected(hostName: String, host: RemoteHostFfi?, onConnect: () -> Unit) {
    val t = LingXiTheme.palette
    Column(Modifier.fillMaxWidth().padding(vertical = 16.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
        Text(hostName, color = t.text, fontSize = 18.sp, fontWeight = FontWeight.SemiBold)
        host?.let {
            Row(horizontalArrangement = Arrangement.spacedBy(6.dp), verticalAlignment = Alignment.CenterVertically) {
                Text(platformLabel(it.platform), color = t.text3, fontSize = 13.sp)
                Text("·", color = t.text4)
                Text(
                    stringResource(if (it.online) R.string.remote_online else R.string.remote_offline),
                    color = if (it.online) t.statusConnected else t.text3, fontSize = 13.sp,
                )
                Text("·", color = t.text4)
                Text(stringResource(if (it.paired) R.string.remote_paired else R.string.remote_not_paired), color = t.text3, fontSize = 13.sp)
            }
            if (!it.online) Text(stringResource(R.string.remote_offline_help), color = t.text3, fontSize = 13.sp)
        }
        if (host?.online == false) {
            OutlinedButton(onClick = onConnect, modifier = Modifier.testTag(RemoteTags.CONNECT)) { Text(stringResource(R.string.remote_try_anyway)) }
        } else {
            Button(onClick = onConnect, modifier = Modifier.testTag(RemoteTags.CONNECT)) { Text(stringResource(R.string.remote_connect)) }
        }
    }
}

/** Pair again and forget, for a computer this phone is paired with; [showRepair] / [showForget] leave out what the failure above already offers. */
internal fun LazyListScope.remoteHostActions(
    paired: Boolean,
    showRepair: Boolean,
    showForget: Boolean,
    onRepair: () -> Unit,
    onForget: () -> Unit,
) {
    if (!paired || (!showRepair && !showForget)) return
    item(key = "actions") {
        val t = LingXiTheme.palette
        Column(Modifier.fillMaxWidth().padding(vertical = 8.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
            if (showRepair) {
                OutlinedButton(onClick = onRepair, modifier = Modifier.testTag(RemoteTags.PAIR_AGAIN)) { Text(stringResource(R.string.remote_pair_again)) }
                Text(stringResource(R.string.remote_pair_again_note), color = t.text3, fontSize = 13.sp)
            }
            if (showForget) {
                OutlinedButton(onClick = onForget, modifier = Modifier.testTag(RemoteTags.FORGET)) { Text(stringResource(R.string.remote_forget)) }
                Text(stringResource(R.string.remote_forget_help), color = t.text3, fontSize = 13.sp)
            }
        }
    }
}

@Composable
private fun RemoteConnecting(title: String, reason: Int?, retryAtMs: Long?, onCancel: () -> Unit) {
    val t = LingXiTheme.palette
    val seconds = rememberCountdown(retryAtMs)
    Column(Modifier.fillMaxWidth().padding(vertical = 16.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
        Text(title, color = t.text, fontSize = 18.sp, fontWeight = FontWeight.SemiBold)
        reason?.let { Text(stringResource(it), color = t.text3, fontSize = 14.sp) }
        seconds?.let { Text(stringResource(R.string.remote_retry_in_fmt, it), color = t.text3, fontSize = 14.sp) }
        OutlinedButton(onClick = onCancel) { Text(stringResource(R.string.common_cancel)) }
    }
}

/** Whole seconds until [retryAtMs] (epoch milliseconds), counting down while shown. */
@Composable
internal fun rememberCountdown(retryAtMs: Long?): Int? {
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
        Modifier.fillMaxWidth().padding(vertical = 16.dp),
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
) {
    val t = LingXiTheme.palette
    Column(Modifier.fillMaxWidth().padding(vertical = 16.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
        Text(hostName, color = t.text, fontSize = 18.sp, fontWeight = FontWeight.SemiBold)
        Text(stringResource(failure.message), color = if (failure.kind == RemoteFailureKindFfi.DENIED) t.text2 else t.danger, fontSize = 14.sp)
        if (failure.repair) Text(stringResource(R.string.remote_pair_again_note), color = t.text3, fontSize = 13.sp)
        Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            when {
                failure.repair -> Button(onClick = onRepair, modifier = Modifier.testTag(RemoteTags.PAIR_AGAIN)) { Text(stringResource(R.string.remote_pair_again)) }
                failure.forget -> OutlinedButton(onClick = onForget, modifier = Modifier.testTag(RemoteTags.FORGET)) { Text(stringResource(R.string.remote_forget)) }
                failure.canRetry -> Button(onClick = onRetry) { Text(stringResource(R.string.common_retry)) }
            }
        }
    }
}

/** Apps owned by the desktop, shown in the same scrolling list as the shared sessions. */
@Composable
private fun RemoteAppsSessionList(
    manager: RemoteControlManager,
    hostId: String,
    attempt: UInt?,
    transport: RemoteTransportFfi?,
    sessions: List<RemoteSessionSummaryFfi>?,
    onOpenSession: (RemoteSessionSummaryFfi) -> Unit,
    footer: LazyListScope.() -> Unit,
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
        transport, sessions, onOpenSession,
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
        footer = footer,
    )
}

/**
 * The connected computer's shared sessions with status and what each waits on, under its apps when [localApps]
 * lists any. The Bots of the computer are not here: they are the tree's.
 */
@Composable
internal fun RemoteSessionList(
    transport: RemoteTransportFfi?,
    sessions: List<RemoteSessionSummaryFfi>?,
    onOpenSession: (RemoteSessionSummaryFfi) -> Unit,
    localApps: LazyListScope.() -> Unit = {},
    sessionsTitle: String? = null,
    footer: LazyListScope.() -> Unit = {},
) {
    val t = LingXiTheme.palette
    Column(Modifier.fillMaxSize().padding(horizontal = 16.dp).testTag(RemoteTags.HOST_PAGE), verticalArrangement = Arrangement.spacedBy(8.dp)) {
        transport?.let {
            Text(
                stringResource(if (it == RemoteTransportFfi.WEBRTC) R.string.remote_direct_help else R.string.remote_tunnel_help),
                color = t.text3, fontSize = 12.sp,
            )
        }
        LazyColumn(Modifier.testTag(RemoteTags.SESSIONS), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            localApps()
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
            footer()
        }
    }
}

/** One shared session: title, where and with which model, what it waits on, its status. */
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
