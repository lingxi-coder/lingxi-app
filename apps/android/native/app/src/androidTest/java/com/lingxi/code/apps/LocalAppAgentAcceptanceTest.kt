package com.lingxi.code.apps

import android.content.Intent
import android.net.Uri
import android.os.ParcelFileDescriptor
import android.util.Base64
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import com.lingxi.code.MainActivity
import com.lingxi.code.acceptance.acceptanceArgument
import com.lingxi.code.acceptance.requireAcceptanceOptIn
import com.lingxi.code.model.canonicalSessionId
import com.lingxi.code.model.ConnStatus
import com.lingxi.code.model.GenericProvider
import com.lingxi.code.model.ProviderKind
import com.lingxi.code.settings.ProviderSettingsRepository
import kotlinx.coroutines.*
import org.json.JSONArray
import org.json.JSONObject
import org.junit.Assert.*
import org.junit.Test
import org.junit.runner.RunWith
import java.io.File
import java.net.HttpURLConnection
import java.net.URL
import java.util.UUID

/** Product Agent acceptance through the actual app gateway and system-selected browser. */
@RunWith(AndroidJUnit4::class)
class LocalAppAgentAcceptanceTest {
    private val instrumentation get() = InstrumentationRegistry.getInstrumentation()
    private val context get() = instrumentation.targetContext

    private fun visibleUiText(node: android.view.accessibility.AccessibilityNodeInfo?): String = buildString {
        fun appendNode(current: android.view.accessibility.AccessibilityNodeInfo) {
            current.text?.let { append(it).append('\n') }
            current.contentDescription?.let { append(it).append('\n') }
            repeat(current.childCount) { index -> current.getChild(index)?.let(::appendNode) }
        }
        node?.let(::appendNode)
    }

    private fun findAccessibilityNodeByText(
        node: android.view.accessibility.AccessibilityNodeInfo?,
        text: String,
    ): android.view.accessibility.AccessibilityNodeInfo? {
        if (node == null) return null
        if (node.text?.toString()?.contains(text) == true || node.contentDescription?.toString()?.contains(text) == true) {
            return node
        }
        for (index in 0 until node.childCount) {
            findAccessibilityNodeByText(node.getChild(index), text)?.let { return it }
        }
        return null
    }

    private fun sha256(bytes: ByteArray): String = java.security.MessageDigest.getInstance("SHA-256")
        .digest(bytes).joinToString("") { "%02x".format(it) }

    private fun persistProviderRowsForAcceptance(providers: List<GenericProvider>) {
        val encoded = ProviderSettingsRepository.encodeProvidersJson(providers)
        check(context.getSharedPreferences("provider_settings", android.content.Context.MODE_PRIVATE)
            .edit().putString("providers_llm", encoded).commit()) {
            "The Android provider fixture preferences could not be flushed to disk."
        }
    }

    private fun restoreProviderSettingsRawValues(
        providersPresent: Boolean,
        providersRaw: String?,
        audioRegionPresent: Boolean,
        audioRegionRaw: String?,
    ) {
        check(!providersPresent || providersRaw != null)
        check(!audioRegionPresent || audioRegionRaw != null)
        val preferences = context.getSharedPreferences("provider_settings", android.content.Context.MODE_PRIVATE)
        val editor = preferences.edit()
        if (providersPresent) editor.putString("providers_llm", providersRaw) else editor.remove("providers_llm")
        if (audioRegionPresent) editor.putString("audio_provider_region", audioRegionRaw)
        else editor.remove("audio_provider_region")
        check(editor.commit()) { "The original provider and audio preference fields could not be restored." }
        check(preferences.contains("providers_llm") == providersPresent)
        check(preferences.getString("providers_llm", null) == providersRaw)
        check(preferences.contains("audio_provider_region") == audioRegionPresent)
        check(preferences.getString("audio_provider_region", null) == audioRegionRaw)
    }

    private fun rawProviderRowsWithFixture(originalRaw: String?, fixtureProvider: GenericProvider): String {
        val originalRows = JSONArray(originalRaw ?: "[]")
        check((0 until originalRows.length()).none {
            originalRows.getJSONObject(it).optString("id") == fixtureProvider.id
        }) { "The fixture provider ID already occurs in the raw provider preferences." }
        val fixtureRows = JSONArray(ProviderSettingsRepository.encodeProvidersJson(listOf(fixtureProvider)))
        check(fixtureRows.length() == 1)
        originalRows.put(fixtureRows.getJSONObject(0))
        return originalRows.toString()
    }

    private fun rawProviderRowsWithoutFixture(
        originalRaw: String?,
        providerId: String,
        requirePresent: Boolean = true,
    ): String? {
        if (originalRaw == null) return null
        val originalRows = JSONArray(originalRaw)
        val restoredRows = JSONArray()
        var removedCount = 0
        for (index in 0 until originalRows.length()) {
            val row = originalRows.getJSONObject(index)
            if (row.optString("id") == providerId) removedCount++ else restoredRows.put(row)
        }
        check(removedCount <= 1 && (!requirePresent || removedCount == 1)) {
            "The exact interrupted fixture ID must occur at most once in raw provider preferences."
        }
        if (removedCount == 0) return originalRaw
        return restoredRows.toString()
    }

    private fun providerRowsFromFreshPreferencesFile(): String? {
        val prefsFile = File(context.applicationContext.dataDir, "shared_prefs/provider_settings.xml")
        check(prefsFile.isFile) { "The Android provider preferences file is missing after cleanup." }
        val parser = android.util.Xml.newPullParser()
        prefsFile.inputStream().buffered().use { stream ->
            parser.setInput(stream, "UTF-8")
            while (parser.next() != org.xmlpull.v1.XmlPullParser.END_DOCUMENT) {
                if (parser.eventType == org.xmlpull.v1.XmlPullParser.START_TAG &&
                    parser.name == "string" && parser.getAttributeValue(null, "name") == "providers_llm"
                ) {
                    return parser.nextText()
                }
            }
        }
        return null
    }

    private fun executeShell(command: String): String {
        val descriptor = instrumentation.uiAutomation.executeShellCommand(command)
        return ParcelFileDescriptor.AutoCloseInputStream(descriptor).bufferedReader().use { it.readText() }
    }

    private fun dumpsysRecord(dump: String, header: String, matches: (String) -> Boolean): String? {
        val lines = dump.lines()
        val starts = lines.indices.filter { lines[it].contains(header) }
        for ((index, start) in starts.withIndex()) {
            val end = starts.getOrNull(index + 1) ?: lines.size
            val block = lines.subList(start, end).joinToString("\n").trim()
            if (matches(block)) return block
        }
        return null
    }

    private data class NativeWakeEvidence(
        val uri: Uri,
        val resolvedActivity: String,
        val packageAtMainActivity: String,
        val activityDump: String,
        val mainActivityObservedElapsedMs: Long,
        val browserReturnElapsedMs: Long,
    )

    private class SkillBrowserDiagnosticException(
        val phase: String,
        val code: String,
        val errorName: String,
        val taskState: String,
        val taskErrorCode: String,
        val eventSummary: String,
        val lastReplay: String,
    ) : IllegalStateException(
        "Browser-side acceptance failed: phase=$phase, code=$code, errorName=$errorName, " +
            "taskState=$taskState, taskErrorCode=$taskErrorCode, eventSummary=$eventSummary, lastReplay=$lastReplay",
    )

    private data class SkillBackgroundSnapshot(
        val requestElapsedMs: Long,
        val runState: JSONObject,
        val runStateReadElapsedMs: Long,
        val hostActivityResumed: Boolean,
        val serviceRecord: String,
        val notificationRecord: String,
        val notificationForegroundFlag: Boolean,
        val activityFile: File,
        val serviceFile: File,
        val notificationFile: File,
    )

    private data class SkillEvidenceContext(
        val apps: UnifiedLocalApps,
        val app: AppRecord,
        val release: AppRelease,
        val savedSettings: JSONObject,
        val launch: AppLaunch,
        val providerBaseUrl: String,
        val providerModel: String,
        val skillId: String,
        val skillNonce: String,
        val skillFileSha256: String,
        val skillNonceSha256: String,
        val unlistedSkillId: String,
        val unlistedSkillFileSha256: String,
        val unlistedSkillNonceSha256: String,
        val prompt: String,
        val outputSchema: JSONObject,
        val chatTitle: String,
        val reportKey: String,
        val runId: String,
        val browserPackage: String,
        val nativeWake: NativeWakeEvidence,
        val background: SkillBackgroundSnapshot,
    )

    private data class SkillFixtureResources(
        val apps: UnifiedLocalApps,
        val sources: MutableList<File>,
        val ids: MutableList<String>,
        val providerSettings: ProviderSettingsRepository,
        val originalProviders: List<GenericProvider>,
        val originalDefaultModel: String,
        val providerBaseUrl: String,
        val fixtureProvider: GenericProvider,
        val fixtureJournal: File,
        val originalProvidersLlmPresent: Boolean,
        val originalProvidersLlmRaw: String?,
        val originalAudioProviderRegionPresent: Boolean,
        val originalAudioProviderRegionRaw: String?,
        var credentialSource: com.lingxi.code.conversation.EngineConversationSource? = null,
        var credentialWriteAttempted: Boolean = false,
        var appAgentHostTouched: Boolean = false,
    )

    private data class SkillAcceptanceScenario(
        val skillId: String,
        val skillNonce: String,
        val skillBody: String,
        val skillFileSha256: String,
        val skillNonceSha256: String,
        val unlistedSkillId: String,
        val unlistedSkillNonce: String,
        val unlistedSkillBody: String,
        val unlistedSkillFileSha256: String,
        val unlistedSkillNonceSha256: String,
        val prompt: String,
        val outputSchema: JSONObject,
        val chatTitle: String,
        val runId: String,
        val reportKey: String,
    )

    private data class PreparedSkillApp(
        val app: AppRecord,
        val release: AppRelease,
        val savedSettings: JSONObject,
        val browserPackages: Set<String>,
    )

    private suspend fun nativeWakeAndReturnToBrowser(
        appId: String,
        revision: String,
        runId: String,
        browserPackages: Set<String>,
        dumpDirectory: File,
    ): NativeWakeEvidence {
        val wakeUri = Uri.Builder().scheme("lingxi").authority("apps").appendPath(appId)
            .appendQueryParameter("review", "agent")
            .appendQueryParameter("runId", runId)
            .appendQueryParameter("revision", revision)
            .build()
        val wakeIntent = Intent(Intent.ACTION_VIEW, wakeUri)
            .setPackage(context.packageName)
            .addCategory(Intent.CATEGORY_BROWSABLE)
            .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_CLEAR_TOP or Intent.FLAG_ACTIVITY_SINGLE_TOP)
        val resolvedWakeActivity = context.packageManager.resolveActivity(wakeIntent, 0)?.activityInfo
            ?: error("The native Local App wake URI does not resolve through Android.")
        assertEquals("The native wake URI must resolve to the installed LingXi app",
            context.packageName, resolvedWakeActivity.packageName)
        assertEquals("The native wake URI must resolve to LingXi MainActivity",
            MainActivity::class.java.name, resolvedWakeActivity.name)
        File(dumpDirectory, "native-wake-uri.txt").writeText(wakeUri.toString())
        val wakeStartedAt = android.os.SystemClock.elapsedRealtime()
        context.startActivity(wakeIntent)

