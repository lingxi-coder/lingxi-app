package com.lingxi.code.conversation

import android.app.ActivityManager
import android.app.ApplicationExitInfo
import android.content.Context
import android.content.SharedPreferences
import android.os.Build
import com.lingxi.code.R
import com.lingxi.code.bindings.client.ClientCommand
import com.lingxi.code.bindings.client.ClientEvent
import com.lingxi.code.bindings.client.ClientTextDto
import com.lingxi.code.bindings.client.ImageRefDto
import com.lingxi.code.bindings.client.PermissionResponseDto
import com.lingxi.code.bindings.client.PromptModeDto
import com.lingxi.code.bindings.client.TurnRecoveryStateDto
import com.lingxi.code.model.EngineModelState
import com.lingxi.code.model.EngineSessionState
import com.lingxi.code.model.MCPServer
import com.lingxi.code.model.Message
import com.lingxi.code.model.SessionMode
import com.lingxi.code.model.canonicalSessionId
import com.lingxi.code.project.ProjectWorkspace
import com.lingxi.code.settings.LinuxRuntimeMode
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Deferred
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.SharedFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.emitAll
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.flow.flow
import kotlinx.coroutines.launch

internal data class DurableConversationTurnRecord(
    val scope: String,
    val sessionId: String,
    val turnId: Long,
    val queuedOwner: Boolean = false,
)

internal data class DurableConversationTurnState(
    val active: DurableConversationTurnRecord?,
    val queuedTurnIds: List<Long>,
)

internal fun recordHandledProcessExit(preferences: SharedPreferences, timestamp: Long, userRequested: Boolean) {
    val editor = preferences.edit()
    if (userRequested) {
        preferences.all.keys.filter { it.startsWith("turn.") }.forEach(editor::remove)
    }
    check(editor.putLong("handled_exit_timestamp", timestamp).commit())
}

/** Minimum non-secret launch context needed for service redelivery recovery. */
data class ConversationRecoverySpec(
    val projectId: String?,
    val hostPath: String?,
    val sessionMode: SessionMode,
    val linuxRuntimeMode: LinuxRuntimeMode,
    val workspaceKey: String? = null,
    val guestPath: String? = null,
    val additionalDirectories: List<String> = emptyList(),
    /** Exact SDK catalog path, which may be a guest path; never rewrite it. */
    val transcriptPath: String? = null,
    /** A new engine generation never inherits another owner's pending actions. */
    val runtimeGeneration: String = java.util.UUID.randomUUID().toString(),
) {
    val scopeKey: String
        get() = "${workspaceKey ?: hostPath ?: "__global__"}#${sessionMode.wireValue}"
    val ownerKey: String get() = "$scopeKey@$runtimeGeneration"

    fun projectWorkspace(): ProjectWorkspace? {
        val path = hostPath ?: return null
        val guest = guestPath ?: return null
        return ProjectWorkspace(projectId = projectId ?: "recovered", hostPath = path, guestPath = guest, additionalDirectories = additionalDirectories)
    }
}

