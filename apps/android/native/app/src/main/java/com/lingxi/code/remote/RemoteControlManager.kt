package com.lingxi.code.remote

import com.lingxi.code.bindings.android.RemoteFileFfi
import android.content.Context
import android.os.Build
import com.google.firebase.auth.FirebaseAuth
import com.lingxi.code.BuildConfig
import com.lingxi.code.bindings.android.AndroidRemoteControlClient
import com.lingxi.code.bindings.android.AndroidRemoteKeyStore
import com.lingxi.code.bindings.android.AndroidRemoteListener
import com.lingxi.code.bindings.android.AndroidRemoteMediaConfigProvider
import com.lingxi.code.bindings.android.AndroidNativeMediaHost
import com.lingxi.code.bindings.android.RemoteMediaConfigRequestFfi
import com.lingxi.code.bindings.android.RemoteMediaConfigFfi
import com.lingxi.code.bindings.android.AndroidRemoteTokenProvider
import com.lingxi.code.bindings.android.RemoteConnectionPhaseFfi
import com.lingxi.code.bindings.android.RemoteConnectionStateFfi
import com.lingxi.code.bindings.android.RemoteFfiException
import com.lingxi.code.bindings.android.RemoteHostFfi
import com.lingxi.code.bindings.android.RemoteLocalAppFfi
import com.lingxi.code.bindings.android.RemoteLocalAppOpenFfi
import com.lingxi.code.bindings.android.RemotePermissionModeFfi
import com.lingxi.code.bindings.android.RemotePromptImageFfi
import com.lingxi.code.bindings.android.RemoteSessionSummaryFfi
import com.lingxi.code.bindings.android.RemoteSessionUpdateFfi
import com.lingxi.code.bindings.android.RemoteTransportFfi
import com.lingxi.code.bindings.android.buildAndroidRemoteControlClient
import com.lingxi.code.bindings.android.defaultRemoteMediaConfig
import com.lingxi.code.bindings.client.ReasoningSelectionDto
import com.lingxi.code.conversation.RemoteSessionPort
import com.lingxi.code.push.PushRegistration
import com.lingxi.code.voice.audio.AndroidNativeMediaHostProvider
import java.util.concurrent.CopyOnWriteArrayList
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.atomic.AtomicInteger
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.buffer
import kotlinx.coroutines.flow.channelFlow
import kotlinx.coroutines.flow.filterNotNull
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.flow.getAndUpdate
import kotlinx.coroutines.flow.combine
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch
import kotlinx.coroutines.withTimeoutOrNull
import kotlinx.coroutines.tasks.await
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock

/** The Firebase user Remote Control signs in as, and the ID tokens its requests carry. */
interface RemoteAccount {
    /** The signed-in user's uid, or null when signed out. */
    val uid: StateFlow<String?>

    suspend fun idToken(forceRefresh: Boolean): String
}

/** [RemoteAccount] over the app's existing Firebase Auth user (see `AccountViewModel`). */
class FirebaseRemoteAccount(private val auth: FirebaseAuth = FirebaseAuth.getInstance()) : RemoteAccount {
    private val mutableUid = MutableStateFlow(auth.currentUser?.uid)
    override val uid: StateFlow<String?> = mutableUid.asStateFlow()

    init {
        // The singleton lives as long as the process, like `PushRegistration`'s listener.
        auth.addAuthStateListener { mutableUid.value = it.currentUser?.uid }
    }

    override suspend fun idToken(forceRefresh: Boolean): String {
        val user = auth.currentUser ?: throw IllegalStateException("not signed in")
        if (BuildConfig.DEBUG) android.util.Log.d("RemoteControl", "Requesting account token; refresh=$forceRefresh")
        return try {
            checkNotNull(user.getIdToken(forceRefresh).await().token) { "no ID token" }.also {
                if (BuildConfig.DEBUG) android.util.Log.d("RemoteControl", "Account token ready")
            }
        } catch (error: Exception) {
            if (BuildConfig.DEBUG) android.util.Log.d("RemoteControl", "Account token failed: ${error.javaClass.simpleName}")
            throw error
        }
    }
}

