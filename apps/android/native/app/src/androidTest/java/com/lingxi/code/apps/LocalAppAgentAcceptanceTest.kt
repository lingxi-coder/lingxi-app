package com.lingxi.code.apps

import android.content.Intent
import android.util.Base64
import android.webkit.WebView
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import com.lingxi.code.settings.ProviderSettingsRepository
import kotlinx.coroutines.*
import org.json.JSONArray
import org.json.JSONObject
import org.junit.Assert.*
import org.junit.Test
import org.junit.runner.RunWith
import java.io.File
import java.util.UUID

/** Actual product JNI and page SDK; uses only the host's configured model.
 * Stage calorie-agent and a public test-photo.jpg under local-app-acceptance-fixtures.
 * Optional agentProvider/agentModel arguments explicitly choose a configured model.
 * No credentials, mocked inference or chat transcript are injected by this test.
 */
@RunWith(AndroidJUnit4::class)
class LocalAppAgentAcceptanceTest {
    private val instrumentation get() = InstrumentationRegistry.getInstrumentation()
    private val context get() = instrumentation.targetContext

    private suspend fun evaluate(view: WebView, script: String): String {
        val value = CompletableDeferred<String>()
        withContext(Dispatchers.Main.immediate) { view.evaluateJavascript(script) { value.complete(it) } }
        return withTimeout(15_000) { value.await() }
    }
    private suspend fun awaitPage(view: WebView, condition: String, timeout: Long = 20_000) {
        try {
            withTimeout(timeout) {
                while (evaluate(view, condition) != "true") {
                    val error = evaluate(view, "window.__agentFailure || null")
                    check(error == "null") { "Agent page failed: $error" }
                    delay(150)
                }
            }
        } catch (timeout: TimeoutCancellationException) {
            val state = runCatching { evaluate(view, "JSON.stringify({url:location.href,ready:document.readyState,sdk:!!window.localApp,status:document.getElementById('status')?.textContent,error:document.getElementById('error')?.textContent,runId:window.__runId,task:window.__task,retryId:window.__retryId,conflictDenied:window.__conflictDenied,unexpected:window.__unexpected,restored:window.__restored,warnings:window.__agentWarnings,saved:localStorage.getItem('agent-acceptance-result')})") }.getOrDefault("unavailable")
            throw AssertionError("Timed out waiting for $condition; state=$state", timeout)
        }
    }
    private suspend fun configure(apps: UnifiedLocalApps, id: String, photo: Boolean = true, tools: Set<String> = emptySet()): JSONObject {
        val catalog = apps.agentHost(JSONObject().put("op", "catalog")) as JSONObject
        val args = InstrumentationRegistry.getArguments()
        val configured = ProviderSettingsRepository(context).engineLaunchConfig()
        val requestedModel = args.getString("agentModel") ?: configured.defaultModel
        val requestedProvider = args.getString("agentProvider")
        val models = catalog.getJSONArray("models")
        val model = (0 until models.length()).map { models.getJSONObject(it) }.firstOrNull {
            it.getString("modelId") == requestedModel && (requestedProvider == null || it.getString("providerId") == requestedProvider) &&
                it.optBoolean("canSelect") && (!photo || (it.optBoolean("supportsImages") && it.optBoolean("supportsStructuredOutput")))
        } ?: error("Configure a vision/tool-capable model or select it explicitly with agentProvider/agentModel")
        val ref = JSONObject().put("providerId", model.getString("providerId")).put("modelId", model.getString("modelId"))
        val grants = JSONObject(); tools.forEach { grants.put(it, catalog.getJSONObject("tools").getJSONObject(it)) }
        val settings = JSONObject().put("model", ref).put("tools", grants).put("background", false)
            .put("maxTurns", 8).put("timeoutMs", 180_000)
        apps.agentHost(JSONObject().put("op", "configure").put("appId", id).put("settings", settings))
        assertEquals("App selection must not change the chat's default model", configured.defaultModel,
            ProviderSettingsRepository(context).engineLaunchConfig().defaultModel)
        return ref
    }
    private suspend fun publish(apps: UnifiedLocalApps, sources: MutableList<File>, tools: Set<String> = emptySet()): AppRecord {
        val source = apps.hostFile("/home/lingxi/.lingxi/.apps/agent-acceptance-${UUID.randomUUID()}")
        check(File(context.filesDir, "local-app-acceptance-fixtures/calorie-agent").copyRecursively(source))
        sources.add(source)
        if (tools.isNotEmpty()) {
            val manifest = File(source, "lingxi-app.json")
            val value = JSONObject(manifest.readText()); value.getJSONObject("agent").put("tools", JSONArray(tools.toList()))
            manifest.writeText(value.toString())
        }
        val value = apps.request("publish") { put("sourcePath", source.absolutePath); put("expectedRevision", 0) } as JSONObject
        return value.getJSONObject("record").appRecord()
    }
    private suspend fun page(apps: UnifiedLocalApps, id: String): WebView = withTimeout(25_000) {
        while (true) {
            val launch = withContext(Dispatchers.Main.immediate) { apps.runtime.launch(id) }
            if (launch != null) return@withTimeout withContext(Dispatchers.Main.immediate) { apps.runtime.open(id, launch) }
            delay(100)
        }
        error("unreachable")
    }