internal class DurableConversationTurnClientStore(
    private val preferences: SharedPreferences,
    private val scope: String,
    latestExit: ApplicationExitInfo? = null,
    initialSessionId: String? = null,
) {
    private var ownerSessionId: String? = initialSessionId?.let(::canonicalSessionId)

    companion object {
        private val lock = Any()
        private fun prefix(scope: String): String = "turn." +
            scope.toByteArray(Charsets.UTF_8).joinToString("") { "%02x".format(it.toInt() and 255) } + "."
        private fun key(scope: String, sessionId: String, turnId: Long): String =
            prefix(scope) + canonicalSessionId(sessionId) + "." + turnId
        private fun queuedKey(scope: String, sessionId: String) = prefix(scope) + canonicalSessionId(sessionId) + ".queued"
        private fun queuedOwnerKey(scope: String, sessionId: String) = prefix(scope) + canonicalSessionId(sessionId) + ".queuedOwner"

        fun ownerModeForSession(context: Context, workspaceKey: String, sessionId: String?): SessionMode? {
            val prefs = context.applicationContext.getSharedPreferences("durable_conversation_turn", Context.MODE_PRIVATE)
            return ownerModeForSession(prefs, workspaceKey, sessionId)
        }

        internal fun ownerModeForSession(prefs: SharedPreferences, workspaceKey: String, sessionId: String?): SessionMode? = synchronized(lock) {
            if (sessionId.isNullOrBlank()) return@synchronized null
            val id = canonicalSessionId(sessionId)
            val matches = SessionMode.entries.filter { mode ->
                val scope = "$workspaceKey#${mode.wireValue}"
                val ownerPrefix = prefix(scope) + id + "."
                prefs.all.keys.any { key ->
                    key.startsWith(ownerPrefix) && key.removePrefix(ownerPrefix).toLongOrNull() != null
                } || !prefs.getString(queuedKey(scope, id), "").isNullOrBlank()
            }
            check(matches.size <= 1) { "Conflicting recovery modes for session $id" }
            matches.singleOrNull()
        }
    }

    init {
        synchronized(lock) {
            val handledAt = preferences.getLong("handled_exit_timestamp", 0L)
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R && latestExit != null && latestExit.timestamp > handledAt) {
                recordHandledProcessExit(preferences, latestExit.timestamp,
                    latestExit.reason == ApplicationExitInfo.REASON_USER_REQUESTED)
            }
        }
    }

    constructor(context: Context, scope: String, initialSessionId: String? = null) : this(
        preferences = context.applicationContext.getSharedPreferences("durable_conversation_turn", Context.MODE_PRIVATE),
        scope = scope,
        latestExit = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
            context.getSystemService(ActivityManager::class.java)
                .getHistoricalProcessExitReasons(context.packageName, 0, 1).firstOrNull()
        } else null,
        initialSessionId = initialSessionId,
    )

    fun begin(sessionId: String, turnId: Long) = synchronized(lock) {
        if (sessionId.isBlank()) return@synchronized
        ownerSessionId = canonicalSessionId(sessionId)
        // A session has one active main turn. Replace its previous checkpoint
        // atomically while leaving checkpoints for other sessions untouched.
        // In particular, a late terminal for the previous turn must not clear
        // the newly admitted turn.
        val ownerPrefix = prefix(scope)
        val editor = preferences.edit()
        preferences.all.keys.filter { candidate ->
            candidate.startsWith(ownerPrefix) &&
                candidate.removePrefix(ownerPrefix).substringBeforeLast('.', "") == ownerSessionId
        }.forEach(editor::remove)
        check(editor.putLong(key(scope, sessionId, turnId), turnId)
            .putString(queuedKey(scope, sessionId), queued(sessionId).filterNot { it == turnId }.joinToString(","))
            .remove(queuedOwnerKey(scope, sessionId)).commit())
    }

    fun load(): DurableConversationTurnRecord? = synchronized(lock) {
        val owner = ownerSessionId ?: return@synchronized null
        val candidates = preferences.all.keys.filter { it.startsWith(prefix(scope)) }.mapNotNull { key ->
            val suffix = key.removePrefix(prefix(scope))
            val separator = suffix.lastIndexOf('.')
            if (separator <= 0) null else {
                val id = suffix.substring(0, separator)
                val turn = suffix.substring(separator + 1).toLongOrNull()
                if (turn == null || owner != id) null
                else DurableConversationTurnRecord(scope, id, turn,
                    preferences.getLong(queuedOwnerKey(scope, id), Long.MIN_VALUE) == turn)
            }
        }
        candidates.singleOrNull()
    }

    fun clear(turnId: Long? = null) = synchronized(lock) {
        val record = load() ?: return@synchronized
        if (turnId == null || record.turnId == turnId) clear(record.sessionId, record.turnId)
    }

    fun clear(sessionId: String, turnId: Long) = synchronized(lock) {
        if (ownerSessionId != canonicalSessionId(sessionId)) return@synchronized
        check(preferences.edit().remove(key(scope, sessionId, turnId))
            .also { if (preferences.getLong(queuedOwnerKey(scope, sessionId), Long.MIN_VALUE) == turnId) it.remove(queuedOwnerKey(scope, sessionId)) }
            .putString(queuedKey(scope, sessionId), queued(sessionId).filterNot { it == turnId }.joinToString(",")).commit())
    }

    /** Clear only the checkpoint proven not admitted by this session's Send. */
    fun clearNotAdmitted(sessionId: String, turnId: Long): Boolean = synchronized(lock) {
        if (ownerSessionId != canonicalSessionId(sessionId)) return@synchronized false
        val recordKey = key(scope, sessionId, turnId)
        !preferences.contains(recordKey) || preferences.edit().remove(recordKey).commit()
    }

    fun selectSession(sessionId: String) = synchronized(lock) { ownerSessionId = canonicalSessionId(sessionId) }

    private fun queued(sessionId: String): List<Long> = preferences.getString(queuedKey(scope, sessionId), "").orEmpty()
        .takeIf { it.isNotEmpty() }?.split(',')?.map { checkNotNull(it.toLongOrNull()) { "Invalid queued conversation identity" } }
        ?.distinct().orEmpty()

    fun state(sessionId: String): DurableConversationTurnState = synchronized(lock) {
        if (ownerSessionId == null) ownerSessionId = canonicalSessionId(sessionId)
        if (ownerSessionId != canonicalSessionId(sessionId)) return@synchronized DurableConversationTurnState(null, emptyList())
        DurableConversationTurnState(load(), queued(sessionId))
    }

    fun enqueue(sessionId: String, ownerTurnId: Long, turnId: Long) = synchronized(lock) {
        check(ownerSessionId == canonicalSessionId(sessionId) && load()?.turnId == ownerTurnId) { "Conversation owner changed before queued submission" }
        val ids = queued(sessionId)
        check(preferences.edit().putString(queuedKey(scope, sessionId), (ids + turnId).distinct().joinToString(",")).commit())
    }

    fun removeQueued(sessionId: String, turnId: Long) = synchronized(lock) {
        check(ownerSessionId == canonicalSessionId(sessionId)) { "Conversation owner changed" }
        check(preferences.edit().putString(queuedKey(scope, sessionId), queued(sessionId).filterNot { it == turnId }.joinToString(",")).commit())
    }

    /** Native proof may name the executor of a retained coalesced member. */
    fun adoptQueuedOwner(sessionId: String, memberTurnId: Long, ownerTurnId: Long, deferPrevious: Boolean = false) = synchronized(lock) {
        val state = state(sessionId)
        check(memberTurnId in state.queuedTurnIds || state.active?.turnId == ownerTurnId) { "Queued executor has no retained admission" }
        var ids = state.queuedTurnIds.filterNot { it == ownerTurnId }
        if (deferPrevious) state.active?.turnId?.takeIf { it != ownerTurnId }?.let { ids = (listOf(it) + ids).distinct() }
        val editor = preferences.edit()
        state.active?.let { editor.remove(key(scope, sessionId, it.turnId)) }
        check(editor.putLong(key(scope, sessionId, ownerTurnId), ownerTurnId)
            .putLong(queuedOwnerKey(scope, sessionId), ownerTurnId)
            .putString(queuedKey(scope, sessionId), ids.joinToString(",")).commit())
    }
}

