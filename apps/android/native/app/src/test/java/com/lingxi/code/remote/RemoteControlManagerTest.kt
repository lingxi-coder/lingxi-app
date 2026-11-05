package com.lingxi.code.remote

import com.lingxi.code.bindings.android.RemoteFileFfi
import com.lingxi.code.bindings.android.RemoteFfiException
import com.lingxi.code.bindings.android.AndroidRemoteBotsListener
import com.lingxi.code.bindings.android.AndroidRemoteListener
import com.lingxi.code.bindings.android.RemoteBotStatusFfi
import com.lingxi.code.bindings.android.RemoteBotSummaryFfi
import com.lingxi.code.bindings.android.RemoteBotThreadsStateFfi
import com.lingxi.code.bindings.android.RemoteBotsStateFfi
import com.lingxi.code.bindings.android.RemoteConnectionPhaseFfi
import com.lingxi.code.bindings.android.RemoteConnectionStateFfi
import com.lingxi.code.bindings.android.RemoteHostFfi
import com.lingxi.code.bindings.android.RemoteLocalAppOpenFfi
import com.lingxi.code.bindings.android.RemoteLocalAppFfi
import com.lingxi.code.bindings.android.RemoteImageFfi
import com.lingxi.code.bindings.android.RemoteModelsFfi
import com.lingxi.code.bindings.android.RemotePermissionModeFfi
import com.lingxi.code.bindings.android.RemotePromptImageFfi
import com.lingxi.code.bindings.android.RemoteSessionErrorFfi
import com.lingxi.code.bindings.android.RemoteSessionStatusFfi
import com.lingxi.code.bindings.android.RemoteSessionSummaryFfi
import com.lingxi.code.bindings.android.RemoteSessionUpdateFfi
import com.lingxi.code.bindings.android.RemoteTransportFfi
import com.lingxi.code.bindings.client.ReasoningSelectionDto
import com.lingxi.code.conversation.remoteUpdate
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.flowOf
import kotlinx.coroutines.launch
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import kotlinx.coroutines.withTimeout
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import kotlinx.coroutines.test.TestScope
import kotlinx.coroutines.test.UnconfinedTestDispatcher
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.advanceTimeBy
import kotlinx.coroutines.test.runTest
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

@OptIn(ExperimentalCoroutinesApi::class)
class RemoteControlManagerTest {
    private class FakeAccount(initial: String?) : RemoteAccount {
        val mutableUid = MutableStateFlow(initial)
        override val uid: StateFlow<String?> = mutableUid
        override suspend fun idToken(forceRefresh: Boolean) = "token"
    }

    private class FakeClient(val uid: String, val listener: AndroidRemoteListener) : RemoteClientPort {
        override suspend fun prepareFile(sessionId: String, path: String, name: String): RemoteFileFfi = error("file API not exercised by manager lifecycle test")
        override suspend fun uploadFile(file: RemoteFileFfi, path: String): RemoteFileFfi = error("file API not exercised by manager lifecycle test")
        override suspend fun downloadFile(file: RemoteFileFfi, path: String): RemoteFileFfi = error("file API not exercised by manager lifecycle test")
        override suspend fun listFiles(sessionId: String): List<RemoteFileFfi> = error("file API not exercised by manager lifecycle test")
        override fun fileProgress(transferId: String): ULong = error("file API not exercised by manager lifecycle test")
        override fun cancelFile(transferId: String) = error("file API not exercised by manager lifecycle test")
        override suspend fun sendPromptWithFiles(sessionId: String, text: String, images: List<com.lingxi.code.bindings.android.RemotePromptImageFfi>, fileRefs: List<String>) = error("file API not exercised by manager lifecycle test")
        val calls = mutableListOf<String>()
        var closed = false
        var pushId: String? = "unset"
        @Volatile var state: RemoteConnectionStateFfi? = null
        var onStateRead: (() -> Unit)? = null
        var stopWatchingGate: CompletableDeferred<Unit>? = null
        var forgetGate: CompletableDeferred<Unit>? = null
        var sessionWriteGate: CompletableDeferred<Unit>? = null
        val sessionWriteHosts = mutableListOf<String?>()
        var listGate: CompletableDeferred<List<RemoteLocalAppFfi>>? = null
        override suspend fun listApps() = checkNotNull(listGate).await()
        var openGate: CompletableDeferred<RemoteLocalAppOpenFfi>? = null
        override suspend fun openApp(appId: String) = checkNotNull(openGate).await()
        override fun setPushInstallationId(id: String?) { pushId = id }
        override suspend fun watchHosts() { calls += "watchHosts" }
        override suspend fun stopWatchingHosts() { calls += "stopWatchingHosts"; stopWatchingGate?.await() }
        override suspend fun connect(hostId: String) {
            calls += "connect:$hostId"
            state = RemoteConnectionStateFfi(hostId, RemoteConnectionPhaseFfi.READY, null, null, null, 0u, null, null, false)
        }
        override suspend fun disconnect() { calls += "disconnect"; state = null }
        override suspend fun retry() { calls += "retry" }
        override suspend fun nudge() { calls += "nudge" }
        override fun connectionState(): RemoteConnectionStateFfi? = state.also { onStateRead?.invoke() }
        override fun sessions(): List<RemoteSessionSummaryFfi>? = null
        var forgetFailure: Exception? = null
        override suspend fun forgetHost(hostId: String) { calls += "forget:$hostId"; forgetGate?.await(); forgetFailure?.let { throw it } }
        override suspend fun repairHost(hostId: String) { calls += "repair:$hostId" }
        var attachFailure: Exception? = null
        override suspend fun attachSession(sessionId: String) { calls += "attach:$sessionId"; attachFailure?.let { throw it } }
        override suspend fun detachSession(sessionId: String) { calls += "detach:$sessionId" }
        override suspend fun setSessionForeground(sessionId: String, foreground: Boolean) { calls += "foreground:$sessionId:$foreground" }
        override suspend fun loadOlderMessages(sessionId: String) { calls += "older:$sessionId" }
        override suspend fun fetchImage(sessionId: String, imageRef: String) = RemoteImageFfi("image/png", byteArrayOf())
        override suspend fun sendPrompt(sessionId: String, text: String, images: List<RemotePromptImageFfi>) {
            sessionWriteGate?.await()
            sessionWriteHosts += state?.hostId
            calls += "send:$sessionId:$text"
        }
        override suspend fun retryPrompt(sessionId: String, promptId: String) { calls += "retry:$sessionId:$promptId" }
        override suspend fun dismissPrompt(sessionId: String, promptId: String) { calls += "dismiss:$sessionId:$promptId" }
        override suspend fun cancelTurn(sessionId: String) { calls += "cancel:$sessionId" }
        override suspend fun respondPermission(sessionId: String, requestId: ULong, allow: Boolean) {
            sessionWriteGate?.await()
            sessionWriteHosts += state?.hostId
            calls += "permission:$sessionId:$requestId:$allow"
        }
        override suspend fun answerQuestion(sessionId: String, requestId: ULong, answers: Map<String, String>) {}
        override suspend fun cancelQuestion(sessionId: String, requestId: ULong) {}
        override suspend fun setModel(sessionId: String, model: String) {}
        override suspend fun setReasoning(sessionId: String, selection: ReasoningSelectionDto) {}
        override suspend fun setPermissionMode(sessionId: String, mode: RemotePermissionModeFfi) {}
        override suspend fun listModels() = RemoteModelsFfi(emptyList(), "", emptyList())

        var boundBotsListener: AndroidRemoteBotsListener? = null
        var activateFailure: Exception? = null
        var createFailure: Exception? = null
        var createGate: CompletableDeferred<String>? = null
        /** Session id to the id of the Bot it is a chat of. */
        val botOf = mutableMapOf<String, String>()
        /** (session id, turn id) to the name of the Bot that handed that prompt over. */
        val botNames = mutableMapOf<Pair<String, ULong>, String>()
        override fun setBotsListener(listener: AndroidRemoteBotsListener?) { boundBotsListener = listener }
        override suspend fun activateBotThreads(botId: String) { calls += "activateBot:$botId"; activateFailure?.let { throw it } }
        override fun deactivateBotThreads(botId: String) { calls += "deactivateBot:$botId" }
        override suspend fun createBotThread(botId: String): String {
            calls += "createBot:$botId"
            createFailure?.let { throw it }
            val id = createGate?.await() ?: "bot-chat"
            calls += "createdBot:$botId:$id"
            return id
        }
        override fun botOfThread(sessionId: String): String? = botOf[sessionId]
        override fun promptOriginBotName(sessionId: String, turnId: ULong): String? = botNames[sessionId to turnId]

        override suspend fun close() { closed = true }
    }

    private class Harness(scope: TestScope, uid: String?, observer: RemoteBotsObserver = RemoteBotsObserver.None, onBuild: (String) -> Unit = {}) {
        val account = FakeAccount(uid)
        val clients = mutableListOf<FakeClient>()
        val manager = RemoteControlManager(
            scope = scope.backgroundScope,
            account = account,
            factory = { id, listener -> onBuild(id); FakeClient(id, listener).also { clients += it } },
            pushInstallationId = { "push-1" },
            observer = observer,
        )
    }

