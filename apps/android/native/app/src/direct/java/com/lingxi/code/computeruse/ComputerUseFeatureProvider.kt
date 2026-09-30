package com.lingxi.code.computeruse

import android.Manifest
import android.accessibilityservice.AccessibilityService
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.graphics.Rect
import android.media.projection.MediaProjectionManager
import android.os.Build
import android.os.Bundle
import android.provider.Settings
import android.util.Base64
import android.view.View
import android.view.accessibility.AccessibilityEvent
import android.view.accessibility.AccessibilityNodeInfo
import androidx.core.app.NotificationCompat
import androidx.core.app.NotificationManagerCompat
import androidx.core.content.ContextCompat
import com.lingxi.code.MainActivity
import com.lingxi.code.R
import com.lingxi.code.bindings.android.AndroidComputerUseFfiException
import com.lingxi.code.bindings.android.AndroidComputerUseHost
import com.lingxi.code.bindings.android.AndroidScreenshotFfi
import com.lingxi.code.settings.AudioConfigurationRepository
import com.lingxi.code.settings.settingsKey
import com.lingxi.code.voice.audio.AudioOperationException
import com.lingxi.code.voice.audio.DeviceAudioErrorKind
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.withContext
import kotlinx.coroutines.withTimeout
import kotlinx.coroutines.TimeoutCancellationException
import org.json.JSONArray
import org.json.JSONObject
import java.nio.ByteBuffer
import java.security.SecureRandom
import java.util.UUID
import java.util.concurrent.atomic.AtomicLong
import javax.crypto.Mac
import javax.crypto.spec.SecretKeySpec

object ComputerUseFeatureProvider : ComputerUseFeature, AndroidComputerUseHost {
    private const val SYSTEM_UI_PACKAGE = "com.android.systemui"
    private const val MAX_NODES = 500
    private const val MAX_DEPTH = 50
    private const val MAX_SECURITY_SCAN_NODES = 2_000
    private const val MAX_TREE_BYTES = 512 * 1024
    private const val APPROVAL_TIMEOUT_MS = 60_000L
    private val ACCESSIBILITY_DISCONNECT_ERRORS = setOf(
        "无障碍服务已断开",
        "系统中断了无障碍服务",
        "无障碍服务断开，控制会话已停止",
    )

    private val mutableState = MutableStateFlow(ComputerUseUiState())
    private val mutableApproval = MutableStateFlow<ComputerUseApproval?>(null)
    private val mutableConfiguration = MutableStateFlow(ComputerUseConfiguration())
    private val generation = AtomicLong(1)
    private val lastEventAt = AtomicLong(System.currentTimeMillis())
    private val lastInteractionAt = AtomicLong(System.currentTimeMillis())
    private val tokenKey = ByteArray(32).also(SecureRandom()::nextBytes)
    private val grantLock = Any()
    private val grants = linkedMapOf<String, ComputerUseGrant>()
    private val operationMutex = Mutex()

    @Volatile
    private var accessibility: LingXiAccessibilityService? = null

    @Volatile
    private var projectionCapture: MediaProjectionCapture? = null

    @Volatile
    private var applicationContext: Context? = null

    @Volatile
    private var emergencyStop: (() -> Unit)? = null

    @Volatile
    private var auditStore: ComputerUseAuditStore? = null

    @Volatile
    private var settingsStore: ComputerUseSettingsStore? = null

    @Volatile
    private var audioController: ComputerUseAudioController? = null

    @Volatile
    private var microphoneForegroundReady = false

    @Volatile
    private var approvalDeferred: CompletableDeferred<Boolean>? = null

    @Volatile
    private var stopping = false

    private val audit: ComputerUseAuditStore
        get() = auditStore ?: ComputerUseAuditStore(checkNotNull(applicationContext)).also {
            auditStore = it
        }

    override val available: Boolean = true
    override val state: StateFlow<ComputerUseUiState> = mutableState
    override val pendingApproval: StateFlow<ComputerUseApproval?> = mutableApproval
    override val configuration: StateFlow<ComputerUseConfiguration> = mutableConfiguration

    override fun attach(context: Context, onEmergencyStop: () -> Unit) {
        applicationContext = context.applicationContext
        if (auditStore == null) auditStore = ComputerUseAuditStore(context.applicationContext)
        if (settingsStore == null) {
            settingsStore = ComputerUseSettingsStore(context.applicationContext).also {
                mutableConfiguration.value = it.load()
            }
        }
        if (audioController == null) {
            audioController = ComputerUseAudioController(context.applicationContext)
        }
        emergencyStop = onEmergencyStop
        refreshServiceStatus()
    }

    override fun listLaunchableApps(context: Context): List<ComputerUseApp> {
        val launcherIntent = Intent(Intent.ACTION_MAIN).addCategory(Intent.CATEGORY_LAUNCHER)
        val apps = context.packageManager
            .queryIntentActivities(launcherIntent, PackageManager.MATCH_DEFAULT_ONLY)
            .asSequence()
            .map { info ->
                ComputerUseApp(
                    packageName = info.activityInfo.packageName,
                    label = info.loadLabel(context.packageManager).toString(),
                )
            }
            .filterNot { it.packageName == context.packageName }
            .distinctBy(ComputerUseApp::packageName)
            .sortedBy { it.label.lowercase() }
            .toMutableList()
        apps.add(
            0,
            ComputerUseApp(
                packageName = SYSTEM_UI_PACKAGE,
                label = "系统界面（主屏幕 / 通知 / 最近任务）",
                systemUi = true,
            ),
        )
        return apps
    }

