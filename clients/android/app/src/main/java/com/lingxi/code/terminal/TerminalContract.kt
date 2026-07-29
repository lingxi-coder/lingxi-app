/*
 * Copyright (C) 2025 OpenMinis contributors
 * Copyright (C) 2026 LingXi contributors
 *
 * This program is free software: you can redistribute it and/or modify
 * it under the terms of the GNU General Public License as published by
 * the Free Software Foundation, version 3 of the License.
 */
package com.lingxi.code.terminal

import androidx.compose.runtime.Immutable
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.StateFlow

@Immutable
data class TerminalRouteArgs(
    val sessionId: String,
    val initCommand: String? = null,
)

enum class TerminalConnectionState { IDLE, CONNECTING, CONNECTED, FAILED, CLOSED }

@Immutable
data class TerminalUiState(
    val args: TerminalRouteArgs,
    val connectionState: TerminalConnectionState = TerminalConnectionState.IDLE,
    val title: String = "Terminal",
    val errorMessage: String? = null,
    val keyboardVisible: Boolean = false,
    val ctrlActive: Boolean = false,
)

sealed interface TerminalAction {
    data object Start : TerminalAction
    data object Close : TerminalAction
    data object Clear : TerminalAction
    data object ToggleKeyboard : TerminalAction
    data object ToggleCtrl : TerminalAction
    data class SendBytes(val bytes: ByteArray) : TerminalAction
    data class Resize(val columns: Int, val rows: Int) : TerminalAction
    data class OpenUrl(val url: String) : TerminalAction
}

data class TerminalStartResult(
    val created: Boolean,
)

/**
 * Minimal host boundary for a PTY-backed terminal. Implementations own the
 * process and handle; the UI owns only rendering and user input.
 *
 * [start] must fail rather than silently selecting another runtime.
 */
interface TerminalSessionGateway {
    val output: Flow<ByteArray>
    val state: StateFlow<TerminalConnectionState>
    val error: StateFlow<String?>

    suspend fun start(sessionId: String): TerminalStartResult
    suspend fun write(bytes: ByteArray)
    suspend fun resize(columns: Int, rows: Int)
    suspend fun clear()
    suspend fun close()
}