    /** What the Bots tab's recorder would be told, in order. */
    private class RecordingObserver(val events: MutableList<String> = mutableListOf()) : RemoteBotsObserver {
        val remembered = mutableMapOf<String, String?>()
        override suspend fun lastHostId(uid: String): String? { events += "lastHostId:$uid"; return remembered[uid] }
        override fun onAccount(uid: String?) { events += "account:$uid" }
        override fun onConnected(uid: String, hostId: String) { events += "connected:$uid:$hostId" }
        override fun onBots(uid: String, hostId: String, state: RemoteBotsStateFfi) {
            events += "bots:$uid:$hostId:" + when (state) {
                is RemoteBotsStateFfi.Bots -> state.bots.size.toString()
                RemoteBotsStateFfi.Unsupported -> "unsupported"
                RemoteBotsStateFfi.Unknown -> "unknown"
            }
        }
        override fun onBotThreads(uid: String, hostId: String, botId: String, state: RemoteBotThreadsStateFfi) {
            events += "threads:$uid:$hostId:$botId:" + (state.threads?.size?.toString() ?: "failed")
        }
        override fun onForgotten(uid: String, hostId: String) { events += "forgotten:$uid:$hostId" }
    }

    private fun connectionState(host: String, phase: RemoteConnectionPhaseFfi) = RemoteConnectionStateFfi(
        hostId = host, phase = phase, transport = null, pairingCode = null, failure = null,
        attempt = 0u, retryAtMs = null, host = null, everReady = false,
    )

    private fun host(id: String, paired: Boolean) = RemoteHostFfi(id, "Mac $id", "macos", "1.0", true, 0uL, paired)

    @Test fun forgettingCurrentHostRejectsPendingAndNewAppsUntilConnectionDeparts() = runTest(UnconfinedTestDispatcher()) {
        val h = Harness(this, "u1"); runCurrent()
        val client = h.clients.single()
        client.state = connectionState("h", RemoteConnectionPhaseFfi.READY)
        client.listener.onConnectionState(client.state!!)
        val appId = "a" + "1".repeat(32)
        val url = "https://${appId}-0123456789abcdef0123.route.apps.example.com/__lingxi_remote_bootstrap#${"A".repeat(43)}"
        val ticket = RemoteLocalAppOpenFfi(appId, 1uL, url, 9_007_199_254_740_991uL)
        client.openGate = CompletableDeferred()
        val pending = launch { assertTrue(runCatching { h.manager.openApp("h", appId, 1uL) }.isFailure) }
        runCurrent()
        client.forgetGate = CompletableDeferred()
        val forget = launch { h.manager.forget("h") }
        runCurrent()
        assertTrue(runCatching { h.manager.openApp("h", appId, 1uL) }.isFailure)
        assertTrue(runCatching { h.manager.listApps("h") }.isFailure)
        client.openGate!!.complete(ticket); pending.join()
        client.forgetGate!!.complete(Unit); forget.join()
        assertTrue(runCatching { h.manager.openApp("h", appId, 1uL) }.isFailure)
        client.state = connectionState("h", RemoteConnectionPhaseFfi.IDLE)
        client.listener.onConnectionState(client.state!!)
        client.state = connectionState("h", RemoteConnectionPhaseFfi.READY)
        client.listener.onConnectionState(client.state!!)
        assertEquals(url, h.manager.openApp("h", appId, 1uL))
    }

    @Test fun forgettingAnotherHostDoesNotInvalidateCurrentAppRequest() = runTest(UnconfinedTestDispatcher()) {
        val h = Harness(this, "u1"); runCurrent()
        val client = h.clients.single()
        client.state = connectionState("h", RemoteConnectionPhaseFfi.READY)
        client.listener.onConnectionState(client.state!!)
        val appId = "a" + "1".repeat(32)
        val url = "https://${appId}-0123456789abcdef0123.route.apps.example.com/__lingxi_remote_bootstrap#${"A".repeat(43)}"
        client.openGate = CompletableDeferred()
        var result: Result<String>? = null
        val pending = launch { result = runCatching { h.manager.openApp("h", appId, 1uL) } }
        runCurrent()
        client.forgetGate = CompletableDeferred()
        val forget = launch { h.manager.forget("other") }
        runCurrent()
        client.openGate!!.complete(RemoteLocalAppOpenFfi(appId, 1uL, url, 9_007_199_254_740_991uL))
        pending.join()
        assertEquals(url, result!!.getOrThrow())
        assertEquals(url, h.manager.openApp("h", appId, 1uL))
        client.forgetGate!!.complete(Unit); forget.join()
    }

    @Test fun failedAndConcurrentForgetsReleaseOnlyTheirOwnBarrier() = runTest(UnconfinedTestDispatcher()) {
        val h = Harness(this, "u1"); runCurrent()
        val client = h.clients.single()
        client.state = connectionState("h", RemoteConnectionPhaseFfi.READY)
        client.listener.onConnectionState(client.state!!)
        client.listGate = CompletableDeferred(emptyList())
        client.forgetGate = CompletableDeferred()
        val first = launch { h.manager.forget("h") }
        val second = launch { runCatching { h.manager.forget("h") } }
        runCurrent()
        first.cancel(); first.join()
        assertTrue(runCatching { h.manager.listApps("h") }.isFailure)
        client.forgetGate!!.completeExceptionally(IllegalStateException("forget failed"))
        second.join()
        assertEquals(emptyList<RemoteLocalAppFfi>(), h.manager.listApps("h"))
    }

    @Test fun forgettingReconnectingHostBlocksUntilAttemptChanges() = runTest(UnconfinedTestDispatcher()) {
        val h = Harness(this, "u1"); runCurrent()
        val client = h.clients.single()
        client.state = connectionState("h", RemoteConnectionPhaseFfi.RECONNECTING)
        client.listener.onConnectionState(client.state!!)
        client.listGate = CompletableDeferred(emptyList())
        client.forgetGate = CompletableDeferred()
        val forget = launch { h.manager.forget("h") }
        runCurrent()
        client.state = connectionState("h", RemoteConnectionPhaseFfi.READY)
        client.listener.onConnectionState(client.state!!)
        assertTrue(runCatching { h.manager.listApps("h") }.isFailure)
        client.state = client.state!!.copy(attempt = 1u)
        client.listener.onConnectionState(client.state!!)
        assertTrue(runCatching { h.manager.listApps("h") }.isFailure)
        client.forgetGate!!.complete(Unit); forget.join()
        assertEquals(emptyList<RemoteLocalAppFfi>(), h.manager.listApps("h"))
    }

    @Test fun newForgetAfterReconnectWaitsForItsOwnDeparture() = runTest(UnconfinedTestDispatcher()) {
        val h = Harness(this, "u1"); runCurrent()
        val client = h.clients.single()
        client.state = connectionState("h", RemoteConnectionPhaseFfi.READY)
        client.listener.onConnectionState(client.state!!)
        client.listGate = CompletableDeferred(emptyList())
        client.forgetGate = CompletableDeferred()
        val first = launch { h.manager.forget("h") }
        runCurrent()
        client.state = connectionState("h", RemoteConnectionPhaseFfi.IDLE)
        client.listener.onConnectionState(client.state!!)
        client.state = connectionState("h", RemoteConnectionPhaseFfi.READY).copy(attempt = 1u)
        client.listener.onConnectionState(client.state!!)
        val gate = client.forgetGate!!
        client.forgetGate = null
        h.manager.forget("h")
        gate.complete(Unit); first.join()
        assertTrue(runCatching { h.manager.listApps("h") }.isFailure)
        client.state = connectionState("h", RemoteConnectionPhaseFfi.IDLE)
        client.listener.onConnectionState(client.state!!)
        client.state = connectionState("h", RemoteConnectionPhaseFfi.READY).copy(attempt = 2u)
        client.listener.onConnectionState(client.state!!)
        assertEquals(emptyList<RemoteLocalAppFfi>(), h.manager.listApps("h"))
    }

    @Test fun accountReplacementDoesNotRetainForgetBarrier() = runTest(UnconfinedTestDispatcher()) {
        val h = Harness(this, "u1"); runCurrent()
        val first = h.clients.single()
        first.state = connectionState("h", RemoteConnectionPhaseFfi.READY)
        first.listener.onConnectionState(first.state!!)
        first.forgetGate = CompletableDeferred()
        val forget = launch { h.manager.forget("h") }
        runCurrent()
        h.account.mutableUid.value = "u2"; runCurrent()
        val replacement = h.clients.last()
        replacement.state = connectionState("h", RemoteConnectionPhaseFfi.READY)
        replacement.listener.onConnectionState(replacement.state!!)
        replacement.listener.onHosts(listOf(host("h", paired = true)))
        replacement.listGate = CompletableDeferred(emptyList())
        assertEquals(emptyList<RemoteLocalAppFfi>(), h.manager.listApps("h"))
        first.forgetGate!!.complete(Unit); forget.join()
        assertTrue(h.manager.hosts.value.hosts.single().paired)
        assertEquals(1, first.calls.count { it == "forget:h" })
        assertFalse(replacement.calls.contains("forget:h"))
    }