/** What the app uses of the generated client; tests substitute it. */
interface RemoteClientPort {
    suspend fun prepareFile(sessionId: String, path: String, name: String): RemoteFileFfi
    suspend fun uploadFile(file: RemoteFileFfi, path: String): RemoteFileFfi
    suspend fun downloadFile(file: RemoteFileFfi, path: String): RemoteFileFfi
    suspend fun listFiles(sessionId: String): List<RemoteFileFfi>
    fun fileProgress(transferId: String): ULong
    fun cancelFile(transferId: String)
    suspend fun sendPromptWithFiles(sessionId: String, text: String, images: List<com.lingxi.code.bindings.android.RemotePromptImageFfi>, fileRefs: List<String>)

    fun setPushInstallationId(id: String?)
    suspend fun watchHosts()
    suspend fun stopWatchingHosts()
    suspend fun connect(hostId: String)
    suspend fun disconnect()
    suspend fun retry()
    suspend fun nudge()
    fun connectionState(): RemoteConnectionStateFfi?
    fun sessions(): List<RemoteSessionSummaryFfi>?
    suspend fun forgetHost(hostId: String)
    suspend fun repairHost(hostId: String)
    suspend fun attachSession(sessionId: String)
    suspend fun detachSession(sessionId: String)
    suspend fun setSessionForeground(sessionId: String, foreground: Boolean)
    suspend fun loadOlderMessages(sessionId: String)
    suspend fun fetchImage(sessionId: String, imageRef: String): com.lingxi.code.bindings.android.RemoteImageFfi
    suspend fun sendPrompt(sessionId: String, text: String, images: List<com.lingxi.code.bindings.android.RemotePromptImageFfi>)
    suspend fun retryPrompt(sessionId: String, promptId: String)
    suspend fun dismissPrompt(sessionId: String, promptId: String)
    suspend fun cancelTurn(sessionId: String)
    suspend fun respondPermission(sessionId: String, requestId: ULong, allow: Boolean)
    suspend fun answerQuestion(sessionId: String, requestId: ULong, answers: Map<String, String>)
    suspend fun cancelQuestion(sessionId: String, requestId: ULong)
    suspend fun setModel(sessionId: String, model: String)
    suspend fun setReasoning(sessionId: String, selection: ReasoningSelectionDto)
    suspend fun setPermissionMode(sessionId: String, mode: com.lingxi.code.bindings.android.RemotePermissionModeFfi)
    suspend fun listApps(): List<RemoteLocalAppFfi> = throw UnsupportedOperationException()
    suspend fun openApp(appId: String): RemoteLocalAppOpenFfi = throw UnsupportedOperationException()
    suspend fun listModels(): com.lingxi.code.bindings.android.RemoteModelsFfi
    suspend fun close()
}

/** The computers of the user as the host list shows them. */
data class RemoteHostsState(
    val hosts: List<RemoteHostFfi> = emptyList(),
    val loaded: Boolean = false,
    val error: String? = null,
)

/** The current computer's sessions; [hostId] says whose they are. */
data class RemoteSessionsState(
    val hostId: String? = null,
    val sessions: List<RemoteSessionSummaryFfi>? = null,
)

/** How a [RemoteClientPort] is made for one user; the production one calls the FFI. */
fun interface RemoteClientFactory {
    suspend fun build(uid: String, listener: AndroidRemoteListener): RemoteClientPort
}

/**
 * The one Remote Control client of the process: it is made for the signed-in
 * user, remade when the account changes, and turns the client's callbacks into
 * state the screens collect. The client runs nothing until a screen asks for a
 * computer; [closeConnection] ends it when the screens go away.
 */
