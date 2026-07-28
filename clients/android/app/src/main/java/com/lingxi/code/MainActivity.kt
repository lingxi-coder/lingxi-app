package com.lingxi.code

import android.Manifest
import android.content.pm.PackageManager
import android.os.Build
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.activity.result.contract.ActivityResultContracts
import androidx.core.content.ContextCompat
import com.lingxi.code.vision.CameraController
import com.lingxi.code.share.ShareController
import com.lingxi.code.notify.NotificationController
import com.lingxi.code.clipboard.ClipboardController
import com.lingxi.code.voice.recorder.RecorderController
import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.slideInVertically
import androidx.compose.animation.slideOutVertically
import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.lifecycle.lifecycleScope
import androidx.lifecycle.viewmodel.compose.viewModel
import com.lingxi.code.cron.CronAlarmScheduler
import com.lingxi.code.onboarding.SetupWizardOverlay
import com.lingxi.code.settings.SettingsHost
import com.lingxi.code.settings.SettingsStore
import com.lingxi.code.theme.AppearancePrefs
import com.lingxi.code.theme.AppearanceStore
import com.lingxi.code.theme.LingXiTheme
import com.lingxi.code.theme.ThemeMode
import kotlinx.coroutines.launch

/**
 * The single Activity host. Edge-to-edge, Compose-only — everything else
 * (drawer, conversation, settings nav graph, voice overlay) is composed under
 * [RootScreen].
 *
 * Theme + accent are read from the DataStore-backed [AppearanceStore] and fed
 * into [LingXiTheme], so the appearance is persisted and live. Later phases fill
 * in [RootScreen] and let the Appearance page mutate the same store.
 */