    @Test fun oldConnectionCallbackWaitingOnLockCannotOverwriteReplacement() = runTest(UnconfinedTestDispatcher()) {
        val h = Harness(this, "u1"); runCurrent()
        val oldListener = h.clients.single().listener
        val lock = RemoteControlManager::class.java.getDeclaredField("appRequestLock").apply { isAccessible = true }.get(h.manager)
        val finished = CountDownLatch(2)
        lateinit var callbacks: List<Thread>
        lateinit var replacement: FakeClient
        lateinit var forget: kotlinx.coroutines.Job
        synchronized(lock) {
            callbacks = listOf(
                Thread {
                    try { runBlocking { oldListener.onConnectionState(connectionState("old", RemoteConnectionPhaseFfi.IDLE)) } }
                    finally { finished.countDown() }
                },
                Thread {
                    try { runBlocking { oldListener.onSessions("old", emptyList()) } }
                    finally { finished.countDown() }
                },
            )
            callbacks.forEach { it.start() }
            val deadline = System.nanoTime() + TimeUnit.SECONDS.toNanos(5)
            while (callbacks.any { it.state != Thread.State.BLOCKED } && System.nanoTime() < deadline) Thread.yield()
            check(callbacks.all { it.state == Thread.State.BLOCKED }) { "old callbacks did not reach the manager lock" }
            h.account.mutableUid.value = "u2"
            runCurrent()
            replacement = h.clients.last()
            check(replacement.uid == "u2")
            replacement.state = connectionState("new", RemoteConnectionPhaseFfi.READY)
            runBlocking { replacement.listener.onConnectionState(replacement.state!!) }
            replacement.forgetGate = CompletableDeferred()
            forget = launch { h.manager.forget("new") }
            runCurrent()
        }
        assertTrue(withContext(Dispatchers.IO) { finished.await(5, TimeUnit.SECONDS) })
        callbacks.forEach { it.join() }
        assertEquals("new", h.manager.connection.value?.hostId)
        assertEquals("new", h.manager.sessions.value.hostId)
        replacement.forgetGate!!.complete(Unit); forget.join()
        replacement.listGate = CompletableDeferred(emptyList())
        assertTrue(runCatching { h.manager.listApps("new") }.isFailure)
        replacement.state = connectionState("new", RemoteConnectionPhaseFfi.IDLE)
        replacement.listener.onConnectionState(replacement.state!!)
        replacement.state = connectionState("new", RemoteConnectionPhaseFfi.READY)
        replacement.listener.onConnectionState(replacement.state!!)
        assertEquals(emptyList<RemoteLocalAppFfi>(), h.manager.listApps("new"))
    }

    @Test fun accountChangeWhileWaitingForClientDoesNotForgetOnReplacement() = runTest(UnconfinedTestDispatcher()) {
        val account = FakeAccount("u1")
        val buildGate = CompletableDeferred<Unit>()
        val clients = mutableListOf<FakeClient>()
        val manager = RemoteControlManager(backgroundScope, account, { uid, listener ->
            if (uid == "u1") buildGate.await()
            FakeClient(uid, listener).also { clients += it }
        }, { null })
        runCurrent()
        val oldForget = launch { runCatching { manager.forget("h") } }
        runCurrent()
        account.mutableUid.value = "u2"
        buildGate.complete(Unit); runCurrent(); oldForget.join()
        assertEquals("u2", clients.last().uid)
        assertTrue(clients.filter { it.uid == "u2" }.none { "forget:h" in it.calls })
    }

    @Test fun requestAdmissionSnapshotsConnectionAndEpochTogether() = runTest {
        val h = Harness(this, "u1"); runCurrent()
        val client = h.clients.single()
        client.state = connectionState("h", RemoteConnectionPhaseFfi.READY)
        client.listener.onConnectionState(client.state!!)
        client.listGate = CompletableDeferred()
        val sampled = CountDownLatch(1)
        val release = CountDownLatch(1)
        var reads = 0
        client.onStateRead = {
            if (++reads == 2) {
                sampled.countDown()
                check(release.await(5, TimeUnit.SECONDS))
            }
        }
        val pending = launch(Dispatchers.Default) { assertTrue(runCatching { h.manager.listApps("h") }.isFailure) }
        try {
            assertTrue(withContext(Dispatchers.IO) { sampled.await(5, TimeUnit.SECONDS) })
            val transitionStarted = CompletableDeferred<Thread>()
            val transition = launch(Dispatchers.Default) {
                transitionStarted.complete(Thread.currentThread())
                client.state = connectionState("h", RemoteConnectionPhaseFfi.IDLE)
                client.listener.onConnectionState(client.state!!)
                client.state = connectionState("h", RemoteConnectionPhaseFfi.READY)
                client.listener.onConnectionState(client.state!!)
            }
            val transitionThread = transitionStarted.await()
            // The listener either completed before admission or is blocked by
            // its lock. In both cases the ordering is known before releasing it.
            assertTrue(withContext(Dispatchers.IO) {
                val deadline = System.nanoTime() + TimeUnit.SECONDS.toNanos(5)
                while (!transition.isCompleted && transitionThread.state != Thread.State.BLOCKED && System.nanoTime() < deadline) Thread.yield()
                transition.isCompleted || transitionThread.state == Thread.State.BLOCKED
            })
            release.countDown()
            // Admission/listener jobs run on real threads. A test-scheduler
            // timeout can expire instantly while those threads are still running.
            withContext(Dispatchers.Default) {
                withTimeout(5_000) { transition.join() }
                client.listGate!!.complete(emptyList())
                withTimeout(5_000) { pending.join() }
            }
        } finally {
            release.countDown()
        }
    }

    @Test fun appListRejectsResponseAfterReconnectEvenIfAttemptResets() = runTest(UnconfinedTestDispatcher()) {
        val h = Harness(this, "u1"); runCurrent()
        val client = h.clients.single()
        client.state = connectionState("h", RemoteConnectionPhaseFfi.READY)
        client.listener.onConnectionState(client.state!!)
        val gate = CompletableDeferred<List<RemoteLocalAppFfi>>(); client.listGate = gate
        var result: Result<List<RemoteLocalAppFfi>>? = null
        val request = launch { result = runCatching { h.manager.listApps("h") } }
        runCurrent()
        client.listener.onConnectionState(connectionState("h", RemoteConnectionPhaseFfi.RECONNECTING))
        client.listener.onConnectionState(client.state!!)
        gate.complete(emptyList()); request.join()
        assertTrue(result!!.isFailure)
    }

    @Test fun appListRejectsResponseWhenConnectionCloseIsRequested() = runTest(UnconfinedTestDispatcher()) {
        val h = Harness(this, "u1"); runCurrent()
        val client = h.clients.single()
        client.state = connectionState("h", RemoteConnectionPhaseFfi.READY)
        val gate = CompletableDeferred<List<RemoteLocalAppFfi>>(); client.listGate = gate
        // Keep the old connection ready while asynchronous close is waiting.
        client.stopWatchingGate = CompletableDeferred()
        var result: Result<List<RemoteLocalAppFfi>>? = null
        val request = launch { result = runCatching { h.manager.listApps("h") } }
        runCurrent(); h.manager.closeConnection(); runCurrent()
        gate.complete(emptyList()); request.join()
        assertTrue(result!!.isFailure)
        client.stopWatchingGate!!.complete(Unit)
    }

    @Test fun appOpenRejectsChangedHostAndAccount() = runTest(UnconfinedTestDispatcher()) {
        val appId = "a" + "1".repeat(32)
        val url = "https://${appId}-0123456789abcdef0123.route.apps.example.com/__lingxi_remote_bootstrap#${"A".repeat(43)}"
        for (changeAccount in listOf(false, true)) {
            val h = Harness(this, "u1"); runCurrent()
            val client = h.clients.single()
            client.state = connectionState("h", RemoteConnectionPhaseFfi.READY)
            val gate = CompletableDeferred<RemoteLocalAppOpenFfi>(); client.openGate = gate
            var result: Result<String>? = null
            val request = launch { result = runCatching { h.manager.openApp("h", appId, 1uL) } }
            runCurrent()
            if (changeAccount) { h.account.mutableUid.value = "u2"; runCurrent() }
            else client.state = connectionState("other", RemoteConnectionPhaseFfi.READY)
            gate.complete(RemoteLocalAppOpenFfi(appId, 1uL, url, 9_007_199_254_740_991uL))
            request.join()
            assertTrue(result!!.isFailure)
        }
    }

    @Test fun appOpenRejectsInsecureExpiredAndMismatchedLinks() = runTest(UnconfinedTestDispatcher()) {
        val h = Harness(this, "u1"); runCurrent()
        val client = h.clients.single(); client.state = connectionState("h", RemoteConnectionPhaseFfi.READY)
        val appId = "a" + "1".repeat(32)
        val url = "https://${appId}-0123456789abcdef0123.route.apps.example.com/__lingxi_remote_bootstrap#${"A".repeat(43)}"
        val expires = 9_007_199_254_740_991uL
        for (response in listOf(
            RemoteLocalAppOpenFfi(appId, 1uL, url.replace("https:", "http:") + "secret-ticket", expires),
            RemoteLocalAppOpenFfi(appId, 1uL, url, 1uL),
            RemoteLocalAppOpenFfi(appId, 2uL, url, expires),
            RemoteLocalAppOpenFfi("a" + "2".repeat(32), 1uL, url, expires),
            RemoteLocalAppOpenFfi(appId, 1uL, url.replace("https://", "https://user@"), expires),
        )) {
            client.openGate = CompletableDeferred(response)
            val failure = runCatching { h.manager.openApp("h", appId, 1uL) }.exceptionOrNull()
            assertTrue(failure != null)
            assertFalse(failure.toString().contains("secret-ticket"))
        }
        client.openGate = CompletableDeferred(RemoteLocalAppOpenFfi(appId, 1uL, url, expires))
        assertEquals(url, h.manager.openApp("h", appId, 1uL))
    }

