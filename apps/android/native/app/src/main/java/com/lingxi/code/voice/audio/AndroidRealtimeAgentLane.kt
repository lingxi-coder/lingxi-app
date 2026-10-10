package com.lingxi.code.voice.audio

import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Job
import kotlinx.coroutines.NonCancellable
import kotlinx.coroutines.async
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import org.json.JSONObject
import java.util.Base64

internal interface RealtimeAgentCallbacks {
    fun onReady()
    fun onTranscript(text: String, assistant: Boolean, final: Boolean)
    fun onPlayback(playing: Boolean)
    fun onError(message: String)
    fun onClosed()
}

/** A bounded native PCM lane; all history, tool approval and provider control stay in Harness. */
internal class AndroidRealtimeAgentLane(
    private val scope: CoroutineScope,
    private val provider: ProviderAudioBridge,
    private val driver: AndroidAudioDeviceDriver,
    private val request: DeviceAudioRequest,
    private val configuration: AudioConfigurationV4,
    private val captureLease: AudioLease,
    private val acquirePlayback: suspend () -> AudioLease,
    private val releasePlayback: suspend (AudioLease) -> Unit,
    private val isLive: () -> Boolean,
    private val callbacks: RealtimeAgentCallbacks,
) {
    private val events = Channel<String>(32)
    private val queuedEventBytes = java.util.concurrent.atomic.AtomicLong(0)
    private var providerSession: ProviderRealtimeSession? = null
    private var captureJob: Job? = null
    private var playbackLease: AudioLease? = null
    private var chunks: Channel<ByteArray>? = null
    private var playbackJob: kotlinx.coroutines.Deferred<Unit>? = null
    private val playedItems = LinkedHashSet<String?>()
    private data class ItemRange(val itemId: String?, val startFrame: Long, var endFrame: Long)
    private val itemRanges = mutableListOf<ItemRange>()
    private var outputFormat: RealtimePcmFormat? = null
    private var outputBytes = 0L
    private var canTruncate = false
    private var closedNormally = false
    private val transcripts = mutableMapOf<String, String>()

    fun commitInput() {
        driver.pauseStreamCapture(captureLease, true)
        scope.launch {
            try { if (isLive()) providerSession?.commitInput() }
            catch (error: Throwable) { events.close(error) }
        }
    }

    suspend fun run(onConnected: () -> Unit) {
        try {
            providerSession = provider.openRealtime(request, configuration) { event ->
                val type = if (event.length <= 65_536) runCatching { JSONObject(event).optString("type") }.getOrNull() else null
                if (type == "usage") {
                    runCatching { AndroidAudioUsageJournal.record(request.identity.id, "realtime", JSONObject(event)) }
                }
                if (!isLive()) return@openRealtime
                if (type == "interrupted") {
                    // No AEC is available: abort the turn-based lane immediately, including a pending playback drain.
                    scope.coroutineContext[Job]?.cancel(CancellationException("Realtime response was interrupted"))
                    return@openRealtime
                }
                val budget = maxOf(65_536L, request.maxPayloadBytes * 2)
                if (event.length.toLong() > budget) {
                    events.close(AudioOperationException(DeviceAudioErrorKind.MediaTooLarge, "Realtime audio event exceeds its bound."))
                    return@openRealtime
                }
                if (queuedEventBytes.addAndGet(event.length.toLong()) > budget) {
                    queuedEventBytes.addAndGet(-event.length.toLong())
                    events.close(AudioOperationException(DeviceAudioErrorKind.MediaTooLarge, "Realtime audio event queue exceeded its bound."))
                    return@openRealtime
                }
                if (events.trySend(event).isFailure) {
                    queuedEventBytes.addAndGet(-event.length.toLong())
                    events.close(AudioOperationException(DeviceAudioErrorKind.MediaTooLarge, "Realtime audio event queue exceeded its bound."))
                }
            }
            onConnected()
            for (raw in events) {
                queuedEventBytes.addAndGet(-raw.length.toLong())
                if (!isLive()) throw CancellationException("Realtime session is no longer current")
                val event = JSONObject(raw)
                when (event.optString("type")) {
                    "session_ready" -> {
                        val input = event.getJSONObject("inputFormat").toPcmFormat()
                        outputFormat = event.getJSONObject("outputFormat").toPcmFormat()
                        input.requireSupported(); outputFormat!!.requireSupported()
                        canTruncate = event.optJSONObject("capabilities")?.optBoolean("audioTruncation") == true
                        captureJob = scope.launch {
                            try {
                                driver.streamCapture(captureLease, input) { pcm ->
                                    if (isLive()) checkNotNull(providerSession).sendAudio(pcm)
                                }
                            } catch (error: Throwable) { events.close(error) }
                        }
                        callbacks.onReady()
                    }
                    "audio_delta" -> {
                        val format = event.toPcmFormat()
                        if (format != outputFormat) throw AudioOperationException(DeviceAudioErrorKind.Unsupported, "Realtime output format changed during playback.")
                        val encoded = event.getString("audioBase64")
                        if (encoded.length.toLong() > ((request.maxPayloadBytes + 2) / 3) * 4) {
                            throw AudioOperationException(DeviceAudioErrorKind.MediaTooLarge, "Realtime audio chunk exceeds its bound.")
                        }
                        val pcm = Base64.getDecoder().decode(encoded)
                        val itemId = event.optionalItemId()
                        val startFrame = outputBytes / 2
                        outputBytes += pcm.size
                        if (pcm.size % 2 != 0 || outputBytes > request.maxPayloadBytes) {
                            throw AudioOperationException(DeviceAudioErrorKind.MediaTooLarge, "Realtime response exceeds its audio bound.")
                        }
                        playedItems.add(itemId)
                        val last = itemRanges.lastOrNull()
                        if (last != null && last.itemId == itemId && last.endFrame == startFrame) last.endFrame = outputBytes / 2
                        else itemRanges.add(ItemRange(itemId, startFrame, outputBytes / 2))
                        if (playedItems.size > 128 || itemRanges.size > 512) throw AudioOperationException(DeviceAudioErrorKind.MediaTooLarge, "Realtime audio item count exceeds its bound.")
                        if (chunks == null) {
                            // This device lane has no verified AEC: mute input while native output plays.
                            driver.pauseStreamCapture(captureLease, true)
                            val lease = acquirePlayback()
                            playbackLease = lease
                            chunks = Channel(16)
                            val inputChunks = checkNotNull(chunks)
                            playbackJob = scope.async {
                                try { driver.streamPlayback(lease, format, inputChunks) }
                                catch (error: Throwable) { inputChunks.close(error); events.close(error); throw error }
                            }
                            callbacks.onPlayback(true)
                        }
                        checkNotNull(chunks).send(pcm)
                    }
                    "turn_completed" -> {
                        finishPlayback(acknowledge = true)
                        driver.pauseStreamCapture(captureLease, false)
                    }
                    "turn_started" -> transcripts.clear()
                    "transcript" -> {
                        val assistant = event.optString("role") == "assistant"
                        val item = event.optionalItemId()
                        val key = "${event.optString("role")}:${item ?: event.optString("turnId")}"
                        val text = event.optString("text")
                        val accumulated = if (event.optString("update") == "delta") transcripts[key].orEmpty() + text else text
                        if (accumulated.length > 65_536) throw AudioOperationException(DeviceAudioErrorKind.MediaTooLarge, "Realtime transcript exceeds its bound.")
                        transcripts[key] = accumulated
                        callbacks.onTranscript(accumulated, assistant, event.optBoolean("final"))
                    }
                    "error" -> throw AudioOperationException(DeviceAudioErrorKind.Unavailable, event.optString("message", "Realtime conversation failed."))
                    "closed" -> { closedNormally = true; break }
                }
            }
        } finally {
            val playedMs = playbackLease?.let(driver::streamingPlaybackPositionMs)
            val rate = outputFormat?.sampleRateHz ?: 0
            val playedFrame = (playedMs ?: 0) * rate / 1000
            val audibleItem = itemRanges.lastOrNull { playedFrame > it.startFrame }
            val itemPlayedMs = audibleItem?.let { ((minOf(playedFrame, it.endFrame) - it.startFrame) * 1000 / rate).coerceIn(0, UInt.MAX_VALUE.toLong()).toUInt() }
            withContext(NonCancellable) {
                if (!closedNormally && canTruncate && audibleItem?.itemId != null && itemPlayedMs != null) {
                    runCatching { providerSession?.interrupt(audibleItem.itemId, itemPlayedMs) }
                }
                driver.pauseStreamCapture(captureLease, true)
                runCatching { driver.stop(captureLease) }
                captureJob?.cancel(); captureJob?.join()
                runCatching { finishPlayback(acknowledge = false) }
                runCatching { if (closedNormally) providerSession?.close() else providerSession?.abort() }
            }
            events.close()
        }
    }

    private suspend fun finishPlayback(acknowledge: Boolean) {
        val lease = playbackLease
        val job = playbackJob
        chunks?.close()
        if (!acknowledge) { lease?.let { driver.stop(it) }; job?.cancel() }
        try {
            job?.await()
            if (acknowledge && isLive()) playedItems.forEach { providerSession?.playbackCompleted(it) }
        } catch (cancelled: CancellationException) {
            if (acknowledge) throw cancelled
        } finally {
            lease?.let { releasePlayback(it) }
            playbackLease = null; playbackJob = null; chunks = null; outputBytes = 0
            playedItems.clear(); itemRanges.clear()
            if (isLive()) callbacks.onPlayback(false)
        }
    }
}

private fun JSONObject.toPcmFormat() = RealtimePcmFormat(getInt("sampleRateHz"), getInt("channels"), getString("encoding"))
private fun JSONObject.optionalItemId(): String? = if (isNull("itemId")) null else optString("itemId").takeIf(String::isNotBlank)
