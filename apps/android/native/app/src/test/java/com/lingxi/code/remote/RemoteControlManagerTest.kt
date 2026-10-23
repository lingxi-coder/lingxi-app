package com.lingxi.code.remote

import com.lingxi.code.bindings.android.RemoteFileFfi
import com.lingxi.code.bindings.android.AndroidRemoteListener
import com.lingxi.code.bindings.android.RemoteConnectionPhaseFfi
import com.lingxi.code.bindings.android.RemoteConnectionStateFfi
import com.lingxi.code.bindings.android.RemoteHostFfi
import com.lingxi.code.bindings.android.RemoteImageFfi
import com.lingxi.code.bindings.android.RemoteModelsFfi
import com.lingxi.code.bindings.android.RemotePermissionModeFfi
import com.lingxi.code.bindings.android.RemotePromptImageFfi
import com.lingxi.code.bindings.android.RemoteSessionSummaryFfi
import com.lingxi.code.bindings.android.RemoteSessionUpdateFfi
import com.lingxi.code.bindings.client.ReasoningSelectionDto
import com.lingxi.code.conversation.remoteUpdate
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.launch
import kotlinx.coroutines.test.TestScope
import kotlinx.coroutines.test.UnconfinedTestDispatcher
import kotlinx.coroutines.test.runCurrent
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
        var state: RemoteConnectionStateFfi? = null
        override fun setPushInstallationId(id: String?) { pushId = id }
        override suspend fun watchHosts() { calls += "watchHosts" }
        override suspend fun stopWatchingHosts() { calls += "stopWatchingHosts" }
        override suspend fun connect(hostId: String) {
            calls += "connect:$hostId"
            state = RemoteConnectionStateFfi(hostId, RemoteConnectionPhaseFfi.READY, null, null, null, 0u, null, null, false)
        }
        override suspend fun disconnect() { calls += "disconnect" }
        override suspend fun retry() { calls += "retry" }
        override suspend fun nudge() { calls += "nudge" }
        override fun connectionState(): RemoteConnectionStateFfi? = state
        override fun sessions(): List<RemoteSessionSummaryFfi>? = null
        override suspend fun forgetHost(hostId: String) { calls += "forget:$hostId" }
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
