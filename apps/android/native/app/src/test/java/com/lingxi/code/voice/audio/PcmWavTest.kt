package com.lingxi.code.voice.audio

import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test
import android.content.ContextWrapper
import java.io.ByteArrayOutputStream
import java.io.File
import java.nio.ByteBuffer
import java.nio.ByteOrder

/**
 * Byte-exact coverage of [pcm16ToWav] — the pure-JVM RIFF/WAV framer shared by
 * the talk-mode AudioTrack path and the chat autoplay save-and-play path. No
 * Android framework, so this runs headless on the plain JVM.
 *
 * A wrong header field here is silent: `MediaPlayer` / desktop tools either
 * refuse the file or play it at the wrong rate. So we assert every field of the
 * 44-byte canonical PCM header against the WAV spec, not just "it parses".
 */
class PcmWavTest {

    private fun chunk(tag: String, payload: ByteArray): ByteArray =
        ByteArrayOutputStream().apply {
            write(tag.toByteArray(Charsets.US_ASCII))
            write(ByteBuffer.allocate(4).order(ByteOrder.LITTLE_ENDIAN).putInt(payload.size).array())
            write(payload)
            if (payload.size % 2 != 0) write(0)
        }.toByteArray()

    private fun wavWithChunks(
        pcm: ByteArray,
        sampleRate: Int = 24_000,
        format: Int = 1,
    ): ByteArray {
        val fmt = ByteBuffer.allocate(16).order(ByteOrder.LITTLE_ENDIAN)
            .putShort(format.toShort())
            .putShort(1)
            .putInt(sampleRate)
            .putInt(sampleRate * 2)
            .putShort(2)
            .putShort(16)
            .array()
        val body = ByteArrayOutputStream().apply {
            write("WAVE".toByteArray(Charsets.US_ASCII))
            write(chunk("JUNK", byteArrayOf(9, 8, 7))) // odd-sized chunk and pad byte
            write(chunk("fmt ", fmt))
            write(chunk("data", pcm))
        }.toByteArray()
        return ByteArrayOutputStream().apply {
            write("RIFF".toByteArray(Charsets.US_ASCII))
            write(ByteBuffer.allocate(4).order(ByteOrder.LITTLE_ENDIAN).putInt(body.size).array())
            write(body)
        }.toByteArray()
    }

    /** Exercises the same parser called after TextToSpeech.synthesizeToFile. */
    private fun readWavThroughSystemProvider(
        bytes: ByteArray,
        maxPcmBytes: Int = 1_000_000,
    ): Pair<ByteArray, Int> {
        val file = File.createTempFile("lingxi-audio-test", ".wav")
        return try {
            file.writeBytes(bytes)
            val provider = SystemTextToSpeechTts(ContextWrapper(null))
            val parser = SystemTextToSpeechTts::class.java.getDeclaredMethod(
                "readWav",
                File::class.java,
                Int::class.javaPrimitiveType,
            )
                .apply { isAccessible = true }
            @Suppress("UNCHECKED_CAST")
            parser.invoke(provider, file, maxPcmBytes) as Pair<ByteArray, Int>
        } finally {
            file.delete()
        }
    }

    /** Read a 4-byte ASCII tag at [offset]. */
    private fun ByteArray.ascii(offset: Int, len: Int): String =
        String(copyOfRange(offset, offset + len), Charsets.US_ASCII)

    /** Read a little-endian 32-bit int at [offset] (WAV is always LE). */
    private fun ByteArray.leInt(offset: Int): Int =
        ByteBuffer.wrap(this, offset, 4).order(ByteOrder.LITTLE_ENDIAN).int

    /** Read a little-endian 16-bit unsigned short at [offset]. */
    private fun ByteArray.leShort(offset: Int): Int =
        ByteBuffer.wrap(this, offset, 2).order(ByteOrder.LITTLE_ENDIAN).short.toInt() and 0xFFFF

    // --- header magic + total size --------------------------------------

    @Test
    fun header_hasRiffWaveMagic_andCanonical44ByteHeader() {
        val pcm = ByteArray(100) { (it and 0xFF).toByte() }
        val wav = pcm16ToWav(pcm, sampleRateHz = 16_000, channels = 1)

        // Total file = 44-byte header + payload.
        assertEquals(44 + pcm.size, wav.size)

        assertEquals("RIFF", wav.ascii(0, 4))
        assertEquals("WAVE", wav.ascii(8, 4))
        assertEquals("fmt ", wav.ascii(12, 4))
        assertEquals("data", wav.ascii(36, 4))
    }

    @Test
    fun header_riffChunkSize_is36PlusDataSize() {
        val pcm = ByteArray(200)
        val wav = pcm16ToWav(pcm, sampleRateHz = 44_100, channels = 1)
        // RIFF size = whole file minus the 8-byte "RIFF"+size prefix = 36 + data.
        assertEquals(36 + pcm.size, wav.leInt(4))
        assertEquals(wav.size - 8, wav.leInt(4))
    }

