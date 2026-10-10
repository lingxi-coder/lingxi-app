package com.lingxi.code.voice

import android.Manifest
import android.content.Context
import android.content.pm.PackageManager
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.SideEffect
import androidx.compose.runtime.remember
import androidx.core.content.ContextCompat
import com.lingxi.code.settings.VersionedAudioConfiguration
import com.lingxi.code.voice.audio.AndroidAudioServiceProvider
import com.lingxi.code.voice.audio.AudioDriverException
import com.lingxi.code.voice.audio.AudioOperationException
import com.lingxi.code.voice.audio.AudioOwnerKey
import com.lingxi.code.voice.audio.DeviceAudioError
import com.lingxi.code.voice.audio.DeviceAudioErrorKind
import com.lingxi.code.voice.audio.DeviceAudioOperation
import com.lingxi.code.voice.audio.DeviceAudioResult
import com.lingxi.code.voice.audio.RealtimeSpeechCallbacks
import com.lingxi.code.voice.audio.RealtimeSpeechSession
import com.lingxi.code.voice.audio.RealtimeAgentCallbacks
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.CoroutineDispatcher
import kotlinx.coroutines.CoroutineStart
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.launch
import java.util.concurrent.atomic.AtomicBoolean
import java.util.concurrent.atomic.AtomicLong
import java.util.concurrent.atomic.AtomicReference

enum class OrbPhase { Idle, Listening, Thinking, Speaking }

private sealed interface FlowListenTerminal {
    data class Transcript(val text: String) : FlowListenTerminal
    data class Error(val error: DeviceAudioError) : FlowListenTerminal
    data object NoSpeech : FlowListenTerminal
}

internal interface FlowVoiceAudioService {
    suspend fun openRealtimeAgent(owner: AudioOwnerKey, configuration: com.lingxi.code.voice.audio.AudioConfigurationV4,
        callbacks: RealtimeAgentCallbacks): RealtimeSpeechSession =
        throw AudioOperationException(DeviceAudioErrorKind.Unsupported, "Realtime Agent conversation is unavailable")
    fun configurationSnapshot(): VersionedAudioConfiguration
    suspend fun perform(owner: AudioOwnerKey, operation: DeviceAudioOperation): DeviceAudioResult
    suspend fun openRealtimeListen(
        owner: AudioOwnerKey,
        operation: DeviceAudioOperation.Listen,
        callbacks: RealtimeSpeechCallbacks,
    ): RealtimeSpeechSession
}

private class AndroidFlowVoiceAudioService(private val context: Context) : FlowVoiceAudioService {
    override suspend fun openRealtimeAgent(owner: AudioOwnerKey, configuration: com.lingxi.code.voice.audio.AudioConfigurationV4,
        callbacks: RealtimeAgentCallbacks): RealtimeSpeechSession = AndroidAudioServiceProvider.openRealtimeAgent(context, owner, configuration, callbacks)
    override fun configurationSnapshot(): VersionedAudioConfiguration =
        AndroidAudioServiceProvider.get(context).configurationSnapshot()

    override suspend fun perform(owner: AudioOwnerKey, operation: DeviceAudioOperation): DeviceAudioResult =
        AndroidAudioServiceProvider.perform(context, owner, operation)

    override suspend fun openRealtimeListen(
        owner: AudioOwnerKey,
        operation: DeviceAudioOperation.Listen,
        callbacks: RealtimeSpeechCallbacks,
    ): RealtimeSpeechSession = AndroidAudioServiceProvider.openRealtimeListen(
        context = context,
        owner = owner,
        language = operation.language,
        callbacks = callbacks,
        configuration = operation.configuration,
        configurationRevision = operation.configurationRevision,
    )
}

internal data class FlowVoiceState(
    val phase: OrbPhase = OrbPhase.Idle,
    val userCaption: String = "",
    val assistantCaption: String = "",
    val didSend: Boolean = false,
    val isFinalizing: Boolean = false,
    val error: DeviceAudioError? = null,
    val configurationRevision: Long? = null,
)

