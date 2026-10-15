package com.lingxi.code.apps

import android.content.res.Configuration
import android.webkit.WebView
import android.webkit.WebViewClient
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import java.util.UUID
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import kotlinx.coroutines.runBlocking
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith

@RunWith(AndroidJUnit4::class)
class ChatAppThemeTest {
    @Test fun webViewMediaQueryFollowsBothSystemAppearances() = runBlocking {
        val instrumentation = InstrumentationRegistry.getInstrumentation()
        val base = instrumentation.targetContext
        for (dark in listOf(false, true)) {
            val config = Configuration(base.resources.configuration).apply {
                uiMode = (uiMode and Configuration.UI_MODE_NIGHT_MASK.inv()) or
                    if (dark) Configuration.UI_MODE_NIGHT_YES else Configuration.UI_MODE_NIGHT_NO
            }
            val runtime = ChatAppRuntime(base.createConfigurationContext(config)) { null }
            val id = "themetest" + UUID.randomUUID().toString().replace("-", "").take(16)
            val view = runtime.open(id)
            try {
                val loaded = CountDownLatch(1)
                instrumentation.runOnMainSync {
                    view.stopLoading()
                    view.webViewClient = object : WebViewClient() {
                        override fun onPageFinished(view: WebView, url: String) { loaded.countDown() }
                    }
                    view.loadDataWithBaseURL(
                        "https://$id.lingxi-app.invalid/",
                        "<html><head><meta name='color-scheme' content='light dark'></head><body>Theme test</body></html>",
                        "text/html", "UTF-8", null,
                    )
                }
                assertTrue("WebView did not finish loading", loaded.await(15, TimeUnit.SECONDS))
                val evaluated = CountDownLatch(1)
                var result: String? = null
                instrumentation.runOnMainSync {
                    view.evaluateJavascript("window.matchMedia('(prefers-color-scheme: dark)').matches") {
                        result = it
                        evaluated.countDown()
                    }
                }
                assertTrue(evaluated.await(10, TimeUnit.SECONDS))
                assertEquals("System dark=$dark", dark.toString(), result)
            } finally {
                runtime.close(id)
            }
        }
    }
}
