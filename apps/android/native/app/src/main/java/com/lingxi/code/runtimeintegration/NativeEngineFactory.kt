package com.lingxi.code.runtimeintegration

import android.app.ActivityManager
import android.content.Context
import android.util.Log
import com.lingxi.code.BuildConfig
import com.lingxi.code.bindings.android.AndroidEngineLaunchConfigFfi
import com.lingxi.code.bindings.android.AndroidEventListener
import com.lingxi.code.bindings.android.AndroidLaunchModeFfi
import com.lingxi.code.bindings.android.AndroidNativeMediaHost
import com.lingxi.code.bindings.android.AndroidPermissionSink
import com.lingxi.code.bindings.android.AndroidProviderConfigFfi
import com.lingxi.code.bindings.android.AndroidShellConfigFfi
import com.lingxi.code.bindings.android.buildAndroidEngine
import com.lingxi.code.bindings.client.ClientEvent
import com.lingxi.code.bindings.client.PermissionRequest
import com.lingxi.code.bindings.client.WorkflowProgressDto
import com.lingxi.code.bindings.runtime.MobileEngineHandle
import com.lingxi.code.clipboard.AndroidClipboardAdapter
import com.lingxi.code.computeruse.ComputerUseFeatureProvider
import com.lingxi.code.device.AndroidDeviceControlAdapter
import com.lingxi.code.location.AndroidLocationAdapter
import com.lingxi.code.model.SessionMode
import com.lingxi.code.model.toDto
import com.lingxi.code.notify.AndroidNotificationAdapter
import com.lingxi.code.project.ProjectWorkspace
import com.lingxi.code.secure.AndroidSecureStorageAdapter
import com.lingxi.code.settings.LinuxRuntimeBridge
import com.lingxi.code.settings.LinuxRuntimeMode
import com.lingxi.code.share.AndroidShareAdapter
import com.lingxi.code.voice.audio.AndroidNativeMediaHostProvider

private const val TAG = "NativeEngineFactory"

/** Callbacks for an interactive engine. Permission callbacks must return promptly;
 * the caller submits the user's answer to the engine asynchronously.
 */
internal data class NativeEngineCallbacks(
    val onEvent: suspend (ClientEvent) -> Unit = { event ->
        Log.d(TAG, "engine event: ${event::class.simpleName}")
    },
    val onWorkflowProgress: suspend (String, String, String, WorkflowProgressDto) -> Unit =
        { originSessionId, taskId, runId, _ ->
            Log.d(TAG, "workflow progress: $originSessionId/$taskId/$runId")
        },
    val onPermission: suspend (PermissionRequest) -> Unit = { request ->
        Log.w(TAG, "permission request dropped (no prompt UI wired): ${request.requestId}")
    },
)

/** The launch scenario chooses the engine's permission policy and callback owner. */
internal sealed interface NativeEngineScenario {
    data class Interactive(val callbacks: NativeEngineCallbacks) : NativeEngineScenario
    data object ScheduledHeadless : NativeEngineScenario
    data object LocalAppAgent : NativeEngineScenario
}

internal data class NativeEngineLaunchPolicy(
    val launchMode: AndroidLaunchModeFfi,
    val appAgentHost: Boolean,
    val callbacks: NativeEngineCallbacks,
)

internal fun nativeEngineLaunchPolicy(scenario: NativeEngineScenario): NativeEngineLaunchPolicy =
    when (scenario) {
        is NativeEngineScenario.Interactive -> NativeEngineLaunchPolicy(
            AndroidLaunchModeFfi.INTERACTIVE, false, scenario.callbacks,
        )
        NativeEngineScenario.ScheduledHeadless -> NativeEngineLaunchPolicy(
            AndroidLaunchModeFfi.SCHEDULED_HEADLESS, false,
            NativeEngineCallbacks(onEvent = {}, onPermission = {}),
        )
        NativeEngineScenario.LocalAppAgent -> NativeEngineLaunchPolicy(
            AndroidLaunchModeFfi.INTERACTIVE, true,
            NativeEngineCallbacks(onEvent = {}, onPermission = {}),
        )
    }