/** UI-owned history kept only in the live source while its current turn runs. */
data class RetainedConversationTranscript(
    val sessionId: String,
    val turnId: Long?,
    val state: ChatState,
    val afterSequence: Long,
    val turnToken: Long,
    val turnOrigin: ConversationTurnOrigin,
    val pendingAssistantIdentity: String?,
    val optimisticRowsByToken: Map<String, String>,
    val assistantTranscriptUuidsByIdentity: Map<String, List<String>>,
    val pendingAssistantTranscriptUuids: Set<String>,
    val assistantRowsByIdentity: Map<String, String>,
    val reasoningBeforeResponse: String?,
    val assistantReasoningByIdentity: Map<String, Pair<String, String>>,
    val assistantIdentityTurnToken: Long,
)

internal class RetainedConversationTranscriptStore {
    @Volatile private var snapshot: RetainedConversationTranscript? = null

    fun save(value: RetainedConversationTranscript) {
        snapshot = value
    }

    fun clear() { snapshot = null }

    fun turnRecord(scope: String, sessionId: String?): DurableConversationTurnRecord? {
        val retained = snapshot ?: return null
        val turnId = retained.turnId ?: return null
        if (sessionId == null || canonicalSessionId(retained.sessionId) != canonicalSessionId(sessionId)) return null
        return DurableConversationTurnRecord(scope, retained.sessionId, turnId)
    }

