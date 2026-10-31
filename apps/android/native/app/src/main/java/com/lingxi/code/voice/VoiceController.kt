package com.lingxi.code.voice

import android.Manifest
import android.content.Context
import android.content.pm.PackageManager
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.rememberUpdatedState
import androidx.core.content.ContextCompat
import com.lingxi.code.R
import com.lingxi.code.voice.audio.AndroidAudioServiceProvider
import com.lingxi.code.voice.audio.AudioOwnerKey
import com.lingxi.code.voice.audio.DeviceAudioErrorKind
import com.lingxi.code.voice.audio.DeviceAudioOperation
import com.lingxi.code.voice.audio.DeviceAudioResult
import com.lingxi.code.voice.audio.AudioOperationException
import com.lingxi.code.voice.audio.AudioDriverException
import com.lingxi.code.voice.audio.DeviceAudioError
import com.lingxi.code.voice.audio.RealtimeSpeechCallbacks
import com.lingxi.code.voice.audio.RealtimeSpeechSession
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Job
import kotlinx.coroutines.launch
import java.util.UUID
import java.util.concurrent.atomic.AtomicBoolean

/**
 * Resolves a localized string for voice-package code that runs OUTSIDE a
 * `@Composable` body — [VoiceCapture] — and therefore cannot call
 * `stringResource()`. Mirrors `ConversationStrings`
 * (conversation/ConversationSource.kt): the fallback keeps `VoiceCaptureLifecycleTest`
 * — which constructs [VoiceCapture] directly with a non-functional
 * `ContextWrapper(null)` — passing unmodified (calling `context.getString`
 * there would NPE, since that test's [android.content.Context] has no base to
 * delegate to). The production resolver ([voiceStrings]) resolves the REAL
 * localized text through [Context.getString].
 */
fun interface VoiceStrings {
    fun resolve(id: Int, fallback: String, vararg args: Any): String
}

/** Test/no-Context fallback: the literal zh-Hans copy, `String.format`-ed. */
val DefaultVoiceStrings = VoiceStrings { _, fallback, args ->
    if (args.isEmpty()) fallback else String.format(java.util.Locale.getDefault(), fallback, *args)
}

/** Production resolver: real localized text via the app's (locale-wrapped) [Context]. */
fun voiceStrings(context: Context): VoiceStrings {
    val appContext = context.applicationContext
    return VoiceStrings { id, _, args -> com.lingxi.code.theme.LocaleWrapper.getString(appContext, id, *args) }
}

/** Result of one hold-to-talk capture. */
sealed interface VoiceCaptureResult {
    data class Transcript(val text: String) : VoiceCaptureResult
    data object PermissionDenied : VoiceCaptureResult
    data class Failed(val message: String) : VoiceCaptureResult
    data object Empty : VoiceCaptureResult
}

enum class VoiceCapturePhase {
    Idle,
    PermissionRequired,
    Starting,
    Listening,
    Stopping,
    Completed,
    Cancelled,
    Failed,
}

data class VoiceCaptureUiState(
    val phase: VoiceCapturePhase = VoiceCapturePhase.Idle,
    val partialTranscript: String = "",
    val finalTranscript: String = "",
    // Blank means "resting, nothing to say yet" — [VoiceFlowOverlay] resolves
    // the localized "hold to talk" hint at render time (`R.string.voice_hold_to_talk_hint`)
    // rather than baking a fixed-locale literal into this process-global default,
    // which is constructed once at class-load with no Context available.
    val statusText: String = "",
    val errorMessage: String? = null,
)

object VoiceCaptureStore {
    private val _state = MutableStateFlow(VoiceCaptureUiState())
    val state: StateFlow<VoiceCaptureUiState> = _state.asStateFlow()

    internal fun update(reducer: (VoiceCaptureUiState) -> VoiceCaptureUiState) {
        _state.value = reducer(_state.value)
    }

    internal fun reset() {
        _state.value = VoiceCaptureUiState()
    }
}