    override fun mediaProjectionRequest(context: Context): Intent? {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) return null
        return context.getSystemService(MediaProjectionManager::class.java)
            .createScreenCaptureIntent()
    }

    override fun start(
        context: Context,
        grants: List<ComputerUseGrant>,
        includeSystemUi: Boolean,
        projectionResultCode: Int?,
        projectionData: Intent?,
    ): Result<Unit> = runCatching {
        attach(context, emergencyStop ?: {})
        check(accessibility != null) { "请先在系统设置中启用灵犀 Computer Use 无障碍服务" }
        check(grants.isNotEmpty()) { "至少选择一个允许控制的应用" }
        if (
            Build.VERSION.SDK_INT >= 33 &&
            ContextCompat.checkSelfPermission(context, Manifest.permission.POST_NOTIFICATIONS) !=
            PackageManager.PERMISSION_GRANTED
        ) {
            error("Android 13+ 必须允许通知，才能显示不可静默关闭的控制提示")
        }
        check(NotificationManagerCompat.from(context).areNotificationsEnabled()) {
            "通知已被系统关闭，不能启动 Computer Use"
        }
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.R) {
            check(projectionResultCode != null && projectionData != null) {
                "Android 10 及以下需要一次屏幕捕获授权"
            }
        }
        synchronized(grantLock) {
            this.grants.clear()
            grants
                .filterNot { ComputerUseSecurity.isHardBlockedPackage(it.packageName) }
                .forEach { this.grants[it.packageName] = it }
            if (includeSystemUi) {
                check(this.grants[SYSTEM_UI_PACKAGE]?.systemUi == true) {
                    "系统界面授权必须包含用户明确选择的权限等级"
                }
            }
            check(this.grants.isNotEmpty()) { "所选应用均属于不可控制界面" }
        }
        val persistedSelections = currentGrants().associate {
            it.packageName to it.tier
        }
        val configurationWithSelections = mutableConfiguration.value.copy(
            appSelections = persistedSelections,
        )
        settingsStore?.save(configurationWithSelections)
        mutableConfiguration.value = configurationWithSelections
        stopping = false
        val now = System.currentTimeMillis()
        lastInteractionAt.set(now)
        mutableState.value = ComputerUseUiState(
            serviceEnabled = true,
            sessionState = ComputerUseSessionState.Starting,
            captureMode = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
                ComputerUseCaptureMode.Accessibility
            } else {
                ComputerUseCaptureMode.MediaProjection
            },
            grants = currentGrants(),
            sessionStartedAtMs = now,
            expiresAtMs = now + ComputerUseSessionService.MAX_SESSION_MS,
        )
        val intent = Intent(context, ComputerUseSessionService::class.java)
            .setAction(ComputerUseSessionService.ACTION_START)
        projectionResultCode?.let {
            intent.putExtra(ComputerUseSessionService.EXTRA_PROJECTION_RESULT_CODE, it)
        }
        projectionData?.let {
            intent.putExtra(ComputerUseSessionService.EXTRA_PROJECTION_DATA, it)
        }
        ContextCompat.startForegroundService(context, intent)
    }.onFailure { error ->
        clearGrants()
        mutableState.value = mutableState.value.copy(
            sessionState = ComputerUseSessionState.Inactive,
            lastError = error.message,
        )
    }

    override fun stop(context: Context, reason: String) {
        if (
            stopping ||
            (
                mutableState.value.sessionState == ComputerUseSessionState.Inactive &&
                    !hasInMemoryGrants()
                )
        ) {
            return
        }
        stopping = true
        mutableState.value = mutableState.value.copy(
            sessionState = ComputerUseSessionState.Stopping,
            lastError = when (reason) {
                "user" -> null
                "security-violation" ->
                    mutableState.value.lastError ?: "检测到未授权或受保护界面，控制会话已停止"
                else -> stopReason(reason)
            },
        )
        approvalDeferred?.complete(false)
        approvalDeferred = null
        mutableApproval.value = null
        emergencyStop?.invoke()
        accessibility?.cancelPendingGestures()
        audioController?.stop()
        context.applicationContext.stopService(
            Intent(context, ComputerUseSessionService::class.java),
        )
        finishStoppedState(reason)
    }

    override fun resolveApproval(id: String, allowed: Boolean) {
        val approval = mutableApproval.value ?: return
        if (approval.id != id || System.currentTimeMillis() > approval.expiresAtMs) {
            approvalDeferred?.complete(false)
        } else {
            approvalDeferred?.complete(allowed)
        }
    }

    override fun clearAudit(context: Context) {
        attach(context, emergencyStop ?: {})
        audit.clear()
    }

    override fun updateConfiguration(
        context: Context,
        configuration: ComputerUseConfiguration,
    ) {
        attach(context, emergencyStop ?: {})
        val previous = mutableConfiguration.value
        val sanitized = configuration.copy(
            maxListenSeconds = configuration.maxListenSeconds.coerceIn(5, 60),
        )
        settingsStore?.save(sanitized)
        mutableConfiguration.value = sanitized
        if (
            previous.listenEnabled != sanitized.listenEnabled &&
            mutableState.value.sessionState in setOf(
                ComputerUseSessionState.Starting,
                ComputerUseSessionState.Active,
                ComputerUseSessionState.AwaitingApproval,
            )
        ) {
            runCatching { ComputerUseSessionService.refreshForegroundTypes(context) }
                .onFailure { error ->
                    microphoneForegroundReady = false
                    mutableState.value = mutableState.value.copy(
                        lastError = error.message
                            ?: "无法更新 Computer Use 麦克风前台服务状态",
                    )
                }
        }
    }

    override fun openAccessibilitySettings(context: Context) {
        context.startActivity(
            Intent(Settings.ACTION_ACCESSIBILITY_SETTINGS)
                .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK),
        )
    }

    override fun engineHost(): AndroidComputerUseHost = this

    internal fun onAccessibilityConnected(service: LingXiAccessibilityService) {
        accessibility = service
        refreshServiceStatus()
    }

    internal fun onAccessibilityDisconnected(
        service: LingXiAccessibilityService,
        reason: String,
    ) {
        if (accessibility !== service) return
        accessibility = null
        generation.incrementAndGet()
        if (mutableState.value.sessionState != ComputerUseSessionState.Inactive && !stopping) {
            applicationContext?.let { stop(it, "accessibility-disconnected") }
        }
        mutableState.value = mutableState.value.copy(serviceEnabled = false, lastError = reason)
    }

    internal fun onAccessibilityEvent(
        service: LingXiAccessibilityService,
        packageName: String?,
        eventType: Int,
    ) {
        if (accessibility !== service) {
            accessibility = service
            refreshServiceStatus()
        }
        generation.incrementAndGet()
        lastEventAt.set(System.currentTimeMillis())
        val observedPackage = packageName?.takeIf(String::isNotBlank) ?: return
        if (
            !ComputerUseSecurity.hasActiveAuthorization(
                mutableState.value.sessionState,
                hasInMemoryGrants(),
            )
        ) {
            return
        }

        // LingXi itself is needed to render one-shot confirmations and the stop
        // controls. It remains non-controllable through requireTarget(), but
        // merely foregrounding it must not tear down an otherwise valid session.
        if (observedPackage == applicationContext?.packageName) return

        // The visible software keyboard owns its AccessibilityEvent package.
        // Keep the app behind it as the active target; the IME never receives a
        // grant and therefore cannot be targeted through requireTarget().
        if (isCurrentInputMethodPackage(observedPackage)) {
            runCatching { currentSurfaceViolation() }
                .getOrNull()
                ?.let(::stopForSecurityViolation)
            return
        }

        mutableState.value = mutableState.value.copy(activePackage = observedPackage)
        when (
            ComputerUseSecurity.observedPackagePolicy(
                packageName = observedPackage,
                isAllowed = isPackageAllowed(observedPackage),
            )
        ) {
            ComputerUseObservedPackagePolicy.StopSession -> {
                stopForSecurityViolation("控制目标切换到了受保护应用 $observedPackage")
                return
            }
            ComputerUseObservedPackagePolicy.BlockActions -> return
            ComputerUseObservedPackagePolicy.Allowed -> Unit
        }

        if (
            eventType == AccessibilityEvent.TYPE_WINDOW_STATE_CHANGED ||
            eventType == AccessibilityEvent.TYPE_WINDOWS_CHANGED ||
            eventType == AccessibilityEvent.TYPE_WINDOW_CONTENT_CHANGED
        ) {
            runCatching { currentSurfaceViolation() }
                .getOrNull()
                ?.let(::stopForSecurityViolation)
        }
    }

    internal fun onServiceStarted(capture: MediaProjectionCapture?) {
        projectionCapture = capture
        val now = System.currentTimeMillis()
        mutableState.value = mutableState.value.copy(
            sessionState = ComputerUseSessionState.Active,
            captureMode = if (capture == null) {
                ComputerUseCaptureMode.Accessibility
            } else {
                ComputerUseCaptureMode.MediaProjection
            },
            sessionStartedAtMs = mutableState.value.sessionStartedAtMs ?: now,
            expiresAtMs = (mutableState.value.sessionStartedAtMs ?: now) +
                ComputerUseSessionService.MAX_SESSION_MS,
            lastError = null,
        )
    }

    internal fun onServiceDestroyed() {
        projectionCapture = null
        microphoneForegroundReady = false
        val wasActive = mutableState.value.sessionState != ComputerUseSessionState.Inactive
        if (!stopping && wasActive) {
            emergencyStop?.invoke()
        }
        if (wasActive) finishStoppedState("service-destroyed")
    }

    internal fun failAndStop(context: Context, message: String) {
        mutableState.value = mutableState.value.copy(lastError = message)
        stop(context, "service-error")
    }

    internal fun hasInMemoryGrants(): Boolean = synchronized(grantLock) { grants.isNotEmpty() }

    internal fun isListenConfigured(): Boolean = mutableConfiguration.value.listenEnabled

    internal fun onForegroundTypesUpdated(microphoneReady: Boolean) {
        microphoneForegroundReady = microphoneReady
    }

    internal fun lastInteractionMs(): Long = lastInteractionAt.get()

    override suspend fun statusJson(): String {
        refreshServiceStatus()
        val current = mutableState.value
        val audioConfiguration = applicationContext
            ?.let { AudioConfigurationRepository(it).load().snapshot.configuration }
        return JSONObject()
            .put("service_enabled", current.serviceEnabled)
            .put("session_state", current.sessionState.name.toSnakeCase())
            .put("capture_mode", current.captureMode.name.toSnakeCase())
            .putNullable("active_package", current.activePackage)
            .put("display_width", displaySize().first)
            .put("display_height", displaySize().second)
            .putNullable(
                "remaining_ms",
                current.expiresAtMs?.minus(System.currentTimeMillis())?.coerceAtLeast(0),
            )
            .putNullable("detail", current.lastError)
            .put("audio_listen_enabled", mutableConfiguration.value.listenEnabled)
            .put("audio_microphone_foreground_ready", microphoneForegroundReady)
            .put("audio_speak_enabled", mutableConfiguration.value.speakEnabled)
            .putNullable(
                "audio_input_language",
                audioConfiguration?.language,
            )
            .putNullable(
                "audio_voice",
                audioConfiguration?.speech?.voice?.settingsKey(),
            )
            .put(
                "audio_speed",
                audioConfiguration?.rate ?: 1.0,
            )
            .toString()
    }

    override suspend fun requestAccessJson(requestJson: String): String {
        requireActive()
        val request = JSONObject(requestJson)
        val requestedTier = request.optString("tier").toTier()
        val requested = request.optJSONArray("apps").orEmptyStrings()
        val includeSystemUi = request.optBoolean("include_system_ui", false)
        val current = currentGrants()
        val appsAllowed = requested.all { packageName ->
            current.any { grant ->
                grant.packageName == packageName && grant.tier.ordinal >= requestedTier.ordinal
            }
        }
        val systemUiAllowed = !includeSystemUi || current.any { grant ->
            grant.systemUi && grant.tier.ordinal >= requestedTier.ordinal
        }
        if (!appsAllowed || !systemUiAllowed) {
            throw AndroidComputerUseFfiException.PermissionDenied(
                "请在灵犀 Computer Use 设置页由用户选择应用和权限等级",
            )
        }
        return grantedAppsJson(current)
    }

    override suspend fun listGrantedAppsJson(): String = grantedAppsJson(currentGrants())

    override suspend fun screenshot(): AndroidScreenshotFfi = operationMutex.withLock {
        requireReadAccess()
        assertCurrentSurfaceSafe()
        val capture = runCatching {
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
                requireService().captureWithAccessibility()
            } else {
                projectionCapture?.capturePng()
                    ?: throw AndroidComputerUseFfiException.SessionInactive()
            }
        }.getOrElse { error ->
            if (error is AndroidComputerUseFfiException) throw error
            throw AndroidComputerUseFfiException.ProtectedSurface(
                "当前窗口拒绝屏幕捕获，可能设置了 FLAG_SECURE",
            )
        }
        touch()
        return AndroidScreenshotFfi(
            width = capture.width.toUInt(),
            height = capture.height.toUInt(),
            pngBytes = capture.pngBytes,
        )
    }

    override suspend fun uiTreeJson(): String = operationMutex.withLock {
        snapshot().json.toString()
    }

    override suspend fun findNodesJson(queryJson: String): String = operationMutex.withLock {
        val query = JSONObject(queryJson)
        val result = snapshot().nodes.filter { node ->
            query.matches(node.json)
        }.take(query.optInt("limit", 20).coerceIn(1, 100))
        JSONArray(result.map(NodeSnapshot::json)).toString()
    }

    override suspend fun inspectNodeJson(nodeId: String): String = operationMutex.withLock {
        val snapshot = snapshot()
        snapshot.nodes.firstOrNull { it.token == nodeId }?.json?.toString()
            ?: throw AndroidComputerUseFfiException.StaleNode("节点已过期，请重新读取 ui_tree")
    }

    override suspend fun performJson(actionJson: String): String = withContext(Dispatchers.Default) {
        operationMutex.withLock {
            val action = JSONObject(actionJson)
            performAction(action).toString()
        }
    }

    override suspend fun waitForJson(conditionJson: String, timeoutMs: ULong): String {
        val timeout = timeoutMs.toLong().coerceIn(1, 30_000)
        val condition = JSONObject(conditionJson)
        return try {
            withTimeout(timeout) {
                while (true) {
                    val met = operationMutex.withLock {
                        requireActive()
                        when (condition.getString("type")) {
                            "node_appears" -> snapshot().nodes.any {
                                condition.getJSONObject("query").matches(it.json)
                            }
                            "node_disappears" -> snapshot().nodes.none {
                                condition.getJSONObject("query").matches(it.json)
                            }
                            "activity" -> {
                                val currentPackage = currentPackage()
                                currentPackage == condition.getString("package_name")
                            }
                            "idle" -> {
                                val quiet = condition.optLong("quiet_ms", 500).coerceIn(50, 5_000)
                                System.currentTimeMillis() - lastEventAt.get() >= quiet
                            }
                            else -> throw AndroidComputerUseFfiException.Unsupported(
                                "未知等待条件",
                            )
                        }
                    }
                    if (met) {
                        return@withTimeout actionResult(true, "条件已满足").toString()
                    }
                    delay(50)
                }
                @Suppress("UNREACHABLE_CODE")
                ""
            }
        } catch (_: kotlinx.coroutines.TimeoutCancellationException) {
            throw AndroidComputerUseFfiException.Timeout("等待条件超时")
        }
    }

    override suspend fun listenJson(requestJson: String): String {
        requireActive()
        val config = mutableConfiguration.value
        if (!config.listenEnabled) {
            throw AndroidComputerUseFfiException.PermissionDenied(
                "请先在 Computer Use 设置中启用“允许听取环境语音”",
            )
        }
        val context = applicationContext
            ?: throw AndroidComputerUseFfiException.SessionInactive()
        if (
            ContextCompat.checkSelfPermission(context, Manifest.permission.RECORD_AUDIO) !=
            PackageManager.PERMISSION_GRANTED
        ) {
            throw AndroidComputerUseFfiException.PermissionDenied(
                "麦克风权限未授予；请先在灵犀前台允许麦克风权限",
            )
        }
        if (!microphoneForegroundReady) {
            throw AndroidComputerUseFfiException.PermissionDenied(
                "麦克风前台服务尚未就绪；请在灵犀前台重新启用听取权限",
            )
        }
        val request = JSONObject(requestJson)
        val voiceConfig = AudioConfigurationRepository(context).load().snapshot.configuration
        val language = request.optString("language")
            .takeIf { it.isNotBlank() && it != "null" }
            ?: voiceConfig.language.takeUnless { it.equals("auto", ignoreCase = true) }
        val timeoutMs = request.optLong(
            "timeout_ms",
            config.maxListenSeconds * 1_000L,
        ).coerceIn(1_000L, config.maxListenSeconds * 1_000L)
        return try {
            val result = requireNotNull(audioController) {
                "Computer Use audio controller is unavailable"
            }.listen(language, timeoutMs)
            touch()
            audit.append(
                "device_audio",
                "listen",
                ComputerUseRisk.Normal,
                "session-setting",
                "completed",
            )
            JSONObject()
                .put("text", result.text)
                .putNullable("language", result.language)
                .putNullable("confidence", result.confidence)
                .put("duration_ms", result.durationMs)
                .toString()
        } catch (_: TimeoutCancellationException) {
            throw AndroidComputerUseFfiException.Timeout("语音听取超时")
        } catch (error: CancellationException) {
            throw error
        } catch (error: AndroidComputerUseFfiException) {
            throw error
        } catch (error: AudioOperationException) {
            throw error.toComputerUseAudioFfiException()
        } catch (error: Throwable) {
            throw AndroidComputerUseFfiException.Other(
                error.message ?: "语音听取失败",
            )
        }
    }

    override suspend fun speakJson(requestJson: String): String {
        requireActive()
        if (!mutableConfiguration.value.speakEnabled) {
            throw AndroidComputerUseFfiException.PermissionDenied(
                "请先在 Computer Use 设置中启用“允许语音播报”",
            )
        }
        val context = applicationContext
            ?: throw AndroidComputerUseFfiException.SessionInactive()
        val request = JSONObject(requestJson)
        val text = request.optString("text")
        if (text.isBlank() || text.length > 4_000) {
            throw AndroidComputerUseFfiException.Other("语音播报文本长度必须为 1–4000 字符")
        }
        val voiceConfig = AudioConfigurationRepository(context).load().snapshot.configuration
        val voice = request.optString("voice")
            .takeIf { it.isNotBlank() && it != "null" }
            ?: voiceConfig.speech.voice?.settingsKey()
        val speed = if (request.has("speed") && !request.isNull("speed")) {
            request.optDouble("speed", voiceConfig.rate).toFloat()
        } else {
            voiceConfig.rate.toFloat()
        }.coerceIn(0.5f, 2.0f)
        return try {
            val result = requireNotNull(audioController) {
                "Computer Use audio controller is unavailable"
            }.speak(text, voice, speed)
            touch()
            audit.append(
                "device_audio",
                "speak",
                ComputerUseRisk.Normal,
                "session-setting",
                "completed",
            )
            JSONObject()
                .put("completed", result.completed)
                .put("duration_ms", result.durationMs)
                .toString()
        } catch (_: TimeoutCancellationException) {
            throw AndroidComputerUseFfiException.Timeout("语音播报超时")
        } catch (error: CancellationException) {
            throw error
        } catch (error: AndroidComputerUseFfiException) {
            throw error
        } catch (error: AudioOperationException) {
            throw error.toComputerUseAudioFfiException()
        } catch (error: Throwable) {
            throw AndroidComputerUseFfiException.Other(
                error.message ?: "语音播报失败",
            )
        }
    }

    override suspend fun stopAudio() {
        requireActive()
        try {
            audioController?.stopAndWait()
        } catch (error: CancellationException) {
            throw error
        } catch (error: AndroidComputerUseFfiException) {
            throw error
        } catch (error: AudioOperationException) {
            throw error.toComputerUseAudioFfiException()
        } catch (error: Throwable) {
            throw AndroidComputerUseFfiException.Other(error.message ?: "音频停止失败")
        }
    }

    override suspend fun stop() {
        val context = applicationContext
            ?: throw AndroidComputerUseFfiException.SessionInactive()
        stop(context, "tool-stop")
    }

    private suspend fun performAction(action: JSONObject): JSONObject {
        requireActive()
        val actionType = action.getString("type")
        val requiredTier = ComputerUseSecurity.requiredTier(actionType)
        val targetPackage = when {
            actionType == "open_app" -> action.getString("package_name")
            actionType == "global" && action.optString("action") in SYSTEM_UI_GLOBAL_ACTIONS ->
                SYSTEM_UI_PACKAGE
            else -> currentPackage()
        }
        requireTarget(targetPackage, requiredTier)
        val originalPackage = currentPackage()
        val currentSnapshot = if (
            actionType == "open_app" ||
            (actionType == "global" && targetPackage == SYSTEM_UI_PACKAGE)
        ) {
            TreeSnapshot(JSONObject(), mutableListOf(), false)
        } else {
            snapshot()
        }
        val nodeSnapshot = action.optString("node_id")
            .takeIf(String::isNotBlank)
            ?.let { id -> currentSnapshot.nodes.firstOrNull { it.token == id } }
            ?: action.coordinateTarget(currentSnapshot.nodes)
        if (action.has("node_id") && action.optString("node_id").isNotBlank() && nodeSnapshot == null) {
            throw AndroidComputerUseFfiException.StaleNode("节点已过期，请重新读取 ui_tree")
        }
        val summary = buildString {
            append(nodeSnapshot?.summary.orEmpty())
            append(' ')
            append(action.optString("package_name"))
        }
        val semanticsReliable = nodeSnapshot?.summary
            ?.takeUnless { it.isBlank() || it == "[REDACTED]" } != null
        val risk = ComputerUseSecurity.riskFor(
            actionType,
            summary,
            semanticsReliable = semanticsReliable,
        )
        if (risk == ComputerUseRisk.Blocked) {
            audit.append(targetPackage, actionType, risk, "not-allowed", "blocked")
            throw AndroidComputerUseFfiException.ProtectedSurface("该操作位于不可解除禁止的安全界面")
        }
        var confirmation = "not-required"
        var executableNode = nodeSnapshot
        if (risk == ComputerUseRisk.ConfirmEveryTime) {
            val allowed = awaitHighRiskApproval(targetPackage, actionType, summary)
            confirmation = if (allowed) "allowed" else "denied"
            if (!allowed) {
                audit.append(targetPackage, actionType, risk, confirmation, "blocked")
                throw AndroidComputerUseFfiException.PermissionDenied("用户未确认高风险操作")
            }
            restoreTargetPackage(targetPackage)
            requireActive()
            if (
                actionType != "open_app" &&
                !(actionType == "global" && targetPackage == SYSTEM_UI_PACKAGE)
            ) {
                executableNode = rebindApprovedNode(
                    action = action,
                    original = nodeSnapshot,
                    refreshed = snapshot().nodes,
                )
            }
        }
        val success = executeAction(action, executableNode)
        val afterPackage = currentPackage()
        if (
            actionType != "open_app" &&
            afterPackage != originalPackage &&
            afterPackage != applicationContext?.packageName
        ) {
            when (
                ComputerUseSecurity.observedPackagePolicy(
                    packageName = afterPackage,
                    isAllowed = isPackageAllowed(afterPackage),
                )
            ) {
                ComputerUseObservedPackagePolicy.StopSession -> {
                    audit.append(targetPackage, actionType, risk, confirmation, "protected-target")
                    stopForSecurityViolation("操作后跳入受保护应用 $afterPackage")
                    throw AndroidComputerUseFfiException.ProtectedSurface(
                        "操作后跳入受保护应用 $afterPackage",
                    )
                }
                ComputerUseObservedPackagePolicy.BlockActions -> {
                    audit.append(targetPackage, actionType, risk, confirmation, "target-changed")
                    throw AndroidComputerUseFfiException.TargetNotAllowed(
                        "操作后跳入未授权应用 $afterPackage",
                    )
                }
                ComputerUseObservedPackagePolicy.Allowed -> Unit
            }
        }
        touch()
        audit.append(
            targetPackage,
            actionType,
            risk,
            confirmation,
            if (success) "success" else "failed",
        )
        return actionResult(success, if (success) "操作已执行" else "Android 拒绝了该操作")
    }

    private suspend fun executeAction(
        action: JSONObject,
        nodeSnapshot: NodeSnapshot?,
    ): Boolean {
        requireActive()
        val service = requireService()
        val type = action.getString("type")
        val node = nodeSnapshot?.let { resolveNode(it.ref) }
        return try {
            when (type) {
                "tap" -> {
                    if (node?.performAction(AccessibilityNodeInfo.ACTION_CLICK) == true) {
                        true
                    } else {
                        val (x, y) = coordinates(action, node)
                        service.tap(x, y, 50)
                    }
                }
                "long_press" -> {
                    if (node?.performAction(AccessibilityNodeInfo.ACTION_LONG_CLICK) == true) {
                        true
                    } else {
                        val (x, y) = coordinates(action, node)
                        service.tap(x, y, action.optLong("duration_ms", 600))
                    }
                }
                "set_text" -> setEditableText(node, action.getString("text"))
                "clear_text" -> setEditableText(node, "")
                "global" -> performGlobal(service, action.getString("action"))
                "enter" -> performImeEnter(node)
                "direction" -> performDirection(service, action.getString("direction"))
                "scroll" -> performScroll(service, action, node)
                "swipe" -> service.swipe(
                    action.getDouble("start_x").toFloat(),
                    action.getDouble("start_y").toFloat(),
                    action.getDouble("end_x").toFloat(),
                    action.getDouble("end_y").toFloat(),
                    action.optLong("duration_ms", 350),
                )
                "pinch" -> service.pinch(
                    action.getDouble("center_x").toFloat(),
                    action.getDouble("center_y").toFloat(),
                    action.getDouble("scale").toFloat(),
                    action.optLong("duration_ms", 500),
                )
                "open_app" -> openAllowedApp(action.getString("package_name"))
                else -> throw AndroidComputerUseFfiException.Unsupported("不支持的动作 $type")
            }
        } finally {
            node?.recycle()
        }
    }

    private fun setEditableText(node: AccessibilityNodeInfo?, text: String): Boolean {
        val target = node ?: focusedEditable() ?: return false
        return try {
            val args = Bundle().apply {
                putCharSequence(
                    AccessibilityNodeInfo.ACTION_ARGUMENT_SET_TEXT_CHARSEQUENCE,
                    text,
                )
            }
            target.performAction(AccessibilityNodeInfo.ACTION_SET_TEXT, args)
        } finally {
            if (target !== node) target.recycle()
        }
    }

    private fun performGlobal(service: LingXiAccessibilityService, action: String): Boolean {
        val global = when (action) {
            "back" -> AccessibilityService.GLOBAL_ACTION_BACK
            "home" -> AccessibilityService.GLOBAL_ACTION_HOME
            "recents" -> AccessibilityService.GLOBAL_ACTION_RECENTS
            "notifications" -> AccessibilityService.GLOBAL_ACTION_NOTIFICATIONS
            "quick_settings" -> AccessibilityService.GLOBAL_ACTION_QUICK_SETTINGS
            else -> throw AndroidComputerUseFfiException.Unsupported("不支持的全局按键")
        }
        return service.performGlobalAction(global)
    }

    private fun performImeEnter(node: AccessibilityNodeInfo?): Boolean {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.R) {
            throw AndroidComputerUseFfiException.Unsupported(
                "Android 11 以下无法通过无障碍 API 可靠执行 Enter",
            )
        }
        val target = node ?: focusedEditable()
            ?: throw AndroidComputerUseFfiException.Unsupported("当前没有可执行 Enter 的输入框")
        return try {
            val imeEnter = AccessibilityNodeInfo.AccessibilityAction.ACTION_IME_ENTER
            if (target.actionList.none { it.id == imeEnter.id }) {
                throw AndroidComputerUseFfiException.Unsupported(
                    "当前输入框没有暴露 IME Enter 动作",
                )
            }
            target.performAction(imeEnter.id)
        } finally {
            if (target !== node) target.recycle()
        }
    }

    private fun performDirection(service: LingXiAccessibilityService, direction: String): Boolean {
        val root = service.activeRoot() ?: return false
        return try {
            val focusDirection = when (direction) {
                "up" -> View.FOCUS_UP
                "down" -> View.FOCUS_DOWN
                "left" -> View.FOCUS_LEFT
                "right" -> View.FOCUS_RIGHT
                else -> throw AndroidComputerUseFfiException.Unsupported("不支持的方向键")
            }
            val focused = root.findFocus(AccessibilityNodeInfo.FOCUS_INPUT) ?: return false
            try {
                val target = focused.focusSearch(focusDirection) ?: return false
                try {
                    target.performAction(AccessibilityNodeInfo.ACTION_FOCUS)
                } finally {
                    target.recycle()
                }
            } finally {
                focused.recycle()
            }
        } finally {
            root.recycle()
        }
    }

    private suspend fun performScroll(
        service: LingXiAccessibilityService,
        action: JSONObject,
        node: AccessibilityNodeInfo?,
    ): Boolean {
        val direction = action.optString("direction", "down")
        val target = node ?: findScrollable()
        if (target != null) {
            return try {
                val actionCode = if (direction in setOf("down", "right")) {
                    AccessibilityNodeInfo.ACTION_SCROLL_FORWARD
                } else {
                    AccessibilityNodeInfo.ACTION_SCROLL_BACKWARD
                }
                var success = true
                repeat(action.optInt("amount", 1).coerceIn(1, 10)) {
                    success = target.performAction(actionCode) && success
                }
                success
            } finally {
                if (target !== node) target.recycle()
            }
        }
        val (width, height) = displaySize()
        val x = action.optDouble("x", width / 2.0).toFloat()
        val y = action.optDouble("y", height / 2.0).toFloat()
        val delta = height * 0.35f
        val endY = if (direction == "down") y - delta else y + delta
        return service.swipe(x, y, x, endY.coerceIn(0f, height.toFloat()), 350)
    }

    private suspend fun openAllowedApp(packageName: String): Boolean {
        requireTarget(packageName, ComputerUseTier.Full)
        val context = checkNotNull(applicationContext)
        val intent = context.packageManager.getLaunchIntentForPackage(packageName)
            ?: throw AndroidComputerUseFfiException.Unsupported("应用不可启动")
        context.startActivity(intent.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
        try {
            withTimeout(5_000) {
                while (true) {
                    requireActive()
                    val foregroundPackage = currentPackage()
                    if (
                        foregroundPackage == packageName ||
                        isSystemUi(foregroundPackage, packageName)
                    ) {
                        break
                    }
                    if (foregroundPackage.isNotBlank() && foregroundPackage != context.packageName) {
                        when (
                            ComputerUseSecurity.observedPackagePolicy(
                                packageName = foregroundPackage,
                                isAllowed = isPackageAllowed(foregroundPackage),
                            )
                        ) {
                            ComputerUseObservedPackagePolicy.StopSession -> {
                                stopForSecurityViolation(
                                    "启动 $packageName 时跳入受保护应用 $foregroundPackage",
                                )
                                throw AndroidComputerUseFfiException.ProtectedSurface(
                                    foregroundPackage,
                                )
                            }
                            // OEM overlays can briefly own the active window while
                            // the selected app is launching. Keep waiting, but every
                            // action remains blocked until the selected app is active.
                            ComputerUseObservedPackagePolicy.BlockActions,
                            ComputerUseObservedPackagePolicy.Allowed,
                            -> Unit
                        }
                    }
                    delay(50)
                }
            }
        } catch (_: kotlinx.coroutines.TimeoutCancellationException) {
            val foregroundPackage = currentPackage()
            if (
                foregroundPackage.isNotBlank() &&
                foregroundPackage != context.packageName &&
                !isPackageAllowed(foregroundPackage)
            ) {
                throw AndroidComputerUseFfiException.TargetNotAllowed(foregroundPackage)
            }
            throw AndroidComputerUseFfiException.Timeout("等待 $packageName 进入前台超时")
        }
        assertCurrentSurfaceSafe()
        return true
    }

    private suspend fun awaitHighRiskApproval(
        packageName: String,
        action: String,
        summary: String,
    ): Boolean {
        val context = checkNotNull(applicationContext)
        val id = UUID.randomUUID().toString()
        val deferred = CompletableDeferred<Boolean>()
        approvalDeferred?.complete(false)
        approvalDeferred = deferred
        mutableApproval.value = ComputerUseApproval(
            id = id,
            targetPackage = packageName,
            action = action,
            summary = safeApprovalSummary(action, summary),
            expiresAtMs = System.currentTimeMillis() + APPROVAL_TIMEOUT_MS,
        )
        mutableState.value = mutableState.value.copy(
            sessionState = ComputerUseSessionState.AwaitingApproval,
        )
        postApprovalNotification(context)
        return try {
            withTimeout(APPROVAL_TIMEOUT_MS) { deferred.await() }
        } catch (_: kotlinx.coroutines.TimeoutCancellationException) {
            false
        } finally {
            approvalDeferred = null
            mutableApproval.value = null
            NotificationManagerCompat.from(context).cancel(APPROVAL_NOTIFICATION_ID)
            if (mutableState.value.sessionState == ComputerUseSessionState.AwaitingApproval) {
                mutableState.value = mutableState.value.copy(
                    sessionState = ComputerUseSessionState.Active,
                )
            }
        }
    }

    private fun postApprovalNotification(context: Context) {
        if (
            Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU &&
            ContextCompat.checkSelfPermission(
                context,
                Manifest.permission.POST_NOTIFICATIONS,
            ) != PackageManager.PERMISSION_GRANTED
        ) {
            return
        }
        val notificationManager = NotificationManagerCompat.from(context)
        if (!notificationManager.areNotificationsEnabled()) return
        val intent = Intent(context, MainActivity::class.java)
            .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_SINGLE_TOP)
        val pending = android.app.PendingIntent.getActivity(
            context,
            77,
            intent,
            android.app.PendingIntent.FLAG_UPDATE_CURRENT or android.app.PendingIntent.FLAG_IMMUTABLE,
        )
        val notification = NotificationCompat.Builder(context, "computer_use_session")
            .setSmallIcon(R.mipmap.ic_launcher_foreground)
            .setContentTitle("Computer Use 操作需要确认")
            .setContentText("返回灵犀查看目标应用和操作影响")
            .setPriority(NotificationCompat.PRIORITY_HIGH)
            .setAutoCancel(true)
            .setContentIntent(pending)
            .build()
        try {
            notificationManager.notify(APPROVAL_NOTIFICATION_ID, notification)
        } catch (_: SecurityException) {
            // Permission can be revoked between the check and notify call.
        }
    }

    private suspend fun restoreTargetPackage(packageName: String) {
        if (currentPackage() == packageName) return
        if (packageName != SYSTEM_UI_PACKAGE) {
            openAllowedApp(packageName)
            return
        }
        requireService().performGlobalAction(AccessibilityService.GLOBAL_ACTION_HOME)
        try {
            withTimeout(5_000) {
                while (!isSystemUi(currentPackage(), packageName)) {
                    requireActive()
                    delay(50)
                }
            }
        } catch (_: kotlinx.coroutines.TimeoutCancellationException) {
            throw AndroidComputerUseFfiException.Timeout("等待系统界面进入前台超时")
        }
        assertCurrentSurfaceSafe()
    }

    private fun snapshot(): TreeSnapshot {
        requireReadAccess()
        val service = requireService()
        val root = service.activeRoot()
            ?: throw AndroidComputerUseFfiException.Other("当前窗口没有可访问的 UI 树")
        val packageName = root.packageName?.toString().orEmpty()
        requireTarget(packageName, ComputerUseTier.Read)
        val surfaceScan = scanSurface(root)
        if (
            ComputerUseSecurity.isHardBlockedSurface(
                packageName = packageName,
                visibleText = surfaceScan.visibleText,
                containsSensitiveNode = surfaceScan.containsSensitiveNode,
                scanTruncated = surfaceScan.truncated,
            )
        ) {
            root.recycle()
            stopForSecurityViolation("当前界面包含密码、支付、系统授权或无法完整检查的内容")
            throw AndroidComputerUseFfiException.ProtectedSurface("当前界面包含密码、支付或系统授权内容")
        }
        val currentGeneration = generation.get()
        val windowId = root.windowId
        val nodes = mutableListOf<NodeSnapshot>()
        var truncated = false
        fun walk(
            node: AccessibilityNodeInfo,
            path: IntArray,
            parentToken: String?,
            depth: Int,
        ) {
            if (nodes.size >= MAX_NODES || depth > MAX_DEPTH) {
                truncated = true
                return
            }
            val ref = NodeRef(currentGeneration, windowId, path)
            val token = encodeNodeRef(ref)
            val bounds = Rect().also(node::getBoundsInScreen)
            val sensitive = ComputerUseSecurity.isSensitiveNode(node)
            val item = JSONObject()
                .put("node_id", token)
                .putNullable("parent_id", parentToken)
                .put("package_name", node.packageName?.toString().orEmpty())
                .put("class_name", node.className?.toString().orEmpty())
                .putNullable("resource_id", node.viewIdResourceName)
                .putNullable("text", if (sensitive) "[REDACTED]" else node.text?.toString())
                .putNullable(
                    "content_description",
                    if (sensitive) "[REDACTED]" else node.contentDescription?.toString(),
                )
                .put(
                    "bounds",
                    JSONObject()
                        .put("left", bounds.left)
                        .put("top", bounds.top)
                        .put("right", bounds.right)
                        .put("bottom", bounds.bottom),
                )
                .put("clickable", node.isClickable)
                .put("long_clickable", node.isLongClickable)
                .put("scrollable", node.isScrollable)
                .put("editable", node.isEditable)
                .put("enabled", node.isEnabled)
                .put("selected", node.isSelected)
                .putNullable("checked", if (node.isCheckable) node.isChecked else null)
                .put("password", node.isPassword)
            nodes += NodeSnapshot(
                token = token,
                ref = ref,
                json = item,
                summary = if (sensitive) "[REDACTED]" else listOfNotNull(
                    node.text?.toString(),
                    node.contentDescription?.toString(),
                    node.viewIdResourceName,
                ).joinToString(" "),
            )
            for (index in 0 until node.childCount) {
                if (nodes.size >= MAX_NODES) {
                    truncated = true
                    break
                }
                val child = node.getChild(index) ?: continue
                try {
                    walk(child, path + index, token, depth + 1)
                } finally {
                    child.recycle()
                }
            }
        }
        try {
            walk(root, intArrayOf(), null, 0)
        } finally {
            root.recycle()
        }
        val json = JSONObject()
            .put("package_name", packageName)
            .putNullable("activity_name", null)
            .put("window_id", windowId)
            .put("generation", currentGeneration)
            .put("captured_at_ms", System.currentTimeMillis())
            .put("nodes", JSONArray(nodes.map(NodeSnapshot::json)))
            .put("truncated", truncated)
        if (json.toString().toByteArray().size > MAX_TREE_BYTES) {
            truncated = true
            while (
                nodes.isNotEmpty() &&
                JSONObject(json.toString())
                    .put("nodes", JSONArray(nodes.map(NodeSnapshot::json)))
                    .toString()
                    .toByteArray()
                    .size > MAX_TREE_BYTES
            ) {
                nodes.removeAt(nodes.lastIndex)
            }
            json.put("nodes", JSONArray(nodes.map(NodeSnapshot::json)))
            json.put("truncated", true)
        }
        touch()
        return TreeSnapshot(json, nodes, truncated)
    }

    private fun resolveNode(ref: NodeRef): AccessibilityNodeInfo? {
        if (ref.generation != generation.get()) {
            throw AndroidComputerUseFfiException.StaleNode("窗口内容已变化")
        }
        var node = requireService().activeRoot() ?: return null
        if (node.windowId != ref.windowId) {
            node.recycle()
            throw AndroidComputerUseFfiException.StaleNode("窗口已变化")
        }
        for (index in ref.path) {
            val child = node.getChild(index)
            node.recycle()
            node = child ?: return null
        }
        return node
    }

    private fun focusedEditable(): AccessibilityNodeInfo? {
        val root = requireService().activeRoot() ?: return null
        return try {
            root.findFocus(AccessibilityNodeInfo.FOCUS_INPUT)
        } finally {
            root.recycle()
        }
    }

    private fun findScrollable(): AccessibilityNodeInfo? {
        val root = requireService().activeRoot() ?: return null
        val queue = ArrayDeque<AccessibilityNodeInfo>()
        queue.add(root)
        while (queue.isNotEmpty()) {
            val node = queue.removeFirst()
            if (node.isScrollable) {
                queue.forEach(AccessibilityNodeInfo::recycle)
                return node
            }
            for (index in 0 until node.childCount) {
                node.getChild(index)?.let(queue::add)
            }
            node.recycle()
        }
        return null
    }

    private fun coordinates(
        action: JSONObject,
        node: AccessibilityNodeInfo?,
    ): Pair<Float, Float> {
        if (action.has("x") && action.has("y") && !action.isNull("x") && !action.isNull("y")) {
            return action.getDouble("x").toFloat() to action.getDouble("y").toFloat()
        }
        val target = node
            ?: throw AndroidComputerUseFfiException.Other("动作缺少节点或坐标")
        val bounds = Rect().also(target::getBoundsInScreen)
        return bounds.exactCenterX() to bounds.exactCenterY()
    }

    private fun requireActive() {
        if (
            !ComputerUseSecurity.hasActiveAuthorization(
                mutableState.value.sessionState,
                hasInMemoryGrants(),
            )
        ) {
            throw AndroidComputerUseFfiException.SessionInactive()
        }
        requireService()
    }

    private fun requireService(): LingXiAccessibilityService =
        accessibility ?: throw AndroidComputerUseFfiException.ServiceDisabled()

    private fun requireReadAccess() {
        requireActive()
        requireTarget(currentPackage(), ComputerUseTier.Read)
    }

    private fun requireTarget(packageName: String, tier: ComputerUseTier) {
        if (ComputerUseSecurity.isHardBlockedPackage(packageName)) {
            throw AndroidComputerUseFfiException.ProtectedSurface("不可控制灵犀授权、密钥或系统安全页面")
        }
        val grant = synchronized(grantLock) {
            grants[packageName] ?: if (isSystemSurfacePackage(packageName)) {
                grants[SYSTEM_UI_PACKAGE]
            } else {
                null
            }
        } ?: throw AndroidComputerUseFfiException.TargetNotAllowed(packageName)
        if (grant.tier.ordinal < tier.ordinal) {
            throw AndroidComputerUseFfiException.TierInsufficient(
                "$packageName 需要 ${tier.name} 权限",
            )
        }
    }

    private fun isPackageAllowed(packageName: String): Boolean = synchronized(grantLock) {
        grants.containsKey(packageName) ||
            (isSystemSurfacePackage(packageName) && grants.containsKey(SYSTEM_UI_PACKAGE))
    }

    private fun isCurrentInputMethodPackage(packageName: String): Boolean {
        val context = applicationContext ?: return false
        val configuredInputMethod = Settings.Secure.getString(
            context.contentResolver,
            Settings.Secure.DEFAULT_INPUT_METHOD,
        )
        return ComputerUseSecurity.isConfiguredInputMethodPackage(
            packageName,
            configuredInputMethod,
        )
    }

    private fun currentPackage(): String =
        requireService().activeRoot()?.let { root ->
            try {
                root.packageName?.toString().orEmpty()
            } finally {
                root.recycle()
            }
        }.orEmpty().ifBlank {
            mutableState.value.activePackage.orEmpty()
        }

    private fun currentGrants(): List<ComputerUseGrant> = synchronized(grantLock) {
        grants.values.toList()
    }

    private fun clearGrants() = synchronized(grantLock) {
        grants.clear()
    }

    private fun refreshServiceStatus() {
        val serviceEnabled = accessibility != null
        val lastError = mutableState.value.lastError
        mutableState.value = mutableState.value.copy(
            serviceEnabled = serviceEnabled,
            lastError = lastError.takeUnless {
                serviceEnabled && it in ACCESSIBILITY_DISCONNECT_ERRORS
            },
        )
    }

    private fun finishStoppedState(reason: String) {
        audioController?.stop()
        microphoneForegroundReady = false
        clearGrants()
        projectionCapture = null
        val error = mutableState.value.lastError ?: if (reason in setOf("user", "tool-stop")) {
            null
        } else {
            stopReason(reason)
        }
        mutableState.value = ComputerUseUiState(
            serviceEnabled = accessibility != null,
            lastError = error,
        )
        stopping = false
    }

    private fun stopReason(reason: String): String = when (reason) {
        "screen-locked" -> "设备锁屏，控制会话已停止"
        "idle-timeout" -> "30 分钟无操作，控制会话已停止"
        "maximum-duration" -> "控制会话已达到 2 小时上限"
        "accessibility-disconnected" -> "无障碍服务断开，控制会话已停止"
        "media-projection-stopped" -> "屏幕捕获授权已结束"
        "service-destroyed" -> "控制服务已结束"
        "service-error" -> mutableState.value.lastError ?: "控制服务启动失败"
        else -> "Computer Use 已停止"
    }

    private fun touch() {
        lastInteractionAt.set(System.currentTimeMillis())
    }

    private fun grantedAppsJson(items: List<ComputerUseGrant>): String = JSONArray(
        items.map { grant ->
            JSONObject()
                .put("package_name", grant.packageName)
                .put("display_name", grant.label)
                .put("tier", grant.tier.name.toSnakeCase())
                .put("system_ui", grant.systemUi)
        },
    ).toString()

    private fun actionResult(success: Boolean, detail: String): JSONObject = JSONObject()
        .put("success", success)
        .putNullable("package_name", currentPackage().ifBlank { null })
        .put("generation", generation.get())
        .put("detail", detail)

    private fun encodeNodeRef(ref: NodeRef): String {
        val pathBytes = ref.path.fold(ByteBuffer.allocate(8 + 4 + 2 + ref.path.size * 2).apply {
            putLong(ref.generation)
            putInt(ref.windowId)
            putShort(ref.path.size.toShort())
        }) { buffer, value ->
            buffer.putShort(value.toShort())
        }.array()
        val mac = Mac.getInstance("HmacSHA256").apply {
            init(SecretKeySpec(tokenKey, "HmacSHA256"))
        }.doFinal(pathBytes).copyOf(12)
        return Base64.encodeToString(pathBytes + mac, Base64.URL_SAFE or Base64.NO_WRAP or Base64.NO_PADDING)
    }

    private fun displaySize(): Pair<Int, Int> {
        val context = applicationContext ?: return 0 to 0
        val metrics = context.resources.displayMetrics
        return metrics.widthPixels to metrics.heightPixels
    }

    private fun scanSurface(root: AccessibilityNodeInfo): SurfaceScan {
        val parts = ArrayList<String>(32)
        var visited = 0
        var containsSensitiveNode = false
        var truncated = false

        fun collect(node: AccessibilityNodeInfo, depth: Int) {
            if (depth > MAX_DEPTH || visited >= MAX_SECURITY_SCAN_NODES) {
                truncated = true
                return
            }
            visited += 1
            val sensitive = ComputerUseSecurity.isSensitiveNode(node)
            containsSensitiveNode = containsSensitiveNode || sensitive
            if (!sensitive && parts.size < 64) {
                node.text?.toString()?.takeIf(String::isNotBlank)?.let(parts::add)
                if (parts.size < 64) {
                    node.contentDescription
                        ?.toString()
                        ?.takeIf(String::isNotBlank)
                        ?.let(parts::add)
                }
            }
            for (index in 0 until node.childCount) {
                if (visited >= MAX_SECURITY_SCAN_NODES) {
                    truncated = true
                    break
                }
                val child = node.getChild(index) ?: continue
                try {
                    collect(child, depth + 1)
                } finally {
                    child.recycle()
                }
            }
        }
        collect(root, 0)
        return SurfaceScan(
            visibleText = parts.joinToString(" ").take(8_192),
            containsSensitiveNode = containsSensitiveNode,
            truncated = truncated,
        )
    }

    private fun assertCurrentSurfaceSafe() {
        val root = requireService().activeRoot()
            ?: throw AndroidComputerUseFfiException.Other("当前窗口不可访问")
        try {
            val packageName = root.packageName?.toString().orEmpty()
            val scan = scanSurface(root)
            if (
                ComputerUseSecurity.isHardBlockedSurface(
                    packageName = packageName,
                    visibleText = scan.visibleText,
                    containsSensitiveNode = scan.containsSensitiveNode,
                    scanTruncated = scan.truncated,
                )
            ) {
                stopForSecurityViolation("当前界面包含密码、支付、系统授权或无法完整检查的内容")
                throw AndroidComputerUseFfiException.ProtectedSurface(
                    "当前界面包含密码、支付或系统授权内容",
                )
            }
        } finally {
            root.recycle()
        }
    }

    private fun currentSurfaceViolation(): String? {
        val root = accessibility?.activeRoot() ?: return null
        return try {
            val packageName = root.packageName?.toString().orEmpty()
            if (
                !shouldInspectComputerUseSurface(
                    hostPackage = applicationContext?.packageName,
                    rootPackage = packageName,
                )
            ) {
                return null
            }
            val scan = scanSurface(root)
            if (
                ComputerUseSecurity.isHardBlockedSurface(
                    packageName = packageName,
                    visibleText = scan.visibleText,
                    containsSensitiveNode = scan.containsSensitiveNode,
                    scanTruncated = scan.truncated,
                )
            ) {
                "检测到密码、支付、系统授权或无法完整检查的界面"
            } else {
                null
            }
        } finally {
            root.recycle()
        }
    }

    private fun stopForSecurityViolation(message: String) {
        val context = applicationContext ?: return
        if (
            !ComputerUseSecurity.hasActiveAuthorization(
                mutableState.value.sessionState,
                hasInMemoryGrants(),
            )
        ) {
            return
        }
        mutableState.value = mutableState.value.copy(lastError = message)
        stop(context, "security-violation")
    }

    private fun JSONObject.matches(node: JSONObject): Boolean {
        fun optionalContains(key: String, nodeKey: String = key): Boolean {
            if (!has(key) || isNull(key)) return true
            val query = optString(key)
            return node.optString(nodeKey).contains(query, ignoreCase = true)
        }
        fun optionalBoolean(key: String): Boolean =
            !has(key) || isNull(key) || optBoolean(key) == node.optBoolean(key)
        return optionalContains("text") &&
            optionalContains("content_description") &&
            optionalContains("resource_id") &&
            optionalContains("class_name") &&
            optionalBoolean("clickable") &&
            optionalBoolean("editable") &&
            optionalBoolean("enabled")
    }

    private fun JSONObject.coordinateTarget(nodes: List<NodeSnapshot>): NodeSnapshot? {
        if (!has("x") || !has("y") || isNull("x") || isNull("y")) return null
        val x = optInt("x")
        val y = optInt("y")
        return nodes.asSequence()
            .filter { node ->
                val bounds = node.json.getJSONObject("bounds")
                x in bounds.getInt("left")..bounds.getInt("right") &&
                    y in bounds.getInt("top")..bounds.getInt("bottom")
            }
            .minByOrNull { node ->
                val bounds = node.json.getJSONObject("bounds")
                (bounds.getInt("right") - bounds.getInt("left")) *
                    (bounds.getInt("bottom") - bounds.getInt("top"))
            }
    }

    private fun rebindApprovedNode(
        action: JSONObject,
        original: NodeSnapshot?,
        refreshed: List<NodeSnapshot>,
    ): NodeSnapshot? {
        if (original == null) return action.coordinateTarget(refreshed)

        val candidate = if (
            action.has("x") &&
            action.has("y") &&
            !action.isNull("x") &&
            !action.isNull("y")
        ) {
            action.coordinateTarget(refreshed)
        } else {
            refreshed.filter { original.hasSameStableTarget(it) }.singleOrNull()
        }
        if (candidate == null || !original.hasSameStableTarget(candidate)) {
            throw AndroidComputerUseFfiException.StaleNode(
                "确认期间目标界面已变化，请重新读取界面并再次操作",
            )
        }
        return candidate
    }

    private fun NodeSnapshot.hasSameStableTarget(other: NodeSnapshot): Boolean {
        fun JSONObject.stableString(key: String): String? =
            optString(key).takeIf(String::isNotBlank)

        val resourceId = json.stableString("resource_id")
        val otherResourceId = other.json.stableString("resource_id")
        if (resourceId != otherResourceId) return false
        if (json.optString("class_name") != other.json.optString("class_name")) return false
        if (summary != other.summary) return false

        val bounds = json.getJSONObject("bounds")
        val otherBounds = other.json.getJSONObject("bounds")
        return listOf("left", "top", "right", "bottom").all { key ->
            bounds.optInt(key) == otherBounds.optInt(key)
        }
    }

    private fun JSONArray?.orEmptyStrings(): List<String> {
        if (this == null) return emptyList()
        return buildList {
            for (index in 0 until length()) {
                optString(index).takeIf(String::isNotBlank)?.let(::add)
            }
        }
    }

    private fun String.toTier(): ComputerUseTier = when (lowercase()) {
        "read" -> ComputerUseTier.Read
        "click" -> ComputerUseTier.Click
        "full" -> ComputerUseTier.Full
        else -> throw AndroidComputerUseFfiException.PermissionDenied("无效权限等级")
    }

    private fun String.toSnakeCase(): String =
        replace(Regex("([a-z])([A-Z])"), "$1_$2").lowercase()

    private fun JSONObject.putNullable(key: String, value: Any?): JSONObject =
        put(key, value ?: JSONObject.NULL)

    private fun safeApprovalSummary(action: String, raw: String): String {
        val sensitive = Regex(
            "(password|passcode|密码|验证码|otp|银行卡|card|cvv|cvc)",
            RegexOption.IGNORE_CASE,
        )
        return if (sensitive.containsMatchIn(raw)) {
            "$action · 敏感内容已隐藏"
        } else {
            "$action · ${raw.take(120)}"
        }
    }

    private fun isSystemUi(current: String, requested: String): Boolean =
        requested == SYSTEM_UI_PACKAGE && isSystemSurfacePackage(current)

    private fun isSystemSurfacePackage(packageName: String): Boolean {
        if (packageName == SYSTEM_UI_PACKAGE) return true
        if (packageName in setOf("com.android.launcher3", "com.google.android.apps.nexuslauncher")) {
            return true
        }
        val context = applicationContext ?: return false
        val home = Intent(Intent.ACTION_MAIN).addCategory(Intent.CATEGORY_HOME)
        return context.packageManager.resolveActivity(home, PackageManager.MATCH_DEFAULT_ONLY)
            ?.activityInfo
            ?.packageName == packageName
    }

    private data class NodeRef(
        val generation: Long,
        val windowId: Int,
        val path: IntArray,
    )

    private data class NodeSnapshot(
        val token: String,
        val ref: NodeRef,
        val json: JSONObject,
        val summary: String,
    )

    private data class TreeSnapshot(
        val json: JSONObject,
        val nodes: MutableList<NodeSnapshot>,
        val truncated: Boolean,
    )

    private data class SurfaceScan(
        val visibleText: String,
        val containsSensitiveNode: Boolean,
        val truncated: Boolean,
    )

    private const val APPROVAL_NOTIFICATION_ID = 0x4356
    private val SYSTEM_UI_GLOBAL_ACTIONS = setOf(
        "home",
        "recents",
        "notifications",
        "quick_settings",
    )
}

internal fun AudioOperationException.toComputerUseAudioFfiException(): AndroidComputerUseFfiException = when (kind) {
    DeviceAudioErrorKind.PermissionDenied -> AndroidComputerUseFfiException.PermissionDenied(
        message ?: "麦克风权限未授予",
    )
    DeviceAudioErrorKind.Timeout -> AndroidComputerUseFfiException.Timeout(message ?: "音频操作超时")
    DeviceAudioErrorKind.Unsupported -> AndroidComputerUseFfiException.Unsupported(message ?: "音频操作不受支持")
    DeviceAudioErrorKind.Busy -> AndroidComputerUseFfiException.Other(
        "音频设备暂时繁忙，请稍后重试：${message ?: "audio operation is busy"}",
    )
    else -> AndroidComputerUseFfiException.Other("${kind.name.lowercase()}: ${message ?: "音频操作失败"}")
}
