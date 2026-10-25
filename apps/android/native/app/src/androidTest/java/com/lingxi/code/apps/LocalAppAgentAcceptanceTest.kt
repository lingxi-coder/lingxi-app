package com.lingxi.code.apps

import android.content.Intent
import android.util.Base64
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import com.lingxi.code.model.canonicalSessionId
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

    private suspend fun browserCall(
        launch: AppLaunch,
        method: String,
        args: JSONObject,
        origin: String = launch.origin,
    ): JSONObject = withContext(Dispatchers.IO) {
        val bytes = JSONObject().put("method", method).put("args", args)
            .toString().toByteArray(Charsets.UTF_8)
        val connection = URL("${launch.origin}/__run/${launch.run.runId}/__host/agent").openConnection() as HttpURLConnection
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
            if (status != 200) JSONObject().put("httpStatus", status)
            else JSONObject(connection.inputStream.bufferedReader().use { it.readText() })
        } finally { connection.disconnect() }
    }

    private suspend fun agentCall(launch: AppLaunch, method: String, args: JSONObject): Any? {
        val response = browserCall(launch, method, args)
        if (response.has("httpStatus")) throw IllegalStateException("Agent gateway HTTP ${response.getInt("httpStatus")}")
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
        outputSchema: JSONObject? = null,
        maxTurns: Int = 8,
        timeoutMs: Int = 180_000,
        runId: String = "run-" + UUID.randomUUID(),
        threadId: String = "thread-" + UUID.randomUUID(),
    ): JSONObject {
        val options = JSONObject()
            .put("version", 1)
            .put("tools", JSONArray(tools.toList()))
            .put("skills", JSONArray())
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
    private suspend fun awaitBrowserReport(launch: AppLaunch, nonce: String, runId: String): JSONObject =
        withTimeout(30_000) {
            val successTitle = "agui:p:$nonce:$runId"
            val errorPrefix = "agui:e:$nonce:$runId:"
            while (true) {
                val chats = agentCall(launch, "agent.chats.list", JSONObject().put("limit", 50)) as JSONArray
                val rows = (0 until chats.length()).map { chats.getJSONObject(it) }
                val failures = rows.filter { it.optString("title").startsWith(errorPrefix) }
                check(failures.size <= 1) { "Multiple browser-side error markers for runId=$runId" }
                failures.singleOrNull()?.let { failure ->
                    val parts = failure.getString("title").removePrefix(errorPrefix).split(':')
                    error("Browser-side acceptance failed: phase=${parts.getOrNull(0) ?: "?"}, code=${parts.getOrNull(1) ?: "?"}")
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
        val chats = agentCall(launch, "agent.chats.list", JSONObject().put("limit", 50)) as JSONArray
        val failures = (0 until chats.length()).map { chats.getJSONObject(it) }
            .filter { it.optString("title").startsWith(errorPrefix) }
        check(failures.size <= 1) { "Multiple browser-side error markers for runId=$runId" }
        failures.singleOrNull()?.let { failure ->
            val parts = failure.getString("title").removePrefix(errorPrefix).split(':')
            error("Browser-side acceptance failed: phase=${parts.getOrNull(0) ?: "?"}, code=${parts.getOrNull(1) ?: "?"}")
        }
    }

    private suspend fun configure(apps: UnifiedLocalApps, id: String, photo: Boolean = true, tools: Set<String> = emptySet()): JSONObject {
        val catalog = apps.agentHost(JSONObject().put("op", "catalog")) as JSONObject
        val args = InstrumentationRegistry.getArguments()
        val configured = ProviderSettingsRepository(context).engineLaunchConfig()
        val requestedModel = args.getString("agentModel") ?: configured.defaultModel
        val requestedProvider = args.getString("agentProvider")
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
        val settings = JSONObject().put("model", ref).put("tools", grants).put("skills", JSONArray())
            .put("background", false).put("maxTurns", 8).put("timeoutMs", 180_000)
        apps.agentHost(JSONObject().put("op", "configure").put("appId", id).put("settings", settings))
        assertEquals("App selection must not change the chat's default model", configured.defaultModel,
            ProviderSettingsRepository(context).engineLaunchConfig().defaultModel)
        return ref
    }

    private suspend fun publish(
        apps: UnifiedLocalApps,
        sources: MutableList<File>,
        tools: Set<String> = emptySet(),
        pageHtml: String? = null,
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
        val value = apps.request("publish") { put("sourcePath", source.absolutePath); put("expectedRevision", 0) } as JSONObject
        return value.getJSONObject("record").appRecord()
    }

    @Test fun stageCalorieAppForInteractiveAcceptance() = runBlocking<Unit> {
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
                  chatListed:Array.isArray(listed)&&listed.some(item=>item.chatId===chat.id),
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
                val chatList = agentCall(launch, "agent.chats.list", JSONObject().put("limit", 50)) as JSONArray
                val summaries = (0 until chatList.length()).map { chatList.getJSONObject(it) }
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

    @Test fun externalGatewayRejectsInvalidRunAgentInput() = runBlocking<Unit> {
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
            val expected = InstrumentationRegistry.getArguments().getString("fixtureNonce") ?: error("Pass fixtureNonce from the local server receipt")
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
