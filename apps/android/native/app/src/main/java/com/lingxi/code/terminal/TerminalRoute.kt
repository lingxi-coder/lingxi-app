/*
 * Copyright (C) 2025 OpenMinis contributors
 * Copyright (C) 2026 LingXi contributors
 * SPDX-License-Identifier: GPL-3.0-only
 */
package com.lingxi.code.terminal

import android.app.Activity
import android.content.Context
import android.content.ContextWrapper
import androidx.activity.compose.BackHandler
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.navigationBars
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.systemBars
import androidx.compose.foundation.layout.windowInsetsPadding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.KeyboardArrowLeft
import androidx.compose.material.icons.automirrored.filled.KeyboardArrowRight
import androidx.compose.material.icons.automirrored.filled.KeyboardTab
import androidx.compose.material.icons.automirrored.filled.Backspace
import androidx.compose.material.icons.filled.CleaningServices
import androidx.compose.material.icons.filled.Close
import androidx.compose.material.icons.filled.KeyboardArrowDown
import androidx.compose.material.icons.filled.KeyboardArrowUp
import androidx.compose.material.icons.filled.KeyboardHide
import androidx.compose.material.icons.filled.Keyboard
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.lifecycle.viewmodel.compose.viewModel
import com.lingxi.code.terminal.emulator.TerminalEmulator
import com.lingxi.code.terminal.view.TerminalInputView
import com.lingxi.code.terminal.view.TerminalNativeView
import com.lingxi.code.terminal.view.rememberTerminalInputController

private val TerminalBackground = Color.Black
private val TerminalForeground = Color(0xffd4d4d4)
private val TerminalAccent = Color(0xff34c759)
private val AccessoryBackground = Color(0xff1f1f1f)
private val KeyBackground = Color(0xff404040)

@Composable
fun TerminalRoute(
    args: TerminalRouteArgs,
    instanceKey: String,
    gateway: TerminalSessionGateway,
    onBack: () -> Unit,
    onOpenUrl: (String) -> Unit,
    modifier: Modifier = Modifier,
    viewModel: TerminalViewModel = viewModel(
        key = "terminal:${args.sessionId}:$instanceKey",
        factory = TerminalViewModel.Factory(args, gateway),
    ),
) {
    val state by viewModel.uiState.collectAsState()
    val context = LocalContext.current
    val emulator = viewModel.emulator

    LaunchedEffect(viewModel) { viewModel.dispatch(TerminalAction.Start) }
    DisposableEffect(viewModel, onOpenUrl) {
        emulator.onResponse = { viewModel.dispatch(TerminalAction.SendBytes(it)) }
        emulator.onOpenUrl = { viewModel.dispatch(TerminalAction.OpenUrl(it)); onOpenUrl(it) }
        onDispose {
            emulator.onResponse = null
            emulator.onOpenUrl = null
            if (context.findActivity()?.isChangingConfigurations != true) {
                viewModel.dispatch(TerminalAction.Close)
            }
        }
    }
    BackHandler {
        viewModel.dispatch(TerminalAction.Close)
        onBack()
    }

    TerminalScreen(
        state = state.copy(title = emulator.title.ifBlank { state.title }),
        emulator = emulator,
        onAction = { action ->
            if (action == TerminalAction.Close) onBack()
            if (action is TerminalAction.OpenUrl) onOpenUrl(action.url)
            viewModel.dispatch(action)
        },
        modifier = modifier,
    )
}

private tailrec fun Context.findActivity(): Activity? =
    when (this) {
        is Activity -> this
        is ContextWrapper -> baseContext.findActivity()
        else -> null
    }

@Composable
fun TerminalScreen(
    state: TerminalUiState,
    emulator: TerminalEmulator,
    onAction: (TerminalAction) -> Unit,
    modifier: Modifier = Modifier,
) {
    val input = rememberTerminalInputController()
    val send: (ByteArray) -> Unit = {
        emulator.scrollOffset = 0
        onAction(TerminalAction.SendBytes(it))
    }

    LaunchedEffect(state.keyboardVisible) {
        if (state.keyboardVisible) input.showKeyboard() else input.hideKeyboard()
    }

    Box(modifier.fillMaxSize().background(TerminalBackground)) {
        Column(
            Modifier.fillMaxSize()
                .windowInsetsPadding(WindowInsets.systemBars)
                .imePadding()
                .padding(top = 52.dp, bottom = 48.dp),
        ) {
            TerminalNativeView(
                emulator = emulator,
                onResize = { columns, rows ->
                    emulator.resize(columns, rows)
                    onAction(TerminalAction.Resize(columns, rows))
                },
                onTap = { onAction(TerminalAction.ToggleKeyboard) },
                onOpenUrl = { onAction(TerminalAction.OpenUrl(it)) },
                modifier = Modifier.fillMaxSize(),
            )
            TerminalInputView(
                onInput = send,
                applicationCursorKeys = emulator.applicationCursorKeys,
                controller = input,
                modifier = Modifier.size(1.dp),
            )
        }

        TerminalTopBar(
            title = state.title,
            connection = state.connectionState,
            onClose = { onAction(TerminalAction.Close) },
            onClear = {
                emulator.reset()
                onAction(TerminalAction.Clear)
            },
            modifier = Modifier.align(Alignment.TopCenter)
                .windowInsetsPadding(WindowInsets.systemBars),
        )
        TerminalAccessoryBar(
            ctrlActive = state.ctrlActive,
            keyboardVisible = state.keyboardVisible,
            appCursor = emulator.applicationCursorKeys,
            onAction = onAction,
            modifier = Modifier.align(Alignment.BottomCenter)
                .imePadding()
                .windowInsetsPadding(WindowInsets.navigationBars),
        )
        state.errorMessage?.let {
            Text(
                text = it,
                color = MaterialTheme.colorScheme.error,
                fontSize = 12.sp,
                modifier = Modifier.align(Alignment.TopCenter)
                    .windowInsetsPadding(WindowInsets.systemBars)
                    .padding(horizontal = 16.dp)
                    .padding(top = 54.dp)
                    .background(Color(0xcc2b1111), RoundedCornerShape(6.dp))
                    .padding(8.dp),
            )
        }
    }
}

