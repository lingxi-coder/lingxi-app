package com.lingxi.code.conversation

import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.mutableStateOf
import androidx.compose.ui.test.assertIsDisplayed
import androidx.compose.ui.test.onNodeWithTag
import androidx.compose.ui.test.onAllNodesWithText
import androidx.compose.ui.test.junit4.createComposeRule
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import com.lingxi.code.R
import com.lingxi.code.bindings.runtime.NoHandle
import com.lingxi.code.bindings.runtime.VisualizationHost
import com.lingxi.code.bindings.runtime.VisualizationMountDto
import com.lingxi.code.bindings.runtime.VisualizationResponseDto
import com.lingxi.code.bindings.runtime.VisualizationThemeDto
import com.lingxi.code.theme.LingXiTheme
import com.lingxi.code.voice.buildVoiceEngine
import java.io.File
import java.util.UUID
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicInteger
import org.junit.Assert.assertTrue
import org.junit.Assert.assertEquals
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith

@RunWith(AndroidJUnit4::class)
class VisualizationCardUiTest {
    @get:Rule val rule = createComposeRule()

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
        val engine = requireNotNull(buildVoiceEngine(
            context = context,
            apiBase = "https://invalid.example",
            apiKey = "",
            model = "",
            providerProfilesJson = "{}",
            routingJson = """{"mobileEnabledProfiles":[]}""",
            visionDelegationEnabled = false,
            onPermission = { error("No permission expected") },
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

    private class ShellHost(private val gate: CountDownLatch? = null) : VisualizationHost(NoHandle) {
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
            mounts.incrementAndGet()
            gate?.await(10, TimeUnit.SECONDS)
            return VisualizationMountDto("ticket", 1uL, "$VISUALIZATION_ORIGIN/doc/test", "Chart")
        }

        override fun unmount(token: String) { releases.incrementAndGet() }

        override fun serve(path: String): VisualizationResponseDto {
            val html = """
                <!doctype html><script>
                const bridge = window.lingxiVisualization;
                bridge.onmessage = event => {
                  const message = JSON.parse(event.data);
                  if (message.type === 'mount') {
                    bridge.postMessage(JSON.stringify({type:'ready', generation:message.generation}));
                  }
                };
                bridge.postMessage(JSON.stringify({type:'shell.ready'}));
                </script>
            """.trimIndent()
            return VisualizationResponseDto(200u.toUShort(), "text/html", emptyList(), html.toByteArray())
        }
    }
}
