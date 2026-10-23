package com.lingxi.code.remote

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.systemBars
import androidx.compose.foundation.layout.windowInsetsPadding
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.LifecycleEventObserver
import androidx.lifecycle.compose.LocalLifecycleOwner
import com.lingxi.code.R
import com.lingxi.code.connectivity.rememberOnlineState
import com.lingxi.code.theme.LingXiTheme

/**
 * Remote Control: the user's computers, one computer's sessions, one session. A
 * full-surface overlay in the manner of settings, with its own back stack.
 *
 * [requestedRoute] opens a destination directly (a notification or a link); it is
 * consumed once through [onRouteHandled]. Closing the overlay ends the connection.
 */
@Composable
fun RemoteOverlay(
    requestedRoute: RemoteRoute?,
    onRouteHandled: () -> Unit,
    isDark: Boolean,
    onOpenAccount: () -> Unit,
    onClose: () -> Unit,
) {
    val context = LocalContext.current
    val manager = remember(context) { RemoteControl.manager(context) }
    var stack by rememberSaveable(stateSaver = RemoteRoute.stackSaver) { mutableStateOf(listOf<RemoteRoute>(RemoteRoute.Hosts)) }
    LaunchedEffect(requestedRoute) {
        requestedRoute?.let {
            stack = RemoteRoute.stackFor(it)
            onRouteHandled()
        }
    }
    val uid by manager.uid.collectAsState()
    val hosts by manager.hosts.collectAsState()
    val connection by manager.connection.collectAsState()

    // The app came back to the front, or the network came back: skip the rest of a backoff.
    val lifecycle = LocalLifecycleOwner.current.lifecycle
    DisposableEffect(lifecycle, manager) {
        val observer = LifecycleEventObserver { _, event -> if (event == Lifecycle.Event.ON_RESUME) manager.nudge() }
        lifecycle.addObserver(observer)
        onDispose { lifecycle.removeObserver(observer) }
    }
    val online by rememberOnlineState()
    LaunchedEffect(online) { if (online) manager.nudge() }
    val activity = context as? android.app.Activity
    DisposableEffect(manager) {
        onDispose { if (activity?.isChangingConfigurations != true) manager.closeConnection() }
    }
    val sessions by manager.sessions.collectAsState()

    val current = stack.last()
    fun pop() {
        if (stack.size > 1) stack = stack.dropLast(1) else onClose()
    }
    BackHandler(onBack = ::pop)

    fun hostName(hostId: String) =
        connection?.takeIf { it.hostId == hostId }?.host?.name ?: hosts.hosts.firstOrNull { it.hostId == hostId }?.name ?: hostId

    Box(
        Modifier.fillMaxSize().background(LingXiTheme.palette.windowBg)
            .windowInsetsPadding(WindowInsets.systemBars).imePadding(),
    ) {
        when {
            uid == null -> RemotePage(stringResource(R.string.remote_title), onBack = onClose) {
                RemoteSignedOut(onOpenAccount)
            }
            current is RemoteRoute.Hosts -> RemotePage(stringResource(R.string.remote_computers), onBack = ::pop) {
                RemoteHostList(manager, onConnect = { stack = stack + RemoteRoute.Host(it) })
            }
            current is RemoteRoute.Host -> RemotePage(hostName(current.hostId), onBack = ::pop) {
                RemoteHostScreen(
                    manager = manager,
                    hostId = current.hostId,
                    hostName = hostName(current.hostId),
                    onOpenSession = { stack = stack + RemoteRoute.Session(current.hostId, it.sessionId) },
                    onLeave = ::pop,
                )
            }
            current is RemoteRoute.Session -> RemoteSessionScreen(
                manager = manager,
                hostId = current.hostId,
                hostName = hostName(current.hostId),
                sessionId = current.sessionId,
                title = sessions.sessions?.firstOrNull { it.sessionId == current.sessionId }?.title,
                isDark = isDark,
                onBack = ::pop,
            )
        }
    }
}
