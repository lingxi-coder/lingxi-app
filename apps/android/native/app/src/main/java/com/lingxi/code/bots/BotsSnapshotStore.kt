package com.lingxi.code.bots

import android.content.Context
import androidx.datastore.core.DataStore
import androidx.datastore.preferences.core.PreferenceDataStoreFactory
import androidx.datastore.preferences.core.Preferences
import androidx.datastore.preferences.core.edit
import androidx.datastore.preferences.core.stringPreferencesKey
import androidx.datastore.preferences.core.stringSetPreferencesKey
import androidx.datastore.preferences.preferencesDataStoreFile
import com.lingxi.code.bindings.android.RemoteBotStatusFfi
import com.lingxi.code.bindings.android.RemoteBotThreadsStateFfi
import com.lingxi.code.bindings.android.RemoteBotsStateFfi
import com.lingxi.code.bindings.android.RemotePendingActionFfi
import com.lingxi.code.bindings.android.RemoteSessionStatusFfi
import com.lingxi.code.remote.RemoteBotsObserver
import java.security.MessageDigest
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.distinctUntilChanged
import kotlinx.coroutines.flow.emitAll
import kotlinx.coroutines.flow.filterNotNull
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.flow.flow
import kotlinx.coroutines.flow.map
import kotlinx.coroutines.launch
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import org.json.JSONArray
import org.json.JSONObject

/*
 * What the Bots tab remembers on the phone (SPEC-mobile-team-bot-ui.md sections 4.2 and 4.7), in one Preferences
 * DataStore (`bots.preferences_pb`):
 *
 *   bots.snap.<uid hash>.<hostId>   one JSON snapshot per computer: its Bots and their chats, as last delivered
 *   bots.collapsedHosts.<uid>       the computers whose section is collapsed
 *   bots.expandedBots.<uid>         the Bots that are open, as `hostId/botId` (absent = the default)
 *   bots.lastHost.<uid>             the computer connected to last
 *
 * Everything is scoped to the signed-in user and removed with it: [BotsSnapshotStore.retainOnly] and
 * [BotsPrefsStore.retainOnly] drop what belongs to anybody else, and run whenever the account changes.
 */

private const val SNAPSHOT_PREFIX = "bots.snap."
private const val COLLAPSED_PREFIX = "bots.collapsedHosts."
private const val EXPANDED_PREFIX = "bots.expandedBots."
private const val LAST_HOST_PREFIX = "bots.lastHost."

/** The preference key of one snapshot: the user is hashed so that the name carries no account id. */
internal fun snapshotKeyName(uid: String, hostId: String): String = SNAPSHOT_PREFIX + uidHash(uid) + "." + hostId

internal fun uidHash(uid: String): String =
    MessageDigest.getInstance("SHA-256").digest(uid.toByteArray(Charsets.UTF_8)).take(8).joinToString("") { "%02x".format(it) }

// --- the stored form -------------------------------------------------------------------------------------------------

/** A snapshot as JSON text, and back. Text that does not read as a snapshot reads as none. */
internal object BotsSnapshotCodec {
    private const val VERSION = 1

    fun encode(snapshot: BotsSnapshot): String = JSONObject()
        .put("v", VERSION)
        .put("uid", snapshot.uid)
        .put("hostId", snapshot.hostId)
        .put("savedAtMs", snapshot.savedAtMs)
        .put("bots", JSONArray().also { bots -> snapshot.bots.forEach { bots.put(encode(it)) } })
        .toString()

    private fun encode(bot: BotItem) = JSONObject()
        .put("id", bot.botId)
        .put("name", bot.name)
        .put("desc", bot.description)
        .put("at", bot.lastActivityAtMs ?: JSONObject.NULL)
        .put("status", bot.status.name)
        .put("waiting", bot.waitingThreads)
        .put("chats", bot.chats?.let { chats -> JSONArray().also { array -> chats.forEach { array.put(encode(it)) } } } ?: JSONObject.NULL)

    private fun encode(chat: ChatItem) = JSONObject()
        .put("id", chat.sessionId)
        .put("title", chat.title)
        .put("status", chat.status.name)
        .put("pending", chat.pendingAction?.name ?: JSONObject.NULL)

    fun decode(text: String): BotsSnapshot? = try {
        val json = JSONObject(text)
        if (json.getInt("v") != VERSION) null
        else BotsSnapshot(
            uid = json.getString("uid"),
            hostId = json.getString("hostId"),
            savedAtMs = json.getLong("savedAtMs"),
            bots = json.getJSONArray("bots").let { array -> (0 until array.length()).map { decodeBot(array.getJSONObject(it)) } },
        )
    } catch (_: Exception) {
        null
    }

