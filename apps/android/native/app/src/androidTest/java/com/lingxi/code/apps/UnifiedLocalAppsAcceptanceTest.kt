package com.lingxi.code.apps

import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import com.lingxi.code.MainActivity
import com.lingxi.code.bindings.android.*
import com.lingxi.code.settings.LinuxRuntimeBridge
import com.lingxi.code.settings.LinuxRuntimeMode
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.withContext
import kotlinx.coroutines.withTimeout
import org.json.JSONObject
import org.junit.Assert.*
import org.junit.Test
import org.junit.runner.RunWith
import java.io.File
import java.net.HttpURLConnection
import java.net.Socket
import java.net.SocketException
import java.net.URI
import java.net.URL
import java.util.UUID

/** Real product JNI, bundled PRoot and shared cross-platform source fixtures.
 * The runner stages only fixtures/local-apps/v2 into this app's private files.
 * No host node_modules or host executable is imported into Android.
 */
@RunWith(AndroidJUnit4::class)
class UnifiedLocalAppsAcceptanceTest {
    @Test
    fun nativeFilePickerAndRotationKeepTheRunAndDocument() = runBlocking<Unit> {
        val instrumentation = InstrumentationRegistry.getInstrumentation()
        val context = instrumentation.targetContext
        val apps = UnifiedLocalApps.get(context)
        val source = apps.hostFile("/home/lingxi/.lingxi/.apps/picker-acceptance-${UUID.randomUUID()}")
        val fixtures = File(context.filesDir, "local-app-acceptance-fixtures/static")
        check(fixtures.copyRecursively(source))
        val manifestFile = File(source, "lingxi-app.json")
        manifestFile.writeText(JSONObject(manifestFile.readText()).put("capabilities", org.json.JSONArray(listOf("files", "microphone"))).toString())
        val published = apps.request("publish") { put("sourcePath", source.absolutePath); put("expectedRevision", 0) } as JSONObject
        val record = published.getJSONObject("record").appRecord()
        val release = published.getJSONObject("release").appRelease()
        apps.permissions.grant(record.id, release.digest, setOf("files"))
        val filename = "LingXi-Local-App-Acceptance-${UUID.randomUUID()}.txt"
        val bytes = "LOCAL-APP-NATIVE-FILE-OK".toByteArray()
        val file = context.contentResolver.insert(android.provider.MediaStore.Downloads.EXTERNAL_CONTENT_URI,
            android.content.ContentValues().apply {
                put(android.provider.MediaStore.MediaColumns.DISPLAY_NAME, filename)
                put(android.provider.MediaStore.MediaColumns.MIME_TYPE, "text/plain")
                put(android.provider.MediaStore.MediaColumns.IS_PENDING, 1)
            }) ?: error("Cannot create the isolated picker fixture")
        context.contentResolver.openOutputStream(file)!!.use { it.write(bytes) }
        context.contentResolver.update(file, android.content.ContentValues().apply { put(android.provider.MediaStore.MediaColumns.IS_PENDING, 0) }, null, null)
        suspend fun evaluate(view: android.webkit.WebView, script: String): String {
            val result = CompletableDeferred<String>()
            withContext(Dispatchers.Main.immediate) { view.evaluateJavascript(script) { result.complete(it) } }
            return withTimeout(15_000) { result.await() }
        }
        suspend fun awaitScript(view: android.webkit.WebView, script: String) {
            try { withTimeout(20_000) { while (evaluate(view, script) != "true") delay(100) } }
            catch (failure: Exception) {
                android.util.Log.e("LocalAppAcceptance", "WAIT_FAILED $script state=" + evaluate(view, "JSON.stringify({url:location.href,ready:document.readyState,sdk:!!window.localApp,error:window.__nativeFileError,file:window.__nativeFile,draft:document.querySelector('#rotation-draft')?.value,mic:window.__micDenied})"))
                throw failure
            }
        }
        fun find(node: android.view.accessibility.AccessibilityNodeInfo?, text: String): android.view.accessibility.AccessibilityNodeInfo? {
            if (node == null) return null
            if (node.text?.toString() == text) return node
            for (index in 0 until node.childCount) find(node.getChild(index), text)?.let { return it }
            return null
        }
        val intent = android.content.Intent().setClassName(context.packageName, "com.lingxi.code.apps.LocalAppAcceptanceActivity")
            .putExtra("appId", record.id).addFlags(android.content.Intent.FLAG_ACTIVITY_NEW_TASK)
        try {
            NativeAppActivityHost(intent).use {
                val page = withTimeout(20_000) {
                    while (true) {
                        val launch = withContext(Dispatchers.Main.immediate) { apps.runtime.launch(record.id) }
                        if (launch != null) return@withTimeout apps.runtime.open(record.id, launch)
                        delay(100)
                    }
                    error("unreachable")
                }
                awaitScript(page, "document.readyState === 'complete' && !!window.localApp")
                val run = apps.runtime.launch(record.id)!!.run.runId
                evaluate(page, "document.body.insertAdjacentHTML('beforeend','<input id=rotation-draft>'); document.querySelector('#rotation-draft').value='keep-native-picker-draft'")
                instrumentation.uiAutomation.setRotation(android.app.UiAutomation.ROTATION_FREEZE_90)
                delay(800)
                assertEquals(run, apps.runtime.launch(record.id)?.run?.runId)
                awaitScript(page, "document.querySelector('#rotation-draft')?.value === 'keep-native-picker-draft'")
                evaluate(page, "localApp.microphone.record().then(() => window.__micDenied=false).catch(() => window.__micDenied=true)")
                awaitScript(page, "window.__micDenied === true")
                evaluate(page, "localApp.files.pick().then(value => window.__nativeFile=value).catch(error => window.__nativeFileError=String(error))")
                withTimeout(15_000) { while (!apps.runtime.deviceRequestActive) delay(100) }
                instrumentation.uiAutomation.setRotation(android.app.UiAutomation.ROTATION_FREEZE_0)
                delay(800)
                withTimeout(25_000) {
                    while (true) {
                        var item = find(instrumentation.uiAutomation.rootInActiveWindow, filename)
                        if (item != null) {
                            while (item != null && !item.isClickable) item = item.parent
                            if (item?.performAction(android.view.accessibility.AccessibilityNodeInfo.ACTION_CLICK) == true) break
                        }
                        delay(250)
                    }
                }
                awaitScript(page, "window.__nativeFile?.base64 === '${android.util.Base64.encodeToString(bytes, android.util.Base64.NO_WRAP)}'")
                awaitScript(page, "document.querySelector('#rotation-draft')?.value === 'keep-native-picker-draft'")
                assertEquals(run, apps.runtime.launch(record.id)?.run?.runId)
                assertFalse(apps.runtime.deviceRequestActive)
                android.util.Log.i("LocalAppAcceptance", "NATIVE_PICKER_ROTATION PASS actual OpenDocument selected-own-fixture unchanged-run preserved-input microphone-denial")
            }
        } finally {
            instrumentation.uiAutomation.setRotation(android.app.UiAutomation.ROTATION_UNFREEZE)
            context.contentResolver.delete(file, null, null)
            apps.remove(record.id)
            source.deleteRecursively()
        }
    }