    @Test fun filePickerReturnWaitsForTheSamePeerToReconnect() = runTest(UnconfinedTestDispatcher()) {
        val h = Harness(this, "u1"); runCurrent(); val client = h.clients.single()
        client.state = connectionState("h", RemoteConnectionPhaseFfi.IDLE)
        var result: Result<RemoteClientPort>? = null
        val request = launch { result = runCatching { h.manager.filePort("u1", "h", flowOf(true)) } }
        runCurrent(); assertNull(result); assertTrue(client.calls.contains("nudge"))
        client.state = connectionState("h", RemoteConnectionPhaseFfi.READY).copy(transport = RemoteTransportFfi.WEBRTC)
        client.listener.onConnectionState(client.state!!); runCurrent(); request.join()
        assertTrue(result!!.getOrThrow() === client)
    }

    @Test fun filePickerReturnCannotResumeIntoAnotherAccount() = runTest(UnconfinedTestDispatcher()) {
        val h = Harness(this, "u1"); runCurrent()
        h.clients.single().state = connectionState("h", RemoteConnectionPhaseFfi.IDLE)
        var result: Result<RemoteClientPort>? = null
        val request = launch { result = runCatching { h.manager.filePort("u1", "h", flowOf(true)) } }
        runCurrent(); h.account.mutableUid.value = "u2"; runCurrent(); request.join()
        assertTrue(result!!.exceptionOrNull() is kotlinx.coroutines.CancellationException)
    }

    @Test fun filePickerReturnTimesOutWithoutStartingFileWork() = runTest(UnconfinedTestDispatcher()) {
        val h = Harness(this, "u1"); runCurrent()
        var result: Result<RemoteClientPort>? = null
        val request = launch { result = runCatching { h.manager.filePort("u1", "h", flowOf(true)) } }
        runCurrent(); assertNull(result); advanceTimeBy(30_001); runCurrent(); request.join()
        assertTrue(result!!.exceptionOrNull()?.message?.contains("reconnecting") == true)
    }

    @Test fun filePickerReturnRejectsTunnelAndAnotherHost() = runTest(UnconfinedTestDispatcher()) {
        val h = Harness(this, "u1"); runCurrent(); val client = h.clients.single()
        client.state = connectionState("other", RemoteConnectionPhaseFfi.READY).copy(transport = RemoteTransportFfi.WEBRTC)
        assertTrue(runCatching { h.manager.filePort("u1", "h", flowOf(true)) }.isFailure)
        client.state = connectionState("h", RemoteConnectionPhaseFfi.READY).copy(transport = RemoteTransportFfi.TUNNEL)
        assertTrue(runCatching { h.manager.filePort("u1", "h", flowOf(true)) }.isFailure)
    }

    @Test fun filePickerReturnWaitsForSessionReattachmentAfterPeerReady() = runTest(UnconfinedTestDispatcher()) {
        val h = Harness(this, "u1"); runCurrent(); val client = h.clients.single()
        client.state = connectionState("h", RemoteConnectionPhaseFfi.READY).copy(transport = RemoteTransportFfi.WEBRTC)
        val attached = MutableStateFlow(false)
        var result: Result<RemoteClientPort>? = null
        val request = launch { result = runCatching { h.manager.filePort("u1", "h", attached) } }
        runCurrent(); assertNull(result)
        attached.value = true; runCurrent(); request.join()
        assertTrue(result!!.getOrThrow() === client)
    }

    @Test fun filePickerReturnBoundsClientConstructionAndFailure() = runTest(UnconfinedTestDispatcher()) {
        for (fails in listOf(false, true)) {
            val account = FakeAccount("u1")
            val gate = CompletableDeferred<Unit>()
            val manager = RemoteControlManager(backgroundScope, account, { _, _ ->
                if (fails) error("client construction failed")
                gate.await()
                error("not reached")
            }, { null })
            var result: Result<RemoteClientPort>? = null
            val request = launch { result = runCatching { manager.filePort("u1", "h", flowOf(true)) } }
            runCurrent(); assertNull(result)
            advanceTimeBy(30_001); runCurrent(); request.join()
            assertTrue(result!!.exceptionOrNull()?.message?.contains("reconnecting") == true)
        }
    }

    @Test fun resumableFileOperationRetriesClosedTransportOnceOnSameClient() = runTest(UnconfinedTestDispatcher()) {
        val h = Harness(this, "u1"); runCurrent(); val client = h.clients.single()
        client.state = connectionState("h", RemoteConnectionPhaseFfi.READY).copy(transport = RemoteTransportFfi.WEBRTC)
        for (message in listOf("closed: ice_failed", "not connected")) {
            var calls = 0
            val answer = h.manager.fileOperation("u1", "h", flowOf(true)) { bound ->
                assertTrue(bound === client)
                if (++calls == 1) {
                    client.state = client.state!!.copy(attempt = client.state!!.attempt + 1u)
                    client.listener.onConnectionState(client.state!!)
                    throw RemoteFfiException.InvalidArgument(message)
                }
                "verified"
            }
            assertEquals("verified", answer); assertEquals(2, calls)
        }
    }

    @Test fun resumableFileOperationDoesNotRetryCancellationOrValidationFailures() = runTest(UnconfinedTestDispatcher()) {
        val h = Harness(this, "u1"); runCurrent()
        h.clients.single().state = connectionState("h", RemoteConnectionPhaseFfi.READY).copy(transport = RemoteTransportFfi.WEBRTC)
        for (failure in listOf(kotlinx.coroutines.CancellationException("cancelled"), RemoteFfiException.InvalidArgument("file checksum mismatch"))) {
            var calls = 0
            val result = runCatching { h.manager.fileOperation("u1", "h", flowOf(true)) { calls++; throw failure } }
            assertTrue(result.exceptionOrNull() === failure); assertEquals(1, calls)
        }
    }

    @Test fun resumableFileOperationRequiresReconnectForAFileOnlyClosure() = runTest(UnconfinedTestDispatcher()) {
        val h = Harness(this, "u1"); runCurrent()
        h.clients.single().state = connectionState("h", RemoteConnectionPhaseFfi.READY).copy(transport = RemoteTransportFfi.WEBRTC)
        var calls = 0
        val result = runCatching { h.manager.fileOperation("u1", "h", flowOf(true)) { calls++; throw RemoteFfiException.InvalidArgument("file channel closed") } }
        assertTrue(result.exceptionOrNull()?.message?.contains("Exit Remote Control and reconnect") == true)
        assertEquals(1, calls)
    }

    @Test fun resumableFileOperationStopsAfterSecondClosureOnNewAttempt() = runTest(UnconfinedTestDispatcher()) {
        val h = Harness(this, "u1"); runCurrent(); val client = h.clients.single()
        client.state = connectionState("h", RemoteConnectionPhaseFfi.READY).copy(transport = RemoteTransportFfi.WEBRTC)
        var calls = 0
        val result = runCatching { h.manager.fileOperation("u1", "h", flowOf(true)) {
            calls++; client.state = client.state!!.copy(attempt = client.state!!.attempt + 1u)
            client.listener.onConnectionState(client.state!!)
            throw RemoteFfiException.InvalidArgument("file channel closed")
        } }
        assertTrue(result.exceptionOrNull()?.message?.contains("Exit Remote Control and reconnect") == true)
        assertEquals(2, calls)
    }

    @Test fun resumableFileOperationCannotRetryAfterAccountChange() = runTest(UnconfinedTestDispatcher()) {
        val h = Harness(this, "u1"); runCurrent()
        h.clients.single().state = connectionState("h", RemoteConnectionPhaseFfi.READY).copy(transport = RemoteTransportFfi.WEBRTC)
        var calls = 0
        val result = runCatching { h.manager.fileOperation("u1", "h", flowOf(true)) {
            calls++; h.account.mutableUid.value = "u2"
            throw RemoteFfiException.InvalidArgument("file channel closed")
        } }
        assertTrue(result.exceptionOrNull() is kotlinx.coroutines.CancellationException); assertEquals(1, calls)
    }

    @Test fun oneClientIsMadeForTheSignedInUserWithThePushInstallationId() = runTest(UnconfinedTestDispatcher()) {
        val harness = Harness(this, "u1")
        runCurrent()
        val client = harness.clients.single()
        assertEquals("u1", client.uid)
        assertEquals("push-1", client.pushId)
    }

    @Test fun nothingIsMadeWhileSignedOut() = runTest(UnconfinedTestDispatcher()) {
        val harness = Harness(this, null)
        runCurrent()
        assertTrue(harness.clients.isEmpty())
        assertNull(harness.manager.uid.value)
    }

