package com.lingxi.code.voice.audio

import com.lingxi.code.voice.AndroidVoiceRuntime
import com.lingxi.code.settings.VersionedAudioConfiguration
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.CoroutineStart
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.NonCancellable
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.TimeoutCancellationException
import kotlinx.coroutines.async
import kotlinx.coroutines.cancel
import kotlinx.coroutines.currentCoroutineContext
import kotlinx.coroutines.ensureActive
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.withContext
import kotlinx.coroutines.withTimeout
import kotlinx.coroutines.launch
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.ConcurrentLinkedDeque
import java.util.concurrent.atomic.AtomicBoolean
import java.util.concurrent.atomic.AtomicLong

internal data class DeviceAudioRequest(
    val identity: AudioOperationIdentity,
    val owner: AudioOwnerKey,
    val timeoutBudgetMs: Long?,
    val maxPayloadBytes: Long,
    val operation: DeviceAudioOperation,
)

internal sealed interface DeviceAudioOperation {
    data class Capture(val sampleRateHz: Int, val format: String) : DeviceAudioOperation
    data class Play(val pcm: ByteArray, val sampleRateHz: Int) : DeviceAudioOperation
    data class Transcribe(val capture: DeviceAudioCapture, val language: String?) : DeviceAudioOperation
    data class StartRecording(val sampleRateHz: Int, val format: String) : DeviceAudioOperation
    data class StopRecording(val handle: String) : DeviceAudioOperation
    data class Listen(
        val language: String?,
        val configuration: AudioConfigurationV4? = null,
        val configurationRevision: Long? = null,
    ) : DeviceAudioOperation
    data class Synthesize(
        val text: String,
        val language: String?,
        val rate: Float?,
        val voice: String?,
        val configuration: AudioConfigurationV4? = null,
        val configurationRevision: Long? = null,
    ) : DeviceAudioOperation
    data class Speak(
        val text: String,
        val language: String?,
        val rate: Float?,
        val voice: String?,
        val foregroundUserInitiated: Boolean = false,
        val flowDuplex: Boolean = false,
        val configuration: AudioConfigurationV4? = null,
        val configurationRevision: Long? = null,
    ) : DeviceAudioOperation
    data class OffloadMediaPlay(val label: String, val target: String) : DeviceAudioOperation
    data class OffloadMediaControl(val label: String, val command: OffloadMediaCommand) : DeviceAudioOperation
    data class Status(val handle: String?) : DeviceAudioOperation
    data object EndOwner : DeviceAudioOperation
}

internal enum class OffloadMediaCommand { PAUSE, RESUME, STOP, STATUS }

internal data class DeviceMediaPlaybackState(
    val playing: Boolean,
    val positionMs: Int,
    val durationMs: Int,
)

internal sealed interface DeviceAudioResult {
    data class RecordingStarted(val handle: String) : DeviceAudioResult
    data class Recording(val bytes: ByteArray, val mimeType: String) : DeviceAudioResult
    data class Transcript(val text: String, val language: String?, val confidence: Float?) : DeviceAudioResult
    data class Synthesized(val pcm: ByteArray, val sampleRateHz: Int) : DeviceAudioResult
    data class PlaybackCompleted(val durationMs: Long) : DeviceAudioResult
    data class OffloadMedia(val state: DeviceMediaPlaybackState) : DeviceAudioResult
    data class Status(val recording: Boolean, val playing: Boolean) : DeviceAudioResult
    data object OwnerEnded : DeviceAudioResult
    data class Failed(val error: DeviceAudioError) : DeviceAudioResult
}

internal enum class DeviceAudioErrorKind {
    PermissionDenied,
    Busy,
    Cancelled,
    Timeout,
    NoSpeech,
    NotRecording,
    Unavailable,
    Unsupported,
    ModelMissing,
    VoiceMissing,
    InvalidRequest,
    SynthesisFailed,
    NativeFailure,
    MediaTooLarge,
}

internal data class DeviceAudioError(val kind: DeviceAudioErrorKind, val message: String)

internal data class DeviceAudioCapture(val bytes: ByteArray, val mimeType: String)

internal class AudioOperationException(val kind: DeviceAudioErrorKind, message: String) : Exception(message)

/** Android media calls stay behind this seam so lease and cancellation rules can be tested on the JVM. */
internal interface AndroidAudioDeviceDriver {
    suspend fun streamCapture(lease: AudioLease, format: RealtimePcmFormat, onChunk: suspend (ByteArray) -> Unit): Unit =
        throw AudioOperationException(DeviceAudioErrorKind.Unsupported, "Streaming PCM capture is unsupported")
    fun pauseStreamCapture(lease: AudioLease, paused: Boolean) = Unit
    fun streamingPlaybackPositionMs(lease: AudioLease): Long? = null
    suspend fun streamPlayback(lease: AudioLease, format: RealtimePcmFormat, chunks: kotlinx.coroutines.channels.ReceiveChannel<ByteArray>): Unit =
        throw AudioOperationException(DeviceAudioErrorKind.Unsupported, "Streaming PCM playback is unsupported")
    suspend fun captureWav(lease: AudioLease, maxPayloadBytes: Int, untilSilence: Boolean, onReady: () -> Unit): DeviceAudioCapture =
        throw AudioOperationException(DeviceAudioErrorKind.Unsupported, "PCM microphone capture is unsupported")
    fun finishWavCapture(lease: AudioLease) = Unit
    suspend fun startRecording(
        lease: AudioLease,
        sampleRateHz: Int,
        format: String,
        maxPayloadBytes: Int,
        onTerminated: (DeviceAudioError) -> Unit,
        mayStart: () -> Boolean,
    ): String

    suspend fun stopRecording(lease: AudioLease, handle: String, maxPayloadBytes: Int): DeviceAudioCapture
    suspend fun play(lease: AudioLease, pcm: ByteArray, sampleRateHz: Int): Long
    suspend fun playMedia(
        lease: AudioLease,
        target: String,
        mayStart: () -> Boolean,
        onTerminal: (DeviceAudioError?) -> Unit,
    ): DeviceMediaPlaybackState
    suspend fun controlMedia(lease: AudioLease, command: OffloadMediaCommand): DeviceMediaPlaybackState
    suspend fun stop(lease: AudioLease)
}

internal enum class DeviceAudioOperationKind { RECORD, LISTEN, SYNTHESIZE, SPEAK, CAPTURE, PLAY, TRANSCRIBE }
internal enum class DeviceAudioReadiness { READY, NEEDS_PERMISSION, BUSY, MISSING_MODEL, UNAVAILABLE }

internal data class DeviceAudioCapabilities(
    val serviceEpoch: Long,
    val supportRevision: Long,
    val supported: Set<DeviceAudioOperationKind>,
    val readiness: Map<DeviceAudioOperationKind, DeviceAudioReadiness>,
    val maxPayloadBytes: Long,
)

internal data class DeviceAudioOperationDiagnostic(
    val identity: AudioOperationIdentity,
    val ownerKind: AudioOwnerKey.Kind,
    val operationKind: String,
    val phase: String,
    val configurationRevision: Long?,
    val requestedSource: String?,
    val effectiveSource: String?,
    val fallbackReason: String?,
)

internal data class DeviceAudioServiceDiagnostics(
    val serviceEpoch: Long,
    val activeLeaseCount: Int,
    val pendingOperationCount: Int,
    val recentOperations: List<DeviceAudioOperationDiagnostic>,
    val usage: List<AndroidAudioUsageRecord> = emptyList(),
)

internal interface AudioServiceSpeechRuntime {
    suspend fun transcribeEncoded(capture: DeviceAudioCapture, language: String?, configuration: AudioConfigurationV4): SttResult =
        throw AudioOperationException(DeviceAudioErrorKind.Unsupported, "Recorded-file recognition is unsupported")
    fun configuration(): AudioConfigurationV4
    fun configurationSnapshot(): VersionedAudioConfiguration
    fun microphonePermissionGranted(): Boolean
    fun resolveRecognition(
        configuration: AudioConfigurationV4,
        language: String? = null,
        systemStatusOverride: AudioReadiness? = null,
    ): AudioRouteResolution
    fun resolveSpeech(
        configuration: AudioConfigurationV4,
        language: String? = null,
        voice: String? = null,
        rate: Float? = null,
        systemStatusOverride: AudioReadiness? = null,
    ): AudioRouteResolution
    suspend fun transcribe(language: String?, configuration: AudioConfigurationV4): SttResult
    fun openRealtimeSession(
        language: String?,
        configuration: AudioConfigurationV4,
        callbacks: RealtimeSpeechCallbacks,
    ): RealtimeSpeechSession
    suspend fun render(
        text: String,
        language: String?,
        voice: String?,
        rate: Float?,
        maxPcmBytes: Int,
        configuration: AudioConfigurationV4,
    ): Pair<ByteArray, Int>
}

internal class AndroidRuntimeSpeechBridge(private val runtime: AndroidVoiceRuntime) : AudioServiceSpeechRuntime {
    override suspend fun transcribeEncoded(capture: DeviceAudioCapture, language: String?, configuration: AudioConfigurationV4): SttResult =
        runtime.transcribeEncoded(capture, language, configuration)
    override fun configuration(): AudioConfigurationV4 = runtime.configurationSnapshot().configuration
    override fun configurationSnapshot(): VersionedAudioConfiguration = runtime.configurationSnapshot()
    override fun microphonePermissionGranted(): Boolean = runtime.microphonePermissionGranted()
    override fun resolveRecognition(
        configuration: AudioConfigurationV4,
        language: String?,
        systemStatusOverride: AudioReadiness?,
    ): AudioRouteResolution = runtime.resolveRecognition(configuration, language, systemStatusOverride).route

    override fun resolveSpeech(
        configuration: AudioConfigurationV4,
        language: String?,
        voice: String?,
        rate: Float?,
        systemStatusOverride: AudioReadiness?,
    ): AudioRouteResolution = runtime.resolveSpeech(configuration, language, voice, rate, systemStatusOverride).route

    override suspend fun transcribe(language: String?, configuration: AudioConfigurationV4): SttResult =
        runtime.transcribe(language, configuration)