/** Drives a single hold-to-talk transcription, gated on RECORD_AUDIO. */
class VoiceCapture internal constructor(
    private val requestPermission: () -> Unit,
    private val hasPermission: () -> Boolean,
    private val onPartialTranscript: (String) -> Unit = {},
    private val openRealtimeSession: (String?, RealtimeSpeechCallbacks) -> RealtimeSpeechSession?,
    private val transcribeOnce: suspend (String?) -> VoiceCaptureResult,
    private val strings: VoiceStrings = DefaultVoiceStrings,
) {
    private var session: RealtimeSpeechSession? = null
    private var pendingResult: ((VoiceCaptureResult) -> Unit)? = null
    private var generation = 0L

    /** True once RECORD_AUDIO has been granted. */
    fun isPermitted(): Boolean = hasPermission()

    /** True while one recognizer session is accepting audio or finalizing. */
    fun isActive(): Boolean = session != null

    /** Ask for RECORD_AUDIO (no-op if already granted). */
    fun ensurePermission() {
        if (!hasPermission()) {
            VoiceCaptureStore.update {
                it.copy(
                    phase = VoiceCapturePhase.PermissionRequired,
                    statusText = strings.resolve(R.string.voice_mic_permission_required, "需要麦克风权限"),
                    errorMessage = null,
                )
            }
            requestPermission()
        }
    }

    fun start(
        language: String? = null,
        onResult: ((VoiceCaptureResult) -> Unit)? = null,
    ) {
        if (!hasPermission()) {
            pendingResult = null
            VoiceCaptureStore.update {
                it.copy(
                    phase = VoiceCapturePhase.PermissionRequired,
                    statusText = strings.resolve(R.string.voice_mic_permission_required, "需要麦克风权限"),
                    errorMessage = null,
                    partialTranscript = "",
                    finalTranscript = "",
                )
            }
            requestPermission()
            onResult?.invoke(VoiceCaptureResult.PermissionDenied)
            return
        }
        val previousSession = session
        val captureGeneration = ++generation
        session = null
        pendingResult = onResult
        val terminalResultDelivered = AtomicBoolean(false)
        previousSession?.cancel()
        VoiceCaptureStore.update {
            it.copy(
                phase = VoiceCapturePhase.Starting,
                partialTranscript = "",
                finalTranscript = "",
                statusText = strings.resolve(R.string.voice_starting_mic, "正在启动麦克风…"),
                errorMessage = null,
            )
        }
        session = try {
            val openedSession = openRealtimeSession(language, object : RealtimeSpeechCallbacks {
                override fun onReady() {
                    if (generation != captureGeneration) return
                    VoiceCaptureStore.update {
                        it.copy(
                            phase = VoiceCapturePhase.Listening,
                            statusText = strings.resolve(R.string.voice_listening, "正在聆听…"),
                            errorMessage = null,
                        )
                    }
                }

                override fun onPartial(text: String) {
                    if (generation != captureGeneration) return
                    onPartialTranscript(text)
                    VoiceCaptureStore.update {
                        it.copy(
                            phase = VoiceCapturePhase.Listening,
                            partialTranscript = text,
                            statusText = text.ifBlank { strings.resolve(R.string.voice_listening, "正在聆听…") },
                            errorMessage = null,
                        )
                    }
                }

                override fun onFinal(text: String) {
                    if (generation != captureGeneration || !terminalResultDelivered.compareAndSet(false, true)) return
                    val trimmed = text.trim()
                    VoiceCaptureStore.update {
                        it.copy(
                            phase = VoiceCapturePhase.Completed,
                            partialTranscript = trimmed,
                            finalTranscript = trimmed,
                            statusText = trimmed.ifBlank {
                                strings.resolve(R.string.voice_no_speech_recognized, "未识别到语音")
                            },
                            errorMessage = null,
                        )
                    }
                    val callback = pendingResult
                    pendingResult = null
                    callback?.invoke(
                        if (trimmed.isBlank()) VoiceCaptureResult.Empty
                        else VoiceCaptureResult.Transcript(trimmed),
                    )
                    session = null
                }

                override fun onError(code: String, message: String, retriable: Boolean) {
                    if (generation != captureGeneration || !terminalResultDelivered.compareAndSet(false, true)) return
                    session = null
                    val result = when (code) {
                        "permission_denied" -> VoiceCaptureResult.PermissionDenied
                        "no_speech" -> VoiceCaptureResult.Empty
                        else -> VoiceCaptureResult.Failed(message)
                    }
                    VoiceCaptureStore.update {
                        it.copy(
                            phase = if (code == "no_speech") VoiceCapturePhase.Cancelled else VoiceCapturePhase.Failed,
                            statusText = when (result) {
                                VoiceCaptureResult.PermissionDenied ->
                                    strings.resolve(R.string.voice_mic_permission_required, "需要麦克风权限")
                                VoiceCaptureResult.Empty ->
                                    strings.resolve(R.string.voice_no_speech_recognized, "未识别到语音")
                                is VoiceCaptureResult.Failed -> if (retriable) {
                                    strings.resolve(R.string.voice_recognition_failed_retriable, "识别失败，可重试")
                                } else {
                                    strings.resolve(R.string.voice_recognition_failed, "识别失败")
                                }
                                is VoiceCaptureResult.Transcript -> result.text
                            },
                            errorMessage = when (result) {
                                is VoiceCaptureResult.Failed -> result.message
                                VoiceCaptureResult.PermissionDenied -> "permission_denied"
                                else -> null
                            },
                        )
                    }
                    val callback = pendingResult
                    pendingResult = null
                    callback?.invoke(result)
                }

                override fun onClosed() {
                    if (generation != captureGeneration) return
                    if (session != null && pendingResult == null) {
                        VoiceCaptureStore.update {
                            if (it.phase == VoiceCapturePhase.Listening || it.phase == VoiceCapturePhase.Starting || it.phase == VoiceCapturePhase.Stopping) {
                                it.copy(
                                    phase = VoiceCapturePhase.Cancelled,
                                    statusText = strings.resolve(R.string.voice_recording_cancelled, "录音已取消"),
                                )
                            } else {
                                it
                            }
                        }
                    }
                }
            })
            if (terminalResultDelivered.get()) null else openedSession
        } catch (e: IllegalStateException) {
            val callback = pendingResult
            pendingResult = null
            VoiceCaptureStore.update {
                it.copy(
                    phase = VoiceCapturePhase.Failed,
                    statusText = strings.resolve(R.string.voice_device_unsupported, "设备不支持语音识别"),
                    errorMessage = e.message,
                )
            }
            callback?.invoke(VoiceCaptureResult.Failed(e.message ?: "speech recognizer unavailable"))
            null
        } catch (t: Throwable) {
            val callback = pendingResult
            pendingResult = null
            VoiceCaptureStore.update {
                it.copy(
                    phase = VoiceCapturePhase.Failed,
                    statusText = strings.resolve(R.string.voice_start_recognition_failed, "启动语音识别失败"),
                    errorMessage = t.message,
                )
            }
            callback?.invoke(VoiceCaptureResult.Failed(t.message ?: "speech error"))
            null
        }
    }

    fun stop(onResult: ((VoiceCaptureResult) -> Unit)? = null) {
        if (!hasPermission()) {
            val callback = onResult ?: pendingResult
            pendingResult = null
            callback?.invoke(VoiceCaptureResult.PermissionDenied)
            return
        }
        val active = session
        if (active == null) {
            val callback = onResult ?: pendingResult
            pendingResult = null
            callback?.invoke(VoiceCaptureResult.Empty)
            return
        }
        if (onResult != null) pendingResult = onResult
        VoiceCaptureStore.update {
            it.copy(
                phase = VoiceCapturePhase.Stopping,
                statusText = it.partialTranscript.ifBlank {
                    strings.resolve(R.string.voice_stopping_recording, "正在结束录音…")
                },
            )
        }
        active.stop()
    }

    fun cancel() {
        generation++
        pendingResult = null
        val active = session
        session = null
        active?.cancel()
        VoiceCaptureStore.update {
            it.copy(
                phase = VoiceCapturePhase.Cancelled,
                statusText = strings.resolve(R.string.voice_recording_cancelled, "录音已取消"),
                errorMessage = null,
            )
        }
    }

    fun dispose() {
        generation++
        pendingResult = null
        val active = session
        session = null
        active?.cancel()
        VoiceCaptureStore.reset()
    }

    suspend fun transcribe(language: String? = null): VoiceCaptureResult {
        if (!hasPermission()) return VoiceCaptureResult.PermissionDenied
        return transcribeOnce(language)
    }
}