    private fun decodeBot(json: JSONObject) = BotItem(
        botId = json.getString("id"),
        name = json.getString("name"),
        description = json.optString("desc", ""),
        status = enumOrDefault(json.optString("status"), RemoteBotStatusFfi.IDLE),
        waitingThreads = json.optInt("waiting", 0).coerceAtLeast(0),
        lastActivityAtMs = if (json.isNull("at")) null else json.getLong("at"),
        chats = if (json.isNull("chats")) null else json.getJSONArray("chats").let { array -> (0 until array.length()).map { decodeChat(array.getJSONObject(it)) } },
    )

    private fun decodeChat(json: JSONObject) = ChatItem(
        sessionId = json.getString("id"),
        title = json.optString("title", ""),
        status = enumOrDefault(json.optString("status"), RemoteSessionStatusFfi.IDLE),
        pendingAction = if (json.isNull("pending")) null else enumOrNull<RemotePendingActionFfi>(json.optString("pending")),
    )

    private inline fun <reified E : Enum<E>> enumOrNull(name: String): E? = enumValues<E>().firstOrNull { it.name == name }
    private inline fun <reified E : Enum<E>> enumOrDefault(name: String, default: E): E = enumOrNull<E>(name) ?: default
}

// --- snapshots ---------------------------------------------------------------------------------------------------------

/**
 * The saved Bots and chats of every computer of the signed-in user. The newest state is held in memory (what the tree
 * reads), and written behind: a burst of updates becomes one atomic DataStore write [debounceMs] after the last. A
 * deletion is written at once. Past [BotsLogic.MAX_HOSTS] computers the least recently saved goes.
 */
internal class BotsSnapshotStore(
    private val dataStore: DataStore<Preferences>,
    private val scope: CoroutineScope,
    private val debounceMs: Long = DEBOUNCE_MS,
) {
    private val loaded = MutableStateFlow<Map<String, BotsSnapshot>?>(null)
    private val loadLock = Mutex()
    private val writeLock = Mutex()
    private val lock = Any()
    private val dirty = mutableSetOf<String>()
    private var pendingWrite: Job? = null

    /** Writes that reached the DataStore, for tests that check the debounce. */
    @Volatile internal var writes = 0
        private set

    private suspend fun ensureLoaded() {
        if (loaded.value != null) return
        loadLock.withLock {
            if (loaded.value != null) return
            val prefs = dataStore.data.first()
            val parsed = mutableMapOf<String, BotsSnapshot>()
            val unreadable = mutableSetOf<String>()
            for ((key, value) in prefs.asMap()) {
                if (!key.name.startsWith(SNAPSHOT_PREFIX) || value !is String) continue
                val snapshot = BotsSnapshotCodec.decode(value)
                if (snapshot == null || key.name != snapshotKeyName(snapshot.uid, snapshot.hostId)) unreadable += key.name else parsed[key.name] = snapshot
            }
            synchronized(lock) { dirty += unreadable }
            loaded.value = parsed
            if (unreadable.isNotEmpty()) schedule()
        }
    }

    /** The saved computers of [uid] by host id. Emits again on every change; never holds another user's. */
    fun snapshots(uid: String): Flow<Map<String, BotsSnapshot>> = flow {
        ensureLoaded()
        emitAll(loaded.filterNotNull().map { all -> all.values.filter { it.uid == uid }.associateBy { it.hostId } }.distinctUntilChanged())
    }

    suspend fun load(uid: String, hostId: String): BotsSnapshot? {
        ensureLoaded()
        return loaded.value?.get(snapshotKeyName(uid, hostId))?.takeIf { it.uid == uid }
    }

    /** Replaces the snapshot of ([uid], [hostId]) by what [transform] makes of it; the same instance back changes nothing. */
    suspend fun update(uid: String, hostId: String, transform: (BotsSnapshot?) -> BotsSnapshot?) {
        ensureLoaded()
        val changed = synchronized(lock) {
            val all = loaded.value.orEmpty()
            val name = snapshotKeyName(uid, hostId)
            val prior = all[name]?.takeIf { it.uid == uid }
            val next = transform(prior)
            if (next === prior || next == prior) false
            else {
                val updated = all.toMutableMap()
                if (next == null) updated.remove(name) else updated[name] = next.copy(uid = uid, hostId = hostId)
                dirty += name
                // Past the cap the least recently saved computers of this user go.
                val mine = updated.values.filter { it.uid == uid }
                if (mine.size > BotsLogic.MAX_HOSTS) {
                    val keep = BotsLogic.evictHosts(mine).map { it.hostId }.toSet()
                    mine.filter { it.hostId !in keep }.forEach { gone ->
                        val goneName = snapshotKeyName(gone.uid, gone.hostId)
                        updated.remove(goneName)
                        dirty += goneName
                    }
                }
                loaded.value = updated
                true
            }
        }
        if (changed) schedule()
    }

    /** Forgets one computer's snapshot (it was forgotten on this phone). Written at once. */
    suspend fun delete(uid: String, hostId: String) {
        ensureLoaded()
        val name = snapshotKeyName(uid, hostId)
        synchronized(lock) {
            val all = loaded.value.orEmpty()
            if (name in all) loaded.value = all - name
            dirty += name
        }
        flush()
    }

    /** Drops every snapshot that is not [uid]'s (all of them when signed out): on sign-out, and on the account changing. */
    suspend fun retainOnly(uid: String?) {
        ensureLoaded()
        synchronized(lock) {
            val all = loaded.value.orEmpty()
            val gone = all.filterValues { it.uid != uid }
            if (gone.isNotEmpty()) {
                loaded.value = all - gone.keys
                dirty += gone.keys
            }
        }
        flush()
    }

    private fun schedule() {
        synchronized(lock) {
            pendingWrite?.cancel()
            pendingWrite = scope.launch {
                delay(debounceMs)
                flush()
            }
        }
    }

    /** Writes what is waiting. A failed write stays waiting and is tried again with the next change. */
    suspend fun flush() = writeLock.withLock {
        val (names, current) = synchronized(lock) {
            dirty.toSet().also { dirty.clear() } to loaded.value.orEmpty()
        }
        if (names.isEmpty()) return@withLock
        try {
            dataStore.edit { prefs ->
                for (name in names) {
                    val key = stringPreferencesKey(name)
                    val snapshot = current[name]
                    if (snapshot == null) prefs.remove(key) else prefs[key] = BotsSnapshotCodec.encode(snapshot)
                }
            }
            writes++
        } catch (cancelled: CancellationException) {
            synchronized(lock) { dirty += names }
            throw cancelled
        } catch (_: Exception) {
            synchronized(lock) { dirty += names }
        }
    }

    companion object {
        /** Writes are held back this long after the last change (section 11). */
        const val DEBOUNCE_MS = 1_000L
    }
}

