/*
 * Copyright (C) 2025 OpenMinis contributors
 * Copyright (C) 2026 LingXi contributors
 * SPDX-License-Identifier: GPL-3.0-only
 */
package com.lingxi.code.terminal

import androidx.lifecycle.ViewModel
import androidx.lifecycle.ViewModelProvider
import androidx.lifecycle.viewModelScope
import com.lingxi.code.terminal.emulator.TerminalEmulator
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Job
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.combine
import kotlinx.coroutines.launch

class TerminalViewModel(
    private val args: TerminalRouteArgs,
    private val gateway: TerminalSessionGateway,
) : ViewModel() {
    private val local = MutableStateFlow(TerminalUiState(args))
    private var startJob: Job? = null
    val emulator = TerminalEmulator()

    private val merged = MutableStateFlow(local.value)
    val uiState: StateFlow<TerminalUiState> = merged.asStateFlow()

    init {
        viewModelScope.launch {
            combine(local, gateway.state, gateway.error) { ui, connection, error ->
                ui.copy(connectionState = connection, errorMessage = error)
            }.collect(merged)
        }
        viewModelScope.launch {
            gateway.output.collect(emulator::feed)
        }
    }

    fun dispatch(action: TerminalAction) {
        when (action) {
            TerminalAction.Start -> start()
            TerminalAction.Close -> viewModelScope.launch { gateway.close() }
            TerminalAction.Clear -> viewModelScope.launch { gateway.clear() }
            TerminalAction.ToggleCtrl -> local.value = local.value.copy(ctrlActive = !local.value.ctrlActive)
            TerminalAction.ToggleKeyboard -> local.value =
                local.value.copy(keyboardVisible = !local.value.keyboardVisible)
            is TerminalAction.Resize -> viewModelScope.launch {
                gateway.resize(action.columns, action.rows)
            }
            is TerminalAction.OpenUrl -> Unit
            is TerminalAction.SendBytes -> send(action.bytes)
        }
    }

    private fun start() {
        if (startJob != null) return
        startJob = viewModelScope.launch {
            try {
                val result = gateway.start(args.sessionId)
                args.initCommand
                    ?.takeIf { it.isNotBlank() && result.created }
                    ?.let {
                        // Deliberately omit CR/LF: the command is editable at the prompt.
                        gateway.write(it.toByteArray())
                    }
            } catch (cancelled: CancellationException) {
                throw cancelled
            } catch (_: Throwable) {
                // Gateway state/error are authoritative and fail closed.
            }
        }
    }

    private fun send(original: ByteArray) {
        var bytes = original
        if (local.value.ctrlActive && original.size == 1) {
            val character = original[0].toInt().toChar().uppercaseChar()
            if (character in 'A'..'Z') {
                bytes = byteArrayOf((character - 'A' + 1).toByte())
                local.value = local.value.copy(ctrlActive = false)
            }
        }
        viewModelScope.launch { gateway.write(bytes) }
    }

    class Factory(
        private val args: TerminalRouteArgs,
        private val gateway: TerminalSessionGateway,
    ) : ViewModelProvider.Factory {
        @Suppress("UNCHECKED_CAST")
        override fun <T : ViewModel> create(modelClass: Class<T>): T =
            TerminalViewModel(args, gateway) as T
    }
}
