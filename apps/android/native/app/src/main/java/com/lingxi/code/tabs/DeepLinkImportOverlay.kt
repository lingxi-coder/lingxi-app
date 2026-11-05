package com.lingxi.code.tabs

import androidx.activity.compose.BackHandler
import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.slideInVertically
import androidx.compose.animation.slideOutVertically
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import com.lingxi.code.components.blockTouchesBehind
import com.lingxi.code.conversation.ConversationSource
import com.lingxi.code.navigation.DeepLink
import com.lingxi.code.settings.DeepLinkImportPage
import com.lingxi.code.settings.SettingsEngineBridge

/**
 * The import preview of a `lingxi://import/...` link (decision D17): a full-screen page above the
 * shell, on whatever tab the user is on. It used to be a Settings page; it is not part of the
 * Settings tab's stack, so opening or dismissing it leaves every tab's stack alone.
 */
@Composable
internal fun DeepLinkImportOverlay(link: DeepLink?, engineSource: ConversationSource?, onDismiss: () -> Unit) {
    // Keeps drawing the last link while the page slides out.
    var shown by remember { mutableStateOf<DeepLink?>(null) }
    LaunchedEffect(link) { if (link != null) shown = link }
    AnimatedVisibility(
        visible = link != null,
        enter = slideInVertically(initialOffsetY = { it }),
        exit = slideOutVertically(targetOffsetY = { it }),
    ) {
        val page = link ?: shown
        if (page != null) ImportPreview(page, engineSource, onDismiss)
    }
}

@Composable
private fun ImportPreview(link: DeepLink, engineSource: ConversationSource?, onDismiss: () -> Unit) {
    // The bridge lives exactly as long as the preview: it subscribes to the engine while it is bound.
    val bridge = remember { SettingsEngineBridge() }
    LaunchedEffect(engineSource) { bridge.bind(engineSource) }
    BackHandler(onBack = onDismiss)
    Box(Modifier.fillMaxSize().blockTouchesBehind()) { DeepLinkImportPage(link, bridge, onDismiss) }
}
