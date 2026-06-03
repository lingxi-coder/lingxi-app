package com.lingxi.code.voice.audio

import java.nio.ByteBuffer
import java.nio.ByteOrder

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