    /** Explicit runner target; leaves only its newly published app for UI checks. */
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

    @Test fun closingAnOldViewCannotDestroyItsReplacementAndNeedsNoModelHost() = runBlocking<Unit> {
        val apps = UnifiedLocalApps.get(context); val sources = mutableListOf<File>()
        val app = publish(apps, sources)
        try {
            val first = apps.start(app.id)
            apps.agentClosed(first) // Idempotent before any model initialization.
            val oldView = apps.runtime.open(app.id, first)
            val replacement = apps.start(app.id, "restart")
            assertNotEquals(first.run.runId, replacement.run.runId)
            val currentView = apps.runtime.open(app.id, replacement)
            assertNotSame(oldView, currentView)
            apps.runtime.close(app.id, oldView) // Delayed old Compose disposal.
            assertEquals(replacement.run.runId, apps.runtime.launch(app.id)?.run?.runId)
            assertSame(currentView, apps.runtime.open(app.id, replacement))
            apps.agentClosed(first) // Old native close must not reject the new run.
            assertEquals(replacement.run.runId, apps.runtime.launch(app.id)?.run?.runId)
            android.util.Log.i("LocalAppAgentAcceptance", "CLOSE_OWNER PASS no-model-bootstrap old-view-disposal replacement-retained")
        } finally { apps.remove(app.id); sources.forEach { it.deleteRecursively() } }
    }

