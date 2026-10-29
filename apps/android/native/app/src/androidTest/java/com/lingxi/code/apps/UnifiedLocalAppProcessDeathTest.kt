package com.lingxi.code.apps

import android.content.Intent
import android.util.AtomicFile
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import com.lingxi.code.BuildConfig
import com.lingxi.code.MainActivity
import com.lingxi.code.acceptance.requireAcceptanceOptIn
import kotlinx.coroutines.*
import org.json.JSONObject
import org.junit.Assert.*
import org.junit.Test
import org.junit.runner.RunWith
import java.io.File
import java.net.InetSocketAddress
import java.net.Socket
import java.net.HttpURLConnection
import java.net.URL
import java.util.UUID

/** Two-stage real host SIGKILL acceptance. Each test process terminates itself
 * after recording its checkpoint; force-stop would mask PRoot orphan leaks.
 */
@RunWith(AndroidJUnit4::class)
class UnifiedLocalAppProcessDeathTest {
    @Test
    fun hostSigkillRequiresFreshRunAndPreservesData() = runBlocking<Unit> {
        requireAcceptanceOptIn(
            "ownerDeathAcceptance", "Two-stage owner-death acceptance needs its external runner",
            "com.lingxi.code.apps.UnifiedLocalAppProcessDeathTest#hostSigkillRequiresFreshRunAndPreservesData",
            "ownerDeathCiCapability", "ownerDeathPhase", "ownerDeathRunNonce",
        )
        check(BuildConfig.MOBILE_LINUX_FULL) { "Owner-death acceptance requires the Direct package" }
        val instrumentation = InstrumentationRegistry.getInstrumentation()
        val context = instrumentation.targetContext
        val phase = InstrumentationRegistry.getArguments().getString("ownerDeathPhase")
        require(phase == "prepare" || phase == "recover") { "Use the two-stage owner-death acceptance runner" }
        val runNonce = InstrumentationRegistry.getArguments().getString("ownerDeathRunNonce")
        require(runNonce != null && Regex("[0-9a-f]{64}").matches(runNonce)) {
            "Owner-death receipt requires a runner-issued invocation nonce"
        }
        val ciCapability = InstrumentationRegistry.getArguments().getString("ownerDeathCiCapability")
        require(ciCapability != null && Regex("[0-9a-f]{64}").matches(ciCapability)) {
            "Owner-death self-SIGKILL requires a one-time runner/test capability"
        }
        check(android.os.Build.HARDWARE in setOf("ranchu", "goldfish")) {
            "Owner-death self-SIGKILL requires an Android emulator"
        }
        val processName = File("/proc/${android.os.Process.myPid()}/cmdline").readBytes()
            .toString(Charsets.UTF_8).substringBefore('\u0000')
        check(processName == context.packageName) {
            "Owner-death test must run in the target app/service process; got $processName"
        }
        consumeCiCapability(context, ciCapability)
        val receipt = File(context.filesDir, "local-app-owner-death-acceptance.json")
        val setupReceipt = File(context.filesDir, "local-app-owner-death-setup.json")
        var recoveryCleanupCompleted = false
        NativeAppActivityHost(Intent(context, MainActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)).use { activityHost ->
            if (phase == "prepare") {
                check(!receipt.exists()) { "A previous acceptance receipt must be recovered first" }
                check(!setupReceipt.exists()) { "A previous owner-death setup receipt must be recovered first" }
                val configHome = newOwnerDeathConfigHome(context)
                val configToken = configHome.name.removePrefix(UnifiedLocalApps.OWNER_DEATH_ACCEPTANCE_CONFIG_PREFIX)
                var scopedApps: UnifiedLocalApps? = null
                var publishedAppId: String? = null
                var ownsConfigHome = false
                var ownerMarkerWritten = false
                var sourceHostPath: String? = null
                var candidateBuildPid: Int? = null
                var candidateBuildStartTicks: String? = null
                var candidateUpdateStarted = false
                var appMayHaveRun = false
                val knownBackendPorts = mutableSetOf<Int>()
                var cleaned = false
                suspend fun cleanupPrepare() {
                    if (cleaned) return
                    cleaned = true
                    withContext(NonCancellable) {
                        var appStopped = publishedAppId == null
                        publishedAppId?.let { id ->
                            appStopped = scopedApps?.let { apps -> runCatching { apps.remove(id) }.isSuccess } ?: false
                        }
                        val buildStopped = when {
                            !candidateUpdateStarted -> true
                            candidateBuildPid != null && candidateBuildStartTicks != null ->
                                runCatching {
                                    withTimeout(5000) {
                                        while (processIsRunning(candidateBuildPid!!, candidateBuildStartTicks!!)) delay(100)
                                    }
                                }.isSuccess
                            else -> false
                        }
                        val portsKnown = !appMayHaveRun || knownBackendPorts.isNotEmpty()
                        val portsClosed = appStopped && portsKnown && runCatching { assertPortsClosed(knownBackendPorts) }.isSuccess
                        if (appStopped && buildStopped && portsClosed && setupReceipt.exists()) {
                            setupReceipt.writeText(JSONObject()
                                .put("cleanupReady", true)
                                .put("appRemoved", true)
                                .put("backendPortsClosed", true)
                                .put("candidateBuildStopped", true)
                                .put("configHome", configHome.absolutePath)
                                .put("configToken", configToken)
                                .put("appId", publishedAppId ?: "")
                                .put("configCreated", ownsConfigHome)
                                .put("ownerMarkerWritten", ownerMarkerWritten)
                                .put("source", sourceHostPath ?: "")
                                .put("cleanupHostPid", android.os.Process.myPid())
                                .put("cleanupHostStartTicks", currentProcessStartTicks())
                                .toString())
                        } else {
                            android.util.Log.e("LocalAppAcceptance", "Prepare cleanup is unproven; preserving setup receipt and isolated config")
                        }
                    }
                }
                try {
                    setupReceipt.writeText(JSONObject()
                        .put("cleanupReady", false)
                        .put("appRemoved", false)
                        .put("backendPortsClosed", false)
                        .put("candidateBuildStopped", false)
                        .put("configHome", configHome.absolutePath)
                        .put("configToken", configToken)
                        .put("configCreated", false)
                        .put("ownerMarkerWritten", false)
                        .put("source", "")
                        .put("cleanupHostPid", android.os.Process.myPid())
                        .put("cleanupHostStartTicks", currentProcessStartTicks())
                        .toString())
                    check(!configHome.exists()) { "Owner-death acceptance config path already exists" }
                    check(configHome.mkdirs()) { "Cannot create isolated app config home" }
                    ownsConfigHome = true
                    File(configHome, UnifiedLocalApps.OWNER_DEATH_ACCEPTANCE_OWNER_MARKER).writeText(configToken)
                    ownerMarkerWritten = true
                    val appsRoot = File(configHome, ".apps")
                    check(appsRoot.mkdirs()) { "Cannot create isolated app source root" }
                    val apps = UnifiedLocalApps.createOwnerDeathAcceptanceInstance(context, configHome)
                    scopedApps = apps
                    val sourceDirectory = ownerDeathSourceDirectory(context, configHome)
                    val source = apps.hostFile(sourceDirectory)
                    sourceHostPath = source.absolutePath
                    check(source.parentFile?.mkdirs() == true || source.parentFile?.isDirectory == true)
                    check(File(context.filesDir, "local-app-acceptance-fixtures/python").copyRecursively(source))
                    assertEquals(sourceDirectory, apps.guestPath(source.absolutePath))
                    val workspaceId = UUID.randomUUID().toString()
                    val projectRef = AppProjectRef("mobileProject", projectId = UUID.randomUUID().toString())
                    val sessionId = UUID.randomUUID().toString()
                    val pending = apps.registerAuthoring(JSONObject()
                        .put("workspaceId", workspaceId)
                        .put("sourceDirectory", sourceDirectory)
                        .put("projectRef", projectRef.toJson())
                        .put("authoringSessionId", sessionId))
                    assertNull(pending.appId)
                    assertEquals(sourceDirectory, pending.sourceDirectory)
                    assertEquals(workspaceId, pending.workspaceId)
                    assertEquals(projectRef, pending.projectRef)
                    assertEquals(sessionId, pending.authoringSessionId)
                    val value = apps.request("publish") { put("sourcePath", source.absolutePath); put("expectedRevision", 0) } as JSONObject
                    val recordValue = value.getJSONObject("record")
                    publishedAppId = recordValue.optString("appId").takeIf { it.isNotBlank() }
                    val record = recordValue.appRecord()
                    assertEquals(sourceDirectory, record.authoring?.sourceDirectory)
                    assertEquals(record.id, record.authoring?.appId)
                    val release = value.getJSONObject("release").appRelease()
                    apps.grant(record.id, release.digest)
                    apps.setKeepRunning(record.id, release.digest, true)
                    appMayHaveRun = true
                    val launch = apps.start(record.id)
                    val data = "owner-death-preserve-${UUID.randomUUID()}".toByteArray()
                    withContext(Dispatchers.IO) {
                        val request = URL(appServiceUrl(launch, "api", "/save", false)).openConnection() as HttpURLConnection
                        request.connectTimeout = 5000; request.readTimeout = 5000; request.setRequestProperty("Origin", launch.origin)
                        try {
                            request.requestMethod = "POST"; request.doOutput = true
                            request.outputStream.use { it.write(data) }
                            assertArrayEquals(data, request.inputStream.use { it.readBytes() })
                        } finally { request.disconnect() }
                    }
                    val oldStatus = apps.request("status", record.id) as JSONObject
                    val oldServices = oldStatus.getJSONObject("run").getJSONArray("services")
                    for (index in 0 until oldServices.length()) {
                        val port = oldServices.getJSONObject(index).getInt("port")
                        require(port in 1..65535) { "Invalid owner-death service port" }
                        knownBackendPorts += port
                    }
                    val manifest = File(source, "lingxi-app.json")
                    val marker = File(source, "build-in-progress")
                    val markerPath = apps.guestPath(marker.absolutePath)
                    File(source, "owner-build-probe.py").writeText("""
                        import json, os, time
                        from pathlib import Path
                        marker = Path(r"$markerPath")
                        prior = json.loads(marker.read_text()) if marker.exists() else {}
                        fields = Path('/proc/self/stat').read_text().rsplit(')', 1)[1].split()
                        state = {'pid': os.getpid(), 'startTicks': fields[19], 'starts': prior.get('starts', 0) + 1, 'heartbeat': 0}
                        while True:
                            state['heartbeat'] += 1
                            temporary = marker.with_suffix('.tmp')
                            temporary.write_text(json.dumps(state))
                            temporary.replace(marker)
                            time.sleep(0.1)
                    """.trimIndent())
                    val changed = JSONObject(manifest.readText()).put("build", org.json.JSONArray().put(JSONObject()
                        .put("command", "python3 owner-build-probe.py")))
                    changed.getJSONObject("package").getJSONArray("files").put("owner-build-probe.py")
                    manifest.writeText(changed.toString())
                    val candidate = apps.request("publish", record.id) { put("sourcePath", source.absolutePath); put("expectedRevision", record.latestRevision) } as JSONObject
                    val release2 = candidate.getJSONObject("release").appRelease()
                    apps.grant(record.id, release2.digest)
                    candidateUpdateStarted = true
                    async { apps.start(record.id, revision = release2.revision) }
                    withTimeout(20_000) {
                        while (true) {
                            val current = (apps.request("status", record.id) as JSONObject).optJSONObject("preparingRun")
                            if (current?.optLong("revision") == release2.revision && current.optString("phase") == "preparing") break
                            delay(100)
                        }
                    }
                    withTimeout(20_000) { while (!marker.isFile) delay(100) }
                    val build = JSONObject(marker.readText())
                    candidateBuildPid = build.getInt("pid")
                    candidateBuildStartTicks = build.getString("startTicks")
                    assertTrue(build.getInt("heartbeat") > 0)
                    assertTrue(processIsRunning(build.getInt("pid"), build.getString("startTicks")))
                    assertEquals(record.currentRevision, apps.get(record.id).currentRevision)
                    withContext(Dispatchers.IO) {
                        val check = URL(appServiceUrl(launch, "api", "/health", false)).openConnection() as HttpURLConnection
                        check.connectTimeout = 5000; check.readTimeout = 5000; check.setRequestProperty("Origin", launch.origin)
                        try { assertEquals("PYTHON-READY", check.inputStream.bufferedReader().use { it.readText() }) }
                        finally { check.disconnect() }
                    }
                    val layout = apps.request("layout", record.id) as JSONObject
                    writeReceipt(receipt, JSONObject().put("appId", record.id).put("source", source.absolutePath)
                        .put("configHome", configHome.absolutePath).put("configToken", configToken)
                        .put("ownerDeathPhase", "prepared").put("ownerDeathRunNonce", runNonce)
                        .put("recoveryComplete", false).put("appRemoved", false)
                        .put("backendPortsClosed", false).put("candidateBuildStopped", false)
                        .put("configCreated", ownsConfigHome).put("ownerMarkerWritten", ownerMarkerWritten)
                        .put("runId", launch.run.runId).put("origin", launch.origin).put("currentRevision", record.currentRevision).put("candidateRevision", release2.revision)
                        .put("backendPort", oldStatus.getJSONObject("run").getJSONArray("services").getJSONObject(0).getInt("port"))
                        .put("dataPath", layout.getString("dataPath")).put("data", data.decodeToString())
                        .put("hostPid", android.os.Process.myPid()).put("hostStartTicks", currentProcessStartTicks())
                        .put("hostProcessName", processName)
                        .put("markerPath", marker.absolutePath)
                        .put("buildPid", build.getInt("pid")).put("buildStartTicks", build.getString("startTicks"))
                        .put("buildStarts", build.getInt("starts")))
                    android.util.Log.i("LocalAppAcceptance", "OWNER_DEATH_READY hostPid=${android.os.Process.myPid()} appId=${record.id} runId=${launch.run.runId}")
                    // Finish the activity before killing this process so the
                    // OS has no foreground activity to recreate. SIGKILL from
                    // the process itself has no PID-reuse window at the signal.
                    activityHost.close()
                    writeReceipt(receipt, JSONObject(receipt.readText())
                        .put("prepareSelfKillRequested", true))
                    android.os.Process.killProcess(android.os.Process.myPid())
                    error("Self SIGKILL returned unexpectedly")
                } finally {
                    cleanupPrepare()
                }
            } else {
                val saved = JSONObject(receipt.readText())
                check(saved.optString("ownerDeathRunNonce") == runNonce) {
                    "Recovery receipt belongs to a different runner invocation"
                }
                val configToken = saved.getString("configToken")
                val configHome = validateOwnerDeathConfigHome(context, saved.getString("configHome"), configToken)
                val apps = UnifiedLocalApps.createOwnerDeathAcceptanceInstance(context, configHome)
                val id = saved.getString("appId")
                val oldRun = saved.getString("runId")
                val oldBackendPort = saved.getInt("backendPort").also { require(it in 1..65535) }
                var freshBackendPort: Int? = null
                assertNotEquals(saved.getInt("hostPid"), android.os.Process.myPid())
                suspend fun assertCandidateStoppedAndNotRerun() {
                    withTimeout(5000) {
                        while (processIsRunning(saved.getInt("buildPid"), saved.getString("buildStartTicks"))) delay(100)
                    }
                    val marker = File(saved.getString("markerPath"))
                    val before = JSONObject(marker.readText())
                    assertEquals(saved.getInt("buildStarts"), before.getInt("starts"))
                    assertEquals(saved.getInt("buildPid"), before.getInt("pid"))
                    assertEquals(saved.getString("buildStartTicks"), before.getString("startTicks"))
                    delay(1000)
                    assertEquals("Candidate heartbeat must stay stopped", before.toString(), JSONObject(marker.readText()).toString())
                }
                var recoveryAssertionsPassed = false
                try {
                    assertCandidateStoppedAndNotRerun()
                    withContext(Dispatchers.IO) {
                        withTimeout(5000) {
                            while (runCatching { Socket().use { it.connect(InetSocketAddress("127.0.0.1", saved.getInt("backendPort")), 300) } }.isSuccess) delay(100)
                        }
                    }
                    assertEquals("interrupted", apps.runs(id).find { it.runId == oldRun }?.phase)
                    assertTrue(apps.runs(id).any { it.revision == saved.getLong("candidateRevision") && it.phase == "interrupted" })
                    assertEquals(saved.getLong("currentRevision"), apps.get(id).currentRevision)
                    val dataFile = File(saved.getString("dataPath"), "acceptance.bin")
                    assertEquals(saved.getString("data"), dataFile.readText())
                    val fresh = apps.start(id)
                    assertNotEquals(oldRun, fresh.run.runId)
                    assertEquals(saved.getString("origin"), fresh.origin)
                    val freshStatus = apps.request("status", id) as JSONObject
                    val freshServices = freshStatus.getJSONObject("run").getJSONArray("services")
                    require(freshServices.length() > 0) { "Fresh app run did not expose a service port" }
                    freshBackendPort = freshServices.getJSONObject(0).getInt("port").also { require(it in 1..65535) }
                    withContext(Dispatchers.IO) {
                        val request = URL(appServiceUrl(fresh, "api", "/health", false)).openConnection() as HttpURLConnection
                        request.connectTimeout = 5000; request.readTimeout = 5000; request.setRequestProperty("Origin", fresh.origin)
                        try { assertEquals("PYTHON-READY", request.inputStream.bufferedReader().use { it.readText() }) }
                        finally { request.disconnect() }
                    }
                    assertEquals(saved.getString("data"), dataFile.readText())
                    assertCandidateStoppedAndNotRerun()
                    recoveryAssertionsPassed = true
                } finally {
                    if (recoveryAssertionsPassed) {
                        apps.stop(id)
                        assertPortsClosed(setOf(oldBackendPort, requireNotNull(freshBackendPort)))
                        apps.remove(id)
                        assertEquals(saved.getString("data"), File(saved.getString("dataPath"), "acceptance.bin").readText())
                        recoveryCleanupCompleted = true
                    } else {
                        // Keep the library and receipt for another recovery attempt. Stop a
                        // fresh serving run without removing its durable app record.
                        runCatching { apps.stop(id) }
                        android.util.Log.e("LocalAppAcceptance", "Owner-death recovery failed; preserving receipt and isolated config")
                    }
                }
                android.util.Log.i("LocalAppAcceptance", "OWNER_DEATH_RECOVERY PASS exact-candidate-PID-start-time heartbeat-stopped start-count-unchanged old-backend-closed interrupted-history new-run stable-origin preserved-data interrupted-update-not-rerun old-release-retained real-Python")
            }
        }
        if (phase == "recover") {
            check(recoveryCleanupCompleted) { "Recovery assertions and app removal did not complete" }
            val recovered = JSONObject(receipt.readText())
                .put("ownerDeathPhase", "recovered")
                .put("recoveryComplete", true)
                .put("appRemoved", true)
                .put("backendPortsClosed", true)
                .put("candidateBuildStopped", true)
                .put("configCreated", true)
                .put("ownerMarkerWritten", true)
                .put("recoveryHostPid", android.os.Process.myPid())
                .put("recoveryHostStartTicks", currentProcessStartTicks())
                .put("recoveryProcessName", processName)
            writeReceipt(receipt, recovered)
            instrumentation.addResults(android.os.Bundle().apply {
                putString("ownerDeathResult", "RECOVERED")
                putString("ownerDeathRecoveryReady", "true")
                putString("ownerDeathRecoveryHostPid", recovered.getInt("recoveryHostPid").toString())
                putString("ownerDeathRecoveryHostStartTicks", recovered.getString("recoveryHostStartTicks"))
            })
            writeReceipt(receipt, JSONObject(receipt.readText())
                .put("recoverySelfKillRequested", true))
            android.os.Process.killProcess(android.os.Process.myPid())
            error("Recovery host self SIGKILL returned unexpectedly")
        }
    }

