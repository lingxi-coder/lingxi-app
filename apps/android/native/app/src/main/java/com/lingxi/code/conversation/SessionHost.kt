package com.lingxi.code.conversation

import android.content.Context
import androidx.compose.runtime.MutableState
import androidx.compose.runtime.mutableLongStateOf
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
import com.lingxi.code.voice.audio.VoiceSpeechPlayer
import kotlinx.coroutines.Job
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.combine
import kotlinx.coroutines.flow.collect
import kotlinx.coroutines.launch
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import java.util.UUID

/** Draft restoration bookkeeping follows its owner across screen recreation. */
internal class ConversationDraftOwnership {
    val scope = mutableStateOf<ConversationScope?>(null)
    val sessionId = mutableStateOf<String?>(null)
    val revision = mutableLongStateOf(0L)
    val loaded = mutableStateOf(false)
    val mayRestoreSaved = mutableStateOf(true)
}

/** Owns complete conversation projections for the lifetime of the app process. */
internal class SessionHost(private val context: Context, initialGeneration: Int) : ViewModel() {
    private val mutex = Mutex()
    val voiceAdmission = Mutex()
    val pendingVoicePermission = java.util.concurrent.atomic.AtomicReference<FlowVoiceController?>(null)
    private val entries = mutableListOf<Entry>()
    private val identities = SessionOwnerRegistry<Entry>()
    private val selectedModeWrites = OrderedSelectionWriter(viewModelScope)
    private val _busy = MutableStateFlow(false)
    val busy: StateFlow<Boolean> = _busy.asStateFlow()
    private val appContext = context.applicationContext
    @Volatile private var ownerClaimsAllowed: () -> Boolean = { true }

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
            entry.mode == announcedMode &&
                registerOwner(scope.persistenceKey(), id, announcedMode, entry)
        }
        entry.observers += viewModelScope.launch {
            vm.state.collect { state ->
                if (state.streaming) entry.flowController?.let { controller ->
                    val row = state.streamingMessage ?: state.messages.lastOrNull()
                    controller.updateReply(row?.takeIf { it.role == com.lingxi.code.model.Role.Ai }?.text.orEmpty(), true)
                }
            }
        }
        entry.observers += viewModelScope.launch {
            vm.turnCompletions.collect { completion ->
                if (completion.origin == ConversationTurnOrigin.Flow) {
                    entry.flowController?.updateReply(
                        completion.finalAssistantText.takeIf { completion.outcome == ConversationTurnOutcome.Completed }.orEmpty(),
                        false,
                    )
                }
            }
        }
        entry.observers += viewModelScope.launch {
            combine(vm.state, vm.pendingPermission) { state, permission ->
                state.blocksEngineReconnect(permission != null)
            }.collect {
                refreshBusy()
            }
        }
        return entry
    }

    private val initial = createEntry(null, initialGeneration - 1)
    private var bootstrapped = false
    private val _selected = MutableStateFlow(initial.viewModel)
    val selected: StateFlow<ChatViewModel> = _selected.asStateFlow()
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
        return entry.flowController ?: create().also { entry.flowController = it }
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
    fun hasRegisteredOwner(vm: ChatViewModel): Boolean {
        val state = vm.state.value
        val mode = state.sessionMode ?: return false
        return identities.find(vm.sourceScope.value.persistenceKey(), canonicalSessionId(state.session.id), mode)?.viewModel === vm
    }
    private fun registerOwner(workspace: String, id: String, mode: SessionMode, entry: Entry): Boolean =
        synchronized(SessionAdmissionGate.lock) {
            identities.register(workspace, id, mode, entry, allowNew = ownerClaimsAllowed())
        }
    fun register(vm: ChatViewModel, scope: ConversationScope, mode: SessionMode) {
        val id = canonicalSessionId(vm.state.value.session.id)
        val entry = entries.first { it.viewModel === vm }
        require(id.isNotBlank() && id != "new")
        check(registerOwner(scope.persistenceKey(), id, mode, entry))
    }

    /** Stage an owner without changing the visible conversation. */
    suspend fun acquire(
        scope: ConversationScope,
        mode: SessionMode,
        target: SessionRef?,
        generation: Int,
        expectedRuntimeGeneration: String? = null,
        createSource: suspend () -> ConversationSource,
    ): Lease? = mutex.withLock {
        if (!ownerClaimsAllowed()) return@withLock null
        val (owner, created) = resolve(scope, mode, target, generation, expectedRuntimeGeneration, createSource)
            ?: return@withLock null
        owner.leases++
        Lease(owner.viewModel, created)
    }

    /** The initial placeholder participates in the same single-flight registry. */
    suspend fun bootstrap(
        scope: ConversationScope,
        mode: SessionMode,
        target: SessionRef?,
        generation: Int,
        createSource: suspend () -> ConversationSource,
    ) = mutex.withLock {
        if (!ownerClaimsAllowed()) return@withLock
        if (initial.viewModel.engineSource.value !is ConnectingConversationSource) return@withLock
        val id = target?.id?.let(::canonicalSessionId)?.takeUnless { it.isBlank() || it == "new" }
        if (id != null && identities.contains(id)) return@withLock
        var source: ConversationSource? = null
        try {
            source = createSource()
            val prepared = checkNotNull(source)
            if (!ownerClaimsAllowed()) return@withLock
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
        } finally {
            source?.let {
                if (ConversationHeadlessRecovery.protectsSource(it)) it.recoverySpec?.ownerKey?.let(ConversationHeadlessRecovery::releaseUiClaim)
                else it.close()
            }
        }
    }

    fun activate(lease: Lease) { _selected.value = lease.viewModel }

    suspend fun release(lease: Lease, discard: Boolean = false) = mutex.withLock {
        val entry = entries.firstOrNull { it.viewModel === lease.viewModel } ?: return@withLock
        entry.leases--
        check(entry.leases >= 0)
        if (discard && lease.created) entry.discardWhenUnleased = true
        if (!discard) entry.discardWhenUnleased = false
        if (entry.discardWhenUnleased && entry.leases == 0 && _selected.value !== entry.viewModel &&
            !ConversationHeadlessRecovery.protectsSource(entry.viewModel.engineSource.value) &&
            !entry.viewModel.state.value.blocksWorkspaceReplacement(entry.viewModel.pendingPermission.value != null, true)) {
            identities.removeOwner(entry)
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
            source?.let {
                if (ConversationHeadlessRecovery.protectsSource(it)) it.recoverySpec?.ownerKey?.let(ConversationHeadlessRecovery::releaseUiClaim)
                else it.close()
            }
        }
    }

    override fun onCleared() {
        entries.toList().forEach { it.flowController?.dispose(); it.speechPlayer?.dispose(); it.store.clear() }
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