private object HeldVoiceSessionRegistry {
    private var capture: VoiceCapture? = null

    fun attach(value: VoiceCapture) {
        capture = value
    }

    fun detach(value: VoiceCapture) {
        if (capture === value) capture = null
    }

    fun cancelActive() {
        capture?.takeIf { it.isActive() }?.cancel()
    }
}

/** Stop held-mic capture without finalizing or submitting a late transcript. */
internal fun cancelActiveHeldVoiceSession() {
    HeldVoiceSessionRegistry.cancelActive()
}

/**
 * Compose entry point: returns the hold-to-talk handlers wired to a live
 * [VoiceCapture]. [onTranscript] receives the recognized text on a successful
 * release; the mic is gated on the RECORD_AUDIO runtime permission, which is
 * requested the first time the user holds the mic without it.
 *
 * @return Pair of (onHoldStart, onHoldRelease) for [VoiceFlowOverlay]'s gesture.
 */
@Composable
fun rememberVoiceCapture(
    onTranscript: (String) -> Unit,
    onPartialTranscript: (String) -> Unit = {},
    onError: (String, String) -> Unit = { _, _ -> },
): Pair<() -> Unit, () -> Unit> {
    val context = androidx.compose.ui.platform.LocalContext.current
    val coroutineScope = rememberCoroutineScope()
    val owner = remember(context) { AudioOwnerKey.ui("held-voice-${UUID.randomUUID()}") }
    val currentOnPartial = rememberUpdatedState(onPartialTranscript)
    val currentOnTranscript = rememberUpdatedState(onTranscript)
    val currentOnError = rememberUpdatedState(onError)

    val permLauncher = rememberLauncherForActivityResult(
        ActivityResultContracts.RequestPermission(),
    ) { granted ->
        if (!granted) currentOnError.value("permissionDenied", "Microphone permission denied")
    }

    val capture = remember(context) {
        VoiceCapture(
            requestPermission = { permLauncher.launch(Manifest.permission.RECORD_AUDIO) },
            hasPermission = {
                ContextCompat.checkSelfPermission(
                    context,
                    Manifest.permission.RECORD_AUDIO,
                ) == PackageManager.PERMISSION_GRANTED
            },
            onPartialTranscript = { currentOnPartial.value(it) },
            openRealtimeSession = { language, callbacks ->
                ServiceRealtimeListenSession(coroutineScope, callbacks) { serviceCallbacks ->
                    AndroidAudioServiceProvider.openRealtimeListen(context, owner, language, serviceCallbacks)
                }
            },
            transcribeOnce = { language ->
                when (val result = AndroidAudioServiceProvider.perform(
                    context = context,
                    owner = owner,
                    operation = DeviceAudioOperation.Listen(language),
                )) {
                    is DeviceAudioResult.Transcript -> VoiceCaptureResult.Transcript(result.text.trim())
                    is DeviceAudioResult.Failed -> when (result.error.kind) {
                        DeviceAudioErrorKind.PermissionDenied -> VoiceCaptureResult.PermissionDenied
                        DeviceAudioErrorKind.NoSpeech -> VoiceCaptureResult.Empty
                        else -> VoiceCaptureResult.Failed(result.error.message)
                    }
                    else -> VoiceCaptureResult.Failed("Audio service returned an unexpected listen result.")
                }
            },
            strings = voiceStrings(context),
        )
    }
    DisposableEffect(capture) {
        HeldVoiceSessionRegistry.attach(capture)
        onDispose {
            HeldVoiceSessionRegistry.detach(capture)
            capture.dispose()
        }
    }

    val onHoldStart: () -> Unit = {
        capture.ensurePermission()
        if (capture.isPermitted()) capture.start()
    }
    val onHoldRelease: () -> Unit = {
        capture.stop { r ->
            when (r) {
                is VoiceCaptureResult.Transcript -> currentOnTranscript.value(r.text)
                is VoiceCaptureResult.PermissionDenied -> currentOnError.value("permissionDenied", "Microphone permission denied")
                is VoiceCaptureResult.Empty -> currentOnError.value("noSpeech", "No speech recognized")
                is VoiceCaptureResult.Failed -> currentOnError.value("", r.message)
            }
        }
    }

    return onHoldStart to onHoldRelease
}

