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
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
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

/** The one team of a friend chat (docs/FRIENDS-CLIENTS.md D9): the signed-in account and the pair team's synthesized summary. */
internal data class PairTeam(val uid: String, val summary: TeamSummary)

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
 *
 * With a [pair] it is a friend chat's workspace instead (see [pinned]): only [messagingModel] runs, on the pair team. [teamsModel]
 * is never started, so nothing lists teams, selects one, watches the others or persists a selection.
 */
internal class TeamsWorkspace(
    context: Context,
    private val scope: CoroutineScope,
    auth: FirebaseAuth,
    private val db: FirebaseFirestore,
    private val prefs: TeamsPrefs,
    apiRoot: String = TEAMS_API_ROOT,
    private val pair: PairTeam? = null,
) {
    val teamsModel = TeamsViewModel(scope, apiRoot, auth, db)
    val messagingModel = TeamMessagingModel(scope, File(context.noBackupFilesDir, "team-messages"), apiRoot, auth, db)
    /** Where a file being opened is written for the app that shows it; emptied before each open and when the account changes. */
    val openedFiles = File(context.cacheDir, TEAM_FILES_DIRECTORY)

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
    private val mutableDraftFiles = MutableStateFlow<Map<String, List<TeamDraftFile>>>(emptyMap())
    private val mutableFileRefusals = MutableStateFlow<Map<String, List<TeamFileRefusal>>>(emptyMap())
    /**
     * The files of unsent messages and what was last refused when files were chosen, by [draftKey]. Flows: an upload moves on while
     * its conversation is closed, and the composer shows where it stands when it is opened again.
     */
    val draftFiles: StateFlow<Map<String, List<TeamDraftFile>>> = mutableDraftFiles
    val fileRefusals: StateFlow<Map<String, List<TeamFileRefusal>>> = mutableFileRefusals
    /** The uploads in progress, by upload id. */
    private val uploads = mutableMapOf<String, Job>()

    fun start() {
        if (started) return
        started = true
        scope.launch(Dispatchers.IO) { openedFiles.deleteRecursively() }
        if (pair != null) {
            currentUid = pair.uid; startedTeam = pair.summary
            messagingModel.start(pair.uid, pair.summary)
            return
        }
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
            if (running != null) { rememberUnread(running); endUploads(); messagingModel.reset(); startedTeam = null; mutableCollapsed.value = emptySet() }
        } else if (running?.id != selected.id || running.generation != selected.generation) {
            if (running != null) rememberUnread(running)
            endUploads()
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
        endUploads()
        messagingModel.reset()
        watchers.values.forEach { it.stop() }; watchers.clear(); watched.value = emptyMap()
        drafts.clear(); mutableCollapsed.value = emptySet()
        mutableDraftFiles.value = emptyMap(); mutableFileRefusals.value = emptyMap()
        scope.launch(Dispatchers.IO) { openedFiles.deleteRecursively() }
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

    /** Ends a friend chat's workspace: its listeners and its coroutines. The process workspace is never closed. */
    fun close() { endUploads(); messagingModel.reset(); scope.cancel() }

    fun selectTeam(teamId: String) { if (teamsModel.state.value.selectedId != teamId) teamsModel.select(teamId) }

    /** Opens a conversation's thread (its messages listener); "" closes it. */
    fun openConversation(conversationId: String) = messagingModel.select(conversationId)

    /** The banner's Reconnect: ask again, restart what stopped, and let the SDK reconnect. */
    fun reconnect() {
        val uid = currentUid ?: return
        teamsModel.refresh()
        val state = messagingModel.state.value
        val team = startedTeam
        if (team != null && (state.revoked || state.error == "INVALID_DATA")) { endUploads(); messagingModel.start(uid, team) } else messagingModel.reconnect()
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

    /** What a conversation's unsent state is kept under: the account, the team, its generation and the conversation. */
    fun draftKey(conversationId: String) = "${currentUid}:${teamsModel.state.value.selectedId}:${startedTeam?.generation ?: 0}:$conversationId"
    fun draft(conversationId: String): ComposerDraft = drafts[draftKey(conversationId)] ?: ComposerDraft()
    fun setDraft(conversationId: String, draft: ComposerDraft) { if (draft == ComposerDraft()) drafts.remove(draftKey(conversationId)) else drafts[draftKey(conversationId)] = draft }

    // --- files of an unsent message --------------------------------------------------------------------

    /**
     * Adds chosen files to a conversation's unsent message and starts uploading the ones it takes; the others are kept as refusals
     * with the reason for each. An upload reports to the draft it was started for and to no other: one of another conversation, team
     * or account never sees it.
     */
    fun addFiles(conversationId: String, chosen: List<TeamChosenFile>) {
        if (!started || conversationId.isEmpty() || messagingModel.state.value.selected != conversationId) return
        val key = draftKey(conversationId)
        val readable = chosen.filter { it.error == null }
        val admission = TeamAttachments.admit(mutableDraftFiles.value[key].orEmpty().map { it.byteLength }, readable.map { TeamAttachments.Picked(it.name, it.type, it.bytes.size) })
        val refused = chosen.filter { it.error != null }.map { TeamFileRefusal(TeamAttachments.name(it.name), it.error!!) } + admission.refused
        mutableFileRefusals.update { if (refused.isEmpty()) it - key else it + (key to refused) }
        for (accepted in admission.accepted) {
            val bytes = readable[accepted.index].bytes
            val file = TeamDraftFile(java.util.UUID.randomUUID().toString(), accepted.name, accepted.mime, bytes.size)
            mutableDraftFiles.update { it + (key to it[key].orEmpty() + file) }
            lateinit var job: Job
            job = scope.launch(start = kotlinx.coroutines.CoroutineStart.LAZY) {
                try {
                    messagingModel.upload(conversationId, file.uploadId, file.name, file.mime, bytes) { sent -> updateFile(key, file.uploadId) { it.copy(sent = sent) } }
                    updateFile(key, file.uploadId) { it.copy(state = TeamDraftFile.State.Ready, sent = it.byteLength) }
                } catch (error: kotlinx.coroutines.CancellationException) { throw error }
                catch (error: Exception) { updateFile(key, file.uploadId) { it.copy(state = TeamDraftFile.State.Failed, error = (error as? TeamMessageFailure)?.code ?: "UPLOAD_FAILED") } }
                finally { if (uploads[file.uploadId] === job) uploads.remove(file.uploadId) }
            }
            uploads[file.uploadId] = job
            job.start()
        }
    }

    private fun updateFile(key: String, uploadId: String, change: (TeamDraftFile) -> TeamDraftFile) {
        mutableDraftFiles.update { all -> all[key]?.let { files -> all + (key to files.map { if (it.uploadId == uploadId) change(it) else it }) } ?: all }
    }

    private fun dropFiles(key: String, drop: (TeamDraftFile) -> Boolean) {
        mutableDraftFiles.update { all -> all[key]?.filterNot(drop)?.let { kept -> if (kept.isEmpty()) all - key else all + (key to kept) } ?: all }
    }

    /** Takes a file out of an unsent message and gives its upload up. */
    fun removeFile(conversationId: String, uploadId: String) {
        val key = draftKey(conversationId)
        if (mutableDraftFiles.value[key]?.none { it.uploadId == uploadId } != false) return
        dropFiles(key) { it.uploadId == uploadId }
        val running = uploads.remove(uploadId)
        running?.cancel()
        scope.launch { running?.join(); messagingModel.cancelUpload(uploadId) }
    }

    fun dismissRefusals(conversationId: String) { mutableFileRefusals.update { it - draftKey(conversationId) } }

    /** The files a message was sent with leave the draft; they are the message's now. */
    fun filesSent(conversationId: String, uploadIds: List<String>) = dropFiles(draftKey(conversationId)) { it.uploadId in uploadIds }

    /** An upload belongs to the team it was started in: closing the team ends it, and the file leaves its draft. Checked files stay. */
    private fun endUploads() {
        uploads.values.forEach { it.cancel() }; uploads.clear()
        mutableDraftFiles.update { all -> all.mapValues { (_, files) -> files.filter { it.state == TeamDraftFile.State.Ready } }.filterValues { it.isNotEmpty() } }
    }

    companion object {
        const val MAX_WATCHED_TEAMS = 20
        /** Under the cache directory; the app's file provider shares it (`res/xml/file_paths.xml`). */
        const val TEAM_FILES_DIRECTORY = "team-files"
        const val SECTION_CHANNELS = "channels"
        const val SECTION_DIRECT = "direct"
        const val SECTION_BOTS = "bots"

        @Volatile private var instance: TeamsWorkspace? = null

        /**
         * The workspace of one friendship's chat (D9), pinned to its pair team so the Teams tab's workspace never sees that team. The
         * caller starts it and [close]s it; it makes no `GET /v1/teams` and no team-detail call.
         */
        fun pinned(context: Context, pair: PairTeam): TeamsWorkspace = TeamsWorkspace(
            context.applicationContext, CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate),
            FirebaseAuth.getInstance(), FirebaseFirestore.getInstance(), TeamsPrefs(context.applicationContext.teamsDataStore), pair = pair,
        )

        /** The process-wide workspace, started on first use. */
        fun get(context: Context): TeamsWorkspace = instance ?: synchronized(this) {
            instance ?: TeamsWorkspace(
                context.applicationContext, CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate),
                FirebaseAuth.getInstance(), FirebaseFirestore.getInstance(), TeamsPrefs(context.applicationContext.teamsDataStore),
            ).also { instance = it; it.start() }
        }
    }
}