        var foregroundPackage = ""
        var packageAtMainActivity = ""
        var activityDump = ""
        var mainActivityObserved = false
        var mainActivityObservedElapsedMs = -1L
        var browserReturnElapsedMs = -1L
        var browserReturned = false
        val returnedToBrowser = withTimeoutOrNull(90_000) {
            while (true) {
                val root = instrumentation.uiAutomation.rootInActiveWindow
                foregroundPackage = root?.packageName?.toString().orEmpty()
                if (foregroundPackage == context.packageName) {
                    if (!mainActivityObserved) {
                        activityDump = executeShell("dumpsys activity activities")
                        val resumedActivityLines = activityDump.lineSequence().filter {
                            it.contains("mResumedActivity") || it.contains("topResumedActivity") ||
                                it.contains("Resumed Activity")
                        }
                        val mainActivityComponent = "${context.packageName}/${MainActivity::class.java.name}"
                        if (resumedActivityLines.any { it.contains(mainActivityComponent) }) {
                            mainActivityObserved = true
                            packageAtMainActivity = foregroundPackage
                            mainActivityObservedElapsedMs = android.os.SystemClock.elapsedRealtime() - wakeStartedAt
                        }
                    }
                    val text = visibleUiText(root)
                    if (text.contains("本地应用")) {
                        var action = findAccessibilityNodeByText(root, "返回浏览器")
                        while (action != null && !action.isClickable) action = action.parent
                        action?.performAction(android.view.accessibility.AccessibilityNodeInfo.ACTION_CLICK)
                    }
                }
                if (mainActivityObserved && foregroundPackage in browserPackages) {
                    browserReturned = true
                    browserReturnElapsedMs = android.os.SystemClock.elapsedRealtime() - wakeStartedAt
                    return@withTimeoutOrNull true
                }
                delay(250)
            }
            false
        }
        assertTrue("The native wake route must return to the actual external browser",
            returnedToBrowser == true && browserReturned && mainActivityObserved)
        assertTrue("The native wake URI must display MainActivity before returning to the browser",
            activityDump.contains(MainActivity::class.java.name))
        return NativeWakeEvidence(
            uri = wakeUri,
            resolvedActivity = resolvedWakeActivity.name,
            packageAtMainActivity = packageAtMainActivity,
            activityDump = activityDump,
            mainActivityObservedElapsedMs = mainActivityObservedElapsedMs,
            browserReturnElapsedMs = browserReturnElapsedMs,
        )
    }

    private suspend fun captureSkillBackgroundSnapshot(
        launch: AppLaunch,
        runId: String,
        providerBaseUrl: String,
        browserPackage: String,
        native: NativeAppActivityHost,
        skillNonceSha256: String,
        unlistedSkillNonceSha256: String,
        dumpDirectory: File,
    ): SkillBackgroundSnapshot {
        suspend fun currentFixtureRequest(): JSONObject? {
            val state = fixtureState(providerBaseUrl)
            val rows = state.getJSONArray("requests")
            return (0 until rows.length()).map { rows.getJSONObject(it) }
                .firstOrNull { it.optString("marker") == "SKILL_BACKGROUND_WAKE_PROBE" }
        }

        val observedRequest = withTimeout(45_000) {
            while (true) {
                val row = currentFixtureRequest()
                if (row != null) return@withTimeout row
                delay(300)
            }
            error("The deterministic provider did not receive the Skill task.")
        }
        assertTrue("The only source of the test nonce must be the materialized Skill instructions",
            observedRequest.optBoolean("skillNoncePresent"))
        assertEquals(skillNonceSha256, observedRequest.optString("skillNonceSha256"))
        val observedNonceDigests = observedRequest.getJSONArray("skillNonceSha256s")
        assertEquals("Only the selected immutable Skill nonce may reach the provider", 1, observedNonceDigests.length())
        assertEquals(skillNonceSha256, observedNonceDigests.getString(0))
        assertFalse((0 until observedNonceDigests.length()).any {
            observedNonceDigests.getString(it) == unlistedSkillNonceSha256
        })
        assertFalse("The submitted user message must not contain the Skill nonce",
            observedRequest.optBoolean("userMessageContainsSkillNonce"))
        assertTrue("The provider must receive the host-owned StructuredOutput tool",
            observedRequest.optBoolean("structuredOutputToolPresent"))
        assertTrue("The provider request must still be pending before the native wake",
            observedRequest.isNull("status"))

        val twentySecondRequest = withTimeout(70_000) {
            while (true) {
                val row = currentFixtureRequest() ?: error("The deterministic provider request disappeared.")
                if (row.optLong("elapsedMs") >= 22_000) {
                    assertTrue("The delayed model request unexpectedly completed before the 20-second snapshot",
                        row.isNull("status"))
                    return@withTimeout row
                }
                delay(750)
            }
            error("unreachable")
        }
        val runStateReadStartedAt = android.os.SystemClock.elapsedRealtime()
        val runStateAtSnapshot = taskGet(launch, runId)
        val runStateReadElapsedMs = android.os.SystemClock.elapsedRealtime() - runStateReadStartedAt
        assertEquals("running", runStateAtSnapshot.getString("state"))
        assertEquals(browserPackage, instrumentation.uiAutomation.rootInActiveWindow?.packageName?.toString())
        val hostActivityResumedAtSnapshot = native.isResumed()
        assertFalse("The native Host Activity must remain paused during the 22-second browser background interval",
            hostActivityResumedAtSnapshot)

        val activityDump = executeShell("dumpsys activity activities")
        val serviceDump = executeShell("dumpsys activity services")
        val notificationDump = executeShell("dumpsys notification")
        val resumedLines = activityDump.lineSequence().filter {
            it.contains("mResumedActivity") || it.contains("topResumedActivity") || it.contains("Resumed Activity")
        }.toList()
        assertTrue("dumpsys activity must record the selected external browser as visible: $resumedLines",
            resumedLines.any { it.contains(browserPackage) } || activityDump.contains(browserPackage))
        val serviceRecord = dumpsysRecord(serviceDump, "ServiceRecord") {
            it.contains(context.packageName) && it.contains("LocalAppProcessService")
        } ?: error("dumpsys activity services omitted the LingXi LocalAppProcessService record.")
        val notificationRecord = dumpsysRecord(notificationDump, "NotificationRecord") { block ->
            val key = "|${context.packageName}|7242|"
            (block.contains("pkg=${context.packageName}") || block.contains(key)) &&
                (Regex("\\bid=7242\\b").containsMatchIn(block) || block.contains(key))
        } ?: error("dumpsys notification omitted LingXi's own foreground-service notification id 7242.")
        val ownForegroundNotifications = context.getSystemService(android.app.NotificationManager::class.java)
            .activeNotifications.filter { it.packageName == context.packageName && it.id == 7242 }
        assertEquals("The Android NotificationManager must report exactly this app's running-app notification",
            1, ownForegroundNotifications.size)
        val foregroundFlagPresent = ownForegroundNotifications.single().notification.flags and
            android.app.Notification.FLAG_FOREGROUND_SERVICE != 0
        assertTrue("The same LingXi notification record must be marked as foreground-service notification",
            foregroundFlagPresent || notificationRecord.contains("FOREGROUND_SERVICE"))

        val activityFile = File(dumpDirectory, "activity-at-22s.txt")
            .apply { writeText(resumedLines.joinToString("\n")) }
        val serviceFile = File(dumpDirectory, "services-at-22s.txt").apply { writeText(serviceRecord) }
        val notificationFile = File(dumpDirectory, "notification-at-22s.txt").apply { writeText(notificationRecord) }
        return SkillBackgroundSnapshot(
            requestElapsedMs = twentySecondRequest.optLong("elapsedMs"),
            runState = runStateAtSnapshot,
            runStateReadElapsedMs = runStateReadElapsedMs,
            hostActivityResumed = hostActivityResumedAtSnapshot,
            serviceRecord = serviceRecord,
            notificationRecord = notificationRecord,
            notificationForegroundFlag = foregroundFlagPresent,
            activityFile = activityFile,
            serviceFile = serviceFile,
            notificationFile = notificationFile,
        )
    }

    private suspend fun verifySkillRecoveryAndWriteEvidence(input: SkillEvidenceContext) {
        val browserMarker = awaitBrowserReport(
            input.launch,
            input.reportKey,
            input.runId,
            timeoutMs = 120_000,
        )
        val task = awaitTerminal(input.launch, input.runId, 120_000)
        assertEquals(task.toString(), "succeeded", task.getString("state"))
        val eventPage = agentCall(input.launch, "agent.tasks.events", JSONObject().put("runId", input.runId)) as JSONObject
        val events = eventPage.getJSONArray("events")
        val startedEvents = (0 until events.length()).map { events.getJSONObject(it) }
            .filter { it.optString("type") == "RUN_STARTED" }
        val terminal = events.getJSONObject(events.length() - 1)
        assertEquals("A browser reload must recover the existing run without a second start", 1, startedEvents.size)
        assertEquals(input.runId, startedEvents.single().getString("runId"))
        val startedInput = startedEvents.single().getJSONObject("input")
        val submittedOptions = startedInput.getJSONObject("forwardedProps").getJSONObject("com.lingxi.localApp")
        assertEquals(input.skillId, submittedOptions.getJSONArray("skills").getString(0))
        assertEquals(1, submittedOptions.getJSONArray("skills").length())
        assertFalse("The submitted AG-UI user input must not reveal the Skill nonce",
            startedInput.toString().contains(input.skillNonce))
        assertEquals("RUN_FINISHED", terminal.getString("type"))
        assertEquals(input.runId, terminal.getString("runId"))
        val result = terminal.getJSONObject("result")
        assertEquals(input.skillNonce, result.getString("answer"))
        assertEquals("The browser-side read-only recovery marker correlates this public run",
            "agui:p:${input.reportKey}:${input.runId}", browserMarker.getString("title"))
        val chatPage = agentCall(input.launch, "agent.chats.list", JSONObject().put("limit", 50)) as JSONObject
        val chatRows = (0 until chatPage.getJSONArray("chats").length()).map {
            chatPage.getJSONArray("chats").getJSONObject(it)
        }
        val chat = chatRows.single { it.optString("title") == input.chatTitle }
        assertFalse(chat.getString("title").contains(input.skillNonce))
        assertEquals(task.getString("threadId"), chat.getString("chatId"))
        val history = agentCall(input.launch, "agent.chats.history",
            JSONObject().put("chatId", chat.getString("chatId")).put("limit", 50)) as JSONObject
        val historyMessages = history.getJSONArray("messages")
        assertTrue((0 until historyMessages.length()).any {
            historyMessages.getJSONObject(it).optString("role") == "assistant"
        })

        val finalFixtureState = fixtureState(input.providerBaseUrl)
        val finalRows = finalFixtureState.getJSONArray("requests")
        val providerCalls = (0 until finalRows.length()).map { finalRows.getJSONObject(it) }
            .filter { it.optString("marker") == "SKILL_BACKGROUND_WAKE_PROBE" }
        assertEquals("The same public run must issue one provider request", 1, providerCalls.size)
        assertEquals(200, providerCalls.single().getInt("status"))
        assertEquals(input.skillNonceSha256, providerCalls.single().optString("skillNonceSha256"))
        val finalNonceDigests = providerCalls.single().getJSONArray("skillNonceSha256s")
        assertEquals(1, finalNonceDigests.length())
        assertFalse((0 until finalNonceDigests.length()).any {
            finalNonceDigests.getString(it) == input.unlistedSkillNonceSha256
        })

        val evidence = JSONObject()
            .put("acceptance", "ANDROID_SKILL_BACKGROUND_WAKE_PASS")
            .put("device", JSONObject()
                .put("serial", InstrumentationRegistry.getArguments().getString("deviceSerial").orEmpty())
                .put("model", android.os.Build.MODEL)
                .put("androidRelease", android.os.Build.VERSION.RELEASE))
            .put("productRevision", "runtime-supplied-by-runner")
            .put("sdkRevision", "runtime-supplied-by-runner")
            .put("appId", input.app.id)
            .put("releaseRevision", input.release.revision)
            .put("releaseDigest", input.release.digest)
            .put("skillId", input.skillId)
            .put("skillPath", "skills/${input.skillId}/SKILL.md")
            .put("skillFileSha256", input.skillFileSha256)
            .put("skillNonceSha256", input.skillNonceSha256)
            .put("skillNonce", input.skillNonce)
            .put("unlistedNeighborSkillId", input.unlistedSkillId)
            .put("unlistedNeighborSkillPath", "skills/${input.unlistedSkillId}/SKILL.md")
            .put("unlistedNeighborSkillFileSha256", input.unlistedSkillFileSha256)
            .put("unlistedNeighborSkillNonceSha256", input.unlistedSkillNonceSha256)
            .put("unlistedNeighborSkillDeclared", false)
            .put("unlistedNeighborSkillNonceObservedByProvider", false)
            .put("skillApprovedByNativeHost", input.savedSettings.getJSONArray("skills").length() == 1)
            .put("prompt", input.prompt)
            .put("outputSchema", input.outputSchema)
            .put("chatTitle", input.chatTitle)
            .put("browserPackage", input.browserPackage)
            .put("publicRunId", input.runId)
            .put("chatId", chat.getString("chatId"))
            .put("taskState", task.getString("state"))
            .put("result", result)
            .put("provider", JSONObject()
                .put("type", "deterministic-local-test-provider")
                .put("modelId", input.providerModel)
                .put("requestCount", providerCalls.size)
                .put("skillNonceObservedInActualModelRequest", providerCalls.single().optBoolean("skillNoncePresent"))
                .put("skillNonceSha256", providerCalls.single().optString("skillNonceSha256"))
                .put("skillNonceSha256s", providerCalls.single().getJSONArray("skillNonceSha256s"))
                .put("unlistedNeighborNonceObservedInActualModelRequest",
                    (0 until providerCalls.single().getJSONArray("skillNonceSha256s").length()).any {
                        providerCalls.single().getJSONArray("skillNonceSha256s").getString(it) == input.unlistedSkillNonceSha256
                    })
                .put("userMessageContainsSkillNonce", providerCalls.single().optBoolean("userMessageContainsSkillNonce"))
                .put("structuredOutputToolPresent", providerCalls.single().optBoolean("structuredOutputToolPresent"))
                .put("requestElapsedAtBackgroundSnapshotMs", input.background.requestElapsedMs)
                .put("providerResponseStatusAtSnapshot", JSONObject.NULL)
                .put("providerResponseStatusAtCompletion", providerCalls.single().getInt("status")))
            .put("pageValidatedCallbackContract", JSONObject()
                .put("firstCallbackRequired", "RUN_STARTED")
                .put("lastCallbackRequired", "RUN_FINISHED")
                .put("runErrorCallbackRejected", true)
                .put("successMarkerCreatedAfterValidation", browserMarker.getString("title")))
            .put("publicEventTypes", JSONArray((0 until events.length()).map { events.getJSONObject(it).getString("type") }))
            .put("runStartedCount", startedEvents.size)
            .put("browserReadOnlyRecoveryMarker", browserMarker.getString("title"))
            .put("nativeWake", JSONObject()
                .put("uri", input.nativeWake.uri.toString())
                .put("resolvedActivity", input.nativeWake.resolvedActivity)
                .put("foregroundPackageAtMainActivity", input.nativeWake.packageAtMainActivity)
                .put("mainActivityObservedElapsedMs", input.nativeWake.mainActivityObservedElapsedMs)
                .put("browserReturnElapsedMs", input.nativeWake.browserReturnElapsedMs)
                .put("mainActivityObservedInDumpsys", input.nativeWake.activityDump.contains("MainActivity")))
            .put("backgroundSnapshot", JSONObject()
                .put("hostAcceptanceActivityResumed", input.background.hostActivityResumed)
                .put("windowPackage", input.browserPackage)
                .put("foregroundServicePresent", input.background.serviceRecord.contains("LocalAppProcessService"))
                .put("notificationRecordPackage", context.packageName)
                .put("notificationRecordId", 7242)
                .put("notificationForegroundFlag", input.background.notificationForegroundFlag)
                .put("elapsedMs", input.background.requestElapsedMs)
                .put("runStateReadOperation", "agent.tasks.get")
                .put("runStateReadElapsedMs", input.background.runStateReadElapsedMs)
                .put("runStateAtSnapshot", input.background.runState.getString("state"))
                .put("files", JSONObject()
                    .put("activity", input.background.activityFile.name)
                    .put("activitySha256", sha256(input.background.activityFile.readBytes()))
                    .put("service", input.background.serviceFile.name)
                    .put("serviceSha256", sha256(input.background.serviceFile.readBytes()))
                    .put("notification", input.background.notificationFile.name)
                    .put("notificationSha256", sha256(input.background.notificationFile.readBytes()))))
        File(context.filesDir, "ag-ui-android-skill-background-wake.json").writeText(evidence.toString(2))
        val screenshot = instrumentation.uiAutomation.takeScreenshot()
            ?: error("Could not capture the returned external browser Skill Chat.")
        File(context.filesDir, "ag-ui-android-skill-background-wake.png").outputStream().use {
            check(screenshot.compress(android.graphics.Bitmap.CompressFormat.PNG, 100, it))
        }
        screenshot.recycle()
        android.util.Log.i("LocalAppAgentAcceptance",
            "SKILL_BACKGROUND_WAKE PASS provider=deterministic nonceObserved=true elapsedMs=${input.background.requestElapsedMs} runId=${input.runId} readOnlyRecovery=true")
    }

    private suspend fun captureSkillFailureEvidence(
        launch: AppLaunch?,
        scenario: SkillAcceptanceScenario,
        dumpDirectory: File,
        failure: Throwable,
    ) = withContext(NonCancellable) {
        fun safeCode(error: Throwable): String = when (error) {
            is SkillBrowserDiagnosticException -> error.code
            is AppServiceException -> error.code
            else -> error.javaClass.simpleName
        }
        val evidence = JSONObject()
            .put("schemaVersion", 1)
            .put("scope", "exact-skill-run-failure-read-only")
            .put("runIdSha256", sha256(scenario.runId.toByteArray()))
            .put("capturedAtEpochMs", System.currentTimeMillis())
            .put("primaryExceptionClass", failure.javaClass.simpleName)
            .put("primaryErrorCode", safeCode(failure))
            .put("primaryMessageRecorded", false)
            .put("suppressedExceptions", JSONArray(failure.suppressed.map { suppressed ->
                JSONObject().put("class", suppressed.javaClass.simpleName).put("code", safeCode(suppressed))
            }))
        if (failure is SkillBrowserDiagnosticException) {
            evidence.put("browserPage", JSONObject()
                .put("phase", failure.phase)
                .put("code", failure.code)
                .put("errorName", failure.errorName)
                .put("taskState", failure.taskState)
                .put("taskErrorCode", failure.taskErrorCode)
                .put("eventSummary", failure.eventSummary)
                .put("lastReplay", failure.lastReplay))
        }

        if (launch == null) {
            evidence.put("taskQueryAttempted", false).put("eventQueryAttempted", false)
        } else {
            evidence.put("taskQueryAttempted", true).put("eventQueryAttempted", true)
            try {
                val task = withTimeout(5_000) { taskGet(launch, scenario.runId) }
                val error = task.optJSONObject("error")
                val result = task.optJSONObject("result")
                val answer = result?.opt("answer") as? String
                evidence.put("task", JSONObject()
                    .put("runIdSha256", sha256(task.optString("runId").toByteArray()))
                    .put("state", task.optString("state", "unknown"))
                    .put("phase", task.optString("phase", ""))
                    .put("updatedAt", task.optLong("updatedAt", -1))
                    .put("errorPresent", error != null)
                    .put("errorCode", error?.optString("code")?.takeIf { it.isNotBlank() } ?: JSONObject.NULL)
                    .put("resultPresent", result != null)
                    .put("resultFields", JSONArray(result?.keys()?.asSequence()?.toList()?.sorted().orEmpty()))
                    .put("answerBytes", answer?.toByteArray()?.size ?: JSONObject.NULL)
                    .put("answerSha256", answer?.let { sha256(it.toByteArray()) } ?: JSONObject.NULL))
            } catch (queryFailure: Throwable) {
                evidence.put("taskQueryErrorCode", safeCode(queryFailure))
            }
            try {
                val page = withTimeout(5_000) {
                    agentCall(launch, "agent.tasks.events", JSONObject().put("runId", scenario.runId)) as JSONObject
                }
                val events = page.optJSONArray("events") ?: JSONArray()
                val timeline = JSONArray()
                for (index in 0 until events.length()) {
                    val event = events.optJSONObject(index) ?: continue
                    val eventId = event.optString("id", event.optString("eventId", ""))
                    val body = event.optJSONObject("event")
                    timeline.put(JSONObject()
                        .put("type", event.optString("type", body?.optString("type") ?: "unknown"))
                        .put("sequence", event.optLong("sequence", -1))
                        .put("eventIdSha256", eventId.takeIf { it.isNotBlank() }?.let { sha256(it.toByteArray()) } ?: JSONObject.NULL)
                        .put("timestamp", event.optString("timestamp", event.optString("createdAt", event.optString("recordedAt", ""))))
                        .put("errorCode", event.optJSONObject("error")?.optString("code") ?: JSONObject.NULL))
                }
                evidence.put("eventCount", timeline.length()).put("eventTimeline", timeline)
            } catch (queryFailure: Throwable) {
                evidence.put("eventQueryErrorCode", safeCode(queryFailure))
            }
        }
        File(dumpDirectory, "skill-failure-readonly.json").writeText(evidence.toString(2))
    }

    private fun splitSkillSections(skill: String): Pair<String, String> {
        check(skill.startsWith("---\n")) { "Acceptance Skill is missing its opening frontmatter delimiter." }
        val closingDelimiter = skill.indexOf("\n---\n", startIndex = 4)
        check(closingDelimiter >= 0) { "Acceptance Skill is missing its closing frontmatter delimiter." }
        return skill.substring(4, closingDelimiter) to skill.substring(closingDelimiter + "\n---\n".length)
    }

    private fun createSkillAcceptanceScenario(): SkillAcceptanceScenario {
        val skillId = "acceptance-" + UUID.randomUUID().toString().replace("-", "").take(12)
        val skillNonce = "SKILL_NONCE_" + UUID.randomUUID().toString().replace("-", "").uppercase()
        val skillDescription = "Return the exact value in this Skill when the user requests acceptance verification."
        val unlistedSkillId = "neighbor-" + UUID.randomUUID().toString().replace("-", "").take(12)
        val unlistedSkillNonce = "SKILL_NONCE_" + UUID.randomUUID().toString().replace("-", "").uppercase()
        val unlistedSkillDescription = "An unlisted package file that this request must not load."
        val unlistedSkillBody = """
            ---
            name: $unlistedSkillId
            description: $unlistedSkillDescription
            ---
            This unlisted neighbor Skill contains a separate value which must not be loaded for the approved Skill request.

            $unlistedSkillNonce
        """.trimIndent()
        val (unlistedSkillFrontMatter, unlistedSkillInstructions) = splitSkillSections(unlistedSkillBody)
        val skillBody = """
            ---
            name: $skillId
            description: $skillDescription
            ---
            When asked to execute the approved acceptance skill for this app, return exactly the token below as the structured answer.
            Do not infer, alter, or add words to the token.

            $skillNonce
        """.trimIndent()
        val (skillFrontMatter, skillInstructions) = splitSkillSections(skillBody)
        assertFalse(skillFrontMatter.contains(skillNonce))
        assertTrue(skillInstructions.contains(skillNonce))
        assertEquals(1, skillBody.split(skillNonce).size - 1)
        assertFalse(unlistedSkillFrontMatter.contains(unlistedSkillNonce))
        assertTrue(unlistedSkillInstructions.contains(unlistedSkillNonce))
        assertEquals(1, unlistedSkillBody.split(unlistedSkillNonce).size - 1)
        val prompt = "SKILL_BACKGROUND_WAKE_PROBE: Run the approved acceptance skill declared by this app. Return its exact value as the answer."
        val outputSchema = JSONObject("""{"type":"object","properties":{"answer":{"type":"string"}},"required":["answer"],"additionalProperties":false}""")
        val chatTitle = "AG-UI Skill acceptance " + UUID.randomUUID()
        val runId = "external-skill-wake-" + UUID.randomUUID()
        val reportKey = UUID.randomUUID().toString().replace("-", "").take(8)
        assertFalse(prompt.contains(skillNonce))
        assertFalse(outputSchema.toString().contains(skillNonce))
        assertFalse(chatTitle.contains(skillNonce))
        assertFalse(skillDescription.contains(skillNonce))
        assertFalse(prompt.contains(unlistedSkillNonce))
        assertFalse(outputSchema.toString().contains(unlistedSkillNonce))
        assertFalse(chatTitle.contains(unlistedSkillNonce))
        assertFalse(unlistedSkillDescription.contains(unlistedSkillNonce))
        return SkillAcceptanceScenario(
            skillId = skillId,
            skillNonce = skillNonce,
            skillBody = skillBody,
            skillFileSha256 = sha256(skillBody.toByteArray(Charsets.UTF_8)),
            skillNonceSha256 = sha256(skillNonce.toByteArray(Charsets.UTF_8)),
            unlistedSkillId = unlistedSkillId,
            unlistedSkillNonce = unlistedSkillNonce,
            unlistedSkillBody = unlistedSkillBody,
            unlistedSkillFileSha256 = sha256(unlistedSkillBody.toByteArray(Charsets.UTF_8)),
            unlistedSkillNonceSha256 = sha256(unlistedSkillNonce.toByteArray(Charsets.UTF_8)),
            prompt = prompt,
            outputSchema = outputSchema,
            chatTitle = chatTitle,
            runId = runId,
            reportKey = reportKey,
        )
    }

    private suspend fun recoverInterruptedSkillFixture(journal: File) {
        if (!journal.isFile) return
        val interruptedProviderId = JSONObject(journal.readText()).getString("providerId")
        check(interruptedProviderId.startsWith("agui-fixture-")) {
            "The interrupted Skill fixture journal contains an unexpected provider ID."
        }
        removeInterruptedSkillFixtureProvider(interruptedProviderId)
        check(journal.delete() || !journal.exists()) {
            "The interrupted Skill fixture journal could not be cleared after secure cleanup."
        }
    }

    private fun createSkillFixtureResources(providerBaseUrl: String, providerModel: String, journal: File): SkillFixtureResources {
        val preferences = context.getSharedPreferences("provider_settings", android.content.Context.MODE_PRIVATE)
        val originalProvidersLlmPresent = preferences.contains("providers_llm")
        val originalProvidersLlmRaw = preferences.getString("providers_llm", null)
        val originalAudioProviderRegionPresent = preferences.contains("audio_provider_region")
        val originalAudioProviderRegionRaw = preferences.getString("audio_provider_region", null)
        check(!originalProvidersLlmPresent || originalProvidersLlmRaw != null) {
            "The existing provider rows are not stored as a string and cannot be snapshotted safely."
        }
        check(!originalAudioProviderRegionPresent || originalAudioProviderRegionRaw != null) {
            "The existing audio region preference is not stored as a string and cannot be snapshotted safely."
        }
        val providerSettings = ProviderSettingsRepository(context)
        try {
            val originalProviders = providerSettings.loadProviderState().first
            val originalDefaultModel = providerSettings.engineLaunchConfig().defaultModel
            check(originalProviders.none { it.id.startsWith("agui-fixture-") }) {
                "An interrupted test fixture provider remains; run the exact fixture credential cleanup first."
            }
            val providerId = "agui-fixture-" + UUID.randomUUID().toString().replace("-", "").take(16)
            check(originalProviders.none { it.id == providerId }) {
                "The random test provider ID unexpectedly collides with a saved provider."
            }
            return SkillFixtureResources(
                apps = UnifiedLocalApps.get(context),
                sources = mutableListOf(),
                ids = mutableListOf(),
                providerSettings = providerSettings,
                originalProviders = originalProviders,
                originalDefaultModel = originalDefaultModel,
                providerBaseUrl = providerBaseUrl,
                fixtureProvider = GenericProvider(
                    id = providerId,
                    preset = "custom",
                    name = "Local App acceptance fixture",
                    url = providerBaseUrl,
                    key = "",
                    model = providerModel,
                    status = ConnStatus.Configured,
                    enabled = true,
                    credentialConfigured = true,
                    showInModelPicker = false,
                ),
                fixtureJournal = journal,
                originalProvidersLlmPresent = originalProvidersLlmPresent,
                originalProvidersLlmRaw = originalProvidersLlmRaw,
                originalAudioProviderRegionPresent = originalAudioProviderRegionPresent,
                originalAudioProviderRegionRaw = originalAudioProviderRegionRaw,
            )
        } catch (failure: Throwable) {
            providerSettings.close()
            throw failure
        }
    }

    private suspend fun configureSkillFixtureProvider(resources: SkillFixtureResources, providerKey: String) {
        check(resources.originalDefaultModel.isNotBlank()) {
            "Configure a host default model before running the test-only provider acceptance."
        }
        check(!resources.fixtureJournal.exists()) { "A Skill fixture journal already exists before provider configuration." }
        resources.fixtureJournal.writeText(JSONObject().put("providerId", resources.fixtureProvider.id).toString())
        resources.providerSettings.persistProviders(ProviderKind.Llm, resources.originalProviders + resources.fixtureProvider)
        persistProviderRowsForAcceptance(resources.originalProviders + resources.fixtureProvider)
        val providerRowsWithFixture = rawProviderRowsWithFixture(resources.originalProvidersLlmRaw, resources.fixtureProvider)
        check(context.getSharedPreferences("provider_settings", android.content.Context.MODE_PRIVATE)
            .edit().putString("providers_llm", providerRowsWithFixture).commit()) {
            "The synthetic provider row could not be added while preserving unknown original provider fields."
        }
        assertEquals(
            ProviderSettingsRepository.encodeProvidersJson(resources.originalProviders + resources.fixtureProvider),
            ProviderSettingsRepository.encodeProvidersJson(resources.providerSettings.loadProviderState().first),
        )
        resources.credentialSource = com.lingxi.code.conversation.EngineConversationSource.create(
            context = context,
            workspaceKey = "acceptance.skill-credential.${resources.fixtureProvider.id}",
            reuseProcessSource = false,
        ) as? com.lingxi.code.conversation.EngineConversationSource
            ?: error("Could not create an isolated native source for the synthetic fixture credential.")
        resources.providerSettings.bindSource(resources.credentialSource)
        resources.credentialWriteAttempted = true
        val credential = resources.providerSettings.setCredential(resources.fixtureProvider, providerKey)
        check(credential.error == null && credential.storageEncrypted &&
            resources.fixtureProvider.id in credential.configuredProviderIds &&
            resources.fixtureProvider.id !in credential.unavailableProviderIds) {
            "The synthetic acceptance provider credential was not stored in the Android secure credential service."
        }
        assertEquals("Adding the test-only provider must preserve the chat default model",
            resources.originalDefaultModel, resources.providerSettings.engineLaunchConfig().defaultModel)
    }

    private suspend fun publishAndConfigureSkillApp(
        resources: SkillFixtureResources,
        scenario: SkillAcceptanceScenario,
    ): PreparedSkillApp {
        val page = """
            <!doctype html><meta name="viewport" content="width=device-width,initial-scale=1"><title>Approved Skill acceptance</title>
            <style>body{font-family:system-ui;margin:20px}#result{white-space:pre-wrap;overflow-wrap:anywhere}</style>
            <h1>Approved Skill acceptance</h1><pre id="result">Waiting for the external browser and Host SDK</pre>
            <script type="module">
            (async()=>{
              const output=document.getElementById('result');
              const runId='${scenario.runId}';
              const skillId='${scenario.skillId}';
              const storageKey='lingxi-local-app-skill-wake-${scenario.runId}';
              let phase='B';
              let recoveredTaskState=null;
              let recoveredTaskErrorCode=null;
              let replayResultPresent=false;
              let replayCallbacks=[];
              let eventTimeline=[];
              function loadState(){try{return JSON.parse(localStorage.getItem(storageKey)||'{}');}catch(_error){return {};}}
              function saveState(value){localStorage.setItem(storageKey,JSON.stringify(value));}
              document.addEventListener('visibilitychange',()=>{
                if(document.visibilityState==='hidden'){
                  const value=loadState();value.sawHidden=true;saveState(value);
                }
              });
              try {
                await import('/__local_app_sdk.js');
                phase='S';
                let state=loadState();
                if(!state.chatId){
                  const created=await localApp.agent.chats.create({title:'${scenario.chatTitle}'});
                  state.chatId=created.id;
                  state.started=false;
                  state.runId=runId;
                  saveState(state);
                }
                const chat=await localApp.agent.chats.open(state.chatId);
                if(!state.started){
                  const input={
                    threadId:chat.id,
                    runId,
                    messages:[{id:'message-'+runId,role:'user',content:[{type:'text',text:'${scenario.prompt}'}]}],
                    forwardedProps:{'com.lingxi.localApp':{
                      version:1,
                      outputSchema:{type:'object',properties:{answer:{type:'string'}},required:['answer'],additionalProperties:false},
                      tools:[],skills:[skillId],maxTurns:8,timeoutMs:180000
                    }}
                  };
                  state.started=true;state.runId=runId;saveState(state);
                  phase='R';
                  const sent=await chat.send(input);
                  if(!sent.result||typeof sent.result.answer!=='string'||!Array.isArray(sent.newMessages)){
                    throw new Error('Persistent Host/Harness Chat did not return a structured Skill result.');
                  }
                }
                phase='W';
                while(document.visibilityState!=='visible'||loadState().sawHidden!==true){
                  await new Promise(resolve=>setTimeout(resolve,150));
                }
                phase='replay';
                const replay=await localApp.agent.createClient({threadId:chat.id}).connectAgent(
                  {runId},{onEvent({event}){replayCallbacks.push(event.type);}}
                );
                replayResultPresent=Boolean(replay.result);
                phase='task-get';
                const task=await localApp.agent.tasks.get(runId);
                recoveredTaskState=typeof task.state==='string'?task.state:'unknown';
                const taskError=task.error&&typeof task.error==='object'?task.error:null;
                recoveredTaskErrorCode=typeof taskError?.code==='string'?taskError.code.replace(/[^A-Za-z0-9_-]/g,'').slice(0,24):null;
                phase='events';
                const eventPage=await localApp.agent.tasks.events({runId});
                eventTimeline=eventPage.events.map(event=>({
                  sequence:Number.isSafeInteger(event.sequence)?event.sequence:null,
                  type:typeof event.type==='string'?event.type:'unknown',
                  timestamp:typeof event.timestamp==='string'?event.timestamp:
                    (typeof event.createdAt==='string'?event.createdAt:null),
                }));
                const eventTypes=eventTimeline.map(event=>event.type);
                const started=eventPage.events.filter(event=>event.type==='RUN_STARTED');
                const terminal=eventPage.events[eventPage.events.length-1];
                phase='validation';
                if(task.state!=='succeeded'||task.threadId!==chat.id||started.length!==1||
                   terminal?.type!=='RUN_FINISHED'||terminal.runId!==runId||
                   replayCallbacks[0]!=='RUN_STARTED'||replayCallbacks[replayCallbacks.length-1]!=='RUN_FINISHED'||
                   replayCallbacks.includes('RUN_ERROR')||!replay.result?.answer||
                   replay.result.answer!==terminal.result.answer){
                  throw new Error('Same-run read-only recovery did not replay one terminal Skill task.');
                }
                phase='history';
                const history=await chat.history({limit:50});
                if(!history.messages.some(message=>message.role==='assistant')){
                  throw new Error('Persistent Chat history did not retain an assistant turn.');
                }
                phase='V';
                output.textContent=JSON.stringify({acceptance:'SKILL_BACKGROUND_WAKE_PASS',runId,chatId:chat.id,
                  taskState:task.state,result:replay.result,replayCallbacks,eventTypes,historyRoles:history.messages.map(message=>message.role)});
                await new Promise(resolve=>requestAnimationFrame(()=>resolve()));
                phase='M';
                await localApp.agent.chats.create({title:'agui:p:${scenario.reportKey}:${scenario.runId}'});
              } catch(error) {
                const code=String(error&&typeof error.code==='string'?error.code:'browser_error').toLowerCase().replace(/[^a-z0-9_-]/g,'').slice(0,8)||'error';
                const errorName=String(error&&typeof error.name==='string'?error.name:'Error').replace(/[^A-Za-z0-9_.-]/g,'').slice(0,48);
                const safeState=['queued','running','succeeded','failed','cancelled','interrupted'].includes(recoveredTaskState)?recoveredTaskState:'unknown';
                const eventSummary=eventTimeline.slice(-3).map(event=>String(event.sequence??'x')+'-'+event.type.replace(/[^A-Z0-9_-]/g,'').slice(0,16)).join(',')||'none';
                const lastReplay=replayCallbacks[replayCallbacks.length-1]||'none';
                output.textContent='SKILL_WAKE_PAGE_DIAGNOSTIC '+JSON.stringify({phase,code,errorName,taskState:safeState,
                  taskErrorCode:recoveredTaskErrorCode,replayResultPresent,eventSummary,lastReplay});
                await new Promise(resolve=>requestAnimationFrame(()=>resolve()));
                try { if(window.localApp?.agent?.chats?.create) await localApp.agent.chats.create({title:'agui:e:${scenario.reportKey}:${scenario.runId}:'+phase+':'+code+':'+errorName+':'+safeState+':'+(recoveredTaskErrorCode||'none')+':'+eventSummary+':'+lastReplay}); } catch(_reportError) {}
              }
            })();
            </script>
        """.trimIndent()
        val app = publish(resources.apps, resources.sources, pageHtml = page,
            skillId = scenario.skillId, skillBody = scenario.skillBody,
            unlistedSkillId = scenario.unlistedSkillId, unlistedSkillBody = scenario.unlistedSkillBody)
        resources.ids.add(app.id)
        val release = resources.apps.release(app.id, app.currentRevision)
        val declaredSkills = release.manifest.getJSONObject("agent").getJSONArray("skills")
        assertEquals(1, declaredSkills.length())
        assertEquals(scenario.skillId, declaredSkills.getJSONObject(0).getString("id"))
        assertEquals("skills/${scenario.skillId}/SKILL.md", declaredSkills.getJSONObject(0).getString("path"))
        assertFalse(release.manifest.toString().contains(scenario.skillNonce))
        assertFalse(release.manifest.toString().contains(scenario.unlistedSkillNonce))
        assertFalse((0 until declaredSkills.length()).any {
            declaredSkills.getJSONObject(it).getString("id") == scenario.unlistedSkillId
        })
        assertFalse(app.name.contains(scenario.skillNonce))
        assertFalse(app.summary.contains(scenario.skillNonce))
        assertEquals("The immutable published Skill body matches its source",
            scenario.skillFileSha256, sha256(File(resources.sources.single(), "skills/${scenario.skillId}/SKILL.md").readBytes()))
        assertEquals("The unlisted neighbor file is packaged separately from the approved Skill",
            scenario.unlistedSkillFileSha256,
            sha256(File(resources.sources.single(), "skills/${scenario.unlistedSkillId}/SKILL.md").readBytes()))
        val immutableRelease = resources.apps.request("getRelease", app.id) { put("revision", app.currentRevision) } as JSONObject
        val immutableFiles = immutableRelease.getJSONArray("files")
        fun immutableFile(path: String): JSONObject = (0 until immutableFiles.length())
            .map { immutableFiles.getJSONObject(it) }.single { it.getString("path") == path }
        assertEquals(scenario.skillFileSha256, immutableFile("skills/${scenario.skillId}/SKILL.md").getString("sha256"))
        assertEquals(scenario.unlistedSkillFileSha256,
            immutableFile("skills/${scenario.unlistedSkillId}/SKILL.md").getString("sha256"))

        resources.appAgentHostTouched = true
        val configured = configure(
            resources.apps,
            app.id,
            photo = false,
            skills = setOf(scenario.skillId),
            selectedModel = JSONObject().put("providerId", resources.fixtureProvider.id)
                .put("modelId", resources.fixtureProvider.model),
        )
        assertEquals(resources.fixtureProvider.id, configured.getString("providerId"))
        assertEquals(resources.fixtureProvider.model, configured.getString("modelId"))
        val savedSettings = resources.apps.agentHost(JSONObject().put("op", "settings").put("appId", app.id)) as JSONObject
        assertEquals(1, savedSettings.getJSONArray("skills").length())
        assertEquals(scenario.skillId, savedSettings.getJSONArray("skills").getString(0))
        assertEquals("Native App Agent settings approve exactly the declared Skill",
            scenario.skillId, declaredSkills.getJSONObject(0).getString("id"))

        val browserPackages = LocalAppBrowser.installedHandlerPackages(context, "http://127.0.0.1/")
        assertTrue("Install or enable an HTTP browser before this acceptance", browserPackages.isNotEmpty())
        return PreparedSkillApp(app, release, savedSettings, browserPackages)
    }

    private suspend fun runSkillInExternalBrowserAndRecover(
        resources: SkillFixtureResources,
        scenario: SkillAcceptanceScenario,
        prepared: PreparedSkillApp,
    ) {
        val hostIntent = Intent().setClassName(context.packageName, "com.lingxi.code.apps.LocalAppAcceptanceActivity")
            .putExtra("appId", prepared.app.id).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
        val dumpDirectory = File(context.filesDir, "ag-ui-android-skill-wake-${scenario.runId}").apply {
            check(mkdirs() || isDirectory)
        }
        var launchForFailure: AppLaunch? = null
        try {
        NativeAppActivityHost(hostIntent).use { native ->
            var browserPackage = ""
            var consentClickAttempted = false
            val browserOpened = withTimeoutOrNull(60_000) {
                while (true) {
                    val root = instrumentation.uiAutomation.rootInActiveWindow
                    browserPackage = root?.packageName?.toString().orEmpty()
                    if (!consentClickAttempted && root != null) {
                        var action = findAccessibilityNodeByText(root, "允许并打开浏览器")
                        while (action != null && !action.isClickable) action = action.parent
                        if (action != null) {
                            consentClickAttempted = true
                            action.performAction(android.view.accessibility.AccessibilityNodeInfo.ACTION_CLICK)
                        }
                    }
                    if (browserPackage in prepared.browserPackages) return@withTimeoutOrNull true
                    delay(250)
                }
                false
            }
            assertTrue("The OS-selected external browser did not open: $browserPackage", browserOpened == true)
            assertFalse("The native Host Activity must remain paused while the OS browser is foreground", native.isResumed())
            assertTrue(LocalAppProcessService.owns(prepared.app.id))
            assertTrue(LocalAppProcessService.notificationReady(prepared.app.id))

            val launch = resources.apps.start(prepared.app.id)
            launchForFailure = launch
            val publicTaskStarted = withTimeout(45_000) {
                while (true) {
                    val current = runCatching { taskGet(launch, scenario.runId) }.getOrNull()
                    if (current?.optString("state") == "running") return@withTimeout current
                    check(current?.optString("state") !in setOf("failed", "cancelled", "interrupted", "succeeded")) {
                        "The browser-created public Agent run ended before the provider delay began."
                    }
                    delay(250)
                }
                error("unreachable")
            }
            assertEquals(scenario.runId, publicTaskStarted.getString("runId"))

            val background = captureSkillBackgroundSnapshot(
                launch = launch,
                runId = scenario.runId,
                providerBaseUrl = resources.providerBaseUrl,
                browserPackage = browserPackage,
                native = native,
                skillNonceSha256 = scenario.skillNonceSha256,
                unlistedSkillNonceSha256 = scenario.unlistedSkillNonceSha256,
                dumpDirectory = dumpDirectory,
            )
            val nativeWake = nativeWakeAndReturnToBrowser(
                appId = prepared.app.id,
                revision = prepared.app.currentRevision.toString(),
                runId = scenario.runId,
                browserPackages = prepared.browserPackages,
                dumpDirectory = dumpDirectory,
            )
            verifySkillRecoveryAndWriteEvidence(
                SkillEvidenceContext(
                    apps = resources.apps,
                    app = prepared.app,
                    release = prepared.release,
                    savedSettings = prepared.savedSettings,
                    launch = launch,
                    providerBaseUrl = resources.providerBaseUrl,
                    providerModel = resources.fixtureProvider.model,
                    skillId = scenario.skillId,
                    skillNonce = scenario.skillNonce,
                    skillFileSha256 = scenario.skillFileSha256,
                    skillNonceSha256 = scenario.skillNonceSha256,
                    unlistedSkillId = scenario.unlistedSkillId,
                    unlistedSkillFileSha256 = scenario.unlistedSkillFileSha256,
                    unlistedSkillNonceSha256 = scenario.unlistedSkillNonceSha256,
                    prompt = scenario.prompt,
                    outputSchema = scenario.outputSchema,
                    chatTitle = scenario.chatTitle,
                    reportKey = scenario.reportKey,
                    runId = scenario.runId,
                    browserPackage = browserPackage,
                    nativeWake = nativeWake,
                    background = background,
                ),
            )
        }
        } catch (failure: Throwable) {
            try {
                captureSkillFailureEvidence(launchForFailure, scenario, dumpDirectory, failure)
            } catch (evidenceFailure: Throwable) {
                if (evidenceFailure !== failure) runCatching { failure.addSuppressed(evidenceFailure) }
            }
            throw failure
        }
    }

    private suspend fun cleanupSkillFixture(resources: SkillFixtureResources) {
        var cleanupFailure: Throwable? = null
        fun recordCleanupFailure(failure: Throwable) {
            val previous = cleanupFailure
            if (previous == null) cleanupFailure = failure else previous.addSuppressed(failure)
        }
        suspend fun collectCleanupFailure(action: suspend () -> Unit) {
            try {
                action()
            } catch (failure: Throwable) {
                recordCleanupFailure(failure)
            }
        }

        var credentialCleanupComplete = !resources.credentialWriteAttempted
        try {
            resources.ids.forEach { id ->
                collectCleanupFailure { resources.apps.remove(id) }
            }
            resources.sources.forEach { source ->
                collectCleanupFailure {
                    check(source.deleteRecursively()) { "The acceptance source directory could not be removed." }
                }
            }
            if (resources.credentialWriteAttempted) {
                credentialCleanupComplete = false
                collectCleanupFailure {
                    val source = checkNotNull(resources.credentialSource) {
                        "The isolated credential source was lost before secure credential cleanup."
                    }
                    resources.providerSettings.bindSource(source)
                    val deleted = resources.providerSettings.deleteCredential(resources.fixtureProvider)
                    check(deleted.error == null && resources.fixtureProvider.id !in deleted.configuredProviderIds) {
                        "The synthetic acceptance provider credential could not be deleted from secure storage."
                    }
                    credentialCleanupComplete = true
                }
            }
        } finally {
            val providerRowsToPersist = if (credentialCleanupComplete) {
                resources.originalProviders
            } else {
                // Keep the exact synthetic row available while its secure credential needs a retry.
                resources.originalProviders + resources.fixtureProvider
            }
            collectCleanupFailure {
                if (resources.appAgentHostTouched) resources.apps.agentHost(JSONObject().put("op", "catalog"))
            }
            collectCleanupFailure { resources.providerSettings.bindSource(null) }
            collectCleanupFailure {
                resources.providerSettings.persistProviders(ProviderKind.Llm, providerRowsToPersist)
            }
            collectCleanupFailure {
                persistProviderRowsForAcceptance(providerRowsToPersist)
            }
            collectCleanupFailure {
                assertEquals(ProviderSettingsRepository.encodeProvidersJson(providerRowsToPersist),
                    ProviderSettingsRepository.encodeProvidersJson(resources.providerSettings.loadProviderState().first))
            }
            collectCleanupFailure {
                assertEquals("The host default model must be restored after the fixture provider is removed",
                    resources.originalDefaultModel, resources.providerSettings.engineLaunchConfig().defaultModel)
            }
            collectCleanupFailure { resources.credentialSource?.close() }
            collectCleanupFailure { resources.providerSettings.close() }

            val rawProviderRowsToRestore = if (credentialCleanupComplete) {
                resources.originalProvidersLlmRaw
            } else {
                rawProviderRowsWithFixture(resources.originalProvidersLlmRaw, resources.fixtureProvider)
            }
            val providerRowsPresentAfterCleanup = resources.originalProvidersLlmPresent || !credentialCleanupComplete
            collectCleanupFailure {
                restoreProviderSettingsRawValues(
                    providersPresent = providerRowsPresentAfterCleanup,
                    providersRaw = rawProviderRowsToRestore,
                    audioRegionPresent = resources.originalAudioProviderRegionPresent,
                    audioRegionRaw = resources.originalAudioProviderRegionRaw,
                )
            }
            collectCleanupFailure {
                assertEquals(rawProviderRowsToRestore, providerRowsFromFreshPreferencesFile())
            }
            if (credentialCleanupComplete) {
                collectCleanupFailure {
                    check(!resources.fixtureJournal.exists() || resources.fixtureJournal.delete()) {
                        "The Skill fixture recovery journal could not be removed after cleanup."
                    }
                }
            }
        }
        cleanupFailure?.let { throw it }
    }

    private suspend fun fixtureState(baseUrl: String): JSONObject = withContext(Dispatchers.IO) {
        val uri = java.net.URI(baseUrl)
        val connection = URL("${uri.scheme}://${uri.host}:${uri.port}/__fixture/state").openConnection() as HttpURLConnection
        try {
            connection.connectTimeout = 5_000
            connection.readTimeout = 5_000
            check(connection.responseCode == 200) { "The deterministic provider fixture state endpoint is unavailable." }
            JSONObject(connection.inputStream.bufferedReader().use { it.readText() })
        } finally { connection.disconnect() }
    }

    private suspend fun browserCall(
        launch: AppLaunch,
        method: String,
        args: JSONObject,
        origin: String = launch.origin,
    ): JSONObject = withContext(Dispatchers.IO) {
        val bytes = JSONObject().put("method", method).put("args", args)
            .toString().toByteArray(Charsets.UTF_8)
        val connection = URL("${launch.origin}/__run/${launch.run.runId}/__host/agent").openConnection() as HttpURLConnection
        val requestStartedAt = android.os.SystemClock.elapsedRealtime()
        try {
            connection.requestMethod = "POST"; connection.doOutput = true
            connection.connectTimeout = 10_000; connection.readTimeout = 30_000
            connection.setRequestProperty("Origin", origin)
            connection.setRequestProperty("Referer", launch.url)
            connection.setRequestProperty("Content-Type", "application/json")
            connection.setRequestProperty("X-LingXi-App-Request", "1")
            connection.setFixedLengthStreamingMode(bytes.size)
            connection.outputStream.use { it.write(bytes) }
            val status = connection.responseCode
            if (status != 200) {
                val errorBody = connection.errorStream?.bufferedReader()?.use { it.readText() }.orEmpty()
                JSONObject().put("httpStatus", status).put("httpErrorBody", errorBody.take(512))
                    .put("httpElapsedMs", android.os.SystemClock.elapsedRealtime() - requestStartedAt)
            } else JSONObject(connection.inputStream.bufferedReader().use { it.readText() })
        } finally { connection.disconnect() }
    }

    private suspend fun agentCall(launch: AppLaunch, method: String, args: JSONObject): Any? {
        val response = browserCall(launch, method, args)
        if (response.has("httpStatus")) {
            val body = response.optString("httpErrorBody").take(256)
            throw IllegalStateException("Agent gateway operation $method returned HTTP ${response.getInt("httpStatus")} " +
                "after ${response.optLong("httpElapsedMs")}ms" + if (body.isNotEmpty()) ": $body" else "")
        }
        if (!response.optBoolean("ok")) {
            val failure = response.optJSONObject("error")
            throw AppServiceException(failure?.optString("code") ?: "agent_error", failure?.optString("message") ?: response.toString())
        }
        return response.opt("value").takeUnless { it === JSONObject.NULL }
    }

    private suspend fun taskStart(launch: AppLaunch, input: JSONObject): JSONObject =
        agentCall(launch, "agent.tasks.start", JSONObject().put("input", input)) as JSONObject

    private suspend fun taskGet(launch: AppLaunch, runId: String): JSONObject =
        agentCall(launch, "agent.tasks.get", JSONObject().put("runId", runId)) as JSONObject

    private suspend fun terminalEvent(launch: AppLaunch, runId: String): JSONObject {
        val page = agentCall(launch, "agent.tasks.events", JSONObject().put("runId", runId)) as JSONObject
        val events = page.getJSONArray("events")
        check(events.length() > 0) { "The AG-UI event journal is empty for $runId." }
        return events.getJSONObject(events.length() - 1)
    }

    /** Native acceptance payload follows the official RunAgentInput JSON contract. */
    private fun runAgentInput(
        prompt: String,
        tools: Set<String> = emptySet(),
        skills: Set<String> = emptySet(),
        outputSchema: JSONObject? = null,
        maxTurns: Int = 8,
        timeoutMs: Int = 180_000,
        runId: String = "run-" + UUID.randomUUID(),
        threadId: String = "thread-" + UUID.randomUUID(),
    ): JSONObject {
        val options = JSONObject()
            .put("version", 1)
            .put("tools", JSONArray(tools.toList()))
            .put("skills", JSONArray(skills.toList()))
            .put("maxTurns", maxTurns)
            .put("timeoutMs", timeoutMs)
        outputSchema?.let { options.put("outputSchema", it) }
        val message = JSONObject()
            .put("id", "message-" + UUID.randomUUID())
            .put("role", "user")
            .put("content", JSONArray().put(JSONObject().put("type", "text").put("text", prompt)))
        return JSONObject()
            .put("threadId", threadId)
            .put("runId", runId)
            .put("messages", JSONArray().put(message))
            .put("forwardedProps", JSONObject().put("com.lingxi.localApp", options))
    }

    private suspend fun awaitTerminal(launch: AppLaunch, runId: String, timeoutMs: Long = 180_000): JSONObject = withTimeout(timeoutMs) {
        while (true) {
            val task = taskGet(launch, runId)
            if (task.getString("state") in setOf("succeeded", "failed", "cancelled", "interrupted")) return@withTimeout task
            delay(250)
        }
        error("unreachable")
    }

    /** Browser-side proof is an empty, app-owned Chat created only after page assertions and DOM update. */
    private suspend fun awaitBrowserReport(
        launch: AppLaunch,
        nonce: String,
        runId: String,
        timeoutMs: Long = 30_000,
    ): JSONObject = withTimeout(timeoutMs) {
            val successTitle = "agui:p:$nonce:$runId"
            val errorPrefix = "agui:e:$nonce:$runId:"
            while (true) {
                val chatPage = agentCall(launch, "agent.chats.list", JSONObject().put("limit", 50)) as JSONObject
                val chats = chatPage.getJSONArray("chats")
                val rows = (0 until chats.length()).map { chats.getJSONObject(it) }
                val failures = rows.filter { it.optString("title").startsWith(errorPrefix) }
                check(failures.size <= 1) { "Multiple browser-side error markers for runId=$runId" }
                failures.singleOrNull()?.let { failure ->
                    val parts = failure.getString("title").removePrefix(errorPrefix).split(':')
                    throw SkillBrowserDiagnosticException(
                        phase = parts.getOrNull(0) ?: "unknown",
                        code = parts.getOrNull(1) ?: "unknown",
                        errorName = parts.getOrNull(2) ?: "unknown",
                        taskState = parts.getOrNull(3) ?: "unknown",
                        taskErrorCode = parts.getOrNull(4) ?: "none",
                        eventSummary = parts.getOrNull(5) ?: "none",
                        lastReplay = parts.getOrNull(6) ?: "none",
                    )
                }
                val matches = rows.filter { it.optString("title") == successTitle }
                check(matches.size <= 1) { "Duplicate browser-side acceptance marker for runId=$runId" }
                matches.singleOrNull()?.let { return@withTimeout it }
                delay(250)
            }
            error("Browser-side acceptance report wait ended unexpectedly")
        }

    private suspend fun failOnBrowserErrorReport(launch: AppLaunch, nonce: String, runId: String) {
        val errorPrefix = "agui:e:$nonce:$runId:"
        val chatPage = agentCall(launch, "agent.chats.list", JSONObject().put("limit", 50)) as JSONObject
        val chats = chatPage.getJSONArray("chats")
        val failures = (0 until chats.length()).map { chats.getJSONObject(it) }
            .filter { it.optString("title").startsWith(errorPrefix) }
        check(failures.size <= 1) { "Multiple browser-side error markers for runId=$runId" }
        failures.singleOrNull()?.let { failure ->
            val parts = failure.getString("title").removePrefix(errorPrefix).split(':')
            error("Browser-side acceptance failed: phase=${parts.getOrNull(0) ?: "?"}, code=${parts.getOrNull(1) ?: "?"}")
        }
    }

    private suspend fun configure(
        apps: UnifiedLocalApps,
        id: String,
        photo: Boolean = true,
        tools: Set<String> = emptySet(),
        skills: Set<String> = emptySet(),
        selectedModel: JSONObject? = null,
    ): JSONObject {
        val catalog = apps.agentHost(JSONObject().put("op", "catalog")) as JSONObject
        val args = InstrumentationRegistry.getArguments()
        val configured = ProviderSettingsRepository(context).engineLaunchConfig()
        val requestedModel = selectedModel?.optString("modelId")?.takeIf { it.isNotBlank() }
            ?: args.getString("agentModel") ?: configured.defaultModel
        val requestedProvider = selectedModel?.optString("providerId")?.takeIf { it.isNotBlank() }
            ?: args.getString("agentProvider")
        val models = catalog.getJSONArray("models")
        val candidates = JSONArray()
        (0 until models.length()).map { models.getJSONObject(it) }
            .filter { it.optBoolean("canSelect") && (it.optBoolean("supportsImages") || it.optBoolean("supportsStructuredOutput")) }
            .forEach { candidate ->
                val fields = JSONObject()
                candidate.keys().forEach { key ->
                    if (key in setOf("providerId", "modelId", "canSelect", "tools") ||
                        key.contains("support", ignoreCase = true) || key.contains("capabilit", ignoreCase = true)
                    ) fields.put(key, candidate.opt(key))
                }
                candidates.put(fields)
            }
        android.util.Log.i("LocalAppAgentAcceptance", "MODEL_CATALOG_CANDIDATES $candidates")
        val model = (0 until models.length()).map { models.getJSONObject(it) }.firstOrNull {
            it.getString("modelId") == requestedModel && (requestedProvider == null || it.getString("providerId") == requestedProvider) &&
                it.optBoolean("canSelect") && (!photo || (it.optBoolean("supportsImages") && it.optBoolean("supportsStructuredOutput")))
        } ?: error("Configure a vision/tool-capable model or select it explicitly with agentProvider/agentModel")
        val capabilityFields = JSONObject()
        model.keys().forEach { key ->
            if (key in setOf("providerId", "modelId", "canSelect", "tools") ||
                key.contains("support", ignoreCase = true) || key.contains("capabilit", ignoreCase = true)
            ) capabilityFields.put(key, model.opt(key))
        }
        android.util.Log.i("LocalAppAgentAcceptance", "MODEL_CATALOG_ROW $capabilityFields")
        val ref = JSONObject().put("providerId", model.getString("providerId")).put("modelId", model.getString("modelId"))
        val grants = JSONObject(); tools.forEach { grants.put(it, catalog.getJSONObject("tools").getJSONObject(it)) }
        val settings = JSONObject().put("model", ref).put("tools", grants).put("skills", JSONArray(skills.toList()))
            .put("background", false).put("maxTurns", 8).put("timeoutMs", 180_000)
        apps.agentHost(JSONObject().put("op", "configure").put("appId", id).put("settings", settings))
        assertEquals("App selection must not change the chat's default model", configured.defaultModel,
            ProviderSettingsRepository(context).engineLaunchConfig().defaultModel)
        return ref
    }

    private suspend fun removeInterruptedSkillFixtureProvider(providerId: String) {
        check(providerId.startsWith("agui-fixture-")) {
            "Interrupted fixture cleanup only accepts the exact synthetic provider namespace."
        }
        val preferences = context.getSharedPreferences("provider_settings", android.content.Context.MODE_PRIVATE)
        val originalProvidersLlmPresent = preferences.contains("providers_llm")
        val originalProvidersLlmRaw = preferences.getString("providers_llm", null)
        val originalAudioProviderRegionPresent = preferences.contains("audio_provider_region")
        val originalAudioProviderRegionRaw = preferences.getString("audio_provider_region", null)
        check(!originalProvidersLlmPresent || originalProvidersLlmRaw != null)
        check(!originalAudioProviderRegionPresent || originalAudioProviderRegionRaw != null)
        val providerSettings = ProviderSettingsRepository(context)
        var credentialSource: com.lingxi.code.conversation.EngineConversationSource? = null
        var rowsForRestore: List<GenericProvider>? = null
        var credentialProvider: GenericProvider? = null
        var fixtureProviderRowPresent = false
        var cleanupComplete = false
        try {
            val (llmProviders, searchProviders, fetchProviders) = providerSettings.loadProviderState()
            val provider = llmProviders.firstOrNull { it.id == providerId }
            fixtureProviderRowPresent = provider != null
            val appContext = context.applicationContext
            val prefsFile = File(appContext.dataDir, "shared_prefs/provider_settings.xml")
            android.util.Log.i("LocalAppAgentAcceptance",
                "FIXTURE_CLEANUP package=${context.packageName} applicationPackage=${appContext.packageName} " +
                    "dataDir=${appContext.dataDir.absolutePath} prefsFileExists=${prefsFile.isFile} " +
                    "providerCount=${llmProviders.size} exactIdPresent=${provider != null}")
            credentialProvider = provider ?: GenericProvider(
                id = providerId,
                preset = "custom",
                name = "Interrupted Local App acceptance fixture",
                url = "",
                key = "",
                model = "",
                status = ConnStatus.Configured,
                enabled = true,
                credentialConfigured = true,
                showInModelPicker = false,
            )
            val originalDefaultModel = providerSettings.engineLaunchConfig().defaultModel
            credentialSource = com.lingxi.code.conversation.EngineConversationSource.create(
                context = context,
                workspaceKey = "acceptance.skill-credential-recovery.$providerId.${UUID.randomUUID()}",
                reuseProcessSource = false,
            ) as? com.lingxi.code.conversation.EngineConversationSource
                ?: error("Could not create an isolated native source to remove the interrupted fixture credential.")
            providerSettings.bindSource(credentialSource)
            val deleted = providerSettings.deleteCredential(checkNotNull(credentialProvider))
            check(deleted.error == null && providerId !in deleted.configuredProviderIds) {
                "The interrupted Skill fixture credential could not be deleted from secure storage."
            }
            providerSettings.bindSource(null)
            val restoredProviders = llmProviders.filterNot { it.id == providerId }
            rowsForRestore = restoredProviders
            providerSettings.persistProviders(ProviderKind.Llm, restoredProviders)
            persistProviderRowsForAcceptance(restoredProviders)
            assertEquals(ProviderSettingsRepository.encodeProvidersJson(restoredProviders),
                ProviderSettingsRepository.encodeProvidersJson(providerSettings.loadProviderState().first))
            assertEquals("Removing the interrupted fixture must preserve the host default model",
                originalDefaultModel, providerSettings.engineLaunchConfig().defaultModel)
            assertEquals(searchProviders, providerSettings.loadProviderState().second)
            assertEquals(fetchProviders, providerSettings.loadProviderState().third)
            cleanupComplete = true
        } finally {
            try {
                providerSettings.bindSource(null)
            } finally {
                try {
                    credentialSource?.close()
                } finally {
                    try {
                        providerSettings.close()
                    } finally {
                        val restoredRaw = if (cleanupComplete) {
                            rawProviderRowsWithoutFixture(
                                originalProvidersLlmRaw,
                                providerId,
                                requirePresent = fixtureProviderRowPresent,
                            )
                        } else if (fixtureProviderRowPresent) {
                            originalProvidersLlmRaw
                        } else {
                            rawProviderRowsWithFixture(originalProvidersLlmRaw, checkNotNull(credentialProvider))
                        }
                        val restoredRawPresent = if (cleanupComplete) {
                            originalProvidersLlmPresent && restoredRaw != null
                        } else {
                            originalProvidersLlmPresent || restoredRaw != null
                        }
                        restoreProviderSettingsRawValues(
                            providersPresent = restoredRawPresent,
                            providersRaw = restoredRaw,
                            audioRegionPresent = originalAudioProviderRegionPresent,
                            audioRegionRaw = originalAudioProviderRegionRaw,
                        )
                    }
                }
            }
        }
        val restoredProviders = checkNotNull(rowsForRestore) {
            "The exact fixture provider was not removed before preference verification."
        }
        val rawProviderRowsRestored = if (cleanupComplete) {
            rawProviderRowsWithoutFixture(originalProvidersLlmRaw, providerId, requirePresent = fixtureProviderRowPresent)
        } else if (fixtureProviderRowPresent) {
            originalProvidersLlmRaw
        } else {
            rawProviderRowsWithFixture(originalProvidersLlmRaw, checkNotNull(credentialProvider))
        }
        assertEquals(
            rawProviderRowsRestored,
            providerRowsFromFreshPreferencesFile(),
        )
    }

    @Test fun cleanupInterruptedSkillFixtureProvider() = runBlocking<Unit> {
        requireAcceptanceOptIn(
            "localAppAgentCleanup", "Interrupted fixture cleanup requires the exact provider ID",
            "com.lingxi.code.apps.LocalAppAgentAcceptanceTest#cleanupInterruptedSkillFixtureProvider", "fixtureCleanupProviderId",
        )
        val providerId = acceptanceArgument("fixtureCleanupProviderId")
        val journal = File(context.filesDir, "ag-ui-android-skill-fixture-provider.json")
        if (journal.isFile) {
            assertEquals(providerId, JSONObject(journal.readText()).getString("providerId"))
        }
        removeInterruptedSkillFixtureProvider(providerId)
        if (journal.isFile) {
            check(journal.delete() || !journal.exists()) {
                "The interrupted Skill fixture journal could not be cleared after secure cleanup."
            }
        }
    }

    private suspend fun publish(
        apps: UnifiedLocalApps,
        sources: MutableList<File>,
        tools: Set<String> = emptySet(),
        pageHtml: String? = null,
        skillId: String? = null,
        skillBody: String? = null,
        unlistedSkillId: String? = null,
        unlistedSkillBody: String? = null,
    ): AppRecord {
        val sourceDirectory = apps.createAuthoring("Agent acceptance ${UUID.randomUUID()}")
        val source = apps.hostFile(sourceDirectory)
        source.deleteRecursively()
        check(File(context.filesDir, "local-app-acceptance-fixtures/calorie-agent").copyRecursively(source))
        sources.add(source)
        val projectRef = AppProjectRef("mobileProject", projectId = UUID.randomUUID().toString())
        apps.registerAuthoring(JSONObject()
            .put("workspaceId", UUID.randomUUID().toString())
            .put("sourceDirectory", sourceDirectory)
            .put("projectRef", projectRef.toJson())
            .put("authoringSessionId", canonicalSessionId(UUID.randomUUID().toString())))
        if (pageHtml != null) File(source, "index.html").writeText(pageHtml)
        if (tools.isNotEmpty()) {
            val manifest = File(source, "lingxi-app.json")
            val value = JSONObject(manifest.readText()); value.getJSONObject("agent").put("tools", JSONArray(tools.toList()))
            manifest.writeText(value.toString())
        }
        if (skillId != null || skillBody != null) {
            require(skillId != null && skillBody != null) { "An acceptance Skill needs both an ID and immutable SKILL.md body." }
            val skillFile = File(source, "skills/$skillId/SKILL.md")
            check(skillFile.parentFile!!.mkdirs())
            skillFile.writeText(skillBody)
            val manifest = File(source, "lingxi-app.json")
            val value = JSONObject(manifest.readText())
            val packageFiles = value.getJSONObject("package").getJSONArray("files")
            val skillPath = "skills/$skillId/SKILL.md"
            if ((0 until packageFiles.length()).none { packageFiles.getString(it) == skillPath }) {
                packageFiles.put(skillPath)
            }
            value.getJSONObject("agent").put(
                "skills",
                JSONArray().put(JSONObject().put("id", skillId).put("path", skillPath)),
            )
            manifest.writeText(value.toString())
        }
        if (unlistedSkillId != null || unlistedSkillBody != null) {
            require(unlistedSkillId != null && unlistedSkillBody != null) {
                "An unlisted acceptance Skill needs both an ID and immutable SKILL.md body."
            }
            require(unlistedSkillId != skillId) { "The unlisted Skill must have a distinct ID." }
            File(source, "skills/$unlistedSkillId/SKILL.md").also { file ->
                check(file.parentFile!!.mkdirs())
                file.writeText(unlistedSkillBody)
            }
            val manifest = File(source, "lingxi-app.json")
            val value = JSONObject(manifest.readText())
            val packageFiles = value.getJSONObject("package").getJSONArray("files")
            val skillPath = "skills/$unlistedSkillId/SKILL.md"
            if ((0 until packageFiles.length()).none { packageFiles.getString(it) == skillPath }) {
                packageFiles.put(skillPath)
            }
            manifest.writeText(value.toString())
        }
        val value = apps.request("publish") { put("sourcePath", source.absolutePath); put("expectedRevision", 0) } as JSONObject
        return value.getJSONObject("record").appRecord()
    }

    @Test fun stageCalorieAppForInteractiveAcceptance() = runBlocking<Unit> {
        requireAcceptanceOptIn(
            "localAppAgentInteractive", "Interactive agent setup requires a configured model",
            "com.lingxi.code.apps.LocalAppAgentAcceptanceTest#stageCalorieAppForInteractiveAcceptance", "agentProvider", "agentModel",
        )
        val apps = UnifiedLocalApps.get(context)
        val sources = mutableListOf<File>()
        val app = publish(apps, sources)
        configure(apps, app.id)
        val catalog = apps.agentHost(JSONObject().put("op", "catalog")) as JSONObject
        File(context.filesDir, "local-app-agent-interactive-acceptance.json").writeText(
            JSONObject().put("appId", app.id).put("sourcePath", sources.single().absolutePath).toString())
        android.util.Log.i("LocalAppAgentAcceptance", "INTERACTIVE_APP appId=${app.id}")
        android.util.Log.i("LocalAppAgentAcceptance", "CONFIGURED_MODELS ${catalog.getJSONArray("models")}")
    }

    /** Requires a real installed browser and a configured vision-capable model. */
    @Test fun externalBrowserFileTaskUsesPublicFileApi() = runBlocking<Unit> {
        requireAcceptanceOptIn(
            "localAppAgentLive", "Live agent acceptance requires a configured provider",
            "com.lingxi.code.apps.LocalAppAgentAcceptanceTest#externalBrowserFileTaskUsesPublicFileApi", "agentProvider", "agentModel",
        )
        val apps = UnifiedLocalApps.get(context)
        val sources = mutableListOf<File>(); val ids = mutableListOf<String>()
        val photo = File(context.filesDir, "local-app-acceptance-fixtures/test-photo.jpg").readBytes()
        val photoSha256 = java.security.MessageDigest.getInstance("SHA-256").digest(photo)
            .joinToString("") { "%02x".format(it) }
        assertEquals("File acceptance uses only the approved public image fixture",
            "bfbaf6828607d9b94fb54056877497ac32926aa75c09de4abd61bc05ba24e2a7", photoSha256)
        val imageBase64 = Base64.encodeToString(photo, Base64.NO_WRAP)
        val runId = "external-file-" + UUID.randomUUID()
        val threadId = "external-thread-" + UUID.randomUUID()
        val reportNonce = UUID.randomUUID().toString().replace("-", "").take(8)
        val quotedImage = JSONObject.quote(imageBase64)
        val page = """
            <!doctype html><meta name="viewport" content="width=device-width,initial-scale=1"><title>External browser Agent acceptance</title>
            <style>#result{white-space:pre-wrap;overflow-wrap:anywhere}</style>
            <h1>External browser Agent acceptance</h1><pre id="result">Waiting for LingXi SDK</pre>
            <script type="module">
            (async()=>{
              const output=document.getElementById('result');
              let phase='B';
              const bootstrap={
                importMode:'dynamic-esm',sdkImport:'/__local_app_sdk.js',
                scriptOrder:['inline type=module','dynamic import /__local_app_sdk.js'],
                scriptTypes:Array.from(document.scripts).map(script=>script.type||'classic')
              };
              try {
                await import('/__local_app_sdk.js');
                phase='S';
                bootstrap.globals={localApp:typeof window.localApp,bridge:typeof window.__lingxiLocalAppBridge,agui:typeof window.LingxiLocalAppAGUI,configuration:typeof window.__LINGXI_APP_CONFIGURATION__};
                const bytes=Uint8Array.from(atob($quotedImage),c=>c.charCodeAt(0));
                const file=new File([bytes],'acceptance-photo.jpg',{type:'image/jpeg'});
                const part=await localApp.agent.file(file);
                phase='F';
                if(part.type!=='image'||part.source?.type!=='data'||part.source?.mimeType!=='image/jpeg'||
                   part.metadata?.['com.lingxi.localApp']?.filename!=='acceptance-photo.jpg'){
                  throw new Error('Invalid AG-UI File ContentPart.');
                }
                const client=localApp.agent.createClient({threadId:'$threadId'});
                const message={id:'message-$runId',role:'user',content:[
                    {type:'text',text:'Identify the visible food in English in food and one visible detail in detail. Use the result tool; no business tools.'},
                    part
                ]};
                const inputSummary={
                  threadId:'$threadId',runId:'$runId',messageCount:1,
                  partTypes:message.content.map(item=>item.type),
                  parts:message.content.map(item=>({
                    type:item.type,sourceType:item.source?.type||null,mimeType:item.source?.mimeType||null,
                    filename:item.metadata?.['com.lingxi.localApp']?.filename||null
                  }))
                };
                if(inputSummary.messageCount!==1||inputSummary.partTypes.join(',')!=='text,image'||
                   inputSummary.parts[1].sourceType!=='data'||inputSummary.parts[1].mimeType!=='image/jpeg'||
                   inputSummary.parts[1].filename!=='acceptance-photo.jpg'){
                  throw new Error('Invalid File RunAgentInput ContentPart summary.');
                }
                client.addMessage(message);
                const callbacks=[];
                phase='R';
                const run=await client.runAgent({
                  runId:'$runId',
                  forwardedProps:{'com.lingxi.localApp':{
                    version:1,outputSchema:{type:'object',properties:{food:{type:'string'},detail:{type:'string'}},required:['food','detail'],additionalProperties:false},
                    tools:[],skills:[],maxTurns:8,timeoutMs:180000
                  }}
                },{onEvent({event}){callbacks.push(event.type);}});
                if(callbacks[0]!=='RUN_STARTED'||callbacks[callbacks.length-1]!=='RUN_FINISHED'||callbacks.includes('RUN_ERROR')){
                  throw new Error('Invalid AG-UI callback event order: '+JSON.stringify(callbacks));
                }
                if(!run.result||typeof run.result.food!=='string'||!Array.isArray(run.newMessages)){
                  throw new Error('Missing assembled AG-UI result or messages.');
                }
                const storedTask=await localApp.agent.tasks.get('$runId');
                if(storedTask.threadId!=='$threadId') throw new Error('Task thread does not match the submitted AG-UI input.');
                phase='P';
                const reconnect=localApp.agent.createClient({threadId:storedTask.threadId});
                const replayCallbacks=[];
                const replay=await reconnect.connectAgent({runId:'$runId'},{onEvent({event}){replayCallbacks.push(event.type);}});
                if(!replay.result||!Array.isArray(replay.newMessages)||replayCallbacks[replayCallbacks.length-1]!=='RUN_FINISHED'){
                  throw new Error('AG-UI reconnect did not replay the recorded terminal event.');
                }
                if(replay.result.food!==run.result.food||replay.result.detail!==run.result.detail||
                   replayCallbacks[0]!=='RUN_STARTED'||replayCallbacks.includes('RUN_ERROR')){
                  throw new Error('AG-UI replay result or callback order did not match the completed run.');
                }
                const eventPage=await localApp.agent.tasks.events({runId:'$runId'});
                if(eventPage.events.filter(event=>event.type==='RUN_STARTED').length!==1||
                   eventPage.events[eventPage.events.length-1]?.type!=='RUN_FINISHED'){
                  throw new Error('Read-only reconnect started a duplicate run or missed RUN_FINISHED.');
                }
                phase='V';
                output.textContent=JSON.stringify({acceptance:'CALLBACK_EVENT_ORDER_PASS',bootstrap,inputSummary,runId:'$runId',run,callbacks,replay,replayCallbacks,eventTypes:eventPage.events.map(event=>event.type)});
                await new Promise(resolve=>requestAnimationFrame(()=>resolve()));
                await localApp.agent.chats.create({title:'agui:p:$reportNonce:$runId'});
              } catch(error) {
                const code=String(error&&typeof error.code==='string'?error.code:'browser_error').toLowerCase().replace(/[^a-z0-9_-]/g,'').slice(0,8)||'error';
                output.textContent='AGUI_PAGE_DIAGNOSTIC '+JSON.stringify({bootstrap,phase,code});
                await new Promise(resolve=>requestAnimationFrame(()=>resolve()));
                try { if(window.localApp?.agent?.chats?.create) await localApp.agent.chats.create({title:'agui:e:$reportNonce:$runId:'+phase+':'+code}); } catch(_reportError) {}
              }
            })();
            </script>
        """.trimIndent()
        try {
            val app = publish(apps, sources, pageHtml = page); ids.add(app.id)
            configure(apps, app.id)
            val browserPackages = LocalAppBrowser.installedHandlerPackages(context, "http://127.0.0.1/")
            assertTrue("Install or enable an HTTP browser before this acceptance", browserPackages.isNotEmpty())
            val intent = Intent().setClassName(context.packageName, "com.lingxi.code.apps.LocalAppAcceptanceActivity")
                .putExtra("appId", app.id).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
            NativeAppActivityHost(intent).use { native ->
                var browserPackage = ""
                var consentButtonFound = false
                var consentClickAttempted = false
                val browserOpened = withTimeoutOrNull(60_000) {
                    while (true) {
                        val root = instrumentation.uiAutomation.rootInActiveWindow
                        browserPackage = root?.packageName?.toString().orEmpty()
                        if (!consentClickAttempted && root != null) {
                            var action = findAccessibilityNodeByText(root, "允许并打开浏览器")
                            consentButtonFound = action != null
                            while (action != null && !action.isClickable) action = action.parent
                            if (action != null) {
                                consentClickAttempted = true
                                action.performAction(android.view.accessibility.AccessibilityNodeInfo.ACTION_CLICK)
                            }
                        }
                        if (browserPackage in browserPackages) return@withTimeoutOrNull true
                        delay(250)
                    }
                    false
                }
                assertTrue("OS-selected browser was not foregrounded; lastWindowPackage=$browserPackage handlers=$browserPackages consentButtonFound=$consentButtonFound consentClickAttempted=$consentClickAttempted", browserOpened == true)
                assertFalse("Application must leave the native host", native.isResumed())
                delay(1500)
                assertTrue(LocalAppProcessService.owns(app.id))
                assertTrue("Foreground stop/view notification exists before browser handoff", LocalAppProcessService.notificationReady(app.id))
                val launch = apps.start(app.id)
                var listed: JSONObject? = null
                withTimeout(30_000) {
                    while (true) {
                        var cursor: String? = null
                        var match: JSONObject? = null
                        do {
                            val options = JSONObject().put("limit", 50)
                            cursor?.let { options.put("cursor", it) }
                            val taskPage = agentCall(launch, "agent.tasks.list", options) as JSONObject
                            val tasks = taskPage.getJSONArray("tasks")
                            match = (0 until tasks.length()).map { tasks.getJSONObject(it) }
                                .firstOrNull { it.optString("runId") == runId }
                            val next = taskPage.opt("nextCursor")
                                ?.takeUnless { it === JSONObject.NULL }
                                ?.toString()
                            cursor = next?.takeIf { it.isNotEmpty() && it != cursor }
                        } while (match == null && cursor != null)
                        if (match != null) { listed = match; break }
                        val pageText = visibleUiText(instrumentation.uiAutomation.rootInActiveWindow)
                        assertFalse("Browser page failed before submitting an AG-UI run: $pageText",
                            pageText.contains("AGUI_PAGE_DIAGNOSTIC"))
                        failOnBrowserErrorReport(launch, reportNonce, runId)
                        delay(500)
                    }
                }
                val task = awaitTerminal(launch, listed!!.getString("runId"))
                assertEquals(task.toString(), "succeeded", task.getString("state"))
                val trustedTask = apps.agentTasks(app.id, launch.run.revision).first { it.requestId == runId }
                val usage = trustedTask.usage ?: error("Trusted native task usage is missing.")
                assertTrue(usage.getLong("outputTokens") > 0)
                assertTrue(usage.getLong("turns") > 0)
                val eventBatch = agentCall(launch, "agent.tasks.events", JSONObject().put("runId", runId)) as JSONObject
                val events = eventBatch.getJSONArray("events")
                assertTrue("A completed run has standard event history", events.length() >= 2)
                val firstEvent = events.getJSONObject(0)
                val lastEvent = events.getJSONObject(events.length() - 1)
                assertEquals("RUN_STARTED", firstEvent.getString("type"))
                assertEquals("1.0", firstEvent.getString("protocolVersion"))
                assertEquals(threadId, firstEvent.getString("threadId"))
                assertEquals(runId, firstEvent.getString("runId"))
                firstEvent.optJSONObject("input")?.let { startedInput ->
                    assertEquals(threadId, startedInput.getString("threadId"))
                    assertEquals(runId, startedInput.getString("runId"))
                    val startedMessages = startedInput.getJSONArray("messages")
                    assertEquals(1, startedMessages.length())
                    val startedMessage = startedMessages.getJSONObject(0)
                    assertEquals("message-$runId", startedMessage.getString("id"))
                    val messageParts = startedMessage.getJSONArray("content")
                    assertEquals("text", messageParts.getJSONObject(0).getString("type"))
                    val imagePart = messageParts.getJSONObject(1)
                    assertEquals("image", imagePart.getString("type"))
                    assertEquals("data", imagePart.getJSONObject("source").getString("type"))
                    assertEquals("image/jpeg", imagePart.getJSONObject("source").getString("mimeType"))
                    assertEquals("acceptance-photo.jpg",
                        imagePart.getJSONObject("metadata").getJSONObject("com.lingxi.localApp").getString("filename"))
                }
                assertEquals("RUN_FINISHED", lastEvent.getString("type"))
                assertEquals(threadId, lastEvent.getString("threadId"))
                assertEquals(runId, lastEvent.getString("runId"))
                assertTrue(lastEvent.optJSONObject("outcome")?.optString("type") in setOf(null, "success"))
                val output = lastEvent.getJSONObject("result")
                assertTrue(output.getString("food").lowercase().contains("apple"))
                assertTrue(output.getString("detail").isNotBlank())
                assertFalse((0 until events.length()).any { events.getJSONObject(it).optString("type") == "RUN_ERROR" })
                assertEquals("Reconnecting must not start a second run", 1,
                    (0 until events.length()).count { events.getJSONObject(it).optString("type") == "RUN_STARTED" })
                val browserMarker = awaitBrowserReport(launch, reportNonce, runId)
                File(context.filesDir, "external-browser-agent-acceptance.json").writeText(
                    JSONObject().put("browserPackage", browserPackage).put("appId", app.id).put("task", task)
                        .put("browserMarkerTitle", browserMarker.getString("title")).toString(2))
                instrumentation.uiAutomation.takeScreenshot()?.let { bitmap ->
                    File(context.filesDir, "external-browser-agent-acceptance.png").outputStream().use { bitmap.compress(android.graphics.Bitmap.CompressFormat.PNG, 100, it) }
                    bitmap.recycle()
                }
                android.util.Log.i("LocalAppAgentAcceptance", "EXTERNAL_BROWSER_FILE_TASK PASS browser=$browserPackage AG-UI-RunAgentInput agent.file standard-events actual-model/schema/system-handler")
            }
        } finally { ids.forEach { runCatching { apps.remove(it) } }; sources.forEach { it.deleteRecursively() } }
    }

    @Test fun externalBrowserPersistentChatStoresHistoryAndIsolatesThreads() = runBlocking<Unit> {
        requireAcceptanceOptIn(
            "localAppAgentLive", "Live agent acceptance requires a configured provider",
            "com.lingxi.code.apps.LocalAppAgentAcceptanceTest#externalBrowserPersistentChatStoresHistoryAndIsolatesThreads", "agentProvider", "agentModel",
        )
        val apps = UnifiedLocalApps.get(context)
        val sources = mutableListOf<File>(); val ids = mutableListOf<String>()
        val runId = "external-chat-" + UUID.randomUUID()
        val reportNonce = UUID.randomUUID().toString().replace("-", "").take(8)
        val page = """
            <!doctype html><meta name="viewport" content="width=device-width,initial-scale=1"><title>External browser Chat acceptance</title>
            <style>#result{white-space:pre-wrap;overflow-wrap:anywhere}</style>
            <h1>External browser Chat acceptance</h1><pre id="result">Waiting for LingXi SDK</pre>
            <script type="module">
            (async()=>{
              const output=document.getElementById('result');
              let phase='B';
              const bootstrap={importMode:'dynamic-esm',sdkImport:'/__local_app_sdk.js'};
              try {
                await import('/__local_app_sdk.js');
                phase='S';
                bootstrap.localAppType=typeof window.localApp;
                const chat=await localApp.agent.chats.create({title:'AG-UI Chat acceptance $runId'});
                const isolated=await localApp.agent.chats.create({title:'AG-UI Chat isolation $runId'});
                const input={
                  threadId:chat.id,runId:'$runId',messages:[{
                    id:'message-$runId',role:'user',content:[{type:'text',text:'Return the exact uppercase word ORBIT using the result tool.'}]
                  }],
                  forwardedProps:{'com.lingxi.localApp':{
                    version:1,outputSchema:{type:'object',properties:{answer:{type:'string'}},required:['answer'],additionalProperties:false},
                    tools:[],skills:[],maxTurns:8,timeoutMs:180000
                  }}
                };
                phase='R';
                const result=await chat.send(input);
                if(!result.result||result.result.answer!=='ORBIT'||!Array.isArray(result.newMessages)){
                  throw new Error('Persistent Chat did not return the structured AG-UI result.');
                }
                const task=await localApp.agent.tasks.get('$runId');
                phase='H';
                const history=await chat.history({limit:50});
                phase='O';
                const reopened=await localApp.agent.chats.open(chat.id);
                phase='Q';
                const reopenedHistory=await reopened.history({limit:50});
                phase='I';
                const isolatedHistory=await isolated.history({limit:50});
                phase='L';
                const listed=await localApp.agent.chats.list({limit:50});
                phase='E';
                const eventPage=await localApp.agent.tasks.events({runId:'$runId'});
                phase='P';
                const reconnect=localApp.agent.createClient({threadId:task.threadId});
                const replayCallbacks=[];
                const replay=await reconnect.connectAgent({runId:'$runId'},{onEvent({event}){replayCallbacks.push(event.type);}});
                const summary={
                  chatId:chat.id,runId:'$runId',threadId:input.threadId,taskThreadId:task.threadId,taskState:task.state,
                  result:result.result,historyRoles:history.messages.map(message=>message.role),
                  historyIds:history.messages.map(message=>message.id),reopenedIds:reopenedHistory.messages.map(message=>message.id),
                  isolatedMessageCount:isolatedHistory.messages.length,
                  chatListed:Array.isArray(listed.chats)&&listed.chats.some(item=>item.chatId===chat.id),
                  eventTypes:eventPage.events.map(event=>event.type),replayCallbacks
                };
                if(task.threadId!==chat.id||task.state!=='succeeded'||history.messages.length<2||
                   reopenedHistory.messages.length!==history.messages.length||isolatedHistory.messages.length!==0||
                   !summary.chatListed||!eventPage.events.some(event=>event.type==='RUN_FINISHED')||
                   eventPage.events.filter(event=>event.type==='RUN_STARTED').length!==1||
                   !replay.result||replay.result.answer!=='ORBIT'||!Array.isArray(replay.newMessages)||
                   replayCallbacks[0]!=='RUN_STARTED'||replayCallbacks[replayCallbacks.length-1]!=='RUN_FINISHED'||
                   replayCallbacks.includes('RUN_ERROR')){
                  throw new Error('Persistent Chat history, isolation, or task correlation failed: '+JSON.stringify(summary));
                }
                phase='V';
                output.textContent=JSON.stringify({acceptance:'CHAT_FLOW_PASS',bootstrap,summary,replay});
                await new Promise(resolve=>requestAnimationFrame(()=>resolve()));
                phase='M';
                await localApp.agent.chats.create({title:'agui:p:$reportNonce:$runId'});
              } catch(error) {
                const code=String(error&&typeof error.code==='string'?error.code:'browser_error').toLowerCase().replace(/[^a-z0-9_-]/g,'').slice(0,8)||'error';
                output.textContent='CHAT_PAGE_DIAGNOSTIC '+JSON.stringify({bootstrap,phase,code});
                await new Promise(resolve=>requestAnimationFrame(()=>resolve()));
                try { if(window.localApp?.agent?.chats?.create) await localApp.agent.chats.create({title:'agui:e:$reportNonce:$runId:'+phase+':'+code}); } catch(_reportError) {}
              }
            })();
            </script>
        """.trimIndent()
        try {
            val app = publish(apps, sources, pageHtml = page); ids.add(app.id)
            configure(apps, app.id, photo = false)
            val browserPackages = LocalAppBrowser.installedHandlerPackages(context, "http://127.0.0.1/")
            assertTrue("Install or enable an HTTP browser before this Chat acceptance", browserPackages.isNotEmpty())
            val intent = Intent().setClassName(context.packageName, "com.lingxi.code.apps.LocalAppAcceptanceActivity")
                .putExtra("appId", app.id).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
            NativeAppActivityHost(intent).use { native ->
                var browserPackage = ""
                var consentButtonFound = false
                var consentClickAttempted = false
                val browserOpened = withTimeoutOrNull(60_000) {
                    while (true) {
                        val root = instrumentation.uiAutomation.rootInActiveWindow
                        browserPackage = root?.packageName?.toString().orEmpty()
                        if (!consentClickAttempted && root != null) {
                            var action = findAccessibilityNodeByText(root, "允许并打开浏览器")
                            consentButtonFound = action != null
                            while (action != null && !action.isClickable) action = action.parent
                            if (action != null) {
                                consentClickAttempted = true
                                action.performAction(android.view.accessibility.AccessibilityNodeInfo.ACTION_CLICK)
                            }
                        }
                        if (browserPackage in browserPackages) return@withTimeoutOrNull true
                        delay(250)
                    }
                    false
                }
                assertTrue("OS-selected browser was not foregrounded; lastWindowPackage=$browserPackage handlers=$browserPackages consentButtonFound=$consentButtonFound consentClickAttempted=$consentClickAttempted", browserOpened == true)
                assertFalse("Application must leave the native host", native.isResumed())
                delay(1500)
                assertTrue(LocalAppProcessService.owns(app.id))
                assertTrue(LocalAppProcessService.notificationReady(app.id))
                val launch = apps.start(app.id)
                var listedTask: JSONObject? = null
                withTimeout(30_000) {
                    while (true) {
                        var cursor: String? = null
                        var match: JSONObject? = null
                        do {
                            val options = JSONObject().put("limit", 50)
                            cursor?.let { options.put("cursor", it) }
                            val taskPage = agentCall(launch, "agent.tasks.list", options) as JSONObject
                            val tasks = taskPage.getJSONArray("tasks")
                            match = (0 until tasks.length()).map { tasks.getJSONObject(it) }
                                .firstOrNull { it.optString("runId") == runId }
                            cursor = taskPage.opt("nextCursor")?.takeUnless { it === JSONObject.NULL }?.toString()
                                ?.takeIf { it.isNotEmpty() && it != cursor }
                        } while (match == null && cursor != null)
                        if (match != null) { listedTask = match; break }
                        val pageText = visibleUiText(instrumentation.uiAutomation.rootInActiveWindow)
                        assertFalse("Browser Chat failed before submitting an AG-UI run: $pageText",
                            pageText.contains("CHAT_PAGE_DIAGNOSTIC"))
                        failOnBrowserErrorReport(launch, reportNonce, runId)
                        delay(500)
                    }
                }
                val task = awaitTerminal(launch, listedTask!!.getString("runId"))
                assertEquals(task.toString(), "succeeded", task.getString("state"))
                val eventBatch = agentCall(launch, "agent.tasks.events", JSONObject().put("runId", runId)) as JSONObject
                val events = eventBatch.getJSONArray("events")
                assertTrue("Persistent Chat has standard event history", events.length() >= 2)
                val firstEvent = events.getJSONObject(0); val lastEvent = events.getJSONObject(events.length() - 1)
                assertEquals("RUN_STARTED", firstEvent.getString("type"))
                assertEquals("1.0", firstEvent.getString("protocolVersion"))
                assertEquals(runId, firstEvent.getString("runId"))
                assertEquals(task.getString("threadId"), firstEvent.getString("threadId"))
                assertEquals("RUN_FINISHED", lastEvent.getString("type"))
                assertEquals(runId, lastEvent.getString("runId"))
                assertEquals(task.getString("threadId"), lastEvent.getString("threadId"))
                assertEquals("ORBIT", lastEvent.getJSONObject("result").getString("answer"))
                assertFalse((0 until events.length()).any { events.getJSONObject(it).optString("type") == "RUN_ERROR" })

                val chatId = task.getString("threadId")
                val chatPage = agentCall(launch, "agent.chats.list", JSONObject().put("limit", 50)) as JSONObject
                val chatRows = chatPage.getJSONArray("chats")
                val summaries = (0 until chatRows.length()).map { chatRows.getJSONObject(it) }
                val mainTitle = "AG-UI Chat acceptance $runId"
                val isolationTitle = "AG-UI Chat isolation $runId"
                val listedChat = summaries.singleOrNull { it.optString("title") == mainTitle }
                    ?: error("The browser-created Chat is missing from the read-only Chat list.")
                val isolatedChat = summaries.singleOrNull { it.optString("title") == isolationTitle }
                    ?: error("The browser-created isolated Chat is missing from the read-only Chat list.")
                assertEquals(chatId, listedChat.getString("chatId"))
                assertNotEquals(chatId, isolatedChat.getString("chatId"))

                val historyArgs = JSONObject().put("chatId", chatId).put("limit", 50)
                val history = agentCall(launch, "agent.chats.history", historyArgs) as JSONObject
                val reopenedHistory = agentCall(launch, "agent.chats.history", historyArgs) as JSONObject
                val isolatedHistory = agentCall(
                    launch,
                    "agent.chats.history",
                    JSONObject().put("chatId", isolatedChat.getString("chatId")).put("limit", 50),
                ) as JSONObject
                val historyMessages = history.getJSONArray("messages")
                val reopenedMessages = reopenedHistory.getJSONArray("messages")
                val isolatedMessages = isolatedHistory.getJSONArray("messages")
                assertTrue("Persistent Chat history contains the user and assistant messages", historyMessages.length() >= 2)
                val historyRoles = (0 until historyMessages.length()).map { historyMessages.getJSONObject(it).getString("role") }
                assertTrue(historyRoles.contains("user"))
                assertTrue(historyRoles.contains("assistant"))
                val historyIds = (0 until historyMessages.length()).map { historyMessages.getJSONObject(it).getString("id") }
                val reopenedIds = (0 until reopenedMessages.length()).map { reopenedMessages.getJSONObject(it).getString("id") }
                assertEquals("A fresh history read preserves the reopened Chat messages", historyIds, reopenedIds)
                assertEquals("The isolated browser-created Chat stays empty", 0, isolatedMessages.length())
                assertEquals("Only the browser-created run was started", 1,
                    (0 until events.length()).count { events.getJSONObject(it).optString("type") == "RUN_STARTED" })
                val browserMarker = awaitBrowserReport(launch, reportNonce, runId)

                val evidence = JSONObject()
                    .put("acceptance", "CHAT_FLOW_PASS")
                    .put("browserPackage", browserPackage)
                    .put("appId", app.id)
                    .put("runId", runId)
                    .put("browserMarkerTitle", browserMarker.getString("title"))
                    .put("chatId", chatId)
                    .put("isolatedChatId", isolatedChat.getString("chatId"))
                    .put("taskState", task.getString("state"))
                    .put("result", lastEvent.getJSONObject("result"))
                    .put("historyRoles", JSONArray(historyRoles))
                    .put("historyIds", JSONArray(historyIds))
                    .put("reopenedIds", JSONArray(reopenedIds))
                    .put("isolatedMessageCount", isolatedMessages.length())
                    .put("eventTypes", JSONArray((0 until events.length()).map { events.getJSONObject(it).getString("type") }))
                    .put("pageScreenshotFile", "external-browser-chat-acceptance.png")
                File(context.filesDir, "external-browser-chat-acceptance.json").writeText(evidence.toString(2))
                val browserScreenshot = instrumentation.uiAutomation.takeScreenshot()
                    ?: error("Could not capture the completed external-browser Chat page.")
                File(context.filesDir, "external-browser-chat-acceptance.png").outputStream().use {
                    check(browserScreenshot.compress(android.graphics.Bitmap.CompressFormat.PNG, 100, it)) {
                        "Could not save the completed external-browser Chat screenshot."
                    }
                }
                browserScreenshot.recycle()
                android.util.Log.i("LocalAppAgentAcceptance", "EXTERNAL_BROWSER_CHAT PASS persistent-thread history-isolation structured-result standard-events")
            }
        } finally { ids.forEach { runCatching { apps.remove(it) } }; sources.forEach { it.deleteRecursively() } }
    }

    @Test fun nativeQuestionApprovalAndTaskCancellationAreTaskScoped() = runBlocking<Unit> {
        requireAcceptanceOptIn(
            "localAppAgentLive", "Live agent acceptance requires a configured provider",
            "com.lingxi.code.apps.LocalAppAgentAcceptanceTest#nativeQuestionApprovalAndTaskCancellationAreTaskScoped", "agentProvider", "agentModel",
        )
        val apps = UnifiedLocalApps.get(context); val sources = mutableListOf<File>()
        val tools = setOf("builtin:AskUserQuestion")
        val app = publish(apps, sources, tools); val other = publish(apps, sources)
        try {
            configure(apps, app.id, photo = false, tools = tools)
            configure(apps, other.id, photo = false)
            val launch = apps.start(app.id)
            val otherLaunch = apps.start(other.id)
            val questionInput = runAgentInput(
                prompt = "Use AskUserQuestion to ask exactly 'Choose acceptance fruit' with options APPLE and PEAR. Wait for the answer, then return that exact uppercase selection with the result tool. Do not guess or skip the question.",
                tools = tools,
                outputSchema = JSONObject("""{"type":"object","properties":{"answer":{"type":"string"}},"required":["answer"],"additionalProperties":false}"""),
            )
            val runId = taskStart(launch, questionInput).getString("runId")
            val interaction = withTimeout(120_000) {
                while (true) {
                    val pending = apps.agentHost(JSONObject().put("op", "interactions")) as JSONArray
                    val found = (0 until pending.length()).map { pending.getJSONObject(it) }.firstOrNull { it.optString("runId") == runId }
                    if (found != null) return@withTimeout found
                    check(taskGet(launch, runId).getString("state") !in setOf("failed", "cancelled", "interrupted", "succeeded"))
                    delay(200)
                }
                error("unreachable")
            }
            assertEquals("question", interaction.getString("type"))
            assertTrue("A task identifier is scoped to its owning app", runCatching { taskGet(otherLaunch, runId) }.isFailure)
            val questions = interaction.getJSONArray("questions")
            val answers = JSONObject(); (0 until questions.length()).forEach { answers.put(questions.getJSONObject(it).getString("question"), "APPLE") }
            apps.agentHost(JSONObject().put("op", "answer").put("runId", runId).put("requestId", interaction.getString("requestId")).put("answers", answers))
            val result = awaitTerminal(launch, runId, 120_000)
            assertEquals(result.toString(), "succeeded", result.getString("state"))
            val finished = terminalEvent(launch, runId)
            assertEquals("RUN_FINISHED", finished.getString("type"))
            assertEquals("APPLE", finished.getJSONObject("result").getString("answer"))

            val cancelInput = runAgentInput(
                prompt = "Use AskUserQuestion to ask exactly 'Cancel acceptance task'. Wait for an answer, then return it. Do not answer yourself.",
                tools = tools,
            )
            val cancelId = taskStart(launch, cancelInput).getString("runId")
            var lastCancelState = "not queried"
            val waitingTask = withTimeoutOrNull(30_000) {
                while (true) {
                    val match = apps.agentTasks(app.id, launch.run.revision).firstOrNull { it.requestId == cancelId }
                    val task = taskGet(launch, cancelId)
                    lastCancelState = JSONObject()
                        .put("state", task.optString("state"))
                        .put("nativeTaskFound", match != null)
                        .put("phase", match?.phase)
                        .put("errorCode", task.optJSONObject("error")?.optString("code"))
                        .toString()
                    if (match?.phase == "waiting_for_answer") return@withTimeoutOrNull match
                    check(task.getString("state") !in setOf("failed", "cancelled", "interrupted", "succeeded")) {
                        "Cancellation task became terminal before asking its question: $lastCancelState"
                    }
                    delay(200)
                }
                null
            }
            val observedWaitingTask = checkNotNull(waitingTask) {
                "Cancellation task did not reach waiting_for_answer within 30s: $lastCancelState"
            }
            assertEquals("running", observedWaitingTask.state)
            val nativeTasks = apps.agentTasks(app.id, launch.run.revision)
            val trustedCancelTask = nativeTasks.firstOrNull { it.requestId == cancelId }
                ?: error("Trusted native task inventory omitted the public runId's requestId")
            val otherTasks = apps.agentTasks(other.id, otherLaunch.run.revision)
            assertFalse("Trusted task inventory is app-scoped", otherTasks.any { it.requestId == cancelId })
            apps.cancelAgentTask(other.id, otherLaunch.run.revision, trustedCancelTask.runId)
            assertEquals("A different app cannot cancel this run", "running", taskGet(launch, cancelId).getString("state"))
            apps.cancelAgentTask(app.id, launch.run.revision, trustedCancelTask.runId)
            assertEquals("cancelled", awaitTerminal(launch, cancelId).getString("state"))
            val cancelledBatch = agentCall(launch, "agent.tasks.events", JSONObject().put("runId", cancelId)) as JSONObject
            val cancelledEvents = cancelledBatch.getJSONArray("events")
            val cancelledTerminal = cancelledEvents.getJSONObject(cancelledEvents.length() - 1)
            assertEquals("RUN_FINISHED", cancelledTerminal.getString("type"))
            assertEquals(cancelId, cancelledTerminal.getString("runId"))
            assertEquals("cancelled", cancelledTerminal.getJSONObject("outcome").getString("type"))
            apps.agentHost(JSONObject().put("op", "configure").put("appId", app.id).put("settings", JSONObject.NULL))
            val deniedInput = runAgentInput(
                prompt = "Use AskUserQuestion to request a new answer.",
                tools = tools,
            )
            val denied = runCatching { taskStart(launch, deniedInput) }
            assertTrue("Revocation must fence new task admission", denied.isFailure)
            android.util.Log.i("LocalAppAgentAcceptance", "NATIVE_TASK_POLICY PASS app-scoped-task question-approval trusted-native-list-cancel revoke")
        } finally { runCatching { apps.remove(other.id) }; runCatching { apps.remove(app.id) }; sources.forEach { it.deleteRecursively() } }
    }

    @Test fun externalBrowserApprovedSkillRunsInBackgroundAndRecoversThroughNativeWakeLink() = runBlocking<Unit> {
        requireAcceptanceOptIn(
            "localAppAgentSkillFixture", "Skill acceptance requires its deterministic provider fixture",
            "com.lingxi.code.apps.LocalAppAgentAcceptanceTest#externalBrowserApprovedSkillRunsInBackgroundAndRecoversThroughNativeWakeLink",
            "fixtureProviderUrl", "fixtureProviderKey", "fixtureProviderModel",
        )
        val arguments = InstrumentationRegistry.getArguments()
        val providerBaseUrl = acceptanceArgument("fixtureProviderUrl")
        val providerKey = acceptanceArgument("fixtureProviderKey")
        val providerModel = arguments.getString("fixtureProviderModel") ?: "lingxi-local-app-fixture"
        val scenario = createSkillAcceptanceScenario()
        val fixtureJournal = File(context.filesDir, "ag-ui-android-skill-fixture-provider.json")
        recoverInterruptedSkillFixture(fixtureJournal)
        val resources = createSkillFixtureResources(providerBaseUrl, providerModel, fixtureJournal)
        var primaryFailure: Throwable? = null
        try {
            configureSkillFixtureProvider(resources, providerKey)
            val prepared = publishAndConfigureSkillApp(resources, scenario)
            runSkillInExternalBrowserAndRecover(resources, scenario, prepared)
        } catch (failure: Throwable) {
            primaryFailure = failure
            throw failure
        } finally {
            try {
                cleanupSkillFixture(resources)
            } catch (cleanupFailure: Throwable) {
                val primary = primaryFailure
                if (primary == null) throw cleanupFailure
                if (primary !== cleanupFailure) runCatching { primary.addSuppressed(cleanupFailure) }
            }
        }
    }

    @Test fun externalGatewayRejectsInvalidRunAgentInput() = runBlocking<Unit> {
        requireAcceptanceOptIn(
            "localAppAgentLive", "Agent gateway acceptance requires a configured provider",
            "com.lingxi.code.apps.LocalAppAgentAcceptanceTest#externalGatewayRejectsInvalidRunAgentInput", "agentProvider", "agentModel",
        )
        val apps = UnifiedLocalApps.get(context); val sources = mutableListOf<File>()
        val app = publish(apps, sources)
        try {
            configure(apps, app.id, photo = false)
            val launch = apps.start(app.id)
            assertEquals(403, browserCall(launch, "agent.run", JSONObject(), "https://other.example").getInt("httpStatus"))
            assertEquals(403, browserCall(launch, "agent.run", JSONObject()).getInt("httpStatus"))
            assertEquals(403, browserCall(launch, "agent.tasks.get", JSONObject().put("runId", "other"), "https://other.example").getInt("httpStatus"))
            val invalid = listOf(
                runAgentInput("invalid", outputSchema = JSONObject("""{"type":"nonexistent"}""")),
                runAgentInput("invalid", maxTurns = 0),
                runAgentInput("invalid").put("cwd", "/"),
                runAgentInput("invalid").put("messages", JSONArray().put(JSONObject().put("role", "user").put("content", "missing id"))),
                runAgentInput("invalid").put("tools", JSONArray().put(JSONObject().put("name", "page-tool").put("description", "untrusted page tool"))),
            )
            val schemaInput = invalid.first()
            val schemaRun = taskStart(launch, schemaInput)
            val schemaRunId = schemaInput.getString("runId")
            assertEquals(schemaRunId, schemaRun.getString("runId"))
            val schemaTask = awaitTerminal(launch, schemaRunId, 20_000)
            assertEquals(schemaTask.toString(), "failed", schemaTask.getString("state"))
            assertEquals("invalid_output_schema", schemaTask.getJSONObject("error").getString("code"))
            val schemaError = terminalEvent(launch, schemaRunId)
            assertEquals("RUN_ERROR", schemaError.getString("type"))
            assertEquals("invalid_output_schema", schemaError.optString("code"))
            assertFalse("An invalid schema cannot yield a result", schemaError.has("result"))
            val schemaUsage = schemaTask.opt("usage")
            assertTrue("Schema compilation fails before model usage is recorded", schemaUsage == null || schemaUsage === JSONObject.NULL)

            invalid.drop(1).forEachIndexed { offset, input ->
                val index = offset + 1
                val attempt = runCatching { taskStart(launch, input) }
                if (attempt.isSuccess) {
                    val publicRunId = attempt.getOrThrow().optString("runId")
                    val nativeTask = apps.agentTasks(app.id, launch.run.revision).firstOrNull { it.requestId == publicRunId }
                    nativeTask?.let { runCatching { apps.cancelAgentTask(app.id, launch.run.revision, it.runId) } }
                }
                assertTrue("Invalid RunAgentInput sample #$index was admitted: $input", attempt.isFailure)
            }
            android.util.Log.i("LocalAppAgentAcceptance", "GATEWAY_POLICY PASS origin/caller/RunAgentInput-schema/unsupported-page-tools/invalid-host-options")
        } finally { apps.remove(app.id); sources.forEach { it.deleteRecursively() } }
    }

    /** Run with adb reverse tcp:18867 tcp:18867 and the public MCP fixture. */
    @Test fun exactMcpGrantRequiresNativeApprovalAndRevokeFencesTask() = runBlocking<Unit> {
        requireAcceptanceOptIn(
            "localAppAgentMcpFixture", "MCP acceptance requires the external nonce fixture",
            "com.lingxi.code.apps.LocalAppAgentAcceptanceTest#exactMcpGrantRequiresNativeApprovalAndRevokeFencesTask",
            "fixtureNonce", "mcpDiscovery", "agentProvider", "agentModel",
        )
        val expected = acceptanceArgument("fixtureNonce")
        val apps = UnifiedLocalApps.get(context); val sources = mutableListOf<File>(); val ids = mutableListOf<String>()
        val config = File(com.lingxi.code.project.MobileUserStorage.directories(context).configHome, "mcp-config.json")
        val original = config.takeIf { it.exists() }?.readText()
        val serverName = "ma-${UUID.randomUUID().toString().take(8)}"
        val content = JSONObject(original ?: "{}")
        val servers = content.optJSONObject("mcpServers") ?: JSONObject().also { content.put("mcpServers", it) }
        servers.put(serverName, JSONObject().put("type", "http").put("url", "http://127.0.0.1:18867/mcp"))
        val staged = content.toString()
        config.parentFile!!.mkdirs(); config.writeText(staged)
        try {
            var catalog = apps.agentHost(JSONObject().put("op", "catalog")) as JSONObject
            val serverList = catalog.getJSONArray("servers")
            val server = (0 until serverList.length()).map { serverList.getJSONObject(it) }.single { it.getString("name") == serverName }
            catalog = apps.agentHost(JSONObject().put("op", "connectMcp").put("serverId", server.getString("id"))) as JSONObject
            val all = catalog.getJSONObject("tools")
            val mcpId = all.keys().asSequence().single { id -> all.getJSONObject(id).optString("rawToolName") == "acceptance_nonce" && catalog.getJSONObject("labels").getString(id).contains(serverName) }
            val forbidden = all.keys().asSequence().single { id -> all.getJSONObject(id).optString("rawToolName") == "forbidden_nonce" && catalog.getJSONObject("labels").getString(id).contains(serverName) }
            // Preserve the isolated inline-discovery lane used by the acceptance runner.
            val granted = if (InstrumentationRegistry.getArguments().getString("mcpDiscovery") == "inline") setOf(mcpId) else setOf("builtin:ToolSearch", mcpId)
            val app = publish(apps, sources, granted); ids.add(app.id)
            configure(apps, app.id, photo = false, tools = granted)
            val launch = apps.start(app.id)
            val rejected = runAgentInput(prompt = "do nothing", tools = setOf(forbidden))
            assertTrue("Unapproved same-server tool cannot enter a task", runCatching { taskStart(launch, rejected) }.isFailure)
            fun input() = runAgentInput(
                prompt = "Call the authorized acceptance_nonce tool on server $serverName once with {}, then return its exact nonce with the result tool. Use ToolSearch only if it is available and needed to discover the tool. Do not invent the nonce. No other business tool is authorized.",
                tools = granted,
                outputSchema = JSONObject("""{"type":"object","properties":{"nonce":{"type":"string"}},"required":["nonce"],"additionalProperties":false}"""),
            )
            val runId = taskStart(launch, input()).getString("runId"); var approvals = 0
            val result = withTimeout(180_000) {
                while (true) {
                    val pending = apps.agentHost(JSONObject().put("op", "interactions")) as JSONArray
                    (0 until pending.length()).map { pending.getJSONObject(it) }.filter { it.optString("runId") == runId }.forEach { interaction ->
                        assertEquals("permission", interaction.getString("type")); approvals++
                        apps.agentHost(JSONObject().put("op", "answer").put("runId", runId).put("requestId", interaction.getString("requestId")).put("approved", true))
                    }
                    val task = taskGet(launch, runId)
                    if (task.getString("state") in setOf("succeeded", "failed", "cancelled", "interrupted")) return@withTimeout task
                    delay(200)
                }
                error("unreachable")
            }
            assertEquals(result.toString(), "succeeded", result.getString("state"))
            assertTrue("MCP calls require native approval", approvals > 0)
            val finished = terminalEvent(launch, runId)
            assertEquals("RUN_FINISHED", finished.getString("type"))
            assertEquals(expected, finished.getJSONObject("result").getString("nonce"))
            val revokeId = taskStart(launch, input()).getString("runId")
            val waiting = withTimeout(120_000) {
                while (true) {
                    val pending = apps.agentHost(JSONObject().put("op", "interactions")) as JSONArray
                    val found = (0 until pending.length()).map { pending.getJSONObject(it) }.firstOrNull { it.optString("runId") == revokeId }
                    if (found != null) return@withTimeout found
                    check(taskGet(launch, revokeId).getString("state") !in setOf("succeeded", "failed", "cancelled", "interrupted"))
                    delay(200)
                }
                error("unreachable")
            }
            apps.agentHost(JSONObject().put("op", "configure").put("appId", app.id).put("settings", JSONObject.NULL))
            assertTrue("Stale pending approval cannot restore revoked access", runCatching {
                apps.agentHost(JSONObject().put("op", "answer").put("runId", revokeId).put("requestId", waiting.getString("requestId")).put("approved", true))
            }.isFailure)
            assertEquals("cancelled", taskGet(launch, revokeId).getString("state"))
            android.util.Log.i("LocalAppAgentAcceptance", "MCP_EXACT_GRANT PASS actual-model exact-nonce native-approval ungranted-tool-denied pending-revoke-fenced")
        } finally {
            for (id in ids) runCatching { apps.remove(id) }; sources.forEach { it.deleteRecursively() }
            if (config.exists() && config.readText() == staged) {
                if (original == null) config.delete() else config.writeText(original)
            } else if (config.exists()) {
                val latest = JSONObject(config.readText()); latest.optJSONObject("mcpServers")?.remove(serverName); config.writeText(latest.toString())
            }
        }
    }
}
