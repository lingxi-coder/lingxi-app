package com.lingxi.code.apps

import android.content.Intent
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import com.lingxi.code.BuildConfig
import com.lingxi.code.MainActivity
import com.lingxi.code.settings.LinuxRuntimeBridge
import com.lingxi.code.settings.LinuxRuntimeMode
import com.lingxi.code.bindings.android.MobileLinuxRootfsStateFfi
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

/** Deliberately damages only verified rootfs files; never user HOME or app data.
 * Backups restore those exact files if repair fails. All user data is retained.
 */
@RunWith(AndroidJUnit4::class)
class UnifiedLocalAppRootfsRepairTest {
    @Test fun repairRestoresImmutableRootfsAndManifestWithoutChangingApplication() = runBlocking<Unit> {
        assumeTrue("Python Local Apps require Direct", BuildConfig.MOBILE_LINUX_FULL)
        val context = InstrumentationRegistry.getInstrumentation().targetContext
        val apps = UnifiedLocalApps.get(context)
        val mode = LinuxRuntimeMode.MobileLinux
        val directories = com.lingxi.code.project.MobileUserStorage.directories(context)
        val source = apps.hostFile(directories.appsGuestPath.trimEnd('/') + "/repair-acceptance-${UUID.randomUUID()}")
        check(File(context.filesDir, "local-app-acceptance-fixtures/python").copyRecursively(source))
        val sourceBytes = File(source, "server.py").readBytes()
        val value = apps.request("publish") { put("sourcePath", source.absolutePath); put("expectedRevision", 0) } as JSONObject
        val record = value.getJSONObject("record").appRecord()
        val release = value.getJSONObject("release").appRelease()
        apps.grant(record.id, release.digest)
        val managed = File(LinuxRuntimeBridge.managedRoot(context))
        val shell = File(managed, "active/bin/busybox")
        check(shell.isFile && !java.nio.file.Files.isSymbolicLink(shell.toPath()))
        val originalShell = shell.readBytes()
        val originalMode = android.system.Os.stat(shell.path).st_mode and 511
        val manifest = File(managed, "rootfs-manifest.json")
        val originalManifest = manifest.readBytes()
        val staged = File(managed, "staged/${LinuxRuntimeBridge.configForWorkspace(context, mode).rootfsVersion}")
        check(!staged.exists()) { "Use an activated rootfs without a pending staged repair" }
        val sentinel = apps.hostFile(directories.guestHome.trimEnd('/') + "/.local-app-repair-sentinel-${UUID.randomUUID()}")
        sentinel.writeText("persistent-home")
        NativeAppActivityHost(Intent(context, MainActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)).use {
            try {
                val first = apps.start(record.id)
                val layout = apps.request("layout", record.id) as JSONObject
                val dataFile = File(layout.getString("dataPath"), "acceptance.bin")
                dataFile.parentFile!!.mkdirs(); dataFile.writeText("preserve-app-data")
                apps.stop(record.id)
                // No staged copy exists. The old repair returned Corrupt forever.
                android.system.Os.chmod(shell.path, originalMode or 128)
                shell.writeText("broken immutable executable")
                assertEquals(MobileLinuxRootfsStateFfi.CORRUPT, LinuxRuntimeBridge.verify(context, mode).rootfsState)
                assertEquals(MobileLinuxRootfsStateFfi.READY, LinuxRuntimeBridge.repair(context, mode).rootfsState)
                assertArrayEquals(originalShell, shell.readBytes())
                assertFalse(staged.exists())
                assertEquals(MobileLinuxRootfsStateFfi.READY, LinuxRuntimeBridge.repair(context, mode).rootfsState)
                assertFalse("Healthy repair must not unpack another rootfs", staged.exists())
                // Shell still works, but status alone can hide a broken manifest.
                manifest.writeText("{broken-manifest")
                assertTrue(runCatching { LinuxRuntimeBridge.verify(context, mode) }.isFailure)
                assertEquals(MobileLinuxRootfsStateFfi.READY, LinuxRuntimeBridge.repair(context, mode).rootfsState)
                assertArrayEquals(originalManifest, manifest.readBytes())
                assertArrayEquals(sourceBytes, File(source, "server.py").readBytes())
                assertEquals("persistent-home", sentinel.readText())
                assertEquals("preserve-app-data", dataFile.readText())
                assertEquals(record.id, apps.get(record.id).id)
                assertEquals(record.currentRevision, apps.get(record.id).currentRevision)
                val resumed = apps.start(record.id)
                assertNotEquals(first.run.runId, resumed.run.runId)
                assertEquals(first.origin, resumed.origin)
                withContext(Dispatchers.IO) {
                    val request = URL(appServiceUrl(resumed, "api", "/health", false)).openConnection() as HttpURLConnection
                    request.connectTimeout = 5000; request.readTimeout = 5000; request.setRequestProperty("Origin", resumed.origin)
                    try { assertEquals("PYTHON-READY", request.inputStream.bufferedReader().use { it.readText() }) }
                    finally { request.disconnect() }
                }
                assertEquals("preserve-app-data", dataFile.readText())
                android.util.Log.i("LocalAppAcceptance", "ROOTFS_REPAIR PASS corrupt-immutable no-existing-stage healthy-no-restage broken-manifest persistent-HOME source data appId revision stable-origin real-Python")
            } finally {
                // Preserve original runtime bytes even if a regression fails.
                if (!shell.isFile || !shell.readBytes().contentEquals(originalShell)) {
                    shell.parentFile!!.mkdirs(); shell.writeBytes(originalShell)
                }
                android.system.Os.chmod(shell.path, originalMode)
                if (!manifest.isFile || !manifest.readBytes().contentEquals(originalManifest)) manifest.writeBytes(originalManifest)
                apps.remove(record.id)
                source.deleteRecursively(); sentinel.delete()
                // A manifest-only repair can leave this newly-created pinned cache.
                if (staged.exists()) check(staged.deleteRecursively())
            }
        }
    }
}
