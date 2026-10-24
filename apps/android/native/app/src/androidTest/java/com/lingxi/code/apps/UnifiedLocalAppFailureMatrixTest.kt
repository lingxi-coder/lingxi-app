package com.lingxi.code.apps

import android.content.Intent
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import com.lingxi.code.MainActivity
import kotlinx.coroutines.*
import org.json.JSONArray
import org.json.JSONObject
import org.junit.Assert.*
import org.junit.Test
import org.junit.runner.RunWith
import java.io.File
import java.net.HttpURLConnection
import java.net.InetAddress
import java.net.ServerSocket
import java.net.URL
import java.util.UUID

@RunWith(AndroidJUnit4::class)
class UnifiedLocalAppFailureMatrixTest {
    @Test fun preparationWriteAndDownloadFailuresKeepServingReleaseAndData() = runBlocking<Unit> {
        val context = InstrumentationRegistry.getInstrumentation().targetContext
        val apps = UnifiedLocalApps.get(context)
        val source = apps.hostFile("/home/lingxi/.lingxi/.apps/failure-matrix-${UUID.randomUUID()}")
        check(File(context.filesDir, "local-app-acceptance-fixtures/python").copyRecursively(source))
        val manifestFile = File(source, "lingxi-app.json")
        val original = manifestFile.readText()
        val published = apps.request("publish") { put("sourcePath", source.path); put("expectedRevision", 0) } as JSONObject
        val record = published.getJSONObject("record").appRecord()
        val release = published.getJSONObject("release").appRelease()
        NativeAppActivityHost(Intent(context, MainActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)).use {
            try {
                apps.grant(record.id, release.digest)
                val serving = apps.start(record.id)
                val layout = apps.request("layout", record.id) as JSONObject
                val data = File(layout.getString("dataPath"), "failure-sentinel").apply { parentFile!!.mkdirs(); writeText("keep-transaction-data") }
                // Android SELinux denies app access to /dev/full. Inject the
                // ENOSPC error in the real candidate process rather than filling
                // the user's disk. This checks failed preparation and rollback,
                // not the physical device's free-space threshold. A closed
                // loopback endpoint models an unavailable download, not a
                // claim that this test toggles global connectivity.
                val unavailablePort = ServerSocket(0, 1, InetAddress.getByName("127.0.0.1")).use { it.localPort }
                val failures = listOf(
                    "python3 -c 'import errno; raise OSError(errno.ENOSPC, \"No space left on device\")'" to "No space left on device",
                    "python3 -c 'import urllib.request; urllib.request.urlopen(\"http://127.0.0.1:$unavailablePort/source.zip\", timeout=1)'" to "Connection refused",
                )
                var latest = record.latestRevision
                for ((command, expectedLog) in failures) {
                    manifestFile.writeText(JSONObject(original).put("build", JSONArray().put(JSONObject().put("command", command))).toString())
                    val candidate = apps.request("publish", record.id) { put("sourcePath", source.path); put("expectedRevision", latest) } as JSONObject
                    val next = candidate.getJSONObject("release").appRelease(); latest = next.revision
                    apps.grant(record.id, next.digest)
                    val startFailure = runCatching { apps.start(record.id, revision = next.revision) }.exceptionOrNull()
                    assertNotNull(startFailure)
                    val failed = apps.runs(record.id).single { run -> run.revision == next.revision && run.phase == "failed" }
                    val logged = withTimeoutOrNull(5000) { while (!apps.logs(record.id, failed.runId).contains(expectedLog)) delay(100); true } ?: false
                    assertTrue("Expected $expectedLog; start=$startFailure; logs=${apps.logs(record.id, failed.runId)}", logged)
                    assertEquals(record.currentRevision, apps.get(record.id).currentRevision)
                    assertEquals(serving.run.runId, (apps.request("status", record.id) as JSONObject).getJSONObject("run").getString("runId"))
                    assertEquals("PYTHON-READY", health(serving))
                    assertEquals("keep-transaction-data", data.readText())
                }
                // A schema without a compatible data/migration transaction must
                // never replace the still-serving code or rewrite user data.
                for ((compatibleFrom, expectedCode) in listOf(2 to "dataIncompatible", 1 to "dataMigrationRequired")) {
                    manifestFile.writeText(JSONObject(original).put("dataSchema", JSONObject().put("version", 2).put("compatibleFrom", compatibleFrom)).toString())
                    val schema = apps.request("publish", record.id) { put("sourcePath", source.path); put("expectedRevision", latest) } as JSONObject
                    val next = schema.getJSONObject("release").appRelease()
                    latest = next.revision
                    apps.grant(record.id, next.digest)
                    val schemaFailure = runCatching { apps.start(record.id, revision = next.revision) }.exceptionOrNull()
                    assertEquals(expectedCode, (schemaFailure as? AppServiceException)?.code)
                    assertEquals(record.currentRevision, apps.get(record.id).currentRevision)
                    assertEquals("PYTHON-READY", health(serving))
                    assertEquals("keep-transaction-data", data.readText())
                }
                android.util.Log.i("LocalAppAcceptance", "FAILURE_MATRIX PASS candidate-ENOSPC-injection unavailable-download incompatible-schema old-run old-revision user-data-retained")
            } finally { apps.remove(record.id); source.deleteRecursively() }
        }
    }

