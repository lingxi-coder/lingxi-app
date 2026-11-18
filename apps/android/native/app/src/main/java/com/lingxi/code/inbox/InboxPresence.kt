package com.lingxi.code.inbox

import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.compose.LocalLifecycleOwner

@Composable
internal fun InboxPresence(inbox: InboxController?, ref: InboxRef?) {
    val lease = remember { Any() }
    val lifecycle by LocalLifecycleOwner.current.lifecycle.currentStateFlow.collectAsState()
    val active = ref?.takeIf { lifecycle.isAtLeast(Lifecycle.State.RESUMED) }
    DisposableEffect(inbox, active) {
        if (active != null) inbox?.enter(lease, active)
        onDispose { if (active != null) inbox?.leave(lease) }
    }
}