    @Test
    fun backgroundOptInOwnsForegroundServiceAndOptOutSuspendsOnlyThisRun() = runBlocking<Unit> {
        val instrumentation = InstrumentationRegistry.getInstrumentation()
        val context = instrumentation.targetContext
        val apps = UnifiedLocalApps.get(context)
        val source = apps.hostFile("/home/lingxi/.lingxi/.apps/background-acceptance-${UUID.randomUUID()}")
        check(File(context.filesDir, "local-app-acceptance-fixtures/python").copyRecursively(source))
        val value = apps.request("publish") { put("sourcePath", source.absolutePath); put("expectedRevision", 0) } as JSONObject
        val record = value.getJSONObject("record").appRecord()
        val release = value.getJSONObject("release").appRelease()
        apps.grant(record.id, release.digest)
        val intent = android.content.Intent().setClassName(context.packageName, "com.lingxi.code.apps.LocalAppAcceptanceActivity")
            .putExtra("appId", record.id).addFlags(android.content.Intent.FLAG_ACTIVITY_NEW_TASK)
        try {
            NativeAppActivityHost(intent).use { host ->
                val first = withTimeout(30_000) {
                    while (true) {
                        val launch = withContext(Dispatchers.Main.immediate) { apps.runtime.launch(record.id) }
                        if (launch != null && host.isResumed()) return@withTimeout launch
                        delay(100)
                    }
                    error("unreachable")
                }
                assertFalse(LocalAppProcessService.owns(record.id))
                apps.setKeepRunning(record.id, release.digest, true)
                assertTrue(LocalAppProcessService.owns(record.id))
                val manager = context.getSystemService(android.app.ActivityManager::class.java)
                @Suppress("DEPRECATION")
                suspend fun foregroundServiceExists(): Boolean = withContext(Dispatchers.Main.immediate) {
                    manager.getRunningServices(100).any { it.service.className == LocalAppProcessService::class.java.name && it.foreground }
                }
                withTimeout(10_000) { while (!foregroundServiceExists()) delay(100) }
                assertTrue(instrumentation.uiAutomation.performGlobalAction(android.accessibilityservice.AccessibilityService.GLOBAL_ACTION_HOME))
                delay(1500)
                assertEquals("running", (apps.request("status", record.id) as JSONObject).getJSONObject("run").getString("phase"))
                assertEquals("PYTHON-READY", http(first, "/health").second.decodeToString())
                assertTrue(foregroundServiceExists())
                host.bringToForeground()
                withTimeout(10_000) { while (!host.isResumed()) delay(100) }
                apps.setKeepRunning(record.id, release.digest, false)
                assertFalse(LocalAppProcessService.owns(record.id))
                assertTrue(instrumentation.uiAutomation.performGlobalAction(android.accessibilityservice.AccessibilityService.GLOBAL_ACTION_HOME))
                withTimeout(15_000) { while ((apps.request("status", record.id) as JSONObject).optJSONObject("run") != null) delay(100) }
                assertEquals("suspended", apps.runs(record.id).find { it.runId == first.run.runId }?.phase)
                assertTrue(runCatching { http(first, "/health") }.isFailure)
                host.bringToForeground()
                val resumed = withTimeout(30_000) {
                    while (true) {
                        val launch = withContext(Dispatchers.Main.immediate) { apps.runtime.launch(record.id) }
                        if (launch != null && launch.run.runId != first.run.runId) return@withTimeout launch
                        delay(100)
                    }
                    error("unreachable")
                }
                assertEquals(first.origin, resumed.origin)
                assertEquals("PYTHON-READY", http(resumed, "/health").second.decodeToString())
                assertFalse(LocalAppProcessService.owns(record.id))
                android.util.Log.i("LocalAppAcceptance", "BACKGROUND_POLICY PASS opt-in-after-start foreground-service Home-survival opt-out-suspend fresh-run-resume stable-origin")
            }
        } finally { apps.remove(record.id); source.deleteRecursively() }
    }

