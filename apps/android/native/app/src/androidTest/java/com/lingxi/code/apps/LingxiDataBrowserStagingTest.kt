package com.lingxi.code.apps

import android.content.pm.PackageManager
import android.os.SystemClock
import androidx.activity.ComponentActivity
import androidx.test.core.app.ActivityScenario
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import com.lingxi.code.testing.finishComposeFixture
import org.junit.Assert.assertTrue
import org.junit.Assume.assumeTrue
import org.junit.Test
import org.junit.runner.RunWith

@RunWith(AndroidJUnit4::class)
class LingxiDataBrowserStagingTest {
    @Test fun opensLocalAppInDefaultBrowser() {
        val args = InstrumentationRegistry.getArguments()
        val port = args.getString("lingxiBrowserPort")?.toIntOrNull()
        assumeTrue(port != null && port in 1024..65535)
        val url = "http://127.0.0.1:$port/launch"
        val instrumentation = InstrumentationRegistry.getInstrumentation()
        val context = instrumentation.targetContext
        val defaultBrowser = requireNotNull(context.packageManager.resolveActivity(
            LocalAppBrowser.intent(url), PackageManager.MATCH_DEFAULT_ONLY,
        )?.activityInfo?.packageName) { "Select a default browser for this device" }
        assertTrue("The system default must be a browser handler",
            defaultBrowser in LocalAppBrowser.installedHandlerPackages(context, url))

        ActivityScenario.launch(ComponentActivity::class.java).use { scenario ->
            lateinit var fixture: ComponentActivity
            scenario.onActivity { activity ->
                fixture = activity
                LocalAppBrowser.open(activity, url)
            }
            try {
                val automation = instrumentation.uiAutomation
                val deadline = SystemClock.uptimeMillis() + 15_000
                while (SystemClock.uptimeMillis() < deadline) {
                    if (automation.rootInActiveWindow?.packageName?.toString() == defaultBrowser) return
                    SystemClock.sleep(200)
                }
                error("The Android default browser did not become visible")
            } finally {
                finishComposeFixture(fixture)
            }
        }
    }
}
