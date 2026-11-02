package com.lingxi.code.model

import com.lingxi.code.bindings.client.SessionModeDto

fun SessionMode.toDto(): SessionModeDto = when (this) {
    SessionMode.Chat -> SessionModeDto.CHAT
    SessionMode.Code -> SessionModeDto.CODE
}

fun SessionModeDto.toUi(): SessionMode = when (this) {
    SessionModeDto.CHAT -> SessionMode.Chat
    SessionModeDto.CODE -> SessionMode.Code
}

fun sessionModeFromWireValue(value: String): SessionMode =
    requireNotNull(SessionMode.entries.firstOrNull { it.wireValue == value }) { "Unknown session mode: $value" }
