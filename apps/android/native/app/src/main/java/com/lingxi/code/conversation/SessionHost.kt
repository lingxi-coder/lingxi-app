package com.lingxi.code.conversation

import android.content.Context
import androidx.compose.runtime.MutableState
import androidx.compose.runtime.mutableLongStateOf
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateOf
import androidx.lifecycle.ViewModel
import androidx.lifecycle.ViewModelProvider
import androidx.lifecycle.ViewModelStore
import androidx.lifecycle.viewModelScope
import com.lingxi.code.model.ConversationScope
import com.lingxi.code.model.SessionMode
import com.lingxi.code.model.SessionRef
import com.lingxi.code.model.canonicalSessionId
import com.lingxi.code.model.persistenceKey
import com.lingxi.code.voice.FlowVoiceController
import com.lingxi.code.voice.VoiceCapturePhase
import com.lingxi.code.voice.VoiceCaptureStore
import com.lingxi.code.voice.audio.DeviceAudioError
import com.lingxi.code.voice.audio.VoiceSpeechPlayer
import kotlinx.coroutines.Job
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asSharedFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.combine
import kotlinx.coroutines.flow.collect
import kotlinx.coroutines.flow.collectLatest
import kotlinx.coroutines.launch
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import java.util.UUID

/** Draft restoration bookkeeping follows its owner across screen recreation. */
internal class ConversationDraftOwnership {
    val scope = mutableStateOf<ConversationScope?>(null)
    val sessionId = mutableStateOf<String?>(null)
    val revision = mutableLongStateOf(0L)
    val attachmentRevision = mutableLongStateOf(0L)
    val loaded = mutableStateOf(false)
    val mayRestoreSaved = mutableStateOf(true)
    val rejected = mutableStateListOf<RejectedComposerDraft>()
}

internal data class RejectedComposerDraft(
    val text: String,
    val attachment: ComposerAttachment?,
    val visualization: VisualizationContextChip? = null,
    val scope: ConversationScope? = null,
    val sessionId: String? = null,
)

/** Owns complete conversation projections for the lifetime of the app process. */
internal class SessionHost(private val context: Context, initialGeneration: Int) : ViewModel() {
    internal fun <T> launchAuthoringOperation(work: suspend () -> T, isCurrent: () -> Boolean,
        hiddenFailure: (Throwable) -> Unit, complete: (Result<T>, Boolean) -> Unit): Job =
        com.lingxi.code.apps.launchOwnerScopedAuthoringOperation(viewModelScope, work, isCurrent, hiddenFailure, complete)
    private val authoringPickerTickets = com.lingxi.code.apps.AuthoringPickerTickets()
    internal fun retainAuthoringPickerTicket(operation: com.lingxi.code.apps.AppAuthoringOperationHost): String =
        authoringPickerTickets.retain(operation)
    internal fun takeAuthoringPickerTicket(ticket: String): com.lingxi.code.apps.AppAuthoringOperationHost? =
        authoringPickerTickets.take(ticket)
    private val mutex = Mutex()
    val voiceAdmission = Mutex()
    val pendingVoicePermission = java.util.concurrent.atomic.AtomicReference<FlowVoiceController?>(null)
    private val entries = mutableListOf<Entry>()
    private val identities = SessionOwnerRegistry<Entry>()
    private val lastSelectedByMode = mutableMapOf<SessionMode, Entry>()
    private val selectedModeWrites = OrderedSelectionWriter(viewModelScope)
    private val _busy = MutableStateFlow(false)
    val busy: StateFlow<Boolean> = _busy.asStateFlow()
    private val appContext = context.applicationContext
    @Volatile private var ownerClaimsAllowed: () -> Boolean = { true }
    private val automaticSpeechGate = AutomaticReplySpeechGate { automaticSpeechBlocked() }
    private val _automaticSpeechErrors = MutableSharedFlow<Pair<ChatViewModel, DeviceAudioError>>(extraBufferCapacity = 8)
    val automaticSpeechErrors = _automaticSpeechErrors.asSharedFlow()