@Composable
private fun TerminalTopBar(
    title: String,
    connection: TerminalConnectionState,
    onClose: () -> Unit,
    onClear: () -> Unit,
    modifier: Modifier,
) {
    Row(
        modifier.fillMaxWidth().height(52.dp).background(TerminalBackground).padding(horizontal = 12.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        RoundIcon(Icons.Default.Close, "Close terminal", onClose)
        Spacer(Modifier.weight(1f))
        Column(horizontalAlignment = Alignment.CenterHorizontally) {
            Text(title, color = TerminalForeground, fontFamily = FontFamily.Monospace, fontSize = 15.sp, maxLines = 1)
            Text(
                connection.name.lowercase(),
                color = if (connection == TerminalConnectionState.CONNECTED) TerminalAccent else Color.Gray,
                fontFamily = FontFamily.Monospace,
                fontSize = 9.sp,
            )
        }
        Spacer(Modifier.weight(1f))
        RoundIcon(Icons.Default.CleaningServices, "Clear terminal", onClear, TerminalAccent)
    }
}

@Composable
private fun RoundIcon(
    icon: ImageVector,
    description: String,
    onClick: () -> Unit,
    tint: Color = TerminalForeground,
) {
    Box(
        Modifier.size(36.dp).clip(CircleShape).background(Color(0xff2c2c2e)).clickable(onClick = onClick),
        contentAlignment = Alignment.Center,
    ) {
        Icon(icon, description, tint = tint, modifier = Modifier.size(18.dp))
    }
}

@Composable
private fun TerminalAccessoryBar(
    ctrlActive: Boolean,
    keyboardVisible: Boolean,
    appCursor: Boolean,
    onAction: (TerminalAction) -> Unit,
    modifier: Modifier,
) {
    val send = { bytes: ByteArray -> onAction(TerminalAction.SendBytes(bytes)) }
    val arrow = { direction: Char ->
        send(if (appCursor) "\u001bO$direction".toByteArray() else "\u001b[$direction".toByteArray())
    }
    Row(
        modifier.fillMaxWidth().height(48.dp).background(AccessoryBackground)
            .horizontalScroll(rememberScrollState()).padding(horizontal = 8.dp, vertical = 7.dp),
        horizontalArrangement = Arrangement.spacedBy(8.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        TerminalKey(if (keyboardVisible) "Hide" else "Keys", if (keyboardVisible) Icons.Default.KeyboardHide else Icons.Default.Keyboard) {
            onAction(TerminalAction.ToggleKeyboard)
        }
        TerminalKey("Esc") { send(byteArrayOf(0x1b)) }
        TerminalKey("Tab", Icons.AutoMirrored.Filled.KeyboardTab) { send(byteArrayOf(9)) }
        TerminalKey("Enter") { send(byteArrayOf(0x0d)) }
        TerminalKey("Ctrl", active = ctrlActive) { onAction(TerminalAction.ToggleCtrl) }
        TerminalKey("↑", Icons.Default.KeyboardArrowUp) { arrow('A') }
        TerminalKey("↓", Icons.Default.KeyboardArrowDown) { arrow('B') }
        TerminalKey("←", Icons.AutoMirrored.Filled.KeyboardArrowLeft) { arrow('D') }
        TerminalKey("→", Icons.AutoMirrored.Filled.KeyboardArrowRight) { arrow('C') }
        TerminalKey("C-c") { send(byteArrayOf(3)) }
        TerminalKey("C-d") { send(byteArrayOf(4)) }
        TerminalKey("⌫", Icons.AutoMirrored.Filled.Backspace) { send(byteArrayOf(0x7f)) }
    }
}

@Composable
private fun TerminalKey(
    label: String,
    icon: ImageVector? = null,
    active: Boolean = false,
    onClick: () -> Unit,
) {
    Row(
        Modifier.height(32.dp).clip(RoundedCornerShape(7.dp))
            .background(if (active) Color(0xff007aff) else KeyBackground)
            .clickable(onClick = onClick).padding(horizontal = 10.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(4.dp),
    ) {
        icon?.let { Icon(it, null, tint = TerminalAccent, modifier = Modifier.size(13.dp)) }
        Text(label, color = if (active) Color.White else TerminalAccent, fontFamily = FontFamily.Monospace, fontSize = 11.sp)
    }
}
