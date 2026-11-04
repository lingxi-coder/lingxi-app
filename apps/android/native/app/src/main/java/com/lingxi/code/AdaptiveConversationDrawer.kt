package com.lingxi.code

import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.width
import androidx.compose.material3.DrawerState
import androidx.compose.material3.DrawerValue
import androidx.compose.material3.ModalNavigationDrawer
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.derivedStateOf
import androidx.compose.runtime.getValue
import androidx.compose.runtime.movableContentOf
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.layout.onSizeChanged
import androidx.compose.ui.unit.dp
import com.lingxi.code.tabs.LocalWideLayout

/**
 * Keep native mobile drawer gestures; expose persistent navigation on tablets.
 *
 * "Tablet" is the shell's call, not this component's: [LocalWideLayout] is measured once for the
 * whole window (decision D3). [onOpenOrOpeningChange] reports the phone drawer being open, being
 * dragged open or animating, so the shell can take its tab bar away (the drawer owns the full height).
 */
@Composable
internal fun AdaptiveConversationDrawer(
    modifier: Modifier = Modifier,
    drawerState: DrawerState,
    scrimColor: Color,
    onOpenOrOpeningChange: (Boolean) -> Unit = {},
    drawerContent: @Composable () -> Unit,
    content: @Composable () -> Unit,
) {
    val wide = LocalWideLayout.current
    val currentContent by rememberUpdatedState(content)
    val currentDrawer by rememberUpdatedState(drawerContent)
    // Moving between phone and tablet layout must not recreate transcript/composer state.
    val stableContent = remember { movableContentOf { currentContent() } }
    val stableDrawer = remember { movableContentOf { currentDrawer() } }

    // The closed drawer rests at an offset of minus its own width; anything past that is "opening".
    var sheetWidthPx by remember { mutableIntStateOf(0) }
    val openOrOpening by remember(drawerState) {
        derivedStateOf {
            val offset = drawerState.currentOffset
            drawerState.currentValue == DrawerValue.Open ||
                drawerState.targetValue == DrawerValue.Open ||
                drawerState.isAnimationRunning ||
                (offset.isFinite() && sheetWidthPx > 0 && offset > -sheetWidthPx + 1f)
        }
    }
    val reportedOpen = openOrOpening && !wide
    val latestOnChange by rememberUpdatedState(onOpenOrOpeningChange)
    LaunchedEffect(reportedOpen) { latestOnChange(reportedOpen) }

    Box(modifier) {
        if (wide) {
            Row(Modifier.fillMaxSize()) {
                Box(Modifier.width(320.dp)) { stableDrawer() }
                Box(Modifier.weight(1f)) { stableContent() }
            }
        } else {
            ModalNavigationDrawer(
                drawerState = drawerState,
                scrimColor = scrimColor,
                drawerContent = { Box(Modifier.onSizeChanged { sheetWidthPx = it.width }) { stableDrawer() } },
                content = stableContent,
            )
        }
    }
}
