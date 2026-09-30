package com.lingxi.code.conversation

import android.app.ActivityManager
import android.app.ApplicationExitInfo
import android.content.Context
import android.content.SharedPreferences
import android.os.Build
import com.lingxi.code.R
import com.lingxi.code.bindings.ClientCommand
import com.lingxi.code.bindings.ClientEvent
import com.lingxi.code.bindings.ImageRefDto
import com.lingxi.code.bindings.PermissionResponseDto
import com.lingxi.code.bindings.TurnRecoveryStateDto
import com.lingxi.code.model.EngineModelState
import com.lingxi.code.model.EngineSessionState
import com.lingxi.code.model.MCPServer
import com.lingxi.code.model.Message
import com.lingxi.code.model.SessionMode
import com.lingxi.code.model.canonicalSessionId
import com.lingxi.code.project.ProjectWorkspace
import com.lingxi.code.settings.LinuxRuntimeMode
import kotlinx.coroutines.CoroutineScope
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

/** Minimum non-secret launch context needed for service redelivery recovery. */
data class ConversationRecoverySpec(
    val projectId: String?,
    val hostPath: String?,
    val sessionMode: SessionMode,
    val linuxRuntimeMode: LinuxRuntimeMode,
    val workspaceKey: String? = null,
) {
    val scopeKey: String
        get() = "${workspaceKey ?: hostPath ?: "__global__"}#${sessionMode.wireValue}"

    fun projectWorkspace(): ProjectWorkspace? = hostPath?.let { path ->
        ProjectWorkspace(projectId = projectId ?: "recovered", hostPath = path)
    }
}

internal class DurableConversationTurnClientStore(
    private val preferences: SharedPreferences,
    private val scope: String,
    latestExit: ApplicationExitInfo? = null,
) {
    init {
        val handledAt = preferences.getLong("handled_exit_timestamp", 0L)
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R && latestExit != null && latestExit.timestamp > handledAt) {
            if (latestExit.reason == ApplicationExitInfo.REASON_USER_REQUESTED) {
                clearRecord()
            }
            preferences.edit().putLong("handled_exit_timestamp", latestExit.timestamp).apply()
        }
    }

    constructor(
        context: Context,
        scope: String,
    ) : this(
        preferences = context.applicationContext.getSharedPreferences(
            "durable_conversation_turn",
            Context.MODE_PRIVATE,
        ),
        scope = scope,
        latestExit = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
            context.getSystemService(ActivityManager::class.java)
                .getHistoricalProcessExitReasons(context.packageName, 0, 1)
                .firstOrNull()
        } else {
            null
        },
    )

    fun begin(sessionId: String, turnId: Long) {
        if (sessionId.isBlank()) return
        preferences.edit()
            .putString("scope", scope)
            .putString("session_id", sessionId)
            .putLong("turn_id", turnId)
            .commit()
    }

    fun load(): DurableConversationTurnRecord? {
        if (preferences.getString("scope", null) != scope) return null
        val sessionId = preferences.getString("session_id", null)?.takeIf(String::isNotBlank)
            ?: return null
        if (!preferences.contains("turn_id")) return null
        return DurableConversationTurnRecord(
            scope = scope,
            sessionId = sessionId,
            turnId = preferences.getLong("turn_id", 0L),
        )
    }

    fun clear(turnId: Long? = null) {
        val current = load() ?: return
        if (turnId == null || current.turnId == turnId) {
            clearRecord()
        }
    }

    /**
     * Clear only the exact session/turn checkpoint that produced a terminal
     * event.  Turn ids are client supplied, so the session comparison is
     * required as well: a late terminal event from a previous session must not
     * erase the checkpoint belonging to the current session.
     */
    fun clear(sessionId: String, turnId: Long) {
        val current = load() ?: return
        if (
            current.turnId == turnId &&
            canonicalSessionId(current.sessionId) == canonicalSessionId(sessionId)
        ) {
            clearRecord()
        }
    }

    private fun clearRecord() {
        preferences.edit()
            .remove("scope")
            .remove("session_id")
            .remove("turn_id")
            .remove("last_sequence")
            .commit()
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
                    activation?.kind == SessionActivationKind.Resumed
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
        is ClientEvent.TurnEnded, is ClientEvent.Error -> {
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
    private val pendingSource: kotlinx.coroutines.CompletableDeferred<EngineConversationSource?>,
    private val strings: ConversationStrings,
    override val recoverySpec: ConversationRecoverySpec,
) : ConversationSource, BackgroundRetainableConversationSource {
    private val delegateScope = CoroutineScope(SupervisorJob() + Dispatchers.Default)
    private val models = MutableStateFlow(EngineModelState())
    private val sessions = MutableStateFlow(EngineSessionState.loading())
    private val activeSession = MutableStateFlow<ActivatedSession?>(null)
    private val permissions = MutableStateFlow<PermissionPromptState?>(null)
    private val mcp = MutableStateFlow(emptyList<MCPServer>())
    @Volatile private var retainedSource: EngineConversationSource? = null
    @Volatile private var closed = false

    init {
        delegateScope.launch {
            val delegate = pendingSource.await() ?: run {
                delegateScope.cancel()
                return@launch
            }
            if (closed) {
                delegate.close()
                delegateScope.cancel()
                return@launch
            }
            retainedSource = delegate
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

    override val workflowProgress: Flow<WorkflowProgressUpdate> = flow {
        pendingSource.await()?.workflowProgress?.let { emitAll(it) }
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

    override suspend fun resumeEmptySession(uuid: String, title: String) {
        pendingSource.await()?.resumeEmptySession(uuid, title)
    }

    override suspend fun refreshMcpServers() {
        pendingSource.await()?.refreshMcpServers()
    }

    override suspend fun newSession() {
        pendingSource.await()?.newSession()
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

    override fun submit(text: String, images: List<ImageRefDto>, turnId: Long): Flow<ReplyEvent> = flow {
        val delegate = pendingSource.await()
        if (delegate == null) {
            emit(ReplyEvent.Error(strings.resolve(R.string.chat_engine_build_failed, "引擎创建失败")))
            emit(ReplyEvent.End)
            return@flow
        }
        emitAll(delegate.submit(text, images, turnId))
    }

    override suspend fun cancel(turnId: Long?) {
        pendingSource.await()?.cancel(turnId)
    }

    override suspend fun discardDurableTurn(turnId: Long) {
        pendingSource.await()?.discardDurableTurn(turnId)
    }

    override fun close() {
        closed = true
        retainedSource?.let {
            it.close()
            delegateScope.cancel()
        }
    }

    override fun engineSourceForBackgroundRetention(): EngineConversationSource? = retainedSource
}