class MainActivity : ComponentActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        // Register the camera/picker launchers before the Activity is STARTED and
        // hand them to the process-global CameraController, which the UniFFI
        // AndroidCamera adapter drives across the FFI seam (the device-vision
        // analog of how the mic is invoked for STT). Must run before super/
        // setContent so registerForActivityResult is valid.
        val cameraPermLauncher = registerForActivityResult(
            ActivityResultContracts.RequestPermission(),
        ) { granted -> CameraController.onCameraPermission(granted) }
        val takePictureLauncher = registerForActivityResult(
            ActivityResultContracts.TakePicturePreview(),
        ) { bitmap -> CameraController.onPictureTaken(bitmap) }
        val pickMediaLauncher = registerForActivityResult(
            ActivityResultContracts.PickVisualMedia(),
        ) { uri -> CameraController.onMediaPicked(uri) }
        // Android 13+ (TIRAMISU) gates posting on the POST_NOTIFICATIONS runtime
        // permission — declaring it in the manifest is NOT enough at targetSdk 33+.
        // Without this request the permission stays denied and EVERY notification
        // (the engine `notification` tool AND the cron result / foreground-service
        // notifications) silently fails. Registered before STARTED; launched below.
        val notificationPermLauncher = registerForActivityResult(
            ActivityResultContracts.RequestPermission(),
        ) { /* best-effort: nothing to do on grant/deny — posting self-gates */ }
        CameraController.attach(
            CameraController.makeLaunchers(
                context = applicationContext,
                requestCameraPermission = cameraPermLauncher,
                takePicture = takePictureLauncher,
                pickMedia = pickMediaLauncher,
            ),
        )
        // Device-share: hand the application Context to the process-global
        // ShareController, which the UniFFI AndroidShare adapter drives across
        // the FFI seam to launch the system share sheet (the device analog of
        // how the camera/picker is invoked, but with no ActivityResult to await).
        ShareController.attach(applicationContext)
        // Device-voice: hand the application Context to the process-global
        // RecorderController, which the UniFFI AndroidVoice adapter drives across
        // the FFI seam to run a MediaRecorder mic session (engine-driven through
        // tool-voice; no UI button, unlike the STT hold-to-talk path).
        RecorderController.attach(applicationContext)
        // Device-notifications: hand the application Context to the process-global
        // NotificationController, which the UniFFI AndroidNotification adapter
        // drives across the FFI seam to post to the system NotificationManager
        // (engine-driven through tool-notification; no UI affordance).
        NotificationController.attach(applicationContext)
        // Device-clipboard: hand the application Context to the process-global
        // ClipboardController, which the UniFFI AndroidClipboard adapter drives
        // across the FFI seam to read/write the system ClipboardManager
        // (engine-driven through tool-clipboard; no UI affordance). No manifest
        // permission is required for clipboard access.
        ClipboardController.attach(applicationContext)
        // Offline voice-model downloader — process-global so a language-pack
        // download started in the setup wizard survives leaving that step.
        com.lingxi.code.voice.offline.VoiceModelDownloader.attach(applicationContext)

        // Cron: re-arm the next scheduled-task alarm on every launch. Exact alarms
        // do not survive process death / app updates, so arming here self-heals the
        // schedule and picks up jobs created via chat. Off the main thread (it
        // builds a transient engine to query the next fire) and best-effort.
        lifecycleScope.launch(kotlinx.coroutines.Dispatchers.Default) {
            runCatching { CronAlarmScheduler.armNext(applicationContext) }
        }

        enableEdgeToEdge()
        super.onCreate(savedInstanceState)

        // Request POST_NOTIFICATIONS on Android 13+ if not already granted (launched
        // AFTER super.onCreate so the ActivityResultRegistry can dispatch). The OS
        // shows the dialog only when undecided; an already-granted or
        // permanently-denied state returns immediately with no prompt, so launching
        // on every start is safe. Notifications stay denied (and silent) until this.
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU &&
            ContextCompat.checkSelfPermission(
                this,
                Manifest.permission.POST_NOTIFICATIONS,
            ) != PackageManager.PERMISSION_GRANTED
        ) {
            notificationPermLauncher.launch(Manifest.permission.POST_NOTIFICATIONS)
        }
        setContent {
            val store = remember { AppearanceStore(applicationContext) }
            val settingsStore: SettingsStore =
                viewModel(factory = SettingsStore.factory(applicationContext))
            val scope = rememberCoroutineScope()
            val prefs by store.prefs.collectAsState(initial = AppearancePrefs())
            val darkTheme = when (prefs.themeMode) {
                ThemeMode.Dark -> true
                ThemeMode.Light -> false
                ThemeMode.System -> isSystemInDarkTheme()
            }
            // Settings is a full-surface overlay that slides up over the
            // conversation (the Android analog of the iOS settings sheet); the
            // drawer's account row opens it, system-back / close dismisses it.
            var settingsOpen by remember { mutableStateOf(false) }
            // Monotonic across configuration changes: rotation must not turn a
            // prior reconnect generation back into zero and rebuild a live engine.
            var engineReconnect by rememberSaveable { mutableIntStateOf(0) }

            LingXiTheme(darkTheme = darkTheme, accentId = prefs.accentId) {
                Box(Modifier.fillMaxSize()) {
                    RootScreen(
                        isDark = darkTheme,
                        onToggleTheme = {
                            scope.launch {
                                store.setThemeMode(if (darkTheme) ThemeMode.Light else ThemeMode.Dark)
                            }
                        },
                        onOpenSettings = { settingsOpen = true },
                        assistantName = prefs.assistantName,
                        inputDialog = prefs.inputDialog,
                        voiceLang = prefs.voiceLang,
                        reconnectToken = engineReconnect,
                        settingsStore = settingsStore,
                    )
                    AnimatedVisibility(
                        visible = settingsOpen,
                        enter = slideInVertically(initialOffsetY = { it }),
                        exit = slideOutVertically(targetOffsetY = { it }),
                    ) {
                        SettingsHost(
                            appearanceStore = store,
                            isDark = darkTheme,
                            accentId = prefs.accentId,
                            store = settingsStore,
                            onClose = { settingsOpen = false },
                            // 关于 → 重新观看引导: clear setupDone (replays the wizard)
                            // and drop back to the conversation behind it.
                            onReplayOnboarding = {
                                scope.launch { store.setSetupDone(false) }
                                settingsOpen = false
                            },
                            // 重新连接引擎: rebuild the engine against the just-saved key
                            // and drop back to the (now-real) conversation.
                            onReconnectEngine = {
                                engineReconnect += 1
                                settingsOpen = false
                            },
                        )
                    }

                    // First-run setup wizard — shown until onboarding completes;
                    // sits above settings so the replay path covers it too. On
                    // finish it persists the chosen profile + flips setupDone.
                    SetupWizardOverlay(
                        visible = !prefs.setupDone,
                        initialAssistantName = prefs.assistantName,
                        initialUserName = prefs.userName,
                        initialVoiceprint = prefs.voiceprint,
                        initialModelId = prefs.defaultModelId,
                        initialVoiceLang = prefs.voiceLang,
                        onFinish = { assistantName, userName, voiceprint, modelId, voiceLang ->
                            scope.launch {
                                store.setAssistantName(assistantName)
                                store.setUserName(userName)
                                store.setVoiceprint(voiceprint)
                                store.setDefaultModel(modelId)
                                store.setVoiceLang(voiceLang)
                                store.setSetupDone(true)
                            }
                        },
                    )
                }
            }
        }
    }

    override fun onDestroy() {
        // Drop the launcher references so a finishing Activity can't be leaked by
        // the process-global controller and any in-flight capture is cancelled.
        CameraController.detach()
        ShareController.detach()
        RecorderController.detach()
        NotificationController.detach()
        ClipboardController.detach()
        super.onDestroy()
    }
}
