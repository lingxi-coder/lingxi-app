package com.lingxi.code.conversation

import android.content.Context
import android.os.Build
import com.lingxi.code.R
import com.lingxi.code.bindings.client.ClientCommand
import com.lingxi.code.bindings.client.ClientEvent
import com.lingxi.code.bindings.client.ErrorKindDto
import com.lingxi.code.bindings.client.ImageRefDto
import com.lingxi.code.bindings.client.ListingKindDto
import com.lingxi.code.bindings.runtime.MobileEngineHandle
import com.lingxi.code.bindings.runtime.VisualizationHost
import com.lingxi.code.bindings.client.PermissionResponseDto
import com.lingxi.code.bindings.client.TurnRecoveryStateDto
import com.lingxi.code.model.EngineModelState
import com.lingxi.code.model.EngineSessionState
import com.lingxi.code.model.MCPServer
import com.lingxi.code.model.Message
import com.lingxi.code.model.SessionMode
import com.lingxi.code.model.canonicalSessionId
import com.lingxi.code.model.sessionCatalogStrings
import com.lingxi.code.project.ProjectWorkspace
import com.lingxi.code.secure.SecureKeyStore
import com.lingxi.code.secure.resolveEngineCredentials
import com.lingxi.code.settings.LinuxRuntimeMode
import com.lingxi.code.settings.ProviderSettingsRepository
import com.lingxi.code.voice.buildVoiceEngine
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
import kotlinx.coroutines.flow.onSubscription
import kotlinx.coroutines.launch
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
 * handle+listener — no second build, no dropped events.
 *
 * NOTE: this type touches the UniFFI bindings + Android `Context`, so it is NOT
 * exercised by JVM unit tests. The pure [clientEventToReply] mapper carries the
 * mapping coverage; this wiring is covered by the on-device integration build.
 */
