package com.lingxi.code

import android.util.Log
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import com.lingxi.code.bindings.ClientCommand
import com.lingxi.code.bindings.ClientEvent
import com.lingxi.code.bindings.MobileEngineHandle
import com.lingxi.code.voice.buildVoiceEngine
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withTimeout
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith

/**
 * Instrumented (androidTest) e2e for the FULL in-process path the Compose chat
 * rides: Kotlin caller → UniFFI (`buildAndroidEngine` + `MobileEngineHandle.submit`)
 * → in-process engine (its handle-owned tokio runtime) → adapter → the Kotlin
 * `AndroidEventListener` callback. This is the Android parity of the iOS
 * `EngineRoundtripTests.swift`: ONE protocol (`client-protocol`), ONE transport
 * (UniFFI), ONE listener.
 *
 * WHY instrumented (androidTest) and NOT a JVM unit test: OFF a device,
 * `buildAndroidEngine(...)` returns `PlatformUnavailable` (the `AndroidPlatform`
 * is only linked under `cfg(target_os = "android")`), so the REAL engine only
 * builds on a device/emulator. The headless CI gate (`assembleDebug` +
 * `testDebugUnitTest`) therefore cannot exercise this path; it lives here and
 * REQUIRES a connected device or emulator
 * (`./gradlew :app:connectedDebugAndroidTest`), exactly like
 * [AppFlowUiTest].
 *
 * It runs — and MUST pass — KEYLESS. With no `ANTHROPIC_API_KEY`:
 *
 *   * `buildVoiceEngine(...)` (which calls `buildAndroidEngine(...)` with the
 *     seven device-capability adapters + permission sink) still succeeds — the
 *     engine host is constructed and the foreign listener is registered (the
 *     "handshake"). Construction does NOT touch the network and does NOT require
 *     a key.
 *   * `submit(ClientCommand.SendPrompt(...))` returns once the turn is queued —
 *     per the binding contract a turn failure is NOT thrown from `submit`; it is
 *     spawned on the engine's runtime and streams to the listener.
 *   * the keyless turn uses the production empty-provider allowlist and resolves
 *     locally to a TERMINAL `ClientEvent`, without depending on external network
 *     timing. We assert a terminal arrived, not its concrete terminal type.
 *
 * Hermetic: the engine roots its filesystem under the instrumentation TARGET
 * context's private `filesDir` (the app under test, on the device sandbox); no
 * secrets are set or read by the test.
 */
@RunWith(AndroidJUnit4::class)
class EngineRoundtripTest {

    private val tag = "EngineRoundtripTest"

    /**
     * Build the real engine over the seven device-capability adapters + permission
     * sink via the same `buildVoiceEngine` the production chat source uses (which
     * calls the generated `buildAndroidEngine(...)`), routing every inbound
     * `ClientEvent` to [onEvent]. On a device this returns a live handle; OFF a
     * device it returns null (`PlatformUnavailable`) — the caller asserts non-null,
     * so a host run fails loudly rather than silently passing.
     *
     * Hermetic: `buildVoiceEngine` roots the engine's filesystem under the
     * instrumentation TARGET context's private `filesDir` (the app under test, on
     * the device sandbox) — no shared global state, cleaned with the app data.
     *
     * KEYLESS: we read `ANTHROPIC_API_KEY` from the env (empty in CI) and never
     * hardcode a secret. `buildVoiceEngine` builds regardless of key, mirroring
     * the iOS test's keyless `buildIosEngine`.
     */
    private fun buildEngineKeyless(
        model: String,
        routingJson: String? = null,
        onEvent: suspend (ClientEvent) -> Unit,
    ): MobileEngineHandle? {
        val targetContext = InstrumentationRegistry.getInstrumentation().targetContext
        return buildVoiceEngine(
            context = targetContext,
            apiBase = System.getenv("ANTHROPIC_BASE_URL") ?: "https://api.anthropic.com",
            apiKey = System.getenv("ANTHROPIC_API_KEY") ?: "",
            model = model,
            routingJson = routingJson,
            onEvent = onEvent,
        )
    }

    private fun ClientEvent.isTerminal(): Boolean = when (this) {
        is ClientEvent.Error, is ClientEvent.TurnEnded -> true
        else -> false
    }

    // --- Test A: keyless SendPrompt drives the listener to a terminal event ---

