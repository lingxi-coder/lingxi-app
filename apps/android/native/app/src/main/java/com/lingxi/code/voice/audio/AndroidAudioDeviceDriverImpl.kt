package com.lingxi.code.voice.audio

import android.Manifest
import android.content.Context
import android.content.pm.PackageManager
import android.media.AudioDeviceCallback
import android.media.AudioDeviceInfo
import android.media.AudioManager
import android.media.AudioAttributes
import android.media.AudioFormat
import android.media.AudioTrack
import android.media.MediaRecorder
import android.media.MediaPlayer
import android.net.Uri
import android.os.Build
import android.os.Handler
import android.os.Looper
import androidx.core.content.ContextCompat
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.NonCancellable
import kotlinx.coroutines.currentCoroutineContext
import kotlinx.coroutines.delay
import kotlinx.coroutines.ensureActive
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.withContext
import kotlinx.coroutines.withTimeout
import java.io.File
import java.util.UUID
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.atomic.AtomicBoolean
import kotlin.math.max

internal class AudioDriverException(val error: DeviceAudioError) : Exception(error.message)

/** Owns the Android recorder and AudioTrack handles behind the app-scoped service lease. */
internal class AndroidAudioDeviceDriverImpl(
    context: Context,
    private val onAudioRouteInvalidated: () -> Unit = {},
) : AndroidAudioDeviceDriver {
    private data class RecordingState(
        val lease: AudioLease,
        val handle: String,
        val recorder: MediaRecorder,
        val file: File,
        val maxBytes: Long,
        val tooLarge: AtomicBoolean = AtomicBoolean(false),
        val stopped: AtomicBoolean = AtomicBoolean(false),
        val released: AtomicBoolean = AtomicBoolean(false),
        @Volatile var stopFailure: Throwable? = null,
    )

    private class PlaybackState(
        val lease: AudioLease,
        val track: AudioTrack,
    ) {
        val stopped = AtomicBoolean(false)
        val released = AtomicBoolean(false)
        val settled = CompletableDeferred<Unit>()
        @Volatile var focus: AudioFocusController? = null

        @Synchronized fun stopNative() {
            stopped.set(true)
            runCatching { focus?.unregister() }
            if (released.get()) return
            runCatching { track.pause() }
            runCatching { track.flush() }
            runCatching { track.stop() }
            track.release()
            released.set(true)
        }
    }

    private class MediaPlaybackState(
        val lease: AudioLease,
        val player: MediaPlayer,
        private val onTerminal: (DeviceAudioError?) -> Unit,
    ) {
        val stopped = AtomicBoolean(false)
        val released = AtomicBoolean(false)
        val prepared = CompletableDeferred<Unit>()
        private val completionSent = AtomicBoolean(false)
        @Volatile var terminalError: DeviceAudioError? = null

        fun snapshot(): DeviceMediaPlaybackState {
            if (released.get()) return DeviceMediaPlaybackState(false, 0, 0)
            return DeviceMediaPlaybackState(
                playing = runCatching { player.isPlaying }.getOrDefault(false),
                positionMs = runCatching { player.currentPosition }.getOrDefault(0).coerceAtLeast(0),
                durationMs = runCatching { player.duration }.getOrDefault(0).coerceAtLeast(0),
            )
        }

        fun complete(error: DeviceAudioError? = null) {
            if (!stopped.get() && completionSent.compareAndSet(false, true)) {
                terminalError = error
                onTerminal(error)
            }
        }

        @Synchronized fun stopNative() {
            stopped.set(true)
            if (!prepared.isCompleted) prepared.completeExceptionally(CancellationException("media playback was stopped during preparation"))
            if (released.get()) return
            runCatching { if (player.isPlaying) player.stop() }
            player.release()
            released.set(true)
        }
    }

    private val appContext = context.applicationContext
    private val recordings = ConcurrentHashMap<Long, RecordingState>()
    private val wavCaptures = ConcurrentHashMap<Long, AndroidWavCapture>()
    private val streamCaptures = ConcurrentHashMap<Long, AndroidRealtimePcmCapture>()
    private val streamCapturePause = ConcurrentHashMap<Long, Boolean>()
    private val streamPlayback = ConcurrentHashMap<Long, AndroidRealtimePcmPlayback>()
    private val playback = ConcurrentHashMap<Long, PlaybackState>()
    private val mediaPlayback = ConcurrentHashMap<Long, MediaPlaybackState>()
    private val recorderLock = Mutex()

    init {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.M) {
            val audioManager = appContext.getSystemService(Context.AUDIO_SERVICE) as? AudioManager
            audioManager?.registerAudioDeviceCallback(
                object : AudioDeviceCallback() {
                    override fun onAudioDevicesRemoved(removedDevices: Array<out AudioDeviceInfo>) {
                        if (removedDevices.any { it.isSink || it.isSource }) onAudioRouteInvalidated()
                    }
                },
                Handler(Looper.getMainLooper()),
            )
        }
    }

    override suspend fun streamCapture(lease: AudioLease, format: RealtimePcmFormat, onChunk: suspend (ByteArray) -> Unit) {
        val capture = AndroidRealtimePcmCapture()
        streamCaptures[lease.leaseId] = capture
        capture.paused.set(streamCapturePause[lease.leaseId] ?: false)
        try { capture.run(format, onChunk) } finally { streamCaptures.remove(lease.leaseId, capture); streamCapturePause.remove(lease.leaseId) }
    }

    override fun pauseStreamCapture(lease: AudioLease, paused: Boolean) {
        streamCapturePause[lease.leaseId] = paused
        streamCaptures[lease.leaseId]?.paused?.set(paused)
    }
    override fun streamingPlaybackPositionMs(lease: AudioLease): Long? = streamPlayback[lease.leaseId]?.positionMs()

    override suspend fun streamPlayback(lease: AudioLease, format: RealtimePcmFormat, chunks: kotlinx.coroutines.channels.ReceiveChannel<ByteArray>) {
        val playback = AndroidRealtimePcmPlayback(appContext)
        streamPlayback[lease.leaseId] = playback
        try { playback.run(format, chunks) } finally { streamPlayback.remove(lease.leaseId, playback) }
    }

    override suspend fun captureWav(
        lease: AudioLease, maxPayloadBytes: Int, untilSilence: Boolean, onReady: () -> Unit,
    ): DeviceAudioCapture {
        val capture = AndroidWavCapture(appContext)
        if (wavCaptures.putIfAbsent(lease.leaseId, capture) != null) {
            throw AudioOperationException(DeviceAudioErrorKind.Busy, "Microphone capture is already active.")
        }
        return try {
            capture.capture(maxPayloadBytes, untilSilence, onReady)
        } finally {
            wavCaptures.remove(lease.leaseId, capture)
        }
    }

    override fun finishWavCapture(lease: AudioLease) {
        wavCaptures[lease.leaseId]?.stop()
    }

    override suspend fun startRecording(
        lease: AudioLease,
        sampleRateHz: Int,
        format: String,
        maxPayloadBytes: Int,
        onTerminated: (DeviceAudioError) -> Unit,
        mayStart: () -> Boolean,
    ): String = recorderLock.withLock {
        if (ContextCompat.checkSelfPermission(appContext, Manifest.permission.RECORD_AUDIO) != PackageManager.PERMISSION_GRANTED) {
            throw AudioDriverException(DeviceAudioError(DeviceAudioErrorKind.PermissionDenied, "Microphone permission is not granted."))
        }
        if (!format.equals("audio/m4a", ignoreCase = true)) {
            throw AudioDriverException(DeviceAudioError(DeviceAudioErrorKind.Unsupported, "Only audio/m4a capture is supported."))
        }
        if (!mayStart()) throw CancellationException("recording was cancelled before device start")
        if (recordings.values.any { !it.stopped.get() }) {
            throw AudioDriverException(DeviceAudioError(DeviceAudioErrorKind.Busy, "another microphone capture is active"))
        }

        val handle = UUID.randomUUID().toString()
        val output = File(appContext.cacheDir, "audio-recordings/$handle.m4a")
        output.parentFile?.mkdirs()
        val recorder = newMediaRecorder(appContext)
        try {
            recorder.setAudioSource(MediaRecorder.AudioSource.MIC)
            recorder.setOutputFormat(MediaRecorder.OutputFormat.MPEG_4)
            recorder.setAudioEncoder(MediaRecorder.AudioEncoder.AAC)
            recorder.setAudioSamplingRate(sampleRateHz)
            recorder.setMaxFileSize(maxPayloadBytes.toLong())
            recorder.setOutputFile(output.absolutePath)
            val state = RecordingState(lease, handle, recorder, output, maxPayloadBytes.toLong())
            recorder.setOnInfoListener { _, what, _ ->
                if (what == MediaRecorder.MEDIA_RECORDER_INFO_MAX_FILESIZE_REACHED && state.tooLarge.compareAndSet(false, true)) {
                    state.stopped.set(true) // MediaRecorder has already stopped itself.
                    onTerminated(DeviceAudioError(DeviceAudioErrorKind.MediaTooLarge, "recording exceeds the payload limit"))
                }
            }
            recorder.prepare()
            if (!mayStart()) throw CancellationException("recording was cancelled before device start")
            recorder.start()
            recordings[lease.leaseId] = state
            if (!mayStart()) {
                stopAndDelete(state)
                throw CancellationException("recording was cancelled during device start")
            }
            handle
        } catch (error: Throwable) {
            if (recordings[lease.leaseId] == null) {
                runCatching { recorder.release() }
                output.delete()
            }
            if (error is AudioDriverException || error is CancellationException) throw error
            throw AudioDriverException(DeviceAudioError(DeviceAudioErrorKind.NativeFailure, error.message ?: "Android microphone could not start"))
        }
    }

    override suspend fun stopRecording(
        lease: AudioLease,
        handle: String,
        maxPayloadBytes: Int,
    ): DeviceAudioCapture = recorderLock.withLock {
        val state = recordings[lease.leaseId]
        if (state == null || state.handle != handle || state.lease.owner != lease.owner) {
            throw AudioDriverException(DeviceAudioError(DeviceAudioErrorKind.NotRecording, "recording handle is unavailable"))
        }
        try {
            val stopFailure = stopRecorder(state)
            if (stopFailure != null) {
                throw AudioDriverException(DeviceAudioError(DeviceAudioErrorKind.NativeFailure, stopFailure.message ?: "Android recorder could not stop"))
            }
            if (state.tooLarge.get() || state.file.length() > maxPayloadBytes.toLong()) {
                throw AudioDriverException(DeviceAudioError(DeviceAudioErrorKind.MediaTooLarge, "recording exceeds the payload limit"))
            }
            val size = state.file.length()
            if (size <= 0L) throw AudioDriverException(DeviceAudioError(DeviceAudioErrorKind.NativeFailure, "recording produced no media"))
            if (size > maxPayloadBytes.toLong() || size > Int.MAX_VALUE) {
                throw AudioDriverException(DeviceAudioError(DeviceAudioErrorKind.MediaTooLarge, "recording exceeds the payload limit"))
            }
            DeviceAudioCapture(state.file.readBytes(), "audio/m4a")
        } catch (error: Throwable) {
            if (error is AudioDriverException) throw error
            throw AudioDriverException(DeviceAudioError(DeviceAudioErrorKind.NativeFailure, error.message ?: "Android recording could not be finalized"))
        } finally {
            if (state.released.get()) {
                recordings.remove(lease.leaseId, state)
                state.file.delete()
            }
        }
    }

    override suspend fun play(lease: AudioLease, pcm: ByteArray, sampleRateHz: Int): Long {
        if (pcm.isEmpty() || pcm.size % 2 != 0 || sampleRateHz <= 0) {
            throw AudioDriverException(DeviceAudioError(DeviceAudioErrorKind.InvalidRequest, "playback requires aligned PCM16 audio and a valid sample rate"))
        }
        val frameCount = pcm.size / 2
        val minimumBuffer = AudioTrack.getMinBufferSize(
            sampleRateHz,
            AudioFormat.CHANNEL_OUT_MONO,
            AudioFormat.ENCODING_PCM_16BIT,
        )
        if (minimumBuffer < 0) {
            throw AudioDriverException(DeviceAudioError(DeviceAudioErrorKind.Unsupported, "Android audio output does not support this sample rate"))
        }
        val track = AudioTrack.Builder()
            .setAudioAttributes(
                AudioAttributes.Builder()
                    .setUsage(AudioAttributes.USAGE_ASSISTANT)
                    .setContentType(AudioAttributes.CONTENT_TYPE_SPEECH)
                    .build(),
            )
            .setAudioFormat(
                AudioFormat.Builder()
                    .setEncoding(AudioFormat.ENCODING_PCM_16BIT)
                    .setSampleRate(sampleRateHz)
                    .setChannelMask(AudioFormat.CHANNEL_OUT_MONO)
                    .build(),
            )
            .setBufferSizeInBytes(max(minimumBuffer, minOf(pcm.size, sampleRateHz * 2)))
            .setTransferMode(AudioTrack.MODE_STREAM)
            .build()
        if (track.state != AudioTrack.STATE_INITIALIZED) {
            runCatching { track.release() }
            throw AudioDriverException(DeviceAudioError(DeviceAudioErrorKind.Unavailable, "Android audio output is unavailable"))
        }
        val state = PlaybackState(lease, track)
        if (playback.putIfAbsent(lease.leaseId, state) != null) {
            state.stopNative()
            throw AudioDriverException(DeviceAudioError(DeviceAudioErrorKind.Busy, "audio playback is already active"))
        }
        val focus = AudioFocusController(
            appContext,
            object : AudioFocusListener {
                override fun onResume() {
                    // A completed OS interruption never revives an older operation.
                }

                override fun onPause() {
                    state.stopNative()
                }

                override fun onStop() = state.stopNative()
            },
        )
        state.focus = focus
        val startedAt = System.nanoTime()
        return try {
            if (!focus.register()) throw AudioDriverException(DeviceAudioError(DeviceAudioErrorKind.Busy, "another app owns audio focus"))
            currentCoroutineContext().ensureActive()
            if (state.stopped.get()) throw CancellationException("playback was stopped before output began")
            track.play()
            withTimeout(playbackTimeoutMs(frameCount, sampleRateHz)) {
                var offset = 0
                while (offset < pcm.size) {
                    currentCoroutineContext().ensureActive()
                    if (state.stopped.get()) throw CancellationException("playback was interrupted")
                    val written = track.write(pcm, offset, pcm.size - offset, AudioTrack.WRITE_NON_BLOCKING)
                    if (state.stopped.get()) throw CancellationException("playback was interrupted")
                    if (written < 0) throw AudioDriverException(DeviceAudioError(DeviceAudioErrorKind.NativeFailure, "Android audio output failed ($written)"))
                    if (written == 0) delay(AUDIO_WRITE_RETRY_MS)
                    else offset += written
                }
                while (track.playbackHeadPosition.toLong() < frameCount.toLong()) {
                    currentCoroutineContext().ensureActive()
                    if (state.stopped.get()) throw CancellationException("playback was interrupted")
                    val remainingFrames = frameCount - track.playbackHeadPosition.toLong()
                    delay(((remainingFrames * 1_000L) / sampleRateHz).coerceIn(10L, 100L))
                }
            }
            ((System.nanoTime() - startedAt) / 1_000_000L).coerceAtLeast(0L)
        } finally {
            withContext(NonCancellable) {
                try {
                    state.stopNative()
                } finally {
                    if (state.released.get()) playback.remove(lease.leaseId, state)
                    state.settled.complete(Unit)
                }
            }
        }
    }

    override suspend fun playMedia(
        lease: AudioLease,
        target: String,
        mayStart: () -> Boolean,
        onTerminal: (DeviceAudioError?) -> Unit,
    ): DeviceMediaPlaybackState {
        if (!mayStart()) throw CancellationException("media playback was cancelled before device start")
        val state = withContext(Dispatchers.Main.immediate) {
            val player = MediaPlayer()
            val candidate = MediaPlaybackState(lease, player, onTerminal)
            if (mediaPlayback.putIfAbsent(lease.leaseId, candidate) != null) {
                runCatching { player.release() }
                throw AudioDriverException(DeviceAudioError(DeviceAudioErrorKind.Busy, "media playback is already active"))
            }
            candidate
        }
        return try {
            withContext(Dispatchers.Main.immediate) {
                val player = state.player
                if (target.contains("://")) {
                    player.setDataSource(appContext, Uri.parse(target))
                } else {
                    val file = File(target).canonicalFile
                    if (!file.exists() || !file.isFile) {
                        throw AudioDriverException(DeviceAudioError(DeviceAudioErrorKind.NotRecording, "media file is unavailable"))
                    }
                    player.setDataSource(file.path)
                }
                player.setOnPreparedListener { state.prepared.complete(Unit) }
                player.setOnCompletionListener { state.complete() }
                player.setOnErrorListener { _, what, extra ->
                    val error = DeviceAudioError(DeviceAudioErrorKind.NativeFailure, "Android media playback failed ($what/$extra)")
                    if (!state.prepared.isCompleted) {
                        state.prepared.completeExceptionally(
                            AudioDriverException(DeviceAudioError(DeviceAudioErrorKind.Unavailable, "Android media could not be prepared")),
                        )
                    }
                    state.complete(error)
                    true
                }
                if (!mayStart()) throw CancellationException("media playback was cancelled before preparation")
                player.prepareAsync()
            }
            withTimeout(MEDIA_PREPARE_TIMEOUT_MS) { state.prepared.await() }
            currentCoroutineContext().ensureActive()
            if (!mayStart()) throw CancellationException("media playback was cancelled before output began")
            withContext(Dispatchers.Main.immediate) { state.player.start() }
            state.terminalError?.let { throw AudioDriverException(it) }
            if (!mayStart()) throw CancellationException("media playback was cancelled during start")
            state.snapshot()
        } catch (error: Throwable) {
            withContext(NonCancellable + Dispatchers.Main.immediate) {
                state.stopNative()
                mediaPlayback.remove(lease.leaseId, state)
            }
            if (error is CancellationException || error is AudioDriverException) throw error
            throw AudioDriverException(DeviceAudioError(DeviceAudioErrorKind.NativeFailure, error.message ?: "Android media playback could not start"))
        }
    }

    override suspend fun controlMedia(lease: AudioLease, command: OffloadMediaCommand): DeviceMediaPlaybackState =
        withContext(Dispatchers.Main.immediate) {
            val state = mediaPlayback[lease.leaseId]
                ?: throw AudioDriverException(DeviceAudioError(DeviceAudioErrorKind.NotRecording, "media playback is unavailable"))
            if (state.lease != lease) {
                throw AudioDriverException(DeviceAudioError(DeviceAudioErrorKind.NotRecording, "media playback lease is stale"))
            }
            try {
                when (command) {
                    OffloadMediaCommand.PAUSE -> if (state.player.isPlaying) state.player.pause()
                    OffloadMediaCommand.RESUME -> if (!state.player.isPlaying) state.player.start()
                    OffloadMediaCommand.STATUS -> Unit
                    OffloadMediaCommand.STOP -> throw AudioDriverException(
                        DeviceAudioError(DeviceAudioErrorKind.InvalidRequest, "stop is handled by the audio service"),
                    )
                }
                state.snapshot()
            } catch (error: Throwable) {
                if (error is AudioDriverException) throw error
                throw AudioDriverException(DeviceAudioError(DeviceAudioErrorKind.NativeFailure, error.message ?: "Android media playback control failed"))
            }
        }

    override suspend fun stop(lease: AudioLease) {
        wavCaptures[lease.leaseId]?.stop()
        streamCaptures[lease.leaseId]?.stop()
        streamCapturePause.remove(lease.leaseId)
        streamPlayback[lease.leaseId]?.stop()
        recordings[lease.leaseId]?.let { state ->
            recorderLock.withLock { stopAndDelete(state) }
        }
        playback[lease.leaseId]?.let { state ->
            state.stopNative()
            withContext(NonCancellable) { state.settled.await() }
            playback.remove(lease.leaseId, state)
        }
        mediaPlayback[lease.leaseId]?.let { state ->
            withContext(NonCancellable + Dispatchers.Main.immediate) {
                state.stopNative()
                mediaPlayback.remove(lease.leaseId, state)
            }
        }
    }

    private fun stopAndDelete(state: RecordingState) {
        stopRecorder(state)
        if (state.released.get()) {
            recordings.remove(state.lease.leaseId, state)
            state.file.delete()
        }
    }

    private fun stopRecorder(state: RecordingState): Throwable? {
        if (state.stopped.compareAndSet(false, true)) {
            state.stopFailure = runCatching { state.recorder.stop() }.exceptionOrNull()
        }
        if (!state.released.get()) {
            state.recorder.release()
            state.released.set(true)
        }
        return state.stopFailure
    }

    @Suppress("DEPRECATION")
    private fun newMediaRecorder(context: Context): MediaRecorder =
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) MediaRecorder(context) else MediaRecorder()

    private fun playbackTimeoutMs(frameCount: Int, sampleRateHz: Int): Long {
        val expectedMs = (frameCount.toLong() * 1_000L + sampleRateHz - 1L) / sampleRateHz
        return (expectedMs + PLAYBACK_TIMEOUT_GRACE_MS).coerceIn(PLAYBACK_TIMEOUT_GRACE_MS, MAX_PLAYBACK_TIMEOUT_MS)
    }

    private companion object {
        const val AUDIO_WRITE_RETRY_MS = 10L
        const val PLAYBACK_TIMEOUT_GRACE_MS = 5_000L
        const val MAX_PLAYBACK_TIMEOUT_MS = 10 * 60 * 1_000L
        const val MEDIA_PREPARE_TIMEOUT_MS = 30_000L
    }
}