    fun activation(current: ActivatedSession): ActivatedSession {
        val retained = snapshot?.takeIf { canonicalSessionId(it.sessionId) == canonicalSessionId(current.sessionId) }
            ?: return current
        return current.copy(transcript = retained.state.messages, transcriptComplete = retained.turnId == null,
            retainedProjection = retained)
    }
}

internal class DurableTurnReplayGate {
    private var pendingColdResumeAttachTurnId: Long? = null
    private var suppressingRetainedReplayTurnId: Long? = null
    private var awaitingTerminalConfirmationTurnId: Long? = null
    // A source can outlive its UI. Keep terminal identities for that source's
    // lifetime so an arbitrarily delayed callback cannot reopen an ended turn.
    private val terminalTurns = mutableSetOf<Pair<String, ULong>>()

    @Synchronized
    fun noteAttachRequested(
        turnId: Long,
        activation: ActivatedSession?,
        afterSequence: Long,
    ) {
        pendingColdResumeAttachTurnId =
            turnId.takeIf {
                afterSequence <= 0L &&
                    activation?.kind == SessionActivationKind.Resumed && activation.transcriptComplete
            }
        if (pendingColdResumeAttachTurnId != turnId) {
            clearTurn(turnId)
        }
    }

    @Synchronized
    fun cancelAttach(turnId: Long) {
        if (pendingColdResumeAttachTurnId == turnId) pendingColdResumeAttachTurnId = null
    }

    @Synchronized
    fun shouldForward(event: ClientEvent): Boolean {
        if (event is ClientEvent.TurnRecoveryState) {
            val snapshot = event.snapshot
            val key = canonicalSessionId(snapshot.sessionId) to snapshot.turnId
            if (snapshot.state in terminalStates) {
                terminalTurns += key
            } else if (key in terminalTurns) {
                return false
            }
        }
        return when (event) {
            is ClientEvent.TurnRecoveryState -> {
                val turnId = event.snapshot.turnId.toLong()
                val terminal = event.snapshot.state in terminalStates
                when {
                    pendingColdResumeAttachTurnId == turnId -> {
                        pendingColdResumeAttachTurnId = null
                        if (terminal) {
                            suppressingRetainedReplayTurnId = turnId
                            awaitingTerminalConfirmationTurnId = turnId
                        } else {
                            clearTurn(turnId)
                        }
                    }
                    awaitingTerminalConfirmationTurnId == turnId && terminal -> clearTurn(turnId)
                }
                true
            }
            is ClientEvent.TurnEventReplay ->
                suppressingRetainedReplayTurnId != event.turnId.toLong()
            is ClientEvent.Error -> {
                if (!isCompactTurnRejection(event.message.displayText())) clearAll()
                true
            }
            is ClientEvent.TurnEnded -> {
                clearAll()
                true
            }
            else -> true
        }
    }

    /** Serialize the decision and enqueue, so concurrent callbacks cannot invert them. */
    @Synchronized
    fun forward(event: ClientEvent, enqueue: (ClientEvent) -> Boolean): Boolean =
        shouldForward(event) && enqueue(event)

    private fun clearTurn(turnId: Long) {
        if (pendingColdResumeAttachTurnId == turnId) pendingColdResumeAttachTurnId = null
        if (suppressingRetainedReplayTurnId == turnId) suppressingRetainedReplayTurnId = null
        if (awaitingTerminalConfirmationTurnId == turnId) awaitingTerminalConfirmationTurnId = null
    }

    private fun clearAll() {
        pendingColdResumeAttachTurnId = null
        suppressingRetainedReplayTurnId = null
        awaitingTerminalConfirmationTurnId = null
    }
    private companion object {
        val terminalStates = setOf(
            TurnRecoveryStateDto.COMPLETED,
            TurnRecoveryStateDto.FAILED,
            TurnRecoveryStateDto.CANCELLED,
        )
    }
}

/**
 * Single-flight ownership for durable-turn attachment on one engine source.
 *
 * The headless service and a newly-created UI can observe the same
 * SessionResumed activation.  They must not both submit the same AttachTurn /
 * ResumeTurn pair, but a UI takeover is allowed to request a new replay from
 * its own cursor.  The key therefore includes the canonical session, turn,
 * and cursor, while ownership is tracked separately from the key.
 */