    private data class Entry(
        var mode: SessionMode?,
        val viewModel: ChatViewModel,
        val store: ViewModelStore,
        val draft: MutableState<String> = mutableStateOf(""),
        val draftOwnership: ConversationDraftOwnership = ConversationDraftOwnership(),
        val attachment: MutableState<ComposerAttachment?> = mutableStateOf(null),
        val flowVisible: MutableState<Boolean> = mutableStateOf(false),
        var flowController: FlowVoiceController? = null,
        var speechPlayer: VoiceSpeechPlayer? = null,
        val observers: MutableList<Job> = mutableListOf(),
        var leases: Int = 0,
        var discardWhenUnleased: Boolean = false,
    )

    data class Lease internal constructor(val viewModel: ChatViewModel, internal val created: Boolean)
    data class RetainedDestination(val scope: ConversationScope, val session: SessionRef)

    private fun releaseUnadoptedSource(source: ConversationSource) {
        // The wrapper owns its collectors even when its delegate is retained by
        // the service; close releases the UI claim without closing that delegate.
        if (source is RecoveringConversationSource) source.close()
        else if (ConversationHeadlessRecovery.protectsSource(source)) {
            source.recoverySpec?.ownerKey?.let(ConversationHeadlessRecovery::releaseUiClaim)
        } else source.close()
    }

    private fun refreshBusy() {
        _busy.value = entries.any { owner ->
            owner.viewModel.state.value.blocksEngineReconnect(owner.viewModel.pendingPermission.value != null)
        }
    }

    private fun createEntry(mode: SessionMode?, generation: Int): Entry {
        val store = ViewModelStore()
        val factory = object : ViewModelProvider.Factory {
            @Suppress("UNCHECKED_CAST")
            override fun <T : ViewModel> create(modelClass: Class<T>): T = ChatViewModel(
                source = ConnectingConversationSource(conversationStrings(appContext)),
                sourceGeneration = generation,
                strings = conversationStrings(appContext),
                backgroundExecution = AndroidConversationBackgroundExecution(appContext),
            ) as T
        }
        val vm = ViewModelProvider(store, factory)[UUID.randomUUID().toString(), ChatViewModel::class.java]
        val entry = Entry(mode, vm, store)
        entries += entry
        vm.setSessionOwnerAdmission { scope, id, announcedMode ->
            if (entry.mode != announcedMode) SessionOwnerAdmission.Conflict
            else admitOwner(scope.persistenceKey(), id, announcedMode, entry)
        }
        entry.observers += viewModelScope.launch {
            var observedSessionId: String? = null
            vm.state.collect { state ->
                val id = canonicalSessionId(state.session.id)
                if (id != observedSessionId) {
                    observedSessionId = id
                    SessionAdmissionGate.runtimeSourceChanged(entry)
                }
                if (state.streaming) entry.flowController?.let { controller ->
                    val row = state.streamingMessage ?: state.messages.lastOrNull()
                    controller.updateReply(row?.takeIf { it.role == com.lingxi.code.model.Role.Ai }?.text.orEmpty(), true)
                }
            }
        }
        entry.observers += viewModelScope.launch {
            vm.engineSource.collectLatest { source ->
                source.workspaceState.collect { SessionAdmissionGate.runtimeSourceChanged(entry) }
            }
        }
        entry.observers += viewModelScope.observeOwnerTurnCompletions(
            completions = vm.turnCompletions,
            autoPlay = automaticSpeechGate::acceptsCompletion,
            flowVisible = { entry.flowVisible.value },
            onFlow = { completion ->
                entry.flowController?.updateReply(
                    completion.finalAssistantText.takeIf { completion.outcome == ConversationTurnOutcome.Completed }.orEmpty(),
                    false,
                )
            },
            onAutomaticReply = { text ->
                val player = speechPlayer(vm)
                vm.engineSource.value.bindAudioOwner(player.audioOwner)
                player.speakAsync(
                    text,
                    automatic = true,
                    awaitStart = automaticSpeechGate::awaitStart,
                    mayStart = automaticSpeechGate::mayStart,
                    retryBusy = true,
                    onError = { error -> _automaticSpeechErrors.tryEmit(vm to error) },
                )
            },
        )
        entry.observers += viewModelScope.launch {
            combine(vm.state, vm.pendingPermission) { state, permission ->
                state.blocksEngineReconnect(permission != null)
            }.collect {
                refreshBusy()
                automaticSpeechGate.signal()
            }
        }
        return entry
    }

