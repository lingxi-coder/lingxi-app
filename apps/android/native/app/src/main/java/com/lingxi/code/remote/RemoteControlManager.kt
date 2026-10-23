package com.lingxi.code.remote

import android.content.Context
import android.os.Build
import com.google.firebase.auth.FirebaseAuth
import com.lingxi.code.BuildConfig
import com.lingxi.code.bindings.android.AndroidRemoteControlClient
import com.lingxi.code.bindings.android.AndroidRemoteKeyStore
import com.lingxi.code.bindings.android.AndroidRemoteListener
import com.lingxi.code.bindings.android.AndroidRemoteTokenProvider
import com.lingxi.code.bindings.android.RemoteConnectionPhaseFfi
import com.lingxi.code.bindings.android.RemoteConnectionStateFfi
import com.lingxi.code.bindings.android.RemoteFfiException
import com.lingxi.code.bindings.android.RemoteHostFfi
import com.lingxi.code.bindings.android.RemotePermissionModeFfi
import com.lingxi.code.bindings.android.RemotePromptImageFfi
import com.lingxi.code.bindings.android.RemoteSessionSummaryFfi
import com.lingxi.code.bindings.android.RemoteSessionUpdateFfi
import com.lingxi.code.bindings.android.buildAndroidRemoteControlClient
import com.lingxi.code.bindings.client.ReasoningSelectionDto
import com.lingxi.code.conversation.RemoteSessionPort
import com.lingxi.code.push.PushRegistration
import java.util.concurrent.CopyOnWriteArrayList
import java.util.concurrent.ConcurrentHashMap
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.channels.awaitClose
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.buffer
import kotlinx.coroutines.flow.callbackFlow
import kotlinx.coroutines.flow.filterNotNull
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.flow.getAndUpdate
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch
import kotlinx.coroutines.tasks.await

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
        return checkNotNull(user.getIdToken(forceRefresh).await().token) { "no ID token" }
    }
}