    @Test fun realPhotoSchemaRetryRefreshAndAppIsolation() = runBlocking<Unit> {
        val apps = UnifiedLocalApps.get(context)
        val sources = mutableListOf<File>(); val ids = mutableListOf<String>()
        try {
            val app = publish(apps, sources); ids.add(app.id)
            val other = publish(apps, sources); ids.add(other.id)
            val model = configure(apps, app.id)
            val photo = File(context.filesDir, "local-app-acceptance-fixtures/test-photo.jpg").readBytes()
            val input = JSONObject().put("requestId", "photo-${UUID.randomUUID()}")
                .put("prompt", "Identify the food visible in this image. Return its English name in food and describe one visible detail in detail. Use the result tool, no business tools.")
                .put("images", JSONArray().put(JSONObject().put("media_type", "image/jpeg").put("base64", Base64.encodeToString(photo, Base64.NO_WRAP))))
                .put("tools", JSONArray()).put("outputSchema", JSONObject("""{"type":"object","properties":{"food":{"type":"string"},"detail":{"type":"string"}},"required":["food","detail"],"additionalProperties":false}"""))
            val intent = Intent().setClassName(context.packageName, "com.lingxi.code.apps.LocalAppAcceptanceActivity")
                .putExtra("appId", app.id).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
            NativeAppActivityHost(intent).use {
                val view = page(apps, app.id)
                awaitPage(view, "document.readyState==='complete' && !!window.localApp", 25_000)
                evaluate(view, "window.__agentWarnings=[];const oldWarn=console.warn.bind(console);console.warn=(...v)=>{__agentWarnings.push(v.join(' '));__agentWarnings=__agentWarnings.slice(-4);oldWarn(...v);}; window.__input=$input; localApp.agent.run(__input).then(v=>{window.__runId=v.runId;localApp.agent.subscribe(v.runId,e=>{window.__task=e.run; if(['failed','interrupted','cancelled'].includes(e.run.state)) window.__agentFailure=JSON.stringify(e.run.error||{state:e.run.state}); if(e.run.state==='succeeded')localStorage.setItem('agent-acceptance-result',JSON.stringify(e.run.result.output));});}).catch(e=>window.__agentFailure=String(e))")
                awaitPage(view, "window.__task?.state==='succeeded'", 180_000)
                assertEquals("true", evaluate(view, "__task.result.output.food.toLowerCase().includes('apple') && __task.result.output.detail.length>0"))
                assertEquals("Real provider usage must be accounted for this task", "true",
                    evaluate(view, "__task.usage.outputTokens>0 && __task.usage.turns>0 && typeof __task.usage.costEstimated==='boolean'"))
                val runId = JSONArray("[${evaluate(view, "window.__runId")}]").getString(0)
                evaluate(view, "const s=__input.outputSchema;__input.outputSchema={additionalProperties:s.additionalProperties,required:s.required,properties:s.properties,type:s.type};localApp.agent.run(__input).then(v=>window.__retryId=v.runId).catch(e=>window.__agentFailure=String(e))")
                awaitPage(view, "window.__retryId===window.__runId")
                evaluate(view, "localApp.agent.run({...__input,prompt:'conflicting request'}).then(()=>window.__unexpected=true).catch(()=>window.__conflictDenied=true)")
                awaitPage(view, "window.__conflictDenied===true")
                val otherLaunch = apps.start(other.id)
                assertTrue(runCatching { apps.agentPage(otherLaunch, "agent.get", JSONObject().put("runId", runId)) }.isFailure)
                evaluate(view, "window.__oldDocumentBeforeReload=true")
                withContext(Dispatchers.Main.immediate) { view.reload() }
                // reload() returns before navigation starts. Do not send the
                // reconnect request into the still-complete old document.
                awaitPage(view, "!window.__oldDocumentBeforeReload && document.readyState==='complete' && !!window.localApp && !!localStorage.getItem('agent-acceptance-result')", 25_000)
                evaluate(view, "localApp.agent.get(${JSONObject.quote(runId)}).then(v=>window.__restored=v).catch(e=>window.__agentFailure=String(e))")
                awaitPage(view, "window.__restored?.state==='succeeded'")
                assertEquals("true", evaluate(view, "JSON.stringify(__restored.result.output)===localStorage.getItem('agent-acceptance-result')"))
                android.util.Log.i("LocalAppAgentAcceptance", "PHOTO_SCHEMA_RETRY_REFRESH_ISOLATION PASS model=$model run=$runId actual-JNI-pageSDK-public-photo no-business-tools")
            }
        } finally {
            for (id in ids) runCatching { apps.remove(id) }
            sources.forEach { it.deleteRecursively() }
        }
    }

    @Test fun mealExampleRecognizesEditsSavesAndReloads() = runBlocking<Unit> {
        val apps = UnifiedLocalApps.get(context); val sources = mutableListOf<File>()
        val app = publish(apps, sources)
        try {
            configure(apps, app.id)
            val intent = Intent().setClassName(context.packageName, "com.lingxi.code.apps.LocalAppAcceptanceActivity")
                .putExtra("appId", app.id).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
            NativeAppActivityHost(intent).use {
                val view = page(apps, app.id)
                awaitPage(view, "document.readyState==='complete' && !!window.localApp && document.getElementById('status')?.textContent.includes('请先选择')", 25_000)
                val photo = Base64.encodeToString(File(context.filesDir, "local-app-acceptance-fixtures/test-photo.jpg").readBytes(), Base64.NO_WRAP)
                // Exercise the shipped example's File -> ImageBitmap -> canvas
                // path and real click handlers. Native picker/codec has its
                // separate device checks; no Agent/model result is mocked.
                evaluate(view, "{const bytes=Uint8Array.from(atob(${JSONObject.quote(photo)}),c=>c.charCodeAt(0));const files=new DataTransfer();files.items.add(new File([bytes],'public-apple.jpg',{type:'image/jpeg'}));const input=document.getElementById('photo-file');input.files=files.files;input.dispatchEvent(new Event('change',{bubbles:true}));}")
                awaitPage(view, "!document.getElementById('analyze').disabled")
                evaluate(view, "document.getElementById('analyze').click()")
                awaitPage(view, "!document.getElementById('result').hidden && document.getElementById('food-0-grams')!==null", 180_000)
                evaluate(view, "{const grams=document.getElementById('food-0-grams');grams.value='150';grams.dispatchEvent(new Event('input',{bubbles:true}));window.__editedTotal=document.getElementById('total').textContent;document.getElementById('save').click();}")
                awaitPage(view, "document.getElementById('status').textContent.includes('已保存')")
                assertEquals("true", evaluate(view, "{const saved=JSON.parse(localStorage.getItem('meal-notes.records.v1'));saved.length===1 && saved[0].items[0].grams===150 && !JSON.stringify(saved).includes('base64') && document.getElementById('history').textContent.includes('150');}"))
                evaluate(view, "window.__oldDocumentBeforeReload=true")
                withContext(Dispatchers.Main.immediate) { view.reload() }
                awaitPage(view, "!window.__oldDocumentBeforeReload && document.readyState==='complete' && document.getElementById('history')?.textContent.includes('150')", 25_000)
                assertEquals("true", evaluate(view, "JSON.parse(localStorage.getItem('meal-notes.records.v1'))[0].items[0].grams===150"))
                android.util.Log.i("LocalAppAgentAcceptance", "MEAL_EXAMPLE PASS actual-model schema edit-150g save reload no-photo-in-history")
            }
        } finally { apps.remove(app.id); sources.forEach { it.deleteRecursively() } }
    }

