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
import kotlinx.coroutines.flow.flow
import kotlinx.coroutines.launch

internal data class DurableConversationTurnRecord(
    val scope: String,
    val sessionId: String,
    val turnId: Long,
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

        fun ownerModeForSession(context: Context, workspaceKey: String, sessionId: String?): SessionMode? {
            val prefs = context.applicationContext.getSharedPreferences("durable_conversation_turn", Context.MODE_PRIVATE)
            return ownerModeForSession(prefs, workspaceKey, sessionId)
        }

        internal fun ownerModeForSession(prefs: SharedPreferences, workspaceKey: String, sessionId: String?): SessionMode? = synchronized(lock) {
            if (sessionId.isNullOrBlank()) return@synchronized null
            val id = canonicalSessionId(sessionId)
            val matches = SessionMode.entries.filter { mode ->
                val scope = "$workspaceKey#${mode.wireValue}"
                prefs.all.keys.any { it.startsWith(prefix(scope) + id + ".") }
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
        check(editor.putLong(key(scope, sessionId, turnId), turnId).commit())
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
                else DurableConversationTurnRecord(scope, id, turn)
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
        preferences.edit().remove(key(scope, sessionId, turnId)).commit()
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
    fun shouldForward(event: ClientEvent): Boolean = when (event) {
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
    override val recoverySpec: ConversationRecoverySpec,
    private val releaseClaim: () -> Boolean = { ConversationHeadlessRecovery.releaseUiClaim(recoverySpec.ownerKey) },
) : ConversationSource, SlashCommandConversationSource, BackgroundRetainableConversationSource {
    private val delegateScope = CoroutineScope(SupervisorJob() + Dispatchers.Default)
    private val models = MutableStateFlow(EngineModelState())
    private val sessions = MutableStateFlow(EngineSessionState.loading())
    private val activeSession = MutableStateFlow<ActivatedSession?>(null)
    private val permissions = MutableStateFlow<PermissionPromptState?>(null)
    private val mcp = MutableStateFlow(emptyList<MCPServer>())
    @Volatile private var retainedSource: ConversationSource? = null
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
            launch { delegate.activeSessionState.collect { activeSession.value = it } }
            launch { delegate.pendingPermission.collect { permissions.value = it } }
            launch { delegate.mcpServers.collect { mcp.value = it } }
        }
    }

    override val clientEvents: Flow<ClientEvent> = flow {
        pendingSource.await()?.clientEvents?.let { emitAll(it) }
    }

    override suspend fun collectClientEvents(onSubscribed: () -> Unit, onEvent: suspend (ClientEvent) -> Unit) {
        val delegate = pendingSource.await()
        if (delegate == null) onSubscribed() else delegate.collectClientEvents(onSubscribed, onEvent)
    }

    override val workflowProgress: Flow<WorkflowProgressUpdate> = flow {
        pendingSource.await()?.workflowProgress?.let { emitAll(it) }
    }

    internal fun bindAudioOwner(owner: com.lingxi.code.voice.audio.AudioOwnerKey) {
        delegateScope.launch { pendingSource.await()?.bindAudioOwner(owner) }
    }

    override val pendingPermission: StateFlow<PermissionPromptState?> = permissions.asStateFlow()
    override val modelState: StateFlow<EngineModelState> = models.asStateFlow()
    override val sessionState: StateFlow<EngineSessionState> = sessions.asStateFlow()
    override val activeSessionState: StateFlow<ActivatedSession?> = activeSession.asStateFlow()
    override val mcpServers: StateFlow<List<MCPServer>> = mcp.asStateFlow()

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
    ): Flow<ReplyEvent> = flow {
        val delegate = pendingSource.await()
        if (delegate == null) {
            emit(ReplyEvent.Error(strings.resolve(R.string.chat_engine_build_failed, "引擎创建失败")))
            emit(ReplyEvent.End)
            return@flow
        }
        emitAll(delegate.submit(text, images, turnId, visualizationContext, promptMode))
    }

    override fun submitSlashCommand(
        raw: ClientTextDto,
        images: List<ImageRefDto>,
        turnId: Long,
    ): Flow<ReplyEvent> = flow {
        val delegate = pendingSource.await() as? SlashCommandConversationSource
        if (delegate == null) {
            emit(ReplyEvent.Error(strings.resolve(R.string.chat_engine_build_failed, "引擎创建失败")))
            emit(ReplyEvent.End)
            return@flow
        }
        emitAll(delegate.submitSlashCommand(raw, images, turnId))
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