class RemoteControlManager(
    private val scope: CoroutineScope,
    private val account: RemoteAccount,
    private val factory: RemoteClientFactory,
    private val pushInstallationId: () -> String?,
) {
    private val mutableHosts = MutableStateFlow(RemoteHostsState())
    private val mutableConnection = MutableStateFlow<RemoteConnectionStateFfi?>(null)
    private val connectionEpoch = AtomicInteger(0)
    private val appRequestLock = Any()
    private class ForgetBarrier(val hostId: String, val client: RemoteClientPort, var state: RemoteConnectionStateFfi?) {
        var pending = 0
        var succeeded = false
        var departed = state?.hostId != hostId || state?.phase == RemoteConnectionPhaseFfi.IDLE
    }
    private val forgetBarriers = mutableListOf<ForgetBarrier>()
    private val mutableSessions = MutableStateFlow(RemoteSessionsState())
    private data class ClientBinding(val uid: String, val client: RemoteClientPort)
    private data class SessionKey(val uid: String, val hostId: String, val sessionId: String)
    private val mutableClient = MutableStateFlow<ClientBinding?>(null)
    private val sinks = ConcurrentHashMap<SessionKey, CopyOnWriteArrayList<Channel<RemoteSessionUpdateFfi>>>()
    private val generation = AtomicInteger()
    private val screenGeneration = AtomicInteger()
    private val sessionLifecycle = Mutex()

    val hosts: StateFlow<RemoteHostsState> = mutableHosts.asStateFlow()
    val connection: StateFlow<RemoteConnectionStateFfi?> = mutableConnection.asStateFlow()
    val sessions: StateFlow<RemoteSessionsState> = mutableSessions.asStateFlow()

    /** The signed-in uid, or null when the screens must ask the user to sign in. */
    val uid: StateFlow<String?> get() = account.uid

    init {
        scope.launch {
            var previous: String? = null
            account.uid.collect { uid ->
                if (uid != previous) {
                    previous = uid
                    rebuild(uid)
                }
            }
        }
    }

    private suspend fun rebuild(uid: String?) {
        val mine = generation.incrementAndGet()
        synchronized(appRequestLock) { forgetBarriers.clear(); connectionEpoch.incrementAndGet() }
        val previous = mutableClient.getAndUpdate { null }
        sinks.values.forEach { channels -> channels.forEach { it.close() } }
        sinks.clear()
        mutableHosts.value = RemoteHostsState()
        mutableConnection.value = null
        mutableSessions.value = RemoteSessionsState()
        previous?.let { runCatching { it.client.close() } }
        if (uid == null) return
        val listener = ManagerListener(mine, uid)
        val client = try {
            factory.build(uid, listener)
        } catch (cancelled: CancellationException) {
            throw cancelled
        } catch (error: Exception) {
            if (account.uid.value == uid && mine == generation.get()) {
                mutableHosts.value = RemoteHostsState(error = error.message ?: error.javaClass.simpleName)
            }
            return
        }
        if (mine != generation.get() || account.uid.value != uid) {
            runCatching { client.close() }
            return
        }
        client.setPushInstallationId(pushInstallationId())
        mutableClient.value = ClientBinding(uid, client)
    }

    private suspend fun client(expectedUid: String? = account.uid.value): RemoteClientPort {
        val owner = checkNotNull(expectedUid) { "not signed in" }
        // A suspended caller must never resume with a different user's client.
        return combine(mutableClient, account.uid) { binding, uid ->
            if (uid != owner) throw CancellationException("remote account changed")
            binding?.takeIf { it.uid == owner }
        }.filterNotNull().first().client
    }

    /** Keep the list of computers current while the host list is shown. */
    suspend fun watchHosts() {
        screenGeneration.incrementAndGet()
        sessionLifecycle.withLock { client().watchHosts() }
    }

    suspend fun stopWatchingHosts() {
        mutableClient.value?.client?.stopWatchingHosts()
    }

    /** Dial [hostId]; dialling the one already connected only nudges it. */
    suspend fun connect(hostId: String) {
        connectionEpoch.incrementAndGet()
        screenGeneration.incrementAndGet()
        sessionLifecycle.withLock { client().connect(hostId) }
    }

    /** Pair again after the computer removed this device. */
    suspend fun repair(hostId: String) = client().repairHost(hostId)

    suspend fun retry() = client().retry()

    suspend fun forget(hostId: String) {
        val owner = checkNotNull(account.uid.value) { "not signed in" }
        // Capture the account before waiting for its client; a later account
        // must never receive this operation or its completion.
        val bound = mutableClient.value?.takeIf { it.uid == owner }?.client ?: client(owner)
        // Block new app requests before the FFI call can suspend. Forgetting a
        // different computer must not invalidate work on the current one.
        val barrier = synchronized(appRequestLock) {
            if (account.uid.value != owner || mutableClient.value?.client !== bound) throw CancellationException("remote account changed")
            val state = bound.connectionState()
            val current = forgetBarriers.firstOrNull { it.hostId == hostId && it.client === bound }
                ?: ForgetBarrier(hostId, bound, state).also { forgetBarriers.add(it) }
            if (current.departed && state?.hostId == hostId && state.phase != RemoteConnectionPhaseFfi.IDLE) {
                current.state = state
                current.departed = false
            }
            current.pending++
            if (state?.hostId == hostId) connectionEpoch.incrementAndGet()
            current
        }
        var succeeded = false
        try {
            bound.forgetHost(hostId)
            succeeded = true
            mutableHosts.update { state ->
                if (account.uid.value == owner && mutableClient.value?.client === bound) {
                    state.copy(hosts = state.hosts.map { if (it.hostId == hostId) it.copy(paired = false) else it })
                } else state
            }
        } finally {
            synchronized(appRequestLock) {
                if (forgetBarriers.contains(barrier)) {
                    barrier.pending--
                    barrier.succeeded = barrier.succeeded || succeeded
                    // On failure, allow a retry when no other forget remains.
                    // A successful forget stays blocked until the old connection departs.
                    if (barrier.pending == 0 && (!barrier.succeeded || barrier.departed)) forgetBarriers.remove(barrier)
                }
            }
        }
    }

    /** The screens went away: end the connection and stop watching. */
    fun closeConnection() {
        connectionEpoch.incrementAndGet()
        val mine = screenGeneration.incrementAndGet()
        val binding = mutableClient.value ?: return
        scope.launch {
            sessionLifecycle.withLock {
                if (mine != screenGeneration.get() || mutableClient.value !== binding) return@withLock
                binding.client.stopWatchingHosts()
                // A new screen may have requested this client while cleanup
                // suspended. Its connect/watch runs after this lock is released.
                if (mine == screenGeneration.get() && mutableClient.value === binding) binding.client.disconnect()
            }
        }
    }

    /** The app is in front again or the network came back. */
    fun nudge() {
        scope.launch { mutableClient.value?.client?.nudge() }
    }

    /**
     * Follow [sessionId] of [hostId] for as long as the flow is collected, dialling
     * the computer first. Updates are deltas and none may be lost, hence the
     * unlimited buffer.
     */
    fun followSession(hostId: String, sessionId: String): Flow<RemoteSessionUpdateFfi> =
        followSession(hostId, sessionId, account.uid.value)

    internal fun followSession(hostId: String, sessionId: String, ownerUid: String?): Flow<RemoteSessionUpdateFfi> = channelFlow {
        screenGeneration.incrementAndGet()
        val key = SessionKey(checkNotNull(ownerUid) { "not signed in" }, hostId, sessionId)
        val sink = Channel<RemoteSessionUpdateFfi>(Channel.UNLIMITED)
        val channels = sinks.getOrPut(key) { CopyOnWriteArrayList() }
        channels.add(sink)
        var bound: RemoteClientPort? = null
        try {
            sessionLifecycle.withLock {
                val client = client(ownerUid)
                bound = client
                val state = client.connectionState() ?: mutableConnection.value
                if (state?.hostId != hostId || state.phase == RemoteConnectionPhaseFfi.IDLE) {
                    client.connect(hostId)
                }
                client.attachSession(sessionId)
            }
            for (update in sink) send(update)
        } finally {
            channels.remove(sink)
            sink.close()
            scope.launch {
                sessionLifecycle.withLock {
                    // A replacement collector may already own this attachment.
                    if (sinks[key]?.isNotEmpty() != true) {
                        sinks.remove(key, channels)
                        bound?.let { client ->
                            if (client.connectionState()?.hostId?.let { it == hostId } != false) {
                                runCatching { client.detachSession(sessionId) }
                            }
                        }
                    }
                }
            }
        }
    }.buffer(Channel.UNLIMITED)

    /** The calls of one followed session. */
    fun sessionPort(hostId: String, sessionId: String): RemoteSessionPort = ManagerSessionPort(this, hostId, sessionId, account.uid.value)

    internal suspend fun port(ownerUid: String?, hostId: String): RemoteClientPort {
        val client = client(ownerUid)
        val activeHost = client.connectionState()?.hostId ?: mutableConnection.value?.hostId
        check(activeHost == null || activeHost == hostId) { "remote computer changed" }
        return client
    }

    /** Bind each request to the account, host, and connection attempt that started it. */
    private suspend fun <T> appRequest(hostId: String, action: suspend (RemoteClientPort) -> T): T {
        val owner = checkNotNull(account.uid.value) { "not signed in" }
        val bound = port(owner, hostId)
        val (state, epoch) = synchronized(appRequestLock) {
            check(account.uid.value == owner && mutableClient.value?.client === bound) { "remote account changed" }
            val current = checkNotNull(bound.connectionState()) { "remote computer disconnected" }
            check(current.hostId == hostId && current.phase == RemoteConnectionPhaseFfi.READY) { "remote computer disconnected" }
            check(forgetBarriers.none { it.hostId == hostId && it.client === bound }) { "remote computer is being forgotten" }
            current to connectionEpoch.get()
        }
        val result = action(bound)
        synchronized(appRequestLock) {
            val current = bound.connectionState()
            check(account.uid.value == owner && mutableClient.value?.client === bound && connectionEpoch.get() == epoch &&
                forgetBarriers.none { it.hostId == hostId && it.client === bound } &&
                current?.hostId == hostId && current.phase == RemoteConnectionPhaseFfi.READY && current.attempt == state.attempt) {
                "remote computer or account changed"
            }
        }
        return result
    }

    suspend fun listApps(hostId: String): List<RemoteLocalAppFfi> = appRequest(hostId) { it.listApps() }

    suspend fun openApp(hostId: String, appId: String, revision: ULong): String = appRequest(hostId) { client ->
        val result = client.openApp(appId)
        val uri = runCatching { java.net.URI(result.url) }.getOrNull()
        check(result.appId == appId && result.revision == revision && result.expiresAt > System.currentTimeMillis().toULong() &&
            uri?.scheme.equals("https", ignoreCase = true) && !uri?.host.isNullOrBlank() && uri?.userInfo == null && !uri?.rawFragment.isNullOrBlank()) {
            "Invalid or expired application link"
        }
        result.url
    }

    /** A system document picker can outlive the peer. Wait for the existing
     * account/host to reconnect before starting file RPCs on return. */
    internal suspend fun filePort(ownerUid: String?, hostId: String, sessionReady: Flow<Boolean>): RemoteClientPort {
        return checkNotNull(withTimeoutOrNull(30_000) {
            val bound = port(ownerUid, hostId)
            bound.nudge()
            combine(mutableClient, mutableConnection, account.uid, sessionReady) { binding, _, uid, attached ->
                if (uid != ownerUid || binding?.client !== bound) throw CancellationException("remote account changed")
                val state = bound.connectionState()
                check(state?.hostId == null || state.hostId == hostId) { "remote computer changed" }
                if (state?.phase == RemoteConnectionPhaseFfi.READY) {
                    check(state.transport == RemoteTransportFfi.WEBRTC) { "File transfer requires a WebRTC connection" }
                    bound.takeIf { attached }
                } else null
            }.filterNotNull().first()
        }) { "Remote computer is still reconnecting; retry when connected" }
    }

    /** A peer can close just after readiness was observed. Original-file RPCs
     * retain their transfer identity/checkpoint, so retry them once on the same
     * authorized session after a transport closure. Never retry prompt sends. */
    internal suspend fun <T> fileOperation(
        ownerUid: String?, hostId: String, sessionReady: Flow<Boolean>,
        action: suspend (RemoteClientPort) -> T,
    ): T {
        val bound = filePort(ownerUid, hostId, sessionReady)
        val attempt = bound.connectionState()?.attempt
        try {
            return action(bound)
        } catch (error: RemoteFfiException) {
            if (!closedFileTransport(error)) throw error
        }
        // A file-only closure does not restart the healthy control peer. Wait
        // for evidence of an actual peer transition, never reuse its dead lane.
        val resumed = withTimeoutOrNull(30_000) {
            combine(mutableClient, mutableConnection, account.uid) { binding, _, uid ->
                if (uid != ownerUid || binding?.client !== bound) throw CancellationException("remote account changed")
                val state = bound.connectionState()
                check(state?.hostId == null || state.hostId == hostId) { "remote computer changed" }
                state?.takeIf { it.phase != RemoteConnectionPhaseFfi.READY || it.attempt != attempt }
            }.filterNotNull().first()
            filePort(ownerUid, hostId, sessionReady)
        } ?: throw IllegalStateException("File connection closed. Exit Remote Control and reconnect, then retry.")
        if (resumed !== bound) throw CancellationException("remote account changed")
        try {
            return action(resumed)
        } catch (error: RemoteFfiException) {
            if (closedFileTransport(error)) throw IllegalStateException("File connection closed. Exit Remote Control and reconnect, then retry.", error)
            throw error
        }
    }

    private fun closedFileTransport(error: RemoteFfiException): Boolean = when (error) {
        is RemoteFfiException.NotConnected -> true
        is RemoteFfiException.Rpc -> error.code == "closed"
        is RemoteFfiException.InvalidArgument -> error.message == "file channel closed" || error.message == "not connected" || error.message?.startsWith("closed:") == true
        else -> false
    }

    private inner class ManagerListener(private val mine: Int, private val ownerUid: String) : AndroidRemoteListener {
        override suspend fun onConnectionState(state: RemoteConnectionStateFfi) {
            if (mine != generation.get() || account.uid.value != ownerUid) return
            synchronized(appRequestLock) {
                forgetBarriers.forEach { barrier ->
                    if (barrier.hostId != state.hostId || barrier.state?.attempt != state.attempt ||
                        state.phase == RemoteConnectionPhaseFfi.IDLE ||
                        (barrier.state?.phase == RemoteConnectionPhaseFfi.READY && state.phase != RemoteConnectionPhaseFfi.READY)) {
                        barrier.departed = true
                    }
                }
                forgetBarriers.removeAll { it.pending == 0 && it.succeeded && it.departed }
                if (mutableConnection.value?.let { it.hostId != state.hostId || it.phase != state.phase || it.attempt != state.attempt } != false) connectionEpoch.incrementAndGet()
                mutableConnection.value = state
            }
            if (state.hostId != mutableSessions.value.hostId) mutableSessions.value = RemoteSessionsState(state.hostId)
        }

        override suspend fun onHosts(hosts: List<RemoteHostFfi>) {
            if (mine == generation.get() && account.uid.value == ownerUid) mutableHosts.value = RemoteHostsState(hosts, loaded = true)
        }

        override suspend fun onHostsFailed(message: String) {
            if (mine == generation.get() && account.uid.value == ownerUid) mutableHosts.update { it.copy(loaded = true, error = message) }
        }

        override suspend fun onSessions(hostId: String, sessions: List<RemoteSessionSummaryFfi>) {
            if (mine == generation.get() && account.uid.value == ownerUid) mutableSessions.value = RemoteSessionsState(hostId, sessions)
        }

        override suspend fun onSessionUpdate(update: RemoteSessionUpdateFfi) {
            if (mine != generation.get() || account.uid.value != ownerUid) return
            val hostId = mutableClient.value?.client?.connectionState()?.hostId
                ?: mutableConnection.value?.hostId ?: return
            sinks[SessionKey(ownerUid, hostId, update.sessionId)]?.forEach { it.trySend(update) }
        }
    }

    companion object {
        /** The default Realtime Database instance of the LingXi Firebase project. */
        const val DATABASE_URL = "https://lingxi-5e4b2-default-rtdb.firebaseio.com"
    }
}

