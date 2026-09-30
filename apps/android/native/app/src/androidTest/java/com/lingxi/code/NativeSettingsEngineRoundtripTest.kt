package com.lingxi.code

import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import com.lingxi.code.bindings.*
import com.lingxi.code.clipboard.AndroidClipboardAdapter
import com.lingxi.code.location.AndroidLocationAdapter
import com.lingxi.code.notify.AndroidNotificationAdapter
import com.lingxi.code.share.AndroidShareAdapter
import com.lingxi.code.vision.AndroidCameraAdapter
import com.lingxi.code.voice.recorder.AndroidVoiceAdapter
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withTimeout
import org.json.JSONObject
import org.junit.Assert.*
import org.junit.Test
import org.junit.runner.RunWith
import java.io.File
import java.util.UUID

/** Real ABI -> mobile engine -> isolated files. No provider, key, prompt or user settings. */
@RunWith(AndroidJUnit4::class)
class NativeSettingsEngineRoundtripTest {
    @Test fun keylessLayerWritesReadBackAndSurviveEngineRebuild() = runBlocking {
        val context = InstrumentationRegistry.getInstrumentation().targetContext
        val root = File(context.cacheDir, "native-settings-${UUID.randomUUID()}").apply { mkdirs() }
        val workspace = File(root, "projects/${UUID.randomUUID()}/workspace").apply { mkdirs() }
        val userFile = File(root, ".lingxi/settings.json").apply {
            parentFile!!.mkdirs(); writeText("""{"viewMode":"focus"}""")
        }
        val layers = listOf(
            Triple(WritableScopeDto.USER, userFile, "terse"),
            Triple(WritableScopeDto.PROJECT, File(workspace, ".lingxi/settings.json"), "verbose"),
            Triple(WritableScopeDto.LOCAL, File(workspace, ".lingxi/settings.local.json"), "default"),
        )
        try {
            suspend fun exercise(write: Boolean) {
                val events = Channel<ClientEvent>(Channel.UNLIMITED)
                val engine = buildAndroidEngine(
                    config = AndroidEngineLaunchConfigFfi(
                        apiBase = "https://invalid.example", apiKey = "", model = "", sessionMode = SessionModeDto.CODE,
                        visionDelegationEnabled = false, appFilesRoot = root.absolutePath, projectCwd = workspace.absolutePath,
                        providerConfig = AndroidProviderConfigFfi("{}", """{"mobileEnabledProfiles":[]}"""),
                        mobileLinux = null, localAppsFullRuntime = false, localAppsRuntimeRoot = null,
                        physicalMemoryBytes = 0u, hostEnvironment = null),
                    listener = object : AndroidEventListener {
                        override suspend fun onEvent(event: ClientEvent) {
                            if (event is ClientEvent.SettingsSnapshot || event is ClientEvent.Error) events.trySend(event)
                        }
                        override suspend fun onWorkflowProgress(originSessionId: String, taskId: String, runId: String, progress: WorkflowProgressDto) {}
                    },
                    stt = object : AndroidStt { override suspend fun transcribe(language: String?): String = error("No speech in settings test") },
                    tts = object : AndroidTts { override suspend fun synthesize(text: String, voice: String?): TtsAudioFfi = error("No speech in settings test") },
                    camera = AndroidCameraAdapter(), share = AndroidShareAdapter(), voice = AndroidVoiceAdapter(),
                    location = AndroidLocationAdapter(), notifications = AndroidNotificationAdapter(), clipboard = AndroidClipboardAdapter(),
                    permissions = object : AndroidPermissionSink { override suspend fun onRequest(request: PermissionRequest) { error("No permission in settings test") } },
                    computerUse = null, shell = null, git = null, gitCredentialProvider = null, secureStorage = null, deviceControl = null)
                suspend fun snapshot(layer: String? = null, value: String? = null): ClientEvent.SettingsSnapshot = withTimeout(20_000) {
                    while (true) {
                        when (val event = events.receive()) {
                            is ClientEvent.Error -> error("Real engine rejected settings command: ${event.message}")
                            is ClientEvent.SettingsSnapshot -> {
                                if (layer == null || JSONObject(event.layersJson ?: "{}").optJSONObject(layer)?.optString("outputStyle") == value) return@withTimeout event
                            }
                            else -> Unit
                        }
                    }
                    @Suppress("UNREACHABLE_CODE") error("Missing settings snapshot")
                }
                try {
                    engine.submit(ClientCommand.RefreshListings(listOf(ListingKindDto.SETTINGS)))
                    assertEquals("focus", JSONObject(snapshot().effectiveJson).getString("viewMode"))
                    if (write) for ((destination, file, value) in layers) {
                        engine.submit(ClientCommand.UpdateSettings(destination, JSONObject().put("outputStyle", value).toString()))
                        val ack = snapshot(destination.name.lowercase(), value)
                        assertEquals(value, JSONObject(ack.effectiveJson).getString("outputStyle"))
                        assertEquals(value, JSONObject(file.readText()).getString("outputStyle"))
                        engine.submit(ClientCommand.RefreshListings(listOf(ListingKindDto.SETTINGS)))
                        snapshot(destination.name.lowercase(), value)
                    }
                    engine.submit(ClientCommand.RefreshListings(listOf(ListingKindDto.SETTINGS)))
                    val final = snapshot("local", "default")
                    val maps = JSONObject(requireNotNull(final.layersJson))
                    for ((destination, file, value) in layers) {
                        assertEquals(value, maps.getJSONObject(destination.name.lowercase()).getString("outputStyle"))
                        assertEquals(value, JSONObject(file.readText()).getString("outputStyle"))
                    }
                    assertEquals("focus", JSONObject(userFile.readText()).getString("viewMode"))
                } finally { engine.destroy(); events.close() }
            }
            exercise(write = true)
            exercise(write = false)
        } finally { root.deleteRecursively() }
    }
}
