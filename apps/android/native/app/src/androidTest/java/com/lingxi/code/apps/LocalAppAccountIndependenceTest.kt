package com.lingxi.code.apps

import android.content.Intent
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import com.google.firebase.auth.FirebaseAuth
import com.lingxi.code.MainActivity
import com.lingxi.code.BuildConfig
import com.lingxi.code.project.MobileUserStorage
import kotlinx.coroutines.*
import org.json.JSONObject
import org.junit.Assert.*
import org.junit.Assume.assumeTrue
import org.junit.Test
import org.junit.runner.RunWith
import java.io.File
import java.net.HttpURLConnection
import java.net.URL
import java.util.UUID

/** Explicit opt-in: actual sign-out ends only the independent test package's login. */
@RunWith(AndroidJUnit4::class)
class LocalAppAccountIndependenceTest {
    @Test fun actualFirebaseSignOutKeepsLocalHomeReleaseDataAndStaticRun() = runBlocking<Unit> {
        assumeTrue("Explicit test-account sign-out opt-in required", InstrumentationRegistry.getArguments().getString("allowAccountSignOut") == "true")
        val context = InstrumentationRegistry.getInstrumentation().targetContext
        check(BuildConfig.DEBUG && BuildConfig.FLAVOR == "play" && !BuildConfig.MOBILE_LINUX_FULL) { "Only PlayDebug may sign out" }
        val expectedPackage = InstrumentationRegistry.getArguments().getString("expectedAcceptancePackage")
        check(expectedPackage == "com.lingxi.code.debug" && context.packageName == expectedPackage && BuildConfig.APPLICATION_ID == expectedPackage) {
            "Explicit installed acceptance package confirmation required; production must never sign out"
        }
        val auth = FirebaseAuth.getInstance()
        checkNotNull(auth.currentUser) { "Actual signed-in test account required" }
        val apps = UnifiedLocalApps.get(context)
        val home = MobileUserStorage.directories(context).home
        val created = mutableListOf<String>()
        val sources = mutableListOf<File>()
        NativeAppActivityHost(Intent(context, MainActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)).use {
            try {
                val rows = listOf("static").map { name ->
                    val source = apps.hostFile("/home/lingxi/.lingxi/.apps/account-$name-${UUID.randomUUID()}")
                    sources.add(source)
                    check(File(context.filesDir, "local-app-acceptance-fixtures/$name").copyRecursively(source))
                    val value = apps.request("publish") { put("sourcePath", source.path); put("expectedRevision", 0) } as JSONObject
                    val record = value.getJSONObject("record").appRecord()
                    val release = value.getJSONObject("release").appRelease()
                    created.add(record.id)
                    if (release.executesCode) apps.grant(record.id, release.digest)
                    val launch = apps.start(record.id)
                    Triple(apps.get(record.id), release, launch)
                }
                val sentinels = rows.map { (record, _, _) ->
                    val layout = apps.request("layout", record.id) as JSONObject
                    File(layout.getString("dataPath"), "account-sentinel").apply { parentFile!!.mkdirs(); writeText("keep-local-data") }
                }
                suspend fun health() = withContext(Dispatchers.IO) { rows.map { (_, release, launch) ->
                    val path = if (release.executesCode) launch.origin + "/__run/${launch.run.runId}/__service/api/health" else launch.url
                    val connection = URL(path).openConnection() as HttpURLConnection
                    connection.connectTimeout = 5000; connection.readTimeout = 5000
                    try { connection.responseCode } finally { connection.disconnect() }
                } }
                val before = health(); assertTrue(before.all { it in 200..299 })
                // Same Firebase sign-out used by account UI, including real auth listeners.
                withContext(Dispatchers.Main.immediate) { auth.signOut() }
                withTimeout(5000) { while (auth.currentUser != null) delay(50) }
                delay(500); apps.refresh()
                assertEquals(home, MobileUserStorage.directories(context).home)
                rows.forEach { (record, release, launch) ->
                    assertEquals(record, apps.get(record.id))
                    assertEquals(release.digest, apps.release(record.id, release.revision).digest)
                    val current = (apps.request("status", record.id) as JSONObject).getJSONObject("run")
                    assertEquals(launch.run.runId, current.getString("runId")); assertEquals("running", current.getString("phase"))
                }
                assertEquals(before, health()); sentinels.forEach { assertEquals("keep-local-data", it.readText()) }
                assertNull(auth.currentUser)
                android.util.Log.i("LocalAppAcceptance", "ACCOUNT PASS actual Firebase sign-out preserves HOME IDs releases static run data; credential chooser UI not exercised")
            } finally { created.forEach { apps.remove(it) }; sources.forEach { it.deleteRecursively() } }
        }
    }
}
