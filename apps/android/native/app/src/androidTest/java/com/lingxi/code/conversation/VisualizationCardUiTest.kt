package com.lingxi.code.conversation

import android.view.View
import android.view.ViewGroup
import android.webkit.WebView
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.material3.Text
import androidx.compose.ui.Modifier
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.mutableStateOf
import androidx.compose.ui.test.assertIsDisplayed
import androidx.compose.ui.test.onNodeWithTag
import androidx.compose.ui.test.onAllNodesWithText
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.performClick
import com.lingxi.code.testing.createComponentComposeRule
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import androidx.test.runner.lifecycle.ActivityLifecycleMonitorRegistry
import androidx.test.runner.lifecycle.Stage
import com.lingxi.code.R
import com.lingxi.code.bindings.runtime.NoHandle
import com.lingxi.code.bindings.runtime.VisualizationHost
import com.lingxi.code.bindings.runtime.VisualizationMountDto
import com.lingxi.code.bindings.runtime.VisualizationResponseDto
import com.lingxi.code.bindings.runtime.VisualizationThemeDto
import com.lingxi.code.theme.LingXiTheme
import com.lingxi.code.components.UiTags
import com.lingxi.code.runtimeintegration.NativeEngineCallbacks
import com.lingxi.code.runtimeintegration.NativeEngineFactory
import com.lingxi.code.runtimeintegration.NativeEngineScenario
import java.io.File
import java.util.UUID
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicInteger
import kotlin.math.roundToInt
import org.junit.Assert.assertTrue
import org.junit.Assert.assertEquals
import org.junit.Assert.assertSame
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith

@RunWith(AndroidJUnit4::class)
class VisualizationCardUiTest {
    @get:Rule val rule = createComponentComposeRule()

    @Test fun focusedWidgetExpandsInLazyColumnWithoutDetachingItsInlineHost() {
        val host = ShellHost()
        val followups = mutableListOf<String>()
        val previewLimits = mutableListOf<Int>()
        rule.setContent {
            LingXiTheme(darkTheme = false) {
                CompositionLocalProvider(
                    LocalVisualizationContext provides VisualizationScreenContext(host, "session", false, {
                        if (it.text.startsWith("max-height:")) previewLimits += it.text.substringAfter(':').toInt()
                        else followups += it.text
                    }),
                ) {
                    LazyColumn(Modifier.fillMaxSize()) {
                        item { VisualizationCard(VisualizationSlotStatus.Ready, VisualizationRef("chart", 1u)) }
                        items(20) { Text("Transcript row $it") }
                    }
                }
            }
        }
        rule.waitUntil(10_000) { host.mounts.get() == 1 }
        val loading = InstrumentationRegistry.getInstrumentation().targetContext.getString(R.string.visualization_loading)
        rule.waitUntil(10_000) { rule.onAllNodesWithText(loading).fetchSemanticsNodes().isEmpty() }
        lateinit var webView: WebView
        lateinit var inlineContainer: ViewGroup
        var inlineHeight = 0
        rule.runOnIdle {
            val activity = ActivityLifecycleMonitorRegistry.getInstance().getActivitiesInStage(Stage.RESUMED).single()
            webView = requireNotNull(findWebView(activity.window.decorView))
            inlineContainer = webView.parent as ViewGroup
            assertTrue(inlineContainer.isAttachedToWindow)
            inlineHeight = inlineContainer.height
            assertTrue(inlineHeight > 0)
        }
        repeat(3) { cycle ->
            rule.runOnIdle {
                assertTrue(webView.requestFocus())
                assertTrue(webView.hasFocus())
            }
            rule.onNodeWithTag("conversation.visualization.expand").performClick()
            rule.onNodeWithTag("conversation.visualization.expanded").assertIsDisplayed()
            rule.waitUntil(10_000) { followups.size == cycle * 2 + 1 }
            rule.runOnIdle {
                // The old host must remain owned by the LazyColumn even while
                // the same focused WebView is displayed in another window.
                assertTrue(inlineContainer.isAttachedToWindow)
                assertEquals(0, inlineContainer.childCount)
                assertEquals(inlineHeight, inlineContainer.height)
                assertTrue(webView.isAttachedToWindow)
                assertTrue(webView.requestFocus())
                assertTrue(webView.hasFocus())
            }
            rule.onNodeWithTag("conversation.visualization.close").performClick()
            rule.onNodeWithTag("conversation.visualization.expand").assertIsDisplayed()
            rule.waitUntil(10_000) { followups.size == (cycle + 1) * 2 }
            rule.runOnIdle {
                assertSame(inlineContainer, webView.parent)
                assertTrue(inlineContainer.isAttachedToWindow)
                assertEquals(inlineHeight, inlineContainer.height)
                assertEquals(List(cycle + 1) { 160 }, previewLimits)
                assertEquals(1, host.mounts.get())
                assertEquals(0, host.releases.get())
            }
        }
        rule.runOnIdle {
            assertEquals((1..6).map { "expanded:$it" }, followups)
            // A collapsed preview still shrinks when the content gets shorter.
            webView.evaluateJavascript("emitResize(96, true)", null)
        }
        rule.waitUntil(10_000) { followups.lastOrNull() == "resized:96" }
        rule.runOnIdle {
            assertEquals((96 * webView.resources.displayMetrics.density).roundToInt(), inlineContainer.height)
            assertTrue(inlineContainer.height < inlineHeight)
        }
    }