    private fun newOwnerDeathConfigHome(context: android.content.Context): File {
        val home = File(com.lingxi.code.project.MobileUserStorage.directories(context).home).canonicalFile
        return File(home, UnifiedLocalApps.OWNER_DEATH_ACCEPTANCE_CONFIG_PREFIX + UUID.randomUUID().toString()).canonicalFile
    }

    /** Consume a one-time runner/test handshake; this does not attest the runner's host identity. */
    private fun consumeCiCapability(context: android.content.Context, expected: String) {
        val capability = File(context.filesDir, "local-app-owner-death-ci-capability")
        require(capability.absoluteFile.path == capability.canonicalFile.path && capability.isFile &&
            capability.readText() == expected) {
            "Owner-death test is missing its one-time runner/test capability"
        }
        check(capability.delete() && !capability.exists()) {
            "Owner-death CI capability could not be consumed exactly once"
        }
    }

    private fun writeReceipt(file: File, value: JSONObject) {
        val atomic = AtomicFile(file)
        val stream = atomic.startWrite()
        try {
            stream.write(value.toString().toByteArray(Charsets.UTF_8))
            atomic.finishWrite(stream)
        } catch (error: Throwable) {
            atomic.failWrite(stream)
            throw error
        }
    }

    private fun validateOwnerDeathConfigHome(context: android.content.Context, path: String, token: String): File {
        val home = File(com.lingxi.code.project.MobileUserStorage.directories(context).home).canonicalFile
        val supplied = File(path).absoluteFile
        val configHome = supplied.canonicalFile
        require(supplied.path == configHome.path && configHome.parentFile == home &&
            configHome.name == UnifiedLocalApps.OWNER_DEATH_ACCEPTANCE_CONFIG_PREFIX + token) {
            "Refusing an owner-death config outside this test's canonical home child"
        }
        require(runCatching { UUID.fromString(token).toString() == token }.getOrDefault(false)) {
            "Refusing an owner-death config without a canonical UUID"
        }
        require(configHome.isDirectory) { "Owner-death config directory is missing" }
        val marker = File(configHome, UnifiedLocalApps.OWNER_DEATH_ACCEPTANCE_OWNER_MARKER)
        require(marker.absoluteFile.path == marker.canonicalFile.path && marker.isFile && marker.readText() == token) {
            "Owner-death config owner marker does not match the receipt"
        }
        return configHome
    }

