package com.lingxi.code.voice.audio

import com.lingxi.code.bindings.client.AudioOwnerDto
import com.lingxi.code.bindings.client.MediaOwnerDto
import com.lingxi.code.bindings.client.MediaOwnerKindDto
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class AudioOwnerDtoMapperTest {
    private val attachmentHostId = "b34a3d7d-0dcb-4ace-8de3-980001c48b88"

    @Test
    fun attachedAudioAndMediaOwnersPreserveAttachmentAndLogicalOwner() {
        val cases = listOf(
            Triple(MediaOwnerKindDto.SESSION, AudioOwnerKey.Kind.Session, "session-a"),
            Triple(MediaOwnerKindDto.UI, AudioOwnerKey.Kind.Ui, "ui-a"),
            Triple(MediaOwnerKindDto.SYSTEM, AudioOwnerKey.Kind.System, "system-a"),
        )

        cases.forEach { (logicalKind, ownerKind, logicalOwnerId) ->
            val expected = AudioOwnerKey.attached(
                ownerKind,
                logicalOwnerId,
                attachmentHostId,
                generation = 7uL,
            )
            val audioOwner = AudioOwnerDto.Attached(
                attachmentHostId = attachmentHostId,
                attachmentGeneration = 7uL,
                logicalKind = logicalKind,
                logicalOwnerId = logicalOwnerId,
            )
            val mediaOwner = MediaOwnerDto.Attached(
                attachmentHostId = attachmentHostId,
                attachmentGeneration = 7uL,
                logicalKind = logicalKind,
                logicalOwnerId = logicalOwnerId,
            )

            assertEquals(expected, audioOwner.toAudioOwnerKeyOrNull())
            assertEquals(expected, mediaOwner.toAudioOwnerKeyOrNull())
        }
    }

    @Test
    fun sameSessionUnderDifferentAttachmentsHasIndependentOwnerTeardown() {
        val first = audioOwner(generation = 11uL).toAudioOwnerKeyOrNull()!!
        val second = mediaOwner(generation = 12uL).toAudioOwnerKeyOrNull()!!
        assertFalse("attachment generation participates in owner equality", first == second)

        val coordinator = AudioResourceCoordinator(initialEpoch = 17L)
        val firstLease = (coordinator.acquire(
            AudioOperationIdentity("00000000-0000-4000-8000-000000000011", 1L, 17L),
            first,
            AudioResource.ProviderRender,
        ) as AudioLeaseDecision.Granted).lease
        val secondLease = (coordinator.acquire(
            AudioOperationIdentity("00000000-0000-4000-8000-000000000012", 1L, 17L),
            second,
            AudioResource.ProviderRender,
        ) as AudioLeaseDecision.Granted).lease

        assertEquals(listOf(firstLease), coordinator.endOwner(first))
        assertFalse(coordinator.isActive(firstLease))
        assertTrue("old attachment teardown must preserve the replacement lease", coordinator.isActive(secondLease))
    }

    @Test
    fun malformedAttachedOwnerFieldsFailClosedForBothDtoFamilies() {
        val invalid = listOf(
            "not-a-uuid" to 1uL,
            "b34a3d7d-0dcb-1ace-8de3-980001c48b88" to 1uL,
            "b34a3d7d-0dcb-4ace-4de3-980001c48b88" to 1uL,
            attachmentHostId to 0uL,
        )
        invalid.forEach { (hostId, generation) ->
            assertNull(audioOwner(hostId, generation).toAudioOwnerKeyOrNull())
            assertNull(mediaOwner(hostId, generation).toAudioOwnerKeyOrNull())
        }

        assertNull(audioOwner(logicalOwnerId = "").toAudioOwnerKeyOrNull())
        assertNull(mediaOwner(logicalOwnerId = "").toAudioOwnerKeyOrNull())
        val oversized = "é".repeat(129)
        assertNull(audioOwner(logicalOwnerId = oversized).toAudioOwnerKeyOrNull())
        assertNull(mediaOwner(logicalOwnerId = oversized).toAudioOwnerKeyOrNull())

        val uppercaseHostId = audioOwner(hostId = attachmentHostId.uppercase()).toAudioOwnerKeyOrNull()
        assertEquals(audioOwner().toAudioOwnerKeyOrNull(), uppercaseHostId)
    }

    @Test
    fun ownerIdentifierValidationMatchesTheRustDtoContract() {
        val expected = AudioOwnerKey.attached(AudioOwnerKey.Kind.Session, " ", attachmentHostId, 1uL)
        assertEquals(expected, audioOwner(logicalOwnerId = " ").toAudioOwnerKeyOrNull())
        assertEquals(expected, mediaOwner(logicalOwnerId = " ").toAudioOwnerKeyOrNull())
    }

    @Test
    fun legacyOwnersKeepTheirExistingLogicalKey() {
        assertEquals(
            AudioOwnerKey.session("session-a"),
            AudioOwnerDto.Session(sessionId = "session-a").toAudioOwnerKeyOrNull(),
        )
        assertEquals(
            AudioOwnerKey.ui("ui-a"),
            MediaOwnerDto.Ui(instanceId = "ui-a").toAudioOwnerKeyOrNull(),
        )
    }

    private fun audioOwner(
        hostId: String = attachmentHostId,
        generation: ULong = 1uL,
        logicalOwnerId: String = "session-a",
    ) = AudioOwnerDto.Attached(
        attachmentHostId = hostId,
        attachmentGeneration = generation,
        logicalKind = MediaOwnerKindDto.SESSION,
        logicalOwnerId = logicalOwnerId,
    )

    private fun mediaOwner(
        hostId: String = attachmentHostId,
        generation: ULong = 1uL,
        logicalOwnerId: String = "session-a",
    ) = MediaOwnerDto.Attached(
        attachmentHostId = hostId,
        attachmentGeneration = generation,
        logicalKind = MediaOwnerKindDto.SESSION,
        logicalOwnerId = logicalOwnerId,
    )
}
