package com.lingxi.code.apps

import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import com.lingxi.code.BuildConfig
import kotlinx.coroutines.runBlocking
import org.json.JSONObject
import org.junit.Assert.*
import org.junit.Assume.assumeFalse
import org.junit.Test
import org.junit.runner.RunWith
import java.util.UUID

@RunWith(AndroidJUnit4::class)
class LocalAppDistributionChannelTest {
    @Test fun installedChannelDoesNotAdvertiseUnavailableLinuxCapabilities() {
        val target = appTarget("web")
        assertEquals(listOf(BuildConfig.FLAVOR), target.getJSONArray("channels").strings())
        assertEquals("android", target.getString("platform"))
        val capabilities = target.getJSONArray("capabilities").strings()
        assertEquals(BuildConfig.MOBILE_LINUX_FULL, "process" in capabilities)
        assertEquals(BuildConfig.MOBILE_LINUX_FULL, "background" in capabilities)
    }

    @Test fun playRejectsGrantedLinuxReleaseBeforeRuntimePreparation() = runBlocking<Unit> {
        assumeFalse("Requires the actual Play package", BuildConfig.MOBILE_LINUX_FULL)
        val context = InstrumentationRegistry.getInstrumentation().targetContext
        val apps = UnifiedLocalApps.get(context)
        val source = apps.hostFile("/home/lingxi/.lingxi/.apps/channel-${UUID.randomUUID()}")
        check(source.mkdirs())
        source.resolve("lingxi-app.json").writeText("""{"schema":2,"name":"Channel refusal fixture","entry":{"type":"server","process":"api"},"targets":[{"platform":"android","environment":"mobile-linux","channels":["direct"],"capabilities":["process"]}],"capabilities":["process"],"processes":[{"name":"api","command":"python3 server.py","ready":{"path":"/","timeoutMs":1000}}]}""")
        source.resolve("server.py").writeText("raise RuntimeError('restricted channel must never execute this')")
        var id: String? = null
        try {
            val published = apps.request("publish") { put("sourcePath", source.path); put("expectedRevision", 0) } as JSONObject
            val record = published.getJSONObject("record").appRecord()
            id = record.id
            apps.grant(record.id, published.getJSONObject("release").appRelease().digest)
            for (operation in listOf("start", "resume", "restart", "repair")) {
                val failure = runCatching { apps.start(record.id, operation) }.exceptionOrNull()
                assertNotNull("Play channel allowed $operation", failure)
                assertTrue("$operation must fail compatibility before binding Linux: $failure", failure is IllegalStateException && failure.message?.contains("不支持当前设备") == true)
            }
            assertTrue("Rejected channel must not create a preparation or run", apps.runs(record.id).isEmpty())
            android.util.Log.i("LocalAppAcceptance", "CHANNEL PASS actual Play package rejects granted Linux start/resume/restart/repair")
        } finally {
            id?.let { apps.remove(it) }
            source.deleteRecursively()
        }
    }
}
