package com.lingxi.code.remote

import com.lingxi.code.bindings.android.RemoteFileFfi
import com.lingxi.code.bindings.android.RemoteFfiException
import com.lingxi.code.bindings.android.AndroidRemoteListener
import com.lingxi.code.bindings.android.RemoteConnectionPhaseFfi
import com.lingxi.code.bindings.android.RemoteConnectionStateFfi
import com.lingxi.code.bindings.android.RemoteHostFfi
import com.lingxi.code.bindings.android.RemoteLocalAppOpenFfi
import com.lingxi.code.bindings.android.RemoteLocalAppFfi
import com.lingxi.code.bindings.android.RemoteImageFfi
import com.lingxi.code.bindings.android.RemoteModelsFfi
import com.lingxi.code.bindings.android.RemotePermissionModeFfi
import com.lingxi.code.bindings.android.RemotePromptImageFfi
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
        override suspend fun forgetHost(hostId: String) { calls += "forget:$hostId"; forgetGate?.await() }
        override suspend fun repairHost(hostId: String) { calls += "repair:$hostId" }
        override suspend fun attachSession(sessionId: String) { calls += "attach:$sessionId" }
        override suspend fun detachSession(sessionId: String) { calls += "detach:$sessionId" }
        override suspend fun setSessionForeground(sessionId: String, foreground: Boolean) { calls += "foreground:$sessionId:$foreground" }
        override suspend fun loadOlderMessages(sessionId: String) { calls += "older:$sessionId" }
        override suspend fun fetchImage(sessionId: String, imageRef: String) = RemoteImageFfi("image/png", byteArrayOf())
        override suspend fun sendPrompt(sessionId: String, text: String, images: List<RemotePromptImageFfi>) { calls += "send:$sessionId:$text" }
        override suspend fun retryPrompt(sessionId: String, promptId: String) { calls += "retry:$sessionId:$promptId" }
        override suspend fun dismissPrompt(sessionId: String, promptId: String) { calls += "dismiss:$sessionId:$promptId" }
        override suspend fun cancelTurn(sessionId: String) { calls += "cancel:$sessionId" }
        override suspend fun respondPermission(sessionId: String, requestId: ULong, allow: Boolean) { calls += "permission:$sessionId:$requestId:$allow" }
        override suspend fun answerQuestion(sessionId: String, requestId: ULong, answers: Map<String, String>) {}
        override suspend fun cancelQuestion(sessionId: String, requestId: ULong) {}
        override suspend fun setModel(sessionId: String, model: String) {}
        override suspend fun setReasoning(sessionId: String, selection: ReasoningSelectionDto) {}
        override suspend fun setPermissionMode(sessionId: String, mode: RemotePermissionModeFfi) {}
        override suspend fun listModels() = RemoteModelsFfi(emptyList(), "", emptyList())
        override suspend fun close() { closed = true }
    }

    private class Harness(scope: TestScope, uid: String?) {
        val account = FakeAccount(uid)
        val clients = mutableListOf<FakeClient>()
        val manager = RemoteControlManager(
            scope = scope.backgroundScope,
            account = account,
            factory = { id, listener -> FakeClient(id, listener).also { clients += it } },
            pushInstallationId = { "push-1" },
        )
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
            withTimeout(5_000) { transition.join() }
            client.listGate!!.complete(emptyList())
            withTimeout(5_000) { pending.join() }
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

}