// --- preferences -----------------------------------------------------------------------------------------------------

/** The tree's own choices (section 4.7). [expandedBots] null = the user never chose, so the default applies. */
internal data class BotsPrefs(val collapsedHosts: Set<String> = emptySet(), val expandedBots: Set<String>? = null)

/** Collapsed computers, open Bots and the last-used computer of one user, in the same DataStore as the snapshots. */
internal class BotsPrefsStore(private val dataStore: DataStore<Preferences>) {
    fun prefs(uid: String): Flow<BotsPrefs> = dataStore.data.map { prefs ->
        BotsPrefs(
            collapsedHosts = prefs[stringSetPreferencesKey(COLLAPSED_PREFIX + uid)].orEmpty(),
            expandedBots = prefs[stringSetPreferencesKey(EXPANDED_PREFIX + uid)],
        )
    }.distinctUntilChanged()

    suspend fun setCollapsed(uid: String, hostId: String, collapsed: Boolean) {
        dataStore.edit { prefs ->
            val key = stringSetPreferencesKey(COLLAPSED_PREFIX + uid)
            val next = prefs[key].orEmpty().let { if (collapsed) it + hostId else it - hostId }
            if (next.isEmpty()) prefs.remove(key) else prefs[key] = next
        }
    }

    /** Saves the whole set of open Bots, `hostId/botId` each: the first choice turns the default into an explicit set. */
    suspend fun setExpandedBots(uid: String, expanded: Set<String>) {
        dataStore.edit { it[stringSetPreferencesKey(EXPANDED_PREFIX + uid)] = expanded }
    }

    suspend fun lastHostId(uid: String): String? = dataStore.data.first()[stringPreferencesKey(LAST_HOST_PREFIX + uid)]

    suspend fun saveLastHost(uid: String, hostId: String) {
        dataStore.edit { it[stringPreferencesKey(LAST_HOST_PREFIX + uid)] = hostId }
    }

    /** A computer that was forgotten leaves the choices too. */
    suspend fun forgetHost(uid: String, hostId: String) {
        dataStore.edit { prefs ->
            val collapsed = stringSetPreferencesKey(COLLAPSED_PREFIX + uid)
            val collapsedNow = prefs[collapsed].orEmpty()
            if (hostId in collapsedNow) {
                if (collapsedNow.size == 1) prefs.remove(collapsed) else prefs[collapsed] = collapsedNow - hostId
            }
            val expanded = stringSetPreferencesKey(EXPANDED_PREFIX + uid)
            val expandedNow = prefs[expanded]
            if (expandedNow != null) prefs[expanded] = expandedNow.filterNot { it.startsWith("$hostId/") }.toSet()
            val last = stringPreferencesKey(LAST_HOST_PREFIX + uid)
            if (prefs[last] == hostId) prefs.remove(last)
        }
    }