    @Test fun anAccountChangeClosesTheOldClientAndMakesANewOne() = runTest(UnconfinedTestDispatcher()) {
        val harness = Harness(this, "u1")
        runCurrent()
        val first = harness.clients.single()
        first.listener.onHosts(listOf(host("h", true)))
        assertEquals(1, harness.manager.hosts.value.hosts.size)
        harness.account.mutableUid.value = "u2"
        runCurrent()
        assertTrue(first.closed)
        assertEquals(listOf("u1", "u2"), harness.clients.map { it.uid })
        assertTrue(harness.manager.hosts.value.hosts.isEmpty())
        // A callback of the old client no longer reaches the screens.
        first.listener.onHosts(listOf(host("stale", true)))
        assertTrue(harness.manager.hosts.value.hosts.isEmpty())
        harness.account.mutableUid.value = null
        runCurrent()
        assertTrue(harness.clients.last().closed)
    }

    @Test fun listenerCallbacksBecomeState() = runTest(UnconfinedTestDispatcher()) {
        val harness = Harness(this, "u1")
        runCurrent()
        val listener = harness.clients.single().listener
        listener.onHosts(listOf(host("a", false), host("b", true)))
        assertTrue(harness.manager.hosts.value.loaded)
        assertEquals(listOf("a", "b"), harness.manager.hosts.value.hosts.map { it.hostId })
        listener.onHostsFailed("offline")
        assertEquals("offline", harness.manager.hosts.value.error)
        listener.onConnectionState(connectionState("b", RemoteConnectionPhaseFfi.READY))
        assertEquals(RemoteConnectionPhaseFfi.READY, harness.manager.connection.value?.phase)
        assertEquals("b", harness.manager.sessions.value.hostId)
        listener.onSessions("b", emptyList())
        assertEquals(emptyList<RemoteSessionSummaryFfi>(), harness.manager.sessions.value.sessions)
    }

    @Test fun forgettingMarksTheHostUnpaired() = runTest(UnconfinedTestDispatcher()) {
        val harness = Harness(this, "u1")
        runCurrent()
        val client = harness.clients.single()
        client.listener.onHosts(listOf(host("b", true)))
        harness.manager.forget("b")
        assertEquals(listOf("forget:b"), client.calls)
        assertFalse(harness.manager.hosts.value.hosts.single().paired)
    }

    @Test fun repairAndRetryAndNudgeGoToTheClient() = runTest(UnconfinedTestDispatcher()) {
        val harness = Harness(this, "u1")
        runCurrent()
        val client = harness.clients.single()
        harness.manager.repair("b")
        harness.manager.retry()
        harness.manager.nudge()
        runCurrent()
        assertEquals(listOf("repair:b", "retry", "nudge"), client.calls)
    }

    @Test fun closingTheScreensEndsTheConnection() = runTest(UnconfinedTestDispatcher()) {
        val harness = Harness(this, "u1")
        runCurrent()
        harness.manager.closeConnection()
        runCurrent()
        assertEquals(listOf("stopWatchingHosts", "disconnect"), harness.clients.single().calls)
    }

    @Test fun aLateScreenCleanupCannotDisconnectAReopenedComputer() = runTest(UnconfinedTestDispatcher()) {
        val harness = Harness(this, "u1")
        runCurrent()
        val client = harness.clients.single()
        harness.manager.connect("old")
        val stopped = CompletableDeferred<Unit>()
        client.stopWatchingGate = stopped
        harness.manager.closeConnection()
        runCurrent()
        val reopened = launch { harness.manager.connect("new") }
        runCurrent()
        stopped.complete(Unit)
        reopened.join()
        runCurrent()
        assertEquals("new", client.connectionState()?.hostId)
        assertEquals("connect:new", client.calls.last())
    }

    @Test fun followingDialsAttachesDeliversEveryUpdateAndDetaches() = runTest(UnconfinedTestDispatcher()) {
        val harness = Harness(this, "u1")
        runCurrent()
        val client = harness.clients.single()
        val received = mutableListOf<RemoteSessionUpdateFfi>()
        val job = launch { harness.manager.followSession("h", "s1").collect { received += it } }
        runCurrent()
        assertEquals(listOf("connect:h", "attach:s1"), client.calls)
        // A burst before the collector runs loses nothing.
        client.listener.onSessionUpdate(remoteUpdate(sessionId = "s1", replace = true))
        client.listener.onSessionUpdate(remoteUpdate(sessionId = "s1"))
        client.listener.onSessionUpdate(remoteUpdate(sessionId = "other"))
        runCurrent()
        assertEquals(listOf(true, false), received.map { it.replace })
        job.cancel()
        runCurrent()
        assertEquals("detach:s1", client.calls.last())
        client.listener.onSessionUpdate(remoteUpdate(sessionId = "s1"))
        runCurrent()
        assertEquals(2, received.size)
    }

    @Test fun anAlreadyConnectedHostIsNotDialledAgain() = runTest(UnconfinedTestDispatcher()) {
        val harness = Harness(this, "u1")
        runCurrent()
        val client = harness.clients.single()
        client.listener.onConnectionState(connectionState("h", RemoteConnectionPhaseFfi.READY))
        val job = launch { harness.manager.followSession("h", "s1").collect { } }
        runCurrent()
        assertEquals(listOf("attach:s1"), client.calls)
        job.cancel()
    }
    @Test fun accountChangeEndsTheOldFollowAndRejectsItsSessionCommands() = runTest(UnconfinedTestDispatcher()) {
        val harness = Harness(this, "u1")
        runCurrent()
        val port = harness.manager.sessionPort("h", "s1")
        val received = mutableListOf<RemoteSessionUpdateFfi>()
        val job = launch { port.follow().collect { received += it } }
        runCurrent()
        harness.clients.first().listener.onSessionUpdate(remoteUpdate())
        runCurrent()
        assertEquals(1, received.size)
        harness.account.mutableUid.value = "u2"
        runCurrent()
        assertTrue(job.isCompleted)
        assertTrue(runCatching { port.sendPrompt("old account", emptyList()) }.isFailure)
        assertTrue(harness.clients.last().calls.none { it.startsWith("send:") })
    }

    @Test fun admittedWritesFinishOnTheirHostBeforeAComputerSwitchEvenIfCallerLeaves() = runTest(UnconfinedTestDispatcher()) {
        val harness = Harness(this, "u1")
        runCurrent()
        val client = harness.clients.single()
        harness.manager.connect("host-a")
        val port = harness.manager.sessionPort("host-a", "same-session")
        val gate = CompletableDeferred<Unit>()
        client.sessionWriteGate = gate
        val send = launch { port.sendPrompt("hello", emptyList()) }
        val approval = launch { port.respondPermission(1uL, true) }
        runCurrent()
        val switching = launch { harness.manager.connect("host-b") }
        runCurrent()
        assertEquals("host-a", client.connectionState()?.hostId)
        assertFalse(client.calls.contains("connect:host-b"))
        send.cancel()
        approval.cancel()
        gate.complete(Unit)
        switching.join()
        runCurrent()
        assertEquals(listOf("host-a", "host-a"), client.sessionWriteHosts)
        assertEquals("host-b", client.connectionState()?.hostId)
        assertTrue(client.calls.indexOf("send:same-session:hello") < client.calls.indexOf("connect:host-b"))
        assertTrue(client.calls.indexOf("permission:same-session:1:true") < client.calls.indexOf("connect:host-b"))
        assertTrue(runCatching { port.sendPrompt("stale", emptyList()) }.isFailure)
    }

    @Test fun closingTheOverlayWaitsForAnAdmittedSendBeforeDisconnecting() = runTest(UnconfinedTestDispatcher()) {
        val harness = Harness(this, "u1")
        runCurrent()
        val client = harness.clients.single()
        harness.manager.connect("host-a")
        val gate = CompletableDeferred<Unit>()
        client.sessionWriteGate = gate
        val send = launch { harness.manager.sessionPort("host-a", "s1").sendPrompt("hello", emptyList()) }
        runCurrent()
        harness.manager.closeConnection()
        runCurrent()
        assertFalse(client.calls.contains("disconnect"))
        gate.complete(Unit)
        send.join()
        runCurrent()
        assertEquals(listOf("host-a"), client.sessionWriteHosts)
        assertTrue(client.calls.indexOf("send:s1:hello") < client.calls.indexOf("disconnect"))
    }

    @Test fun departingWaitsForPermissionAndAReplacementCancelsTheOldDetach() = runTest(UnconfinedTestDispatcher()) {
        val harness = Harness(this, "u1")
        runCurrent()
        val client = harness.clients.single()
        val port = harness.manager.sessionPort("host-a", "s1")
        val old = launch { port.follow().collect {} }
        runCurrent()
        val gate = CompletableDeferred<Unit>()
        client.sessionWriteGate = gate
        val approval = launch { port.respondPermission(1uL, true) }
        runCurrent()
        old.cancel()
        runCurrent()
        assertFalse(client.calls.contains("detach:s1"))
        val replacement = launch { port.follow().collect {} }
        runCurrent()
        gate.complete(Unit)
        approval.join()
        runCurrent()
        assertTrue(client.calls.contains("permission:s1:1:true"))
        assertFalse(client.calls.contains("detach:s1"))
        replacement.cancel()
        runCurrent()
        assertEquals(1, client.calls.count { it == "detach:s1" })
    }

    @Test fun reopeningARetainedOwnerReattachesItsHostAndSession() = runTest(UnconfinedTestDispatcher()) {
        val harness = Harness(this, "u1")
        runCurrent()
        val client = harness.clients.single()
        val following = launch { harness.manager.followSession("host-a", "same-session").collect {} }
        runCurrent()
        harness.manager.connect("host-b")
        harness.manager.reattachSession("host-a", "same-session", "u1")
        assertEquals("host-a", client.connectionState()?.hostId)
        assertEquals(listOf("connect:host-a", "attach:same-session", "connect:host-b", "connect:host-a", "attach:same-session"), client.calls)
        following.cancel()
    }

