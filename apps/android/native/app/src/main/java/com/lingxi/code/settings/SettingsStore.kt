package com.lingxi.code.settings

import androidx.lifecycle.ViewModel
import androidx.lifecycle.ViewModelProvider
import androidx.lifecycle.viewModelScope
import androidx.compose.ui.graphics.Color
import com.lingxi.code.R
import com.lingxi.code.bindings.client.ProviderModelCatalogEntryDto
import com.lingxi.code.bindings.client.ClientEvent
import kotlinx.coroutines.Job
import kotlinx.coroutines.flow.collect
import com.lingxi.code.conversation.ConversationSource
import com.lingxi.code.model.CatalogModelDetails
import com.lingxi.code.model.ConnStatus
import com.lingxi.code.model.DreamConfig
import com.lingxi.code.model.GenericProvider
import com.lingxi.code.model.MCPServer
import com.lingxi.code.model.NotifConfig
import com.lingxi.code.model.ProviderKind
import com.lingxi.code.model.ProviderPreset
import com.lingxi.code.model.LlmProviderCatalogEntry
import com.lingxi.code.model.SettingsMock
import com.lingxi.code.model.Skill
import com.lingxi.code.voice.audio.AudioConfigurationNormalizer
import com.lingxi.code.voice.audio.AudioConfigurationV4
import android.content.Context
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock

/**
 * Mutable settings state — the Android analog of the iOS `SettingsStore`
 * (`ObservableObject`). Holds the mock provider / skill / MCP / Dream / voice
 * data plus the simple toggles (language, notifications, privacy switches) and
 * exposes them as a single [StateFlow]. Provider launch-affecting edits are
 * persisted immediately but remain explicitly pending until the user applies
 * them and the host rebuilds the mobile engine.
 *
 * A6 reads [SettingsUiState] for the main-list counts and drives the simple
 * pages' toggles/radios; A7/A8 build the provider / skills / MCP / Dream editors
 * on the same state. Appearance is *not* held here — it lives in the
 * DataStore-backed `AppearanceStore` so theme + accent persist and stay live.
 */
data class SettingsUiState(
    val llmProviders: List<GenericProvider> = emptyList(),
    val llmCatalogEntries: List<LlmProviderCatalogEntry> = emptyList(),
    val llmCatalogLoaded: Boolean = false,
    val accountModelAvailability: List<ProviderAccountModelAvailability> = emptyList(),
    val searchProviders: List<GenericProvider> = emptyList(),
    val fetchProviders: List<GenericProvider> = emptyList(),
    val voice: AudioConfigurationV4 = AudioConfigurationNormalizer.defaults,
    val voiceRevision: Long = 0,
    val voiceSaving: Boolean = false,
    val voiceSaveError: String? = null,
    val voiceCapability: VoiceCapabilitySnapshot = VoiceCapabilitySnapshot(),
    val linuxRuntime: LinuxRuntimeUiState = LinuxRuntimeUiState(),
    val skills: List<Skill> = emptyList(),
    val mcpServers: List<MCPServer> = SettingsMock.mcpServers(),
    val dream: DreamConfig = DreamConfig(),
    val language: String = "zh-CN",
    val notifs: NotifConfig = NotifConfig(),
    val pendingLlmProviderChanges: Set<String> = emptySet(),
    val visionDelegationEnabled: Boolean = true,
    val bioLock: Boolean = true,
    val telemetry: Boolean = false,
    val autoUpdate: Boolean = true,
    val permissionMode: String = "auto",
    val effectivePermissionMode: String = "auto",
    val permissionModeError: String? = null,
    val typescriptLspMode: String = "auto",
    val effectiveTypescriptLspMode: String = "auto",
    val typescriptLspAvailable: Boolean = true,
    val typescriptLspError: String? = null,
) {
    /** The execution Host projects enabled bindings, model permissions and current account health. */
    val needsLlmSetup: Boolean
        get() = accountModelAvailability.none { it.hasEligibleAccounts }

    /** Providers for a [ProviderKind] (used by A7's list page). */
    fun providers(kind: ProviderKind): List<GenericProvider> = when (kind) {
        ProviderKind.Llm -> llmProviders
        ProviderKind.Search -> searchProviders
        ProviderKind.Fetch -> fetchProviders
    }
}