    @Test
    fun marketInstallUpdateRemixReviewAndTakedown() = runBlocking<Unit> {
        val instrumentation = InstrumentationRegistry.getInstrumentation()
        val context = instrumentation.targetContext
        val endpoint = InstrumentationRegistry.getArguments().getString("localAppMarketRoot")
            ?: error("Start the isolated TLS market fixture and pass localAppMarketRoot")
        require(URI(endpoint).host == "127.0.0.1")
        val certificate = File(context.filesDir, "local-app-acceptance-fixtures/market-ca.pem")
        val ca = certificate.inputStream().use { java.security.cert.CertificateFactory.getInstance("X.509").generateCertificate(it) }
        val keys = java.security.KeyStore.getInstance(java.security.KeyStore.getDefaultType()).apply { load(null, null); setCertificateEntry("local-acceptance", ca) }
        val trust = javax.net.ssl.TrustManagerFactory.getInstance(javax.net.ssl.TrustManagerFactory.getDefaultAlgorithm()).apply { init(keys) }
        val tls = javax.net.ssl.SSLContext.getInstance("TLS").apply { init(null, trust.trustManagers, null) }
        val previousTLS = javax.net.ssl.HttpsURLConnection.getDefaultSSLSocketFactory()
        javax.net.ssl.HttpsURLConnection.setDefaultSSLSocketFactory(tls.socketFactory)
        val apps = UnifiedLocalApps.get(context)
        val market = UnifiedAppMarket(context, apps, endpoint)
        val created = mutableListOf<String>(); val sources = mutableListOf<File>()
        suspend fun fixture(path: String, post: Boolean = false): JSONObject = withContext(Dispatchers.IO) {
            val url = URI(endpoint).resolve("/__test/$path").toURL()
            (url.openConnection() as javax.net.ssl.HttpsURLConnection).run {
                instanceFollowRedirects = false; connectTimeout = 15_000; readTimeout = 60_000
                if (post) { requestMethod = "POST"; doOutput = true; outputStream.use { it.write("{}".toByteArray()) } }
                try {
                    check(responseCode == 200) { "Isolated market fixture $path returned HTTP $responseCode: ${errorStream?.use { it.readNBytes(4096).decodeToString() }}" }
                    inputStream.use { JSONObject(it.readBytes().decodeToString()) }
                } finally { disconnect() }
            }
        }
        NativeAppActivityHost(android.content.Intent(context, MainActivity::class.java).addFlags(android.content.Intent.FLAG_ACTIVITY_NEW_TASK)).use {
            try {
                market.capabilities()
                val state = fixture("state"); val remote = state.getString("appId")
                val installed = market.install(remote); created.add(installed.id)
                assertNotEquals(remote, installed.id)
                assertEquals(state.getString("initialVersion"), installed.marketOrigin?.releaseId)
                val first = apps.start(installed.id)
                assertTrue(http(first, "", entry = true).second.decodeToString().contains("NATIVE-MARKET-V1"))
                val layout = apps.request("layout", installed.id) as JSONObject
                val note = File(layout.getString("dataPath"), "native-update-note").apply { writeText("retain across market releases") }
                val remix = apps.restoreSource(installed.id, true); created.add(remix.id)
                remix.authoring?.sourceDirectory?.let { sources.add(apps.hostFile(it)) }
                assertNotEquals(installed.id, remix.id)
                assertEquals(remote, remix.marketOrigin?.remixedFrom)
                val fork = apps.start(remix.id)
                fixture("advance", true)
                val advanced = fixture("state")
                val updated = market.install(remote, apps.get(installed.id))
                assertEquals(installed.id, updated.id)
                assertEquals(advanced.getString("compatibleVersion"), updated.marketOrigin?.releaseId)
                assertNotEquals(advanced.getString("incompatibleVersion"), updated.marketOrigin?.releaseId)
                val second = apps.start(updated.id)
                assertEquals(first.origin, second.origin)
                assertNotEquals(first.run.runId, second.run.runId)
                assertTrue(http(second, "", entry = true).second.decodeToString().contains("NATIVE-MARKET-V2"))
                assertEquals("retain across market releases", note.readText())
                assertTrue(http(fork, "", entry = true).second.decodeToString().contains("NATIVE-MARKET-V1"))
                assertThrows(IllegalArgumentException::class.java) { runBlocking { market.install(remote, remix) } }
                fixture("executable-update", true)
                val reviewed = fixture("state")
                val pending = market.install(remote, apps.get(installed.id))
                assertEquals(updated.currentRevision, pending.currentRevision)
                val candidate = apps.release(pending.id, pending.latestRevision)
                assertEquals(updated.marketOrigin?.releaseId, pending.marketOrigin?.releaseId)
                assertEquals(reviewed.getString("executableVersion"), candidate.marketOrigin?.releaseId)
                assertNotEquals(reviewed.getString("rejectedVersion"), candidate.marketOrigin?.releaseId)
                assertEquals(pending.latestRevision, market.install(remote, pending).latestRevision)
                assertTrue(candidate.executesCode)
                assertFalse(candidate.digest in pending.executionGrants)
                assertTrue(http(second, "", entry = true).second.decodeToString().contains("NATIVE-MARKET-V2"))
                apps.grant(pending.id, candidate.digest)
                apps.activate(pending.id, candidate.revision)
                assertEquals(reviewed.getString("executableVersion"), apps.get(pending.id).marketOrigin?.releaseId)
                val python = apps.start(pending.id)
                assertEquals("PYTHON-READY", http(python, "/health").second.decodeToString())
                assertEquals("retain across market releases", note.readText())
                fixture("takedown", true)
                try { market.install(remote, apps.get(pending.id)); fail("Unavailable listings cannot update") } catch (_: com.lingxi.code.market.MarketException) { }
                assertEquals("PYTHON-READY", http(python, "/health").second.decodeToString())
                assertEquals("retain across market releases", note.readText())
                android.util.Log.i("LocalAppAcceptance", "MARKET_NATIVE PASS HTTPS real API processor compatible-update independent-remix manual-review exact-consent staged-activation takedown-preserves-data")
            } finally {
                for (id in created) runCatching { apps.remove(id) }
                sources.forEach { it.deleteRecursively() }
                javax.net.ssl.HttpsURLConnection.setDefaultSSLSocketFactory(previousTLS)
            }
        }
    }