    /** Drops every choice that is not [uid]'s. */
    suspend fun retainOnly(uid: String?) {
        dataStore.edit { prefs ->
            val mine = setOf(COLLAPSED_PREFIX + uid, EXPANDED_PREFIX + uid, LAST_HOST_PREFIX + uid)
            prefs.asMap().keys
                .filter { key -> (key.name.startsWith(COLLAPSED_PREFIX) || key.name.startsWith(EXPANDED_PREFIX) || key.name.startsWith(LAST_HOST_PREFIX)) && (uid == null || key.name !in mine) }
                .forEach { prefs.remove(it) }
        }
    }
}

// --- the process's stores ----------------------------------------------------------------------------------------------

/** The Bots tab's stores over the one DataStore file of the process (a file takes only one DataStore). */
internal class BotsStores(dataStore: DataStore<Preferences>, scope: CoroutineScope, debounceMs: Long = BotsSnapshotStore.DEBOUNCE_MS) {
    val snapshots = BotsSnapshotStore(dataStore, scope, debounceMs)
    val prefs = BotsPrefsStore(dataStore)

    suspend fun retainOnly(uid: String?) {
        snapshots.retainOnly(uid)
        prefs.retainOnly(uid)
    }

    companion object {
        @Volatile private var instance: BotsStores? = null

        fun get(context: Context): BotsStores = instance ?: synchronized(this) {
            instance ?: create(context.applicationContext).also { instance = it }
        }

        private fun create(context: Context): BotsStores {
            val scope = CoroutineScope(SupervisorJob() + Dispatchers.IO)
            return BotsStores(PreferenceDataStoreFactory.create(scope = scope) { context.preferencesDataStoreFile("bots") }, scope)
        }
    }
}

/**
 * What the manager tells the stores, one thing at a time and in order: the last-used computer, the live Bots and
 * chats of the connected computer (kept as that computer's snapshot), a forgotten computer, a new account.
 */
internal class BotsRecorder(
    private val stores: BotsStores,
    scope: CoroutineScope,
    private val clock: () -> Long = System::currentTimeMillis,
) : RemoteBotsObserver {
    private val work = Channel<suspend () -> Unit>(Channel.UNLIMITED)
    private val accountLock = Any()
    @Volatile private var activeUid: String? = null
    @Volatile private var accountEpoch = 0L

    private fun record(uid: String, action: suspend () -> Unit) {
        synchronized(accountLock) {
            if (activeUid != uid) return
            val epoch = accountEpoch
            work.trySend { if (activeUid == uid && accountEpoch == epoch) action() }
        }
    }

    init {
        scope.launch {
            for (job in work) {
                try {
                    job()
                } catch (cancelled: CancellationException) {
                    throw cancelled
                } catch (_: Exception) {
                    // A failed save costs a snapshot, never the tab.
                }
            }
        }
    }

    override suspend fun lastHostId(uid: String): String? = stores.prefs.lastHostId(uid)

    override fun onAccount(uid: String?) {
        synchronized(accountLock) {
            activeUid = uid
            accountEpoch++
            work.trySend { stores.retainOnly(uid) }
        }
    }

    override fun onConnected(uid: String, hostId: String) {
        record(uid) { stores.prefs.saveLastHost(uid, hostId) }
    }

    override fun onBots(uid: String, hostId: String, state: RemoteBotsStateFfi) {
        val live = (state as? RemoteBotsStateFfi.Bots)?.bots ?: return
        val items = live.map { it.toItem() }
        record(uid) { stores.snapshots.update(uid, hostId) { prior -> BotsLogic.applyBots(prior, uid, hostId, items, clock()) } }
    }

    override fun onBotThreads(uid: String, hostId: String, botId: String, state: RemoteBotThreadsStateFfi) {
        val threads = state.threads ?: return
        val chats = threads.map { it.toItem() }
        record(uid) {
            stores.snapshots.update(uid, hostId) { prior ->
                // The same chats again change nothing, so a chatty list does not rewrite the file.
                val saved = prior?.bots?.firstOrNull { it.botId == botId }?.chats
                if (saved == chats.take(BotsLogic.MAX_CHATS)) prior else BotsLogic.applyChats(prior, botId, chats, clock())
            }
        }
    }

    override fun onForgotten(uid: String, hostId: String) {
        record(uid) {
            stores.snapshots.delete(uid, hostId)
            stores.prefs.forgetHost(uid, hostId)
        }
    }
}