private class ManagerSessionPort(
    private val manager: RemoteControlManager,
    private val hostId: String,
    private val sessionId: String,
    private val ownerUid: String?,
) : RemoteSessionPort {
    private suspend fun client() = manager.port(ownerUid, hostId)
    override fun follow() = manager.followSession(hostId, sessionId, ownerUid)
    override suspend fun setForeground(foreground: Boolean) = client().setSessionForeground(sessionId, foreground)
    override suspend fun loadOlder() = client().loadOlderMessages(sessionId)
    override suspend fun fetchImage(imageRef: String) = client().fetchImage(sessionId, imageRef)
    override suspend fun sendPrompt(text: String, images: List<RemotePromptImageFfi>) =
        client().sendPrompt(sessionId, text, images)
    override suspend fun retryPrompt(promptId: String) = client().retryPrompt(sessionId, promptId)
    override suspend fun dismissPrompt(promptId: String) = client().dismissPrompt(sessionId, promptId)
    override suspend fun cancelTurn() = client().cancelTurn(sessionId)
    override suspend fun respondPermission(requestId: ULong, allow: Boolean) =
        client().respondPermission(sessionId, requestId, allow)
    override suspend fun answerQuestion(requestId: ULong, answers: Map<String, String>) =
        client().answerQuestion(sessionId, requestId, answers)
    override suspend fun cancelQuestion(requestId: ULong) = client().cancelQuestion(sessionId, requestId)
    override suspend fun setModel(model: String) = client().setModel(sessionId, model)
    override suspend fun setReasoning(selection: ReasoningSelectionDto) = client().setReasoning(sessionId, selection)
    override suspend fun setPermissionMode(mode: RemotePermissionModeFfi) =
        client().setPermissionMode(sessionId, mode)
    override suspend fun listModels() = client().listModels()
}

