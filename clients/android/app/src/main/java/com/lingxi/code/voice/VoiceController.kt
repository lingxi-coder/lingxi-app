package com.lingxi.code.voice

import android.Manifest
import android.app.ActivityManager
import android.content.Context
import android.content.pm.PackageManager
import android.content.res.Configuration
import android.os.Build
import android.util.Log
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.core.content.ContextCompat
import androidx.core.content.pm.PackageInfoCompat
import com.lingxi.code.BuildConfig
import com.lingxi.code.R
import com.lingxi.code.bindings.AndroidEventListener
import com.lingxi.code.bindings.AndroidEngineLaunchConfigFfi
import com.lingxi.code.bindings.AndroidDeviceClassFfi
import com.lingxi.code.bindings.AndroidExecutionTargetFfi
import com.lingxi.code.bindings.AndroidHostEnvironmentFfi
import com.lingxi.code.bindings.AndroidLaunchModeFfi
import com.lingxi.code.bindings.AndroidPermissionSink
import com.lingxi.code.bindings.AndroidProviderConfigFfi
import com.lingxi.code.bindings.AndroidShellConfigFfi
import com.lingxi.code.bindings.ClientEvent
import com.lingxi.code.bindings.MobileEngineHandle
import com.lingxi.code.bindings.PermissionRequest
import com.lingxi.code.bindings.SpeechFfiException
import com.lingxi.code.bindings.WorkflowProgressDto
import com.lingxi.code.bindings.buildAndroidEngineWithMobileLinux
import com.lingxi.code.location.AndroidLocationAdapter
import com.lingxi.code.computeruse.ComputerUseFeatureProvider
import com.lingxi.code.voice.audio.AndroidSttAdapter
import com.lingxi.code.voice.audio.AndroidTtsAdapter
import com.lingxi.code.voice.audio.SystemSpeechRecognizerStt
import com.lingxi.code.voice.audio.SystemTextToSpeechTts
import com.lingxi.code.vision.AndroidCameraAdapter
import com.lingxi.code.share.AndroidShareAdapter
import com.lingxi.code.notify.AndroidNotificationAdapter
import com.lingxi.code.project.ProjectWorkspace
import com.lingxi.code.clipboard.AndroidClipboardAdapter
import com.lingxi.code.secure.AndroidSecureStorageAdapter
import com.lingxi.code.settings.LinuxRuntimeBridge
import com.lingxi.code.settings.LinuxRuntimeMode
import com.lingxi.code.voice.recorder.AndroidVoiceAdapter
import com.lingxi.code.voice.audio.RealtimeSpeechCallbacks
import com.lingxi.code.voice.audio.RealtimeSpeechSession
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow

private const val TAG = "VoiceController"

internal fun androidDeviceClass(smallestScreenWidthDp: Int): AndroidDeviceClassFfi = when {
    smallestScreenWidthDp <= Configuration.SMALLEST_SCREEN_WIDTH_DP_UNDEFINED ->
        AndroidDeviceClassFfi.UNKNOWN
    smallestScreenWidthDp >= 600 -> AndroidDeviceClassFfi.TABLET
    else -> AndroidDeviceClassFfi.PHONE
}

internal fun androidExecutionTarget(
    fingerprint: String,
    model: String,
    manufacturer: String,
    brand: String,
    device: String,
    product: String,
    hardware: String,
): AndroidExecutionTargetFfi {
    val values = listOf(fingerprint, model, manufacturer, brand, device, product, hardware)
    if (values.all { it.isBlank() || it.equals("unknown", ignoreCase = true) }) {
        return AndroidExecutionTargetFfi.UNKNOWN
    }

    val emulator = fingerprint.startsWith("generic", ignoreCase = true) ||
        model.contains("google_sdk", ignoreCase = true) ||
        model.contains("emulator", ignoreCase = true) ||
        model.contains("android sdk built for", ignoreCase = true) ||
        manufacturer.contains("genymotion", ignoreCase = true) ||
        hardware.equals("goldfish", ignoreCase = true) ||
        hardware.equals("ranchu", ignoreCase = true) ||
        product.contains("sdk_gphone", ignoreCase = true) ||
        product.contains("google_sdk", ignoreCase = true) ||
        product.contains("emulator", ignoreCase = true) ||
        product.contains("simulator", ignoreCase = true) ||
        (brand.startsWith("generic", ignoreCase = true) &&
            device.startsWith("generic", ignoreCase = true))

    return if (emulator) AndroidExecutionTargetFfi.EMULATOR
    else AndroidExecutionTargetFfi.PHYSICAL_DEVICE
}