    @Test fun builtinQuestionRequiresTrustedHostAndResumes() = runBlocking<Unit> {
        val apps = UnifiedLocalApps.get(context); val sources = mutableListOf<File>()
        val tools = setOf("builtin:AskUserQuestion")
        val app = publish(apps, sources, tools)
        try {
            configure(apps, app.id, photo = false, tools = tools)
            val launch = apps.start(app.id)
            val request = JSONObject().put("requestId", "question-${UUID.randomUUID()}")
                .put("prompt", "Use AskUserQuestion to ask exactly 'Choose acceptance fruit' with options APPLE and PEAR. Wait for the answer, then return that exact uppercase selection with the result tool. Do not guess or skip the question.")
                .put("tools", JSONArray(tools.toList()))
                .put("outputSchema", JSONObject("""{"type":"object","properties":{"answer":{"type":"string"}},"required":["answer"],"additionalProperties":false}"""))
            val task = apps.agentPage(launch, "agent.run", request) as JSONObject
            val runId = task.getString("runId")
            val interaction = withTimeout(120_000) {
                while (true) {
                    val pending = apps.agentHost(JSONObject().put("op", "interactions")) as JSONArray
                    val found = (0 until pending.length()).map { pending.getJSONObject(it) }.firstOrNull { it.optString("runId") == runId }
                    if (found != null) return@withTimeout found
                    val state = apps.agentPage(launch, "agent.get", JSONObject().put("runId", runId)) as JSONObject
                    check(state.getString("state") !in setOf("failed", "cancelled", "interrupted", "succeeded")) { "Question ended before host reply: $state" }
                    delay(200)
                }
                error("unreachable")
            }
            assertEquals("question", interaction.getString("type"))
            assertTrue("Page cannot impersonate the trusted host", runCatching {
                apps.agentPage(launch, "agent.answer", JSONObject().put("runId", runId).put("approved", true))
            }.isFailure)
            val questions = interaction.getJSONArray("questions")
            val answers = JSONObject(); (0 until questions.length()).forEach { answers.put(questions.getJSONObject(it).getString("question"), "APPLE") }
            apps.agentHost(JSONObject().put("op", "answer").put("runId", runId).put("requestId", interaction.getString("requestId")).put("answers", answers))
            val result = withTimeout(120_000) {
                while (true) {
                    val value = apps.agentPage(launch, "agent.get", JSONObject().put("runId", runId)) as JSONObject
                    if (value.getString("state") in setOf("succeeded", "failed", "cancelled", "interrupted")) return@withTimeout value
                    delay(200)
                }
                error("unreachable")
            }
            assertEquals(result.toString(), "succeeded", result.getString("state"))
            assertEquals("APPLE", result.getJSONObject("result").getJSONObject("output").getString("answer"))
            android.util.Log.i("LocalAppAgentAcceptance", "BUILTIN_QUESTION PASS actual-model trusted-host-answer page-denied resume-schema")
        } finally { apps.remove(app.id); sources.forEach { it.deleteRecursively() } }
    }