/** The production wiring: Firebase Auth, the Keystore-sealed key store, the generated client. */
object RemoteControl {
    @Volatile private var instance: RemoteControlManager? = null

    fun manager(context: Context): RemoteControlManager = instance ?: synchronized(this) {
        instance ?: create(context.applicationContext).also { instance = it }
    }

    private fun create(context: Context): RemoteControlManager {
        val scope = CoroutineScope(SupervisorJob() + Dispatchers.Default)
        val account = FirebaseRemoteAccount()
        val keys = DataStoreRemoteKeyStore.create(context, scope)
        val mediaHost = AndroidNativeMediaHostProvider.get(context)
        return RemoteControlManager(
            scope = scope,
            account = account,
            factory = FfiRemoteClientFactory(
                account,
                keys,
                deviceName(),
                mediaHost,
                AppRemoteMediaConfigProvider,
            ),
            pushInstallationId = { runCatching { PushRegistration.identity(context).id }.getOrNull() },
        )
    }

    /** Shown next to the pairing code on the computer. */
    internal fun deviceName(): String =
        listOf(Build.MANUFACTURER, Build.MODEL).filter { !it.isNullOrBlank() }.joinToString(" ").ifBlank { "Android" }
}

private class FfiTokenProvider(private val account: RemoteAccount) : AndroidRemoteTokenProvider {
    override suspend fun idToken(forceRefresh: Boolean): String = try {
        account.idToken(forceRefresh)
    } catch (cancelled: CancellationException) {
        throw cancelled
    } catch (error: Exception) {
        throw RemoteFfiException.Token(error.message ?: error.javaClass.simpleName)
    }
}