    private fun findWebView(view: View): WebView? {
        if (view is WebView) return view
        if (view is ViewGroup) {
            for (index in 0 until view.childCount) {
                findWebView(view.getChildAt(index))?.let { return it }
            }
        }
        return null
    }

    @Test fun failedMountCanBeRetriedFromTheCard() {
        val host = ShellHost(failFirstMount = true)
        rule.setContent {
            LingXiTheme(darkTheme = false) {
                CompositionLocalProvider(
                    LocalVisualizationContext provides VisualizationScreenContext(host, "session", false, {}),
                ) {
                    VisualizationCard(VisualizationSlotStatus.Ready, VisualizationRef("chart", 1u))
                }
            }
        }
        rule.waitUntil(10_000) {
            rule.onAllNodesWithText(InstrumentationRegistry.getInstrumentation().targetContext.getString(R.string.common_retry))
                .fetchSemanticsNodes().isNotEmpty()
        }
        rule.onNodeWithTag("conversation.visualization.retry").performClick()
        val loading = InstrumentationRegistry.getInstrumentation().targetContext.getString(R.string.visualization_loading)
        rule.waitUntil(10_000) { host.mounts.get() == 2 && rule.onAllNodesWithText(loading).fetchSemanticsNodes().isEmpty() }
        rule.onNodeWithTag("conversation.visualization").assertIsDisplayed()
    }

    @Test fun skillInstructionsStayCollapsedUntilOpened() {
        val expanded = mutableStateOf(false)
        val body = "Publish a small, self-contained HTML widget with the Visualization tool."
        rule.setContent {
            LingXiTheme(darkTheme = false) {
                ToolCallView(
                    call = ToolCallUi(id = "skill", tool = "Skill", status = AgentToolStatus.Completed,
                        header = ToolHeaderUi(ToolVerbUi.Skill, "Skill", primary = "visualize", title = "Skill(visualize)"),
                        display = ToolResultDisplayUi(headline = "Launching skill: visualize", body = body, bodyLines = 100)),
                    expanded = expanded.value,
                    onToggleExpanded = { expanded.value = !expanded.value },
                )
            }
        }
        rule.onNodeWithText(body).assertDoesNotExist()
        rule.onNodeWithTag(UiTags.TOOL_CALL_TOGGLE).performClick()
        rule.onNodeWithText(body, substring = true).assertIsDisplayed()
    }

    @Test fun firstSessionWaitsForItsOwnerBeforeMounting() {
        val host = ShellHost()
        val sessionId = mutableStateOf("new")
        rule.setContent {
            LingXiTheme(darkTheme = false) {
                CompositionLocalProvider(
                    LocalVisualizationContext provides VisualizationScreenContext(host, sessionId.value, false, {}),
                ) {
                    VisualizationCard(VisualizationSlotStatus.Ready, VisualizationRef("chart", 1u))
                }
            }
        }
        rule.runOnIdle {
            assertEquals(0, host.mounts.get())
            sessionId.value = "session"
        }
        rule.waitUntil(10_000) { host.mounts.get() == 1 }
    }