    private val initial = createEntry(null, initialGeneration - 1)
    private var bootstrapped = false
    private val _selected = MutableStateFlow(initial.viewModel)
    val selected: StateFlow<ChatViewModel> = _selected.asStateFlow()
    init {
        viewModelScope.launch { VoiceCaptureStore.state.collect { automaticSpeechGate.signal() } }
    }

    fun configureAutomaticSpeech(enabled: Boolean, foreground: Boolean) = automaticSpeechGate.configure(enabled, foreground)
    fun setAutomaticSpeechForeground(foreground: Boolean) = automaticSpeechGate.setForeground(foreground)
    fun automaticSpeechStateChanged() = automaticSpeechGate.signal()
    fun yieldOtherOwnersAutomaticSpeech(requester: ChatViewModel) {
        entries.filter { it.viewModel !== requester }.forEach { it.speechPlayer?.yieldAutomaticPlayback() }
    }
    private fun automaticSpeechBlocked(): Boolean {
        if (pendingVoicePermission.get() != null || VoiceCaptureStore.state.value.phase in setOf(
                VoiceCapturePhase.PermissionRequired, VoiceCapturePhase.Starting,
                VoiceCapturePhase.Listening, VoiceCapturePhase.Stopping,
            )) return true
        val owner = entries.firstOrNull { it.viewModel === _selected.value } ?: return true
        return owner.flowVisible.value || owner.viewModel.pendingPermission.value != null || owner.viewModel.state.value.pendingQuestions.isNotEmpty() ||
            owner.flowController?.hasAudioWork() == true
    }
    fun needsBootstrap(vm: ChatViewModel): Boolean = vm === initial.viewModel && !bootstrapped
    fun rememberModeSelection(store: com.lingxi.code.project.ScopeStateStore, mode: SessionMode) {
        val owner = _selected.value
        selectedModeWrites.write(mode, store::persistSelectedMode) { owner.reportHostError(it.message.orEmpty()) }
    }
    suspend fun readSelectedMode(store: com.lingxi.code.project.ScopeStateStore): SessionMode? =
        selectedModeWrites.read { store.readSelectedMode() }

    fun draftState(vm: ChatViewModel): MutableState<String> = entries.first { it.viewModel === vm }.draft
    fun draftOwnership(vm: ChatViewModel): ConversationDraftOwnership = entries.first { it.viewModel === vm }.draftOwnership
    fun attachmentState(vm: ChatViewModel): MutableState<ComposerAttachment?> = entries.first { it.viewModel === vm }.attachment
    fun flowVisibleState(vm: ChatViewModel): MutableState<Boolean> = entries.first { it.viewModel === vm }.flowVisible
    fun flowController(vm: ChatViewModel, create: () -> FlowVoiceController): FlowVoiceController {
        val entry = entries.first { it.viewModel === vm }
        return entry.flowController ?: create().also { controller ->
            entry.flowController = controller
            entry.observers += viewModelScope.launch { controller.state.collect { automaticSpeechGate.signal() } }
        }
    }
    fun speechPlayer(vm: ChatViewModel): VoiceSpeechPlayer {
        val entry = entries.first { it.viewModel === vm }
        return entry.speechPlayer ?: VoiceSpeechPlayer(appContext, voiceAdmission).also { entry.speechPlayer = it }
    }
    fun allOwners(): List<ChatViewModel> = entries.map { it.viewModel }
    fun ownerHasVoiceWork(vm: ChatViewModel): Boolean = entries.firstOrNull { it.viewModel === vm }?.let {
        it.flowController?.hasAudioWork() == true || it.speechPlayer?.hasAudioWork() == true
    } == true
    fun cancelComputerUseTurns(inFlightOwners: Set<String>) {
        viewModelScope.launch {
            entries.map { it.viewModel }.filter {
                it.engineSource.value.recoverySpec?.ownerKey in inFlightOwners || it.hasActiveComputerUseTurn()
            }.forEach { it.cancel() }
        }
    }
    fun find(scope: ConversationScope, sessionId: String): ChatViewModel? =
        identities.find(scope.persistenceKey(), canonicalSessionId(sessionId))?.viewModel