    /** Run with adb reverse tcp:18867 tcp:18867 and the public MCP fixture. */
    @Test fun exactMcpGrantRequiresApprovalAndRevokeFencesPendingCall() = runBlocking<Unit> {
        val apps = UnifiedLocalApps.get(context); val sources = mutableListOf<File>(); val ids = mutableListOf<String>()
        val home = com.lingxi.code.project.MobileUserStorage.directories(context).configHome
        val config = File(home, "mcp-config.json"); val original = config.takeIf { it.exists() }?.readText()
        val serverName = "ma-${UUID.randomUUID().toString().take(8)}"
        val content = JSONObject(original ?: "{}")
        val servers = content.optJSONObject("mcpServers") ?: JSONObject().also { content.put("mcpServers", it) }
        servers.put(serverName, JSONObject().put("type", "http").put("url", "http://127.0.0.1:18867/mcp"))
        val staged = content.toString()
        config.parentFile!!.mkdirs(); config.writeText(staged); config.setReadable(false, false); config.setReadable(true, true); config.setWritable(false, false); config.setWritable(true, true)
        try {
            var catalog = apps.agentHost(JSONObject().put("op", "catalog")) as JSONObject
            val serverList = catalog.getJSONArray("servers")
            val server = (0 until serverList.length()).map { serverList.getJSONObject(it) }.single { it.getString("name") == serverName }
            catalog = apps.agentHost(JSONObject().put("op", "connectMcp").put("serverId", server.getString("id"))) as JSONObject
            val all = catalog.getJSONObject("tools")
            val mcpId = all.keys().asSequence().single { id -> all.getJSONObject(id).optString("rawToolName") == "acceptance_nonce" && catalog.getJSONObject("labels").getString(id).contains(serverName) }
            val forbidden = all.keys().asSequence().single { id -> all.getJSONObject(id).optString("rawToolName") == "forbidden_nonce" && catalog.getJSONObject("labels").getString(id).contains(serverName) }
            val granted = setOf("builtin:ToolSearch", mcpId)
            val app = publish(apps, sources, granted); ids.add(app.id)
            configure(apps, app.id, photo = false, tools = granted)
            val launch = apps.start(app.id)
            val rejected = JSONObject().put("requestId", "ungranted-${UUID.randomUUID()}").put("prompt", "do nothing").put("tools", JSONArray().put(forbidden))
            assertTrue("Unapproved same-server tool cannot enter a task", runCatching { apps.agentPage(launch, "agent.run", rejected) }.isFailure)
            fun input() = JSONObject().put("requestId", "mcp-${UUID.randomUUID()}")
                .put("prompt", "Use ToolSearch to locate acceptance_nonce on server $serverName, call that tool once with {}, then return its exact nonce with the result tool. Do not invent it. No other business tool is authorized.")
                .put("tools", JSONArray(granted.toList()))
                .put("outputSchema", JSONObject("""{"type":"object","properties":{"nonce":{"type":"string"}},"required":["nonce"],"additionalProperties":false}"""))
            val admitted = apps.agentPage(launch, "agent.run", input()) as JSONObject
            val runId = admitted.getString("runId"); var approvals = 0
            val result = withTimeout(180_000) {
                while (true) {
                    val pending = apps.agentHost(JSONObject().put("op", "interactions")) as JSONArray
                    (0 until pending.length()).map { pending.getJSONObject(it) }.filter { it.optString("runId") == runId }.forEach { interaction ->
                        assertEquals("permission", interaction.getString("type")); approvals++
                        apps.agentHost(JSONObject().put("op", "answer").put("runId", runId).put("requestId", interaction.getString("requestId")).put("approved", true))
                    }
                    val value = apps.agentPage(launch, "agent.get", JSONObject().put("runId", runId)) as JSONObject
                    if (value.getString("state") in setOf("succeeded", "failed", "cancelled", "interrupted")) return@withTimeout value
                    delay(200)
                }
                error("unreachable")
            }
            assertEquals(result.toString(), "succeeded", result.getString("state"))
            assertTrue("MCP calls require native approval", approvals > 0)
            val expected = InstrumentationRegistry.getArguments().getString("fixtureNonce") ?: error("Pass fixtureNonce from the local server receipt")
            assertEquals(expected, result.getJSONObject("result").getJSONObject("output").getString("nonce"))
            val revokeRun = (apps.agentPage(launch, "agent.run", input()) as JSONObject).getString("runId")
            val waiting = withTimeout(120_000) {
                while (true) {
                    val pending = apps.agentHost(JSONObject().put("op", "interactions")) as JSONArray
                    val found = (0 until pending.length()).map { pending.getJSONObject(it) }.firstOrNull { it.optString("runId") == revokeRun }
                    if (found != null) return@withTimeout found
                    val value = apps.agentPage(launch, "agent.get", JSONObject().put("runId", revokeRun)) as JSONObject
                    check(value.getString("state") !in setOf("succeeded", "failed", "cancelled", "interrupted")) { "MCP did not wait for new approval: $value" }
                    delay(200)
                }
                error("unreachable")
            }
            apps.agentHost(JSONObject().put("op", "configure").put("appId", app.id).put("settings", JSONObject.NULL))
            assertTrue("Stale pending approval cannot restore revoked access", runCatching {
                apps.agentHost(JSONObject().put("op", "answer").put("runId", revokeRun).put("requestId", waiting.getString("requestId")).put("approved", true))
            }.isFailure)
            assertEquals("cancelled", (apps.agentPage(launch, "agent.get", JSONObject().put("runId", revokeRun)) as JSONObject).getString("state"))
            android.util.Log.i("LocalAppAgentAcceptance", "MCP_EXACT_GRANT PASS actual-model exact-nonce native-approval ungranted-tool-denied pending-revoke-fenced")
        } finally {
            for (id in ids) runCatching { apps.remove(id) }; sources.forEach { it.deleteRecursively() }
            // Preserve concurrent edits; remove only our unique fixture server.
            if (config.exists() && config.readText() == staged) {
                if (original == null) config.delete() else config.writeText(original)
            } else if (config.exists()) {
                val latest = JSONObject(config.readText()); latest.optJSONObject("mcpServers")?.remove(serverName); config.writeText(latest.toString())
            }
        }
    }

