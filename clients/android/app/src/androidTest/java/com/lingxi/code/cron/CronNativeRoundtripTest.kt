package com.lingxi.code.cron

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
import kotlinx.coroutines.withTimeout
import kotlinx.coroutines.runBlocking
import org.json.JSONObject
import org.junit.Assert.*
import org.junit.Test
import org.junit.runner.RunWith
import java.io.File
import java.util.UUID

/** Real JNI store path: isolated data, no network or model credentials. */
@RunWith(AndroidJUnit4::class)
class CronNativeRoundtripTest {
    @Test
    fun configuredTaskPersistsLifecycleAndModelAcrossStoreRebuild() = runBlocking {
        val context = InstrumentationRegistry.getInstrumentation().targetContext
        val root = File(context.cacheDir, "cron-native-${UUID.randomUUID()}").apply { mkdirs() }
        try {
            val config = CronAutomation.defaults("provider/model").change("name", "Daily brief")
                .withReasoning(ReasoningSelectionDto.Level("high"))
                .change("notificationPolicy", "failed").change("runMode", "task_session")
            val store = buildAndroidCronStore(root.path, null)
            val created = try {
                store.setMigrationDefaults("provider/model", """{"type":"automatic"}""")
                val task = store.createConfigured("0 9 * * *", "Summarize changes", true, config.json)
                assertEquals("provider/model", CronAutomation.from(task).model)
                store.updateAutomation(task.id, config.change("status", "paused").json)
            } finally { store.destroy() }
            assertTrue(File(root, "scheduled/workspace/.lingxi/scheduled_tasks.json").isFile)
            assertFalse(File(root, ".lingxi/scheduled_tasks.json").exists())
            val rebuilt = buildAndroidCronStore(root.path, null)
            try {
                val task = rebuilt.list().single()
                assertEquals(created.id, task.id)
                assertEquals("paused", CronAutomation.from(task).status)
                assertEquals("Daily brief", CronAutomation.from(task).name)
                assertEquals("task_session", CronAutomation.from(task).runMode)
                assertEquals("High", CronAutomation.from(task).reasoningLabel)
                assertEquals("failed", CronAutomation.from(task).notificationPolicy)
                assertNull(task.nextFireMs)
                assertTrue(rebuilt.dueOccurrences(System.currentTimeMillis().toULong()).isEmpty())
                val completed = rebuilt.updateAutomation(task.id, config.change("status", "completed").json)
                assertEquals("completed", JSONObject(completed.automationJson!!).getString("status"))
                assertEquals(1, rebuilt.list().size)
                assertTrue(rebuilt.delete(task.id))
                assertTrue(rebuilt.list().isEmpty())
            } finally { rebuilt.destroy() }
        } finally { root.deleteRecursively() }
    }
    @Test
    fun managedNoProjectSessionRestoresAcrossEngineRebuild(): Unit = runBlocking {
        val context = InstrumentationRegistry.getInstrumentation().targetContext
        val root = File(context.cacheDir, "cron-session-${UUID.randomUUID()}").apply { mkdirs() }
        val workspace = File(root, "scheduled/workspace").apply { mkdirs() }
        val id = UUID.randomUUID().toString()
        try {
            repeat(2) { pass ->
                val events = Channel<ClientEvent>(Channel.UNLIMITED)
                val engine = buildAndroidEngineWithMobileLinux(
                    config = AndroidEngineLaunchConfigFfi(
                        apiBase = "https://invalid.example", apiKey = "", model = "", sessionMode = SessionModeDto.CODE,
                        visionDelegationEnabled = false, appFilesRoot = root.absolutePath, projectCwd = workspace.absolutePath,
                        providerConfig = AndroidProviderConfigFfi("{}", """{"mobileEnabledProfiles":[]}"""),
                        mobileLinux = null, localAppsFullRuntime = false, localAppsRuntimeRoot = null,
                        physicalMemoryBytes = 0u, hostEnvironment = null),
                    listener = object : AndroidEventListener {
                        override suspend fun onEvent(event: ClientEvent) {
                            if (event is ClientEvent.SessionResumed || event is ClientEvent.Error) events.trySend(event)
                        }
                        override suspend fun onWorkflowProgress(originSessionId: String, taskId: String, runId: String, progress: WorkflowProgressDto) {}
                    },
                    stt = object : AndroidStt { override suspend fun transcribe(language: String?): String = error("No speech in settings test") },
                    tts = object : AndroidTts { override suspend fun synthesize(text: String, voice: String?): TtsAudioFfi = error("No speech in settings test") },
                    camera = AndroidCameraAdapter(), share = AndroidShareAdapter(), voice = AndroidVoiceAdapter(),
                    location = AndroidLocationAdapter(), notifications = AndroidNotificationAdapter(), clipboard = AndroidClipboardAdapter(),
                    permissions = object : AndroidPermissionSink { override suspend fun onRequest(request: PermissionRequest) { error("No permission in settings test") } },
                    computerUse = null, shell = null, git = null, gitCredentialProvider = null, secureStorage = null, deviceControl = null)

                try {
                    if (pass == 0) engine.resumeEmptySession(id, "Scheduled result")
                    else engine.submit(ClientCommand.ResumeSession(id, null))
                    withTimeout(20_000) {
                        while (true) {
                            when (val event = events.receive()) {
                                is ClientEvent.SessionResumed -> {
                                    assertEquals(id, event.sessionId.removePrefix("sess:"))
                                    return@withTimeout
                                }
                                is ClientEvent.Error -> error(event.message)
                                else -> Unit
                            }
                        }
                    }
                } finally { engine.destroy(); events.close() }
            }
        } finally { root.deleteRecursively() }
    }

}