class SettingsStore(
    private val providerRepo: ProviderSettingsRepository? = null,
    private val voiceRepo: AudioConfigurationRepository? = null,
    private val notifRepo: NotificationPrefsStore? = null,
    private val permissionModeRepo: PermissionModeSettingsRepository? = null,
    private val onVoiceConfigurationSaved: (AudioConfigurationV4) -> Unit = {},
    /**
     * Resolves a string resource id to its localized text. The production
     * factory wires the application context's `getString`; unit tests keep the
     * empty default (they never assert on these user-facing messages).
     */
    private val resolveString: (Int) -> String = { _ -> "" },
) : ViewModel() {
    /**
     * Adapts [resolveString] (empty-string in tests, real `Context.getString`
     * in production — see the factory below) to [SettingsMock.mcpServers]'s (id, fallback) shape: falls back to the
     * literal zh-Hans copy whenever [resolveString] has nothing (i.e. every
     * JVM test that constructs [SettingsStore] with no [Context] at all).
     */
    private fun resolveWithFallback(id: Int, fallback: String): String =
        resolveString(id).ifEmpty { fallback }

    private val initialPermissionMode: String = permissionModeRepo?.load() ?: "auto"
    private val initialAudioConfiguration = voiceRepo?.load()

    private val _state = MutableStateFlow(
        providerRepo?.loadProviderState()?.let { (llm, search, fetch) ->
            SettingsUiState(
                llmProviders = llm,
                searchProviders = search,
                fetchProviders = fetch,
                visionDelegationEnabled = providerRepo.visionDelegationEnabled(),
                permissionMode = initialPermissionMode,
                effectivePermissionMode = initialPermissionMode,
                voice = initialAudioConfiguration?.snapshot?.configuration ?: AudioConfigurationNormalizer.defaults,
                voiceRevision = initialAudioConfiguration?.snapshot?.revision ?: 0,
                voiceSaveError = initialAudioConfiguration?.persistenceError,
                notifs = notifRepo?.load() ?: NotifConfig(),
                mcpServers = SettingsMock.mcpServers(::resolveWithFallback),
                dream = DreamConfig(
                    lastRun = resolveWithFallback(
                        R.string.settings_dream_last_run_seed,
                        "今早 03:24 · 整理 7 条记忆 / 草拟今日计划",
                    ),
                ),
            )
        } ?: SettingsUiState(
            visionDelegationEnabled = providerRepo?.visionDelegationEnabled() ?: true,
            voice = initialAudioConfiguration?.snapshot?.configuration ?: AudioConfigurationNormalizer.defaults,
            voiceRevision = initialAudioConfiguration?.snapshot?.revision ?: 0,
            voiceSaveError = initialAudioConfiguration?.persistenceError,
            notifs = notifRepo?.load() ?: NotifConfig(),
            permissionMode = initialPermissionMode,
            effectivePermissionMode = initialPermissionMode,
            mcpServers = SettingsMock.mcpServers(::resolveWithFallback),
            dream = DreamConfig(
                lastRun = resolveWithFallback(
                    R.string.settings_dream_last_run_seed,
                    "今早 03:24 · 整理 7 条记忆 / 草拟今日计划",
                ),
            ),
        )
    )
    val state: StateFlow<SettingsUiState> = _state.asStateFlow()
    private var accountBindingGeneration = 0L

    val providerAccounts = ProviderAccountsRepository()
    private var providerAccountEvents: Job? = null

    fun bindProviderAccountSource(source: ConversationSource?) {
        providerAccounts.bindSource(source)
        providerAccountEvents?.cancel()
        providerAccountEvents = source?.let { activeSource -> viewModelScope.launch {
            launch { activeSource.clientEvents.collect { event ->
                if (event is ClientEvent.ProviderAccountRouting) providerAccounts.recordRouting(activeSource, event.eventJson)
            } }
            providerAccounts.state.collect { applyAccountSnapshot(it) }
        } }
        accountBindingGeneration += 1
        applyAccountSnapshot(providerAccounts.state.value)
        if (source != null) refreshProviderStatuses()
    }

    // --- simple toggles / radios (A6) --------------------------------------

    fun setLanguage(code: String) = _state.update { it.copy(language = code) }
    fun setBioLock(on: Boolean) = _state.update { it.copy(bioLock = on) }
    fun setTelemetry(on: Boolean) = _state.update { it.copy(telemetry = on) }
    fun setAutoUpdate(on: Boolean) = _state.update { it.copy(autoUpdate = on) }
    private val permissionModeUpdates = Mutex()

    fun setPermissionMode(mode: String, onApply: (suspend (String) -> Unit)? = null) {
        if (mode !in PermissionModeOptions.values) return
        viewModelScope.launch {
            permissionModeUpdates.withLock {
                try {
                    // Only a successfully applied explicit selection becomes the
                    // preference. Engine snapshots (including EnterPlan) remain
                    // separate, effective state and never overwrite this value.
                    if (onApply != null) {
                        // The engine persists the acknowledged user selection.
                        onApply(mode)
                    } else {
                        permissionModeRepo?.save(mode)
                    }
                    _state.update { it.copy(permissionMode = mode, permissionModeError = null) }
                } catch (error: CancellationException) {
                    throw error
                } catch (error: Exception) {
                    _state.update {
                        it.copy(permissionModeError = error.message ?: "permission mode could not be saved")
                    }
                }
            }
        }
    }

    fun clearPermissionModeError() = _state.update { it.copy(permissionModeError = null) }

    fun setEffectivePermissionMode(mode: String) {
        if (mode in PermissionModeOptions.values) {
            _state.update { it.copy(effectivePermissionMode = mode) }
        }
    }
    fun setTypescriptLspMode(mode: String, onApply: suspend (String) -> Unit) {
        if (mode !in TypeScriptLspModeOptions.values) return
        val previous = _state.value
        _state.update { it.copy(typescriptLspMode = mode, typescriptLspError = null) }
        viewModelScope.launch {
            runCatching { onApply(mode) }.onFailure { error ->
                _state.update {
                    it.copy(
                        typescriptLspMode = previous.typescriptLspMode,
                        effectiveTypescriptLspMode = previous.effectiveTypescriptLspMode,
                        typescriptLspAvailable = previous.typescriptLspAvailable,
                        typescriptLspError = error.message ?: "TypeScript LSP mode rejected",
                    )
                }
            }
        }
    }

    fun setTypescriptLspState(requested: String, effective: String, available: Boolean) {
        if (requested !in TypeScriptLspModeOptions.values || effective !in TypeScriptLspModeOptions.values) return
        _state.update {
            it.copy(
                typescriptLspMode = requested,
                effectiveTypescriptLspMode = effective,
                typescriptLspAvailable = available,
                typescriptLspError = null,
            )
        }
    }
    /**
     * Persists as well as updating state. This used to only touch the
     * in-memory copy and had ZERO callers — the notification settings screen
     * rendered four toggles that nothing read and nothing saved.
     */
    fun setNotifs(notifs: NotifConfig) {
        notifRepo?.save(notifs)
        // The notifier picks this up by collecting `state` in RootScreen — it
        // holds ARMED TIMERS and cannot notice a preference it is not told
        // about, and a callback field here would be one more thing that can be
        // left unset while still compiling.
        _state.update { it.copy(notifs = notifs) }
    }
    private val voiceUpdates = Mutex()

    /** Updates one or more voice fields against the latest serialized snapshot. */
    fun updateVoice(update: (AudioConfigurationV4) -> AudioConfigurationV4) {
        viewModelScope.launch {
            voiceUpdates.withLock {
                val current = _state.value
                val updatedVoice = update(current.voice)
                _state.update { it.copy(voiceSaving = true, voiceSaveError = null) }
                val repository = voiceRepo
                if (repository == null) {
                    _state.update {
                        it.copy(
                            voice = updatedVoice,
                            voiceSaving = false,
                            voiceSaveError = null,
                        )
                    }
                    return@withLock
                }
                val result = withContext(Dispatchers.IO) {
                    repository.save(updatedVoice, expectedRevision = current.voiceRevision)
                }
                when (result) {
                    is AudioConfigurationSaveResult.Saved -> {
                        val mediaSettingsError = runCatching {
                            onVoiceConfigurationSaved(result.snapshot.configuration)
                        }.exceptionOrNull()
                        _state.update {
                            it.copy(
                                voice = result.snapshot.configuration,
                                voiceRevision = result.snapshot.revision,
                                voiceSaving = false,
                                voiceSaveError = mediaSettingsError?.message
                                    ?: mediaSettingsError?.javaClass?.simpleName,
                            )
                        }
                    }
                    is AudioConfigurationSaveResult.Conflict -> _state.update {
                        it.copy(
                            voice = result.current.configuration,
                            voiceRevision = result.current.revision,
                            voiceSaving = false,
                            voiceSaveError = "Audio settings changed elsewhere. Please retry your edit.",
                        )
                    }
                    is AudioConfigurationSaveResult.Failed -> _state.update {
                        it.copy(voiceSaving = false, voiceSaveError = result.message)
                    }
                }
            }
        }
    }

    /** Replaces the configuration for non-editor callers; editors should use [updateVoice]. */
    fun setVoice(voice: AudioConfigurationV4) = updateVoice { voice }

    fun setVoiceCapability(snapshot: VoiceCapabilitySnapshot) =
        _state.update { it.copy(voiceCapability = snapshot) }

    fun setLlmCatalogEntries(entries: List<ProviderModelCatalogEntryDto>) {
        _state.update { state ->
            val existing = state.llmCatalogEntries.associateBy(LlmProviderCatalogEntry::profileId)
            state.copy(
                llmCatalogLoaded = true,
                llmCatalogEntries = entries.map { entry ->
                    val prior = existing[entry.providerId]
                    val presetId = if (entry.providerId == "gemini") "google" else entry.providerId
                    val preset = ProviderKind.Llm.presets.firstOrNull { it.id == presetId }
                    LlmProviderCatalogEntry(
                        profileId = entry.providerId,
                        displayName = entry.providerLabel,
                        baseUrl = entry.definition?.baseUrl ?: prior?.baseUrl ?: preset?.defaultUrl.orEmpty(),
                        protocol = entry.definition?.protocol ?: prior?.protocol ?: preset?.sub.orEmpty(),
                        auth = entry.definition?.auth ?: prior?.auth.orEmpty(),
                        credentialEnv = entry.definition?.credentialEnv ?: prior?.credentialEnv,
                        modelIds = entry.models.map { it.modelId },
                        modelDetails = entry.models.map(CatalogModelDetails::fromDto),
                    )
                },
            )
        }
    }

    @Synchronized
    fun tryBeginLinuxRuntimeAction(
        action: LinuxRuntimeAction,
        mode: LinuxRuntimeMode,
    ): Boolean {
        val current = _state.value
        if (current.linuxRuntime.selectedMode != mode || current.linuxRuntime.busyAction != null) {
            return false
        }
        _state.value = current.copy(
            linuxRuntime = current.linuxRuntime.copy(busyAction = action),
        )
        return true
    }

    @Synchronized
    fun completeLinuxRuntimeAction(
        action: LinuxRuntimeAction,
        mode: LinuxRuntimeMode,
        snapshot: LinuxRuntimeUiState,
    ) {
        val current = _state.value
        if (current.linuxRuntime.selectedMode != mode || current.linuxRuntime.busyAction != action) {
            return
        }
        _state.value = current.copy(linuxRuntime = snapshot.copy(busyAction = null))
    }

    @Synchronized
    fun failLinuxRuntimeAction(
        action: LinuxRuntimeAction,
        mode: LinuxRuntimeMode,
        message: String,
    ) {
        val current = _state.value
        if (current.linuxRuntime.selectedMode != mode || current.linuxRuntime.busyAction != action) {
            return
        }
        _state.value = current.copy(
            linuxRuntime = current.linuxRuntime.copy(
                summaryRes = R.string.settings_linux_op_failed,
                detail = message,
                lastAction = action,
                lastActionMessage = message,
                busyAction = null,
            ),
        )
    }

    @Synchronized
    fun cancelLinuxRuntimeAction(action: LinuxRuntimeAction, mode: LinuxRuntimeMode) {
        val current = _state.value
        if (current.linuxRuntime.selectedMode != mode || current.linuxRuntime.busyAction != action) {
            return
        }
        _state.value = current.copy(
            linuxRuntime = current.linuxRuntime.copy(busyAction = null),
        )
    }
    // --- provider store access by kind (A7) --------------------------------

    private fun setProviders(kind: ProviderKind, value: List<GenericProvider>) {
        _state.update {
            when (kind) {
                ProviderKind.Llm -> it.copy(llmProviders = value)
                ProviderKind.Search -> it.copy(searchProviders = value)
                ProviderKind.Fetch -> it.copy(fetchProviders = value)
            }
        }
        providerRepo?.persistProviders(kind, _state.value.providers(kind))
    }

    fun updateProvider(kind: ProviderKind, id: String, mutate: (GenericProvider) -> GenericProvider) {
        val before = _state.value.providers(kind).firstOrNull { it.id == id }
        val updated = _state.value.providers(kind).map {
            if (it.id != id) it else {
                val next = mutate(it)
                if (kind == ProviderKind.Llm && ProviderSettingsRepository.profileNameFor(it) != ProviderSettingsRepository.profileNameFor(next)) {
                    next.copy(credentialConfigured = false, status = ConnStatus.Idle)
                } else next
            }
        }
        setProviders(kind, updated)
        val after = updated.firstOrNull { it.id == id }
        if (
            kind == ProviderKind.Llm &&
            before != null &&
            after != null &&
            providerLaunchConfigurationChanged(before, after)
        ) {
            markLlmProviderConfigurationPending(id)
        }
    }

    fun setDefaultProvider(kind: ProviderKind, id: String) {
        val previousDefault = _state.value.providers(kind).firstOrNull { it.isDefault }?.id
        setProviders(kind, _state.value.providers(kind).map { it.copy(isDefault = it.id == id) })
        if (kind == ProviderKind.Llm && previousDefault != id) {
            markLlmProviderConfigurationPending(id)
        }
    }

    fun removeProvider(
        kind: ProviderKind,
        id: String,
        onDone: (String?) -> Unit = {},
    ) {
        if (_state.value.providers(kind).none { it.id == id }) return onDone("provider not found")
        // Provider configuration is independent of host-global accounts. Only the
        // account manager may remove one account and its precise credential.
        setProviders(kind, _state.value.providers(kind).filter { it.id != id })
        if (kind == ProviderKind.Llm) markLlmProviderConfigurationPending(id)
        onDone(null)
    }

    /** Adds a provider from a preset; returns the new id. */
    fun addProvider(kind: ProviderKind, presetId: String): String {
        val preset = kind.presets.firstOrNull { it.id == presetId }
            ?: catalogPreset(kind, presetId)
            ?: error("provider preset '$presetId' is not present in the engine catalog")
        val next = ProviderSettingsRepository.newProvider(
            kind,
            preset,
            _state.value.llmCatalogEntries,
        )
        setProviders(kind, _state.value.providers(kind) + next)
        if (kind == ProviderKind.Llm) markLlmProviderConfigurationPending(next.id)
        return next.id
    }

    private fun catalogPreset(kind: ProviderKind, presetId: String): ProviderPreset? {
        if (kind != ProviderKind.Llm) return null
        val profileId = if (presetId == "google") "gemini" else presetId
        val entry = _state.value.llmCatalogEntries.firstOrNull { it.profileId == profileId }
            ?: return null
        val fallback = kind.presets.firstOrNull { it.id == presetId }
        return ProviderPreset(
            id = presetId,
            name = entry.displayName,
            sub = fallback?.sub ?: entry.protocol,
            color = fallback?.color ?: Color.Gray,
            defaultUrl = entry.baseUrl,
            keyPrefix = fallback?.keyPrefix.orEmpty(),
            models = entry.modelIds,
            needsCx = fallback?.needsCx ?: false,
        )
    }

    /** A busy host may reject reconnecting; keep saved changes available to retry. */
    fun applyLlmConfiguration(reconnect: () -> Boolean): Boolean {
        if (!reconnect()) return false
        _state.update { it.copy(pendingLlmProviderChanges = emptySet()) }
        return true
    }

    fun setVisionDelegationEnabled(enabled: Boolean) {
        if (_state.value.visionDelegationEnabled == enabled) return
        providerRepo?.setVisionDelegationEnabled(enabled)
        _state.update {
            it.copy(
                visionDelegationEnabled = enabled,
                pendingLlmProviderChanges = it.pendingLlmProviderChanges + VISION_DELEGATION_PENDING_KEY,
            )
        }
    }

    private fun markLlmProviderConfigurationPending(id: String) {
        _state.update {
            it.copy(pendingLlmProviderChanges = it.pendingLlmProviderChanges + id)
        }
    }

    fun refreshAfterProviderAccountMutation() = refreshProviderStatuses()

    private fun applyAccountSnapshot(snapshot: ProviderAccountsState) {
        _state.update { current -> current.copy(accountModelAvailability = snapshot.modelAvailability, llmProviders = current.llmProviders.map { provider ->
            val accounts = snapshot.accounts.filter { it.executionBindingId == ProviderSettingsRepository.profileNameFor(provider) }
            val configured = snapshot.loaded && accounts.any { it.enabled && it.status in setOf("ready", "cooling_down", "quota_exhausted") }
            provider.copy(credentialConfigured = configured, status = when {
                snapshot.error != null -> ConnStatus.Error
                configured -> ConnStatus.Configured
                else -> ConnStatus.Idle
            })
        }) }
    }

    fun refreshProviderStatuses(onDone: (String?) -> Unit = {}) {
        val generation = accountBindingGeneration
        viewModelScope.launch {
            providerAccounts.refresh()
            if (generation != accountBindingGeneration) return@launch
            applyAccountSnapshot(providerAccounts.state.value)
            onDone(providerAccounts.state.value.error)
        }
    }

    // --- skills / mcp / dream (A8) ------------------------------------------

    fun setSkillEnabled(id: String, on: Boolean) =
        _state.update { s -> s.copy(skills = s.skills.map { if (it.id == id) it.copy(enabled = on) else it }) }

    fun updateMcp(id: String, mutate: (MCPServer) -> MCPServer) =
        _state.update { s ->
            s.copy(
                mcpServers = s.mcpServers.map { server ->
                    if (server.id != id) server else mutate(server)
                },
            )
        }

    fun setMcpStatus(id: String, status: ConnStatus) = updateMcp(id) { it.copy(status = status) }
    /** Replace the whole MCP list — used to mirror the engine's REAL listing in. */
    fun setMcpServers(servers: List<MCPServer>) = _state.update { state ->
        val existing = state.mcpServers.associateBy { it.id }
        state.copy(
            mcpServers = servers.map { incoming ->
                existing[incoming.id]?.let { prior ->
                    incoming.copy(
                        url = incoming.url.ifBlank { prior.url },
                        tools = if (incoming.tools == 0) prior.tools else incoming.tools,
                        auth = incoming.auth ?: prior.auth,
                    )
                } ?: incoming
            },
        )
    }

    fun removeMcp(id: String) =
        _state.update { s ->
            s.copy(
                mcpServers = s.mcpServers.filterNot { it.id == id },
            )
        }

    fun setDream(dream: DreamConfig) = _state.update { it.copy(dream = dream) }

    override fun onCleared() {
        providerAccounts.bindSource(null)
        super.onCleared()
    }

    companion object {
        private const val VISION_DELEGATION_PENDING_KEY = "__vision_delegation__"

        fun factory(context: Context): ViewModelProvider.Factory =
            object : ViewModelProvider.Factory {
                override fun <T : ViewModel> create(modelClass: Class<T>): T {
                    @Suppress("UNCHECKED_CAST")
                    val appContext = context.applicationContext
                    return SettingsStore(
                        providerRepo = ProviderSettingsRepository(appContext),
                        voiceRepo = AudioConfigurationRepository(appContext),
                        notifRepo = NotificationPrefsStore(appContext),
                        permissionModeRepo = PermissionModeSettingsRepository(appContext),
                        onVoiceConfigurationSaved = { configuration ->
                            com.lingxi.code.voice.audio.AndroidNativeMediaHostProvider
                                .updateAudioSettings(configuration)
                        },
                        resolveString = { id -> com.lingxi.code.theme.LocaleWrapper.getString(appContext, id) },
                    ) as T
                }
            }
    }
}

object TypeScriptLspModeOptions {
    val values: Set<String> = linkedSetOf("auto", "off", "on")
}

internal fun providerLaunchConfigurationChanged(
    before: GenericProvider,
    after: GenericProvider,
): Boolean =
    before.url.trim() != after.url.trim() ||
        before.model.trim() != after.model.trim() ||
        before.enabled != after.enabled ||
        before.connections != after.connections