    @Test fun missingGrantAndInvalidInputsFailBeforeExecution() = runBlocking<Unit> {
        val apps = UnifiedLocalApps.get(context); val sources = mutableListOf<File>()
        val app = publish(apps, sources)
        try {
            val launch = apps.start(app.id)
            fun input() = JSONObject().put("requestId", "invalid-${UUID.randomUUID()}").put("prompt", "Do not run this invalid request.").put("tools", JSONArray())
            assertTrue("Missing host grant must reject admission", runCatching { apps.agentPage(launch, "agent.run", input()) }.isFailure)
            configure(apps, app.id, photo = false)
            val invalid = listOf(
                input().put("images", JSONArray().put(JSONObject().put("media_type", "image/jpeg").put("base64", "bm90LWEtanBlZw=="))),
                input().put("outputSchema", JSONObject("""{"type":"nonexistent"}""")),
                input().put("outputSchema", JSONObject("""{"${'$'}ref":"https://example.invalid/schema"}""")),
                input().put("maxTurns", 0),
                input().put("tools", JSONArray().put("builtin:Read")),
                input().put("cwd", "/")
            )
            invalid.forEachIndexed { index, value ->
                val admitted = runCatching { apps.agentPage(launch, "agent.run", value) as JSONObject }
                if (admitted.isSuccess) {
                    // Decoding/schema compilation happens asynchronously after
                    // durable admission. It must terminate as a typed failure.
                    val id = admitted.getOrThrow().getString("runId")
                    val terminal = withTimeout(20_000) {
                        while (true) {
                            val task = apps.agentPage(launch, "agent.get", JSONObject().put("runId", id)) as JSONObject
                            if (task.getString("state") in setOf("failed", "succeeded", "cancelled", "interrupted")) return@withTimeout task
                            delay(100)
                        }
                        error("unreachable")
                    }
                    assertEquals("Invalid case $index: $terminal", "failed", terminal.getString("state"))
                    assertNotNull(terminal.optJSONObject("error"))
                    delay(200) // Let terminal cleanup release the admission lease.
                } else {
                    assertFalse("A busy lease is not validation: ${admitted.exceptionOrNull()}", admitted.exceptionOrNull().toString().contains("busy"))
                }
            }
            val settings = apps.agentHost(JSONObject().put("op", "settings").put("appId", app.id)) as JSONObject
            settings.put("background", true)
            assertTrue("Mobile must not promise an unsupported background execution lease", runCatching {
                apps.agentHost(JSONObject().put("op", "configure").put("appId", app.id).put("settings", settings))
            }.isFailure)
            android.util.Log.i("LocalAppAgentAcceptance", "INVALID_INPUTS PASS missing-grant image schema external-ref limits ungranted-tool unknown-field unsupported-background")
        } finally { apps.remove(app.id); sources.forEach { it.deleteRecursively() } }
    }