internal fun NativeEngineCallbacks.eventListener(): AndroidEventListener = object : AndroidEventListener {
    override suspend fun onEvent(event: ClientEvent) = this@eventListener.onEvent(event)

    override suspend fun onWorkflowProgress(
        originSessionId: String,
        taskId: String,
        runId: String,
        progress: WorkflowProgressDto,
    ) = this@eventListener.onWorkflowProgress(originSessionId, taskId, runId, progress)
}

internal fun NativeEngineCallbacks.permissionSink(): AndroidPermissionSink = object : AndroidPermissionSink {
    override suspend fun onRequest(request: PermissionRequest) = this@permissionSink.onPermission(request)
}

/**
 * Constructs an Android engine using the process-owned media host and platform
 * adapters. The caller owns and destroys the returned handle; destroying a handle
 * must not destroy the app-scoped media host shared with other engines and Remote
 * Control. Returns null when the native engine is unavailable.
 */
internal object NativeEngineFactory {
    fun build(
        context: Context,
        scenario: NativeEngineScenario,
        apiBase: String = "",
        apiKey: String = "",
        model: String = "",
        providerProfilesJson: String = "{}",
        routingJson: String? = null,
        visionDelegationEnabled: Boolean = true,
        mediaHost: AndroidNativeMediaHost? = null,
        projectWorkspace: ProjectWorkspace? = null,
        sessionMode: SessionMode = SessionMode.Code,
        linuxRuntimeMode: LinuxRuntimeMode = LinuxRuntimeMode.MobileLinux,
    ): MobileEngineHandle? {
        val appContext = context.applicationContext
        val appMediaHost = mediaHost ?: AndroidNativeMediaHostProvider.get(appContext)
        val policy = nativeEngineLaunchPolicy(scenario)
        val share = AndroidShareAdapter()
        val notifications = AndroidNotificationAdapter()
        val clipboard = AndroidClipboardAdapter()
        val location = AndroidLocationAdapter()
        val listener = policy.callbacks.eventListener()
        val permissions = policy.callbacks.permissionSink()
        return try {
            buildAndroidEngine(
                config = AndroidEngineLaunchConfigFfi(
                    appVersion = BuildConfig.VERSION_NAME,
                    appAgentHost = policy.appAgentHost,
                    apiBase = apiBase,
                    apiKey = apiKey,
                    model = model,
                    sessionMode = sessionMode.toDto(),
                    visionDelegationEnabled = visionDelegationEnabled,
                    appFilesRoot = appContext.filesDir.canonicalPath,
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
                    physicalMemoryBytes = runCatching {
                        val memoryInfo = ActivityManager.MemoryInfo()
                        val activityManager = appContext.getSystemService(Context.ACTIVITY_SERVICE) as ActivityManager
                        activityManager.getMemoryInfo(memoryInfo)
                        memoryInfo.totalMem.takeIf { it > 0L }?.toULong() ?: 0uL
                    }.getOrDefault(0uL),
                    hostEnvironment = androidHostEnvironment(appContext, policy.launchMode),
                ),
                listener = listener,
                mediaHost = appMediaHost,
                share = share,
                location = location,
                notifications = notifications,
                clipboard = clipboard,
                permissions = permissions,
                // Direct builds supply the user-started control host; Play builds
                // supply null. Runtime grants remain scoped to the control session.
                computerUse = ComputerUseFeatureProvider.engineHost(),
                // The Agent Shell shares the terminal's verified PRoot runtime.
                shell = AndroidShellConfigFfi(
                    enableShell = true,
                    secretsInKeystore = true,
                    shellDataExposureAccepted = true,
                ),
                git = null,
                gitCredentialProvider = null,
                secureStorage = AndroidSecureStorageAdapter(appContext),
                deviceControl = AndroidDeviceControlAdapter(),
            )
        } catch (t: Throwable) {
            Log.w(TAG, "buildAndroidEngine unavailable: ${t.message}", t)
            null
        }
    }
}