/** What the app uses of the generated client; tests substitute it. */
interface RemoteClientPort {
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
    suspend fun dismissPrompt(sessionId: String, promptId: String)
    suspend fun cancelTurn(sessionId: String)
    suspend fun respondPermission(sessionId: String, requestId: ULong, allow: Boolean)
    suspend fun answerQuestion(sessionId: String, requestId: ULong, answers: Map<String, String>)
    suspend fun cancelQuestion(sessionId: String, requestId: ULong)
    suspend fun setModel(sessionId: String, model: String)
    suspend fun setReasoning(sessionId: String, selection: ReasoningSelectionDto)
    suspend fun setPermissionMode(sessionId: String, mode: com.lingxi.code.bindings.android.RemotePermissionModeFfi)
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
    private val mutableSessions = MutableStateFlow(RemoteSessionsState())
    private val mutableClient = MutableStateFlow<RemoteClientPort?>(null)
    private val sinks = ConcurrentHashMap<String, CopyOnWriteArrayList<Channel<RemoteSessionUpdateFfi>>>()
    private var generation = 0

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
        val mine = ++generation
        mutableClient.getAndUpdate { null }?.let { runCatching { it.close() } }
        mutableHosts.value = RemoteHostsState()
        mutableConnection.value = null
        mutableSessions.value = RemoteSessionsState()
        if (uid == null) return
        val listener = ManagerListener(mine)
        val client = try {
            factory.build(uid, listener)
        } catch (cancelled: CancellationException) {
            throw cancelled
        } catch (error: Exception) {
            mutableHosts.value = RemoteHostsState(error = error.message ?: error.javaClass.simpleName)
            return
        }
        if (mine != generation) {
            runCatching { client.close() }
            return
        }
        client.setPushInstallationId(pushInstallationId())
        mutableClient.value = client
    }

    private suspend fun client(): RemoteClientPort {
        mutableClient.value?.let { return it }
        // The account may have changed a moment ago; wait for the matching client.
        return mutableClient.filterNotNull().first()
    }

    /** Keep the list of computers current while the host list is shown. */
    suspend fun watchHosts() = client().watchHosts()

    suspend fun stopWatchingHosts() {
        mutableClient.value?.stopWatchingHosts()
    }

    /** Dial [hostId]; dialling the one already connected only nudges it. */
    suspend fun connect(hostId: String) = client().connect(hostId)

    /** Pair again after the computer removed this device. */
    suspend fun repair(hostId: String) = client().repairHost(hostId)

    suspend fun retry() = client().retry()

    suspend fun forget(hostId: String) {
        client().forgetHost(hostId)
        mutableHosts.update { state -> state.copy(hosts = state.hosts.map { if (it.hostId == hostId) it.copy(paired = false) else it }) }
    }

    /** The screens went away: end the connection and stop watching. */
    fun closeConnection() {
        scope.launch {
            val client = mutableClient.value ?: return@launch
            client.stopWatchingHosts()
            client.disconnect()
        }
    }

    /** The app is in front again or the network came back. */
    fun nudge() {
        scope.launch { mutableClient.value?.nudge() }
    }

    /**
     * Follow [sessionId] of [hostId] for as long as the flow is collected, dialling
     * the computer first. Updates are deltas and none may be lost, hence the
     * unlimited buffer.
     */
    fun followSession(hostId: String, sessionId: String): Flow<RemoteSessionUpdateFfi> = callbackFlow {
        val sink = Channel<RemoteSessionUpdateFfi>(Channel.UNLIMITED)
        sinks.getOrPut(sessionId) { CopyOnWriteArrayList() }.add(sink)
        val client = client()
        if (mutableConnection.value?.hostId != hostId || mutableConnection.value?.phase == RemoteConnectionPhaseFfi.IDLE) {
            client.connect(hostId)
        }
        client.attachSession(sessionId)
        val forwarding = launch { for (update in sink) send(update) }
        awaitClose {
            forwarding.cancel()
            sinks[sessionId]?.remove(sink)
            sink.close()
            scope.launch { runCatching { client.detachSession(sessionId) } }
        }
    }.buffer(Channel.UNLIMITED)

    /** The calls of one followed session. */
    fun sessionPort(hostId: String, sessionId: String): RemoteSessionPort = ManagerSessionPort(this, hostId, sessionId)

    internal suspend fun port(): RemoteClientPort = client()

    private inner class ManagerListener(private val mine: Int) : AndroidRemoteListener {
        override suspend fun onConnectionState(state: RemoteConnectionStateFfi) {
            if (mine != generation) return
            mutableConnection.value = state
            if (state.hostId != mutableSessions.value.hostId) mutableSessions.value = RemoteSessionsState(state.hostId)
        }

        override suspend fun onHosts(hosts: List<RemoteHostFfi>) {
            if (mine == generation) mutableHosts.value = RemoteHostsState(hosts, loaded = true)
        }

        override suspend fun onHostsFailed(message: String) {
            if (mine == generation) mutableHosts.update { it.copy(loaded = true, error = message) }
        }

        override suspend fun onSessions(hostId: String, sessions: List<RemoteSessionSummaryFfi>) {
            if (mine == generation) mutableSessions.value = RemoteSessionsState(hostId, sessions)
        }

        override suspend fun onSessionUpdate(update: RemoteSessionUpdateFfi) {
            if (mine != generation) return
            sinks[update.sessionId]?.forEach { it.trySend(update) }
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
) : RemoteSessionPort {
    override fun follow() = manager.followSession(hostId, sessionId)
    override suspend fun setForeground(foreground: Boolean) = manager.port().setSessionForeground(sessionId, foreground)
    override suspend fun loadOlder() = manager.port().loadOlderMessages(sessionId)
    override suspend fun fetchImage(imageRef: String) = manager.port().fetchImage(sessionId, imageRef)
    override suspend fun sendPrompt(text: String, images: List<RemotePromptImageFfi>) =
        manager.port().sendPrompt(sessionId, text, images)
    override suspend fun dismissPrompt(promptId: String) = manager.port().dismissPrompt(sessionId, promptId)
    override suspend fun cancelTurn() = manager.port().cancelTurn(sessionId)
    override suspend fun respondPermission(requestId: ULong, allow: Boolean) =
        manager.port().respondPermission(sessionId, requestId, allow)
    override suspend fun answerQuestion(requestId: ULong, answers: Map<String, String>) =
        manager.port().answerQuestion(sessionId, requestId, answers)
    override suspend fun cancelQuestion(requestId: ULong) = manager.port().cancelQuestion(sessionId, requestId)
    override suspend fun setModel(model: String) = manager.port().setModel(sessionId, model)
    override suspend fun setReasoning(selection: ReasoningSelectionDto) = manager.port().setReasoning(sessionId, selection)
    override suspend fun setPermissionMode(mode: RemotePermissionModeFfi) =
        manager.port().setPermissionMode(sessionId, mode)
    override suspend fun listModels() = manager.port().listModels()
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
        return RemoteControlManager(
            scope = scope,
            account = account,
            factory = FfiRemoteClientFactory(account, keys, deviceName()),
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
            ),
        )
}

/** [RemoteClientPort] over the generated [AndroidRemoteControlClient]. */
internal class FfiRemoteClientPort(private val client: AndroidRemoteControlClient) : RemoteClientPort {
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
    override suspend fun listModels() = client.listModels()
    override suspend fun close() {
        client.disconnect()
        client.stopWatchingHosts()
        client.close()
    }
}