    // --- fmt sub-chunk --------------------------------------------------

    @Test
    fun fmtChunk_pcmMono_hasCorrectByteRateBlockAlignAndBitDepth() {
        val sampleRate = 16_000
        val channels = 1
        val pcm = ByteArray(64)
        val wav = pcm16ToWav(pcm, sampleRateHz = sampleRate, channels = channels)

        assertEquals(16, wav.leInt(16))           // fmt chunk size for PCM
        assertEquals(1, wav.leShort(20))          // audio format = PCM (1)
        assertEquals(channels, wav.leShort(22))   // num channels
        assertEquals(sampleRate, wav.leInt(24))   // sample rate
        // byteRate = sampleRate * channels * bytesPerSample(2)
        assertEquals(sampleRate * channels * 2, wav.leInt(28))
        assertEquals(channels * 2, wav.leShort(32)) // blockAlign
        assertEquals(16, wav.leShort(34))           // bits per sample
    }

    @Test
    fun fmtChunk_stereo_doublesByteRateAndBlockAlign() {
        val sampleRate = 44_100
        val channels = 2
        val wav = pcm16ToWav(ByteArray(8), sampleRateHz = sampleRate, channels = channels)

        assertEquals(channels, wav.leShort(22))
        assertEquals(sampleRate, wav.leInt(24))
        // Stereo 16-bit: byteRate = 44100 * 2 * 2 = 176400, blockAlign = 4.
        assertEquals(sampleRate * channels * 2, wav.leInt(28))
        assertEquals(176_400, wav.leInt(28))
        assertEquals(channels * 2, wav.leShort(32))
        assertEquals(4, wav.leShort(32))
    }

    // --- data sub-chunk + payload round-trip ----------------------------

    @Test
    fun dataChunk_sizeMatchesPcmLength_andPayloadIsByteIdentical() {
        // Distinct, non-zero payload so a truncation/offset bug is visible.
        val pcm = ByteArray(513) { ((it * 7 + 3) and 0xFF).toByte() }
        val wav = pcm16ToWav(pcm, sampleRateHz = 24_000, channels = 1)

        assertEquals(pcm.size, wav.leInt(40)) // data sub-chunk size

        // Payload after the 44-byte header is byte-for-byte the input PCM.
        val body = wav.copyOfRange(44, wav.size)
        assertArrayEquals(pcm, body)
    }

    @Test
    fun emptyPcm_producesBareHeader_withZeroDataSize() {
        val wav = pcm16ToWav(ByteArray(0), sampleRateHz = 8_000, channels = 1)
        assertEquals(44, wav.size)
        assertEquals(0, wav.leInt(40))   // data size
        assertEquals(36, wav.leInt(4))   // riff size = 36 + 0
        assertEquals("data", wav.ascii(36, 4))
    }

    @Test
    fun defaultChannels_isMono() {
        // channels defaults to 1; verify the default path matches explicit mono.
        val pcm = ByteArray(16) { it.toByte() }
        val viaDefault = pcm16ToWav(pcm, sampleRateHz = 16_000)
        val viaExplicit = pcm16ToWav(pcm, sampleRateHz = 16_000, channels = 1)
        assertArrayEquals(viaExplicit, viaDefault)
        assertEquals(1, viaDefault.leShort(22))
    }

    @Test
    fun rejectsUnsupportedChannelCount() {
        val ex = runCatching { pcm16ToWav(ByteArray(4), sampleRateHz = 16_000, channels = 3) }
        assertTrue("3 channels must be rejected", ex.isFailure)
        assertTrue(ex.exceptionOrNull() is IllegalArgumentException)
    }

    @Test
    fun parserFindsPcmAndSampleRateAcrossUnknownOddSizedRiffChunks() {
        val pcm = byteArrayOf(1, 0, 2, 0, 3, 0)

        val (actualPcm, actualRate) = readWavThroughSystemProvider(wavWithChunks(pcm))

        assertArrayEquals(pcm, actualPcm)
        assertEquals(24_000, actualRate)
    }

    @Test
    fun parserRejectsUnsupportedWavEncoding() {
        val result = runCatching {
            readWavThroughSystemProvider(wavWithChunks(byteArrayOf(1, 2, 3, 4), format = 3))
        }

        assertTrue("float WAV is not PCM16 and must be rejected", result.isFailure)
    }

    @Test
    fun parserRejectsDataLargerThanRequestLimitBeforeAllocatingPcm() {
        val result = runCatching {
            readWavThroughSystemProvider(
                wavWithChunks(byteArrayOf(1, 0, 2, 0, 3, 0)),
                maxPcmBytes = 4,
            )
        }

        assertTrue("oversize output must fail against the request's raw-byte limit", result.isFailure)
    }
}
