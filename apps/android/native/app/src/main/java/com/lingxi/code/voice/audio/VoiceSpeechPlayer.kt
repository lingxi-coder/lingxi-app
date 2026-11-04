package com.lingxi.code.voice.audio

import android.content.Context
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.CoroutineDispatcher
import kotlinx.coroutines.CoroutineStart
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.currentCoroutineContext
import kotlinx.coroutines.delay
import kotlinx.coroutines.ensureActive
import kotlinx.coroutines.launch
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import java.util.UUID

/** Owns UI speech playback through the app-scoped audio service. */
internal class VoiceSpeechPlayer(
    private val performAudio: suspend (AudioOwnerKey, DeviceAudioOperation) -> DeviceAudioResult,
    private val admission: Mutex? = null,
    dispatcher: CoroutineDispatcher = Dispatchers.Main.immediate,
) {
    constructor(context: Context, admission: Mutex? = null) : this(
        performAudio = { owner, operation -> AndroidAudioServiceProvider.perform(context.applicationContext, owner, operation) },
        admission = admission,
    )

    internal val audioOwner = AudioOwnerKey.ui("speech-player-${UUID.randomUUID()}")
    private val lifecycleScope = CoroutineScope(SupervisorJob() + dispatcher)
    private var playbackJob: Job? = null
    private var activeAutomaticPlayback: Job? = null
    private var ownerDrain: Job? = null
    private val speechJobs = java.util.concurrent.ConcurrentHashMap.newKeySet<Job>()
    private val generation = java.util.concurrent.atomic.AtomicLong()

    init { AndroidAudioSessionContext.reserve(audioOwner) }

    @Synchronized fun hasAudioWork(): Boolean = speechJobs.any { it.isActive } || ownerDrain?.isActive == true

    @Synchronized fun speakAsync(
        text: String,
        voice: String? = null,
        speed: Float? = null,
        foregroundUserInitiated: Boolean = false,
        flowDuplex: Boolean = false,
        automatic: Boolean = false,
        awaitStart: suspend () -> Boolean = { true },
        mayStart: () -> Boolean = { true },
        retryBusy: Boolean = false,
        onError: (DeviceAudioError) -> Unit = {},
    ) {
        val preceding = playbackJob
        val token = generation.get()
        val job = lifecycleScope.launch(start = CoroutineStart.LAZY) {
            try {
                preceding?.join()
                if (generation.get() != token) return@launch
                suspend fun play() = performAudio(
                    audioOwner,
                    DeviceAudioOperation.Speak(
                        text = text,
                        language = null,
                        rate = speed,
                        voice = voice,
                        foregroundUserInitiated = foregroundUserInitiated,
                        flowDuplex = flowDuplex,
                    ),
                )
                var busyRetryDelayMs = 250L
                while (generation.get() == token) {
                    if (!awaitStart()) return@launch
                    var deferred = false
                    suspend fun playIfReady(): DeviceAudioResult? = when {
                        generation.get() != token -> null
                        !mayStart() -> { deferred = true; null }
                        else -> {
                            val current = currentCoroutineContext()[Job]
                            if (automatic) synchronized(this@VoiceSpeechPlayer) { activeAutomaticPlayback = current }
                            try {
                                currentCoroutineContext().ensureActive()
                                play()
                            } finally {
                                if (automatic) synchronized(this@VoiceSpeechPlayer) {
                                    if (activeAutomaticPlayback === current) activeAutomaticPlayback = null
                                }
                            }
                        }
                    }
                    val result = if (admission == null) playIfReady() else admission.withLock { playIfReady() }
                    currentCoroutineContext().ensureActive()
                    if (generation.get() != token) return@launch
                    if (deferred) continue
                    if (result is DeviceAudioResult.Failed && retryBusy && result.error.kind == DeviceAudioErrorKind.Busy) {
                        // Busy means audio admission failed before playback; do not lose this reply.
                        delay(busyRetryDelayMs)
                        busyRetryDelayMs = (busyRetryDelayMs * 2).coerceAtMost(5_000L)
                        continue
                    }
                    if (result is DeviceAudioResult.Failed) onError(result.error)
                    break
                }
            } catch (cancelled: CancellationException) {
                throw cancelled
            } catch (error: Throwable) {
                currentCoroutineContext().ensureActive()
                if (generation.get() == token) {
                    onError(DeviceAudioError(DeviceAudioErrorKind.Unavailable, error.message.orEmpty()))
                }
            }
        }
        speechJobs.add(job)
        job.invokeOnCompletion { speechJobs.remove(job) }
        playbackJob = job
        job.start()
    }

    /** Explicit microphone input takes priority over another owner's automatic reply. */
    @Synchronized fun yieldAutomaticPlayback() {
        activeAutomaticPlayback?.cancel()
    }

    @Synchronized fun stop() {
        generation.incrementAndGet()
        speechJobs.forEach { it.cancel() }
        val previousDrain = ownerDrain
        val job = lifecycleScope.launch(start = CoroutineStart.LAZY) {
            previousDrain?.join()
            performAudio(audioOwner, DeviceAudioOperation.EndOwner)
        }
        ownerDrain = job
        playbackJob = job
        job.start()
    }

    @Synchronized fun dispose() {
        generation.incrementAndGet()
        speechJobs.forEach { it.cancel() }
        val previousDrain = ownerDrain
        playbackJob = null
        lifecycleScope.launch {
            try {
                previousDrain?.join()
                performAudio(audioOwner, DeviceAudioOperation.EndOwner)
            }
            finally {
                AndroidAudioSessionContext.release(audioOwner)
                lifecycleScope.coroutineContext[Job]?.cancel()
            }
        }
    }
}