internal enum class DurableAttachOwner {
    Headless,
    Ui,
}

internal data class DurableAttachRequest(
    val sessionId: String,
    val turnId: Long,
    val afterSequence: Long,
)

/** A durable attach/resume failed after the checkpoint was already identified. */
internal class DurableAttachFailure(
    val turnId: Long,
    val phase: String,
    cause: Throwable,
) : IllegalStateException("Unable to $phase durable turn $turnId", cause)

/** Resume is needed for fresh UI/cold-headless sources, not retained takeovers. */
internal fun shouldResumeDurableTurn(
    forceAttach: Boolean,
    resumeOnUiAttach: Boolean,
): Boolean = !forceAttach || resumeOnUiAttach

internal class DurableAttachCoordinator {
    private var claimedByUi = false
    private var lastOwner: DurableAttachOwner? = null
    private var lastRequest: DurableAttachRequest? = null

    /** Claim the source for UI attachment; subsequent headless requests skip. */
    @Synchronized
    fun claimForUi() {
        claimedByUi = true
    }

    /** Release the UI claim when the Activity is gone and service monitoring resumes. */
    @Synchronized
    fun releaseToHeadless() {
        claimedByUi = false
        // A later Activity has a new collector, even at the same cursor. It
        // must receive its own recovery snapshot and replay rather than being
        // coalesced with the Activity that just released the source.
        lastOwner = null
        lastRequest = null
    }

    /**
     * Reserve one command pair. A UI takeover is intentionally a new owner,
     * even when it starts at cursor zero, because the service's SharedFlow
     * collector may have consumed that earlier replay.
     */
    @Synchronized
    fun reserve(request: DurableAttachRequest, owner: DurableAttachOwner): Boolean {
        if (owner == DurableAttachOwner.Headless && claimedByUi) return false
        if (lastOwner == owner && lastRequest == request) return false
        lastOwner = owner
        lastRequest = request
        return true
    }

    @Synchronized
    fun rollback(request: DurableAttachRequest, owner: DurableAttachOwner) {
        if (lastOwner == owner && lastRequest == request) {
            lastOwner = null
            lastRequest = null
        }
    }

    internal fun ownerForTesting(): DurableAttachOwner? = synchronized(this) { lastOwner }
}