    private fun ownerDeathSourceDirectory(context: android.content.Context, configHome: File): String {
        val token = configHome.name.removePrefix(UnifiedLocalApps.OWNER_DEATH_ACCEPTANCE_CONFIG_PREFIX)
        val root = validateOwnerDeathConfigHome(context, configHome.path, token)
        val dirs = com.lingxi.code.project.MobileUserStorage.directories(context)
        val home = File(dirs.home).canonicalFile.toPath()
        val appsRoot = File(root, ".apps")
        require(appsRoot.absoluteFile.path == appsRoot.canonicalFile.path && appsRoot.isDirectory) {
            "Owner-death app source root must be a real directory"
        }
        val source = File(appsRoot, "owner-death-${UUID.randomUUID()}").absoluteFile
        require(!source.exists() && source.canonicalFile.parentFile == appsRoot.canonicalFile) {
            "Owner-death source must be a new direct child of the isolated app root"
        }
        val relative = home.relativize(source.canonicalFile.toPath()).toString().replace(File.separatorChar, '/')
        return dirs.guestHome.trimEnd('/') + "/" + relative
    }

    private suspend fun assertPortsClosed(ports: Set<Int>) {
        for (port in ports) {
            withContext(Dispatchers.IO) {
                withTimeout(5000) {
                    while (runCatching { Socket().use { it.connect(InetSocketAddress("127.0.0.1", port), 300) } }.isSuccess) {
                        delay(100)
                    }
                }
            }
        }
    }

    private fun currentProcessStartTicks(): String {
        val fields = File("/proc/${android.os.Process.myPid()}/stat").readText()
            .substringAfterLast(")").trim().split(Regex("\\s+"))
        require(fields.size > 19) { "Unable to read the owner-death host process identity" }
        return fields[19]
    }

    private fun processIsRunning(pid: Int, startTicks: String): Boolean {
        val stat = File("/proc/$pid/stat")
        if (!stat.exists()) return false
        val text = try { stat.readText() } catch (error: java.io.FileNotFoundException) {
            if (!stat.exists()) return false
            throw error
        }
        val fields = text.substringAfterLast(")").trim().split(Regex("\\s+"))
        return fields[0] != "Z" && fields[19] == startTicks
    }
}