    @Test
    fun browserSdkAndStorageUseTheInstallationOrigin() = runBlocking<Unit> {
        val instrumentation = InstrumentationRegistry.getInstrumentation()
        val context = instrumentation.targetContext
        val apps = UnifiedLocalApps.get(context)
        val fixture = File(context.filesDir, "local-app-acceptance-fixtures/static")
        val created = mutableListOf<String>()
        val sources = mutableListOf<File>()
        NativeAppActivityHost(android.content.Intent(context, MainActivity::class.java).addFlags(android.content.Intent.FLAG_ACTIVITY_NEW_TASK)).use {
            suspend fun publish(): AppRecord {
                val source = apps.hostFile("/home/lingxi/.lingxi/.apps/browser-acceptance-${UUID.randomUUID()}")
                check(fixture.copyRecursively(source)); sources.add(source)
                val result = apps.request("publish") { put("sourcePath", source.absolutePath); put("expectedRevision", 0) } as JSONObject
                return result.getJSONObject("record").appRecord().also { created.add(it.id) }
            }
            suspend fun evaluate(view: android.webkit.WebView, script: String): String {
                val result = CompletableDeferred<String>()
                withContext(Dispatchers.Main.immediate) { view.evaluateJavascript(script) { result.complete(it) } }
                return withTimeout(10_000) { result.await() }
            }
            suspend fun awaitScript(view: android.webkit.WebView, script: String) {
                withTimeout(15_000) { while (evaluate(view, script) != "true") delay(100) }
            }
            var failure: Throwable? = null
            try {
                val first = publish(); val launch = apps.start(first.id)
                val exported = apps.sourceArchive(first.id, launch.run.revision, context.cacheDir)
                try {
                    assertEquals(setOf("lingxi-app.json", "dist/index.html", "dist/client.js"), exported.files.toSet())
                    org.apache.commons.compress.archivers.zip.ZipFile.builder().setFile(exported.archive).get().use { zip ->
                        assertEquals(exported.files.toSet(), zip.entries.asSequence().map { it.name }.toSet())
                    }
                    val imported = apps.importArchive(android.net.Uri.fromFile(exported.archive))
                    created.add(imported.id)
                    assertNotEquals(first.id, imported.id)
                    assertNull(imported.marketOrigin)
                    assertTrue(imported.executionGrants.isEmpty())
                    val importedRun = apps.start(imported.id)
                    assertTrue(http(importedRun, "", entry = true).second.decodeToString().contains("html"))
                    assertEquals(3, apps.sourceArchive(imported.id, importedRun.run.revision, context.cacheDir).use { it.files.size })
                    android.util.Log.i("LocalAppAcceptance", "SOURCE_EXPORT_IMPORT PASS new-installation-id no-market-origin no-execution-grants")
                } finally { exported.close() }
                val page = apps.runtime.open(first.id, launch)
                awaitScript(page, "document.readyState === 'complete' && !!window.localApp")
                android.util.Log.i("LocalAppAcceptance", "BROWSER_PAGE_READY")
                assertEquals(launch.origin, org.json.JSONTokener(evaluate(page, "location.origin")).nextValue())
                evaluate(page, "localApp.info().then(value => window.__acceptanceInfo = value)")
                awaitScript(page, "window.__acceptanceInfo?.runId === ${JSONObject.quote(launch.run.runId)}")
                android.util.Log.i("LocalAppAcceptance", "BROWSER_INFO_READY")
                evaluate(page, "localStorage.setItem('acceptance-note', 'keep-on-restart')")
                evaluate(page, "localApp.notify('must-not-post').then(() => window.__denied = false).catch(() => window.__denied = true)")
                awaitScript(page, "window.__denied === true")
                evaluate(page, """(() => {
                    const child = document.createElement('iframe');
                    child.srcdoc = '<script>lingxiNative.onmessage = e => parent.__childDenied = !JSON.parse(e.data).ok; lingxiNative.postMessage(JSON.stringify({id:"child",method:"info"}));</script>';
                    document.body.appendChild(child);
                })()""")
                awaitScript(page, "window.__childDenied === true")
                apps.stop(first.id)
                val restarted = apps.start(first.id)
                assertEquals(launch.origin, restarted.origin)
                val reopened = apps.runtime.open(first.id, restarted)
                awaitScript(reopened, "document.readyState === 'complete' && !!window.localApp")
                assertEquals(JSONObject.quote("keep-on-restart"), evaluate(reopened, "localStorage.getItem('acceptance-note')"))
                apps.request("stop", first.id)
                evaluate(reopened, "localApp.info().then(() => window.__staleDenied = false).catch(() => window.__staleDenied = true)")
                awaitScript(reopened, "window.__staleDenied === true")
                val second = publish(); val isolated = apps.start(second.id)
                assertNotEquals(launch.origin, isolated.origin)
                val separate = apps.runtime.open(second.id, isolated)
                awaitScript(separate, "document.readyState === 'complete' && !!window.localApp")
                assertEquals("null", evaluate(separate, "localStorage.getItem('acceptance-note')"))
                android.util.Log.i("LocalAppAcceptance", "BROWSER_STORAGE_SDK PASS real WebView info permission-denial child-frame-denial stopped-run-denial persistent-origin isolated-profile")
            } catch (error: Throwable) { failure = error; throw error
            } finally {
                for (id in created) runCatching { apps.remove(id) }.onFailure { cleanup ->
                    if (failure != null) failure.addSuppressed(cleanup) else throw cleanup
                }
                for (source in sources) source.deleteRecursively()
            }
        }
    }