    override fun openRealtimeSession(
        language: String?,
        configuration: AudioConfigurationV4,
        callbacks: RealtimeSpeechCallbacks,
    ): RealtimeSpeechSession = runtime.openRealtimeSession(language, callbacks, configuration)

    override suspend fun render(
        text: String,
        language: String?,
        voice: String?,
        rate: Float?,
        maxPcmBytes: Int,
        configuration: AudioConfigurationV4,
    ): Pair<ByteArray, Int> = runtime.renderSpeech(text, language, voice, rate, maxPcmBytes, configuration)
}

/** One app-scoped operation router. All Android entry points use this object, including engine callbacks. */
internal class AndroidAudioServiceCore(
    private val runtime: AudioServiceSpeechRuntime,
    private val driver: AndroidAudioDeviceDriver,
    private val maxPayloadBytes: Long,
    private val initialEpoch: Long = 1L,
    private val provider: ProviderAudioBridge? = null,
) {
    private data class OperationState(
        val request: DeviceAudioRequest,
        val job: Job,
        val cancelled: AtomicBoolean = AtomicBoolean(false),
        val terminal: AtomicBoolean = AtomicBoolean(false),
        val releasing: AtomicBoolean = AtomicBoolean(false),
        val done: CompletableDeferred<Unit> = CompletableDeferred(),
        @Volatile var lease: AudioLease? = null,
        @Volatile var runtimeSession: RealtimeSpeechSession? = null,
        @Volatile var runtimeClosed: CompletableDeferred<Unit>? = null,
        @Volatile var configurationRevision: Long? = null,
        @Volatile var requestedSource: String? = null,
        @Volatile var effectiveSource: String? = null,
        @Volatile var fallbackReason: String? = null,
    ) {
        fun isLive(): Boolean = !cancelled.get() && !terminal.get() && job.isActive
        fun cancel() {
            cancelled.set(true)
            job.cancel(CancellationException("audio operation was cancelled"))
        }
    }

    private data class RecordingSession(
        val owner: AudioOwnerKey,
        val epoch: Long,
        val originIdentity: AudioOperationIdentity,
        val lease: AudioLease,
    ) {
        val stopMutex = Mutex()
        var finalizedCapture: DeviceAudioCapture? = null
        val started = CompletableDeferred<Unit>()
        @Volatile var failure: DeviceAudioError? = null
    }

    private data class OffloadMediaKey(val owner: AudioOwnerKey, val label: String)

    private class OffloadMediaSession(
        val key: OffloadMediaKey,
        val lease: AudioLease,
    ) {
        val started = CompletableDeferred<Unit>()
        val terminal = AtomicBoolean(false)
        val stopMutex = Mutex()
    }

    private data class RenderedSpeech(val pcm: ByteArray, val sampleRateHz: Int, val lease: AudioLease)

    private val operationMutex = Mutex()
    private val serviceOperationScope = CoroutineScope(SupervisorJob() + Dispatchers.IO)
    private val coordinator = AudioResourceCoordinator(initialEpoch)
    private val recordings = AudioRecordingRegistry()
    private val operations = ConcurrentHashMap<AudioOperationIdentity, OperationState>()
    private val cancelledOperations = ConcurrentHashMap.newKeySet<AudioOperationIdentity>()
    private val cancelledOperationOrder = ConcurrentLinkedDeque<AudioOperationIdentity>()
    private val cancellationLock = Any()
    private val seenOperations = LinkedHashSet<AudioOperationIdentity>()
    private val operationDiagnostics = ConcurrentLinkedDeque<DeviceAudioOperationDiagnostic>()
    private val providerCapabilities = ConcurrentHashMap<String, Pair<AudioConfigurationV4, ProviderAudioCapability>>()
    private val recordingSessions = ConcurrentHashMap<String, RecordingSession>()
    private val leaseOperations = ConcurrentHashMap<Long, OperationState>()
    private val playbackByOwner = ConcurrentHashMap<AudioOwnerKey, AudioLease>()
    private val offloadMedia = ConcurrentHashMap<OffloadMediaKey, OffloadMediaSession>()
    private val offloadMediaFailures = ConcurrentHashMap<OffloadMediaKey, DeviceAudioError>()
    private val offloadMediaFailureOrder = ConcurrentLinkedDeque<OffloadMediaKey>()
    private val nativeStopCompletion = ConcurrentHashMap<Long, CompletableDeferred<Unit>>()
    private val stopInProgress = ConcurrentHashMap.newKeySet<Long>()
    private val pendingMediaCompletions = AtomicLong(0L)
    /** Brief owner fence while EndOwner drains already admitted operations. */
    private val endingOwners = ConcurrentHashMap.newKeySet<AudioOwnerKey>()
    private val admission = AudioLeaseAdmission(coordinator) { lease -> stopLease(lease) }
    @Volatile private var epoch = initialEpoch
    @Volatile private var supportRevision = 1L
    @Volatile private var invalidating = false

    init {
        require(maxPayloadBytes > 0L) { "published audio payload limit must be positive" }
    }

    suspend fun execute(request: DeviceAudioRequest): DeviceAudioResult {
        if (!request.identity.isValid()) {
            return failure(DeviceAudioErrorKind.InvalidRequest, "audio operation identity is invalid")
        }
        if (request.maxPayloadBytes !in 1..maxPayloadBytes) {
            return failure(DeviceAudioErrorKind.InvalidRequest, "audio payload limit is outside the published bound")
        }
        val callerContext = currentCoroutineContext()
        val parentJob = callerContext[Job]
            ?: return failure(DeviceAudioErrorKind.InvalidRequest, "audio operation has no cancellable job")
        val operationScope = CoroutineScope(callerContext + SupervisorJob(parentJob))
        lateinit var state: OperationState
        val work = operationScope.async(start = CoroutineStart.LAZY) {
            if (request.timeoutBudgetMs != null) {
                if (request.timeoutBudgetMs <= 0L) throw AudioOperationException(DeviceAudioErrorKind.Timeout, "audio operation deadline elapsed")
                withTimeout(request.timeoutBudgetMs) { executeActive(state) }
            } else {
                executeActive(state)
            }
        }
        state = OperationState(request, work)
        val registered = operationMutex.withLock {
            when {
                invalidating || request.identity.serviceEpoch != epoch -> false
                request.operation != DeviceAudioOperation.EndOwner && request.owner in endingOwners -> false
                else -> registerOperation(state)
            }
        }
        if (!registered) {
            operationScope.cancel()
            return when {
                invalidating || request.identity.serviceEpoch != epoch -> failure(DeviceAudioErrorKind.Cancelled, "audio service instance has changed")
                request.operation != DeviceAudioOperation.EndOwner && request.owner in endingOwners -> failure(DeviceAudioErrorKind.Cancelled, "audio owner is ending")
                request.identity in cancelledOperations -> failure(DeviceAudioErrorKind.Cancelled, "audio operation was cancelled before admission")
                else -> failure(DeviceAudioErrorKind.InvalidRequest, "audio operation identity has already been used")
            }
        }
        recordDiagnostic(state, "started")
        work.start()
        return try {
            val result = work.await()
            state.terminal.set(true)
            result
        } catch (_: TimeoutCancellationException) {
            state.cancelled.set(true)
            failure(DeviceAudioErrorKind.Timeout, "audio operation timed out")
        } catch (cancelled: CancellationException) {
            state.cancelled.set(true)
            failure(DeviceAudioErrorKind.Cancelled, "audio operation was cancelled")
        } catch (error: AudioOperationException) {
            state.terminal.set(true)
            failure(error.kind, error.message ?: error.kind.name)
        } catch (error: AudioDriverException) {
            state.terminal.set(true)
            failure(error.error.kind, error.error.message)
        } catch (error: Throwable) {
            state.terminal.set(true)
            failure(DeviceAudioErrorKind.NativeFailure, error.message ?: "Android audio operation failed")
        } finally {
            withContext(NonCancellable) { runCatching { provider?.cancel(request.identity.id) } }
            if (!work.isCompleted) {
                work.cancel(CancellationException("audio operation ended"))
                withContext(NonCancellable) { work.join() }
            }
            state.terminal.set(true)
            recordDiagnostic(state, "settled")
            state.done.complete(Unit)
            operations.remove(request.identity, state)
            operationScope.cancel()
        }
    }

    suspend fun cancel(identity: AudioOperationIdentity) {
        val operation = operationMutex.withLock {
            rememberCancelled(identity)
            operations[identity]
        }
        operation?.cancel()
        if (operation != null) withContext(NonCancellable) { operation.done.await() }
        val session = recordingSessions.entries.firstOrNull { it.value.originIdentity == identity } ?: return
        val handle = session.key
        val ownerSession = session.value
        val stopped = withContext(NonCancellable) { admission.release(ownerSession.lease) }
        if (!stopped) return
        recordingSessions.remove(handle, ownerSession)
        recordings.remove(handle, ownerSession.owner, ownerSession.epoch)
        retireLeaseMetadata(ownerSession.lease)
    }

    /** Starts a UI-owned live recognizer while retaining the same Capture lease as service Listen. */
    suspend fun openRealtimeListen(
        request: DeviceAudioRequest,
        callbacks: RealtimeSpeechCallbacks,
    ): RealtimeSpeechSession {
        val operation = request.operation as? DeviceAudioOperation.Listen
            ?: throw AudioOperationException(DeviceAudioErrorKind.InvalidRequest, "realtime session requires a Listen request")
        if (!request.identity.isValid() || request.maxPayloadBytes !in 1..maxPayloadBytes) {
            throw AudioOperationException(DeviceAudioErrorKind.InvalidRequest, "audio operation identity or payload limit is invalid")
        }

        val operationScope = CoroutineScope(
            serviceOperationScope.coroutineContext + SupervisorJob(serviceOperationScope.coroutineContext[Job]),
        )
        val ready = CompletableDeferred<RealtimeSpeechSession>()
        val terminal = CompletableDeferred<Unit>()
        val terminalResultReported = AtomicBoolean(false)
        val recognizerClosed = AtomicBoolean(false)
        lateinit var state: OperationState
        val work = operationScope.async(start = CoroutineStart.LAZY) {
            try {
                state.job.ensureActive()
                if (!state.isLive() || request.identity.serviceEpoch != epoch) {
                    throw CancellationException("audio operation is no longer active")
                }
                requireMicrophonePermission()
                val currentSnapshot = runtime.configurationSnapshot()
                val snapshot = operation.configuration ?: currentSnapshot.configuration
                val route = if (snapshot.recognition.source == AudioSource.PROVIDER) {
                    providerRoute(state, snapshot, "recognition")
                } else runtime.resolveRecognition(snapshot, operation.language)
                state.configurationRevision = operation.configurationRevision ?: currentSnapshot.revision
                state.requestedSource = route.requested.source.value
                state.effectiveSource = route.effective?.source?.value
                state.fallbackReason = route.fallbackReason
                recordDiagnostic(state, "route_resolved")
                requireReadyRoute(route)
                val modelId = route.effective?.takeIf { it.source == AudioSource.OFFLINE }?.modelId
                var lease = acquire(state, AudioResource.Capture, modelId = modelId)
                state.lease = lease
                val guardedCallbacks = object : RealtimeSpeechCallbacks {
                    override fun onReady() {
                        if (state.isLive()) callbacks.onReady()
                    }

                    override fun onPartial(text: String) {
                        if (state.isLive()) callbacks.onPartial(text)
                    }

                    override fun onFinal(text: String) {
                        if (state.isLive() && terminalResultReported.compareAndSet(false, true)) {
                            callbacks.onFinal(text)
                        }
                    }

                    override fun onError(code: String, message: String, retriable: Boolean) {
                        if (state.isLive() && terminalResultReported.compareAndSet(false, true)) {
                            callbacks.onError(code, message, retriable)
                        }
                    }

                    override fun onClosed() {
                        if (!recognizerClosed.compareAndSet(false, true)) return
                        if (state.isLive() && terminalResultReported.compareAndSet(false, true)) {
                            callbacks.onError("no_speech", "Speech recognition ended without a result.", false)
                        }
                        try {
                            callbacks.onClosed()
                        } finally {
                            terminal.complete(Unit)
                        }
                    }
                }
                val nativeSession = try {
                    if (snapshot.recognition.source == AudioSource.PROVIDER) {
                        openProviderCapture(state, lease, snapshot, guardedCallbacks)
                    } else runtime.openRealtimeSession(operation.language, snapshot, guardedCallbacks)
                } catch (unavailable: AudioOperationException) {
                    if (unavailable.kind != DeviceAudioErrorKind.Unavailable ||
                        snapshot.recognition.source != AudioSource.AUTOMATIC || route.effective?.source != AudioSource.SYSTEM ||
                        !isAudioFallbackAllowed(AudioFallbackFailure.UNAVAILABLE, operationStarted = false)
                    ) {
                        throw unavailable
                    }
                    val fallback = runtime.resolveRecognition(
                        configuration = snapshot,
                        language = operation.language,
                        systemStatusOverride = AudioReadiness.UNAVAILABLE,
                    )
                    requireReadyRoute(fallback)
                    val effectiveFallback = fallback.effective
                        ?.takeIf { it.source == AudioSource.OFFLINE }
                        ?: throw routeFailure(fallback)
                    val fallbackModelId = effectiveFallback.modelId
                        ?: throw AudioOperationException(DeviceAudioErrorKind.ModelMissing, "selected offline recognition model is unavailable")
                    lease = coordinator.addModelReference(lease, fallbackModelId)
                        ?: throw AudioOperationException(DeviceAudioErrorKind.Busy, "offline recognition model is already in use")
                    state.lease = lease
                    state.effectiveSource = effectiveFallback.source.value
                    state.fallbackReason = fallback.fallbackReason ?: "systemUnavailable"
                    recordDiagnostic(state, "fallback")
                    runtime.openRealtimeSession(
                        operation.language,
                        snapshot.copy(
                            recognition = snapshot.recognition.copy(
                                source = AudioSource.OFFLINE,
                                offlineModelId = fallbackModelId,
                            ),
                        ),
                        guardedCallbacks,
                    )
                }
                state.runtimeSession = nativeSession
                if (!state.isLive() || !coordinator.isActive(lease)) {
                    nativeSession.cancel()
                    throw CancellationException("audio operation was cancelled during recognizer admission")
                }
                ready.complete(nativeSession)
                val timeout = request.timeoutBudgetMs
                if (timeout != null) {
                    if (timeout <= 0L) throw AudioOperationException(DeviceAudioErrorKind.Timeout, "audio operation deadline elapsed")
                    withTimeout(timeout) { terminal.await() }
                } else {
                    terminal.await()
                }
            } catch (error: Throwable) {
                if (!ready.isCompleted) ready.completeExceptionally(error)
                if (error is TimeoutCancellationException && state.isLive()) {
                    callbacks.onError("timeout", "Speech recognition timed out.", true)
                }
                throw error
            } finally {
                withContext(NonCancellable) { runCatching { provider?.cancel(request.identity.id) } }
                if (state.runtimeSession == null) terminal.complete(Unit)
                state.lease?.let { releaseAfterStop(it) }
            }
        }
        state = OperationState(request, work)
        state.runtimeClosed = terminal
        val registered = operationMutex.withLock {
            when {
                invalidating || request.identity.serviceEpoch != epoch -> false
                request.owner in endingOwners -> false
                else -> registerOperation(state)
            }
        }
        if (!registered) {
            operationScope.cancel()
            throw CancellationException(
                if (request.identity in cancelledOperations) "audio operation was cancelled before admission"
                else "audio owner or service instance is no longer active",
            )
        }
        recordDiagnostic(state, "started")
        work.invokeOnCompletion {
            state.terminal.set(true)
            recordDiagnostic(state, "settled")
            state.done.complete(Unit)
            operations.remove(request.identity, state)
            operationScope.cancel()
        }
        work.start()
        val native = try {
            ready.await()
        } catch (error: Throwable) {
            state.cancel()
            withContext(NonCancellable) { state.done.await() }
            throw error
        }
        return object : RealtimeSpeechSession {
            override fun stop() {
                native.stop()
            }

            override fun cancel() {
                state.cancel()
            }

            override fun close() {
                cancel()
            }
        }
    }

    fun capabilities(): DeviceAudioCapabilities {
        val config = runtime.configuration()
        val recognition = runtime.resolveRecognition(config)
        val speech = runtime.resolveSpeech(config)
        val cloudRecognition = providerCapabilities["recognition"]?.takeIf { it.first == config }?.second
        val cloudSpeech = providerCapabilities["speech"]?.takeIf { it.first == config }?.second
        val micGranted = runtime.microphonePermissionGranted()
        val activeLeases = coordinator.allLeases()
        val readiness = buildMap {
            put(DeviceAudioOperationKind.CAPTURE, when {
                !micGranted -> DeviceAudioReadiness.NEEDS_PERMISSION
                activeLeases.any { it.resource in setOf(AudioResource.Capture, AudioResource.Playback, AudioResource.SystemRender) } -> DeviceAudioReadiness.BUSY
                else -> DeviceAudioReadiness.READY
            })
            put(DeviceAudioOperationKind.PLAY, if (activeLeases.any { it.resource in setOf(AudioResource.Capture, AudioResource.Playback, AudioResource.SystemRender) }) DeviceAudioReadiness.BUSY else DeviceAudioReadiness.READY)
            put(DeviceAudioOperationKind.TRANSCRIBE, when {
                config.recognition.source == AudioSource.PROVIDER -> if (cloudRecognition?.readiness == "ready") DeviceAudioReadiness.READY else DeviceAudioReadiness.UNAVAILABLE
                config.recognition.source == AudioSource.SYSTEM -> DeviceAudioReadiness.UNAVAILABLE
                runtime.resolveRecognition(config, systemStatusOverride = AudioReadiness.UNAVAILABLE).status == AudioRouteStatus.READY -> DeviceAudioReadiness.READY
                else -> DeviceAudioReadiness.MISSING_MODEL
            })
            put(DeviceAudioOperationKind.RECORD, when {
                !micGranted -> DeviceAudioReadiness.NEEDS_PERMISSION
                activeLeases.any { it.resource in setOf(AudioResource.Capture, AudioResource.Playback, AudioResource.SystemRender) } -> DeviceAudioReadiness.BUSY
                else -> DeviceAudioReadiness.READY
            })
            put(DeviceAudioOperationKind.LISTEN, when {
                !micGranted -> DeviceAudioReadiness.NEEDS_PERMISSION
                activeLeases.any { it.resource in setOf(AudioResource.Capture, AudioResource.Playback, AudioResource.SystemRender) } -> DeviceAudioReadiness.BUSY
                config.recognition.source == AudioSource.PROVIDER -> if (cloudRecognition?.readiness == "ready") DeviceAudioReadiness.READY else DeviceAudioReadiness.UNAVAILABLE
                recognition.status == AudioRouteStatus.READY -> DeviceAudioReadiness.READY
                recognition.reason.startsWith("offlineModel") -> DeviceAudioReadiness.MISSING_MODEL
                else -> DeviceAudioReadiness.UNAVAILABLE
            })
            put(DeviceAudioOperationKind.SYNTHESIZE, when {
                activeLeases.any { it.resource == AudioResource.SystemRender } -> DeviceAudioReadiness.BUSY
                config.speech.source == AudioSource.PROVIDER -> if (cloudSpeech?.readiness == "ready") DeviceAudioReadiness.READY else DeviceAudioReadiness.UNAVAILABLE
                speech.status == AudioRouteStatus.READY -> DeviceAudioReadiness.READY
                speech.reason.startsWith("offlineModel") -> DeviceAudioReadiness.MISSING_MODEL
                else -> DeviceAudioReadiness.UNAVAILABLE
            })
            put(DeviceAudioOperationKind.SPEAK, when {
                activeLeases.any { it.resource in setOf(AudioResource.Capture, AudioResource.Playback, AudioResource.SystemRender) } -> DeviceAudioReadiness.BUSY
                config.speech.source == AudioSource.PROVIDER -> if (cloudSpeech?.readiness == "ready") DeviceAudioReadiness.READY else DeviceAudioReadiness.UNAVAILABLE
                speech.status == AudioRouteStatus.READY -> DeviceAudioReadiness.READY
                speech.reason.startsWith("offlineModel") -> DeviceAudioReadiness.MISSING_MODEL
                else -> DeviceAudioReadiness.UNAVAILABLE
            })
        }
        return DeviceAudioCapabilities(
            serviceEpoch = epoch,
            supportRevision = supportRevision,
            supported = DeviceAudioOperationKind.entries.filterNot {
                (config.recognition.source == AudioSource.SYSTEM && it == DeviceAudioOperationKind.TRANSCRIBE) ||
                (config.recognition.source == AudioSource.PROVIDER && cloudRecognition?.supported == false && it in listOf(DeviceAudioOperationKind.LISTEN, DeviceAudioOperationKind.TRANSCRIBE)) ||
                    (config.speech.source == AudioSource.PROVIDER && cloudSpeech?.supported == false && it in listOf(DeviceAudioOperationKind.SYNTHESIZE, DeviceAudioOperationKind.SPEAK))
            }.toSet(),
            readiness = readiness,
            maxPayloadBytes = maxPayloadBytes,
        )
    }

    fun configurationSnapshot(): VersionedAudioConfiguration = runtime.configurationSnapshot()

    suspend fun openRealtimeAgent(
        request: DeviceAudioRequest, configuration: AudioConfigurationV4, callbacks: RealtimeAgentCallbacks,
    ): RealtimeSpeechSession {
        if (!request.identity.isValid() || request.maxPayloadBytes !in 1..maxPayloadBytes) {
            throw AudioOperationException(DeviceAudioErrorKind.InvalidRequest, "Realtime audio identity or payload limit is invalid.")
        }
        if (configuration.conversation.interaction != "turn_based") {
            throw AudioOperationException(DeviceAudioErrorKind.Unsupported, "This device has no verified acoustic echo cancellation. Choose turn based realtime conversation.")
        }
        val scope = CoroutineScope(serviceOperationScope.coroutineContext + SupervisorJob(serviceOperationScope.coroutineContext[Job]))
        val ready = CompletableDeferred<RealtimeSpeechSession>()
        lateinit var state: OperationState
        val work = scope.async(start = CoroutineStart.LAZY) {
            try {
                requireMicrophonePermission()
                val capability = provider?.capabilities(request, configuration, "realtime")
                    ?: throw AudioOperationException(DeviceAudioErrorKind.Unavailable, "Realtime Agent audio is unavailable.")
                requireLive(state)
                if (!capability.supported) throw AudioOperationException(DeviceAudioErrorKind.Unsupported, capability.reason ?: "The current Agent provider does not support realtime conversation.")
                if (capability.readiness != "ready") throw AudioOperationException(DeviceAudioErrorKind.Unavailable, capability.reason ?: "Configure the selected realtime audio profile in provider settings.")
                state.requestedSource = AudioSource.PROVIDER.value
                state.effectiveSource = AudioSource.PROVIDER.value
                state.configurationRevision = runtime.configurationSnapshot().revision
                val capture = acquire(state, AudioResource.Capture, foregroundUserInitiated = true, flowDuplex = true)
                val operationScope = CoroutineScope(currentCoroutineContext())
                val guarded = object : RealtimeAgentCallbacks {
                    override fun onReady() { if (state.isLive()) callbacks.onReady() }
                    override fun onTranscript(text: String, assistant: Boolean, final: Boolean) { if (state.isLive()) callbacks.onTranscript(text, assistant, final) }
                    override fun onPlayback(playing: Boolean) { if (state.isLive()) callbacks.onPlayback(playing) }
                    override fun onError(message: String) { if (state.isLive()) callbacks.onError(message) }
                    override fun onClosed() = callbacks.onClosed()
                }
                val lane = AndroidRealtimeAgentLane(operationScope, checkNotNull(provider), driver, request, configuration, capture,
                    acquirePlayback = {
                        val decision = admission.acquire(request.identity.copy(id = java.util.UUID.randomUUID().toString()), request.owner,
                            AudioResource.Playback, flowDuplex = true)
                        val lease = (decision as? AudioLeaseDecision.Granted)?.lease
                            ?: throw AudioOperationException(DeviceAudioErrorKind.Busy, "Realtime playback is busy.")
                        leaseOperations[lease.leaseId] = state
                        playbackByOwner[request.owner] = lease
                        lease
                    }, releasePlayback = { lease ->
                        playbackByOwner.remove(request.owner, lease)
                        releaseAfterStop(lease)
                    }, isLive = { state.isLive() && coordinator.isActive(capture) }, callbacks = guarded)
                lane.run {
                    ready.complete(object : RealtimeSpeechSession {
                        override fun stop() = lane.commitInput()
                        override fun cancel() = state.cancel()
                        override fun close() = cancel()
                    })
                }
            } catch (error: Throwable) {
                if (!ready.isCompleted) ready.completeExceptionally(error)
                if (error !is CancellationException && state.isLive()) callbacks.onError(error.message ?: "Realtime conversation failed.")
                throw error
            } finally {
                withContext(NonCancellable) {
                    runCatching { provider?.cancel(request.identity.id) }
                    state.lease?.let { releaseAfterStop(it) }
                }
                callbacks.onClosed()
            }
        }
        state = OperationState(request, work)
        val registered = operationMutex.withLock {
            !invalidating && request.identity.serviceEpoch == epoch && request.owner !in endingOwners && registerOperation(state)
        }
        if (!registered) { scope.cancel(); throw CancellationException("Realtime audio owner or identity is stale") }
        recordDiagnostic(state, "realtime_started")
        work.invokeOnCompletion {
            state.terminal.set(true); recordDiagnostic(state, "settled")
            state.done.complete(Unit); operations.remove(request.identity, state); scope.cancel()
        }
        work.start()
        return try { ready.await() } catch (error: Throwable) {
            state.cancel(); withContext(NonCancellable) { state.done.await() }; throw error
        }
    }

    @Synchronized fun rememberProviderCapability(configuration: AudioConfigurationV4, kind: String, capability: ProviderAudioCapability) {
        val previous = providerCapabilities.put(kind, configuration to capability)
        if (previous != (configuration to capability)) supportRevision += 1
    }

    fun diagnostics(): DeviceAudioServiceDiagnostics = DeviceAudioServiceDiagnostics(
            serviceEpoch = epoch,
            activeLeaseCount = coordinator.allLeases().size,
            pendingOperationCount = operations.size + pendingMediaCompletions.get().coerceAtMost(Int.MAX_VALUE.toLong()).toInt(),
        recentOperations = operationDiagnostics.toList(),
        usage = AndroidAudioUsageJournal.snapshot(),
    )

    suspend fun invalidate(): Long = operationMutex.withLock {
        invalidating = true
        val nextEpoch = epoch + 1L
        val pending = operations.values.toList()
        try {
            pending.forEach { state ->
                rememberCancelled(state.request.identity)
                state.cancel()
            }
            val ended = withContext(NonCancellable) { admission.invalidate(nextEpoch) }
                ?: throw AudioOperationException(DeviceAudioErrorKind.NativeFailure, "audio resources could not be stopped")
            ended.forEach(::retireLeaseMetadata)
            recordingSessions.clear()
            recordings.invalidate(nextEpoch)
            playbackByOwner.clear()
            offloadMedia.values.toList().forEach { session ->
                session.terminal.set(true)
                session.started.complete(Unit)
                offloadMedia.remove(session.key, session)
            }
            synchronized(offloadMediaFailureOrder) {
                offloadMediaFailures.clear()
                offloadMediaFailureOrder.clear()
            }
            leaseOperations.clear()
            nativeStopCompletion.clear()
            epoch = nextEpoch
            endingOwners.clear()
            supportRevision += 1L
            epoch
        } finally {
            try {
                withContext(NonCancellable) { pending.forEach { it.job.join() } }
            } finally {
                invalidating = false
            }
        }
    }

    private suspend fun executeActive(state: OperationState): DeviceAudioResult {
        state.job.ensureActive()
        if (!state.isLive()) throw CancellationException("audio operation is no longer active")
        if (state.request.identity.serviceEpoch != epoch) {
            throw AudioOperationException(DeviceAudioErrorKind.Cancelled, "audio service instance has changed")
        }
        return when (val operation = state.request.operation) {
            is DeviceAudioOperation.Capture -> capture(state, operation)
            is DeviceAudioOperation.Play -> playPcm(state, operation)
            is DeviceAudioOperation.Transcribe -> transcribeEncoded(state, operation)
            is DeviceAudioOperation.StartRecording -> startRecording(state, operation)
            is DeviceAudioOperation.StopRecording -> stopRecording(state, operation)
            is DeviceAudioOperation.Listen -> listen(state, operation)
            is DeviceAudioOperation.Synthesize -> synthesize(state, operation)
            is DeviceAudioOperation.Speak -> speak(state, operation)
            is DeviceAudioOperation.OffloadMediaPlay -> offloadMediaPlay(state, operation)
            is DeviceAudioOperation.OffloadMediaControl -> offloadMediaControl(state, operation)
            is DeviceAudioOperation.Status -> status(state, operation)
            DeviceAudioOperation.EndOwner -> endOwner(state)
        }
    }

    private suspend fun capture(state: OperationState, operation: DeviceAudioOperation.Capture): DeviceAudioResult {
        if (operation.sampleRateHz != 16_000 || operation.format !in listOf("wav", "audio/wav")) {
            throw AudioOperationException(DeviceAudioErrorKind.Unsupported, "Bounded Android capture supports mono PCM16 WAV at 16000 Hz.")
        }
        requireMicrophonePermission()
        val lease = acquire(state, AudioResource.Capture)
        return try {
            val recording = driver.captureWav(lease, payloadLimitInt(state.request.maxPayloadBytes), untilSilence = true) {}
            requireLive(state)
            DeviceAudioResult.Recording(recording.bytes, recording.mimeType)
        } finally { releaseAfterStop(lease) }
    }

    private suspend fun playPcm(state: OperationState, operation: DeviceAudioOperation.Play): DeviceAudioResult {
        if (operation.pcm.isEmpty() || operation.pcm.size % 2 != 0 || operation.sampleRateHz !in 1..MAX_AUDIO_SAMPLE_RATE_HZ) {
            throw AudioOperationException(DeviceAudioErrorKind.InvalidRequest, "Playback requires valid mono PCM16 and a supported sample rate.")
        }
        validatePcm(operation.pcm, operation.sampleRateHz, payloadLimitInt(state.request.maxPayloadBytes))
        val lease = acquire(state, AudioResource.Playback)
        playbackByOwner[state.request.owner] = lease
        return try {
            val duration = driver.play(lease, operation.pcm, operation.sampleRateHz)
            requireLive(state)
            DeviceAudioResult.PlaybackCompleted(duration)
        } finally { playbackByOwner.remove(state.request.owner, lease); releaseAfterStop(lease) }
    }

    private suspend fun transcribeEncoded(state: OperationState, operation: DeviceAudioOperation.Transcribe): DeviceAudioResult {
        if (operation.capture.bytes.isEmpty() || operation.capture.bytes.size > state.request.maxPayloadBytes) {
            throw AudioOperationException(DeviceAudioErrorKind.MediaTooLarge, "Recorded audio is empty or exceeds the payload limit.")
        }
        val snapshot = runtime.configurationSnapshot()
        state.configurationRevision = snapshot.revision
        val configuration = snapshot.configuration
        val result = if (configuration.recognition.source == AudioSource.PROVIDER) {
            providerRoute(state, configuration, "recognition")
            checkNotNull(provider).transcribe(state.request, configuration, operation.capture)
        } else {
            if (configuration.recognition.source == AudioSource.SYSTEM) throw AudioOperationException(DeviceAudioErrorKind.Unsupported,
                "Android SpeechRecognizer only accepts live microphone audio. Choose an installed offline model or provider for recorded audio.")
            val route = runtime.resolveRecognition(configuration, operation.language, AudioReadiness.UNAVAILABLE)
            requireReadyRoute(route)
            val lease = acquire(state, AudioResource.OfflineRender, modelId = route.effective?.modelId)
            try { runtime.transcribeEncoded(operation.capture, operation.language, configuration) } finally { releaseAfterStop(lease) }
        }
        requireLive(state)
        return when (result) {
            is SttResult.Ok -> {
                if (result.text.isBlank()) throw AudioOperationException(DeviceAudioErrorKind.NoSpeech, "No speech was recognized.")
                DeviceAudioResult.Transcript(result.text, result.language, result.confidence)
            }
            is SttResult.Err -> throw result.toAudioOperationException()
        }
    }

    private suspend fun startRecording(
        state: OperationState,
        operation: DeviceAudioOperation.StartRecording,
    ): DeviceAudioResult {
        validateCaptureRequest(operation.sampleRateHz, operation.format)
        requireMicrophonePermission()
        recordingSessions.entries.filter { it.value.owner == state.request.owner && it.value.failure != null }
            .forEach { retireTerminatedRecording(it.key, it.value) }
        val lease = acquire(state, AudioResource.Capture)
        val session = RecordingSession(state.request.owner, epoch, state.request.identity, lease)
        try {
            state.job.ensureActive()
            requireLive(state)
            val limit = payloadLimitInt(state.request.maxPayloadBytes)
            val handle = driver.startRecording(lease, operation.sampleRateHz, "audio/m4a", limit, onTerminated = { error ->
                session.failure = error
                serviceOperationScope.launch {
                    session.started.await()
                    session.stopMutex.withLock {
                        // Keep a failed lease addressable if native cleanup needs retrying.
                        runCatching { releaseAfterStop(lease) }
                    }
                }
            }) { state.isLive() && coordinator.isActive(lease) }
            session.failure?.let { throw AudioOperationException(it.kind, it.message) }
            requireLive(state)
            if (requestEpochIsCurrent(state) && recordings.lookup(handle, state.request.owner, epoch) is RecordingLookup.NotFound) {
                recordings.register(handle, state.request.owner, epoch)
                recordingSessions[handle] = session
                // A recording deliberately outlives the completed start operation.
                state.lease = null
                leaseOperations.remove(lease.leaseId, state)
                return DeviceAudioResult.RecordingStarted(handle)
            }
            throw AudioOperationException(DeviceAudioErrorKind.Cancelled, "recording start became stale")
        } catch (error: Throwable) {
            runCatching { driver.stop(lease) }
            releaseAfterStop(lease)
            throw error
        } finally {
            session.started.complete(Unit)
        }
    }

    private suspend fun retireTerminatedRecording(handle: String, session: RecordingSession) {
        session.started.await()
        session.stopMutex.withLock {
            if (session.failure == null) return@withLock
            releaseAfterStop(session.lease)
            recordingSessions.remove(handle, session)
            recordings.remove(handle, session.owner, session.epoch)
        }
    }

    private suspend fun stopRecording(
        state: OperationState,
        operation: DeviceAudioOperation.StopRecording,
    ): DeviceAudioResult {
        val lookup = recordings.lookup(operation.handle, state.request.owner, epoch)
        if (lookup !is RecordingLookup.Found) {
            val message = if (lookup == RecordingLookup.WrongOwner) "recording belongs to a different owner" else "recording handle is unavailable"
            throw AudioOperationException(DeviceAudioErrorKind.NotRecording, message)
        }
        val session = recordingSessions[operation.handle]
            ?: throw AudioOperationException(DeviceAudioErrorKind.NotRecording, "recording handle is no longer active")
        session.failure?.let { failure ->
            retireTerminatedRecording(operation.handle, session)
            throw AudioOperationException(failure.kind, failure.message)
        }
        if (session.owner != state.request.owner || session.epoch != epoch || !coordinator.isActive(session.lease)) {
            throw AudioOperationException(DeviceAudioErrorKind.NotRecording, "recording handle is stale")
        }
        val limit = payloadLimitInt(state.request.maxPayloadBytes)
        return session.stopMutex.withLock {
            if (recordingSessions[operation.handle] !== session || !coordinator.isActive(session.lease)) {
                throw AudioOperationException(DeviceAudioErrorKind.NotRecording, "recording handle is no longer active")
            }
            val capture = try {
                session.finalizedCapture ?: driver.stopRecording(session.lease, operation.handle, limit).also {
                    session.finalizedCapture = it
                }
            } finally {
                try {
                    releaseAfterStop(session.lease)
                } finally {
                    if (!coordinator.isActive(session.lease)) {
                        recordingSessions.remove(operation.handle, session)
                        recordings.remove(operation.handle, state.request.owner, session.epoch)
                    }
                }
            }
            if (capture.bytes.isEmpty()) throw AudioOperationException(DeviceAudioErrorKind.NativeFailure, "recording produced no media")
            if (capture.bytes.size > limit) throw AudioOperationException(DeviceAudioErrorKind.MediaTooLarge, "recording exceeds the payload limit")
            DeviceAudioResult.Recording(capture.bytes, capture.mimeType)
        }
    }

    private suspend fun listen(
        state: OperationState,
        operation: DeviceAudioOperation.Listen,
    ): DeviceAudioResult {
        requireMicrophonePermission()
        val currentSnapshot = runtime.configurationSnapshot()
        val snapshot = operation.configuration ?: currentSnapshot.configuration
        state.configurationRevision = operation.configurationRevision ?: currentSnapshot.revision
        val resolution = if (snapshot.recognition.source == AudioSource.PROVIDER) {
            providerRoute(state, snapshot, "recognition")
        } else runtime.resolveRecognition(snapshot, operation.language)
        state.requestedSource = resolution.requested.source.value
        state.effectiveSource = resolution.effective?.source?.value
        state.fallbackReason = resolution.fallbackReason
        recordDiagnostic(state, "route_resolved")
        requireReadyRoute(resolution)
        val modelId = resolution.effective
            ?.takeIf { it.source == AudioSource.OFFLINE }
            ?.modelId
        var lease = acquire(state, AudioResource.Capture, modelId = modelId)
        state.lease = lease
        return try {
            var result = if (snapshot.recognition.source == AudioSource.PROVIDER) {
                val capture = driver.captureWav(lease, payloadLimitInt(state.request.maxPayloadBytes), untilSilence = true) {}
                requireLive(state)
                checkNotNull(provider).transcribe(state.request, snapshot, capture)
            } else runtime.transcribe(operation.language, snapshot)
            if (
                result is SttResult.Err && result.code == "no_provider_configured" &&
                snapshot.recognition.source == AudioSource.AUTOMATIC &&
                resolution.effective?.source == AudioSource.SYSTEM &&
                isAudioFallbackAllowed(AudioFallbackFailure.UNAVAILABLE, operationStarted = false)
            ) {
                val fallback = runtime.resolveRecognition(
                    configuration = snapshot,
                    language = operation.language,
                    systemStatusOverride = AudioReadiness.UNAVAILABLE,
                )
                requireReadyRoute(fallback)
                val effectiveFallback = fallback.effective
                    ?.takeIf { it.source == AudioSource.OFFLINE }
                    ?: throw routeFailure(fallback)
                val fallbackModelId = effectiveFallback.modelId
                    ?: throw AudioOperationException(DeviceAudioErrorKind.ModelMissing, "selected offline recognition model is unavailable")
                lease = coordinator.addModelReference(lease, fallbackModelId)
                    ?: throw AudioOperationException(DeviceAudioErrorKind.Busy, "offline recognition model is already in use")
                state.lease = lease
                state.effectiveSource = effectiveFallback.source.value
                state.fallbackReason = fallback.fallbackReason ?: "systemUnavailable"
                recordDiagnostic(state, "fallback")
                val fallbackConfiguration = snapshot.copy(
                    recognition = snapshot.recognition.copy(
                        source = AudioSource.OFFLINE,
                        offlineModelId = fallbackModelId,
                    ),
                )
                result = runtime.transcribe(operation.language, fallbackConfiguration)
            }
            requireLive(state)
            when (result) {
                is SttResult.Ok -> {
                    if (result.text.isBlank()) throw AudioOperationException(DeviceAudioErrorKind.NoSpeech, "No speech was recognized.")
                    DeviceAudioResult.Transcript(result.text, result.language, result.confidence)
                }
                is SttResult.Err -> throw result.toAudioOperationException()
            }
        } finally {
            state.lease?.let { releaseAfterStop(it) }
        }
    }

    private suspend fun synthesize(
        state: OperationState,
        operation: DeviceAudioOperation.Synthesize,
    ): DeviceAudioResult {
        validateText(operation.text)
        val currentSnapshot = runtime.configurationSnapshot()
        val snapshot = operation.configuration ?: currentSnapshot.configuration
        state.configurationRevision = operation.configurationRevision ?: currentSnapshot.revision
        val resolution = if (snapshot.speech.source == AudioSource.PROVIDER) {
            providerRoute(state, snapshot, "speech")
        } else runtime.resolveSpeech(snapshot, operation.language, operation.voice, operation.rate)
        state.requestedSource = resolution.requested.source.value
        state.effectiveSource = resolution.effective?.source?.value
        state.fallbackReason = resolution.fallbackReason
        recordDiagnostic(state, "route_resolved")
        requireReadyRoute(resolution)
        val resource = renderResource(resolution)
        val lease = acquire(state, resource, modelId = resolution.effective?.modelId)
        state.lease = lease
        try {
            if (snapshot.speech.source == AudioSource.PROVIDER) {
                val rendered = checkNotNull(provider).synthesize(state.request, snapshot, operation.text)
                requireLive(state)
                validatePcm(rendered.first, rendered.second, payloadLimitInt(state.request.maxPayloadBytes))
                return DeviceAudioResult.Synthesized(rendered.first, rendered.second)
            }
            val rendered = renderWithAutomaticFallback(
                state = state,
                text = operation.text,
                language = operation.language,
                voice = operation.voice,
                rate = operation.rate,
                configuration = snapshot,
                initialRoute = resolution,
                initialLease = lease,
                playbackLease = false,
            )
            requireLive(state)
            validatePcm(rendered.pcm, rendered.sampleRateHz, payloadLimitInt(state.request.maxPayloadBytes))
            return DeviceAudioResult.Synthesized(rendered.pcm, rendered.sampleRateHz)
        } catch (error: Throwable) {
            throw mapSpeechFailure(error)
        } finally {
            state.lease?.let { releaseAfterStop(it) }
        }
    }

    private suspend fun speak(
        state: OperationState,
        operation: DeviceAudioOperation.Speak,
    ): DeviceAudioResult {
        validateText(operation.text)
        val currentSnapshot = runtime.configurationSnapshot()
        val snapshot = operation.configuration ?: currentSnapshot.configuration
        state.configurationRevision = operation.configurationRevision ?: currentSnapshot.revision
        val resolution = if (snapshot.speech.source == AudioSource.PROVIDER) {
            providerRoute(state, snapshot, "speech")
        } else runtime.resolveSpeech(snapshot, operation.language, operation.voice, operation.rate)
        state.requestedSource = resolution.requested.source.value
        state.effectiveSource = resolution.effective?.source?.value
        state.fallbackReason = resolution.fallbackReason
        recordDiagnostic(state, "route_resolved")
        requireReadyRoute(resolution)
        val lease = acquire(
            state,
            AudioResource.Playback,
            modelId = resolution.effective?.modelId,
            foregroundUserInitiated = operation.foregroundUserInitiated,
            flowDuplex = operation.flowDuplex,
        )
        state.lease = lease
        playbackByOwner[state.request.owner] = lease
        try {
            if (snapshot.speech.source == AudioSource.PROVIDER) {
                val rendered = checkNotNull(provider).synthesize(state.request, snapshot, operation.text)
                requireLive(state)
                validatePcm(rendered.first, rendered.second, payloadLimitInt(state.request.maxPayloadBytes))
                val duration = driver.play(lease, rendered.first, rendered.second)
                requireLive(state)
                return DeviceAudioResult.PlaybackCompleted(duration)
            }
            val rendered = renderWithAutomaticFallback(
                state = state,
                text = operation.text,
                language = operation.language,
                voice = operation.voice,
                rate = operation.rate,
                configuration = snapshot,
                initialRoute = resolution,
                initialLease = lease,
                playbackLease = true,
            )
            requireLive(state)
            validatePcm(rendered.pcm, rendered.sampleRateHz, payloadLimitInt(state.request.maxPayloadBytes))
            val duration = driver.play(rendered.lease, rendered.pcm, rendered.sampleRateHz)
            requireLive(state)
            return DeviceAudioResult.PlaybackCompleted(duration)
        } catch (error: Throwable) {
            throw mapSpeechFailure(error)
        } finally {
            state.lease?.let { activeLease ->
                playbackByOwner.remove(state.request.owner, activeLease)
                releaseAfterStop(activeLease)
            }
        }
    }

    private suspend fun offloadMediaPlay(
        state: OperationState,
        operation: DeviceAudioOperation.OffloadMediaPlay,
    ): DeviceAudioResult {
        requireSessionOwner(state.request.owner)
        requireMediaArguments(operation.label, operation.target)
        val key = OffloadMediaKey(state.request.owner, operation.label)
        offloadMedia[key]?.let { previous -> stopOffloadMedia(previous) }
        clearOffloadMediaFailure(key)
        requireLive(state)
        val lease = acquire(state, AudioResource.Playback)
        val session = OffloadMediaSession(key, lease)
        if (offloadMedia.putIfAbsent(key, session) != null) {
            releaseAfterStop(lease)
            throw AudioOperationException(DeviceAudioErrorKind.Busy, "media session is already active for this owner")
        }
        try {
            val snapshot = driver.playMedia(
                lease = lease,
                target = operation.target,
                mayStart = { state.isLive() && coordinator.isActive(lease) },
                onTerminal = { error -> dispatchOffloadMediaCompletion(session, error) },
            )
            requireLive(state)
            if (!coordinator.isActive(lease) || offloadMedia[key] !== session) {
                throw CancellationException("media playback was stopped before admission completed")
            }
            session.started.complete(Unit)
            // The media session owns this Playback lease after the short command returns.
            state.lease = null
            leaseOperations.remove(lease.leaseId, state)
            return DeviceAudioResult.OffloadMedia(snapshot)
        } catch (error: Throwable) {
            session.started.complete(Unit)
            offloadMedia.remove(key, session)
            session.terminal.set(true)
            runCatching { driver.stop(lease) }
            releaseAfterStop(lease)
            throw error
        }
    }

    private suspend fun offloadMediaControl(
        state: OperationState,
        operation: DeviceAudioOperation.OffloadMediaControl,
    ): DeviceAudioResult {
        requireSessionOwner(state.request.owner)
        requireMediaArguments(operation.label, target = null)
        val key = OffloadMediaKey(state.request.owner, operation.label)
        val session = offloadMedia[key]
        if (session == null) {
            if (operation.command != OffloadMediaCommand.STOP) {
                offloadMediaFailures[key]?.let { throw AudioOperationException(it.kind, it.message) }
            }
            clearOffloadMediaFailure(key)
            return if (operation.command == OffloadMediaCommand.STOP) {
                DeviceAudioResult.OffloadMedia(DeviceMediaPlaybackState(false, 0, 0))
            } else {
                throw AudioOperationException(DeviceAudioErrorKind.NotRecording, "media session is unavailable for this owner")
            }
        }
        if (session.lease.identity.serviceEpoch != epoch || !coordinator.isActive(session.lease)) {
            offloadMedia.remove(key, session)
            throw AudioOperationException(DeviceAudioErrorKind.NotRecording, "media session is stale")
        }
        return when (operation.command) {
            OffloadMediaCommand.STOP -> {
                stopOffloadMedia(session)
                DeviceAudioResult.OffloadMedia(DeviceMediaPlaybackState(false, 0, 0))
            }
            OffloadMediaCommand.PAUSE, OffloadMediaCommand.RESUME, OffloadMediaCommand.STATUS ->
                DeviceAudioResult.OffloadMedia(driver.controlMedia(session.lease, operation.command))
        }
    }

    private suspend fun stopOffloadMedia(session: OffloadMediaSession) {
        session.started.await()
        releaseOffloadMedia(session)
    }

    private suspend fun completeOffloadMedia(session: OffloadMediaSession, error: DeviceAudioError?) {
        session.started.await()
        releaseOffloadMedia(session, error)
    }

    private suspend fun releaseOffloadMedia(session: OffloadMediaSession, error: DeviceAudioError? = null) {
        session.stopMutex.withLock {
            if (session.terminal.get() || offloadMedia[session.key] !== session) return@withLock
            // Keep the session addressable if the native stop fails, so STOP or
            // EndOwner can retry the same lease.
            if (error != null) rememberOffloadMediaFailure(session.key, error)
            releaseAfterStop(session.lease)
            session.terminal.set(true)
            offloadMedia.remove(session.key, session)
        }
    }

    private fun dispatchOffloadMediaCompletion(session: OffloadMediaSession, error: DeviceAudioError?) {
        pendingMediaCompletions.incrementAndGet()
        serviceOperationScope.launch {
            try {
                completeOffloadMedia(session, error)
            } finally {
                pendingMediaCompletions.decrementAndGet()
            }
        }
    }

    private fun clearOffloadMediaFailure(key: OffloadMediaKey) {
        synchronized(offloadMediaFailureOrder) {
            offloadMediaFailures.remove(key)
            offloadMediaFailureOrder.remove(key)
        }
    }

    private fun rememberOffloadMediaFailure(key: OffloadMediaKey, error: DeviceAudioError) {
        synchronized(offloadMediaFailureOrder) {
            offloadMediaFailureOrder.remove(key)
            offloadMediaFailures[key] = error
            offloadMediaFailureOrder.addLast(key)
            while (offloadMediaFailureOrder.size > 64) {
                offloadMediaFailureOrder.pollFirst()?.let(offloadMediaFailures::remove)
            }
        }
    }

    private fun requireSessionOwner(owner: AudioOwnerKey) {
        if (owner.kind != AudioOwnerKey.Kind.Session || owner.id.isBlank()) {
            throw AudioOperationException(DeviceAudioErrorKind.InvalidRequest, "native media playback requires a session owner")
        }
    }

    private fun requireMediaArguments(label: String, target: String?) {
        if (label.isBlank() || label.length > MAX_MEDIA_LABEL_LENGTH ||
            target != null && (target.isBlank() || target.length > MAX_MEDIA_TARGET_LENGTH)
        ) {
            throw AudioOperationException(DeviceAudioErrorKind.InvalidRequest, "native media request is invalid")
        }
    }

    private suspend fun renderWithAutomaticFallback(
        state: OperationState,
        text: String,
        language: String?,
        voice: String?,
        rate: Float?,
        configuration: AudioConfigurationV4,
        initialRoute: AudioRouteResolution,
        initialLease: AudioLease,
        playbackLease: Boolean,
    ): RenderedSpeech {
        val maxPcmBytes = payloadLimitInt(state.request.maxPayloadBytes)
        try {
            val (pcm, sampleRateHz) = runtime.render(text, language, voice, rate, maxPcmBytes, configuration)
            requireLive(state)
            return RenderedSpeech(pcm, sampleRateHz, initialLease)
        } catch (unavailable: AudioOperationException) {
            val automaticWithoutFixedVoice =
                initialRoute.requested.source == AudioSource.AUTOMATIC && initialRoute.requested.voice == null
            if (unavailable.kind != DeviceAudioErrorKind.Unavailable ||
                initialRoute.effective?.source != AudioSource.SYSTEM || !automaticWithoutFixedVoice ||
                !isAudioFallbackAllowed(AudioFallbackFailure.UNAVAILABLE, operationStarted = false)
            ) {
                throw unavailable
            }

            val fallback = runtime.resolveSpeech(
                configuration = configuration,
                language = language,
                voice = voice,
                rate = rate,
                systemStatusOverride = AudioReadiness.UNAVAILABLE,
            )
            requireReadyRoute(fallback)
            val effectiveFallback = fallback.effective ?: throw routeFailure(fallback)
            if (effectiveFallback.source != AudioSource.OFFLINE) throw routeFailure(fallback)
            state.requestedSource = fallback.requested.source.value
            state.effectiveSource = effectiveFallback.source.value
            state.fallbackReason = fallback.fallbackReason ?: "systemUnavailable"
            recordDiagnostic(state, "fallback")
            val modelId = effectiveFallback.modelId
                ?: throw AudioOperationException(DeviceAudioErrorKind.ModelMissing, "selected offline speech model is unavailable")
            val fallbackConfiguration = configuration.copy(
                speech = configuration.speech.copy(
                    source = AudioSource.OFFLINE,
                    offlineModelId = modelId,
                    voice = null,
                ),
            )

            val fallbackLease = if (playbackLease) {
                coordinator.addModelReference(initialLease, modelId)
                    ?: throw AudioOperationException(DeviceAudioErrorKind.Busy, "offline speech model is already rendering")
            } else {
                releaseAfterStop(initialLease)
                acquire(state, AudioResource.OfflineRender, modelId = modelId)
            }
            state.lease = fallbackLease
            if (playbackLease) playbackByOwner[state.request.owner] = fallbackLease
            val (pcm, sampleRateHz) = runtime.render(
                text = text,
                language = language,
                voice = voice,
                rate = rate,
                maxPcmBytes = maxPcmBytes,
                configuration = fallbackConfiguration,
            )
            requireLive(state)
            return RenderedSpeech(pcm, sampleRateHz, fallbackLease)
        }
    }

    private suspend fun status(state: OperationState, operation: DeviceAudioOperation.Status): DeviceAudioResult {
        recordingSessions.entries.filter {
            it.value.owner == state.request.owner && it.value.epoch == epoch && it.value.failure != null
                && (operation.handle == null || operation.handle == it.key)
        }.forEach { retireTerminatedRecording(it.key, it.value) }
        val recording = operation.handle?.let { handle ->
            when (val lookup = recordings.lookup(handle, state.request.owner, epoch)) {
                is RecordingLookup.Found -> true
                RecordingLookup.NotFound -> false
                RecordingLookup.WrongOwner -> throw AudioOperationException(DeviceAudioErrorKind.NotRecording, "recording belongs to a different owner")
                RecordingLookup.StaleEpoch -> throw AudioOperationException(DeviceAudioErrorKind.NotRecording, "recording handle is stale")
            }
        } ?: recordingSessions.values.any { it.owner == state.request.owner && it.epoch == epoch }
        val mediaSession = offloadMedia.values.firstOrNull {
            it.key.owner == state.request.owner && coordinator.isActive(it.lease)
        }
        val mediaPlaying = mediaSession?.let {
            driver.controlMedia(it.lease, OffloadMediaCommand.STATUS).playing
        } ?: false
        val playing = playbackByOwner[state.request.owner]?.let(coordinator::isActive) == true || mediaPlaying
        return DeviceAudioResult.Status(recording, playing)
    }

    private suspend fun endOwner(state: OperationState): DeviceAudioResult {
        val owner = state.request.owner
        val pending = operationMutex.withLock {
            endingOwners.add(owner)
            operations.values.filter { it !== state && it.request.owner == owner }.also { owned ->
                owned.forEach { pendingState ->
                    rememberCancelled(pendingState.request.identity)
                    pendingState.cancel()
                }
            }
        }
        try {
            val ended = admission.endOwner(owner)
                ?: throw AudioOperationException(DeviceAudioErrorKind.NativeFailure, "audio resources could not be stopped")
            ended.forEach(::retireLeaseMetadata)
            withContext(NonCancellable) { pending.forEach { it.job.join() } }
            recordings.endOwner(owner, epoch).forEach { handle -> recordingSessions.remove(handle) }
            playbackByOwner.remove(owner)
            offloadMedia.values.filter { it.key.owner == owner }.forEach { session ->
                session.terminal.set(true)
                session.started.complete(Unit)
                offloadMedia.remove(session.key, session)
            }
            synchronized(offloadMediaFailureOrder) {
                offloadMediaFailureOrder.filter { it.owner == owner }.forEach { key ->
                    offloadMediaFailures.remove(key)
                    offloadMediaFailureOrder.remove(key)
                }
            }
            return DeviceAudioResult.OwnerEnded
        } finally {
            // The fence is concurrent; reacquiring operationMutex here could block invalidation
            // while it waits for this cancelled EndOwner operation to settle.
            endingOwners.remove(owner)
        }
    }

    private suspend fun acquire(
        state: OperationState,
        resource: AudioResource,
        modelId: String? = null,
        foregroundUserInitiated: Boolean = false,
        flowDuplex: Boolean = false,
    ): AudioLease {
        val previousLeases = coordinator.allLeases()
        val decision = try {
            admission.acquire(
                identity = state.request.identity,
                owner = state.request.owner,
                resource = resource,
                modelId = modelId,
                foregroundUserInitiated = foregroundUserInitiated,
                flowDuplex = flowDuplex,
            )
        } finally {
            val activeLeaseIds = coordinator.allLeases().mapTo(mutableSetOf()) { it.leaseId }
            previousLeases.filter { it.leaseId !in activeLeaseIds }.forEach(::retireLeaseMetadata)
        }
        val lease = when (decision) {
            is AudioLeaseDecision.Granted -> decision.lease
            is AudioLeaseDecision.NativeFailure -> throw AudioOperationException(DeviceAudioErrorKind.NativeFailure, decision.message)
            AudioLeaseDecision.Busy -> throw AudioOperationException(DeviceAudioErrorKind.Busy, "another audio operation is active")
            AudioLeaseDecision.StaleEpoch -> throw AudioOperationException(DeviceAudioErrorKind.Cancelled, "audio service instance has changed")
            is AudioLeaseDecision.PreemptionRequired -> error("admission must settle preemption before returning")
        }
        state.lease = lease
        leaseOperations[lease.leaseId] = state
        if (!state.isLive()) {
            runCatching { driver.stop(lease) }
            releaseAfterStop(lease)
            throw CancellationException("audio operation was cancelled during admission")
        }
        return lease
    }

    private suspend fun stopLease(lease: AudioLease) {
        val firstStop = CompletableDeferred<Unit>()
        val existingStop = nativeStopCompletion.putIfAbsent(lease.leaseId, firstStop)
        val finished = existingStop ?: firstStop
        if (existingStop == null) {
            stopInProgress.add(lease.leaseId)
            try {
                driver.stop(lease)
                val operation = leaseOperations[lease.leaseId]
                val cancelOperation = operation != null && !operation.terminal.get() && !operation.releasing.get()
                if (cancelOperation) {
                    rememberCancelled(operation!!.request.identity)
                    operation.cancel()
                }
                operation?.runtimeSession?.cancel()
                operation?.runtimeClosed?.let { withContext(NonCancellable) { it.await() } }
                if (cancelOperation) operation!!.job.join()
                finished.complete(Unit)
            } catch (error: Throwable) {
                finished.completeExceptionally(error)
                nativeStopCompletion.remove(lease.leaseId, finished)
                throw error
            } finally {
                stopInProgress.remove(lease.leaseId)
            }
        }
        finished.await()
    }

    private suspend fun releaseAfterStop(lease: AudioLease) {
        if (lease.leaseId in stopInProgress && leaseOperations[lease.leaseId]?.cancelled?.get() == true) {
            return
        }
        leaseOperations[lease.leaseId]?.releasing?.set(true)
        val released = withContext(NonCancellable) { admission.release(lease) }
        if (released) {
            retireLeaseMetadata(lease)
        } else if (coordinator.allLeases().any { it.leaseId == lease.leaseId }) {
            throw AudioOperationException(DeviceAudioErrorKind.NativeFailure, "Android audio could not be stopped")
        } else {
            retireLeaseMetadata(lease)
        }
    }

    private fun retireLeaseMetadata(lease: AudioLease) {
        leaseOperations.remove(lease.leaseId)
        nativeStopCompletion.remove(lease.leaseId)
        playbackByOwner[lease.owner]?.takeIf { it.leaseId == lease.leaseId }?.let {
            playbackByOwner.remove(lease.owner, it)
        }
        offloadMedia.values.filter { it.lease.leaseId == lease.leaseId }.forEach { session ->
            session.terminal.set(true)
            session.started.complete(Unit)
            offloadMedia.remove(session.key, session)
        }
    }

    private suspend fun requireMicrophonePermission() {
        if (!runtime.microphonePermissionGranted()) {
            throw AudioOperationException(DeviceAudioErrorKind.PermissionDenied, "Microphone permission is not granted.")
        }
    }

    private fun requireLive(state: OperationState) {
        state.job.ensureActive()
        if (!state.isLive() || !requestEpochIsCurrent(state)) {
            throw CancellationException("audio operation is no longer active")
        }
    }

    private fun requestEpochIsCurrent(state: OperationState): Boolean =
        state.request.identity.serviceEpoch == epoch && state.lease?.let(coordinator::isActive) != false

    private fun renderResource(route: AudioRouteResolution): AudioResource = when (route.effective?.source) {
        AudioSource.SYSTEM -> AudioResource.SystemRender
        AudioSource.OFFLINE -> AudioResource.OfflineRender
        AudioSource.PROVIDER -> AudioResource.ProviderRender
        else -> throw routeFailure(route)
    }

    private suspend fun providerRoute(state: OperationState, configuration: AudioConfigurationV4, kind: String): AudioRouteResolution {
        val bridge = provider ?: throw AudioOperationException(DeviceAudioErrorKind.Unavailable,
            "Provider audio is unavailable. Open provider settings to configure an audio-capable profile.")
        val capability = bridge.capabilities(state.request, configuration, kind)
        rememberProviderCapability(configuration, kind, capability)
        requireLive(state)
        if (!capability.supported) throw AudioOperationException(DeviceAudioErrorKind.Unsupported,
            capability.reason ?: "The selected session provider does not support $kind audio. Choose an explicit audio provider in settings.")
        if (capability.readiness != "ready") throw AudioOperationException(DeviceAudioErrorKind.Unavailable,
            capability.reason ?: "Provider audio needs configuration. Open provider settings and check the selected profile and credential.")
        val source = if (kind == "recognition") configuration.recognition.source else configuration.speech.source
        return AudioRouteResolution(RequestedAudioRoute(source, null, configuration.speech.voice.takeIf { kind == "speech" }),
            EffectiveAudioRoute(AudioSource.PROVIDER, capability.modelId, configuration.speech.voice?.id.takeIf { kind == "speech" },
                capability.profileId, capability.providerId), AudioRouteStatus.READY, "ready")
    }

    private fun openProviderCapture(
        state: OperationState, lease: AudioLease, configuration: AudioConfigurationV4, callbacks: RealtimeSpeechCallbacks,
    ): RealtimeSpeechSession {
        val captureJob = serviceOperationScope.launch {
            try {
                val capture = driver.captureWav(lease, payloadLimitInt(state.request.maxPayloadBytes), untilSilence = false) {
                    if (state.isLive()) callbacks.onReady()
                }
                requireLive(state)
                when (val result = checkNotNull(provider).transcribe(state.request, configuration, capture)) {
                    is SttResult.Ok -> if (result.text.isBlank()) callbacks.onError("no_speech", "No speech was recognized.", false)
                        else callbacks.onFinal(result.text)
                    is SttResult.Err -> callbacks.onError(result.code, result.message, result.retriable)
                }
            } catch (cancelled: CancellationException) {
                throw cancelled
            } catch (error: Throwable) {
                if (state.isLive()) callbacks.onError("provider_audio_failed", error.message ?: "Provider transcription failed.", true)
            } finally {
                callbacks.onClosed()
            }
        }
        return object : RealtimeSpeechSession {
            override fun stop() = driver.finishWavCapture(lease)
            override fun cancel() { driver.finishWavCapture(lease); captureJob.cancel() }
            override fun close() = cancel()
        }
    }

    private fun requireReadyRoute(route: AudioRouteResolution) {
        if (route.status != AudioRouteStatus.READY) throw routeFailure(route)
    }

    private fun validateCaptureRequest(sampleRateHz: Int, format: String) {
        if (sampleRateHz !in MIN_RECORDING_SAMPLE_RATE_HZ..MAX_RECORDING_SAMPLE_RATE_HZ) {
            throw AudioOperationException(DeviceAudioErrorKind.Unsupported, "requested recording sample rate is unsupported")
        }
        if (!format.equals("m4a", ignoreCase = true) && !format.equals("audio/m4a", ignoreCase = true)) {
            throw AudioOperationException(DeviceAudioErrorKind.Unsupported, "only AAC audio/m4a recordings are supported")
        }
    }

    private fun validateText(text: String) {
        if (text.isBlank()) throw AudioOperationException(DeviceAudioErrorKind.InvalidRequest, "speech text must not be blank")
    }

    private fun validatePcm(pcm: ByteArray, sampleRateHz: Int, max: Int) {
        if (pcm.isEmpty()) throw AudioOperationException(DeviceAudioErrorKind.SynthesisFailed, "synthesis returned no audio")
        if (pcm.size > max) throw AudioOperationException(DeviceAudioErrorKind.MediaTooLarge, "synthesized audio exceeds the payload limit")
        if (pcm.size % 2 != 0 || sampleRateHz !in 1..MAX_AUDIO_SAMPLE_RATE_HZ) {
            throw AudioOperationException(DeviceAudioErrorKind.SynthesisFailed, "synthesis returned invalid PCM16 audio")
        }
    }

    private fun payloadLimitInt(max: Long): Int {
        if (max > Int.MAX_VALUE) throw AudioOperationException(DeviceAudioErrorKind.InvalidRequest, "payload limit exceeds Android array bounds")
        return max.toInt()
    }

    private fun AudioOperationIdentity.isValid(): Boolean =
        id.isNotBlank() && generation in 0L..MAX_SAFE_INTEGER && serviceEpoch in 0L..MAX_SAFE_INTEGER

    private fun failure(kind: DeviceAudioErrorKind, message: String) = DeviceAudioResult.Failed(DeviceAudioError(kind, message))

    private fun recordDiagnostic(state: OperationState, phase: String) {
        operationDiagnostics.addLast(
            DeviceAudioOperationDiagnostic(
                identity = state.request.identity,
                ownerKind = state.request.owner.kind,
                operationKind = operationName(state.request.operation),
                phase = phase,
                configurationRevision = state.configurationRevision,
                requestedSource = state.requestedSource,
                effectiveSource = state.effectiveSource,
                fallbackReason = state.fallbackReason,
            ),
        )
        while (operationDiagnostics.size > DIAGNOSTIC_HISTORY_LIMIT) operationDiagnostics.pollFirst()
    }

    private fun registerOperation(state: OperationState): Boolean = synchronized(cancellationLock) {
        val identity = state.request.identity
        if (identity in cancelledOperations || identity in seenOperations
            || operations.putIfAbsent(identity, state) != null
        ) return@synchronized false
        seenOperations.add(identity)
        while (seenOperations.size > SEEN_IDENTITY_HISTORY_LIMIT) {
            val oldest = seenOperations.iterator()
            oldest.next()
            oldest.remove()
        }
        true
    }

    private fun rememberCancelled(identity: AudioOperationIdentity) = synchronized(cancellationLock) {
        if (!cancelledOperations.add(identity)) return@synchronized
        cancelledOperationOrder.addLast(identity)
        while (cancelledOperationOrder.size > CANCELLED_IDENTITY_HISTORY_LIMIT) {
            cancelledOperationOrder.pollFirst()?.let(cancelledOperations::remove)
        }
    }

    private fun operationName(operation: DeviceAudioOperation): String = when (operation) {
        is DeviceAudioOperation.Capture -> "capture"
        is DeviceAudioOperation.Play -> "play"
        is DeviceAudioOperation.Transcribe -> "transcribe"
        is DeviceAudioOperation.StartRecording -> "record"
        is DeviceAudioOperation.StopRecording -> "stop_recording"
        is DeviceAudioOperation.Listen -> "listen"
        is DeviceAudioOperation.Synthesize -> "synthesize"
        is DeviceAudioOperation.Speak -> "speak"
        is DeviceAudioOperation.OffloadMediaPlay -> "offload_media_play"
        is DeviceAudioOperation.OffloadMediaControl -> "offload_media_control"
        is DeviceAudioOperation.Status -> "status"
        DeviceAudioOperation.EndOwner -> "end_owner"
    }

    private fun mapSpeechFailure(error: Throwable): Throwable {
        if (error is CancellationException) return error
        if (error is AudioOperationException) return error
        if (error is AudioDriverException) return AudioOperationException(error.error.kind, error.error.message)
        return AudioOperationException(DeviceAudioErrorKind.NativeFailure, error.message ?: "Android speech operation failed")
    }

    private fun routeFailure(route: AudioRouteResolution): AudioOperationException = when {
        route.status == AudioRouteStatus.INVALID_REQUEST -> AudioOperationException(DeviceAudioErrorKind.InvalidRequest, "audio route is invalid (${route.reason})")
        route.requested.voice != null -> AudioOperationException(DeviceAudioErrorKind.VoiceMissing, "selected voice is unavailable")
        route.requested.source == AudioSource.OFFLINE || route.reason == "noCompatibleOfflineModel" -> AudioOperationException(DeviceAudioErrorKind.ModelMissing, "selected offline model is unavailable")
        else -> AudioOperationException(DeviceAudioErrorKind.Unavailable, "selected speech provider is unavailable")
    }

    private fun SttResult.Err.toAudioOperationException(): AudioOperationException = when (code) {
        "permission_denied" -> AudioOperationException(DeviceAudioErrorKind.PermissionDenied, message)
        "no_speech" -> AudioOperationException(DeviceAudioErrorKind.NoSpeech, message)
        "audio_io_unavailable" -> AudioOperationException(DeviceAudioErrorKind.Unavailable, message)
        "model_missing" -> AudioOperationException(DeviceAudioErrorKind.ModelMissing, message)
        "invalid_request" -> AudioOperationException(DeviceAudioErrorKind.InvalidRequest, message)
        "no_provider_configured" -> AudioOperationException(DeviceAudioErrorKind.Unavailable, message)
        else -> AudioOperationException(DeviceAudioErrorKind.NativeFailure, message)
    }

    private companion object {
        const val MIN_RECORDING_SAMPLE_RATE_HZ = 8_000
        const val MAX_RECORDING_SAMPLE_RATE_HZ = 48_000
        const val MAX_AUDIO_SAMPLE_RATE_HZ = 768_000
        const val MAX_SAFE_INTEGER = 9_007_199_254_740_991L
        const val DIAGNOSTIC_HISTORY_LIMIT = 64
        const val SEEN_IDENTITY_HISTORY_LIMIT = 4_096
        const val CANCELLED_IDENTITY_HISTORY_LIMIT = 2_048
        const val MAX_MEDIA_LABEL_LENGTH = 256
        const val MAX_MEDIA_TARGET_LENGTH = 4_096
    }
}
