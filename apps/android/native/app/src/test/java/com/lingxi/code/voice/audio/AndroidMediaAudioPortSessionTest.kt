package com.lingxi.code.voice.audio

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertThrows
import org.junit.Assert.assertTrue
import org.junit.Test

class AndroidMediaAudioPortSessionTest {
    @Test
    fun outputNormalizationPreservesPackedStereoAlignment() {
        val pcm = byteArrayOf(
            0, 0, 0, 0,
            0, 0, 0, 0,
            0, 0, 0, 0,
            0, 0, 0, 0,
        )

        val result = normalizeToDeviceFormat(pcm, sourceRate = 48_000, channels = 2, targetRate = 24_000)

        // Four interleaved stereo source frames become two mono output frames.
        assertEquals(4, result.size)
        assertEquals(0, result.size % 2)
    }

    @Test
    fun outputNormalizationRejectsInsteadOfTruncatingFramesLongerThanTwoSeconds() {
        val sourceFrames = 16_001 // Just over two seconds at 8 kHz.
        val pcm = ByteArray(sourceFrames * 2)

        val error = assertThrows(AudioOperationException::class.java) {
            normalizeToDeviceFormat(pcm, sourceRate = 8_000, channels = 1, targetRate = 24_000)
        }

        assertEquals(DeviceAudioErrorKind.MediaTooLarge, error.kind)
    }

    @Test
    fun playbackHeadReceiptHandlesCounterWrapAndCapsToAcceptedSourceFrames() {
        assertEquals(32L, mediaDeviceFramesSince(0xfffffff0L, 0x10L))
        assertEquals(
            64,
            mediaPlayedSourceFrames(
                startDeviceHead = 0xfffffff0L,
                currentDeviceHead = 0x10L,
                acceptedDeviceFrames = 100,
                sourceSampleFrames = 200,
            ),
        )
    }

    @Test
    fun playbackReceiptCountsRoundedResamplingAsFullyPlayedAtTheEnd() {
        assertEquals(2, mediaPlayedSourceFrames(0, 1, acceptedDeviceFrames = 2, sourceSampleFrames = 3))
        assertEquals(3, mediaPlayedSourceFrames(0, 2, acceptedDeviceFrames = 2, sourceSampleFrames = 3))
    }

    @Test
    fun interruptClaimsPendingReceiptsBeforeAWaitingFlushCanCountThem() {
        val receipt = AndroidMediaReceiptClaim()
        val flush = AndroidMediaBarrierToken()
        val interrupt = AndroidMediaBarrierToken()

        assertTrue(receipt.claim(interrupt))
        assertFalse(receipt.claim(flush))
        assertTrue(receipt.isClaimedBy(interrupt))
        assertFalse(receipt.isClaimedBy(flush))
    }

    @Test
    fun failedInterruptReleasesItsClaimForTheFlushThatWillFinishNaturally() {
        val receipt = AndroidMediaReceiptClaim()
        val interrupt = AndroidMediaBarrierToken()
        val flush = AndroidMediaBarrierToken()

        assertTrue(receipt.claim(interrupt))
        assertTrue(receipt.release(interrupt))
        assertTrue(receipt.claim(flush))
    }
}