    /** In-memory last selection, revalidated against the exact live owner before use. */
    fun retainedDestination(mode: SessionMode): RetainedDestination? = synchronized(SessionAdmissionGate.lock) {
        val entry = lastSelectedByMode[mode] ?: return@synchronized null
        val owner = entry.viewModel
        val scope = owner.sourceScope.value
        val session = owner.state.value.session
        val lease = Lease(owner, false)
        if (isRetainedLeaseCurrent(lease, scope, mode, session.id)) RetainedDestination(scope, session) else null
    }

    /** Re-select an already admitted owner without waiting for another owner's disk write. */
    suspend fun acquireRetained(
        scope: ConversationScope,
        mode: SessionMode,
        sessionId: String,
        expectedRuntimeGeneration: String? = null,
        expectedTranscriptPath: String? = null,
    ): Lease? = mutex.withLock {
        val entry = identities.find(scope.persistenceKey(), canonicalSessionId(sessionId), mode) ?: return@withLock null
        val lease = Lease(entry.viewModel, false)
        if (!isRetainedLeaseCurrent(lease, scope, mode, sessionId, expectedRuntimeGeneration, expectedTranscriptPath)) return@withLock null
        entry.leases++
        lease
    }

    fun isRetainedLeaseCurrent(
        lease: Lease,
        scope: ConversationScope,
        mode: SessionMode,
        sessionId: String,
        expectedRuntimeGeneration: String? = null,
        expectedTranscriptPath: String? = null,
    ): Boolean = synchronized(SessionAdmissionGate.lock) {
        val id = canonicalSessionId(sessionId)
        val entry = identities.find(scope.persistenceKey(), id, mode) ?: return@synchronized false
        val owner = entry.viewModel
        val state = owner.state.value
        val source = owner.engineSource.value
        val spec = source.recoverySpec ?: return@synchronized false
        val path = spec.transcriptPath?.takeIf { it.startsWith('/') } ?: return@synchronized false
        val workspace = source.workspaceState.value
        val runtime = SessionAdmissionGate.runtimeIdentity(id) ?: return@synchronized false
        entry.viewModel === lease.viewModel && !owner.sessionOwnerRejected && owner.sourceScope.value == scope &&
            state.sessionReady && !state.sessionTransitioning && state.sessionMode == mode &&
            canonicalSessionId(state.session.id) == id && runtime.workspace == scope.persistenceKey() &&
            runtime.mode == mode && runtime.transcriptPath == path &&
            (expectedTranscriptPath == null || selectedTranscriptPathMatches(expectedTranscriptPath, path)) &&
            (workspace == null || (canonicalSessionId(workspace.sessionId) == id && workspace.transcriptPath == path)) &&
            (expectedRuntimeGeneration == null || spec.runtimeGeneration == expectedRuntimeGeneration)
    }
    fun hasRegisteredOwner(vm: ChatViewModel): Boolean {
        val state = vm.state.value
        val mode = state.sessionMode ?: return false
        return identities.find(vm.sourceScope.value.persistenceKey(), canonicalSessionId(state.session.id), mode)?.viewModel === vm
    }
    private fun admitOwner(workspace: String, id: String, mode: SessionMode, entry: Entry): SessionOwnerAdmission =
        synchronized(SessionAdmissionGate.lock) {
            val canonicalId = canonicalSessionId(id)
            if (!SessionAdmissionGate.mayRegisterRuntimeOwner(canonicalId, workspace, mode, entry)) {
                return@synchronized if (SessionAdmissionGate.publicationPending(canonicalId)) SessionOwnerAdmission.Deferred
                else SessionOwnerAdmission.Conflict
            }
            val registered = identities.register(workspace, canonicalId, mode, entry, allowNew = ownerClaimsAllowed())
            if (registered) SessionAdmissionGate.recordRuntimeOwner(canonicalId, workspace, mode, entry) {
                    entry.viewModel.engineSource.value.recoverySpec?.transcriptPath
                        .takeIf { canonicalSessionId(entry.viewModel.state.value.session.id) == canonicalId }
            }
            if (registered && _selected.value === entry.viewModel) lastSelectedByMode[mode] = entry
            if (registered) SessionOwnerAdmission.Accepted else SessionOwnerAdmission.Conflict
        }

