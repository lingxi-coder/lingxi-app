package com.lingxi.code

import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.activity.result.contract.ActivityResultContracts
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
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import com.lingxi.code.settings.SettingsHost
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

        enableEdgeToEdge()
        super.onCreate(savedInstanceState)
        setContent {
            val store = remember { AppearanceStore(applicationContext) }
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
                            onClose = { settingsOpen = false },
                        )
                    }
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
