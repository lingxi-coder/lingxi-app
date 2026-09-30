package com.lingxi.code.localapps

import android.net.Uri
import android.view.ViewGroup
import android.webkit.WebView
import android.webkit.WebViewClient
import androidx.activity.ComponentActivity
import androidx.test.core.app.ActivityScenario
import androidx.test.ext.junit.runners.AndroidJUnit4
import java.net.InetAddress
import java.net.ServerSocket
import java.net.SocketException
import java.net.SocketTimeoutException
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicInteger
import kotlin.concurrent.thread
import org.json.JSONObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith

@RunWith(AndroidJUnit4::class)
class LocalAppWebViewSpaHistoryTest {

    @Test
    fun controllerCommitsInteractiveSpaRoutesBackAndFreshCaptureWithoutPageStarts() {
        withController(
            """
            <!doctype html>
            <button id="route">Route</button>
            <script>
              document.getElementById('route').onclick = function() {
                history.pushState({}, '', '/first?tab=all&lingxi_runtime=42');
              };
            </script>
            """.trimIndent(),
        ) { scenario, host ->
            val initialPageStarts = host.pageStarts.get()
            val first = host.runtime.replace("/?", "/first?tab=all&")
            val second = host.runtime.replace("/?", "/second?tab=recent&")

            val click = executeQa(
                scenario,
                host,
                LocalAppUiAutomationAction.Click(LocalAppUiTarget(elementId = "route")),
            )
            assertQaOperationSuccess(click)
            assertEquals(first, loadedRuntimeUrl(click))
            assertEquals(initialPageStarts, host.pageStarts.get())

            evaluateJavascript(
                scenario,
                host.webView,
                "history.pushState({}, '', '/second?tab=recent&lingxi_runtime=42')",
            )
            val inspect = executeQa(scenario, host, LocalAppUiAutomationAction.Inspect)
            assertQaInspectionSuccess(inspect)
            assertEquals(second, loadedRuntimeUrl(inspect))
            assertEquals(initialPageStarts, host.pageStarts.get())

            val back = executeQa(scenario, host, LocalAppUiAutomationAction.Back)
            assertQaOperationSuccess(back)
            assertEquals(first, loadedRuntimeUrl(back))
            assertEquals(initialPageStarts, host.pageStarts.get())

            val capture = executeQa(scenario, host, LocalAppUiAutomationAction.CaptureView())
            assertQaOperationSuccess(capture)
            assertEquals(first, loadedRuntimeUrl(capture))
            assertEquals(
                "capture_view",
                JSONObject(capture.resultJson.orEmpty()).getJSONObject("result").getString("action"),
            )
        }
    }

    @Test
    fun controllerRetainsStartupReplaceStateUntilLoadFinishes() {
        withController(
            """
            <!doctype html>
            <script>
              history.replaceState({}, '', '/startup?lingxi_runtime=42');
            </script>
            <p>ready</p>
            """.trimIndent(),
        ) { scenario, host ->
            val expected = host.runtime.replace("/?", "/startup?")
            val inspect = executeQa(scenario, host, LocalAppUiAutomationAction.Inspect)

            assertQaInspectionSuccess(inspect)
            assertEquals(expected, loadedRuntimeUrl(inspect))
            assertEquals(1, host.pageStarts.get())
        }
    }

    @Test
    fun controllerRejectsInspectAfterInvalidStartupHistoryMarker() {
        withController(
            """
            <!doctype html>
            <script>
              history.replaceState({}, '', '/startup?lingxi_runtime=41');
            </script>
            <p>untrusted</p>
            """.trimIndent(),
        ) { scenario, host ->
            val inspect = executeQa(scenario, host, LocalAppUiAutomationAction.Inspect)

            assertNull(inspect.resultJson)
            assertEquals("Local App QA document identity did not become ready", inspect.error)
            assertEquals(1, host.pageStarts.get())
        }
    }

    @Test
    fun controllerRejectsCaptureWhenHistoryChangesDuringItsFreshFrameFence() {
        withController("<!doctype html><p>ready</p>") { scenario, host ->
            val baseline = executeQa(scenario, host, LocalAppUiAutomationAction.CaptureView())
            assertQaOperationSuccess(baseline)

            val duringCapture = host.runtime.replace("/?", "/during-capture?")
            val resultLatch = CountDownLatch(1)
            var result: LocalAppUiExecutionResult? = null

            scenario.onActivity {
                host.controller.execute(
                    LocalAppUiAutomationAction.Qa(
                        expectedRuntimeUrl = host.runtime,
                        action = LocalAppUiAutomationAction.CaptureView(),
                    ),
                ) {
                    result = it
                    resultLatch.countDown()
                }
                host.webView.evaluateJavascript(
                    "history.pushState({}, '', '/during-capture?lingxi_runtime=42')",
                    null,
                )
                // Exercise the production callback path deterministically;
                // the real callback generated above may arrive before or after it.
                host.controller.onVisitedHistoryUpdated(duringCapture, false)
            }

            assertTrue("timed out waiting for capture rejection", resultLatch.await(10, TimeUnit.SECONDS))
            assertNotNull(result)
            assertNull(result?.resultJson)
            assertNotNull(result?.error)

            val inspect = executeQa(scenario, host, LocalAppUiAutomationAction.Inspect)
            assertQaInspectionSuccess(inspect)
            assertEquals(duringCapture, loadedRuntimeUrl(inspect))
        }
    }