internal class FfiRemoteClientFactory(
    private val account: RemoteAccount,
    private val keys: AndroidRemoteKeyStore,
    private val deviceName: String,
    private val mediaHost: AndroidNativeMediaHost,
    private val mediaConfigProvider: AndroidRemoteMediaConfigProvider,
) : RemoteClientFactory {
    override suspend fun build(uid: String, listener: AndroidRemoteListener): RemoteClientPort =
        FfiRemoteClientPort(
            buildAndroidRemoteControlClient(
                databaseUrl = RemoteControlManager.DATABASE_URL,
                uid = uid,
                deviceName = deviceName,
                appVersion = BuildConfig.VERSION_NAME,
                tokens = FfiTokenProvider(account),
                keys = keys,
                listener = listener,
                mediaHost = mediaHost,
                mediaConfigProvider = mediaConfigProvider,
            ),
        )
}

/** Android currently uses the product's public STUN defaults for media routes. */
private object AppRemoteMediaConfigProvider : AndroidRemoteMediaConfigProvider {
    override suspend fun resolve(request: RemoteMediaConfigRequestFfi): RemoteMediaConfigFfi =
        defaultRemoteMediaConfig()
}

/** [RemoteClientPort] over the generated [AndroidRemoteControlClient]. */
internal class FfiRemoteClientPort(private val client: AndroidRemoteControlClient) : RemoteClientPort {
    override suspend fun prepareFile(sessionId: String, path: String, name: String) = client.prepareFile(sessionId, path, name)
    override suspend fun uploadFile(file: RemoteFileFfi, path: String) = client.uploadFile(file, path)
    override suspend fun downloadFile(file: RemoteFileFfi, path: String) = client.downloadFile(file, path)
    override suspend fun listFiles(sessionId: String) = client.listFiles(sessionId)
    override fun fileProgress(transferId: String) = client.fileProgress(transferId)
    override fun cancelFile(transferId: String) = client.cancelFile(transferId)
    override suspend fun sendPromptWithFiles(sessionId: String, text: String, images: List<com.lingxi.code.bindings.android.RemotePromptImageFfi>, fileRefs: List<String>) = client.sendPromptWithFiles(sessionId, text, images, fileRefs)

