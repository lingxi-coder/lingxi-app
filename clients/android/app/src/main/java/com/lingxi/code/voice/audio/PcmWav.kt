package com.lingxi.code.voice.audio

import java.nio.ByteBuffer
import java.nio.ByteOrder
import java.io.File
import java.io.RandomAccessFile

/**
 * Wraps raw 16-bit signed little-endian PCM in a minimal RIFF/WAV
 * container so non-streaming players ([android.media.MediaPlayer], desktop
 * audio tools, file managers) can read it. The header is 44 bytes; total
 * file size is `pcm.size + 44`.
 *
 * Pure JVM — no Android dependency. Shared by the talk-mode AudioTrack path
 * (streaming) and the chat autoplay path (save-and-play).
 */
fun pcm16ToWav(pcm: ByteArray, sampleRateHz: Int, channels: Int = 1): ByteArray {
    require(channels == 1 || channels == 2) { "channels must be 1 or 2" }
    val bitsPerSample = 16
    val byteRate = sampleRateHz * channels * (bitsPerSample / 8)
    val blockAlign = channels * (bitsPerSample / 8)
    val dataSize = pcm.size
    val riffSize = 36 + dataSize  // fmt(16) + data hdr(8) + data + RIFF type(4) - 8

    return ByteBuffer.allocate(44 + dataSize).order(ByteOrder.LITTLE_ENDIAN).apply {
        put("RIFF".toByteArray(Charsets.US_ASCII))
        putInt(riffSize)
        put("WAVE".toByteArray(Charsets.US_ASCII))
        put("fmt ".toByteArray(Charsets.US_ASCII))
        putInt(16)                          // fmt chunk size for PCM
        putShort(1)                         // audio format = PCM
        putShort(channels.toShort())
        putInt(sampleRateHz)
        putInt(byteRate)
        putShort(blockAlign.toShort())
        putShort(bitsPerSample.toShort())
        put("data".toByteArray(Charsets.US_ASCII))
        putInt(dataSize)
        put(pcm)
    }.array()
}

/**
 * Reads PCM16 little-endian mono data from a RIFF/WAVE file. RIFF chunks may
 * appear in any order and unknown chunks are skipped, including their required
 * pad byte when their declared size is odd.
 */
internal fun readPcm16Wav(
    file: File,
    maxPcmBytes: Int,
): Pair<ByteArray, Int> {
    require(maxPcmBytes >= 0) { "audio limit must not be negative" }
    require(file.exists()) { "WAV output is missing" }
    RandomAccessFile(file, "r").use { raf ->
        require(raf.length() >= 12L) { "WAV header is truncated" }
        val riff = ByteArray(12)
        raf.readFully(riff)
        require(riff.ascii(0, 4) == "RIFF" && riff.ascii(8, 4) == "WAVE") {
            "invalid RIFF/WAVE header"
        }
        val riffSize = riff.leUInt(4)
        val riffEnd = 8L + riffSize
        require(riffEnd >= 12L && riffEnd <= raf.length()) { "RIFF size exceeds the file" }

        var cursor = 12L
        var sampleRateHz: Int? = null
        var dataOffset: Long? = null
        var dataSize: Long? = null
        while (cursor + 8L <= riffEnd) {
            raf.seek(cursor)
            val chunkHeader = ByteArray(8)
            raf.readFully(chunkHeader)
            val tag = chunkHeader.ascii(0, 4)
            val chunkSize = chunkHeader.leUInt(4)
            val payloadOffset = cursor + 8L
            val nextChunk = payloadOffset + chunkSize + (chunkSize and 1L)
            require(nextChunk <= riffEnd && nextChunk <= raf.length()) {
                "RIFF chunk exceeds the declared file size"
            }

            when (tag) {
                "fmt " -> {
                    require(chunkSize >= 16L) { "WAV format chunk is too short" }
                    val format = ByteArray(16)
                    raf.seek(payloadOffset)
                    raf.readFully(format)
                    val audioFormat = format.leUShort(0)
                    val channels = format.leUShort(2)
                    val rate = format.leUInt(4)
                    val byteRate = format.leUInt(8)
                    val blockAlign = format.leUShort(12)
                    val bitsPerSample = format.leUShort(14)
                    require(audioFormat == 1 && channels == 1 && bitsPerSample == 16) {
                        "WAV must contain PCM16 mono audio"
                    }
                    require(rate in 1L..Int.MAX_VALUE.toLong()) { "invalid WAV sample rate" }
                    require(blockAlign == 2 && byteRate == rate * 2L) { "invalid PCM frame layout" }
                    sampleRateHz = rate.toInt()
                }
                "data" -> {
                    require(dataOffset == null) { "multiple WAV data chunks are unsupported" }
                    dataOffset = payloadOffset
                    dataSize = chunkSize
                }
            }
            cursor = nextChunk
        }

        val rate = requireNotNull(sampleRateHz) { "WAV format chunk is missing" }
        val offset = requireNotNull(dataOffset) { "WAV data chunk is missing" }
        val size = requireNotNull(dataSize)
        if (size > maxPcmBytes.toLong()) {
            throw AudioDriverException(
                DeviceAudioError(DeviceAudioErrorKind.MediaTooLarge, "WAV audio exceeds the payload limit"),
            )
        }
        require(size <= Int.MAX_VALUE.toLong()) { "WAV audio is too large" }
        require(size % 2L == 0L) { "PCM16 data is not sample-aligned" }
        val pcm = ByteArray(size.toInt())
        raf.seek(offset)
        raf.readFully(pcm)
        return pcm to rate
    }
}

private fun ByteArray.ascii(offset: Int, size: Int): String =
    String(this, offset, size, Charsets.US_ASCII)

private fun ByteArray.leUInt(offset: Int): Long =
    Integer.toUnsignedLong(ByteBuffer.wrap(this, offset, 4).order(ByteOrder.LITTLE_ENDIAN).int)

private fun ByteArray.leUShort(offset: Int): Int =
    ByteBuffer.wrap(this, offset, 2).order(ByteOrder.LITTLE_ENDIAN).short.toInt() and 0xFFFF