    @Test fun anAccountThatChangesDuringBuildNeverPublishesTheOldClient() = runTest(UnconfinedTestDispatcher()) {
        val account = FakeAccount("u1")
        val gate = CompletableDeferred<Unit>()
        val clients = mutableListOf<FakeClient>()
        val manager = RemoteControlManager(backgroundScope, account, { uid, listener ->
            FakeClient(uid, listener).also {
                clients += it
                if (uid == "u1") gate.await()
            }
        }, { null })
        runCurrent()
        account.mutableUid.value = "u2"
        clients.first().listener.onHosts(listOf(host("private", true)))
        assertTrue(manager.hosts.value.hosts.isEmpty())
        gate.complete(Unit)
        runCurrent()
        assertTrue(clients.first().closed)
        manager.watchHosts()
        assertEquals(listOf("watchHosts"), clients.last().calls)
    }


    // --- Bots ------------------------------------------------------------

    private fun bot(id: String, waiting: UInt = 0u) =
        RemoteBotSummaryFfi(id, "Bot $id", "does $id", RemoteBotStatusFfi.IDLE, waiting, null)

    private fun chat(id: String) = RemoteSessionSummaryFfi(id, "Chat $id", "", RemoteSessionStatusFfi.IDLE, null, null, 0u)

    @Test fun theBotsListenerIsSetOnceTheClientIsBuiltAndClearedWhenItIsReplaced() = runTest(UnconfinedTestDispatcher()) {
        val harness = Harness(this, "u1")
        runCurrent()
        val first = harness.clients.single()
        assertNotNull(first.boundBotsListener)
        harness.account.mutableUid.value = "u2"
        runCurrent()
        assertNull(first.boundBotsListener)
        assertTrue(first.closed)
        assertNotNull(harness.clients.last().boundBotsListener)
        harness.account.mutableUid.value = null
        runCurrent()
        assertNull(harness.clients.last().boundBotsListener)
    }

    @Test fun botsAndTheirChatsBecomeStateOfTheirComputer() = runTest(UnconfinedTestDispatcher()) {
        val harness = Harness(this, "u1")
        runCurrent()
        val listener = harness.clients.single().boundBotsListener!!
        assertEquals(RemoteBotsState(), harness.manager.bots.value)
        assertEquals(RemoteBotChatsState(), harness.manager.botChats.value)
        listener.onBots("h", RemoteBotsStateFfi.Bots(listOf(bot("b1"), bot("b2", waiting = 2u))))
        assertEquals(listOf("b1", "b2"), harness.manager.bots.value.listFor("h").map { it.botId })
        assertTrue(harness.manager.bots.value.listFor("other").isEmpty())
        listener.onBotThreads("h", "b1", RemoteBotThreadsStateFfi(listOf(chat("s1")), null))
        listener.onBotThreads("h", "b2", RemoteBotThreadsStateFfi(null, RemoteSessionErrorFfi("unknown_session", "gone")))
        val chats = harness.manager.botChats.value
        assertEquals("h", chats.hostId)
        assertEquals(setOf("b1", "b2"), chats.byBot.keys)
        assertEquals("Chat s1", chats.summaryOf("h", "s1")?.title)
        assertNull(chats.summaryOf("other", "s1"))
        // A Bot's list is replaced as a whole, not merged into.
        listener.onBotThreads("h", "b1", RemoteBotThreadsStateFfi(emptyList(), null))
        assertNull(harness.manager.botChats.value.summaryOf("h", "s1"))
    }

    @Test fun connectingToAnotherComputerDropsTheFirstOnesBotsButReconnectingKeepsThem() = runTest(UnconfinedTestDispatcher()) {
        val harness = Harness(this, "u1")
        runCurrent()
        val client = harness.clients.single()
        val listener = client.boundBotsListener!!
        client.listener.onConnectionState(connectionState("h", RemoteConnectionPhaseFfi.READY))
        listener.onBots("h", RemoteBotsStateFfi.Bots(listOf(bot("b1"))))
        listener.onBotThreads("h", "b1", RemoteBotThreadsStateFfi(emptyList(), null))
        client.listener.onConnectionState(connectionState("h", RemoteConnectionPhaseFfi.RECONNECTING))
        assertEquals(1, harness.manager.bots.value.listFor("h").size)
        assertEquals(setOf("b1"), harness.manager.botChats.value.byBot.keys)
        client.listener.onConnectionState(connectionState("h2", RemoteConnectionPhaseFfi.READY))
        assertEquals(RemoteBotsState("h2"), harness.manager.bots.value)
        assertEquals(RemoteBotChatsState("h2"), harness.manager.botChats.value)
        // The chats of the new computer start a map of their own.
        listener.onBotThreads("h2", "b9", RemoteBotThreadsStateFfi(emptyList(), null))
        assertEquals(setOf("b9"), harness.manager.botChats.value.byBot.keys)
    }

    @Test fun callbacksOfAReplacedClientNeverReachTheScreens() = runTest(UnconfinedTestDispatcher()) {
        val harness = Harness(this, "u1")
        runCurrent()
        val stale = harness.clients.single().boundBotsListener!!
        harness.account.mutableUid.value = "u2"
        runCurrent()
        stale.onBots("h", RemoteBotsStateFfi.Bots(listOf(bot("b1"))))
        stale.onBotThreads("h", "b1", RemoteBotThreadsStateFfi(emptyList(), null))
        assertEquals(RemoteBotsState(), harness.manager.bots.value)
        assertEquals(RemoteBotChatsState(), harness.manager.botChats.value)
        harness.clients.last().boundBotsListener!!.onBots("h", RemoteBotsStateFfi.Unsupported)
        assertEquals(RemoteBotsState("h", RemoteBotsStateFfi.Unsupported), harness.manager.bots.value)
    }

    @Test fun showingABotDialsActivatesItsChatsAndEndsTheCountWhenLeft() = runTest(UnconfinedTestDispatcher()) {
        val harness = Harness(this, "u1")
        runCurrent()
        val client = harness.clients.single()
        val shown = launch { harness.manager.showBotChats("h", "b1") }
        runCurrent()
        assertEquals(listOf("connect:h", "activateBot:b1"), client.calls)
        assertTrue(shown.isActive)
        shown.cancel()
        runCurrent()
        assertEquals(listOf("connect:h", "activateBot:b1", "deactivateBot:b1"), client.calls)
    }

    @Test fun aBotOfTheConnectedComputerIsNotDialledAgain() = runTest(UnconfinedTestDispatcher()) {
        val harness = Harness(this, "u1")
        runCurrent()
        val client = harness.clients.single()
        client.listener.onConnectionState(connectionState("h", RemoteConnectionPhaseFfi.READY))
        val shown = launch { harness.manager.showBotChats("h", "b1") }
        runCurrent()
        assertEquals(listOf("activateBot:b1"), client.calls)
        shown.cancel()
    }

    @Test fun aBotOfAnotherComputerIsActivatedOnlyAfterDiallingIt() = runTest(UnconfinedTestDispatcher()) {
        val harness = Harness(this, "u1")
        runCurrent()
        val client = harness.clients.single()
        // The activations of the connection to "x" are gone with it: the new connection starts from none.
        client.state = connectionState("x", RemoteConnectionPhaseFfi.READY)
        client.listener.onConnectionState(client.state!!)
        val shown = launch { harness.manager.showBotChats("h", "b1") }
        runCurrent()
        assertEquals(listOf("connect:h", "activateBot:b1"), client.calls)
        shown.cancel()
    }

    @Test fun anActivationThatFailsIsHeldAsTheChatsErrorAndKeepsWhatWasHeld() = runTest(UnconfinedTestDispatcher()) {
        val harness = Harness(this, "u1")
        runCurrent()
        val client = harness.clients.single()
        client.boundBotsListener!!.onBotThreads("h", "b1", RemoteBotThreadsStateFfi(listOf(chat("s1")), null))
        client.activateFailure = RemoteFfiException.InvalidArgument("no such bot")
        val shown = launch { harness.manager.showBotChats("h", "b1") }
        runCurrent()
        val held = harness.manager.botChats.value.byBot["b1"]!!
        assertEquals(listOf("s1"), held.threads?.map { it.sessionId })
        assertEquals("internal", held.error?.code)
        assertEquals("no such bot", held.error?.message)
        // Leaving still ends whatever count the failed asking may have left.
        shown.cancel()
        runCurrent()
        assertEquals("deactivateBot:b1", client.calls.last())
    }

    @Test fun retryingEndsTheScreensCountAndStartsItAgain() = runTest(UnconfinedTestDispatcher()) {
        val harness = Harness(this, "u1")
        runCurrent()
        val client = harness.clients.single()
        client.listener.onConnectionState(connectionState("h", RemoteConnectionPhaseFfi.READY))
        harness.manager.retryBotChats("h", "b1")
        assertEquals(listOf("deactivateBot:b1", "activateBot:b1"), client.calls)
    }

