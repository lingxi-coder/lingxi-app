package com.lingxi.code.teams

import android.content.Context
import androidx.datastore.core.DataStore
import androidx.datastore.preferences.core.Preferences
import androidx.datastore.preferences.core.edit
import androidx.datastore.preferences.core.stringPreferencesKey
import androidx.datastore.preferences.core.stringSetPreferencesKey
import androidx.datastore.preferences.preferencesDataStore
import com.google.firebase.auth.FirebaseAuth
import com.google.firebase.firestore.FirebaseFirestore
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.SharingStarted
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.combine
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.flow.map
import kotlinx.coroutines.flow.stateIn
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch
import java.io.File

private val Context.teamsDataStore: DataStore<Preferences> by preferencesDataStore(name = "teams")

/** What the Teams tab remembers on the device, per account: the selected team and the collapsed list sections of each team. */
internal class TeamsPrefs(private val store: DataStore<Preferences>) {
    private fun selectedKey(uid: String) = stringPreferencesKey("teams.selected.$uid")
    private fun collapsedKey(uid: String, teamId: String) = stringSetPreferencesKey("teams.collapsed.$uid.$teamId")
    suspend fun selected(uid: String): String? = store.data.first()[selectedKey(uid)]
    suspend fun setSelected(uid: String, teamId: String) { store.edit { it[selectedKey(uid)] = teamId } }
    suspend fun collapsed(uid: String, teamId: String): Set<String> = store.data.first()[collapsedKey(uid, teamId)] ?: emptySet()
    suspend fun setCollapsed(uid: String, teamId: String, sections: Set<String>) {
        store.edit { if (sections.isEmpty()) it.remove(collapsedKey(uid, teamId)) else it[collapsedKey(uid, teamId)] = sections }
    }
}

/** Unsent text of one conversation: kept in memory by the workspace only (Desktop parity), never on disk. */
internal data class ComposerDraft(val text: String = "", val replyToId: String? = null, val editingId: String? = null, val targets: Set<String> = emptySet())

/**
 * The app-level Teams workspace (SPEC-mobile-team-bot-ui section 6.6): one per process, started by the Teams tab or by the
 * first read of [TeamsBadge.count], stopped only by sign-out or an account change, never by a page closing.
 *
 * It composes what already existed and adds the app-level parts:
 * 1. [teamsModel]: the teams list, the selected team's members and invites, and the team commands.
 * 2. [messagingModel]: full messaging (directory, reads, signals, the open thread, command journal, Bots) of the selected team.
 * 3. Directory-only [TeamsUnreadWatcher]s for every other team (at most [MAX_WATCHED_TEAMS]) so the switcher and the tab badge
 *    know every team's unread count.
 * 4. The persisted selected team and collapsed sections ([TeamsPrefs]) and the in-memory composer drafts.
 */