class EngineConversationSource private constructor(
    private val handle: MobileEngineHandle,
    private val events: SharedFlow<ClientEvent>,
    private val eventRelay: LosslessEventRelay<ClientEvent>,
    private val workflowEvents: SharedFlow<WorkflowProgressUpdate>,
    private val workflowRelay: LosslessEventRelay<WorkflowProgressUpdate>,
    private val eventScope: CoroutineScope,
    private val permissions: MutableStateFlow<PermissionPromptState?>,
    private val permissionIngress: PermissionIngress,
    private val models: MutableStateFlow<EngineModelState>,
    private val sessions: MutableStateFlow<EngineSessionState>,
    private val activeSession: MutableStateFlow<ActivatedSession?>,
    private val mcp: MutableStateFlow<List<MCPServer>>,
    private val strings: ConversationStrings,
    private val durableTurns: DurableConversationTurnClientStore,
    private val durableReplayGate: DurableTurnReplayGate,
    private val autoAttachDurableTurns: Boolean,
    @Volatile private var resumeOnUiAttach: Boolean,
    override val recoverySpec: ConversationRecoverySpec,
) : ConversationSource, BackgroundRetainableConversationSource {

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
    override val workflowProgress: Flow<WorkflowProgressUpdate> = workflowEvents

    override suspend fun submitClientCommand(command: ClientCommand) {
        handle.submit(command)
    }

    override suspend fun refreshExecutionStatus() {
        handle.submit(ClientCommand.TaskList(statusFilter = null, requestId = null))
        handle.submit(ClientCommand.ListSessionAgents)
    }

    override suspend fun attachDurableTurnForUi(afterSequence: Long) {
        val record = durableTurns.load() ?: return
        if (
            activeSession.value?.let { canonicalSessionId(it.sessionId) } ==
                canonicalSessionId(record.sessionId)
        ) {
            attachAndResume(record, forceAttach = true, afterSequence = afterSequence)
        }
    }

    private suspend fun attachAndResume(
        record: DurableConversationTurnRecord,
        forceAttach: Boolean,
        afterSequence: Long = 0L,
    ) = durableAttachMutex.withLock {
        val latest = durableTurns.load()?.takeIf { it.turnId == record.turnId } ?: record
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
        durableReplayGate.noteAttachRequested(
            turnId = latest.turnId,
            activation = activeSession.value?.takeIf { canonicalSessionId(it.sessionId) == canonicalSessionId(latest.sessionId) },
            afterSequence = request.afterSequence,
        )
        try {
            handle.submit(
                ClientCommand.AttachTurn(
                    turnId = latest.turnId.toULong(),
                    afterSequence = request.afterSequence.toULong(),
                ),
            )
        } catch (error: Throwable) {
            durableReplayGate.cancelAttach(latest.turnId)
            durableAttachCoordinator.rollback(request, owner)
            ConversationHeadlessRecovery.markDurableAttachFailed(recoverySpec.scopeKey, this)
            throw DurableAttachFailure(latest.turnId, "attach", error)
        }
        // Cold-process recovery needs ResumeTurn. A UI takeover resumes only
        // when the process coordinator reports that no headless/UI executor is
        // already attached to this source; retained-live sources only Attach.
        if (shouldResumeDurableTurn(forceAttach, resumeForThisAttach)) {
            try {
                handle.submit(ClientCommand.ResumeTurn(latest.turnId.toULong()))
            } catch (error: Throwable) {
                durableReplayGate.cancelAttach(latest.turnId)
                durableAttachCoordinator.rollback(request, owner)
                ConversationHeadlessRecovery.markDurableAttachFailed(recoverySpec.scopeKey, this)
                throw DurableAttachFailure(latest.turnId, "resume", error)
            }
            if (forceAttach) {
                ConversationHeadlessRecovery.markDurableUiExecutorActive(
                    recoverySpec.scopeKey,
                    this,
                )
            }
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
     * the listener folding `SessionStarted` / `SessionResumed` through
     * [sessionActivationFrom] (see [create]). The ViewModel observes this to
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

    override suspend fun resumeSession(uuid: String) {
        if (uuid.isBlank()) return
        val record = durableTurns.load()
        if (record != null && canonicalSessionId(uuid) != canonicalSessionId(record.sessionId)) {
            durableTurns.clear()
        }
        // Propagate command failures: the ViewModel must keep the composer gated
        // and surface an explicit session error instead of pretending the locally
        // selected transcript was resumed.
        handle.submit(
            ClientCommand.ResumeSession(
                sessionId = canonicalSessionId(uuid),
                cwd = null,
            ),
        )
    }

    override suspend fun resumeEmptySession(uuid: String, title: String) {
        if (uuid.isBlank()) return
        handle.resumeEmptySession(
            sessionId = canonicalSessionId(uuid),
            title = title.ifBlank { strings.resolve(R.string.chat_new_conversation, "新对话") },
        )
    }

    override suspend fun newSession() {
        durableTurns.clear()
        handle.submit(ClientCommand.NewSession(cwd = null, model = null))
    }

    override suspend fun setModel(id: String) {
        if (id.isBlank()) return
        // Let the caller surface delivery/rejection errors; only ModelChanged
        // advances modelState, so a failed switch retains the confirmed model.
        handle.submit(ClientCommand.SetModel(model = id))
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

    override fun submit(text: String, images: List<ImageRefDto>): Flow<ReplyEvent> =
        submit(text, images, nextFallbackTurnId())

    override fun submit(text: String, images: List<ImageRefDto>, turnId: Long): Flow<ReplyEvent> =
        submit(text, images, turnId, null)

    override val visualizationHost: VisualizationHost? by lazy {
        runCatching { handle.visualizationHost(VISUALIZATION_ORIGIN) }.getOrNull()
    }

    override fun submit(
        text: String,
        images: List<ImageRefDto>,
        turnId: Long,
        visualizationContext: VisualizationRef?,
    ): Flow<ReplyEvent> =
        // Subscribe-before-submit: the returned reply stream maps the shared
        // engine flow through `mapReplyStream`, but the `SendPrompt` is fired
        // from `events.onSubscription { … }` — which runs ONLY AFTER this
        // collector is registered as a subscriber of the SharedFlow. That ordering
        // closes the race the prior `flow { submit(); emitAll(events…) }` had: a
        // `TextDelta` emitted by the engine's listener thread in the window between
        // `submit` returning and the collector subscribing is no longer dropped,
        // because the collector is already subscribed before the turn is spawned.
        mapReplyStream(
            events.onSubscription {
                try {
                    permissionIngress.beginTurn()
                    activeSession.value?.sessionId?.let { sessionId ->
                        durableTurns.begin(sessionId, turnId)
                    }
                    handle.submit(
                        ClientCommand.SendPrompt(
                            text = text,
                            promptMode = null,
                            images = images,
                            turnId = turnId.toULong(),
                            visualizationContext = visualizationContext?.toDto(),
                        ),
                    )
                } catch (t: Throwable) {
                    permissionIngress.endTurn()
                    // Inject the build/submit failure into the same stream the
                    // collector is already reading, so the mapper terminates it.
                    emit(
                        ClientEvent.Error(
                            kind = ErrorKindDto.TRANSPORT,
                            message = strings.resolve(
                                R.string.chat_error_engine_submit_failed,
                                "引擎错误：%1\$s",
                                "${t.message ?: t::class.simpleName}",
                            ),
                        ),
                    )
                }
            },
            strings,
        )

    override suspend fun cancel() {
        cancel(null)
    }

    override suspend fun cancel(turnId: Long?) {
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
        submitTurnCancellation(permissionIngress) {
            handle.submit(ClientCommand.Cancel(turnId = turnId?.toULong()))
        }
    }

    override suspend fun discardDurableTurn(turnId: Long) {
        cancel(turnId)
    }

    override fun close() {
        ConversationHeadlessRecovery.unregister(recoverySpec.scopeKey, this)
        eventRelay.close()
        workflowRelay.close()
        eventScope.cancel()
        runCatching { handle.destroy() }
    }

    override fun engineSourceForBackgroundRetention(): EngineConversationSource = this

    companion object {
        private val fallbackTurnIds = java.util.concurrent.atomic.AtomicLong(
            (System.currentTimeMillis() * 1_000L).coerceAtLeast(1L),
        )

        private fun nextFallbackTurnId(): Long = fallbackTurnIds.incrementAndGet()

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
        ): ConversationSource {
            val recoverySpec = ConversationRecoverySpec(
                projectId = projectWorkspace?.projectId,
                hostPath = projectWorkspace?.hostPath,
                sessionMode = sessionMode,
                linuxRuntimeMode = linuxRuntimeMode,
                workspaceKey = workspaceKey,
            )
            if (reuseProcessSource) {
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
            val eventRelay = LosslessEventRelay<ClientEvent>(eventScope)
            val workflowRelay = LosslessEventRelay<WorkflowProgressUpdate>(eventScope)
            // Resolves user-facing copy in the app's actual selected language
            // (via Context.getString, so it honors AppLanguageStore's locale
            // wrap) for every non-Composable emission site below.
            val strings = conversationStrings(context)
            val durableTurns = DurableConversationTurnClientStore(
                context = context,
                scope = workspaceKey ?: projectWorkspace?.hostPath ?: "__global__",
            )
            val durableReplayGate = DurableTurnReplayGate()
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
            // The engine's most recent active-session transition (sibling of
            // `sessions`). The listener below folds both `SessionStarted` and
            // `SessionResumed` into this StateFlow so the ViewModel adopts the
            // real session id and any restored transcript.
            val activeSession = MutableStateFlow<ActivatedSession?>(null)
            // The engine's REAL MCP listing (sibling of `models`). The listener
            // folds every inbound `McpServers` into this StateFlow; empty until a
            // `RefreshListings(Mcp)` reply lands (settings shows the mock list).
            val mcp = MutableStateFlow(emptyList<MCPServer>())
            // Credentials: the encrypted-at-rest SecureKeyStore FIRST (the shipped
            // app's source of truth — SHIP-BLOCKER #1), falling back to the process
            // environment as a dev override. A shipped mobile app has no process
            // env, so the key normally comes from the secure store the Settings
            // screen writes; ANTHROPIC_API_KEY only ever overrides on a dev host.
            val store = SecureKeyStore.create(context)
            val creds = resolveEngineCredentials(
                storedKey = store?.apiKey() ?: "",
                storedBase = store?.apiBase() ?: "",
                env = System.getenv(),
            )
            val providerLaunch = ProviderSettingsRepository(context).engineLaunchConfig()
            val handle = buildVoiceEngine(
                context = context,
                apiBase = creds.apiBase,
                apiKey = creds.apiKey,
                // Empty `creds.model` → the engine starts on MobileConfig.default_model
                // (a real Anthropic wire id), never a branded `lx-*` mock id. A
                // The shared engine restores the last confirmed model for an
                // interactive launch; this is the provider fallback for new installs.
                model = creds.model.ifBlank { providerLaunch.defaultModel },
                providerProfilesJson = providerLaunch.providerProfilesJson,
                routingJson = providerLaunch.routingJson,
                visionDelegationEnabled = providerLaunch.visionDelegationEnabled,
                projectWorkspace = projectWorkspace,
                sessionMode = sessionMode,
                linuxRuntimeMode = linuxRuntimeMode,
                onEvent = { event ->
                    // The OUT-OF-BAND state paths: fold model catalog + session
                    // catalog + live-resume events into the StateFlows the picker /
                    // drawer / conversation observe, BEFORE forwarding to the
                    // per-turn stream. Every reducer is a no-op for unrelated
                    // events, so every event still reaches `events` unchanged
                    // (lifecycle events like SessionStarted ride the per-turn
                    // stream; the ViewModel acts on them there).
                    models.value = reduceModelEvent(models.value, event)
                    sessions.value = reduceSessionEvent(sessions.value, event, strings = sessionCatalogStrings(context))
                    sessionActivationFrom(event, strings)?.let { activeSession.value = it }
                    // Out-of-band MCP listing: fold `McpServers` into its StateFlow.
                    if (event is ClientEvent.McpServers) mcp.value = event.servers.map { it.toMcpServer() }
                    if (event is ClientEvent.TurnStarted) permissionIngress.confirmTurnStarted()
                    if (event is ClientEvent.PermissionRequestResolved) {
                        permissionIngress.resolve(event.requestId)
                    }
                    if (
                        event is ClientEvent.TurnRecoveryState &&
                            event.snapshot.state in setOf(
                                TurnRecoveryStateDto.COMPLETED,
                                TurnRecoveryStateDto.FAILED,
                                TurnRecoveryStateDto.CANCELLED,
                            )
                    ) {
                        durableTurns.clear(
                            sessionId = event.snapshot.sessionId,
                            turnId = event.snapshot.turnId.toLong(),
                        )
                    }
                    if (event is ClientEvent.TurnEnded || event is ClientEvent.Error) {
                        permissionIngress.endTurn()
                    }
                    if (durableReplayGate.shouldForward(event)) {
                        eventRelay.offer(event)
                    }
                },
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
            // Ask the engine for its REAL catalog now that the handle exists; the
            // reply (`ModelList`) flows back through the listener above into the
            // `models` StateFlow, populating the picker with real wire ids.
            // `handle.submit` is suspend, so fire it off the calling thread — a
            // failed ListModels leaves the catalog explicitly empty; it never
            // blocks building the source.
            CoroutineScope(Dispatchers.Default).launch {
                try {
                    handle.submit(ClientCommand.ListModels)
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
                    handle.submit(ClientCommand.ListSessionAgents)
                } catch (_: Throwable) {
                    // Status is best-effort; a later foreground refresh retries.
                }
            }
            val source = EngineConversationSource(
                handle = handle,
                events = eventRelay.events,
                eventRelay = eventRelay,
                workflowEvents = workflowRelay.events,
                workflowRelay = workflowRelay,
                eventScope = eventScope,
                permissions = permissions,
                permissionIngress = permissionIngress,
                models = models,
                sessions = sessions,
                activeSession = activeSession,
                mcp = mcp,
                strings = strings,
                durableTurns = durableTurns,
                durableReplayGate = durableReplayGate,
                autoAttachDurableTurns = !reuseProcessSource,
                resumeOnUiAttach = reuseProcessSource,
                recoverySpec = recoverySpec,
            )
            ConversationHeadlessRecovery.register(recoverySpec.scopeKey, source)
            return source
        }
    }
}