    @Test fun aChatIsStartedAndItsSessionIdReturned() = runTest(UnconfinedTestDispatcher()) {
        val harness = Harness(this, "u1")
        runCurrent()
        val client = harness.clients.single()
        assertEquals("bot-chat", harness.manager.createBotThread("h", "b1"))
        assertEquals(listOf("createBot:b1", "createdBot:b1:bot-chat"), client.calls)
        client.createFailure = RemoteFfiException.Rpc("not_remote_enabled", "off")
        val failure = runCatching { harness.manager.createBotThread("h", "b1") }.exceptionOrNull()
        assertEquals("not_remote_enabled", (failure as RemoteFfiException.Rpc).code)
    }

    @Test fun aChatIsMadeOnTheComputerEvenWhenTheCallerStopsWaiting() = runTest(UnconfinedTestDispatcher()) {
        val harness = Harness(this, "u1")
        runCurrent()
        val client = harness.clients.single()
        val gate = CompletableDeferred<String>()
        client.createGate = gate
        var answer: String? = null
        val asking = launch { answer = harness.manager.createBotThread("h", "b1") }
        runCurrent()
        assertEquals(listOf("createBot:b1"), client.calls)
        asking.cancel()
        gate.complete("s9")
        runCurrent()
        // The request was not cut short by the caller leaving, and the leaver is not told which chat it is.
        assertEquals(listOf("createBot:b1", "createdBot:b1:s9"), client.calls)
        assertNull(answer)
    }

    @Test fun aChatCannotBeStartedOnAnotherComputerThanTheConnectedOne() = runTest(UnconfinedTestDispatcher()) {
        val harness = Harness(this, "u1")
        runCurrent()
        val client = harness.clients.single()
        client.listener.onConnectionState(connectionState("x", RemoteConnectionPhaseFfi.READY))
        assertTrue(runCatching { harness.manager.createBotThread("h", "b1") }.isFailure)
        assertTrue(client.calls.none { it.startsWith("createBot") })
    }

    @Test fun theBotOfAChatIsAskedOfTheClientOfTheConnectedComputerOnly() = runTest(UnconfinedTestDispatcher()) {
        val harness = Harness(this, "u1")
        runCurrent()
        val client = harness.clients.single()
        client.botOf["s1"] = "b1"
        assertNull(harness.manager.botOfThread("h", "s1"))
        client.state = connectionState("h", RemoteConnectionPhaseFfi.READY)
        assertEquals("b1", harness.manager.botOfThread("h", "s1"))
        assertNull(harness.manager.botOfThread("h", "s2"))
        assertNull(harness.manager.botOfThread("x", "s1"))
        harness.account.mutableUid.value = null
        runCurrent()
        assertNull(harness.manager.botOfThread("h", "s1"))
    }

    @Test fun aSessionPortNamesTheBotThatHandedOverAPrompt() = runTest(UnconfinedTestDispatcher()) {
        val harness = Harness(this, "u1")
        runCurrent()
        val client = harness.clients.single()
        client.state = connectionState("h", RemoteConnectionPhaseFfi.READY)
        client.botNames["s1" to 4uL] = "Inbox triage"
        val port = harness.manager.sessionPort("h", "s1")
        assertEquals("Inbox triage", port.promptOriginBotName(4uL))
        assertNull(port.promptOriginBotName(5uL))
        assertNull(harness.manager.sessionPort("other", "s1").promptOriginBotName(4uL))
        // A port of the previous account hears of no Bot of the next one's.
        harness.account.mutableUid.value = "u2"
        runCurrent()
        assertNull(port.promptOriginBotName(4uL))
    }

    // --- the Bots tab: what the manager keeps and tells ----------------------------------

    @Test fun theLastUsedComputerIsReadBeforeTheClientIsMade() = runTest(UnconfinedTestDispatcher()) {
        val observer = RecordingObserver().also { it.remembered["u1"] = "h-last" }
        val harness = Harness(this, "u1", observer) { observer.events += "build:$it" }
        runCurrent()
        // Known before any computer can be listed, so the first listing can already be compared with it.
        assertEquals(listOf("account:u1", "lastHostId:u1", "build:u1"), observer.events)
        assertEquals("h-last", harness.manager.lastHostId.value)
    }

    @Test fun eachAccountStartsWithItsOwnLastUsedComputer() = runTest(UnconfinedTestDispatcher()) {
        val observer = RecordingObserver().also { it.remembered["u1"] = "a"; it.remembered["u2"] = "b" }
        val harness = Harness(this, "u1", observer)
        runCurrent()
        assertEquals("a", harness.manager.lastHostId.value)
        harness.account.mutableUid.value = "u2"
        runCurrent()
        assertEquals("b", harness.manager.lastHostId.value)
        harness.account.mutableUid.value = null
        runCurrent()
        assertNull(harness.manager.lastHostId.value)
        assertEquals(
            listOf("account:u1", "lastHostId:u1", "account:u2", "lastHostId:u2", "account:null"),
            observer.events.filter { it.startsWith("account:") || it.startsWith("lastHostId:") },
        )
    }

    @Test fun noLastUsedComputerIsKnownBeforeTheFirstConnection() = runTest(UnconfinedTestDispatcher()) {
        val harness = Harness(this, "u1", RecordingObserver())
        runCurrent()
        assertNull(harness.manager.lastHostId.value)
    }

    @Test fun aReadyConnectionIsRememberedOncePerComputer() = runTest(UnconfinedTestDispatcher()) {
        val observer = RecordingObserver()
        val harness = Harness(this, "u1", observer)
        runCurrent()
        val listener = harness.clients.single().listener
        listener.onConnectionState(connectionState("h", RemoteConnectionPhaseFfi.CONNECTING))
        assertNull(harness.manager.lastHostId.value)
        listener.onConnectionState(connectionState("h", RemoteConnectionPhaseFfi.READY))
        assertEquals("h", harness.manager.lastHostId.value)
        listener.onConnectionState(connectionState("h", RemoteConnectionPhaseFfi.RECONNECTING))
        listener.onConnectionState(connectionState("h", RemoteConnectionPhaseFfi.READY))
        listener.onConnectionState(connectionState("h2", RemoteConnectionPhaseFfi.CONNECTING))
        assertEquals("h", harness.manager.lastHostId.value)
        listener.onConnectionState(connectionState("h2", RemoteConnectionPhaseFfi.READY))
        assertEquals("h2", harness.manager.lastHostId.value)
        assertEquals(listOf("connected:u1:h", "connected:u1:h2"), observer.events.filter { it.startsWith("connected:") })
    }

    @Test fun forgettingTheLastUsedComputerForgetsTheMarkAndTellsTheObserver() = runTest(UnconfinedTestDispatcher()) {
        val observer = RecordingObserver()
        val harness = Harness(this, "u1", observer)
        runCurrent()
        harness.clients.single().listener.onConnectionState(connectionState("h", RemoteConnectionPhaseFfi.READY))
        harness.manager.forget("h")
        assertNull(harness.manager.lastHostId.value)
        assertEquals(listOf("forgotten:u1:h"), observer.events.filter { it.startsWith("forgotten:") })
    }

    @Test fun forgettingAnotherComputerKeepsTheMark() = runTest(UnconfinedTestDispatcher()) {
        val observer = RecordingObserver()
        val harness = Harness(this, "u1", observer)
        runCurrent()
        harness.clients.single().listener.onConnectionState(connectionState("h", RemoteConnectionPhaseFfi.READY))
        harness.manager.forget("other")
        assertEquals("h", harness.manager.lastHostId.value)
        assertEquals(listOf("forgotten:u1:other"), observer.events.filter { it.startsWith("forgotten:") })
    }

    @Test fun aForgetThatFailsKeepsTheMarkAndTellsNobody() = runTest(UnconfinedTestDispatcher()) {
        val observer = RecordingObserver()
        val harness = Harness(this, "u1", observer)
        runCurrent()
        val client = harness.clients.single()
        client.listener.onConnectionState(connectionState("h", RemoteConnectionPhaseFfi.READY))
        client.forgetFailure = IllegalStateException("keychain")
        assertTrue(runCatching { harness.manager.forget("h") }.isFailure)
        assertEquals("h", harness.manager.lastHostId.value)
        assertTrue(observer.events.none { it.startsWith("forgotten:") })
    }

    @Test fun botsAndChatsAreToldToTheObserverWithTheirAccountAndComputer() = runTest(UnconfinedTestDispatcher()) {
        val observer = RecordingObserver()
        val harness = Harness(this, "u1", observer)
        runCurrent()
        val listener = harness.clients.single().boundBotsListener!!
        listener.onBots("h", RemoteBotsStateFfi.Bots(listOf(bot("b1"), bot("b2"))))
        listener.onBots("h", RemoteBotsStateFfi.Unsupported)
        listener.onBotThreads("h", "b1", RemoteBotThreadsStateFfi(listOf(chat("s1")), null))
        listener.onBotThreads("h", "b1", RemoteBotThreadsStateFfi(null, RemoteSessionErrorFfi("timeout", "slow")))
        assertEquals(
            listOf("bots:u1:h:2", "bots:u1:h:unsupported", "threads:u1:h:b1:1", "threads:u1:h:b1:failed"),
            observer.events.filter { it.startsWith("bots:") || it.startsWith("threads:") },
        )
    }