/** Owns the Flow listen/reply lifecycle and pinned configuration outside Compose. */
internal class FlowVoiceController(
    private val audioService: FlowVoiceAudioService,
    private val hasMicrophonePermission: () -> Boolean,
    private val requestMicrophonePermission: () -> Unit,
    dispatcher: CoroutineDispatcher = Dispatchers.Main.immediate,
) {
    private val owner = AudioOwnerKey.ui("flow-mode-${java.util.UUID.randomUUID()}")
    private val scope = CoroutineScope(SupervisorJob() + dispatcher)
    private val generation = AtomicLong(0L)
    private val mutableState = MutableStateFlow(FlowVoiceState())
    val state: StateFlow<FlowVoiceState> = mutableState.asStateFlow()

    private var operation: Job? = null
    private var listenSession: RealtimeSpeechSession? = null
    private var listenSessionGeneration: Long? = null
    private var stopRequestedGeneration: Long? = null
    private var pinnedConfiguration: VersionedAudioConfiguration? = null
    private var sessionBinding: Pair<String, String>? = null
    private var pendingPermissionListen: PendingPermissionListen? = null

    private data class PendingPermissionListen(
        val generation: Long,
        val onSend: (String) -> Unit,
        val onCancel: () -> Unit,
    )

    fun listen(onSend: (String) -> Unit, onCancel: () -> Unit) {
        val token = generation.incrementAndGet()
        cancelListenSession()
        operation?.cancel()
        operation = null
        stopRequestedGeneration = null
        pendingPermissionListen = null
        if (!hasMicrophonePermission()) {
            pendingPermissionListen = PendingPermissionListen(token, onSend, onCancel)
            requestMicrophonePermission()
            mutableState.value = FlowVoiceState(
                error = DeviceAudioError(DeviceAudioErrorKind.PermissionDenied, "Microphone permission is required to listen."),
            )
            return
        }

        mutableState.value = FlowVoiceState(phase = OrbPhase.Listening)
        onCancel()
        val listenJob = scope.launch(start = CoroutineStart.LAZY) {
            try {
                // Drain prior playback on this stable owner before admitting another listen.
                audioService.perform(owner, DeviceAudioOperation.EndOwner)
                if (!isCurrent(token)) return@launch
                val snapshot = audioService.configurationSnapshot()
                pinnedConfiguration = snapshot
                mutableState.value = FlowVoiceState(
                    phase = OrbPhase.Listening,
                    configurationRevision = snapshot.revision,
                )

                if (snapshot.configuration.conversation.mode == "realtime") {
                    val realtimeClosed = CompletableDeferred<Unit>()
                    val opened = audioService.openRealtimeAgent(owner, snapshot.configuration, object : RealtimeAgentCallbacks {
                        override fun onReady() { if (isCurrent(token)) mutableState.value = mutableState.value.copy(phase = OrbPhase.Listening) }
                        override fun onTranscript(text: String, assistant: Boolean, final: Boolean) {
                            if (!isCurrent(token)) return
                            mutableState.value = if (assistant) mutableState.value.copy(assistantCaption = text,
                                phase = OrbPhase.Speaking, isFinalizing = false)
                            else mutableState.value.copy(userCaption = text, phase = if (final) OrbPhase.Thinking else OrbPhase.Listening,
                                didSend = final, isFinalizing = false)
                        }
                        override fun onPlayback(playing: Boolean) { if (isCurrent(token)) mutableState.value = mutableState.value.copy(
                            phase = if (playing) OrbPhase.Speaking else OrbPhase.Listening, isFinalizing = false) }
                        override fun onError(message: String) { if (isCurrent(token)) mutableState.value = mutableState.value.copy(
                            phase = OrbPhase.Idle, error = DeviceAudioError(DeviceAudioErrorKind.Unavailable, message)) }
                        override fun onClosed() {
                            if (isCurrent(token)) mutableState.value = mutableState.value.copy(phase = OrbPhase.Idle, isFinalizing = false)
                            realtimeClosed.complete(Unit)
                        }
                    })
                    if (!isCurrent(token)) { opened.cancel(); return@launch }
                    listenSession = opened; listenSessionGeneration = token
                    if (stopRequestedGeneration == token) opened.stop()
                    realtimeClosed.await()
                    return@launch
                }

                val closed = CompletableDeferred<Unit>()
                val closeCallbackHandled = AtomicBoolean(false)
                val terminal = AtomicReference<FlowListenTerminal?>(null)
                val callbacks = object : RealtimeSpeechCallbacks {
                    override fun onFinal(text: String) {
                        if (!isCurrent(token)) return
                        terminal.compareAndSet(null, FlowListenTerminal.Transcript(text.trim()))
                    }

                    override fun onError(code: String, message: String, retriable: Boolean) {
                        val error = realtimeListenError(code, message)
                        if (!isCurrent(token)) return
                        terminal.compareAndSet(null, FlowListenTerminal.Error(error))
                    }

                    override fun onClosed() {
                        if (!closeCallbackHandled.compareAndSet(false, true)) return
                        if (!isCurrent(token)) {
                            closed.complete(Unit)
                            return
                        }
                        terminal.compareAndSet(null, FlowListenTerminal.NoSpeech)
                        val result = terminal.get()
                        scope.launch {
                            try {
                                if (isCurrent(token)) {
                                    when (result) {
                                        is FlowListenTerminal.Error -> {
                                            mutableState.value = FlowVoiceState(
                                                error = result.error,
                                                configurationRevision = snapshot.revision,
                                            )
                                        }
                                        is FlowListenTerminal.NoSpeech -> {
                                            mutableState.value = FlowVoiceState(
                                                error = DeviceAudioError(
                                                    DeviceAudioErrorKind.NoSpeech,
                                                    "Speech recognition ended without a result.",
                                                ),
                                                configurationRevision = snapshot.revision,
                                            )
                                        }
                                        is FlowListenTerminal.Transcript -> {
                                            val text = result.text
                                            if (text.isBlank()) {
                                                mutableState.value = FlowVoiceState(configurationRevision = snapshot.revision)
                                            } else {
                                                mutableState.value = FlowVoiceState(
                                                    phase = OrbPhase.Thinking,
                                                    userCaption = text,
                                                    didSend = true,
                                                    configurationRevision = snapshot.revision,
                                                )
                                                onSend(text)
                                            }
                                        }
                                        null -> Unit
                                    }
                                }
                            } finally {
                                closed.complete(Unit)
                            }
                        }
                    }
                }

                val openedSession = audioService.openRealtimeListen(
                    owner,
                    DeviceAudioOperation.Listen(
                        language = null,
                        configuration = snapshot.configuration,
                        configurationRevision = snapshot.revision,
                    ),
                    callbacks,
                )
                if (!isCurrent(token)) {
                    openedSession.cancel()
                    return@launch
                }
                if (!closed.isCompleted) {
                    listenSession = openedSession
                    listenSessionGeneration = token
                    if (stopRequestedGeneration == token) openedSession.stop()
                    closed.await()
                }
            } catch (cancelled: CancellationException) {
                throw cancelled
            } catch (error: Throwable) {
                if (isCurrent(token)) {
                    mutableState.value = FlowVoiceState(
                        error = error.toDeviceAudioError(),
                        configurationRevision = pinnedConfiguration?.revision,
                    )
                }
            } finally {
                if (listenSessionGeneration == token) {
                    listenSession = null
                    listenSessionGeneration = null
                }
            }
        }
        operation = listenJob
        listenJob.start()
    }

    fun onMicrophonePermissionResult(granted: Boolean) {
        val pending = pendingPermissionListen ?: return
        pendingPermissionListen = null
        if (!isCurrent(pending.generation)) return
        if (!granted || !hasMicrophonePermission()) {
            mutableState.value = FlowVoiceState(
                error = DeviceAudioError(DeviceAudioErrorKind.PermissionDenied, "Microphone permission is required to listen."),
            )
            return
        }
        listen(pending.onSend, pending.onCancel)
    }

    fun updateReply(text: String, streaming: Boolean) {
        if (pinnedConfiguration?.configuration?.conversation?.mode == "realtime") return
        val current = mutableState.value
        if (current.phase != OrbPhase.Thinking && current.phase != OrbPhase.Speaking) return
        if (streaming) {
            if (text.isNotEmpty() && current.phase == OrbPhase.Thinking) {
                mutableState.value = current.copy(phase = OrbPhase.Speaking)
            }
            return
        }

        val pinned = pinnedConfiguration
        val token = generation.get()
        operation?.cancel()
        if (text.isBlank() || pinned == null) {
            mutableState.value = current.copy(phase = OrbPhase.Idle)
            return
        }
        mutableState.value = current.copy(phase = OrbPhase.Speaking)
        operation = scope.launch {
            when (val result = audioService.perform(
                owner,
                DeviceAudioOperation.Speak(
                    text = text,
                    language = null,
                    rate = null,
                    voice = null,
                    configuration = pinned.configuration,
                    configurationRevision = pinned.revision,
                ),
            )) {
                is DeviceAudioResult.Failed -> if (isCurrent(token)) {
                    mutableState.value = mutableState.value.copy(phase = OrbPhase.Idle, error = result.error)
                }
                else -> if (isCurrent(token)) {
                    mutableState.value = mutableState.value.copy(phase = OrbPhase.Idle)
                }
            }
        }
    }

    fun stopListening() {
        val current = mutableState.value
        if (current.phase != OrbPhase.Listening || current.isFinalizing) return
        mutableState.value = current.copy(isFinalizing = true)
        val token = generation.get()
        stopRequestedGeneration = token
        if (listenSessionGeneration == token) listenSession?.stop()
    }

    fun updateSessionBinding(modelId: String, profileId: String) {
        val previous = sessionBinding
        sessionBinding = modelId to profileId
        if (previous != null && previous != sessionBinding) pause()
    }

    /** Backgrounding ends device work. The overlay stays visible and waits for an explicit listen. */
    fun pause() {
        generation.incrementAndGet()
        cancelListenSession()
        operation?.cancel()
        operation = null
        stopRequestedGeneration = null
        pendingPermissionListen = null
        pinnedConfiguration = null
        mutableState.value = FlowVoiceState()
    }

    fun close() {
        pause()
        scope.launch {
            runCatching { audioService.perform(owner, DeviceAudioOperation.EndOwner) }
        }
    }

    fun dispose() {
        pause()
        scope.launch {
            runCatching { audioService.perform(owner, DeviceAudioOperation.EndOwner) }
            scope.coroutineContext[kotlinx.coroutines.Job]?.cancel()
        }
    }

    private fun isCurrent(token: Long): Boolean = generation.get() == token

    private fun cancelListenSession() {
        listenSession?.cancel()
        listenSession = null
        listenSessionGeneration = null
    }
}