    private fun registerOwner(workspace: String, id: String, mode: SessionMode, entry: Entry): Boolean =
        admitOwner(workspace, id, mode, entry) == SessionOwnerAdmission.Accepted
    fun register(vm: ChatViewModel, scope: ConversationScope, mode: SessionMode) {
        val id = canonicalSessionId(vm.state.value.session.id)
        val entry = entries.first { it.viewModel === vm }
        require(id.isNotBlank() && id != "new")
        check(registerOwner(scope.persistenceKey(), id, mode, entry))
    }

    /** Reconnect keeps the existing owner while it releases and rebuilds its native source. */
    suspend fun awaitSourceReplacement(vm: ChatViewModel): AutoCloseable {
        val id = canonicalSessionId(vm.state.value.session.id).takeUnless { it.isBlank() || it == "new" }
            ?: return AutoCloseable { }
        val token = SessionAdmissionGate.awaitSourcePreparation(id)
        return AutoCloseable { SessionAdmissionGate.finishSourcePreparation(token) }
    }

    /** Stage an owner without changing the visible conversation. */
    suspend fun acquire(
        scope: ConversationScope,
        mode: SessionMode,
        target: SessionRef?,
        generation: Int,
        expectedRuntimeGeneration: String? = null,
        isCurrent: () -> Boolean = { true },
        createSource: suspend () -> ConversationSource,
    ): Lease? {
        val id = target?.id?.let(::canonicalSessionId)?.takeUnless { it.isBlank() || it == "new" }
        val preparation = if (id != null && !identities.contains(id)) SessionAdmissionGate.awaitSourcePreparation(id) else null
        var preparedSource: ConversationSource? = null
        return try {
            // An existing owner is immediately reusable. Never keep this mutex
            // across a new engine's potentially slow native construction: an
            // unrelated retained owner must remain available for navigation.
            mutex.withLock {
                if (!isCurrent() || !ownerClaimsAllowed()) return null
                if (id != null && identities.contains(id)) {
                    val (owner, created) = resolve(scope, mode, target, generation, expectedRuntimeGeneration, preparation) {
                        error("An existing owner must not construct another source")
                    } ?: return null
                    owner.leases++
                    return Lease(owner.viewModel, created)
                }
                if (id != null && preparation == null) return null
            }
            if (!isCurrent()) return null
            preparedSource = createSource()
            mutex.withLock {
                if (!isCurrent() || !ownerClaimsAllowed()) return@withLock null
                val (owner, created) = resolve(scope, mode, target, generation, expectedRuntimeGeneration, preparation) {
                    checkNotNull(preparedSource).also { preparedSource = null }
                } ?: return@withLock null
                owner.leases++
                Lease(owner.viewModel, created)
            }
        } finally {
            preparedSource?.let(::releaseUnadoptedSource)
            preparation?.let(SessionAdmissionGate::finishSourcePreparation)
        }
    }

    /** The initial placeholder participates in the same single-flight registry. */
    suspend fun bootstrap(
        scope: ConversationScope,
        mode: SessionMode,
        target: SessionRef?,
        generation: Int,
        createSource: suspend () -> ConversationSource,
    ) {
        val id = target?.id?.let(::canonicalSessionId)?.takeUnless { it.isBlank() || it == "new" }
        if (id != null && identities.contains(id)) return
        val preparation = id?.let { SessionAdmissionGate.awaitSourcePreparation(it) }
        var source: ConversationSource? = null
        try {
            val shouldBuild = mutex.withLock {
                if (!ownerClaimsAllowed() || initial.viewModel.engineSource.value !is ConnectingConversationSource ||
                    _selected.value !== initial.viewModel || (id != null && identities.contains(id))) return@withLock false
                true
            }
            if (!shouldBuild) return
            source = createSource()
            mutex.withLock {
                val prepared = checkNotNull(source)
                if (!ownerClaimsAllowed() || initial.viewModel.engineSource.value !is ConnectingConversationSource ||
                    _selected.value !== initial.viewModel || (id != null && identities.contains(id))) return@withLock
                if (prepared is UnavailableConversationSource) {
                    initial.viewModel.reportHostError(prepared.reason)
                    return@withLock
                }
                initial.mode = mode
                initial.viewModel.prepareInitialSessionRestore(target, scope)
                initial.viewModel.ensureSource(generation) { prepared }
                bootstrapped = true
                source = null
                if (id != null && !registerOwner(scope.persistenceKey(), id, mode, initial) && ownerClaimsAllowed()) {
                    error("Session owner conflicts with an existing owner")
                }
            }
        } finally {
            source?.let(::releaseUnadoptedSource)
            preparation?.let(SessionAdmissionGate::finishSourcePreparation)
        }
    }