    @Test fun immediateCancelReapsBeforeNextAdmissionAndRevokeBlocksPage() = runBlocking<Unit> {
        val apps = UnifiedLocalApps.get(context); val sources = mutableListOf<File>()
        val app = publish(apps, sources)
        try {
            configure(apps, app.id, photo = false)
            val launch = apps.start(app.id)
            val request = JSONObject().put("requestId", "cancel-${UUID.randomUUID()}")
                .put("prompt", "Count from 1 to 20000 in separate lines.").put("tools", JSONArray())
            val first = apps.agentPage(launch, "agent.run", request) as JSONObject
            val runId = first.getString("runId")
            apps.agentPage(launch, "agent.cancel", JSONObject().put("runId", runId))
            val cancelled = apps.agentPage(launch, "agent.get", JSONObject().put("runId", runId)) as JSONObject
            assertEquals("cancelled", cancelled.getString("state"))
            val second = apps.agentPage(launch, "agent.run", request.put("requestId", "next-${UUID.randomUUID()}")) as JSONObject
            apps.agentHost(JSONObject().put("op", "configure").put("appId", app.id).put("settings", JSONObject.NULL))
            assertEquals("cancelled", (apps.agentPage(launch, "agent.get", JSONObject().put("runId", second.getString("runId"))) as JSONObject).getString("state"))
            assertTrue(runCatching { apps.agentPage(launch, "agent.run", request.put("requestId", "revoked-${UUID.randomUUID()}")) }.isFailure)
            android.util.Log.i("LocalAppAgentAcceptance", "CANCEL_REAP_REVOKE PASS actual-JNI no-concurrent-replacement cancelled-result no-grant-reuse")
        } finally { apps.remove(app.id); sources.forEach { it.deleteRecursively() } }
    }

    @Test fun restartCancelsOldTaskAndDelayedClosePreservesNewTask() = runBlocking<Unit> {
        val apps = UnifiedLocalApps.get(context); val sources = mutableListOf<File>()
        val app = publish(apps, sources)
        try {
            configure(apps, app.id, photo = false)
            val first = apps.start(app.id)
            val input = JSONObject().put("requestId", "old-${UUID.randomUUID()}")
                .put("prompt", "Count from 1 to 20000 in separate lines.").put("tools", JSONArray())
            val oldTask = apps.agentPage(first, "agent.run", input) as JSONObject
            val replacement = apps.start(app.id, "restart")
            val oldState = apps.agentPage(replacement, "agent.get", JSONObject().put("runId", oldTask.getString("runId"))) as JSONObject
            assertEquals("cancelled", oldState.getString("state"))
            val next = apps.agentPage(replacement, "agent.run", input.put("requestId", "new-${UUID.randomUUID()}")) as JSONObject
            apps.agentClosed(first)
            val newState = apps.agentPage(replacement, "agent.get", JSONObject().put("runId", next.getString("runId"))) as JSONObject
            assertNotEquals("cancelled", newState.getString("state"))
            apps.agentPage(replacement, "agent.cancel", JSONObject().put("runId", next.getString("runId")))
            android.util.Log.i("LocalAppAgentAcceptance", "RESTART_CLOSE_OWNER PASS old-task-cancelled stale-close-does-not-cancel-replacement")
        } finally { apps.remove(app.id); sources.forEach { it.deleteRecursively() } }
    }