private fun realtimeListenError(code: String, message: String): DeviceAudioError {
    val kind = when (code) {
        "permission_denied" -> DeviceAudioErrorKind.PermissionDenied
        "busy" -> DeviceAudioErrorKind.Busy
        "cancelled" -> DeviceAudioErrorKind.Cancelled
        "timeout" -> DeviceAudioErrorKind.Timeout
        "no_speech" -> DeviceAudioErrorKind.NoSpeech
        "unavailable", "audio_io_unavailable" -> DeviceAudioErrorKind.Unavailable
        "unsupported" -> DeviceAudioErrorKind.Unsupported
        "model_missing" -> DeviceAudioErrorKind.ModelMissing
        "voice_missing" -> DeviceAudioErrorKind.VoiceMissing
        "invalid_request" -> DeviceAudioErrorKind.InvalidRequest
        else -> DeviceAudioErrorKind.NativeFailure
    }
    return DeviceAudioError(kind, message)
}

private fun Throwable.toDeviceAudioError(): DeviceAudioError = when (this) {
    is AudioOperationException -> DeviceAudioError(kind, message ?: "Audio operation failed.")
    is AudioDriverException -> error
    else -> DeviceAudioError(DeviceAudioErrorKind.NativeFailure, message ?: "Speech recognition failed.")
}

@Composable
internal fun rememberFlowVoiceController(): FlowVoiceController {
    val context = androidx.compose.ui.platform.LocalContext.current
    val controllerRef = remember { AtomicReference<FlowVoiceController?>(null) }
    val permissionLauncher = rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) { granted ->
        controllerRef.get()?.onMicrophonePermissionResult(granted)
    }
    val controller = remember(context, permissionLauncher) {
        FlowVoiceController(
            audioService = AndroidFlowVoiceAudioService(context.applicationContext),
            hasMicrophonePermission = {
                ContextCompat.checkSelfPermission(context, Manifest.permission.RECORD_AUDIO) == PackageManager.PERMISSION_GRANTED
            },
            requestMicrophonePermission = {
                permissionLauncher.launch(Manifest.permission.RECORD_AUDIO)
            },
        )
    }
    SideEffect { controllerRef.set(controller) }
    DisposableEffect(controller) {
        onDispose { controller.dispose() }
    }
    return controller
}