internal class TeamsWorkspace(
    context: Context,
    private val scope: CoroutineScope,
    auth: FirebaseAuth,
    private val db: FirebaseFirestore,
    private val prefs: TeamsPrefs,
    apiRoot: String = TEAMS_API_ROOT,
) {
    val teamsModel = TeamsViewModel(scope, apiRoot, auth, db)
    val messagingModel = TeamMessagingModel(scope, File(context.noBackupFilesDir, "team-messages"), apiRoot, auth, db)

    val teams: StateFlow<TeamsState> get() = teamsModel.state
    val messaging: StateFlow<TeamMessagingState> get() = messagingModel.state

    private val watchers = mutableMapOf<String, TeamsUnreadWatcher>()
    private val watched = MutableStateFlow<Map<String, Int>>(emptyMap())
    private val mutableCollapsed = MutableStateFlow<Set<String>>(emptySet())
    /** Collapsed list sections of the selected team ([SECTION_CHANNELS], [SECTION_DIRECT], [SECTION_BOTS]). */
    val collapsed: StateFlow<Set<String>> = mutableCollapsed

    /** Unread messages per team: the selected team from its full model, every other team from its watcher. */
    val unreadByTeam: StateFlow<Map<String, Int>> = combine(teams, messaging, watched) { teamsState, messagingState, others ->
        teamsState.teams.associate { team ->
            team.id to if (team.id == teamsState.selectedId && messagingState.readsReady && messagingState.cache?.teamId == team.id) selectedUnread(messagingState) else others[team.id] ?: 0
        }
    }.stateIn(scope, SharingStarted.Eagerly, emptyMap())

    /** The Teams tab badge. */
    val totalUnread: StateFlow<Int> = unreadByTeam.map { values -> values.values.sum() }.stateIn(scope, SharingStarted.Eagerly, 0)

    private var started = false
    private var currentUid: String? = null
    private var startedTeam: TeamSummary? = null
    private var persistedSelection: String? = null
    private var autoSelecting = false
    private var wasRevoked = false
    private var lastSelectedUnread: Pair<String, Int>? = null
    private val drafts = mutableMapOf<String, ComposerDraft>()

    fun start() {
        if (started) return
        started = true
        teamsModel.start()
        scope.launch { teamsModel.state.collect { onTeams(it) } }
        scope.launch { messagingModel.state.collect { onMessaging(it) } }
    }

    // --- reacting to the models ----------------------------------------------------------------------

    private fun onTeams(state: TeamsState) {
        if (state.uid != currentUid) accountChanged(state.uid)
        val uid = state.uid ?: return
        val selected = state.teams.firstOrNull { it.id == state.selectedId }
        val running = startedTeam
        if (selected == null) {
            if (running != null) { rememberUnread(running); messagingModel.reset(); startedTeam = null; mutableCollapsed.value = emptySet() }
        } else if (running?.id != selected.id || running.generation != selected.generation) {
            if (running != null) rememberUnread(running)
            messagingModel.start(uid, selected)
            startedTeam = selected
        }
        if (state.selectedId.isNotEmpty() && state.selectedId != persistedSelection) {
            persistedSelection = state.selectedId
            val teamId = state.selectedId
            scope.launch {
                prefs.setSelected(uid, teamId)
                val sections = prefs.collapsed(uid, teamId)
                if (teamsModel.state.value.selectedId == teamId) mutableCollapsed.value = sections
            }
        }
        if (state.loaded && state.selectedId.isEmpty() && state.loadError == null && !state.busy && state.teams.isNotEmpty() && !autoSelecting) {
            autoSelecting = true
            scope.launch {
                val saved = prefs.selected(uid)
                autoSelecting = false
                val now = teamsModel.state.value
                if (now.uid == uid && now.selectedId.isEmpty() && now.teams.isNotEmpty()) teamsModel.select((now.teams.firstOrNull { it.id == saved } ?: now.teams.first()).id)
            }
        }
        syncWatchers(uid, state)
    }

    private fun onMessaging(state: TeamMessagingState) {
        val team = startedTeam
        if (team != null && state.readsReady && state.cache?.teamId == team.id) lastSelectedUnread = team.id to selectedUnread(state)
        // Membership was revoked under us: the teams list is the authority on what is left.
        if (state.revoked && !wasRevoked) teamsModel.refresh()
        wasRevoked = state.revoked
    }

    private fun accountChanged(uid: String?) {
        currentUid = uid; startedTeam = null; persistedSelection = null; autoSelecting = false; wasRevoked = false; lastSelectedUnread = null
        messagingModel.reset()
        watchers.values.forEach { it.stop() }; watchers.clear(); watched.value = emptyMap()
        drafts.clear(); mutableCollapsed.value = emptySet()
    }

    private fun rememberUnread(team: TeamSummary) {
        lastSelectedUnread?.takeIf { it.first == team.id }?.let { (id, unread) -> watched.update { it + (id to unread) } }
    }

    private fun selectedUnread(state: TeamMessagingState): Int =
        (state.cache?.conversations ?: emptyList()).sumOf { TeamsListLogic.unreadCount(it).toLong() }.coerceAtMost(Int.MAX_VALUE.toLong()).toInt()

    private fun syncWatchers(uid: String, state: TeamsState) {
        val wanted = state.teams.filter { it.id != state.selectedId }.take(MAX_WATCHED_TEAMS).associateBy { it.id }
        for ((id, watcher) in watchers.toList()) if (wanted[id]?.generation != watcher.generation) { watcher.stop(); watchers.remove(id) }
        for ((id, team) in wanted) if (id !in watchers) {
            lateinit var watcher: TeamsUnreadWatcher
            watcher = TeamsUnreadWatcher(db, uid, id, team.generation) { unread -> if (watchers[id] === watcher) watched.update { it + (id to unread) } }
            watchers[id] = watcher
            watcher.start()
        }
        val known = state.teams.map { it.id }.toSet()
        watched.update { values -> if (values.keys.all { it in known }) values else values.filterKeys { it in known } }
    }

    // --- what the screens call -----------------------------------------------------------------------

    fun selectTeam(teamId: String) { if (teamsModel.state.value.selectedId != teamId) teamsModel.select(teamId) }

    /** Opens a conversation's thread (its messages listener); "" closes it. */
    fun openConversation(conversationId: String) = messagingModel.select(conversationId)

    /** The banner's Reconnect: ask again, restart what stopped, and let the SDK reconnect. */
    fun reconnect() {
        val uid = currentUid ?: return
        teamsModel.refresh()
        val state = messagingModel.state.value
        val team = startedTeam
        if (team != null && (state.revoked || state.error == "INVALID_DATA")) messagingModel.start(uid, team) else messagingModel.reconnect()
        watchers.values.forEach { it.stop() }; watchers.clear()
        syncWatchers(uid, teamsModel.state.value)
    }

    fun toggleSection(section: String) {
        val uid = currentUid ?: return
        val teamId = teamsModel.state.value.selectedId.takeIf { it.isNotEmpty() } ?: return
        val next = mutableCollapsed.value.let { if (section in it) it - section else it + section }
        mutableCollapsed.value = next
        scope.launch { prefs.setCollapsed(uid, teamId, next) }
    }

    fun draft(conversationId: String): ComposerDraft = drafts[conversationId] ?: ComposerDraft()
    fun setDraft(conversationId: String, draft: ComposerDraft) { if (draft == ComposerDraft()) drafts.remove(conversationId) else drafts[conversationId] = draft }

    companion object {
        const val MAX_WATCHED_TEAMS = 20
        const val SECTION_CHANNELS = "channels"
        const val SECTION_DIRECT = "direct"
        const val SECTION_BOTS = "bots"

        @Volatile private var instance: TeamsWorkspace? = null

        /** The process-wide workspace, started on first use. */
        fun get(context: Context): TeamsWorkspace = instance ?: synchronized(this) {
            instance ?: TeamsWorkspace(
                context.applicationContext, CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate),
                FirebaseAuth.getInstance(), FirebaseFirestore.getInstance(), TeamsPrefs(context.applicationContext.teamsDataStore),
            ).also { instance = it; it.start() }
        }
    }
}