    fun activate(lease: Lease) {
        val entry = entries.first { it.viewModel === lease.viewModel }
        synchronized(SessionAdmissionGate.lock) { entry.mode?.let { lastSelectedByMode[it] = entry } }
        _selected.value = lease.viewModel
        automaticSpeechGate.signal()
    }

    suspend fun release(lease: Lease, discard: Boolean = false) = mutex.withLock {
        val entry = entries.firstOrNull { it.viewModel === lease.viewModel } ?: return@withLock
        entry.leases--
        check(entry.leases >= 0)
        if (discard && lease.created) entry.discardWhenUnleased = true
        if (!discard) entry.discardWhenUnleased = false
        if (entry.discardWhenUnleased && entry.leases == 0 && _selected.value !== entry.viewModel &&
            !ConversationHeadlessRecovery.protectsSource(entry.viewModel.engineSource.value) &&
            !entry.viewModel.state.value.blocksWorkspaceReplacement(entry.viewModel.pendingPermission.value != null, true)) {
            synchronized(SessionAdmissionGate.lock) {
                identities.removeOwner(entry)
                SessionAdmissionGate.removeRuntimeOwner(entry)
                lastSelectedByMode.entries.removeAll { it.value === entry }
            }
            entry.observers.forEach(Job::cancel)
            entries.remove(entry)
            entry.flowController?.dispose()
            entry.speechPlayer?.dispose()
            entry.store.clear()
            refreshBusy()
        }
    }

    private suspend fun resolve(
        scope: ConversationScope,
        mode: SessionMode,
        target: SessionRef?,
        generation: Int,
        expectedRuntimeGeneration: String?,
        sourcePreparation: Long?,
        createSource: suspend () -> ConversationSource,
    ): Pair<Entry, Boolean>? {
        val id = target?.id?.let(::canonicalSessionId)?.takeUnless { it.isBlank() || it == "new" }
        val key = id?.let { scope.persistenceKey() to it }
        val existing = key?.let { identities.find(it.first, it.second) }
        if (existing != null) {
            if (existing.viewModel.sessionOwnerRejected) {
                _selected.value.reportConversationLaunchFailed()
                return null
            }
            if (key == null || identities.find(key.first, key.second, mode) !== existing) return null
            if (expectedRuntimeGeneration != null &&
                existing.viewModel.engineSource.value.recoverySpec?.runtimeGeneration != expectedRuntimeGeneration) return null
            return existing to false
        }
        if (id != null && identities.contains(id)) return null
        if (id != null && sourcePreparation == null) return null
        var source: ConversationSource? = null
        try {
            source = createSource()
            val prepared = checkNotNull(source)
            if (!ownerClaimsAllowed()) return null
            if (prepared is UnavailableConversationSource) {
                _selected.value.reportHostError(prepared.reason)
                return null
            }
            if (expectedRuntimeGeneration != null && prepared.recoverySpec?.runtimeGeneration != expectedRuntimeGeneration) return null
            val entry = createEntry(mode, generation - 1)
            entry.viewModel.prepareInitialSessionRestore(target, scope)
            entry.viewModel.ensureSource(generation) { prepared }
            source = null
            if (key != null && !registerOwner(key.first, key.second, mode, entry) && ownerClaimsAllowed()) {
                error("Session owner conflicts with an existing owner")
            }
            return entry to true
        } finally {
            source?.let(::releaseUnadoptedSource)
        }
    }

    override fun onCleared() {
        entries.toList().forEach {
            SessionAdmissionGate.removeRuntimeOwner(it)
            it.flowController?.dispose(); it.speechPlayer?.dispose(); it.store.clear()
        }
        super.onCleared()
    }

    companion object {
        @Volatile private var processHost: SessionHost? = null
        fun get(context: Context, generation: Int, ownerClaimsAllowed: () -> Boolean): SessionHost {
            val host = processHost ?: synchronized(this) {
                processHost ?: SessionHost(context.applicationContext, generation).also { processHost = it }
            }
            host.ownerClaimsAllowed = ownerClaimsAllowed
            return host
        }
    }
}
