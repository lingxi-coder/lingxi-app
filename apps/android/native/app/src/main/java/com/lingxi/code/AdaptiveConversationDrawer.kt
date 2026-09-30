package com.lingxi.code

import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.width
import androidx.compose.material3.DrawerState
import androidx.compose.material3.ModalNavigationDrawer
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.movableContentOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.unit.dp

/** Keep native mobile drawer gestures; expose persistent navigation on tablets. */
@Composable
internal fun AdaptiveConversationDrawer(
    modifier: Modifier = Modifier,
    drawerState: DrawerState,
    scrimColor: Color,
    drawerContent: @Composable () -> Unit,
    content: @Composable () -> Unit,
) {
    val currentContent by rememberUpdatedState(content)
    val currentDrawer by rememberUpdatedState(drawerContent)
    // Moving between phone and tablet layout must not recreate transcript/composer state.
    val stableContent = remember { movableContentOf { currentContent() } }
    val stableDrawer = remember { movableContentOf { currentDrawer() } }
    BoxWithConstraints(modifier) {
        if (maxWidth >= 840.dp) {
            Row(Modifier.fillMaxSize()) {
                Box(Modifier.width(320.dp)) { stableDrawer() }
                Box(Modifier.weight(1f)) { stableContent() }
            }
        } else {
            ModalNavigationDrawer(drawerState = drawerState, scrimColor = scrimColor,
                drawerContent = stableDrawer, content = stableContent)
        }
    }
}