    @Test fun publicationCompareAndSwapAndOriginPortConflictHaveExplicitOutcomes() = runBlocking<Unit> {
        val context = InstrumentationRegistry.getInstrumentation().targetContext
        val apps = UnifiedLocalApps.get(context)
        val source = apps.hostFile("/home/lingxi/.lingxi/.apps/conflict-matrix-${UUID.randomUUID()}")
        check(File(context.filesDir, "local-app-acceptance-fixtures/static").copyRecursively(source))
        val published = apps.request("publish") { put("sourcePath", source.path); put("expectedRevision", 0) } as JSONObject
        val record = published.getJSONObject("record").appRecord()
        try {
            File(source, "dist/index.html").appendText("\n<!-- concurrent release -->")
            val outcomes = (1..2).map { async(Dispatchers.IO) {
                runCatching { apps.request("publish", record.id) { put("sourcePath", source.path); put("expectedRevision", record.latestRevision) } }
            } }.awaitAll()
            assertEquals(1, outcomes.count { it.isSuccess })
            assertEquals(1, outcomes.count { it.isFailure })
            val rejected = outcomes.single { it.isFailure }.exceptionOrNull()
            assertEquals("revisionConflict", (rejected as? AppServiceException)?.code)
            assertEquals(record.latestRevision + 1, apps.get(record.id).latestRevision)
            val blocker = ServerSocket(record.originPort, 1, InetAddress.getByName("127.0.0.1"))
            try {
                val failure = runCatching { apps.start(record.id) }.exceptionOrNull()
                assertEquals("originPortUnavailable", (failure as? AppServiceException)?.code)
                assertEquals(record.originPort, apps.get(record.id).originPort)
            } finally { blocker.close() }
            val retry = apps.start(record.id)
            assertEquals("http://127.0.0.1:${record.originPort}", retry.origin)
            android.util.Log.i("LocalAppAcceptance", "CONFLICT_MATRIX PASS concurrent-publication-one-winner explicit-port-conflict retry-same-origin")
        } finally { apps.remove(record.id); source.deleteRecursively() }
    }

    private suspend fun health(launch: AppLaunch): String = withContext(Dispatchers.IO) {
        val connection = URL(appServiceUrl(launch, "api", "/health", false)).openConnection() as HttpURLConnection
        connection.connectTimeout = 5000; connection.readTimeout = 5000
        connection.setRequestProperty("Origin", launch.origin)
        try { connection.inputStream.bufferedReader().use { it.readText() } }
        finally { connection.disconnect() }
    }
}