    @Test fun aReplacedClientsBotsNeverReachTheObserver() = runTest(UnconfinedTestDispatcher()) {
        val observer = RecordingObserver()
        val harness = Harness(this, "u1", observer)
        runCurrent()
        val stale = harness.clients.single().boundBotsListener!!
        val staleListener = harness.clients.single().listener
        harness.account.mutableUid.value = "u2"
        runCurrent()
        val before = observer.events.toList()
        stale.onBots("h", RemoteBotsStateFfi.Bots(listOf(bot("b1"))))
        stale.onBotThreads("h", "b1", RemoteBotThreadsStateFfi(emptyList(), null))
        staleListener.onConnectionState(connectionState("h", RemoteConnectionPhaseFfi.READY))
        assertEquals(before, observer.events)
        assertNull(harness.manager.lastHostId.value)
    }

    @Test fun backgroundDisconnectsAndForegroundReattachesAnOpenSession() = runTest {
        val harness = Harness(this, "A")
        runCurrent()
        val client = harness.clients.single()
        val following = backgroundScope.launch { harness.manager.followSession("host", "session").collect {} }
        runCurrent()
        assertTrue(client.calls.contains("attach:session"))
        client.calls.clear()
        harness.manager.background()
        runCurrent()
        assertTrue(client.calls.contains("disconnect"))
        client.calls.clear()
        harness.manager.foreground()
        runCurrent()
        assertEquals(listOf("connect:host", "attach:session", "foreground:session:true"), client.calls)
        following.cancel()
    }

    @Test fun foregroundInvalidatesABackgroundDisconnectStillQueuedForTheSameClient() = runTest {
        val harness = Harness(this, "A")
        runCurrent()
        val client = harness.clients.single()
        val following = backgroundScope.launch { harness.manager.followSession("host", "session").collect {} }
        runCurrent()
        client.calls.clear()
        harness.manager.background()
        harness.manager.foreground()
        runCurrent()
        assertFalse(client.calls.contains("disconnect"))
        assertTrue(client.calls.contains("attach:session"))
        following.cancel()
    }

    @Test fun aFailedForegroundAttachmentDoesNotEscapeTheProcessCoroutine() = runTest {
        val harness = Harness(this, "A")
        runCurrent()
        val client = harness.clients.single()
        val following = backgroundScope.launch { harness.manager.followSession("host", "session").collect {} }
        runCurrent()
        harness.manager.background()
        runCurrent()
        client.attachFailure = IllegalStateException("connection closed during resume")
        client.calls.clear()
        harness.manager.foreground()
        runCurrent()
        assertTrue(client.calls.contains("attach:session"))
        assertFalse(client.calls.contains("foreground:session:true"))
        following.cancel()
    }

    // --- the grace after leaving the Bots tab (section 4.4) -------------------------------------

    @Test fun leavingTheTabEndsTheConnectionAfterTheGraceAndNotBefore() = runTest(UnconfinedTestDispatcher()) {
        assertEquals(60_000L, RemoteControlManager.LEAVE_GRACE_MS)
        val harness = Harness(this, "u1")
        runCurrent()
        val client = harness.clients.single()
        harness.manager.scheduleCloseConnection()
        advanceTimeBy(RemoteControlManager.LEAVE_GRACE_MS - 1)
        runCurrent()
        assertEquals(emptyList<String>(), client.calls)
        advanceTimeBy(2)
        runCurrent()
        assertEquals(listOf("stopWatchingHosts", "disconnect"), client.calls)
    }

    @Test fun comingBackBeforeTheGraceCallsTheTeardownOff() = runTest(UnconfinedTestDispatcher()) {
        val harness = Harness(this, "u1")
        runCurrent()
        val client = harness.clients.single()
        harness.manager.scheduleCloseConnection()
        advanceTimeBy(30_000)
        harness.manager.cancelScheduledClose()
        advanceTimeBy(120_000)
        runCurrent()
        assertEquals(emptyList<String>(), client.calls)
    }

    @Test fun leavingAgainStartsTheWaitOver() = runTest(UnconfinedTestDispatcher()) {
        val harness = Harness(this, "u1")
        runCurrent()
        val client = harness.clients.single()
        harness.manager.scheduleCloseConnection()
        advanceTimeBy(40_000)
        harness.manager.scheduleCloseConnection()
        advanceTimeBy(40_000)
        runCurrent()
        assertEquals(emptyList<String>(), client.calls)
        advanceTimeBy(20_001)
        runCurrent()
        assertEquals(listOf("stopWatchingHosts", "disconnect"), client.calls)
    }

    @Test fun closingNowCallsOffAPendingTeardownAndClosesOnlyOnce() = runTest(UnconfinedTestDispatcher()) {
        val harness = Harness(this, "u1")
        runCurrent()
        val client = harness.clients.single()
        harness.manager.scheduleCloseConnection()
        harness.manager.closeConnection()
        runCurrent()
        assertEquals(listOf("stopWatchingHosts", "disconnect"), client.calls)
        advanceTimeBy(180_000)
        runCurrent()
        assertEquals(listOf("stopWatchingHosts", "disconnect"), client.calls)
    }

    @Test fun noGraceMeansClosingAtOnce() = runTest(UnconfinedTestDispatcher()) {
        val harness = Harness(this, "u1")
        runCurrent()
        harness.manager.scheduleCloseConnection(graceMs = 0)
        runCurrent()
        assertEquals(listOf("stopWatchingHosts", "disconnect"), harness.clients.single().calls)
    }

    @Test fun aTeardownScheduledForTheOldAccountNeverEndsTheNextOnesConnection() = runTest(UnconfinedTestDispatcher()) {
        val harness = Harness(this, "u1")
        runCurrent()
        harness.manager.scheduleCloseConnection()
        harness.account.mutableUid.value = "u2"
        runCurrent()
        advanceTimeBy(180_000)
        runCurrent()
        assertEquals(emptyList<String>(), harness.clients.last().calls)
    }

    @Test fun theConnectionOfAComputerOpenedAfterTheGraceWasScheduledStaysUntilTheNextLeave() = runTest(UnconfinedTestDispatcher()) {
        val harness = Harness(this, "u1")
        runCurrent()
        val client = harness.clients.single()
        harness.manager.scheduleCloseConnection()
        // The tab is entered again and a computer is dialled; the earlier wait must not close it.
        harness.manager.cancelScheduledClose()
        harness.manager.connect("h")
        advanceTimeBy(180_000)
        runCurrent()
        assertEquals(listOf("connect:h"), client.calls)
        harness.manager.scheduleCloseConnection()
        advanceTimeBy(60_001)
        runCurrent()
        assertEquals(listOf("connect:h", "stopWatchingHosts", "disconnect"), client.calls)
    }

    // --- the Bots tab's badge --------------------------------------------------------------------

    @Test fun theBadgeCountsTheChatsWaitingOnTheConnectedComputer() = runTest(UnconfinedTestDispatcher()) {
        val harness = Harness(this, "u1")
        runCurrent()
        val client = harness.clients.single()
        assertEquals(0, harness.manager.waitingChats.value)
        client.listener.onConnectionState(connectionState("h", RemoteConnectionPhaseFfi.READY))
        client.boundBotsListener!!.onBots("h", RemoteBotsStateFfi.Bots(listOf(bot("b1", 2u), bot("b2"), bot("b3", 3u))))
        runCurrent()
        assertEquals(5, harness.manager.waitingChats.value)
        client.boundBotsListener!!.onBots("h", RemoteBotsStateFfi.Bots(listOf(bot("b1", 1u))))
        runCurrent()
        assertEquals(1, harness.manager.waitingChats.value)
        // The link going away takes the count with it, whatever Bots are still held.
        client.listener.onConnectionState(connectionState("h", RemoteConnectionPhaseFfi.IDLE))
        runCurrent()
        assertEquals(0, harness.manager.waitingChats.value)
        client.listener.onConnectionState(connectionState("h", RemoteConnectionPhaseFfi.READY))
        runCurrent()
        assertEquals(1, harness.manager.waitingChats.value)
    }

    @Test fun theBadgeIgnoresAnotherComputersBotsAndAnOldDesktop() = runTest(UnconfinedTestDispatcher()) {
        val harness = Harness(this, "u1")
        runCurrent()
        val client = harness.clients.single()
        client.listener.onConnectionState(connectionState("h", RemoteConnectionPhaseFfi.READY))
        client.boundBotsListener!!.onBots("other", RemoteBotsStateFfi.Bots(listOf(bot("b1", 4u))))
        runCurrent()
        assertEquals(0, harness.manager.waitingChats.value)
        client.boundBotsListener!!.onBots("h", RemoteBotsStateFfi.Unsupported)
        runCurrent()
        assertEquals(0, harness.manager.waitingChats.value)
    }

    @Test fun theBadgeResetsWhenTheAccountChanges() = runTest(UnconfinedTestDispatcher()) {
        val harness = Harness(this, "u1")
        runCurrent()
        val client = harness.clients.single()
        client.listener.onConnectionState(connectionState("h", RemoteConnectionPhaseFfi.READY))
        client.boundBotsListener!!.onBots("h", RemoteBotsStateFfi.Bots(listOf(bot("b1", 2u))))
        runCurrent()
        assertEquals(2, harness.manager.waitingChats.value)
        harness.account.mutableUid.value = null
        runCurrent()
        assertEquals(0, harness.manager.waitingChats.value)
    }
}
