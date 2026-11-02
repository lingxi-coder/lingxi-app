package com.lingxi.code

import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import com.lingxi.code.bindings.android.*
import com.lingxi.code.bindings.client.*
import com.lingxi.code.bindings.runtime.*
import com.lingxi.code.clipboard.AndroidClipboardAdapter
import com.lingxi.code.location.AndroidLocationAdapter
import com.lingxi.code.notify.AndroidNotificationAdapter
import com.lingxi.code.secure.AndroidSecureStorageAdapter
import com.lingxi.code.share.AndroidShareAdapter
import com.lingxi.code.voice.audio.AndroidNativeMediaHostProvider
import java.io.File
import java.util.UUID
import java.util.concurrent.atomic.AtomicInteger
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withTimeout
import org.json.JSONArray
import org.json.JSONObject
import org.junit.Assert.*
import org.junit.Test
import org.junit.runner.RunWith

/** Actual JNI + encrypted Android storage, isolated from all installed-app accounts. No network. */
@RunWith(AndroidJUnit4::class)
class ProviderAccountsNativeRoundtripTest {
    @Test fun independentAccountsRotateReorderAndSurviveEngineRebuild() = runBlocking {
        withTimeout(120_000) {
            val context = InstrumentationRegistry.getInstrumentation().targetContext
            // Fresh roots contain no MCP definitions, hooks or scheduled work.
            val root = File(context.cacheDir, "provider-accounts-native-${UUID.randomUUID()}").apply { mkdirs() }.canonicalFile
            val runtime = mobileWorkspaceTestConfiguration(root)
            val paths = mobileUserDirectories(runtime.managedRoot)
            val workspace = File(paths.projects, "${UUID.randomUUID()}/workspace").apply { mkdirs() }.canonicalFile
            val secureStorage = AndroidSecureStorageAdapter(File(root, "secure-store"))
            val payloadReads = AtomicInteger(0)
            val storageBoundary = object : AndroidSecureStorage by secureStorage {
                override suspend fun retrieve(service: String, account: String): ByteArray? {
                    payloadReads.incrementAndGet()
                    return secureStorage.retrieve(service, account)
                }
            }
            fun buildOnce(): MobileEngineHandle = buildAndroidEngine(
                config = AndroidEngineLaunchConfigFfi(
                    // None maps to MobileConfig.startup_catalog=None: no catalog
                    // prefetch/load/subscription or upstream discovery. The Host
                    // still projects its compiled-in canonical model definitions.
                    appVersion = null, appAgentHost = false,
                    apiBase = "https://invalid.example", model = "", sessionMode = SessionModeDto.CODE,
                    visionDelegationEnabled = false, appFilesRoot = root.path, projectCwd = workspace.path,
                    providerConfig = AndroidProviderConfigFfi("{}", """{"mobileEnabledProfiles":["openai"]}"""),
                    mobileLinux = runtime, physicalMemoryBytes = 0u, hostEnvironment = null,
                ),
                listener = object : AndroidEventListener {
                    override suspend fun onEvent(event: ClientEvent) {}
                    override suspend fun onWorkflowProgress(originSessionId: String, taskId: String, runId: String, progress: WorkflowProgressDto) {}
                },
                mediaHost = AndroidNativeMediaHostProvider.get(context.applicationContext), share = AndroidShareAdapter(),
                location = AndroidLocationAdapter(), notifications = AndroidNotificationAdapter(), clipboard = AndroidClipboardAdapter(),
                permissions = object : AndroidPermissionSink {
                    override suspend fun onRequest(request: PermissionRequest) { error("Account metadata test must not request permission") }
                },
                computerUse = null, shell = null, git = null, gitCredentialProvider = null,
                secureStorage = storageBoundary, deviceControl = null,
            )
            suspend fun build(): MobileEngineHandle = withTimeout(15_000) {
                // Disposal starts asynchronous native media cleanup. Only retry
                // its explicit admission fence; every other build failure propagates.
                var rebuilt: MobileEngineHandle? = null
                while (rebuilt == null) {
                    try { rebuilt = buildOnce() }
                    catch (error: MobileEngineException.Internal) {
                        if (error.message?.contains("a previous media attachment has an unconfirmed drain") != true) throw error
                        kotlinx.coroutines.delay(50)
                    }
                }
                rebuilt
            }
            var engine: MobileEngineHandle? = null
            try {
                engine = build()
                suspend fun snapshot(): JSONObject = JSONObject(requireNotNull(engine).providerAccountsSnapshot())
                suspend fun command(action: String, fields: JSONObject = JSONObject(), secret: String? = null) {
                    fields.put("action", action).put("expectedRevision", snapshot().getLong("configRevision"))
                    requireNotNull(engine).providerAccountCommand(fields.toString(), secret)
                }
                fun rows(snapshot: JSONObject): List<JSONObject> = snapshot.getJSONArray("accounts").let { items ->
                    (0 until items.length()).map { items.getJSONObject(it) }.sortedBy { it.getLong("userPriority") }
                }
                val first = UUID.randomUUID().toString()
                val second = UUID.randomUUID().toString()
                val keyA = "synthetic-local-only-A-${UUID.randomUUID()}"
                val keyB = "synthetic-local-only-B-${UUID.randomUUID()}"
                assertEquals(0, snapshot().getJSONArray("accounts").length())
                for ((id, key) in listOf(first to keyA, second to keyB)) {
                    command("add", JSONObject().put("accountId", id).put("providerId", "openai")
                        .put("displayName", id).put("executionBindingId", "openai").put("authKind", "api_key"), key)
                }
                val initial = snapshot()
                assertFalse(initial.toString().contains(keyA))
                assertFalse(initial.toString().contains(keyB))
                assertEquals(listOf(first, second), rows(initial).map { it.getString("accountId") })
                assertNotEquals(rows(initial)[0].getJSONObject("credentialRef").getString("secretRef"), rows(initial)[1].getJSONObject("credentialRef").getString("secretRef"))
                val secondGeneration = rows(initial)[1].getLong("identityGeneration")
                val firstGeneration = rows(initial)[0].getLong("identityGeneration")
                command("set_credential", JSONObject().put("accountId", first), "synthetic-rotated-A")
                val rotated = rows(snapshot()).associateBy { it.getString("accountId") }
                assertTrue(rotated.getValue(first).getLong("identityGeneration") > firstGeneration)
                assertEquals(secondGeneration, rotated.getValue(second).getLong("identityGeneration"))
                command("update", JSONObject().put("accountId", first).put("displayName", "Renamed first"))
                command("reorder", JSONObject().put("providerId", "openai").put("accountIds", JSONArray(listOf(second, first))))
                command("set_enabled", JSONObject().put("accountId", second).put("enabled", false))
                command("set_policy", JSONObject().put("providerId", "openai").put("allowSubscriptionToMetered", true))
                val beforeStale = snapshot().getLong("configRevision")
                var staleRejected = false
                try {
                    requireNotNull(engine).providerAccountCommand(JSONObject().put("action", "remove").put("accountId", second)
                        .put("expectedRevision", 0).toString(), null)
                } catch (_: Exception) { staleRejected = true }
                assertTrue("Stale mutations must be rejected", staleRejected)
                assertEquals(beforeStale, snapshot().getLong("configRevision"))
                engine?.destroy()
                engine = null
                engine = build()
                val rebuilt = snapshot()
                assertEquals(initial.getString("storeScope"), rebuilt.getString("storeScope"))
                assertEquals(listOf(second, first), rows(rebuilt).map { it.getString("accountId") })
                assertFalse(rows(rebuilt).first().getBoolean("enabled"))
                assertEquals("Renamed first", rows(rebuilt).last().getString("displayName"))
                assertTrue(rebuilt.getJSONObject("providerPolicies").getJSONObject("openai").getBoolean("allowSubscriptionToMetered"))
                command("clear_credential", JSONObject().put("accountId", first))
                val cleared = snapshot().getJSONArray("health")
                val firstHealth = (0 until cleared.length()).map { cleared.getJSONObject(it) }.first { it.getString("accountId") == first }
                assertEquals("needs_auth", firstHealth.getString("status"))
                command("remove", JSONObject().put("accountId", first))
                assertEquals(listOf(second), rows(snapshot()).map { it.getString("accountId") })
                assertEquals(secondGeneration, rows(snapshot()).single().getLong("identityGeneration"))
                command("remove", JSONObject().put("accountId", second))
                assertEquals(0, snapshot().getJSONArray("accounts").length())

                // Prove the real mobile [openai] logical enable reaches its trusted
                // ChatGPT execution binding. This is synthetic fixture setup, not OAuth login.
                command("set_policy", JSONObject().put("providerId", "openai").put("allowSubscriptionToMetered", false))
                val oauthId = UUID.randomUUID().toString()
                command("add", JSONObject().put("accountId", oauthId).put("providerId", "openai")
                    .put("displayName", "Synthetic OAuth projection fixture").put("executionBindingId", "openai-chatgpt").put("authKind", "oauth"))
                val oauthAccount = rows(snapshot()).single()
                engine?.destroy()
                engine = null
                val reference = oauthAccount.getJSONObject("credentialRef")
                val scope = reference.getString("storeScope")
                val generation = oauthAccount.getLong("identityGeneration")
                val fakeToken = "synthetic-no-upstream-${UUID.randomUUID()}"
                val nowSeconds = System.currentTimeMillis() / 1000
                fun timestamp(seconds: Long) = JSONObject().put("secs_since_epoch", seconds).put("nanos_since_epoch", 0)
                // Exact secret::AccountCredential + core::SecureStorageData serde
                // contracts. Write only the fixture's encrypted store while no engine is live.
                val grant = JSONObject().put("credential_revision", 1).put("identity_generation", generation)
                    .put("kind", JSONObject().put("kind", "oauth").put("access_token", fakeToken)
                        .put("refresh_token", JSONObject.NULL).put("expires_at", timestamp(nowSeconds + 3600))
                        .put("scopes", JSONArray()).put("issuer", "https://auth.openai.com")
                        .put("subject", "synthetic-fixture-principal").put("organization_id", "synthetic-fixture-workspace")
                        .put("email", JSONObject.NULL).put("fedramp", false))
                val envelope = JSONObject().put("bytes", JSONArray(grant.toString().toByteArray(Charsets.UTF_8).map { it.toInt() and 255 }))
                    .put("metadata", JSONObject().put("created_at", timestamp(nowSeconds))
                        .put("last_accessed", JSONObject.NULL).put("kind", "provider_account_credential"))
                secureStorage.store("provider-accounts.$scope", reference.getString("secretRef"), envelope.toString().toByteArray(Charsets.UTF_8))
                // Match the non-secret account_versions ledger, also inside the isolated home.
                val version = File(paths.configHome, "provider-accounts/versions/$scope/$oauthId.json")
                assertTrue(version.canonicalFile.toPath().startsWith(File(paths.configHome).canonicalFile.toPath()))
                version.parentFile!!.mkdirs()
                version.writeText(JSONObject().put("accountId", oauthId).put("identityGeneration", generation).put("credentialRevision", 1).toString())
                payloadReads.set(0)
                engine = build()
                val projected = snapshot()
                assertFalse(projected.toString().contains(fakeToken))
                assertFalse(projected.getJSONObject("providerPolicies").getJSONObject("openai").getBoolean("allowSubscriptionToMetered"))
                val availability = projected.getJSONArray("modelAvailability")
                val eligibleOAuthRows = (0 until availability.length()).map { availability.getJSONObject(it) }.filter { row ->
                    row.getString("executionBindingId") == "openai-chatgpt" &&
                        (0 until row.getJSONArray("eligibleAccountIds").length()).any { row.getJSONArray("eligibleAccountIds").getString(it) == oauthId }
                }
                assertTrue("Logical OpenAI enable must retain eligible OAuth models", eligibleOAuthRows.isNotEmpty())
                (0 until availability.length()).map { availability.getJSONObject(it) }.forEach { row ->
                    val eligible = row.getJSONArray("eligibleAccountIds")
                    if ((0 until eligible.length()).any { eligible.getString(it) == oauthId }) {
                        assertEquals("Subscription account must not cross into metered bindings without consent", "openai-chatgpt", row.getString("executionBindingId"))
                    }
                }
                eligibleOAuthRows.forEach { row ->
                    assertEquals("openai", row.getString("providerId"))
                    assertTrue(row.getString("modelId").isNotBlank())
                    assertEquals(0, row.getJSONArray("confirmedAccountIds").length())
                    assertEquals(listOf(oauthId), (0 until row.getJSONArray("unverifiedAccountIds").length()).map { row.getJSONArray("unverifiedAccountIds").getString(it) })
                }
                assertEquals("Startup and model projection must use attributes only, never credential payloads", 0, payloadReads.get())
                command("remove", JSONObject().put("accountId", oauthId))
                assertEquals(0, snapshot().getJSONArray("accounts").length())
            } finally {
                engine?.destroy()
                root.deleteRecursively()
            }
        }
    }
}