internal fun androidHostOsVersion(release: String, apiLevel: Int): String? {
    val normalizedRelease = release.trim()
    return when {
        normalizedRelease.isNotEmpty() && apiLevel > 0 -> "$normalizedRelease (API $apiLevel)"
        normalizedRelease.isNotEmpty() -> normalizedRelease
        apiLevel > 0 -> "API $apiLevel"
        else -> null
    }
}

private fun androidHostEnvironment(
    context: Context,
    launchMode: AndroidLaunchModeFfi,
): AndroidHostEnvironmentFfi = AndroidHostEnvironmentFfi(
    hostOsVersion = androidHostOsVersion(Build.VERSION.RELEASE, Build.VERSION.SDK_INT),
    deviceClass = androidDeviceClass(context.resources.configuration.smallestScreenWidthDp),
    executionTarget = androidExecutionTarget(
        fingerprint = Build.FINGERPRINT,
        model = Build.MODEL,
        manufacturer = Build.MANUFACTURER,
        brand = Build.BRAND,
        device = Build.DEVICE,
        product = Build.PRODUCT,
        hardware = Build.HARDWARE,
    ),
    launchMode = launchMode,
)

/**
 * Resolves a localized string for voice-package code that runs OUTSIDE a
 * `@Composable` body — [VoiceCapture] — and therefore cannot call
 * `stringResource()`. Mirrors `ConversationStrings`
 * (conversation/ConversationSource.kt) and `LocalAppsStrings`
 * (localapps/LocalAppsContract.kt): the fallback keeps `VoiceCaptureLifecycleTest`
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
fun voiceStrings(context: Context): VoiceStrings =
    VoiceStrings { id, _, args -> context.getString(id, *args) }

/**
 * T3.3 — wire the device-audio path into the engine + the Compose hold-to-talk
 * surface.
 *
 * Two concerns live here:
 *  1. [buildVoiceEngine] constructs the real [MobileEngineHandle] through the
 *     generated UniFFI `buildAndroidEngine(...)`, handing it the [AndroidStt] /
 *     [AndroidTts] adapters (T3.2) over the device's system recognizer /
 *     synthesizer plus a foreground event listener and config. This is the
 *     end-to-end FFI seam: the engine's `tool-speech` now routes through the
 *     device's native STT/TTS. On a non-Android host (and if the cdylib fails to
 *     load) it returns `null` rather than crashing the shell.
 *  2. [rememberVoiceCapture] turns the hold-to-talk release into a real
 *     transcription. Releasing the held mic checks the `RECORD_AUDIO` runtime
 *     permission (requesting it on first use) and, when granted, drives the same
 *     [SystemSpeechRecognizerStt] adapter to capture one utterance; the
 *     recognized text is routed back into the composer draft via [onTranscript].
 */