    @Test fun storedWidgetRendersThroughTheRealNativeHost() {
        val context = InstrumentationRegistry.getInstrumentation().targetContext
        val sessionId = UUID.randomUUID().toString()
        val store = File(context.filesDir, ".lingxi/visualizations/$sessionId").apply { mkdirs() }
        val fragment = "<div style='height:240px;background:#20b874'>Native visualization</div>"
        File(store, "revisions/chart/1.html").apply { parentFile!!.mkdirs(); writeText(fragment) }
        File(store, "index.json").writeText(
            """{"schema":1,"totalBytes":${fragment.toByteArray().size},"items":{"chart":{"revisions":[{"rev":1,"title":"Native chart","bytes":${fragment.toByteArray().size},"createdAtMs":${System.currentTimeMillis()}}]}}}""",
        )
        val engine = requireNotNull(NativeEngineFactory.build(
            context = context,
            apiBase = "https://invalid.example",
            apiKey = "",
            model = "",
            providerProfilesJson = "{}",
            routingJson = """{"mobileEnabledProfiles":[]}""",
            visionDelegationEnabled = false,
            scenario = NativeEngineScenario.Interactive(NativeEngineCallbacks(
                onPermission = { error("No permission expected") },
            )),
        ))
        try {
            val host = requireNotNull(engine.visualizationHost(VISUALIZATION_ORIGIN))
            val visible = mutableStateOf(true)
            assertTrue(host.list(sessionId).isNotEmpty())
            rule.setContent {
                LingXiTheme(darkTheme = false) {
                    if (visible.value) {
                        CompositionLocalProvider(
                            LocalVisualizationContext provides VisualizationScreenContext(host, sessionId, false, {}),
                        ) {
                            VisualizationCard(VisualizationSlotStatus.Ready, VisualizationRef("chart", 1u))
                        }
                    }
                }
            }
            val loading = context.getString(R.string.visualization_loading)
            rule.waitUntil(15_000) { rule.onAllNodesWithText(loading).fetchSemanticsNodes().isEmpty() }
            rule.onNodeWithTag("conversation.visualization").assertIsDisplayed()
            assertTrue(rule.onAllNodesWithText(context.getString(R.string.visualization_unavailable)).fetchSemanticsNodes().isEmpty())
            // A ready bridge can precede the WebView's first presented frame.
            // Require actual fragment pixels, rather than accepting a blank
            // surface merely because its JS handshake completed.
            var rendered: android.graphics.Bitmap? = null
            rule.waitUntil(15_000) {
                val bitmap = InstrumentationRegistry.getInstrumentation().uiAutomation.takeScreenshot()
                    ?: return@waitUntil false
                var greenPixels = 0
                for (y in 0 until bitmap.height step 32) {
                    for (x in 0 until bitmap.width step 32) {
                        val pixel = bitmap.getPixel(x, y)
                        if (kotlin.math.abs(android.graphics.Color.red(pixel) - 32) <= 8 &&
                            kotlin.math.abs(android.graphics.Color.green(pixel) - 184) <= 8 &&
                            kotlin.math.abs(android.graphics.Color.blue(pixel) - 116) <= 8
                        ) greenPixels++
                    }
                }
                if (greenPixels > 50) {
                    rendered = bitmap
                    true
                } else {
                    bitmap.recycle()
                    false
                }
            }
            requireNotNull(rendered).let { bitmap ->
                File(context.cacheDir, "visualization-native-render.png").outputStream().use {
                    bitmap.compress(android.graphics.Bitmap.CompressFormat.PNG, 100, it)
                }
                bitmap.recycle()
            }
            // Dispose before the runtime and its temporary store are removed.
            rule.runOnIdle { visible.value = false }
            rule.waitForIdle()
        } finally {
            engine.destroy()
            store.deleteRecursively()
        }
    }