    override fun setPushInstallationId(id: String?) = client.setPushInstallationId(id)
    override suspend fun watchHosts() = client.watchHosts()
    override suspend fun stopWatchingHosts() = client.stopWatchingHosts()
    override suspend fun connect(hostId: String) = client.connect(hostId)
    override suspend fun disconnect() = client.disconnect()
    override suspend fun retry() = client.retry()
    override suspend fun nudge() = client.nudge()
    override fun connectionState() = client.connectionState()
    override fun sessions() = client.sessions()
    override suspend fun forgetHost(hostId: String) = client.forgetHost(hostId)
    override suspend fun repairHost(hostId: String) = client.repairHost(hostId)
    override suspend fun attachSession(sessionId: String) = client.attachSession(sessionId)
    override suspend fun detachSession(sessionId: String) = client.detachSession(sessionId)
    override suspend fun setSessionForeground(sessionId: String, foreground: Boolean) =
        client.setSessionForeground(sessionId, foreground)
    override suspend fun loadOlderMessages(sessionId: String) = client.loadOlderMessages(sessionId)
    override suspend fun fetchImage(sessionId: String, imageRef: String) = client.fetchImage(sessionId, imageRef)
    override suspend fun sendPrompt(
        sessionId: String,
        text: String,
        images: List<com.lingxi.code.bindings.android.RemotePromptImageFfi>,
    ) = client.sendPrompt(sessionId, text, images)
    override suspend fun retryPrompt(sessionId: String, promptId: String) = client.retryPrompt(sessionId, promptId)
    override suspend fun dismissPrompt(sessionId: String, promptId: String) = client.dismissPrompt(sessionId, promptId)
    override suspend fun cancelTurn(sessionId: String) = client.cancelTurn(sessionId)
    override suspend fun respondPermission(sessionId: String, requestId: ULong, allow: Boolean) =
        client.respondPermission(sessionId, requestId, allow)
    override suspend fun answerQuestion(sessionId: String, requestId: ULong, answers: Map<String, String>) =
        client.answerQuestion(sessionId, requestId, answers)
    override suspend fun cancelQuestion(sessionId: String, requestId: ULong) = client.cancelQuestion(sessionId, requestId)
    override suspend fun setModel(sessionId: String, model: String) = client.setModel(sessionId, model)
    override suspend fun setReasoning(sessionId: String, selection: ReasoningSelectionDto) =
        client.setReasoning(sessionId, selection)
    override suspend fun setPermissionMode(
        sessionId: String,
        mode: com.lingxi.code.bindings.android.RemotePermissionModeFfi,
    ) = client.setPermissionMode(sessionId, mode)
    override suspend fun listApps() = client.listApps()
    override suspend fun openApp(appId: String) = client.openApp(appId)
    override suspend fun listModels() = client.listModels()
    override suspend fun close() {
        client.disconnect()
        client.stopWatchingHosts()
        client.close()
    }
}