/**
 * Build the engine over the device's native speech stack. The STT/TTS adapters
 * are the exact objects the engine bridges onto `traits::SpeechToText` /
 * `traits::TextToSpeech`, so the speech tool runs on-device.
 *
 * [onEvent] is the single sink for every inbound engine [ClientEvent]: the
 * caller (the conversation source) owns the registered listener so the chat
 * surface streams the REAL turn loop. It defaults to a `Log.d` trace so callers
 * that only need the device-capability wiring (e.g. the voice path) still build
 * a working handle without re-implementing a listener.
 *
 * [onPermission] is the single sink for every OUTBOUND [PermissionRequest]: when
 * a tool needs approval the engine parks the turn and emits the request through
 * the `AndroidPermissionSink` registered here. The caller (the conversation
 * source) enqueues a prompt and resolves it via
 * `handle.submit(ClientCommand.Approve/DenyPermission)`. SHIP-BLOCKER #3: without
 * this sink the prior `NoopPermissionSink` DROPPED the request and the turn hung
 * forever. It defaults to a `Log.w` trace so the voice-only path still builds.
 *
 * Returns `null` when the engine cannot be built — on a JVM/unit host the
 * `buildAndroidEngine` export returns `PlatformUnavailable`, and a missing
 * cdylib throws on class init; either way the chat shell stays usable.
 */
