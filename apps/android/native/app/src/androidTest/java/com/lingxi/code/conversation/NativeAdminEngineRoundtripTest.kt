package com.lingxi.code.conversation

import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import com.lingxi.code.bindings.client.*
import com.lingxi.code.bindings.runtime.*
import com.lingxi.code.bindings.android.*
import com.lingxi.code.clipboard.AndroidClipboardAdapter
import com.lingxi.code.location.AndroidLocationAdapter
import com.lingxi.code.notify.AndroidNotificationAdapter
import com.lingxi.code.share.AndroidShareAdapter
import com.lingxi.code.vision.AndroidCameraAdapter
import com.lingxi.code.voice.audio.AndroidNativeAudioServiceAdapter
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withTimeout
import org.json.JSONObject
import org.junit.Assert.*
import org.junit.Test
import org.junit.runner.RunWith
import java.io.File
import java.util.UUID

/** No-op command acceptance cannot pass: every admin domain must return its real typed result. */
@RunWith(AndroidJUnit4::class)
class NativeAdminEngineRoundtripTest {
    @Test fun allAdminReadsAndCorrelatedHookValidationReturnThroughNativeCallbacks() = runBlocking {
        val context = InstrumentationRegistry.getInstrumentation().targetContext
        val root = File(context.cacheDir, "native-admin-${UUID.randomUUID()}").apply { mkdirs() }
        val workspace = File(root, "projects/${UUID.randomUUID()}/workspace").apply { mkdirs() }
        val events = Channel<ClientEvent>(Channel.UNLIMITED)
        val engine = buildAndroidEngine(
            config = AndroidEngineLaunchConfigFfi(
                apiBase = "https://invalid.example", apiKey = "", model = "", sessionMode = SessionModeDto.CODE,
                visionDelegationEnabled = false, appFilesRoot = root.absolutePath, projectCwd = workspace.absolutePath,
                providerConfig = AndroidProviderConfigFfi("{}", """{"mobileEnabledProfiles":[]}"""),
                mobileLinux = null, localAppsFullRuntime = false, localAppsRuntimeRoot = null,
                physicalMemoryBytes = 0u, hostEnvironment = null),
            listener = object : AndroidEventListener {
                override suspend fun onEvent(event: ClientEvent) { events.trySend(event) }
                override suspend fun onWorkflowProgress(originSessionId: String, taskId: String, runId: String, progress: WorkflowProgressDto) {}
            },
            camera = AndroidCameraAdapter(), share = AndroidShareAdapter(), audio = AndroidNativeAudioServiceAdapter(context),
            location = AndroidLocationAdapter(), notifications = AndroidNotificationAdapter(), clipboard = AndroidClipboardAdapter(),
            permissions = object : AndroidPermissionSink { override suspend fun onRequest(request: PermissionRequest) { error("No permission in admin test") } },
            computerUse = null, shell = null, git = null, gitCredentialProvider = null, secureStorage = null, deviceControl = null)
        suspend fun response(command: ClientCommand, matches: (ClientEvent) -> Boolean): ClientEvent {
            while (true) {
                val pending = events.tryReceive().getOrNull() ?: break
                if (pending is ClientEvent.Error) error("Engine startup failed: ${pending.message}")
            }
            engine.submit(command)
            return withTimeout(20_000) {
                while (true) {
                    val event = events.receive()
                    if (event is ClientEvent.Error) error("Admin command failed: ${event.message}")
                    if (matches(event)) return@withTimeout event
                }
                @Suppress("UNREACHABLE_CODE") error("No admin response")
            }
        }
        try {
            val mcp = response(ClientCommand.McpAdmin(McpAdminCommandDto("get_snapshot", null, null, null, null, null))) {
                it is ClientEvent.McpConfigurationSnapshot
            } as ClientEvent.McpConfigurationSnapshot
            val scopes = JSONObject(mcp.snapshotJson).getJSONArray("scopes")
            assertTrue(scopes.length() > 0)
            assertEquals(64, scopes.getJSONObject(0).getString("revision_sha256").length)

            val skills = response(ClientCommand.SkillAdmin(SkillAdminCommandDto("get_catalog", null, null, null, null, null))) {
                it is ClientEvent.SkillCatalog
            } as ClientEvent.SkillCatalog
            assertNotNull(JSONObject(skills.catalogJson).getJSONArray("entries"))

            val plugins = response(ClientCommand.PluginAdmin(PluginAdminCommandDto("get_catalog", null, null, null, null, null))) {
                it is ClientEvent.PluginCatalog
            } as ClientEvent.PluginCatalog
            assertNotNull(JSONObject(plugins.catalogJson).getJSONArray("installed"))
            assertNotNull(JSONObject(plugins.catalogJson).getJSONObject("revisions"))

            val hooks = response(ClientCommand.HookAdmin(HookAdminCommandDto("get_document", null, null, "user", null, null))) {
                it is ClientEvent.ConfigurationOperation && it.domain == ConfigurationDomainDto.HOOK && it.detailsJson != null
            } as ClientEvent.ConfigurationOperation
            assertEquals(ConfigurationOperationStatusDto.SUCCEEDED, hooks.status)
            val document = JSONObject(requireNotNull(hooks.detailsJson))
            assertEquals("user", document.getString("scope"))
            assertEquals(64, document.getString("revision_sha256").length)
            assertNotNull(JSONObject(document.getString("own_json")))

            val validation = response(ClientCommand.HookAdmin(HookAdminCommandDto(
                "validate_document", 917uL, null, "user", null, """{"scope":"user","hooks":{}}"""))) {
                it is ClientEvent.ConfigurationOperation && it.domain == ConfigurationDomainDto.HOOK && it.operationId == 917uL &&
                    it.status in setOf(ConfigurationOperationStatusDto.SUCCEEDED, ConfigurationOperationStatusDto.FAILED)
            } as ClientEvent.ConfigurationOperation
            assertEquals(ConfigurationOperationStatusDto.SUCCEEDED, validation.status)
            assertEquals(ConfigurationEffectDto.NOT_APPLICABLE, validation.effect)
        } finally {
            engine.destroy()
            events.close()
            root.deleteRecursively()
        }
    }
}