    /**
     * KEYLESS end-to-end: build the engine, submit a prompt, and assert a TERMINAL
     * `ClientEvent` (`Error` or `TurnEnded`) is delivered to the listener over the
     * UniFFI callback within a generous timeout.
     *
     * This single test exercises every hop of the Compose→UniFFI→engine→listener
     * path without network access or a secret. With no enabled provider, the turn
     * is expected to surface a terminal `Error`; we also accept a clean
     * `TurnEnded` and assert only that a terminal arrived.
     */
    @Test
    fun keylessSendPromptDeliversTerminalEvent() {
        // The terminal-event signal: a rendezvous channel the listener offers the
        // first observed terminal event onto. Rust delivers `onEvent` from the
        // engine runtime; the channel is the thread-safe handoff to the test
        // coroutine. CONFLATED so a late second terminal never suspends.
        val terminal = Channel<ClientEvent>(Channel.CONFLATED)
        val received = java.util.Collections.synchronizedList(mutableListOf<ClientEvent>())

        // HANDSHAKE / engine-build: must succeed even with NO api key.
        val handle = buildEngineKeyless(
            model = "claude-sonnet-4-20250514",
            // A fresh production install has no enabled provider rows. Exercise
            // that fail-closed state explicitly so this handshake test is
            // hermetic and never waits on an unauthenticated network request.
            routingJson = """{"mobileEnabledProfiles":[]}""",
            onEvent = { event ->
                received.add(event)
                if (event.isTerminal()) terminal.trySend(event)
            },
        )
        assertNotNull(
            "buildVoiceEngine must return a live handle on-device (keyless handshake); " +
                "null means PlatformUnavailable — run on a device/emulator, not the JVM host",
            handle,
        )
        val engine = handle!!

        val terminalEvent = runBlocking {
            // SUBMIT: per the binding contract this returns once the turn is
            // queued; a turn failure streams to the listener, it is NOT thrown.
            engine.submit(
                ClientCommand.SendPrompt(
                    text = "Reply with exactly: hello from lingxi",
                    promptMode = null,
                    images = emptyList(),
                    turnId = null,
                ),
            )
            // Wait for the streamed terminal event to reach the listener over the
            // UniFFI callback. The empty provider catalog must fail locally.
            withTimeout(60_000) { terminal.receive() }
        }

        // PROOF: at least one real engine-originated event arrived on the listener,
        // and the keyless turn surfaced a TERMINAL event.
        Log.d(
            tag,
            "keyless ClientEvents received via UniFFI listener: " +
                synchronized(received) { received.joinToString(" | ") { it::class.simpleName ?: "?" } },
        )
        assertFalse(
            "the listener must receive at least one ClientEvent over UniFFI",
            synchronized(received) { received.isEmpty() },
        )
        assertTrue(
            "keyless turn must deliver a TERMINAL ClientEvent (Error or TurnEnded) to the " +
                "listener — proves Compose→UniFFI→engine→listener end to end. Got: " +
                "${terminalEvent::class.simpleName}",
            terminalEvent.isTerminal(),
        )
        // If it terminated with an Error (the expected keyless outcome), the error
        // must carry a human-readable, engine-originated message. We do NOT assert
        // the key value or print any secret.
        (terminalEvent as? ClientEvent.Error)?.let {
            assertFalse("the terminal error must carry a message", it.message.isEmpty())
        }
    }

    // --- Test B (mirror iOS): ListModels → ModelList over the listener --------

    /**
     * KEYLESS, mirror of the iOS model-event test: submit `ClientCommand.ListModels`
     * (an OUT-OF-BAND command, not a text turn) and assert a `ModelList` event
     * arrives on the same UniFFI listener. The engine's `list_available_models`
     * returns a real fallback catalog keyless, so this is hermetic.
     */
    @Test
    fun keylessListModelsDeliversModelList() {
        val modelListCh = Channel<ClientEvent.ModelList>(Channel.CONFLATED)

        // SHIP-BLOCKER #2 mirror: build with an EMPTY model id — the engine must
        // start on `MobileConfig.default_model` (a real Anthropic wire id), never a
        // branded mock id. `build_android_engine` only overrides default_model when
        // the passed id is non-empty, so "" exercises exactly that path.
        val handle = buildEngineKeyless(
            model = "",
            onEvent = { event ->
                if (event is ClientEvent.ModelList) modelListCh.trySend(event)
            },
        )
        assertNotNull(
            "buildVoiceEngine must return a live handle on-device (keyless); null means " +
                "PlatformUnavailable — run on a device/emulator, not the JVM host",
            handle,
        )
        val engine = handle!!

        val modelList = runBlocking {
            // OUT-OF-BAND: ask for the catalog (not a text turn).
            engine.submit(ClientCommand.ListModels)
            withTimeout(30_000) { modelListCh.receive() }
        }

        assertFalse(
            "ModelList must carry real model ids",
            modelList.models.isEmpty(),
        )
        // The reported active model (engine default) and the catalog must NOT be the
        // branded mock ids the apps used to hardcode (lx-72b, …).
        val mockIds = setOf("lx-72b", "lx-72b-r", "lx-32b", "lx-code")
        assertFalse(
            "active model must be a REAL engine id, not a branded mock id: ${modelList.current}",
            mockIds.contains(modelList.current),
        )
        for (id in modelList.models) {
            assertFalse(
                "catalog must contain only REAL engine ids, found mock id: $id",
                mockIds.contains(id),
            )
        }
        Log.d(
            tag,
            "keyless ModelList via UniFFI listener: current=${modelList.current} " +
                "models=${modelList.models}",
        )
    }
}