fun buildVoiceEngine(
    context: Context,
    apiBase: String = "",
    apiKey: String = "",
    model: String = "",
    providerProfilesJson: String = "{}",
    routingJson: String? = null,
    projectWorkspace: ProjectWorkspace? = null,
    linuxRuntimeMode: LinuxRuntimeMode = LinuxRuntimeMode.Legacy,
    launchMode: AndroidLaunchModeFfi = AndroidLaunchModeFfi.INTERACTIVE,
    onEvent: suspend (ClientEvent) -> Unit = { event ->
        Log.d(TAG, "engine event: ${event::class.simpleName}")
    },
    onWorkflowProgress: suspend (String, String, String, WorkflowProgressDto) -> Unit =
        { originSessionId, taskId, runId, _ ->
            Log.d(TAG, "workflow progress: $originSessionId/$taskId/$runId")
        },
    onPermission: suspend (PermissionRequest) -> Unit = { request ->
        Log.w(TAG, "permission request dropped (no prompt UI wired): ${request.requestId}")
    },
): MobileEngineHandle? {
    val appContext = context.applicationContext
    val stt = AndroidSttAdapter(SystemSpeechRecognizerStt(appContext))
    val tts = AndroidTtsAdapter(SystemTextToSpeechTts(appContext))
    // Device-vision: the camera adapter drives the process-global CameraController,
    // whose ActivityResult launchers are registered by MainActivity. The engine
    // bridges this onto `traits::CameraControl`, lighting up `tool-camera` on-device.
    val camera = AndroidCameraAdapter()
    // Device-share: the share adapter drives the process-global ShareController,
    // whose Context is attached by MainActivity. The engine bridges this onto
    // `traits::SharingService`, lighting up `tool-share` on-device.
    val share = AndroidShareAdapter()
    // Device-voice: the voice adapter drives the process-global RecorderController,
    // whose Context is attached by MainActivity. The engine bridges this onto
    // `traits::VoiceRecorder`, lighting up `tool-voice`'s raw mic recorder
    // on-device (distinct from the STT hold-to-talk path above).
    val voice = AndroidVoiceAdapter()
    // Device-notifications: the notification adapter drives the process-global
    // NotificationController, whose Context is attached by MainActivity. The
    // engine bridges this onto `traits::NotificationService`, lighting up
    // `tool-notification` on-device (engine-driven; no UI affordance).
    val notifications = AndroidNotificationAdapter()
    // Device-clipboard: the clipboard adapter drives the process-global
    // ClipboardController, whose Context is attached by MainActivity. The engine
    // bridges this onto `traits::Clipboard`, lighting up `tool-clipboard`
    // on-device (engine-driven; no UI affordance).
    val clipboard = AndroidClipboardAdapter()
    // Device-location: the local-app capability gate authorizes the operation
    // before this adapter requests Android's fine/coarse runtime permission.
    val location = AndroidLocationAdapter()
    val listener = object : AndroidEventListener {
        override suspend fun onEvent(event: ClientEvent) {
            // Single sink: forward every inbound engine event to the caller's
            // [onEvent]. EngineConversationSource passes a sink that pushes into
            // its SharedFlow, so the chat surface streams the REAL turn loop.
            onEvent(event)
        }

        override suspend fun onWorkflowProgress(
            originSessionId: String,
            taskId: String,
            runId: String,
            progress: WorkflowProgressDto,
        ) {
            onWorkflowProgress(originSessionId, taskId, runId, progress)
        }
    }
    // Device-permissions: the engine's adapter gate emits an OUTBOUND
    // PermissionRequest through this sink whenever a tool needs approval. The
    // adapter forwards it to the caller's [onPermission] (EngineConversationSource
    // pushes it into its pending-permission StateFlow, which the Compose prompt
    // renders). This MUST return promptly — the user's answer comes back
    // asynchronously via `handle.submit(Approve/DenyPermission)`.
    val permissions = object : AndroidPermissionSink {
        override suspend fun onRequest(request: PermissionRequest) {
            onPermission(request)
        }
    }
    val shellWorkspace = projectWorkspace?.hostPath
        ?.let { java.io.File(it) }
        ?: java.io.File(appContext.filesDir, "shell/workspaces/default")
    if (!shellWorkspace.exists() && !shellWorkspace.mkdirs()) {
        Log.w(TAG, "Unable to create shell workspace at ${shellWorkspace.absolutePath}")
    }
    val packageInfo = runCatching {
        appContext.packageManager.getPackageInfo(appContext.packageName, 0)
    }.getOrNull()
    val writableRoots = buildList {
        add(appContext.filesDir.absolutePath)
        add(appContext.cacheDir.absolutePath)
        add(appContext.codeCacheDir.absolutePath)
        add(appContext.noBackupFilesDir.absolutePath)
        projectWorkspace?.hostPath?.let(::add)
    }.distinct()
    return try {
        buildAndroidEngineWithMobileLinux(
            config = AndroidEngineLaunchConfigFfi(
                apiBase = apiBase,
                apiKey = apiKey,
                model = model,
                appFilesRoot = appContext.filesDir.absolutePath,
                projectCwd = projectWorkspace?.hostPath,
                providerConfig = AndroidProviderConfigFfi(
                    providerProfilesJson = providerProfilesJson,
                    routingJson = routingJson,
                ),
                mobileLinux = LinuxRuntimeBridge.configForWorkspace(
                    context = appContext,
                    mode = linuxRuntimeMode,
                    workspace = projectWorkspace,
                ),
                localAppsFullRuntime = BuildConfig.MOBILE_LINUX_FULL,
                localAppsRuntimeRoot = null,
                physicalMemoryBytes = runCatching {
                    val memoryInfo = ActivityManager.MemoryInfo()
                    val activityManager = appContext.getSystemService(Context.ACTIVITY_SERVICE) as ActivityManager
                    activityManager.getMemoryInfo(memoryInfo)
                    memoryInfo.totalMem.takeIf { it > 0L }?.toULong() ?: 0uL
                }.getOrDefault(0uL),
                hostEnvironment = androidHostEnvironment(appContext, launchMode),
            ),
            listener = listener,
            stt = stt,
            tts = tts,
            camera = camera,
            share = share,
            voice = voice,
            location = location,
            notifications = notifications,
            clipboard = clipboard,
            permissions = permissions,
            // Direct builds inject the user-started Accessibility/MediaProjection
            // controller. Play builds return null, so `android_use` is absent.
            computerUse = ComputerUseFeatureProvider.engineHost(),
            // This single registration seam selects the ProcessRunner supplied
            // by AndroidPlatform: MobileLinux when explicitly selected, or the
            // bundled minijail/mksh/toybox backend in Legacy mode. MobileLinux
            // probe failures remain visible and never downgrade silently.
            shell = AndroidShellConfigFfi(
                nativeLibraryDir = appContext.applicationInfo.nativeLibraryDir,
                shellWorkspaceRoot = shellWorkspace.absolutePath,
                appCacheRoot = appContext.cacheDir.absolutePath,
                packageName = appContext.packageName,
                packageVersionCode = packageInfo
                    ?.let(PackageInfoCompat::getLongVersionCode)
                    ?: 0L,
                appWritableRoots = writableRoots,
                enableShell = true,
                secretsInKeystore = true,
                // Shell-visible workspace data is an advertised capability of
                // this Android distribution. Sensitive invocations still flow
                // through AndroidPermissionSink.
                shellDataExposureAccepted = true,
            ),
            // P4: Git-tool config not surfaced in the app UI yet — null keeps
            // Git support fully absent (spec P4 §G5 registration gate).
            git = null,
            // Provider profiles contain only non-secret endpoint/model metadata.
            // API keys travel through SetProviderCredential into Keystore.
            // Per-op credential provider not surfaced in the app UI yet — null
            // means no Git secrets are available (anonymous/public remotes only).
            gitCredentialProvider = null,
            // Native Android Keystore secure store — enables OAuth /login token
            // persist (flips the engine's oauth_supported true). Rooted under the
            // app-private filesDir.
            secureStorage = AndroidSecureStorageAdapter(appContext),
        )
    } catch (t: Throwable) {
        // PlatformUnavailable on a host build, or UnsatisfiedLinkError when the
        // native lib for this ABI is absent — degrade to the UI shell.
        Log.w(TAG, "buildAndroidEngine unavailable: ${t.message}", t)
        null
    }
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
    private val context: Context,
    private val requestPermission: () -> Unit,
    private val hasPermission: () -> Boolean,
    private val onPartialTranscript: (String) -> Unit = {},
    private val openRealtimeSession: (String?, RealtimeSpeechCallbacks) -> RealtimeSpeechSession? = { language, callbacks ->
        SystemSpeechRecognizerStt(context.applicationContext).openRealtimeSession(language, callbacks)
    },
    private val transcribeOnce: suspend (String?) -> VoiceCaptureResult = { language ->
        val adapter = AndroidSttAdapter(SystemSpeechRecognizerStt(context.applicationContext))
        try {
            val text = adapter.transcribe(language).trim()
            if (text.isEmpty()) VoiceCaptureResult.Empty
            else VoiceCaptureResult.Transcript(text)
        } catch (e: SpeechFfiException.PermissionDenied) {
            VoiceCaptureResult.PermissionDenied
        } catch (e: SpeechFfiException.NoSpeech) {
            VoiceCaptureResult.Empty
        } catch (e: SpeechFfiException) {
            VoiceCaptureResult.Failed(e.message ?: "speech error")
        } catch (t: Throwable) {
            VoiceCaptureResult.Failed(t.message ?: "speech error")
        }
    },
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
            openRealtimeSession(language, object : RealtimeSpeechCallbacks {
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
                    if (generation != captureGeneration) return
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
                    if (generation != captureGeneration) return
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
                            errorMessage = if (result is VoiceCaptureResult.Failed) result.message else null,
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

/**
 * Lifecycle-aware control for Flow Mode's tap-to-talk recognizer.
 *
 * A first tap starts one live recognizer session, a second tap asks Android to
 * finish that same utterance, and [cancel] tears the session down without
 * delivering a late transcript.
 */
interface OrbVoiceListenController : ((String?) -> Unit) -> Unit {
    fun isActive(): Boolean
    fun start(onResult: (String?) -> Unit)
    fun stop()
    fun cancel()

    override fun invoke(onResult: (String?) -> Unit) {
        start(onResult)
    }
}

internal class DefaultOrbVoiceListenController(
    private val capture: VoiceCapture,
) : OrbVoiceListenController {
    override fun isActive(): Boolean = capture.isActive()

    override fun start(onResult: (String?) -> Unit) {
        if (capture.isActive()) {
            capture.stop()
            return
        }
        capture.ensurePermission()
        if (!capture.isPermitted()) {
            onResult(null)
            return
        }
        capture.start { result ->
            onResult((result as? VoiceCaptureResult.Transcript)?.text)
        }
    }

    override fun stop() {
        capture.stop()
    }

    override fun cancel() {
        capture.cancel()
    }
}

private object OrbVoiceSessionRegistry {
    private var controller: OrbVoiceListenController? = null

    fun attach(value: OrbVoiceListenController) {
        controller = value
    }

    fun detach(value: OrbVoiceListenController) {
        if (controller === value) controller = null
    }

    fun stopActive(): Boolean {
        val activeController = controller?.takeIf { it.isActive() } ?: return false
        activeController.stop()
        return true
    }

    fun cancelActive() {
        controller?.takeIf { it.isActive() }?.cancel()
    }
}

internal fun stopActiveOrbVoiceSession(): Boolean =
    OrbVoiceSessionRegistry.stopActive()

internal fun cancelActiveOrbVoiceSession() {
    OrbVoiceSessionRegistry.cancelActive()
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
): Pair<() -> Unit, () -> Unit> {
    val context = androidx.compose.ui.platform.LocalContext.current
    val currentOnPartial = rememberUpdatedState(onPartialTranscript)
    val currentOnTranscript = rememberUpdatedState(onTranscript)

    val permLauncher = rememberLauncherForActivityResult(
        ActivityResultContracts.RequestPermission(),
    ) { /* result observed on the next hold via hasPermission() */ }

    val capture = remember(context) {
        VoiceCapture(
            context = context,
            requestPermission = { permLauncher.launch(Manifest.permission.RECORD_AUDIO) },
            hasPermission = {
                ContextCompat.checkSelfPermission(
                    context,
                    Manifest.permission.RECORD_AUDIO,
                ) == PackageManager.PERMISSION_GRANTED
            },
            onPartialTranscript = { currentOnPartial.value(it) },
            strings = voiceStrings(context),
        )
    }
    DisposableEffect(capture) {
        onDispose { capture.dispose() }
    }

    val onHoldStart: () -> Unit = {
        capture.ensurePermission()
        if (capture.isPermitted()) capture.start()
    }
    val onHoldRelease: () -> Unit = {
        capture.stop { r ->
            when (r) {
                is VoiceCaptureResult.Transcript -> currentOnTranscript.value(r.text)
                is VoiceCaptureResult.PermissionDenied -> capture.ensurePermission()
                is VoiceCaptureResult.Empty -> Unit
                is VoiceCaptureResult.Failed -> Log.w(TAG, "transcription failed: ${r.message}")
            }
        }
    }

    return onHoldStart to onHoldRelease
}

/**
 * Compose entry point for the FlowMode orb's one-shot listen. The recognizer
 * remains live after [OrbVoiceListenController.start] until Android delivers a
 * final/error result, the user calls [OrbVoiceListenController.stop], or the
 * overlay calls [OrbVoiceListenController.cancel].
 */
@Composable
fun rememberOrbVoiceListen(): OrbVoiceListenController {
    val context = androidx.compose.ui.platform.LocalContext.current
    val permLauncher = rememberLauncherForActivityResult(
        ActivityResultContracts.RequestPermission(),
    ) { /* observed on the next listen via hasPermission() */ }
    val capture = remember(context) {
        VoiceCapture(
            context = context,
            requestPermission = { permLauncher.launch(Manifest.permission.RECORD_AUDIO) },
            hasPermission = {
                ContextCompat.checkSelfPermission(
                    context, Manifest.permission.RECORD_AUDIO,
                ) == PackageManager.PERMISSION_GRANTED
            },
            strings = voiceStrings(context),
        )
    }
    val controller = remember(capture) {
        DefaultOrbVoiceListenController(capture)
    }
    DisposableEffect(capture, controller) {
        OrbVoiceSessionRegistry.attach(controller)
        onDispose {
            OrbVoiceSessionRegistry.detach(controller)
            capture.dispose()
        }
    }
    return controller
}
