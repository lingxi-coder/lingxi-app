package com.lingxi.code.apps

import android.content.Intent
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

/** Two-stage real host SIGKILL acceptance. The external runner kills only the
 * recorded target PID after prepare; force-stop would mask PRoot orphan leaks.
 */
@RunWith(AndroidJUnit4::class)
class UnifiedLocalAppProcessDeathTest {
    @Test
    fun hostSigkillRequiresFreshRunAndPreservesData() = runBlocking<Unit> {
        requireAcceptanceOptIn(
            "ownerDeathAcceptance", "Two-stage owner-death acceptance needs its external runner",
            "com.lingxi.code.apps.UnifiedLocalAppProcessDeathTest#hostSigkillRequiresFreshRunAndPreservesData", "ownerDeathPhase",
        )
        check(BuildConfig.MOBILE_LINUX_FULL) { "Owner-death acceptance requires the Direct package" }
        val instrumentation = InstrumentationRegistry.getInstrumentation()
        val context = instrumentation.targetContext
        val phase = InstrumentationRegistry.getArguments().getString("ownerDeathPhase")
        require(phase == "prepare" || phase == "recover") { "Use the two-stage owner-death acceptance runner" }
        val receipt = File(context.filesDir, "local-app-owner-death-acceptance.json")
        val apps = UnifiedLocalApps.get(context)
        NativeAppActivityHost(Intent(context, MainActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)).use {
            if (phase == "prepare") {
                check(!receipt.exists()) { "A previous acceptance receipt must be recovered first" }
                val source = apps.hostFile(com.lingxi.code.project.MobileUserStorage.directories(context).appsGuestPath.trimEnd('/') + "/owner-death-${UUID.randomUUID()}")
                check(File(context.filesDir, "local-app-acceptance-fixtures/python").copyRecursively(source))
                val value = apps.request("publish") { put("sourcePath", source.absolutePath); put("expectedRevision", 0) } as JSONObject
                val record = value.getJSONObject("record").appRecord()
                val release = value.getJSONObject("release").appRelease()
                try {
                    apps.grant(record.id, release.digest)
                    apps.setKeepRunning(record.id, release.digest, true)
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
                    receipt.writeText(JSONObject().put("appId", record.id).put("source", source.absolutePath)
                        .put("runId", launch.run.runId).put("origin", launch.origin).put("currentRevision", record.currentRevision).put("candidateRevision", release2.revision)
                        .put("backendPort", oldStatus.getJSONObject("run").getJSONArray("services").getJSONObject(0).getInt("port"))
                        .put("dataPath", layout.getString("dataPath")).put("data", data.decodeToString())
                        .put("hostPid", android.os.Process.myPid()).put("markerPath", marker.absolutePath)
                        .put("buildPid", build.getInt("pid")).put("buildStartTicks", build.getString("startTicks"))
                        .put("buildStarts", build.getInt("starts")).toString())
                    android.util.Log.i("LocalAppAcceptance", "OWNER_DEATH_READY hostPid=${android.os.Process.myPid()} appId=${record.id} runId=${launch.run.runId}")
                    withTimeout(120_000) { awaitCancellation() }
                } finally {
                    withContext(NonCancellable) { apps.remove(record.id); source.deleteRecursively(); receipt.delete() }
                }
            } else {
                val saved = JSONObject(receipt.readText())
                val id = saved.getString("appId")
                val oldRun = saved.getString("runId")
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
                    withContext(Dispatchers.IO) {
                        val request = URL(appServiceUrl(fresh, "api", "/health", false)).openConnection() as HttpURLConnection
                        request.connectTimeout = 5000; request.readTimeout = 5000; request.setRequestProperty("Origin", fresh.origin)
                        try { assertEquals("PYTHON-READY", request.inputStream.bufferedReader().use { it.readText() }) }
                        finally { request.disconnect() }
                    }
                    assertEquals(saved.getString("data"), dataFile.readText())
                    assertCandidateStoppedAndNotRerun()
                    android.util.Log.i("LocalAppAcceptance", "OWNER_DEATH_RECOVERY PASS exact-candidate-PID-start-time heartbeat-stopped start-count-unchanged old-backend-closed interrupted-history new-run stable-origin preserved-data interrupted-update-not-rerun old-release-retained real-Python")
                } finally {
                    apps.remove(id)
                    File(saved.getString("source")).deleteRecursively()
                    receipt.delete()
                }
            }
        }
        if (phase == "recover") {
            instrumentation.addResults(android.os.Bundle().apply { putString("ownerDeathResult", "RECOVERED") })
        }
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