    /** Actual external browser foreground, product JNI HTTP server and model.
     * Browser DOM/photo interaction is checked separately through device UI. */
    @Test fun externalBrowserForegroundAndRealHTTPAgent() = runBlocking<Unit> {
        val apps = UnifiedLocalApps.get(context)
        val sources = mutableListOf<File>(); val ids = mutableListOf<String>()
        suspend fun call(launch: AppLaunch, method: String, args: JSONObject, origin: String = launch.origin): JSONObject = withContext(Dispatchers.IO) {
            val connection = java.net.URL("${launch.origin}/__run/${launch.run.runId}/__host/agent").openConnection() as java.net.HttpURLConnection
            try {
                connection.requestMethod = "POST"; connection.doOutput = true
                connection.connectTimeout = 10_000; connection.readTimeout = 30_000
                connection.setRequestProperty("Origin", origin)
                connection.setRequestProperty("Referer", launch.url)
                connection.setRequestProperty("Content-Type", "application/json")
                connection.setRequestProperty("X-LingXi-App-Request", "1")
                val body = JSONObject().put("method", method).put("args", args).toString().toByteArray(Charsets.UTF_8)
                connection.setFixedLengthStreamingMode(body.size)
                connection.outputStream.use { it.write(body) }
                if (connection.responseCode != 200) JSONObject().put("httpStatus", connection.responseCode)
                else JSONObject(connection.inputStream.bufferedReader().use { it.readText() })
            } finally { connection.disconnect() }
        }
        try {
            val app = publish(apps, sources); ids.add(app.id)
            val other = publish(apps, sources); ids.add(other.id)
            configure(apps, app.id)
            val intent = Intent().setClassName(context.packageName, "com.lingxi.code.apps.LocalAppAcceptanceActivity")
                .putExtra("appId", app.id).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
            NativeAppActivityHost(intent).use { native ->
                var browserPackage = ""
                withTimeout(35_000) {
                    while (true) {
                        val root = instrumentation.uiAutomation.rootInActiveWindow
                        browserPackage = root?.packageName?.toString().orEmpty()
                        if (browserPackage == context.packageName) {
                            // Exercise the trusted native exact-content consent UI.
                            root?.findAccessibilityNodeInfosByText("允许并打开浏览器")?.firstOrNull { it.text?.toString() == "允许并打开浏览器" }
                                ?.performAction(android.view.accessibility.AccessibilityNodeInfo.ACTION_CLICK)
                        }
                        if (browserPackage == "com.android.chrome" || browserPackage == "com.sec.android.app.sbrowser" || browserPackage.contains("firefox")) break
                        delay(250)
                    }
                }
                assertFalse("Application must leave the native host", native.isResumed())
                delay(1500)
                assertTrue(LocalAppProcessService.owns(app.id))
                val launch = apps.start(app.id)
                val photo = File(context.filesDir, "local-app-acceptance-fixtures/test-photo.jpg").readBytes()
                val input = JSONObject().put("requestId", "browser-photo-${UUID.randomUUID()}")
                    .put("prompt", "Identify the visible food in English in food and one visible detail in detail. Use the result tool; no business tools.")
                    .put("images", JSONArray().put(JSONObject().put("media_type", "image/jpeg").put("base64", Base64.encodeToString(photo, Base64.NO_WRAP))))
                    .put("tools", JSONArray()).put("outputSchema", JSONObject("""{"type":"object","properties":{"food":{"type":"string"},"detail":{"type":"string"}},"required":["food","detail"],"additionalProperties":false}"""))
                assertEquals(403, call(launch, "agent.run", JSONObject(), "https://other.example").getInt("httpStatus"))
                assertEquals(403, call(launch, "answer", JSONObject()).getInt("httpStatus"))
                val admitted = call(launch, "agent.run", input)
                assertTrue(admitted.toString(), admitted.getBoolean("ok"))
                val runId = admitted.getJSONObject("value").getString("runId")
                assertEquals(runId, call(launch, "agent.run", input).getJSONObject("value").getString("runId"))
                val otherLaunch = apps.start(other.id)
                assertFalse(call(otherLaunch, "agent.get", JSONObject().put("runId", runId)).getBoolean("ok"))
                var task = JSONObject()
                withTimeout(180_000) {
                    while (true) {
                        val response = call(launch, "agent.get", JSONObject().put("runId", runId))
                        assertTrue(response.toString(), response.getBoolean("ok"))
                        task = response.getJSONObject("value")
                        if (task.getString("state") in setOf("succeeded", "failed", "cancelled", "interrupted")) break
                        delay(750)
                    }
                }
                assertEquals(task.toString(), "succeeded", task.getString("state"))
                assertTrue(task.getJSONObject("result").getJSONObject("output").getString("food").lowercase().contains("apple"))
                assertTrue(task.getJSONObject("usage").getLong("outputTokens") > 0)
                assertTrue(task.getJSONObject("usage").getLong("turns") > 0)
                File(context.filesDir, "external-browser-agent-acceptance.json").writeText(JSONObject().put("browserPackage", browserPackage).put("appId", app.id).put("task", task).toString(2))
                instrumentation.uiAutomation.takeScreenshot()?.let { bitmap ->
                    File(context.filesDir, "external-browser-agent-acceptance.png").outputStream().use { bitmap.compress(android.graphics.Bitmap.CompressFormat.PNG, 100, it) }
                    bitmap.recycle()
                }
                val next = apps.start(app.id, "restart")
                assertNotEquals(launch.run.runId, next.run.runId)
                assertEquals(410, call(launch, "agent.get", JSONObject().put("runId", runId)).getInt("httpStatus"))
                assertEquals("succeeded", call(next, "agent.get", JSONObject().put("runId", runId)).getJSONObject("value").getString("state"))
                android.util.Log.i("LocalAppAgentAcceptance", "EXTERNAL_BROWSER_HTTP PASS browser=$browserPackage actual-model/schema/positive-usage/dedup/cross-app/cross-origin/no-approval-route/stale-epoch/terminal-recovery")
            }
        } finally { ids.forEach { runCatching { apps.remove(it) } }; sources.forEach { it.deleteRecursively() } }
    }

}
