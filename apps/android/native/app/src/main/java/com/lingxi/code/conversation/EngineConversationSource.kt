package com.lingxi.code.conversation

import android.content.Context
import android.os.Build
import com.lingxi.code.R
import com.lingxi.code.bindings.client.ClientCommand
import com.lingxi.code.bindings.client.ClientEvent
import com.lingxi.code.bindings.client.ClientException
import com.lingxi.code.bindings.client.ClientTextDto
import com.lingxi.code.bindings.client.ErrorKindDto
import com.lingxi.code.bindings.client.ImageRefDto
import com.lingxi.code.bindings.client.ListingKindDto
import com.lingxi.code.bindings.client.PromptModeDto
import com.lingxi.code.bindings.runtime.ProviderConnectionTestDto
import com.lingxi.code.bindings.client.ProviderCredentialSecretDto
import com.lingxi.code.bindings.runtime.MobileEngineHandle
import com.lingxi.code.bindings.runtime.VisualizationHost
import com.lingxi.code.bindings.android.AndroidNativeMediaHost
import com.lingxi.code.bindings.client.PermissionResponseDto
import com.lingxi.code.bindings.client.TurnRecoveryStateDto
import com.lingxi.code.bindings.client.decodeTurnEventEnvelope
import com.lingxi.code.model.EngineModelState
import com.lingxi.code.model.EngineSessionState
import com.lingxi.code.model.MCPServer
import com.lingxi.code.model.Message
import com.lingxi.code.model.ReasoningControl
import com.lingxi.code.model.SessionMode
import com.lingxi.code.model.canonicalSessionId
import com.lingxi.code.model.sessionCatalogStrings
import com.lingxi.code.project.ProjectWorkspace
import com.lingxi.code.settings.LinuxRuntimeMode
import com.lingxi.code.settings.ProviderSettingsRepository
import com.lingxi.code.voice.buildVoiceEngine
import com.lingxi.code.voice.audio.AndroidNativeMediaHostProvider
import com.lingxi.code.voice.resolveEngineEnvironmentOverrides
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.NonCancellable
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.async
import kotlinx.coroutines.cancel
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.SharedFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.emitAll
import kotlinx.coroutines.flow.filter
import kotlinx.coroutines.flow.flow
import kotlinx.coroutines.flow.mapNotNull
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch
import kotlinx.coroutines.withTimeout
import kotlinx.coroutines.withTimeoutOrNull
import kotlinx.coroutines.withContext
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock

/**
 * The real conversation source: an in-process engine reached over UniFFI.
 *
 * Owns the single [MobileEngineHandle] (built once via [buildVoiceEngine], which
 * wires every device-capability adapter through the FFI seam) and the single
 * registered `AndroidEventListener`. The listener pushes every inbound
 * [ClientEvent] into [events]; [submit] fires the `SendPrompt` command and
 * returns a [Flow] that maps the shared stream through [clientEventToReply],
 * completing on [ReplyEvent.End] / [ReplyEvent.Error].
 *
 * The handle is built eagerly in the constructor (the factory below catches a
 * build failure and falls back to the mock, mirroring iOS) so this type is only
 * ever instantiated when the engine is actually available. The listener is
 * passed INTO [buildVoiceEngine] so the source is the sole owner of the
 * handle+listener — no second build, no dropped events. Ordinary prompts use
 * `SendPrompt`; main slash commands use `RunSlashCommand` so display results
 * and prompt-expanding commands retain the host's command lifecycle.
 *
 * NOTE: this type touches the UniFFI bindings + Android `Context`, so it is NOT
 * exercised by JVM unit tests. The pure [clientEventToReply] mapper carries the
 * mapping coverage; this wiring is covered by the on-device integration build.
 */
internal class MediaAudioSessionContextBindings(
    private val removeContext: (String, ULong) -> Unit,
) {
    private val lock = Any()
    private val generations = mutableMapOf<String, ULong>()
    private var closed = false

    fun register(sessionId: String, generation: ULong) {
        val removeImmediately = synchronized(lock) {
            if (closed) true else {
                generations[sessionId] = generation
                false
            }
        }
        if (removeImmediately) {
            runCatching { removeContext(sessionId, generation) }
        }
    }

    fun remove(sessionId: String) {
        val generation = synchronized(lock) { generations.remove(sessionId) } ?: return
        runCatching { removeContext(sessionId, generation) }
    }

    fun close() {
        val registrations = synchronized(lock) {
            closed = true
            generations.toList().also { generations.clear() }
        }
        registrations.forEach { (sessionId, generation) ->
            runCatching { removeContext(sessionId, generation) }
        }
    }
}