    private fun withController(
        html: String,
        assertion: (ActivityScenario<ComponentActivity>, ControllerHost) -> Unit,
    ) {
        TestPageServer(html).use { server ->
            val runtime = "${server.origin}/?lingxi_runtime=42"
            ActivityScenario.launch(ComponentActivity::class.java).use { scenario ->
                val loaded = CountDownLatch(1)
                lateinit var host: ControllerHost
                scenario.onActivity { activity ->
                    val pageStarts = AtomicInteger()
                    val webView = WebView(activity).apply {
                        settings.javaScriptEnabled = true
                        settings.domStorageEnabled = true
                    }
                    lateinit var controller: LocalAppWebViewController
                    val client = object : WebViewClient() {
                        override fun onPageStarted(
                            view: WebView,
                            url: String,
                            favicon: android.graphics.Bitmap?,
                        ) {
                            pageStarts.incrementAndGet()
                            controller.onPageStarted(url)
                        }

                        override fun onPageFinished(view: WebView, url: String) {
                            controller.onPageFinished(url)
                            loaded.countDown()
                        }

                        override fun doUpdateVisitedHistory(
                            view: WebView,
                            url: String,
                            isReload: Boolean,
                        ) {
                            controller.onVisitedHistoryUpdated(url, isReload)
                        }
                    }
                    val broker = LocalAppBridgeBroker(
                        appId = "instrumented",
                        trustedOrigin = Uri.parse(server.origin),
                        webView = webView,
                        onMessage = {},
                    )
                    controller = LocalAppWebViewController(webView, broker, client, runtime)
                    webView.webViewClient = client
                    activity.setContentView(
                        webView,
                        ViewGroup.LayoutParams(
                            ViewGroup.LayoutParams.MATCH_PARENT,
                            ViewGroup.LayoutParams.MATCH_PARENT,
                        ),
                    )
                    controller.beginNavigation(runtime)
                    webView.loadUrl(runtime)
                    host = ControllerHost(webView, controller, runtime, pageStarts)
                }

                try {
                    assertTrue("timed out waiting for initial page load", loaded.await(10, TimeUnit.SECONDS))
                    assertion(scenario, host)
                } finally {
                    scenario.onActivity {
                        host.controller.detach()
                        (host.webView.parent as? ViewGroup)?.removeView(host.webView)
                        host.webView.stopLoading()
                        host.webView.destroy()
                    }
                }
            }
        }
    }

    private fun executeQa(
        scenario: ActivityScenario<ComponentActivity>,
        host: ControllerHost,
        action: LocalAppUiAutomationAction,
    ): LocalAppUiExecutionResult {
        val latch = CountDownLatch(1)
        var result: LocalAppUiExecutionResult? = null
        scenario.onActivity {
            host.controller.execute(LocalAppUiAutomationAction.Qa(host.runtime, action)) {
                result = it
                latch.countDown()
            }
        }
        assertTrue("timed out waiting for QA action $action", latch.await(10, TimeUnit.SECONDS))
        return requireNotNull(result)
    }

    private fun evaluateJavascript(
        scenario: ActivityScenario<ComponentActivity>,
        webView: WebView,
        script: String,
    ) {
        val latch = CountDownLatch(1)
        scenario.onActivity {
            webView.evaluateJavascript(script) {
                latch.countDown()
            }
        }
        assertTrue("timed out evaluating JavaScript", latch.await(10, TimeUnit.SECONDS))
    }

    private fun loadedRuntimeUrl(result: LocalAppUiExecutionResult): String =
        JSONObject(result.resultJson.orEmpty()).getJSONObject("lingxi_qa").getString("loaded_runtime_url")

    private fun assertQaOperationSuccess(result: LocalAppUiExecutionResult) {
        assertNull(result.error)
        assertTrue(JSONObject(result.resultJson.orEmpty()).getJSONObject("result").getBoolean("ok"))
    }

    private fun assertQaInspectionSuccess(result: LocalAppUiExecutionResult) {
        assertNull(result.error)
        assertTrue(JSONObject(result.resultJson.orEmpty()).getJSONObject("result").has("elements"))
    }

    private data class ControllerHost(
        val webView: WebView,
        val controller: LocalAppWebViewController,
        val runtime: String,
        val pageStarts: AtomicInteger,
    )

    private class TestPageServer(private val html: String) : AutoCloseable {
        private val server = ServerSocket(0, 8, InetAddress.getByName("127.0.0.1")).apply {
            soTimeout = 250
        }
        val origin: String = "http://127.0.0.1:${server.localPort}"
        private val worker = thread(name = "local-app-webview-test-server") {
            val body = html.toByteArray(Charsets.UTF_8)
            while (!server.isClosed) {
                try {
                    server.accept().use { socket ->
                        socket.soTimeout = 1_000
                        val reader = socket.getInputStream().bufferedReader()
                        while (!reader.readLine().isNullOrEmpty()) Unit
                        socket.getOutputStream().buffered().use { output ->
                            output.write(
                                (
                                    "HTTP/1.1 200 OK\r\n" +
                                        "Content-Type: text/html; charset=utf-8\r\n" +
                                        "Content-Length: ${body.size}\r\n" +
                                        "Connection: close\r\n\r\n"
                                    ).toByteArray(Charsets.US_ASCII),
                            )
                            output.write(body)
                        }
                    }
                } catch (_: SocketTimeoutException) {
                    // Re-check close state.
                } catch (error: SocketException) {
                    if (!server.isClosed) throw error
                }
            }
        }

        override fun close() {
            server.close()
            worker.join(2_000)
        }
    }
}