    @Test
    fun staticPythonNodeAndOwnedLifecycle() = runBlocking<Unit> {
        val instrumentation = InstrumentationRegistry.getInstrumentation()
        val context = instrumentation.targetContext
        val fixtures = File(context.filesDir, "local-app-acceptance-fixtures")
        check(File(fixtures, "node/package-lock.json").isFile) { "Stage the reviewed cross-platform fixtures before running acceptance" }
        val apps = UnifiedLocalApps.get(context)
        val created = mutableListOf<String>()
        val sources = mutableListOf<File>()
        NativeAppActivityHost(android.content.Intent(context, MainActivity::class.java).addFlags(android.content.Intent.FLAG_ACTIVITY_NEW_TASK)).use {
            instrumentation.waitForIdleSync()
            suspend fun publish(name: String): AppRecord {
                val guest = "/home/lingxi/.lingxi/.apps/acceptance-${UUID.randomUUID()}"
                val source = apps.hostFile(guest)
                check(File(fixtures, name).copyRecursively(source))
                sources.add(source)
                val value = apps.request("publish") {
                    put("sourcePath", source.absolutePath); put("expectedRevision", 0)
                    put("authoring", JSONObject().put("sourceDirectory", guest).put("sourceSessionId", "acceptance-source"))
                } as JSONObject
                return value.getJSONObject("record").appRecord().also { created.add(it.id) }
            }
            try {
                val static = publish("static")
                val staticLaunch = apps.start(static.id)
                assertTrue(http(staticLaunch, "", entry = true).second.decodeToString().contains("html"))
                val movedSession = apps.request("prepareAuthoring", static.id) {
                    put("sessionDirectories", JSONObject().put("acceptance-source", "/home/lingxi/other-project"))
                } as JSONObject
                assertEquals("createSession", movedSession.getString("action"))
                assertEquals(static.authoring!!.sourceDirectory, movedSession.getString("sourceDirectory"))
                sources.first().deleteRecursively()
                assertEquals("sourceMissing", (apps.request("prepareAuthoring", static.id) {
                    put("sessionDirectories", JSONObject())
                } as JSONObject).getString("action"))
                assertEquals(200, http(staticLaunch, "", entry = true).first)
                android.util.Log.i("LocalAppAcceptance", "STATIC_SOURCE_INDEPENDENCE PASS appId=${static.id}")

                val mode = LinuxRuntimeMode.MobileLinux
                when (LinuxRuntimeBridge.load(context, mode).rootfsState) {
                    MobileLinuxRootfsStateFfi.MISSING -> LinuxRuntimeBridge.install(context, mode)
                    MobileLinuxRootfsStateFfi.CORRUPT -> error("Existing rootfs is corrupt; repair requires a separate acceptance step")
                    else -> Unit
                }
                val linux = buildAndroidMobileLinuxRuntimeHandle(LinuxRuntimeBridge.configForWorkspace(context, mode))
                assertEquals(MobileLinuxRootfsStateFfi.READY, linux.boot().state)
                val tools = linux.runCommand(MobileLinuxCommandRequestFfi(
                    command = "/bin/sh", args = listOf("-lc", "set -e; id -u; printf '%s\\n' \"\$HOME\"; command -v python3; command -v node; command -v npm"),
                    cwd = null, env = emptyList(), stdin = null, timeoutMs = 30_000uL, allowNetwork = false, mounts = emptyList(),
                ))
                assertEquals(tools.stderr, 0, tools.exitCode)
                assertTrue(tools.stdout, tools.stdout.startsWith("1000\n/home/lingxi\n"))

                val python = publish("python")
                val node = publish("node")
                for (app in listOf(python, node)) {
                    val denied = runCatching { apps.start(app.id) }.exceptionOrNull()
                    assertNotNull("Execution must be refused before exact-release consent", denied)
                    val release = apps.release(app.id, app.currentRevision)
                    apps.grant(app.id, release.digest)
                }
                val py = apps.start(python.id)
                val js = apps.start(node.id)
                val bytes = byteArrayOf(0, 1, -1, 42)
                for ((launch, health) in listOf(py to "PYTHON-READY", js to "NODE-READY")) {
                    assertEquals(health, http(launch, "/health").second.decodeToString())
                    assertArrayEquals(bytes, http(launch, "/echo", bytes).second)
                    val stream = connection(launch, "/events")
                    stream.inputStream.bufferedReader().use { reader -> assertEquals("data: 0", reader.readLine()) }
                    stream.disconnect()
                    assertEquals(health, http(launch, "/health").second.decodeToString())
                }
                assertArrayEquals(bytes, http(py, "/save", bytes).second)
                val layout = apps.request("layout", python.id) { put("revision", py.run.revision) } as JSONObject
                assertArrayEquals(bytes, File(layout.getString("dataPath"), "acceptance.bin").readBytes())
                val socket = binaryWebSocket(js, bytes)
                val pty = linux.openPty(MobileLinuxPtyOpenRequestFfi(
                    command = "/bin/sh", args = listOf("-i"), cwd = null, env = emptyList(),
                    size = MobileLinuxPtySizeFfi(cols = 80u, rows = 24u), mounts = emptyList(),
                ))
                try {
                    assertNotNull("Maintenance must be rejected while app services and a terminal own Linux", runCatching { linux.repairRootfs() }.exceptionOrNull())
                    apps.stop(python.id)
                    assertEquals("NODE-READY", http(js, "/health").second.decodeToString())
                    assertEquals(200, http(staticLaunch, "", entry = true).first)
                    val command = linux.runCommand(MobileLinuxCommandRequestFfi(
                        command = "/bin/sh", args = listOf("-lc", "printf ordinary-command-still-alive"), cwd = null,
                        env = emptyList(), stdin = null, timeoutMs = 10_000uL, allowNetwork = false, mounts = emptyList(),
                    ))
                    assertEquals(0, command.exitCode); assertEquals("ordinary-command-still-alive", command.stdout)
                    val restarted = apps.start(node.id, "restart")
                    assertEquals(js.origin, restarted.origin)
                    assertNotEquals(js.run.runId, restarted.run.runId)
                    assertEquals(410, http(js, "/health").first)
                    val closed = try { val next = socket.getInputStream().read(); next < 0 || next and 15 == 8 } catch (_: SocketException) { true }
                    assertTrue("Old WebSocket must close after its run is replaced", closed)
                    assertEquals("NODE-READY", http(restarted, "/health").second.decodeToString())
                    val repaired = apps.start(python.id, "repair")
                    assertEquals(py.origin, repaired.origin)
                    assertArrayEquals(bytes, File(layout.getString("dataPath"), "acceptance.bin").readBytes())
                    apps.revoke(node.id)
                    assertNotNull(runCatching { apps.start(node.id) }.exceptionOrNull())
                    android.util.Log.i("LocalAppAcceptance", "MOBILE_LINUX_SERVICES PASS python+node POST binary SSE-cancel WS restart-revocation data owned-stop maintenance")
                } finally {
                    socket.close()
                    linux.closePty(pty)
                }
            } finally {
                for (id in created.asReversed()) apps.stop(id)
                for (source in sources) source.deleteRecursively()
            }
        }
    }