internal class RecoveringConversationSource(
    private val pendingSource: Deferred<ConversationSource?>,
    private val strings: ConversationStrings,
    recoverySpec: ConversationRecoverySpec,
    private val releaseClaim: () -> Boolean = { ConversationHeadlessRecovery.releaseUiClaim(recoverySpec.ownerKey) },
) : ConversationSource, SlashCommandConversationSource, BackgroundRetainableConversationSource, DurableTranscriptSource, NativePromptReceiptSource {
    private val delegateScope = CoroutineScope(SupervisorJob() + Dispatchers.Default)
    private val models = MutableStateFlow(EngineModelState())
    private val sessions = MutableStateFlow(EngineSessionState.loading())
    private val workspace = MutableStateFlow<com.lingxi.code.project.SessionWorkspaceState?>(null)
    private val activeSession = MutableStateFlow<ActivatedSession?>(null)
    private val permissions = MutableStateFlow<PermissionPromptState?>(null)
    private val mcp = MutableStateFlow(emptyList<MCPServer>())
    private val coldAuthoringRecovery = MutableStateFlow(ColdAuthoringRecoveryState.NotNeeded)
    override val coldAuthoringRecoveryState: StateFlow<ColdAuthoringRecoveryState> = coldAuthoringRecovery.asStateFlow()
    override suspend fun retryColdAuthoringRecovery() {
        val delegate = pendingSource.await() ?: error("engine is not connected")
        delegate.retryColdAuthoringRecovery()
    }
    @Volatile private var retainedSource: ConversationSource? = null
    override val turnAdmissionReady: Boolean get() = retainedSource?.turnAdmissionReady == true
    internal fun acknowledgeAuthoringBinding(identity: com.lingxi.code.project.AuthoringSourceIdentity, sessionId: String): Boolean =
        (retainedSource as? EngineConversationSource)?.acknowledgeAuthoringBinding(identity, sessionId) == true
    internal fun verifiedAuthoringSource(): com.lingxi.code.project.AuthoringSourceIdentity? =
        (retainedSource as? EngineConversationSource)?.takeIf { it.turnAdmissionReady }?.authoringSourceIdentity
    internal fun pendingAuthoringSourceIdentity(): com.lingxi.code.project.AuthoringSourceIdentity? =
        (retainedSource as? EngineConversationSource)?.authoringSourceIdentity
    internal fun reserveAuthoringSelection(): Boolean =
        (retainedSource as? EngineConversationSource)?.reserveAuthoringSelection() == true
    internal fun releaseAuthoringSelection() {
        (retainedSource as? EngineConversationSource)?.releaseAuthoringSelection()
    }
    internal suspend fun authoritativeAuthoringWorkspace(): com.lingxi.code.project.SessionWorkspaceState =
        (retainedSource as? EngineConversationSource)?.authoritativeAuthoringWorkspace()
            ?: error("Authoring source is unavailable")
    private val initialRecoverySpec = recoverySpec
    override val recoverySpec: ConversationRecoverySpec
        get() = retainedSource?.recoverySpec ?: initialRecoverySpec
    private val ownershipLock = Any()
    private var closed = false
    private var releasedToRecovery = false

    init {
        delegateScope.launch {
            val delegate = pendingSource.await() ?: run {
                delegateScope.cancel()
                return@launch
            }
            val abandoned = synchronized(ownershipLock) {
                if (closed) !releasedToRecovery else {
                    retainedSource = delegate
                    null
                }
            }
            if (abandoned != null) {
                if (abandoned) delegate.close()
                delegateScope.cancel()
                return@launch
            }
            launch { delegate.modelState.collect { models.value = it } }
            launch { delegate.sessionState.collect { sessions.value = it } }
            launch { delegate.workspaceState.collect { workspace.value = it } }
            launch { delegate.activeSessionState.collect { activeSession.value = it } }
            launch { delegate.pendingPermission.collect { permissions.value = it } }
            launch { delegate.mcpServers.collect { mcp.value = it } }
            delegate.coldAuthoringRecoveryState?.let { state ->
                launch { state.collect { coldAuthoringRecovery.value = it } }
            }
        }
    }

    override val clientEvents: Flow<ClientEvent> = flow {
        pendingSource.await()?.clientEvents?.let { emitAll(it) }
    }

    override suspend fun collectClientEvents(onSubscribed: () -> Unit, onEvent: suspend (ClientEvent) -> Unit) {
        val delegate = pendingSource.await()
        if (delegate == null) onSubscribed() else delegate.collectClientEvents(onSubscribed, onEvent)
    }

    override suspend fun submitQueuedPrompt(command: ClientCommand.SendPrompt, ownerTurnId: Long,
                                           tryBeginNative: () -> Boolean, onNotAdmitted: () -> Unit) {
        val delegate = pendingSource.await() ?: error("engine is not connected")
        delegate.submitQueuedPrompt(command, ownerTurnId, tryBeginNative, onNotAdmitted)
    }

    override fun queuedExecutorFor(event: ClientEvent): Long? = retainedSource?.queuedExecutorFor(event)
    override fun queuedRecoveryOwnerFor(event: ClientEvent): Long? = retainedSource?.queuedRecoveryOwnerFor(event)
    override fun queuedProjectionSessionFor(event: ClientEvent): String? = retainedSource?.queuedProjectionSessionFor(event)
    override fun rawProjectionOwnerFor(event: ClientEvent): Long? = retainedSource?.rawProjectionOwnerFor(event)
    override fun isDurableAttachSnapshot(event: ClientEvent): Boolean = retainedSource?.isDurableAttachSnapshot(event) == true
    override fun requiresQueuedTranscriptRepair(event: ClientEvent): Boolean = retainedSource?.requiresQueuedTranscriptRepair(event) == true
    override suspend fun prepareHeadlessUiActivation(activation: ActivatedSession, onOwner: (Long, Boolean) -> Unit): ActivatedSession =
        (pendingSource.await() ?: error("engine is not connected")).prepareHeadlessUiActivation(activation, onOwner)
    override fun confirmHeadlessUiActivation(activation: ActivatedSession) { retainedSource?.confirmHeadlessUiActivation(activation) }
    override suspend fun snapshotForQueuedHandoff(): List<Message>? = retainedSource?.snapshotForQueuedHandoff()
    override fun hasQueuedTurnRecovery(): Boolean = retainedSource?.hasQueuedTurnRecovery() == true

    override val workflowProgress: Flow<WorkflowProgressUpdate> = flow {
        pendingSource.await()?.workflowProgress?.let { emitAll(it) }
    }

    internal fun bindAudioOwner(owner: com.lingxi.code.voice.audio.AudioOwnerKey) {
        delegateScope.launch { pendingSource.await()?.bindAudioOwner(owner) }
    }

    override val pendingPermission: StateFlow<PermissionPromptState?> = permissions.asStateFlow()
    override val modelState: StateFlow<EngineModelState> = models.asStateFlow()
    override val sessionState: StateFlow<EngineSessionState> = sessions.asStateFlow()
    override val workspaceState: StateFlow<com.lingxi.code.project.SessionWorkspaceState?> = workspace.asStateFlow()
    override val activeSessionState: StateFlow<ActivatedSession?> = activeSession.asStateFlow()
    override val mcpServers: StateFlow<List<MCPServer>> = mcp.asStateFlow()

    override suspend fun awaitTurnTranscriptPath(): String {
        val expected = initialRecoverySpec.transcriptPath?.takeIf(String::isNotBlank)
            ?: error("Conversation transcript is unavailable")
        val delegate = pendingSource.await() ?: error("engine is not connected")
        val actual = if (delegate is EngineConversationSource) delegate.awaitTurnTranscriptPath() else {
            val id = delegate.activeSessionState.value?.sessionId ?: error("Conversation session is not ready")
            kotlinx.coroutines.withTimeoutOrNull(20_000L) {
                delegate.workspaceState.first { state -> state != null &&
                    canonicalSessionId(state.sessionId) == canonicalSessionId(id) && state.transcriptPath.isNotBlank() }
            }?.transcriptPath ?: error("Conversation transcript is unavailable")
        }
        check(delegate.recoverySpec?.ownerKey == initialRecoverySpec.ownerKey && actual == expected) {
            "Conversation recovery source changed"
        }
        return actual
    }

    override suspend fun submitClientCommand(command: ClientCommand) {
        pendingSource.await()?.submitClientCommand(command)
    }

    override suspend fun refreshExecutionStatus() {
        pendingSource.await()?.refreshExecutionStatus()
    }

    override suspend fun attachDurableTurnForUi(afterSequence: Long) {
        pendingSource.await()?.attachDurableTurnForUi(afterSequence)
    }

    override fun initialMessages(): List<Message> = emptyList()

    override suspend fun refreshSessions() {
        pendingSource.await()?.refreshSessions()
    }

    override suspend fun resumeSession(uuid: String) {
        pendingSource.await()?.resumeSession(uuid)
    }

    override fun retainedUiActivation(activation: ActivatedSession): ActivatedSession =
        retainedSource?.retainedUiActivation(activation) ?: activation

    override fun retainUiTranscript(snapshot: RetainedConversationTranscript) {
        retainedSource?.retainUiTranscript(snapshot)
    }

    override suspend fun refreshMcpServers() {
        pendingSource.await()?.refreshMcpServers()
    }

    override suspend fun newSession() {
        val delegate = pendingSource.await() ?: error("engine is not connected")
        delegate.newSession()
    }

    override suspend fun newSession(cwd: String) {
        val delegate = pendingSource.await() ?: error("engine is not connected")
        delegate.newSession(cwd)
    }

    override suspend fun setPermissionMode(mode: String) {
        val source = pendingSource.await() ?: error("engine is not connected")
        source.setPermissionMode(mode)
    }

    override suspend fun setModel(id: String) {
        val source = pendingSource.await() ?: error("engine is not connected")
        source.setModel(id)
    }

    override suspend fun approvePermission(requestId: ULong, response: PermissionResponseDto) {
        pendingSource.await()?.approvePermission(requestId, response)
    }

    override suspend fun denyPermission(requestId: ULong) {
        pendingSource.await()?.denyPermission(requestId)
    }

    override fun submit(text: String, images: List<ImageRefDto>, turnId: Long): Flow<ReplyEvent> =
        submit(text, images, turnId, null, null)

    override fun submit(
        text: String,
        images: List<ImageRefDto>,
        turnId: Long,
        visualizationContext: VisualizationRef?,
    ): Flow<ReplyEvent> = submit(text, images, turnId, visualizationContext, null)

    override fun submit(
        text: String,
        images: List<ImageRefDto>,
        turnId: Long,
        visualizationContext: VisualizationRef?,
        promptMode: PromptModeDto?,
    ): Flow<ReplyEvent> = submitForComposer(text, images, turnId, visualizationContext, promptMode, onNotAdmitted = {})

    override fun submitForComposer(
        text: String, images: List<ImageRefDto>, turnId: Long,
        visualizationContext: VisualizationRef?, promptMode: PromptModeDto?, onNotAdmitted: () -> Unit,
        tryBeginNative: () -> Boolean,
        bindNativeIdentity: (NativePromptIdentity) -> Boolean,
        onNativeResult: (Result<Unit>) -> Unit,
    ): Flow<ReplyEvent> = flow {
        val delegate = pendingSource.await()
        if (delegate == null) {
            onNotAdmitted()
            emit(ReplyEvent.Error(strings.resolve(R.string.chat_engine_build_failed, "引擎创建失败")))
            emit(ReplyEvent.End)
            return@flow
        }
        emitAll(if (delegate is NativePromptReceiptSource)
            delegate.submitForComposer(text, images, turnId, visualizationContext, promptMode, onNotAdmitted, tryBeginNative, bindNativeIdentity, onNativeResult)
        else {
            if (!tryBeginNative()) throw kotlinx.coroutines.CancellationException("Turn cancelled before submission")
            delegate.submit(text, images, turnId, visualizationContext, promptMode)
        })
    }

    override suspend fun currentNativePromptIdentity(): NativePromptIdentity? =
        (pendingSource.await() as? NativePromptReceiptSource)?.currentNativePromptIdentity()

    override suspend fun clearNotAdmitted(identity: NativePromptIdentity, turnId: Long): Boolean =
        (pendingSource.await() as? NativePromptReceiptSource)?.clearNotAdmitted(identity, turnId) == true

    override fun submitSlashCommand(
        raw: ClientTextDto,
        images: List<ImageRefDto>,
        turnId: Long,
    ): Flow<ReplyEvent> = submitSlashForComposer(raw, images, turnId, onNotAdmitted = {})

    internal fun submitSlashForComposer(
        raw: ClientTextDto, images: List<ImageRefDto>, turnId: Long, onNotAdmitted: () -> Unit,
        tryBeginNative: () -> Boolean = { true },
    ): Flow<ReplyEvent> = flow {
        val delegate = pendingSource.await() as? SlashCommandConversationSource
        if (delegate == null) {
            onNotAdmitted()
            emit(ReplyEvent.Error(strings.resolve(R.string.chat_engine_build_failed, "引擎创建失败")))
            emit(ReplyEvent.End)
            return@flow
        }
        emitAll(if (delegate is EngineConversationSource)
            delegate.submitSlashForComposer(raw, images, turnId, onNotAdmitted, tryBeginNative)
        else {
            if (!tryBeginNative()) throw kotlinx.coroutines.CancellationException("Turn cancelled before submission")
            delegate.submitSlashCommand(raw, images, turnId)
        })
    }

    override suspend fun cancel(turnId: Long?) {
        pendingSource.await()?.cancel(turnId)
    }

    override suspend fun discardDurableTurn(turnId: Long) {
        pendingSource.await()?.discardDurableTurn(turnId)
    }

    override fun close() {
        val (released, delegate) = synchronized(ownershipLock) {
            closed = true
            releasedToRecovery = releaseClaim()
            releasedToRecovery to retainedSource
        }
        if (released) {
            // This wrapper borrowed a source still being built or monitored
            // by the service. Its cancellation releases only the UI claim.
            delegateScope.cancel()
        } else if (delegate != null) {
            delegate.close()
            delegateScope.cancel()
        }
    }

    override fun engineSourceForBackgroundRetention(): EngineConversationSource? =
        (retainedSource as? BackgroundRetainableConversationSource)?.engineSourceForBackgroundRetention()
}