    @Test fun switchingEngineRecreatesTheWidgetForTheSameReference() {
        val first = ShellHost()
        val second = ShellHost()
        val host = mutableStateOf<VisualizationHost>(first)
        rule.setContent {
            LingXiTheme(darkTheme = false) {
                CompositionLocalProvider(
                    LocalVisualizationContext provides VisualizationScreenContext(host.value, "session", false, {}),
                ) {
                    VisualizationCard(VisualizationSlotStatus.Ready, VisualizationRef("chart", 1u))
                }
            }
        }
        rule.waitUntil(10_000) { first.mounts.get() == 1 }
        rule.runOnIdle { host.value = second }
        rule.waitUntil(10_000) { second.mounts.get() == 1 && first.releases.get() == 1 }
    }

    @Test fun leavingWhileMountIsInFlightReleasesItsLateTicket() {
        val gate = CountDownLatch(1)
        val host = ShellHost(gate)
        val visible = mutableStateOf(true)
        rule.setContent {
            LingXiTheme(darkTheme = false) {
                if (visible.value) {
                    CompositionLocalProvider(
                        LocalVisualizationContext provides VisualizationScreenContext(host, "session", false, {}),
                    ) {
                        VisualizationCard(VisualizationSlotStatus.Ready, VisualizationRef("chart", 1u))
                    }
                }
            }
        }
        try {
            rule.waitUntil(10_000) { host.mounts.get() == 1 }
            rule.runOnIdle { visible.value = false }
        } finally {
            gate.countDown()
        }
        rule.waitUntil(10_000) { host.releases.get() == 1 }
        assertTrue(host.releases.get() == host.mounts.get())
    }

    private class ShellHost(private val gate: CountDownLatch? = null, private val failFirstMount: Boolean = false) : VisualizationHost(NoHandle) {
        val mounts = AtomicInteger()
        val releases = AtomicInteger()

        override fun shellUrl(): String = "$VISUALIZATION_ORIGIN/shell.html"

        override fun mount(
            sessionId: String,
            id: String,
            revision: UInt,
            themeDto: VisualizationThemeDto,
            locale: String,
            expanded: Boolean,
        ): VisualizationMountDto {
            val attempt = mounts.incrementAndGet()
            if (failFirstMount && attempt == 1) error("Mount unavailable")
            gate?.await(10, TimeUnit.SECONDS)
            return VisualizationMountDto("ticket", 1uL, "$VISUALIZATION_ORIGIN/doc/test", "Chart")
        }

        override fun unmount(token: String) { releases.incrementAndGet() }

        override fun serve(path: String): VisualizationResponseDto {
            val html = """
                <!doctype html><script>
                const bridge = window.lingxiVisualization;
                let expansionCount = 0;
                let generation;
                function emitResize(height, announce = false) {
                  bridge.postMessage(JSON.stringify({type:'resize', generation, height, contentHeight:height, clamped:false}));
                  if (announce) bridge.postMessage(JSON.stringify({type:'followup.draft', generation, text:'resized:' + height}));
                }
                bridge.onmessage = event => {
                  const message = JSON.parse(event.data);
                  if (message.type === 'mount') {
                    generation = message.generation;
                    bridge.postMessage(JSON.stringify({type:'ready', generation:message.generation}));
                    emitResize(160);
                  } else if (message.type === 'max-height') {
                    // A fullscreen measurement may already be in flight when
                    // the native host requests the original preview height.
                    emitResize(1200);
                    bridge.postMessage(JSON.stringify({type:'followup.draft', generation, text:'max-height:' + message.maxHeight}));
                  } else if (message.type === 'expanded') {
                    // Cover fullscreen reports and the shell's stale replay
                    // immediately after expanded=false, before remeasurement.
                    emitResize(1200);
                    bridge.postMessage(JSON.stringify({type:'followup.draft', generation:message.generation, text:'expanded:' + (++expansionCount)}));
                  }
                };
                bridge.postMessage(JSON.stringify({type:'shell.ready'}));
                </script>
            """.trimIndent()
            return VisualizationResponseDto(200u.toUShort(), "text/html", emptyList(), html.toByteArray())
        }
    }
}