/** Adapts the service's live Listen operation to VoiceCapture's callback lifecycle. */
internal class ServiceRealtimeListenSession(
    scope: CoroutineScope,
    callbacks: RealtimeSpeechCallbacks,
    private val openServiceSession: suspend (RealtimeSpeechCallbacks) -> RealtimeSpeechSession,
) : RealtimeSpeechSession {
    @Volatile private var terminal = false
    @Volatile private var stopRequested = false
    @Volatile private var nativeSession: RealtimeSpeechSession? = null
    private val job: Job = scope.launch {
        try {
            val session = openServiceSession(callbacks)
            nativeSession = session
            if (stopRequested) session.stop()
            if (terminal) session.cancel()
        } catch (cancelled: kotlinx.coroutines.CancellationException) {
            if (!terminal) {
                terminal = true
                callbacks.onError("cancelled", "Speech recognition was cancelled.", false)
                callbacks.onClosed()
            }
        } catch (error: Throwable) {
            if (!terminal) {
                terminal = true
                val audioError = when (error) {
                    is AudioOperationException -> DeviceAudioError(error.kind, error.message ?: "Speech recognition failed.")
                    is AudioDriverException -> error.error
                    else -> DeviceAudioError(DeviceAudioErrorKind.NativeFailure, error.message ?: "Speech recognition failed.")
                }
                val code = when (audioError.kind) {
                    DeviceAudioErrorKind.PermissionDenied -> "permission_denied"
                    DeviceAudioErrorKind.NoSpeech -> "no_speech"
                    DeviceAudioErrorKind.Timeout -> "timeout"
                    DeviceAudioErrorKind.Cancelled -> "cancelled"
                    else -> audioError.kind.name.lowercase()
                }
                callbacks.onError(
                    code,
                    audioError.message,
                    audioError.kind in setOf(DeviceAudioErrorKind.Busy, DeviceAudioErrorKind.Timeout, DeviceAudioErrorKind.Unavailable, DeviceAudioErrorKind.NativeFailure),
                )
                callbacks.onClosed()
            }
        }
    }

    override fun stop() {
        stopRequested = true
        nativeSession?.stop()
    }

    override fun cancel() {
        terminal = true
        nativeSession?.cancel()
        job.cancel()
    }

    override fun close() = cancel()
}