class EngineConversationSource private constructor(
    private val handle: MobileEngineHandle,
    private val mediaHost: AndroidNativeMediaHost,
    private val mediaContextBindings: MediaAudioSessionContextBindings,
    private val events: Flow<ClientEvent>,
    private val eventRelay: LosslessEventRelay<ConversationDelivery>,
    private val workflowEvents: SharedFlow<WorkflowProgressUpdate>,
    private val workflowRelay: LosslessEventRelay<WorkflowProgressUpdate>,
    private val eventScope: CoroutineScope,
    private val permissions: MutableStateFlow<PermissionPromptState?>,
    private val permissionIngress: PermissionIngress,
    private val models: MutableStateFlow<EngineModelState>,
    private val sessions: MutableStateFlow<EngineSessionState>,
    private val workspace: MutableStateFlow<com.lingxi.code.project.SessionWorkspaceState?>,
    private val activeSession: MutableStateFlow<ActivatedSession?>,
    private val mcp: MutableStateFlow<List<MCPServer>>,
    private val strings: ConversationStrings,
    private val durableTurns: DurableConversationTurnClientStore,
    private val durableReplayGate: DurableTurnReplayGate,
    private val eventIngress: DurableRecoveryIngress.Source,
    private val turnIngressInterest: DurableTurnIngressInterest,
    private val attachIngress: DurableAttachIngress,
    private val attachConsumers: AttachConsumerRegistry,
    private val beginTurnInterest: (String, Long, Boolean) -> Boolean,
    private val retireTurnInterest: (String, ULong) -> Unit,
    private val finishAttachInterest: (DurableAttachIngress.Attempt) -> Unit,
    private val onCancellationDrained: (String, Long) -> Unit,
    private val onCancellationFailed: (String, Long) -> Unit,
    private val autoAttachDurableTurns: Boolean,
    @Volatile private var resumeOnUiAttach: Boolean,
    private val initialRecoverySpec: ConversationRecoverySpec,
    private val userHomePath: String,
    private val userHomeGuestPath: String,
) : ConversationSource, SlashCommandConversationSource, BackgroundRetainableConversationSource {
    override val orderedCancellationBoundaries: Boolean get() = true
    override val supportsSubmissionAdmission: Boolean get() = true

    override val recoverySpec: ConversationRecoverySpec
        get() {
            val directory = workspace.value?.cwd ?: return initialRecoverySpec
            val home = userHomeGuestPath
            if (directory != home && !directory.startsWith("$home/")) return initialRecoverySpec
            val relative = directory.removePrefix(home).removePrefix("/")
            return initialRecoverySpec.copy(hostPath = java.io.File(userHomePath, relative).absolutePath, guestPath = directory, additionalDirectories = workspace.value?.additionalDirectories.orEmpty())
        }

    override val workspaceState = workspace.asStateFlow()

    internal fun newSideChatSource() = SideChatSource(handle)

    private val durableAttachMutex = Mutex()
    private val durableAttachCoordinator = DurableAttachCoordinator()

    init {
        if (autoAttachDurableTurns) {
            eventScope.launch {
                activeSession.collect { activated ->
                    val record = durableTurns.load() ?: return@collect
                    if (
                        activated != null &&
                        canonicalSessionId(activated.sessionId) == canonicalSessionId(record.sessionId)
                    ) {
                        attachAndResume(record, forceAttach = false)
                    }
                }
            }
        }
    }

    /** UI takes over attachment, while the service remains a passive observer. */
    internal fun claimDurableTurnForUi() {
        durableAttachCoordinator.claimForUi()
    }

    /**
     * Update whether a UI Attach must also Resume. This is changed by the
     * process coordinator when ownership moves between a retained headless
     * executor and a fresh UI source; the immutable construction path alone
     * cannot distinguish those lifecycles.
     */
    internal fun setUiAttachResumeRequired(required: Boolean) {
        resumeOnUiAttach = required
    }

    /** Allow service redelivery to resume ownership after the UI is destroyed. */
    internal fun releaseDurableTurnToHeadless() {
        durableAttachCoordinator.releaseToHeadless()
    }

    override val clientEvents: Flow<ClientEvent> = events
    override val conversationDeliveries: Flow<ConversationDelivery> = attachConsumers.track(eventRelay.events)
    override val workflowProgress: Flow<WorkflowProgressUpdate> = workflowEvents

    override suspend fun submitClientCommand(command: ClientCommand) {
        val queuedPrompt = command as? ClientCommand.SendPrompt
        val queuedTurn = queuedPrompt?.turnId
        if (queuedTurn != null) {
            val owner = queuedPrompt.sessionId?.takeIf(String::isNotBlank)
                ?: throw IllegalArgumentException("A main conversation prompt needs its origin session")
            val prepared = kotlinx.coroutines.CompletableDeferred<Boolean>()
            if (!eventIngress.dispatchBoundary {
                val valid = queuedTurn <= Long.MAX_VALUE.toULong() &&
                    activeSession.value?.sessionId?.let(::canonicalSessionId) == canonicalSessionId(owner) &&
                    durableTurns.enqueue(owner, queuedTurn.toLong())
                if (valid) {
                    turnIngressInterest.enqueue(owner, queuedTurn)
                }
                prepared.complete(valid)
            }) prepared.complete(false)
            if (!prepared.await()) {
                if (!durableTurns.isHealthy()) eventRelay.offer(ConversationDelivery.RecoveryStorageFailed)
                throw ClientException.SubmissionNotAdmitted("The queued prompt was not submitted")
            }
        }
        try {
            handle.submit(command)
        } catch (error: Throwable) {
            if (queuedTurn != null && error is ClientException.SubmissionNotAdmitted) eventIngress.dispatchBoundary {
                turnIngressInterest.removeQueued(queuedTurn)
                queuedPrompt.sessionId?.let { durableTurns.removeQueued(it, queuedTurn.toLong()) }
                if (!durableTurns.isHealthy()) eventRelay.offer(ConversationDelivery.RecoveryStorageFailed)
            }
            throw error
        }
    }

    override suspend fun testProviderConnection(
        providerId: String,
        providerPreset: String,
        apiBase: String,
        model: String,
        credentialOverride: ProviderCredentialSecretDto?,
    ): ProviderConnectionTestDto {
        return handle.testProviderConnection(providerId, providerPreset, apiBase, model, credentialOverride)
    }

    override suspend fun refreshExecutionStatus() {
        handle.submit(ClientCommand.TaskList(statusFilter = null, requestId = null))
        handle.submit(ClientCommand.ListSessionAgents)
    }

    override suspend fun attachDurableTurnForUi(
        afterSequence: Long,
        onAttach: (String, Long, Boolean, String) -> Unit,
    ) {
        val record = durableTurns.load() ?: return
        if (
            activeSession.value?.let { canonicalSessionId(it.sessionId) } ==
                canonicalSessionId(record.sessionId)
        ) {
            attachAndResume(record, forceAttach = true, afterSequence = afterSequence, onAttach = onAttach)
        }
    }

    override suspend fun reconcileDurableTurnForUi(sessionId: String, turnId: Long, afterSequence: Long, trustNotFound: Boolean,
        onAttach: (String, Long, Boolean, String) -> Unit) {
        val record = durableTurns.load()?.takeIf {
            it.turnId == turnId && canonicalSessionId(it.sessionId) == canonicalSessionId(sessionId)
        } ?: error("The recovery record is unavailable")
        check(activeSession.value?.sessionId?.let(::canonicalSessionId) == canonicalSessionId(sessionId)) {
            "Session changed before recovery query"
        }
        attachAndResume(record, forceAttach = true, afterSequence = afterSequence, onAttach = onAttach,
            resumeOverride = false, trustNotFound = trustNotFound)
    }

    override suspend fun terminalRecoveryReceipts(sessionId: String): List<Long> =
        durableTurns.terminalReceipts(sessionId).map { it.turnId }
    override suspend fun cancelledQueuedReceipts(sessionId: String): List<Long> =
        durableTurns.terminalReceipts(sessionId).filter { it.cancelledBeforeStart }.map { it.turnId }
    override suspend fun recoveryStorageAvailable(): Boolean = durableTurns.isHealthy()

    override suspend fun pendingQueuedRecoveryTurns(sessionId: String): List<Long> =
        durableTurns.queued(sessionId).map { it.turnId }

    override suspend fun queryQueuedRecovery(sessionId: String, turnId: Long, requestId: String) {
        withTimeout(10_000) { attachConsumers.awaitSubscriber() }
        handle.submit(ClientCommand.QueryQueuedTurn(sessionId = sessionId, turnId = turnId.toULong(), requestId = requestId))
    }

    override suspend fun resolveQueuedRecovery(sessionId: String, turns: List<Long>, resolution: QueuedRecoveryResolution): Boolean {
        val result = kotlinx.coroutines.CompletableDeferred<Boolean>()
        if (!eventIngress.dispatchBoundary {
            val active = activeSession.value?.sessionId?.let(::canonicalSessionId) == canonicalSessionId(sessionId)
            val saved = active && turns.all { turn ->
                val done = when (resolution) {
                    QueuedRecoveryResolution.Delivered -> durableTurns.removeDeliveredQueued(sessionId, turn)
                    QueuedRecoveryResolution.CancelledBeforeStart -> durableTurns.finishQueued(sessionId, turn, cancelledBeforeStart = true)
                    QueuedRecoveryResolution.Terminal -> durableTurns.finishQueued(sessionId, turn)
                }
                if (done) {
                    turnIngressInterest.removeQueued(turn.toULong())
                    eventRelay.offer(ConversationDelivery.QueuedTurnSettled(sessionId, turn.toULong(),
                        reviewRequired = resolution != QueuedRecoveryResolution.Delivered,
                        cancelledBeforeStart = resolution == QueuedRecoveryResolution.CancelledBeforeStart))
                }
                done
            }
            if (!durableTurns.isHealthy()) eventRelay.offer(ConversationDelivery.RecoveryStorageFailed)
            result.complete(saved)
        }) result.complete(false)
        return result.await()
    }

    override suspend fun acknowledgeTerminalRecovery(sessionId: String, turns: List<Long>): Boolean {
        val result = kotlinx.coroutines.CompletableDeferred<Boolean>()
        if (!eventIngress.dispatchBoundary {
            val current = turnIngressInterest.currentIdentity()
            val ready = turns.filterNot { current == (canonicalSessionId(sessionId) to it.toULong()) }
            val saved = durableTurns.acknowledgeTerminals(sessionId, ready)
            if (!durableTurns.isHealthy()) eventRelay.offer(ConversationDelivery.RecoveryStorageFailed)
            result.complete(saved)
        }) result.complete(false)
        return result.await()
    }

    private suspend fun attachAndResume(
        record: DurableConversationTurnRecord,
        forceAttach: Boolean,
        afterSequence: Long = 0L,
        onAttach: (String, Long, Boolean, String) -> Unit = { _, _, _, _ -> },
        resumeOverride: Boolean? = null,
        trustNotFound: Boolean = false,
    ) = durableAttachMutex.withLock {
        val latest = durableTurns.load()?.takeIf {
            it.turnId == record.turnId && canonicalSessionId(it.sessionId) == canonicalSessionId(record.sessionId)
        } ?: return@withLock
        // A second Activity can claim the shared source while AttachTurn is
        // suspended. Snapshot the first claim's disposition before yielding;
        // that claim must still consume its required Resume exactly once.
        val resumeForThisAttach = resumeOnUiAttach
        val owner = if (forceAttach) DurableAttachOwner.Ui else DurableAttachOwner.Headless
        val request = DurableAttachRequest(
            sessionId = canonicalSessionId(latest.sessionId),
            turnId = latest.turnId,
            afterSequence = afterSequence.coerceAtLeast(0L),
        )
        if (!durableAttachCoordinator.reserve(request, owner)) return@withLock
        val expectsResume = resumeOverride ?: shouldResumeDurableTurn(forceAttach, resumeForThisAttach, latest.admissionPending)
        val requestId = java.util.UUID.randomUUID().toString()
        val begun = kotlinx.coroutines.CompletableDeferred<DurableAttachIngress.Attempt>()
        if (!eventIngress.dispatchBoundary {
            val existingOwner = attachIngress.hasEstablishedOwner(latest.sessionId, latest.turnId.toULong())
            if (!beginTurnInterest(latest.sessionId, latest.turnId, true)) {
                begun.completeExceptionally(IllegalStateException("Cannot acquire recovery interest"))
                return@dispatchBoundary
            }
            begun.complete(attachIngress.begin(latest.sessionId, latest.turnId.toULong(), requestId, forceAttach,
                expectsResume, existingOwner, latest.admissionPending))
        }) begun.completeExceptionally(IllegalStateException("Recovery source has retired"))
        val attempt = try { begun.await() } catch (error: Throwable) {
            durableAttachCoordinator.rollback(request, owner)
            throw DurableAttachFailure(latest.turnId, "attach", error)
        }
        if (forceAttach) onAttach(latest.sessionId, latest.turnId, expectsResume, requestId)
        durableReplayGate.noteAttachRequested(
            sessionId = latest.sessionId,
            turnId = latest.turnId,
            activation = activeSession.value?.takeIf { canonicalSessionId(it.sessionId) == canonicalSessionId(latest.sessionId) },
            afterSequence = request.afterSequence,
        )
        try {
            if (forceAttach) withTimeout(10_000) { attachConsumers.awaitSubscriber() }
            if (forceAttach && latest.admissionPending) eventIngress.dispatchBoundary {
                eventRelay.offer(ConversationDelivery.AdmissionPending(latest.sessionId, latest.turnId, requestId,
                    eventIngress.fence(latest.sessionId, latest.turnId.toULong())))
            }
            handle.submit(
                ClientCommand.AttachTurn(
                    sessionId = latest.sessionId,
                    turnId = latest.turnId.toULong(),
                    afterSequence = request.afterSequence.toULong(),
                    requestId = requestId,
                ),
            )
            check(withTimeout(10_000) { attempt.consumed.await() }) { "Attach replay was not safely consumed" }
        } catch (error: Throwable) {
            if (forceAttach && trustNotFound && error is ClientException.NotFound) {
                val offered = eventIngress.dispatchBoundary {
                    val barrier = attachConsumers.barrier(attempt.consumed)
                    attempt.barrier = barrier
                    if (!eventRelay.offer(ConversationDelivery.ReconciliationNotFound(
                            latest.sessionId, latest.turnId, requestId, barrier,
                            isActive = eventIngress.fence(latest.sessionId, latest.turnId.toULong()),
                        ))) attempt.consumed.complete(false)
                }
                val consumed = offered && withContext(NonCancellable) {
                    withTimeoutOrNull(10_000) { attempt.consumed.await() } == true
                }
                if (consumed) {
                    eventIngress.dispatchBoundary {
                        if (durableTurns.clear(latest.sessionId, latest.turnId)) {
                            attachIngress.end(attempt)
                            retireTurnInterest(latest.sessionId, latest.turnId.toULong())
                        } else if (!durableTurns.isHealthy()) eventRelay.offer(ConversationDelivery.RecoveryStorageFailed)
                    }
                    attachConsumers.forget(attempt.barrier)
                    durableReplayGate.cancelAttach(latest.sessionId, latest.turnId)
                    durableAttachCoordinator.rollback(request, owner)
                    return@withLock
                }
            }
            attachConsumers.forget(attempt.barrier)
            eventIngress.dispatchBoundary { finishAttachInterest(attempt) }
            durableReplayGate.cancelAttach(latest.sessionId, latest.turnId)
            durableAttachCoordinator.rollback(request, owner)
            ConversationHeadlessRecovery.markDurableAttachFailed(recoverySpec.scopeKey, this)
            val failure = DurableAttachFailure(latest.turnId, "attach", error)
            if (forceAttach) failure.orderedDelivery = eventIngress.dispatchBoundary {
                eventRelay.offer(ConversationDelivery.AttachFailed(latest.sessionId, latest.turnId, failure, requestId))
            }
            throw failure
        }
        // Cold-process recovery needs ResumeTurn. A UI takeover resumes only
        // when the process coordinator reports that no headless/UI executor is
        // already attached to this source; retained-live sources only Attach.
        if (expectsResume) {
            try {
                if (activeSession.value?.sessionId?.let(::canonicalSessionId) != canonicalSessionId(latest.sessionId)) {
                    throw IllegalStateException("Session changed during recovery")
                }
                handle.submit(ClientCommand.ResumeTurn(sessionId = latest.sessionId, turnId = latest.turnId.toULong()))
            } catch (error: Throwable) {
                attachConsumers.forget(attempt.barrier)
                eventIngress.dispatchBoundary { finishAttachInterest(attempt) }
                durableReplayGate.cancelAttach(latest.sessionId, latest.turnId)
                durableAttachCoordinator.rollback(request, owner)
                ConversationHeadlessRecovery.markDurableAttachFailed(recoverySpec.scopeKey, this)
                val failure = DurableAttachFailure(latest.turnId, "resume", error)
                if (forceAttach) failure.orderedDelivery = eventIngress.dispatchBoundary {
                    eventRelay.offer(ConversationDelivery.AttachFailed(latest.sessionId, latest.turnId, failure, requestId))
                }
                throw failure
            }
            if (forceAttach) {
                ConversationHeadlessRecovery.markDurableUiExecutorActive(
                    recoverySpec.scopeKey,
                    this,
                )
            }
        }
        eventIngress.dispatchBoundary {
            if (attempt.admissionPending && !durableTurns.confirmAdmission(attempt.sessionId, attempt.turnId.toLong())) {
                eventRelay.offer(ConversationDelivery.TurnEventDecodeFailed(attempt.sessionId, attempt.turnId))
            }
            finishAttachInterest(attempt)
        }
    }

    override suspend fun setPermissionMode(mode: String) {
        require(mode in com.lingxi.code.settings.PermissionModeOptions.values)
        handle.submit(ClientCommand.SetPermissionMode(mode))
    }

    /** A fresh engine session starts empty (the engine streams the transcript). */
    override fun initialMessages(): List<Message> = emptyList()

    /**
     * The engine's REAL model catalog + active id, driven OUT-OF-BAND by the
     * listener folding `ModelList` / `ModelChanged` through [reduceModelEvent]
     * (see [create]). The picker observes this; [setModel] confirms a pick.
     */
    override val modelState: StateFlow<EngineModelState> = models.asStateFlow()

    /**
     * The engine's REAL resumable-session catalog, driven OUT-OF-BAND by the
     * listener folding `SessionList` through [reduceSessionEvent] (see [create]).
     * The drawer observes this to render real history; [resumeSession] /
     * [newSession] act on a pick.
     */
    override val sessionState: StateFlow<EngineSessionState> = sessions.asStateFlow()

    /**
     * The engine's most recent active-session transition, driven OUT-OF-BAND by
     * the listener folding the initial agent listing and New/Resume events
     * through [reduceActivatedSession] (see [create]). The ViewModel observes this to
     * adopt the real session id and, on resume, replace the transcript with the
     * rehydrated conversation. Sibling of [sessionState] (the catalog); both
     * ride the same listener, neither the per-turn stream.
     */
    override val activeSessionState: StateFlow<ActivatedSession?> = activeSession.asStateFlow()

    override val mcpServers: StateFlow<List<MCPServer>> = mcp.asStateFlow()

    override suspend fun refreshMcpServers() {
        try {
            handle.submit(ClientCommand.RefreshListings(which = listOf(ListingKindDto.MCP)))
        } catch (_: Throwable) {
            // A RefreshListings that can't be delivered leaves the MCP list as-is;
            // the settings page keeps whatever it last rendered (mock if empty).
        }
    }

    override suspend fun refreshSessions() {
        try {
            handle.submit(completeSessionListCommand())
        } catch (t: Throwable) {
            sessions.value = EngineSessionState.error(
                strings.resolve(
                    R.string.chat_error_session_list_failed,
                    "会话列表加载失败：%1\$s",
                    "${t.message ?: t::class.simpleName}",
                ),
            )
        }
    }

    override suspend fun changeWorkspace(path: String, expectedSessionId: String) {
        handle.changeWorkspace(path, expectedSessionId)
        refreshSessions()
    }

    override suspend fun addWorkspaceDirectory(path: String, expectedSessionId: String) {
        handle.addWorkspaceDirectory(path, expectedSessionId)
        refreshSessions()
    }

    override suspend fun resumeSession(uuid: String) {
        if (uuid.isBlank()) return
        checkRecoveryStorage(durableTurns.restoreParked(uuid))
        // Propagate command failures: the ViewModel must keep the composer gated
        // and surface an explicit session error instead of pretending the locally
        // selected transcript was resumed.
        sessions.value.rows.firstOrNull { canonicalSessionId(it.uuid) == canonicalSessionId(uuid) }
            ?.transcriptPath?.let { path ->
                handle.resumeWorkspaceSession(canonicalSessionId(uuid), path)
                return
            }
        handle.submit(
            ClientCommand.ResumeSession(
                sessionId = canonicalSessionId(uuid),
                cwd = null,
            ),
        )
    }

    override suspend fun resumeEmptySession(uuid: String, title: String) {
        if (uuid.isBlank()) return
        checkRecoveryStorage(durableTurns.restoreParked(uuid))
        handle.resumeEmptySession(
            sessionId = canonicalSessionId(uuid),
            title = title.ifBlank { strings.resolve(R.string.chat_new_conversation, "新对话") },
        )
    }

    override suspend fun newSession() {
        checkRecoveryStorage(durableTurns.parkCurrent())
        handle.submit(ClientCommand.NewSession(cwd = null, model = null))
    }

    override suspend fun newSession(cwd: String) {
        checkRecoveryStorage(durableTurns.parkCurrent())
        handle.submit(ClientCommand.NewSession(cwd = cwd, model = null))
    }

    override suspend fun setModel(id: String) {
        if (id.isBlank()) return
        // Let the caller surface delivery/rejection errors; only ModelChanged
        // advances modelState, so a failed switch retains the confirmed model.
        handle.submit(ClientCommand.SetModel(model = id.toClientTextDto()))
    }

    private fun checkRecoveryStorage(saved: Boolean) {
        if (saved) return
        if (!durableTurns.isHealthy()) eventRelay.offer(ConversationDelivery.RecoveryStorageFailed)
        throw IllegalStateException("Unable to preserve the previous conversation")
    }

    override suspend fun setReasoningSelection(id: String) {
        // Like setModel: only the engine's controls snapshot moves the selection.
        handle.submit(ClientCommand.SetReasoningSelection(selection = ReasoningControl.selection(id)))
    }

    /**
     * The head parked permission request, driven by the engine's outbound
     * `AndroidPermissionSink.onRequest` (registered in [create]). The UI observes
     * this and resolves it via [approvePermission] / [denyPermission].
     */
    override val pendingPermission: StateFlow<PermissionPromptState?> =
        permissions.asStateFlow()

    override suspend fun approvePermission(
        requestId: ULong,
        response: PermissionResponseDto,
    ) {
        submitPermissionResolution {
            handle.submit(ClientCommand.ApprovePermission(requestId = requestId, response = response))
        }
    }

    override suspend fun denyPermission(requestId: ULong) {
        submitPermissionResolution {
            handle.submit(ClientCommand.DenyPermission(requestId = requestId))
        }
    }

    /**
     * Submit the user's decision without speculatively mutating the queue. The
     * engine emits `PermissionRequestResolved` for approved, denied, cancelled,
     * and expired gates; that correlated event is the sole dequeue authority.
     * A delivery failure deliberately leaves the prompt available for retry.
     */
    private inline fun submitPermissionResolution(
        submit: () -> Unit,
    ) {
        try {
            submit()
        } catch (_: Throwable) {
            // Keep the request visible. The engine may still resolve it later,
            // otherwise the user can retry after reconnecting.
        }
    }

    override fun submit(text: String, images: List<ImageRefDto>): Flow<ReplyEvent> {
        val turn = DurableConversationTurnIds.next()
            ?: return flow { emit(ReplyEvent.Error(strings.resolve(R.string.chat_engine_unavailable, "移动端引擎当前不可用"))); emit(ReplyEvent.End) }
        return submit(text, images, turn)
    }

    override fun submit(text: String, images: List<ImageRefDto>, turnId: Long): Flow<ReplyEvent> =
        submit(text, images, turnId, null, null)

    override val visualizationHost: VisualizationHost? by lazy {
        runCatching { handle.visualizationHost(VISUALIZATION_ORIGIN) }.getOrNull()
    }

    override fun submit(
        text: String,
        images: List<ImageRefDto>,
        turnId: Long,
        visualizationContext: VisualizationRef?,
        promptMode: PromptModeDto?,
    ): Flow<ReplyEvent> {
        val owner = activeSession.value?.sessionId?.takeIf(String::isNotBlank)
            ?: return flow { emit(ReplyEvent.Error("Cannot start a turn without a session owner")); emit(ReplyEvent.End) }
        return submit(text, images, turnId, visualizationContext, promptMode, owner, TurnSubmissionAdmission())
    }

    override fun submit(
        text: String, images: List<ImageRefDto>, turnId: Long, visualizationContext: VisualizationRef?,
        promptMode: PromptModeDto?, sessionId: String, admission: TurnSubmissionAdmission,
    ): Flow<ReplyEvent> =
        // Subscribe-before-submit: the returned reply stream maps the shared
        // engine flow through `mapReplyStream`, but the `SendPrompt` is fired
        // from `events.onSubscription { … }` — which runs ONLY AFTER this
        // collector is registered as a subscriber of the SharedFlow. That ordering
        // closes the race the prior `flow { submit(); emitAll(events…) }` had: a
        // `TextDelta` emitted by the engine's listener thread in the window between
        // `submit` returning and the collector subscribing is no longer dropped,
        // because the collector is already subscribed before the turn is spawned.
        mapDeliveryReplyStream(
            attachConsumers.track(eventRelay.events, ui = false, onSubscribed = {
                try {
                    permissionIngress.beginTurn()
                    val owner = prepareTurnOwner(turnId, sessionId)
                    if (!admission.begin(owner)) {
                        abandonUnsubmittedTurn(owner, turnId)
                    } else {
                        val submission = eventScope.async {
                            handle.submit(ClientCommand.SendPrompt(
                                sessionId = owner,
                                text = text.toClientTextDto(),
                                promptMode = promptMode,
                                images = images,
                                turnId = turnId.toULong(),
                                visualizationContext = visualizationContext?.toDto(),
                            ))
                            if (!durableTurns.confirmAdmission(owner, turnId)) {
                                if (!durableTurns.isHealthy()) eventRelay.offer(ConversationDelivery.RecoveryStorageFailed)
                                error("Unable to save the submission state")
                            }
                        }
                        submission.invokeOnCompletion { admission.finished(it) }
                        submission.await()
                    }
                } catch (t: Throwable) {
                    permissionIngress.endTurn()
                    if (t is ClientException.SubmissionNotAdmitted) abandonUnsubmittedTurn(sessionId, turnId)
                    // A failed/cancelled submit can race host admission. Preserve
                    // any checkpoint already written until a correlated terminal.
                    // Inject the build/submit failure into the same stream the
                    // collector is already reading, so the mapper terminates it.
                    emit(
                        ConversationDelivery.Event(ClientEvent.Error(
                            kind = ErrorKindDto.TRANSPORT,
                            message = strings.resolve(
                                R.string.chat_error_engine_submit_failed,
                                "引擎错误：%1\$s",
                                "${t.message ?: t::class.simpleName}",
                            ).toClientTextDto(),
                        )),
                    )
                }
            }),
            strings,
            acceptsTurn = { eventSession, durableTurnId ->
                durableTurnId == turnId.toULong() && canonicalSessionId(eventSession) == canonicalSessionId(sessionId) &&
                    activeSession.value?.sessionId?.let(::canonicalSessionId) == canonicalSessionId(sessionId)
            },
            acceptsRaw = { it !is ClientEvent.SystemNotice },
        )

    override fun submitSlashCommand(
        raw: ClientTextDto,
        images: List<ImageRefDto>,
        turnId: Long,
    ): Flow<ReplyEvent> {
        val owner = activeSession.value?.sessionId?.takeIf(String::isNotBlank)
            ?: return flow { emit(ReplyEvent.Error("Cannot start a turn without a session owner")); emit(ReplyEvent.End) }
        return submitSlashCommand(raw, images, turnId, owner, TurnSubmissionAdmission())
    }

    override fun submitSlashCommand(raw: ClientTextDto, images: List<ImageRefDto>, turnId: Long,
        sessionId: String, admission: TurnSubmissionAdmission): Flow<ReplyEvent> =
        mapDeliveryReplyStream(
            attachConsumers.track(eventRelay.events, ui = false, onSubscribed = {
                try {
                    permissionIngress.beginTurn()
                    val owner = prepareTurnOwner(turnId, sessionId)
                    if (!admission.begin(owner)) {
                        abandonUnsubmittedTurn(owner, turnId)
                    } else {
                        val submission = eventScope.async {
                            handle.submit(runSlashCommandClientCommand(owner, raw, images, turnId))
                            if (!durableTurns.confirmAdmission(owner, turnId)) {
                                if (!durableTurns.isHealthy()) eventRelay.offer(ConversationDelivery.RecoveryStorageFailed)
                                error("Unable to save the submission state")
                            }
                        }
                        submission.invokeOnCompletion { admission.finished(it) }
                        submission.await()
                    }
                } catch (t: Throwable) {
                    permissionIngress.endTurn()
                    if (t is ClientException.SubmissionNotAdmitted) abandonUnsubmittedTurn(sessionId, turnId)
                    emit(
                        ConversationDelivery.Event(ClientEvent.Error(
                            kind = ErrorKindDto.TRANSPORT,
                            message = strings.resolve(
                                R.string.chat_error_engine_submit_failed,
                                "引擎错误：%1\$s",
                                "${t.message ?: t::class.simpleName}",
                            ).toClientTextDto(),
                        )),
                    )
                }
            }),
            strings,
            acceptSlashCommandResults = true,
            acceptsRaw = { event -> event !is ClientEvent.SystemNotice && slashCommandEventBelongsToTurn(event, turnId) },
            acceptsTurn = { eventSession, durableTurnId ->
                durableTurnId == turnId.toULong() && canonicalSessionId(eventSession) == canonicalSessionId(sessionId) &&
                    activeSession.value?.sessionId?.let(::canonicalSessionId) == canonicalSessionId(sessionId)
            },
        )

    private suspend fun prepareTurnOwner(turnId: Long, sessionId: String): String {
        val session = sessionId.takeIf(String::isNotBlank)
            ?: throw IllegalStateException("Cannot start a turn without a session owner")
        val prepared = kotlinx.coroutines.CompletableDeferred<Boolean>()
        if (!eventIngress.dispatchBoundary {
                val confirmedOwner = activeSession.value?.sessionId?.let(::canonicalSessionId) == canonicalSessionId(session)
                val restored = confirmedOwner && durableTurns.restoreParked(session)
                val existing = durableTurns.load()
                val available = restored &&
                    (existing == null || (existing.turnId == turnId &&
                        canonicalSessionId(existing.sessionId) == canonicalSessionId(session)))
                if (!available || !beginTurnInterest(session, turnId, false)) {
                    if (!durableTurns.isHealthy()) eventRelay.offer(ConversationDelivery.RecoveryStorageFailed)
                    prepared.complete(false)
                }
                else {
                    val saved = durableTurns.begin(session, turnId, admissionPending = true)
                    val stored = durableTurns.load()
                    val matches = saved && stored != null && stored.turnId == turnId && stored.admissionPending &&
                        canonicalSessionId(stored.sessionId) == canonicalSessionId(session)
                    if (!matches) {
                        if (!durableTurns.isHealthy()) eventRelay.offer(ConversationDelivery.RecoveryStorageFailed)
                        retireTurnInterest(session, turnId.toULong())
                    }
                    else attachIngress.noteLiveOwnership(session, turnId.toULong())
                    prepared.complete(matches)
                }
            }) prepared.complete(false)
        check(prepared.await()) { "A previous durable turn still owns this scope" }
        return session
    }

    override suspend fun abandonUnsubmittedTurn(sessionId: String?, turnId: Long) {
        val completed = kotlinx.coroutines.CompletableDeferred<Unit>()
        if (!eventIngress.dispatchBoundary {
                val record = durableTurns.load()
                if (record != null && record.turnId == turnId &&
                    (sessionId == null || canonicalSessionId(record.sessionId) == canonicalSessionId(sessionId))
                ) {
                    if (durableTurns.clear(record.sessionId, turnId)) retireTurnInterest(record.sessionId, turnId.toULong())
                    else if (!durableTurns.isHealthy()) eventRelay.offer(ConversationDelivery.RecoveryStorageFailed)
                }
                completed.complete(Unit)
            }) completed.complete(Unit)
        completed.await()
    }

    override suspend fun cancel() {
        cancel(null)
    }

    override suspend fun cancel(turnId: Long?) {
        val owner = activeSession.value?.sessionId?.takeIf(String::isNotBlank)
            ?: throw IllegalStateException("Cannot cancel without a session owner")
        cancelForSession(owner, turnId)
    }

    override suspend fun cancelForSession(sessionId: String, turnId: Long?) {
        // Narrow `Cancel(turnId = null)` cancels the current turn (bindings doc:
        // "None cancels the current one"). The engine emits `TurnEnded`, which
        // flows back through the active `submit` stream as `ReplyEvent.End`.
        // Do not clear permission UI speculatively here: a background workflow
        // child can own the prompt while the main turn is being cancelled. The
        // correlated PermissionRequestResolved event is the only authority that
        // removes a parked request.
        // The host intentionally returns Ok for a stale id or when no turn is
        // active.  A successful command submission therefore is not evidence
        // that this durable checkpoint was cancelled.  Keep the client record
        // until its correlated terminal TurnRecoveryState arrives.
        val record = durableTurns.load()
        val owner = canonicalSessionId(sessionId).takeIf(String::isNotBlank)
            ?: throw IllegalStateException("Cannot cancel without a session owner")
        val target = turnId ?: record?.takeIf { canonicalSessionId(it.sessionId) == owner }?.turnId
        if (target != null) eventIngress.dispatchBoundary {
            turnIngressInterest.beginCancellation(owner, target.toULong())
        }
        try {
            submitTurnCancellation(permissionIngress) {
                handle.submit(ClientCommand.Cancel(sessionId = owner, turnId = turnId?.toULong()))
            }
        } catch (error: Throwable) {
            if (target != null) eventIngress.dispatchBoundary { onCancellationFailed(owner, target) }
            throw error
        }
        if (target != null) eventIngress.dispatchBoundary { onCancellationDrained(owner, target) }
    }

    override suspend fun discardDurableTurn(turnId: Long) {
        cancel(turnId)
    }

    override suspend fun discardDurableTurnForSession(sessionId: String, turnId: Long) {
        cancelForSession(sessionId, turnId)
    }

    override fun close() {
        eventIngress.retire()
        com.lingxi.code.voice.audio.AndroidAudioSessionContext.detach(handle)
        mediaContextBindings.close()
        ConversationHeadlessRecovery.unregister(recoverySpec.scopeKey, this)
        eventRelay.close()
        workflowRelay.close()
        eventScope.cancel()
        runCatching { handle.destroy() }
    }

    fun activateAudioContext() { com.lingxi.code.voice.audio.AndroidAudioSessionContext.attach(handle) }

    override fun engineSourceForBackgroundRetention(): EngineConversationSource = this

    companion object {
        // Keep terminal versions across source replacement and session changes.
        // Old callbacks can remain in flight until their source is destroyed.
        private val processEventIngress = DurableRecoveryIngress()

        /** Claim reusable sources on the UI lane; only a new native engine is
         * constructed in IO. Cancellation owns and closes only that new handle.
         */
        internal suspend fun createInBackground(
            context: Context,
            projectWorkspace: ProjectWorkspace? = null,
            workspaceKey: String? = null,
            sessionMode: SessionMode = SessionMode.Code,
            linuxRuntimeMode: LinuxRuntimeMode = LinuxRuntimeMode.MobileLinux,
            reuseProcessSource: Boolean = true,
            currentSource: ConversationSource? = null,
        ): ConversationSource {
            val spec = ConversationRecoverySpec(
                projectId = if (workspaceKey == "global" || workspaceKey == "scheduled") null else projectWorkspace?.projectId,
                hostPath = projectWorkspace?.hostPath,
                sessionMode = sessionMode,
                linuxRuntimeMode = linuxRuntimeMode,
                workspaceKey = workspaceKey ?: projectWorkspace?.projectId?.let { "project.$it" } ?: "global",
                guestPath = projectWorkspace?.guestPath,
                additionalDirectories = projectWorkspace?.additionalDirectories.orEmpty(),
            )
            if (reuseProcessSource) {
                if (currentSource?.recoverySpec == spec) return currentSource
                when (val claim = ConversationHeadlessRecovery.acquireForUi(spec, conversationStrings(context))) {
                    is ConversationHeadlessRecovery.UiSourceClaim.Existing -> return claim.source
                    is ConversationHeadlessRecovery.UiSourceClaim.Pending -> return claim.source
                    ConversationHeadlessRecovery.UiSourceClaim.Build -> Unit
                }
            }
            return prepareConversationSource {
                create(context, projectWorkspace, workspaceKey, sessionMode, linuxRuntimeMode,
                    reuseProcessSource = reuseProcessSource, replaceProcessSource = true)
            }
        }

        /**
         * Build the engine + register the event-bridging listener, or return an
         * explicit unavailable source when the engine is not usable.
         */
        fun create(
            context: Context,
            projectWorkspace: ProjectWorkspace? = null,
            workspaceKey: String? = null,
            sessionMode: SessionMode = SessionMode.Code,
            linuxRuntimeMode: LinuxRuntimeMode = LinuxRuntimeMode.MobileLinux,
            reuseProcessSource: Boolean = true,
            /**
             * A reconnect: build a fresh engine although the UI already holds
             * this scope's source. [ConversationHeadlessRecovery.register] then
             * moves the scope to the new source, so closing the old one is safe.
             */
            replaceProcessSource: Boolean = false,
        ): ConversationSource {
            DurableConversationTurnIds.initialize(context)
            val recoverySpec = ConversationRecoverySpec(
                projectId = if (workspaceKey == "global" || workspaceKey == "scheduled") null else projectWorkspace?.projectId,
                hostPath = projectWorkspace?.hostPath,
                sessionMode = sessionMode,
                linuxRuntimeMode = linuxRuntimeMode,
                workspaceKey = workspaceKey ?: projectWorkspace?.projectId?.let { "project.$it" } ?: "global",
                guestPath = projectWorkspace?.guestPath,
                additionalDirectories = projectWorkspace?.additionalDirectories.orEmpty(),
            )
            if (reuseProcessSource && !replaceProcessSource) {
                when (
                    val claim = ConversationHeadlessRecovery.acquireForUi(
                        recoverySpec = recoverySpec,
                        strings = conversationStrings(context),
                    )
                ) {
                    is ConversationHeadlessRecovery.UiSourceClaim.Existing -> return claim.source
                    is ConversationHeadlessRecovery.UiSourceClaim.Pending -> return claim.source
                    ConversationHeadlessRecovery.UiSourceClaim.Build -> Unit
                }
            }
            // Callback ingress is non-blocking and lossless. A dedicated pump may
            // suspend behind a slow collector without ever stalling Rust's event
            // callback or dropping assistant text / terminal events.
            val eventScope = CoroutineScope(SupervisorJob() + Dispatchers.Default)
            val eventRelay = LosslessEventRelay<ConversationDelivery>(eventScope)
            val workflowRelay = LosslessEventRelay<WorkflowProgressUpdate>(eventScope)
            val audioContextUpdates = OrderedSuspendQueue(eventScope)
            // Resolves user-facing copy in the app's actual selected language
            // (via Context.getString, so it honors AppLanguageStore's locale
            // wrap) for every non-Composable emission site below.
            val strings = conversationStrings(context)
            val durableTurns = DurableConversationTurnClientStore(
                context = context,
                scope = recoverySpec.scopeKey,
            )
            val durableReplayGate = DurableTurnReplayGate()
            // The Host's mobile-turn store is rooted under this app files root;
            // UI project scopes can share that same underlying store.
            val eventIngress = processEventIngress.newSource(context.applicationContext.filesDir.canonicalPath)
            val turnIngressInterest = DurableTurnIngressInterest()
            val attachIngress = DurableAttachIngress()
            val attachConsumers = AttachConsumerRegistry()
            fun retireLease(session: String, turn: ULong) {
                val lease = eventIngress.detachInterest(session, turn) ?: return
                attachIngress.forgetIdentity(session, turn)
                val barrier = attachConsumers.retirement { eventIngress.releaseInterest(lease) }
                if (barrier != null && !eventRelay.offer(ConversationDelivery.RecoveryInterestEnded(barrier))) barrier.force()
            }
            fun beginInterest(session: String, turn: Long, newGeneration: Boolean): Boolean {
                val old = turnIngressInterest.currentIdentity()
                val next = canonicalSessionId(session) to turn.toULong()
                if (old != null && (old != next || newGeneration)) {
                    turnIngressInterest.retire(old.first, old.second)
                    retireLease(old.first, old.second)
                }
                turnIngressInterest.begin(session, turn)
                return eventIngress.acquireInterest(session, turn.toULong()) != null
            }
            // The head parked permission request. The engine's outbound
            // `AndroidPermissionSink.onRequest` pushes each request here (mapped
            // to the UI render model); PermissionIngress retains all concurrent
            // workflow-child requests FIFO and advances this StateFlow when the
            // engine emits the correlated resolved event.
            val permissions = MutableStateFlow<PermissionPromptState?>(null)
            val permissionIngress = PermissionIngress(permissions, strings)
            // The engine's REAL model catalog + active id (SHIP-BLOCKER #2). The
            // listener below folds every inbound `ModelList` / `ModelChanged`
            // into this StateFlow via the pure `reduceModelEvent`, so the picker
            // is driven by real wire ids out-of-band from the per-turn stream.
            val models = MutableStateFlow(EngineModelState())
            // The engine's REAL resumable-session catalog (sibling of `models`).
            // The listener below folds every inbound `SessionList` into this
            // StateFlow via the pure `reduceSessionEvent`, so the drawer is driven
            // by real history out-of-band from the per-turn stream.
            val sessions = MutableStateFlow(EngineSessionState.loading())
            val workspace = MutableStateFlow<com.lingxi.code.project.SessionWorkspaceState?>(null)
            // The engine's most recent active-session transition (sibling of
            // `sessions`). The first agent listing binds the cold engine's
            // owner; explicit New/Resume events subsequently replace it with
            // the confirmed session id and any restored transcript.
            val activeSession = MutableStateFlow<ActivatedSession?>(null)
            // A later submitted turn can overtake its predecessor if a host
            // producer is cancelled. Hold only explicitly submitted identities
            // until the predecessor's terminal checkpoint is released.
            val deferredQueuedEvents = mutableListOf<ClientEvent>()
            var deferredQueuedTimeout: Job? = null
            lateinit var processIncomingEvent: (ClientEvent) -> Unit
            fun queuedIdentity(event: ClientEvent): Pair<String, ULong>? = when (event) {
                is ClientEvent.TurnRecoveryState -> event.snapshot.sessionId to event.snapshot.turnId
                is ClientEvent.TurnAttachStarted -> event.snapshot.sessionId to event.snapshot.turnId
                is ClientEvent.TurnAttachFinished -> event.sessionId to event.turnId
                is ClientEvent.TurnEventReplay -> event.sessionId to event.turnId
                is ClientEvent.TurnEventLiveUnretained -> event.sessionId to event.turnId
                else -> null
            }
            fun failDeferredQueuedEvents() {
                val identities = deferredQueuedEvents.mapNotNull(::queuedIdentity).toSet()
                deferredQueuedEvents.clear()
                deferredQueuedTimeout?.cancel()
                deferredQueuedTimeout = null
                identities.forEach { (session, turn) ->
                    turnIngressInterest.removeQueued(turn)
                    eventRelay.offer(ConversationDelivery.QueuedTurnIncomplete(session, turn))
                }
            }
            fun deferQueuedEvent(event: ClientEvent) {
                if (deferredQueuedEvents.size >= 512) {
                    deferredQueuedEvents.add(event)
                    failDeferredQueuedEvents()
                    return
                }
                deferredQueuedEvents.add(event)
                if (deferredQueuedTimeout == null) deferredQueuedTimeout = eventScope.launch {
                    delay(5_000)
                    eventIngress.dispatchBoundary { failDeferredQueuedEvents() }
                }
            }
            fun releaseDeferredQueuedEvents() {
                if (deferredQueuedEvents.isEmpty()) return
                val pending = deferredQueuedEvents.toList()
                deferredQueuedEvents.clear()
                deferredQueuedTimeout?.cancel()
                deferredQueuedTimeout = null
                pending.forEach(processIncomingEvent)
            }
            fun finishAttach(attempt: DurableAttachIngress.Attempt) {
                attachIngress.end(attempt)
                if (attempt.terminalObserved) {
                    if (!durableTurns.finishTerminal(attempt.sessionId, attempt.turnId.toLong())) {
                        if (!durableTurns.isHealthy()) eventRelay.offer(ConversationDelivery.RecoveryStorageFailed)
                        eventRelay.offer(ConversationDelivery.TurnEventDecodeFailed(attempt.sessionId, attempt.turnId))
                        return
                    }
                    turnIngressInterest.markSettled(attempt.sessionId, attempt.turnId, recoverySnapshot = true)
                    turnIngressInterest.retire(attempt.sessionId, attempt.turnId)
                    retireLease(attempt.sessionId, attempt.turnId)
                    releaseDeferredQueuedEvents()
                }
            }
            // The engine's REAL MCP listing (sibling of `models`). The listener
            // folds every inbound `McpServers` into this StateFlow; empty until a
            // `RefreshListings(Mcp)` reply lands (settings shows the mock list).
            val mcp = MutableStateFlow(emptyList<MCPServer>())
            // Persisted secrets are resolved from the Rust shared secure store;
            // these scalar values remain only as explicit development overrides.
            val environment = resolveEngineEnvironmentOverrides(System.getenv())
            val providerLaunch = ProviderSettingsRepository(context).engineLaunchConfig()
            val mediaHost = AndroidNativeMediaHostProvider.get(context.applicationContext)
            val mediaContextBindings = MediaAudioSessionContextBindings { sessionId, generation ->
                mediaHost.removeAudioSessionContext(sessionId, generation)
            }
            val mediaContextEngine = java.util.concurrent.atomic.AtomicReference<MobileEngineHandle?>(null)
            processIncomingEvent = { event ->
                var finishedAttempt: DurableAttachIngress.Attempt? = null
                var envelopeRoute: DurableAttachIngress.EnvelopeRoute? = null
                eventIngress.dispatch(event, admit = {
                    when (event) {
                        is ClientEvent.TurnAttachStarted -> {
                            val snapshot = event.snapshot
                            val belongs = turnIngressInterest.accepts(snapshot.sessionId, snapshot.turnId, activeSession.value?.sessionId) ||
                                turnIngressInterest.isQueued(snapshot.sessionId, snapshot.turnId, activeSession.value?.sessionId)
                            if (!attachIngress.started(event.requestId, snapshot, belongs)) return@dispatch false
                        }
                        is ClientEvent.TurnAttachFinished -> {
                            finishedAttempt = attachIngress.finished(event.requestId, event.sessionId, event.turnId)
                            if (finishedAttempt == null) return@dispatch false
                        }
                        is ClientEvent.TurnRecoveryState -> if (attachIngress.bufferSnapshot(event)) return@dispatch false
                        is ClientEvent.TurnEventReplay -> {
                            envelopeRoute = attachIngress.routeEnvelope(event.sessionId, event.turnId)
                            if (envelopeRoute?.accepted != true) return@dispatch false
                        }
                        is ClientEvent.TurnEventLiveUnretained -> {
                            envelopeRoute = attachIngress.routeEnvelope(event.sessionId, event.turnId)
                            if (envelopeRoute?.excludedRequestId != null || envelopeRoute?.accepted != true) {
                                attachIngress.noteUnretainedBeforeBootstrap(event.sessionId, event.turnId)
                            }
                            if (envelopeRoute?.accepted != true) return@dispatch false
                        }
                        else -> Unit
                    }
                    val identity = queuedIdentity(event)
                    if (identity == null) true else {
                        val (session, turn) = identity
                        val active = activeSession.value?.sessionId
                        when {
                            turnIngressInterest.accepts(session, turn, active) -> true
                            !turnIngressInterest.isQueued(session, turn, active) -> false
                            event is ClientEvent.TurnRecoveryState && event.snapshot.state in setOf(
                                TurnRecoveryStateDto.COMPLETED, TurnRecoveryStateDto.FAILED, TurnRecoveryStateDto.CANCELLED,
                            ) -> eventIngress.acquireInterest(session, turn) != null
                            turnIngressInterest.canAdopt(session, turn, active) && durableTurns.load() == null &&
                                (event !is ClientEvent.TurnRecoveryState || event.snapshot.state == TurnRecoveryStateDto.RUNNING) -> {
                                val acquired = eventIngress.acquireInterest(session, turn) != null
                                if (acquired) eventRelay.offer(ConversationDelivery.QueuedTurnRegistered(session, turn))
                                acquired
                            }
                            else -> { deferQueuedEvent(event); false }
                        }
                    }
                }, onRevisionRejected = {
                    if (event is ClientEvent.TurnAttachStarted) attachIngress.rejectStarted()
                }) {
                        if (event is ClientEvent.TurnRecoveryState && event.snapshot.state in setOf(
                                TurnRecoveryStateDto.COMPLETED, TurnRecoveryStateDto.FAILED, TurnRecoveryStateDto.CANCELLED,
                            ) && turnIngressInterest.isQueued(event.snapshot.sessionId, event.snapshot.turnId, activeSession.value?.sessionId)
                        ) {
                            val snapshot = event.snapshot
                            if (snapshot.turnId > Long.MAX_VALUE.toULong() ||
                                !durableTurns.finishQueued(snapshot.sessionId, snapshot.turnId.toLong())
                            ) {
                                if (!durableTurns.isHealthy()) eventRelay.offer(ConversationDelivery.RecoveryStorageFailed)
                                eventRelay.offer(ConversationDelivery.QueuedTurnIncomplete(snapshot.sessionId, snapshot.turnId))
                                retireLease(snapshot.sessionId, snapshot.turnId)
                                return@dispatch
                            }
                            turnIngressInterest.removeQueued(snapshot.turnId)
                            eventRelay.offer(ConversationDelivery.QueuedTurnSettled(snapshot.sessionId, snapshot.turnId, reviewRequired = true))
                            retireLease(snapshot.sessionId, snapshot.turnId)
                            return@dispatch
                        }
                        if (event is ClientEvent.ModSessionEvent &&
                            activeSession.value?.sessionId?.let { canonicalSessionId(it) == canonicalSessionId(event.sessionId) } != true
                        ) return@dispatch
                        if (event is ClientEvent.TurnRecoveryState && event.snapshot.state == TurnRecoveryStateDto.RUNNING &&
                            turnIngressInterest.canAdopt(event.snapshot.sessionId, event.snapshot.turnId, activeSession.value?.sessionId)
                        ) {
                            val checkpoint = durableTurns.load()
                            if (checkpoint == null || (checkpoint.turnId == event.snapshot.turnId.toLong() &&
                                    canonicalSessionId(checkpoint.sessionId) == canonicalSessionId(event.snapshot.sessionId))
                            ) {
                                if (!durableTurns.promoteQueued(event.snapshot.sessionId, event.snapshot.turnId.toLong())) {
                                    if (!durableTurns.isHealthy()) eventRelay.offer(ConversationDelivery.RecoveryStorageFailed)
                                    eventRelay.offer(ConversationDelivery.QueuedTurnIncomplete(event.snapshot.sessionId, event.snapshot.turnId))
                                    retireLease(event.snapshot.sessionId, event.snapshot.turnId)
                                    return@dispatch
                                }
                                turnIngressInterest.adopt(event.snapshot.sessionId, event.snapshot.turnId)
                                attachIngress.noteLiveOwnership(event.snapshot.sessionId, event.snapshot.turnId)
                            }
                        }
                        val delivery = when (event) {
                            is ClientEvent.TurnRecoveryState -> ConversationDelivery.RecoveryState(event.snapshot,
                                eventIngress.fence(event.snapshot.sessionId, event.snapshot.turnId))
                            is ClientEvent.TurnAttachStarted -> {
                                attachIngress.confirmStarted(event.snapshot)
                                ConversationDelivery.AttachStarted(event.requestId, event.snapshot,
                                    eventIngress.fence(event.snapshot.sessionId, event.snapshot.turnId),
                                    admissionPending = attachIngress.isAdmissionPending(event.requestId))
                            }
                            is ClientEvent.TurnAttachFinished -> {
                                val attempt = finishedAttempt ?: return@dispatch
                                if (attempt.incomplete) eventRelay.offer(ConversationDelivery.TurnEventDecodeFailed(event.sessionId, event.turnId,
                                    attachRequestId = event.requestId, isActive = eventIngress.fence(event.sessionId, event.turnId)))
                                attempt.pendingSnapshots.toList().forEach(processIncomingEvent)
                                attempt.pendingSnapshots.clear()
                                // Reentrant dispatch queues those snapshots. Queue
                                // the finish boundary after them, so Resume cannot
                                // overtake an earlier unassociated live state.
                                eventIngress.dispatchBoundary {
                                    if (attempt.uiOwner) {
                                        val barrier = attachConsumers.barrier(attempt.consumed)
                                        attempt.barrier = barrier
                                        eventRelay.offer(ConversationDelivery.AttachReplayEnded(event.sessionId, event.turnId.toLong(),
                                            attempt.expectsResume, event.requestId, barrier = barrier,
                                            isActive = eventIngress.fence(event.sessionId, event.turnId)))
                                    } else attempt.consumed.complete(!attempt.incomplete)
                                }
                                return@dispatch
                            }
                            is ClientEvent.TurnEventReplay, is ClientEvent.TurnEventLiveUnretained -> {
                                val session = when (event) {
                                    is ClientEvent.TurnEventReplay -> event.sessionId
                                    is ClientEvent.TurnEventLiveUnretained -> event.sessionId
                                    else -> error("unreachable")
                                }
                                val turn = when (event) {
                                    is ClientEvent.TurnEventReplay -> event.turnId
                                    is ClientEvent.TurnEventLiveUnretained -> event.turnId
                                    else -> error("unreachable")
                                }
                                val decoded = runCatching { decodeTurnEventEnvelope(event) }.getOrElse {
                                    eventRelay.offer(ConversationDelivery.TurnEventDecodeFailed(session, turn,
                                        attachRequestId = envelopeRoute?.batchRequestId, isActive = eventIngress.fence(session, turn)))
                                    return@dispatch
                                }
                                if (canonicalSessionId(decoded.sessionId) != canonicalSessionId(session) || decoded.turnId != turn) return@dispatch
                                if (!turnIngressInterest.accepts(session, turn, activeSession.value?.sessionId)) {
                                    val started = decoded.event as? ClientEvent.TurnStarted
                                    if (started == null || (started.turnId != null && started.turnId != turn) ||
                                        (decoded.sequence != null && decoded.sequence != 1uL) ||
                                        !turnIngressInterest.canAdopt(session, turn, activeSession.value?.sessionId) || durableTurns.load() != null
                                    ) {
                                        eventRelay.offer(ConversationDelivery.QueuedTurnIncomplete(session, turn))
                                        turnIngressInterest.removeQueued(turn)
                                        return@dispatch
                                    }
                                    if (!durableTurns.promoteQueued(session, turn.toLong())) {
                                        if (!durableTurns.isHealthy()) eventRelay.offer(ConversationDelivery.RecoveryStorageFailed)
                                        eventRelay.offer(ConversationDelivery.QueuedTurnIncomplete(session, turn))
                                        retireLease(session, turn)
                                        return@dispatch
                                    }
                                    turnIngressInterest.adopt(session, turn)
                                    attachIngress.noteLiveOwnership(session, turn)
                                }
                                ConversationDelivery.TurnEvent(decoded.sessionId, decoded.turnId, decoded.sequence, decoded.event,
                                    attachRequestId = envelopeRoute?.batchRequestId,
                                    excludedAttachRequestId = envelopeRoute?.excludedRequestId,
                                    isActive = eventIngress.fence(session, turn))
                            }
                            else -> ConversationDelivery.Event(event)
                        }
                        val projected = when (delivery) {
                            is ConversationDelivery.Event -> delivery.event
                            is ConversationDelivery.TurnEvent -> delivery.event
                            is ConversationDelivery.RecoveryState -> ClientEvent.TurnRecoveryState(delivery.snapshot)
                            is ConversationDelivery.AttachStarted -> ClientEvent.TurnRecoveryState(delivery.snapshot)
                            else -> return@dispatch
                        }
                        var terminalPersisted = true
                        if (projected is ClientEvent.TurnRecoveryState && projected.snapshot.state in setOf(
                                TurnRecoveryStateDto.COMPLETED, TurnRecoveryStateDto.FAILED, TurnRecoveryStateDto.CANCELLED,
                            )
                        ) {
                            terminalPersisted = projected.snapshot.turnId <= Long.MAX_VALUE.toULong() &&
                                durableTurns.recordTerminalEvidence(projected.snapshot.sessionId, projected.snapshot.turnId.toLong())
                            if (!durableTurns.isHealthy()) eventRelay.offer(ConversationDelivery.RecoveryStorageFailed)
                            if (!terminalPersisted) eventRelay.offer(ConversationDelivery.TurnEventDecodeFailed(
                                projected.snapshot.sessionId, projected.snapshot.turnId))
                        }
                        if (delivery is ConversationDelivery.TurnEvent &&
                            (projected is ClientEvent.TurnEnded || (projected is ClientEvent.Error &&
                                !turnIngressInterest.cancellationHasPendingTerminal(delivery.sessionId, delivery.turnId)))
                        ) turnIngressInterest.markSettled(delivery.sessionId, delivery.turnId)
                        val protectedTerminal = event is ClientEvent.TurnRecoveryState && projected is ClientEvent.TurnRecoveryState &&
                            projected.snapshot.state in setOf(TurnRecoveryStateDto.COMPLETED, TurnRecoveryStateDto.FAILED, TurnRecoveryStateDto.CANCELLED) &&
                            attachIngress.noteTerminal(projected.snapshot)
                        if (event is ClientEvent.TurnRecoveryState && projected is ClientEvent.TurnRecoveryState && projected.snapshot.state in setOf(
                                TurnRecoveryStateDto.COMPLETED, TurnRecoveryStateDto.FAILED, TurnRecoveryStateDto.CANCELLED,
                            )
                        ) turnIngressInterest.markSettled(projected.snapshot.sessionId, projected.snapshot.turnId, recoverySnapshot = true)
                        // A transcript row token is not proof that a queued id
                        // was durably merged into this turn. Keep the ledger
                        // until the host supplies a correlated durable outcome.
                        // The OUT-OF-BAND state paths: fold model catalog + session
                        // catalog + live-resume events into the StateFlows the picker /
                        // drawer / conversation observe, BEFORE forwarding to the
                        // per-turn stream. Every reducer is a no-op for unrelated
                        // events, so every event still reaches `events` unchanged
                        // (lifecycle events like SessionStarted ride the per-turn
                        // stream; the ViewModel acts on them there).
                        models.value = reduceModelEvent(models.value, projected)
                        sessions.value = reduceSessionEvent(sessions.value, projected, strings = sessionCatalogStrings(context))
                        val previousActiveSessionId = activeSession.value?.sessionId
                        activeSession.update { current ->
                            reduceActivatedSession(current, projected, sessionMode, strings)
                        }
                        if (previousActiveSessionId != null && activeSession.value?.sessionId?.let {
                                canonicalSessionId(it) != canonicalSessionId(previousActiveSessionId)
                            } == true
                        ) {
                            turnIngressInterest.currentIdentity()?.let { retireLease(it.first, it.second) }
                            turnIngressInterest.reset()
                            deferredQueuedEvents.clear()
                            deferredQueuedTimeout?.cancel()
                            deferredQueuedTimeout = null
                        }
                        activeSession.value?.sessionId?.takeIf { session ->
                            previousActiveSessionId?.let(::canonicalSessionId) != canonicalSessionId(session)
                        }?.let { session ->
                            durableTurns.queued(session).forEach { turnIngressInterest.enqueue(it.sessionId, it.turnId.toULong()) }
                            if (durableTurns.load() == null) turnIngressInterest.allowQueuedWhenNoOwner()
                        }
                        when (projected) {
                            ClientEvent.SessionEnded -> previousActiveSessionId?.let(mediaContextBindings::remove)
                            else -> Unit
                        }
                        if (projected is ClientEvent.WorkspaceChanged) {
                            val value = projected.state
                            workspace.value = com.lingxi.code.project.SessionWorkspaceState(
                                value.sessionId, value.cwd, value.projectRoot,
                                value.additionalDirectories, value.transcriptPath,
                            )
                        }
                        // Out-of-band MCP listing: fold `McpServers` into its StateFlow.
                        if (projected is ClientEvent.McpServers) mcp.value = projected.servers.map { it.toMcpServer() }
                        if (projected is ClientEvent.SettingsSnapshot) {
                            runCatching {
                                val repository = ProviderSettingsRepository(context)
                                try { repository.recordAudioProviderRegion(projected.effectiveJson) } finally { repository.close() }
                            }
                        }
                        // A StateFlow observer can synchronously close this
                        // source while the reentrant monitor is still held.
                        if (!eventIngress.isActive()) return@dispatch
                        if (projected is ClientEvent.TurnStarted) permissionIngress.confirmTurnStarted()
                        if (projected is ClientEvent.PermissionRequestResolved) {
                            permissionIngress.resolve(projected.requestId)
                        }
                        if (!eventIngress.isActive()) return@dispatch
                        if (
                            event is ClientEvent.TurnRecoveryState && projected is ClientEvent.TurnRecoveryState &&
                                projected.snapshot.state in setOf(
                                    TurnRecoveryStateDto.COMPLETED,
                                    TurnRecoveryStateDto.FAILED,
                                    TurnRecoveryStateDto.CANCELLED,
                                )
                        ) {
                            if (terminalPersisted && !protectedTerminal && !turnIngressInterest.cancellationHasPendingTerminal(projected.snapshot.sessionId, projected.snapshot.turnId)) {
                                terminalPersisted = durableTurns.finishTerminal(projected.snapshot.sessionId, projected.snapshot.turnId.toLong())
                                if (!durableTurns.isHealthy()) eventRelay.offer(ConversationDelivery.RecoveryStorageFailed)
                                if (!terminalPersisted) eventRelay.offer(ConversationDelivery.TurnEventDecodeFailed(
                                    projected.snapshot.sessionId, projected.snapshot.turnId))
                            }
                        }
                        if (!eventIngress.isActive()) return@dispatch
                        var slashSaved = true
                        if (projected is ClientEvent.SlashCommandResult) {
                            slashSaved = clearDurableSlashCommandCheckpoint(durableTurns, projected)
                            if (!slashSaved && !durableTurns.isHealthy()) eventRelay.offer(ConversationDelivery.RecoveryStorageFailed)
                            permissionIngress.endTurn()
                        }
                        if (projected is ClientEvent.TurnEnded ||
                            (projected is ClientEvent.Error && !isCompactTurnRejection(projected.message.displayText()))) {
                            permissionIngress.endTurn()
                        }
                        if (!eventIngress.isActive()) return@dispatch
                        if (durableReplayGate.shouldForward(event) || delivery is ConversationDelivery.TurnEvent) {
                            eventRelay.offer(delivery)
                        }
                        if (event is ClientEvent.TurnRecoveryState && projected is ClientEvent.TurnRecoveryState && projected.snapshot.state in setOf(
                                TurnRecoveryStateDto.COMPLETED, TurnRecoveryStateDto.FAILED, TurnRecoveryStateDto.CANCELLED,
                            )
                        && terminalPersisted && !protectedTerminal && !turnIngressInterest.cancellationHasPendingTerminal(projected.snapshot.sessionId, projected.snapshot.turnId)
                        ) {
                            turnIngressInterest.retire(projected.snapshot.sessionId, projected.snapshot.turnId)
                            retireLease(projected.snapshot.sessionId, projected.snapshot.turnId)
                            releaseDeferredQueuedEvents()
                        }
                        if (delivery is ConversationDelivery.TurnEvent && projected is ClientEvent.TurnEnded &&
                            turnIngressInterest.finishFailedCancellationAfterTurnEnd(delivery.sessionId, delivery.turnId)
                        ) {
                            val saved = durableTurns.finishTerminal(delivery.sessionId, delivery.turnId.toLong())
                            eventRelay.offer(ConversationDelivery.CancellationDrained(delivery.sessionId, delivery.turnId.toLong(), saved))
                            if (saved) {
                                turnIngressInterest.retire(delivery.sessionId, delivery.turnId)
                                retireLease(delivery.sessionId, delivery.turnId)
                                releaseDeferredQueuedEvents()
                            }
                        }
                        if (projected is ClientEvent.SlashCommandResult && slashSaved) {
                            turnIngressInterest.currentIdentity()?.takeIf { projected.turnId == it.second }?.let {
                                turnIngressInterest.retire(it.first, it.second)
                                retireLease(it.first, it.second)
                            }
                        }
                        if (projected is ClientEvent.SessionStarted || projected is ClientEvent.SessionResumed ||
                            projected is ClientEvent.ModelChanged
                        ) {
                            // Schedule the suspending UniFFI call from the queued
                            // item itself, including for synchronous reentry.
                            audioContextUpdates.offer {
                                runCatching {
                                    mediaContextEngine.get()?.let { engine ->
                                        val binding = mediaHost.updateAudioSessionContext(engine)
                                        mediaContextBindings.register(binding.sessionId, binding.generation)
                                    }
                                }
                            }
                        }
                    }
            }
            val handle = buildVoiceEngine(
                context = context,
                apiBase = environment.apiBase,
                apiKey = environment.apiKey,
                model = environment.model.ifBlank { providerLaunch.defaultModel },
                providerProfilesJson = providerLaunch.providerProfilesJson,
                routingJson = providerLaunch.routingJson,
                visionDelegationEnabled = providerLaunch.visionDelegationEnabled,
                projectWorkspace = projectWorkspace,
                sessionMode = sessionMode,
                linuxRuntimeMode = linuxRuntimeMode,
                mediaHost = mediaHost,
                onEvent = processIncomingEvent,
                onWorkflowProgress = { originSessionId, taskId, runId, progress ->
                    workflowRelay.offer(
                        WorkflowProgressUpdate(
                            originSessionId = originSessionId,
                            taskId = taskId,
                            runId = runId,
                            progress = progress,
                        ),
                    )
                },
                onPermission = { request ->
                    permissionIngress.publish(request)
                },
            ) ?: run {
                eventIngress.retire()
                if (reuseProcessSource) {
                    ConversationHeadlessRecovery.releaseUiReservation(recoverySpec.scopeKey)
                }
                eventRelay.close()
                workflowRelay.close()
                eventScope.cancel()
                return UnavailableConversationSource(
                    context.getString(R.string.chat_engine_build_failed),
                )
            }
            mediaContextEngine.set(handle)
            val workspaceRestore = runCatching { kotlinx.coroutines.runBlocking {
                audioContextUpdates.run {
                    val binding = mediaHost.updateAudioSessionContext(handle)
                    mediaContextBindings.register(binding.sessionId, binding.generation)
                }
                val owner = handle.workspaceState().sessionId
                eventIngress.dispatchBoundary {
                    if (owner.isNotBlank()) check(durableTurns.restoreParked(owner)) { "Unable to restore the conversation owner" }
                    activeSession.update { current -> bootstrapWorkspaceSession(current, owner, sessionMode) }
                    durableTurns.queued(owner).forEach { turnIngressInterest.enqueue(it.sessionId, it.turnId.toULong()) }
                    if (durableTurns.load() == null) turnIngressInterest.allowQueuedWhenNoOwner()
                }
                val userDirectories = com.lingxi.code.project.MobileUserStorage.directories(context)
                val home = userDirectories.guestHome
                val userHome = userDirectories.home
                projectWorkspace?.additionalDirectories.orEmpty().forEach { directory ->
                    if ((directory == home || directory.startsWith("$home/")) && java.io.File(userHome, directory.removePrefix(home).removePrefix("/")).isDirectory) {
                        handle.addWorkspaceDirectory(directory, owner)
                    }
                }
            } }
            workspaceRestore.exceptionOrNull()?.let { failure ->
                eventIngress.retire()
                mediaContextBindings.close()
                runCatching { handle.destroy() }
                if (reuseProcessSource) ConversationHeadlessRecovery.releaseUiReservation(recoverySpec.scopeKey)
                eventRelay.close()
                workflowRelay.close()
                eventScope.cancel()
                return UnavailableConversationSource(failure.message ?: context.getString(R.string.chat_engine_build_failed))
            }
            // Ask the engine for its REAL catalog now that the handle exists; the
            // reply (`ModelList`) flows back through the listener above into the
            // `models` StateFlow, populating the picker with real wire ids.
            // `handle.submit` is suspend, so fire it off the calling thread — a
            // failed ListModels leaves the catalog explicitly empty; it never
            // blocks building the source.
            CoroutineScope(Dispatchers.Default).launch {
                // The initial media context was bound before the source was
                // returned; this listing only primes session-agent UI state.
                try {
                    handle.submit(ClientCommand.ListSessionAgents)
                    val value = handle.workspaceState()
                    workspace.value = com.lingxi.code.project.SessionWorkspaceState(value.sessionId, value.cwd, value.projectRoot, value.additionalDirectories, value.transcriptPath)
                } catch (_: Throwable) {
                    // A later foreground execution refresh retries this query.
                }
                try {
                    handle.submit(ClientCommand.ListModels)
                    handle.submit(ClientCommand.GetConversationControls)
                } catch (_: Throwable) {
                    // benign: the picker keeps its current selection until a
                    // later catalog refresh succeeds.
                }
                // Same out-of-band priming for the session catalog: the reply
                // (`SessionList`) flows back through the listener into `sessions`,
                // populating the drawer with real history.
                try {
                    handle.submit(completeSessionListCommand())
                } catch (t: Throwable) {
                    sessions.value = EngineSessionState.error(
                        strings.resolve(
                            R.string.chat_error_session_list_failed,
                            "会话列表加载失败：%1\$s",
                            "${t.message ?: t::class.simpleName}",
                        ),
                    )
                }
                // Prime the unified execution card on cold start as well as
                // after lifecycle resume. Task rows and session-agent rows are
                // independent listing replies and may arrive in either order.
                try {
                    handle.submit(ClientCommand.TaskList(statusFilter = null, requestId = null))
                } catch (_: Throwable) {
                    // Status is best-effort; a later foreground refresh retries.
                }
            }
            val source = EngineConversationSource(
                handle = handle,
                mediaHost = mediaHost,
                mediaContextBindings = mediaContextBindings,
                events = attachConsumers.track(eventRelay.events, ui = false).mapNotNull {
                    when (it) {
                        is ConversationDelivery.Event -> it.event
                        is ConversationDelivery.RecoveryState -> if (it.isActive()) ClientEvent.TurnRecoveryState(it.snapshot) else null
                        is ConversationDelivery.TurnEvent -> if (it.isActive()) it.event else null
                        is ConversationDelivery.AttachStarted -> if (it.isActive()) ClientEvent.TurnRecoveryState(it.snapshot) else null
                        else -> null
                    }
                },
                eventRelay = eventRelay,
                workflowEvents = workflowRelay.events,
                workflowRelay = workflowRelay,
                eventScope = eventScope,
                permissions = permissions,
                permissionIngress = permissionIngress,
                models = models,
                sessions = sessions,
                workspace = workspace,
                activeSession = activeSession,
                mcp = mcp,
                strings = strings,
                durableTurns = durableTurns,
                durableReplayGate = durableReplayGate,
                eventIngress = eventIngress,
                turnIngressInterest = turnIngressInterest,
                attachIngress = attachIngress,
                attachConsumers = attachConsumers,
                beginTurnInterest = ::beginInterest,
                retireTurnInterest = { session, turn ->
                    turnIngressInterest.retire(session, turn)
                    retireLease(session, turn)
                },
                finishAttachInterest = ::finishAttach,
                onCancellationDrained = { session, turn ->
                    val observed = turnIngressInterest.finishCancellation(session, turn.toULong())
                    val saved = observed && durableTurns.finishTerminal(session, turn)
                    eventRelay.offer(ConversationDelivery.CancellationDrained(session, turn, saved))
                    if (saved) {
                        turnIngressInterest.retire(session, turn.toULong())
                        retireLease(session, turn.toULong())
                        releaseDeferredQueuedEvents()
                    }
                },
                onCancellationFailed = { session, turn ->
                    val (observed, ended) = turnIngressInterest.abortCancellation(session, turn.toULong())
                    eventRelay.offer(ConversationDelivery.CancellationAborted(session, turn, observed))
                    if (observed && ended && turnIngressInterest.finishFailedCancellationAfterTurnEnd(session, turn.toULong())) {
                        val saved = durableTurns.finishTerminal(session, turn)
                        eventRelay.offer(ConversationDelivery.CancellationDrained(session, turn, saved))
                        if (saved) {
                            turnIngressInterest.retire(session, turn.toULong())
                            retireLease(session, turn.toULong())
                            releaseDeferredQueuedEvents()
                        }
                    }
                },
                autoAttachDurableTurns = !reuseProcessSource,
                resumeOnUiAttach = reuseProcessSource,
                initialRecoverySpec = recoverySpec,
                userHomePath = com.lingxi.code.project.MobileUserStorage.directories(context).home,
                userHomeGuestPath = com.lingxi.code.project.MobileUserStorage.directories(context).guestHome,
            )
            ConversationHeadlessRecovery.register(recoverySpec.scopeKey, source)
            return source
        }
    }
}

/** Keep the protocol command construction independently testable on the JVM. */
internal fun runSlashCommandClientCommand(
    sessionId: String,
    raw: ClientTextDto,
    images: List<ImageRefDto>,
    turnId: Long,
): ClientCommand.RunSlashCommand =
    ClientCommand.RunSlashCommand(sessionId = sessionId, raw = raw, images = images, turnId = turnId.toULong())

internal fun slashCommandEventBelongsToTurn(event: ClientEvent, turnId: Long): Boolean =
    event !is ClientEvent.SlashCommandResult || event.turnId == turnId.toULong()

internal fun clearDurableSlashCommandCheckpoint(
    durableTurns: DurableConversationTurnClientStore,
    event: ClientEvent,
): Boolean {
    if (event is ClientEvent.SlashCommandResult) {
        val current = durableTurns.load()
        if (current != null && event.turnId == current.turnId.toULong()) {
            return durableTurns.finishTerminal(current.sessionId, current.turnId)
        }
    }
    return true
}
