package com.lingxi.code.model

import com.lingxi.code.bindings.SessionModeDto

fun SessionMode.toDto(): SessionModeDto = when (this) {
    SessionMode.Chat -> SessionModeDto.CHAT
    SessionMode.Code -> SessionModeDto.CODE
}

fun SessionModeDto.toUi(): SessionMode = when (this) {
    SessionModeDto.CHAT -> SessionMode.Chat
    SessionModeDto.CODE -> SessionMode.Code
}

fun sessionModeFromWireValue(value: String?): SessionMode =
    SessionMode.entries.firstOrNull { it.wireValue == value } ?: SessionMode.Code

internal data class PersistedSessionTarget(
    val ref: SessionRef,
    val resumeEmpty: Boolean,
)

/**
 * Keep a durable `(workspace, mode)` session resumable while its async catalog
 * is still loading. `resumeEmpty` is conservative when metadata is absent:
 * that command also replays populated transcripts, while plain resume rejects
 * a real zero-message session.
 */
internal fun persistedSessionTarget(
    sessionId: String?,
    catalogRow: SessionRow?,
): PersistedSessionTarget? {
    val id = sessionId?.trim()?.takeIf { it.isNotEmpty() && it != "new" } ?: return null
    val matchingRow = catalogRow?.takeIf { it.uuid == id }
    return PersistedSessionTarget(
        ref = SessionRef(id, matchingRow?.title.orEmpty()),
        resumeEmpty = matchingRow?.messageCount?.let { it == 0 } ?: true,
    )
}
