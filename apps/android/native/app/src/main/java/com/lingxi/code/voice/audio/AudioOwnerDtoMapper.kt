package com.lingxi.code.voice.audio

import com.lingxi.code.bindings.client.AudioOwnerDto
import com.lingxi.code.bindings.client.MediaOwnerDto
import com.lingxi.code.bindings.client.MediaOwnerKindDto
import java.util.UUID

/** Convert a host DTO without discarding an attached engine's owner scope. */
internal fun AudioOwnerDto.toAudioOwnerKeyOrNull(): AudioOwnerKey? = when (this) {
    is AudioOwnerDto.Session -> sessionId
        .takeIf { it.isValidAudioOwnerIdentifier() }
        ?.let(AudioOwnerKey::session)
    is AudioOwnerDto.Ui -> instanceId
        .takeIf { it.isValidAudioOwnerIdentifier() }
        ?.let(AudioOwnerKey::ui)
    is AudioOwnerDto.System -> instanceId
        .takeIf { it.isValidAudioOwnerIdentifier() }
        ?.let(AudioOwnerKey::system)
    is AudioOwnerDto.Attached -> attachedOwnerKey(
        attachmentHostId,
        attachmentGeneration,
        logicalKind,
        logicalOwnerId,
    )
}

/** Convert a media-device DTO without collapsing two engines for one session. */
internal fun MediaOwnerDto.toAudioOwnerKeyOrNull(): AudioOwnerKey? = when (this) {
    is MediaOwnerDto.Session -> sessionId
        .takeIf { it.isValidAudioOwnerIdentifier() }
        ?.let(AudioOwnerKey::session)
    is MediaOwnerDto.Ui -> instanceId
        .takeIf { it.isValidAudioOwnerIdentifier() }
        ?.let(AudioOwnerKey::ui)
    is MediaOwnerDto.System -> instanceId
        .takeIf { it.isValidAudioOwnerIdentifier() }
        ?.let(AudioOwnerKey::system)
    is MediaOwnerDto.Attached -> attachedOwnerKey(
        attachmentHostId,
        attachmentGeneration,
        logicalKind,
        logicalOwnerId,
    )
}

private fun attachedOwnerKey(
    attachmentHostId: String,
    attachmentGeneration: ULong,
    logicalKind: MediaOwnerKindDto,
    logicalOwnerId: String,
): AudioOwnerKey? {
    if (!attachmentHostId.isCanonicalUuidV4() || attachmentGeneration == 0uL) return null
    if (!logicalOwnerId.isValidAudioOwnerIdentifier()) return null
    val kind = when (logicalKind) {
        MediaOwnerKindDto.SESSION -> AudioOwnerKey.Kind.Session
        MediaOwnerKindDto.UI -> AudioOwnerKey.Kind.Ui
        MediaOwnerKindDto.SYSTEM -> AudioOwnerKey.Kind.System
    }
    return AudioOwnerKey.attached(kind, logicalOwnerId, attachmentHostId.lowercase(), attachmentGeneration)
}

private fun String.isCanonicalUuidV4(): Boolean {
    if (length != 36) return false
    val parsed = runCatching { UUID.fromString(this) }.getOrNull() ?: return false
    return parsed.version() == 4 &&
        parsed.variant() == 2 &&
        parsed.toString().equals(this, ignoreCase = true)
}

private fun String.isValidAudioOwnerIdentifier(): Boolean =
    isNotEmpty() && toByteArray(Charsets.UTF_8).size <= MAX_AUDIO_OWNER_ID_BYTES

private const val MAX_AUDIO_OWNER_ID_BYTES = 256