    private fun connection(launch: AppLaunch, path: String, entry: Boolean = false): HttpURLConnection =
        (URL(if (entry) launch.url else appServiceUrl(launch, "api", path, false)).openConnection() as HttpURLConnection).apply {
            connectTimeout = 10_000; readTimeout = 15_000; setRequestProperty("Origin", launch.origin)
        }

    private fun http(launch: AppLaunch, path: String, body: ByteArray? = null, entry: Boolean = false): Pair<Int, ByteArray> {
        val request = connection(launch, path, entry)
        try {
            if (body != null) {
                request.requestMethod = "POST"; request.doOutput = true; request.setFixedLengthStreamingMode(body.size)
                request.setRequestProperty("Content-Type", "application/octet-stream")
                request.outputStream.use { it.write(body) }
            }
            val code = request.responseCode
            val input = if (code >= 400) request.errorStream else request.inputStream
            return code to (input?.use { it.readBytes() } ?: byteArrayOf())
        } finally { request.disconnect() }
    }

    /** Minimal masked binary client; traffic still crosses the product's native WS broker. */
    private fun binaryWebSocket(launch: AppLaunch, bytes: ByteArray): Socket {
        val uri = URI(appServiceUrl(launch, "api", "/socket", true))
        val socket = Socket(uri.host, uri.port).apply { soTimeout = 10_000 }
        try {
            val output = socket.getOutputStream(); val input = socket.getInputStream()
            output.write(("GET ${uri.rawPath} HTTP/1.1\r\nHost: ${uri.host}:${uri.port}\r\n" +
                "Origin: ${launch.origin}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n" +
                "Sec-WebSocket-Version: 13\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\r\n").toByteArray())
            output.flush()
            val header = StringBuilder()
            while (!header.endsWith("\r\n\r\n")) { val c = input.read(); check(c >= 0 && header.length < 8192); header.append(c.toChar()) }
            assertTrue(header.toString(), header.startsWith("HTTP/1.1 101"))
            val mask = byteArrayOf(1, 2, 3, 4)
            output.write(byteArrayOf(0x82.toByte(), (0x80 or bytes.size).toByte()) + mask + bytes.mapIndexed { index, byte -> (byte.toInt() xor mask[index % 4].toInt()).toByte() }.toByteArray())
            output.flush()
            assertEquals(0x82, input.read()); assertEquals(bytes.size, input.read())
            val echoed = ByteArray(bytes.size)
            for (index in echoed.indices) { val next = input.read(); check(next >= 0); echoed[index] = next.toByte() }
            assertArrayEquals(bytes, echoed)
            return socket
        } catch (error: Throwable) { socket.close(); throw error }
    }
}
