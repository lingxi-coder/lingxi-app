package com.lingxi.code.settings

import androidx.lifecycle.ViewModel
import androidx.lifecycle.ViewModelProvider
import androidx.lifecycle.viewModelScope
import com.lingxi.code.R
import com.lingxi.code.model.ConnStatus
import com.lingxi.code.model.DreamConfig
import com.lingxi.code.model.GenericProvider
import com.lingxi.code.model.MCPServer
import com.lingxi.code.model.NotifConfig
import com.lingxi.code.model.ProviderKind
import com.lingxi.code.model.SettingsMock
import com.lingxi.code.model.Skill
import com.lingxi.code.model.VoiceConfig
import android.content.Context
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch

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
    val searchProviders: List<GenericProvider> = emptyList(),
    val fetchProviders: List<GenericProvider> = emptyList(),
    val voice: VoiceConfig = VoiceConfig(),
    val linuxRuntime: LinuxRuntimeUiState = LinuxRuntimeUiState(),
    val skills: List<Skill> = SettingsMock.bundledSkills(),
    val mcpServers: List<MCPServer> = SettingsMock.mcpServers(),
    val dream: DreamConfig = DreamConfig(),
    val language: String = "zh-CN",
    val notifs: NotifConfig = NotifConfig(),
    val pendingLlmProviderChanges: Set<String> = emptySet(),
    val bioLock: Boolean = true,
    val telemetry: Boolean = false,
    val autoUpdate: Boolean = true,
) {
    /**
     * True until an enabled LLM has both a selected model and a credential that
     * secure storage has confirmed. A persisted provider row only describes a
     * draft configuration; it must never make chat present a usable model when
     * its API key is absent.
     */
    val needsLlmSetup: Boolean
        get() = llmProviders.none { provider ->
            provider.enabled &&
                provider.model.isNotBlank() &&
                provider.credentialConfigured
        }

    /** Providers for a [ProviderKind] (used by A7's list page). */
    fun providers(kind: ProviderKind): List<GenericProvider> = when (kind) {
        ProviderKind.Llm -> llmProviders
        ProviderKind.Search -> searchProviders
        ProviderKind.Fetch -> fetchProviders
    }
}

class SettingsStore(
    private val providerRepo: ProviderSettingsRepository? = null,
    private val voiceRepo: VoiceSettingsRepository? = null,
    /**
     * Resolves a string resource id to its localized text. The production
     * factory wires the application context's `getString`; unit tests keep the
     * empty default (they never assert on these user-facing messages).
     */
    private val resolveString: (Int) -> String = { _ -> "" },
) : ViewModel() {
    /**
     * Adapts [resolveString] (empty-string in tests, real `Context.getString`
     * in production — see the factory below) to [SettingsMock.bundledSkills]/
     * [SettingsMock.mcpServers]'s (id, fallback) shape: falls back to the
     * literal zh-Hans copy whenever [resolveString] has nothing (i.e. every
     * JVM test that constructs [SettingsStore] with no [Context] at all).
     */
    private fun resolveWithFallback(id: Int, fallback: String): String =
        resolveString(id).ifEmpty { fallback }

    private val _state = MutableStateFlow(
        providerRepo?.loadProviderState()?.let { (llm, search, fetch) ->
            SettingsUiState(
                llmProviders = llm,
                searchProviders = search,
                fetchProviders = fetch,
                voice = voiceRepo?.load() ?: VoiceConfig(),
                skills = SettingsMock.bundledSkills(::resolveWithFallback),
                mcpServers = SettingsMock.mcpServers(::resolveWithFallback),
                dream = DreamConfig(
                    lastRun = resolveWithFallback(
                        R.string.settings_dream_last_run_seed,
                        "今早 03:24 · 整理 7 条记忆 / 草拟今日计划",
                    ),
                ),
            )
        } ?: SettingsUiState(
            voice = voiceRepo?.load() ?: VoiceConfig(),
            skills = SettingsMock.bundledSkills(::resolveWithFallback),
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

    init {
        if (providerRepo != null) {
            viewModelScope.launch {
                val result = providerRepo.refreshLlmStatuses(_state.value.llmProviders)
                _state.update { it.copy(llmProviders = mergeProviderStatuses(it.llmProviders, result.providers)) }
                providerRepo.persistProviders(ProviderKind.Llm, _state.value.llmProviders)
            }
        }
    }

    // --- simple toggles / radios (A6) --------------------------------------

    fun setLanguage(code: String) = _state.update { it.copy(language = code) }
    fun setBioLock(on: Boolean) = _state.update { it.copy(bioLock = on) }
    fun setTelemetry(on: Boolean) = _state.update { it.copy(telemetry = on) }
    fun setAutoUpdate(on: Boolean) = _state.update { it.copy(autoUpdate = on) }
    fun setNotifs(notifs: NotifConfig) = _state.update { it.copy(notifs = notifs) }
    fun setVoice(voice: VoiceConfig) {
        voiceRepo?.save(voice)
        _state.update { it.copy(voice = voice) }
    }
    @Synchronized
    fun setLinuxRuntimeMode(mode: LinuxRuntimeMode) {
        val current = _state.value
        if (current.linuxRuntime.selectedMode == mode) return
        _state.value = current.copy(
            linuxRuntime = current.linuxRuntime.copy(
                selectedMode = mode,
                busyAction = null,
            ),
        )
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
        val updated = _state.value.providers(kind).map { if (it.id == id) mutate(it) else it }
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
        val provider = _state.value.providers(kind).firstOrNull { it.id == id }
            ?: return onDone("provider not found")
        val repo = providerRepo
        if (kind == ProviderKind.Llm && repo == null && provider.credentialConfigured) {
            return onDone("provider repository unavailable; saved credential was not deleted")
        }
        viewModelScope.launch {
            val error = removeProviderCredentialFirst(
                kind = kind,
                provider = provider,
                deleteCredential = { target ->
                    repo?.deleteCredential(target) ?: ProviderCredentialSnapshot(
                        configuredProviderIds = emptySet(),
                        unavailableProviderIds = emptySet(),
                        storageEncrypted = false,
                    )
                },
                removePersistedProvider = {
                    setProviders(kind, _state.value.providers(kind).filter { it.id != id })
                    if (kind == ProviderKind.Llm) markLlmProviderConfigurationPending(id)
                },
            )
            onDone(error)
        }
    }

    /** Adds a provider from a preset; returns the new id. */
    fun addProvider(kind: ProviderKind, presetId: String): String {
        val preset = kind.presets.first { it.id == presetId }
        val next = ProviderSettingsRepository.newProvider(kind, preset)
        setProviders(kind, _state.value.providers(kind) + next)
        if (kind == ProviderKind.Llm) markLlmProviderConfigurationPending(next.id)
        return next.id
    }

    fun markLlmConfigurationApplied() {
        _state.update { it.copy(pendingLlmProviderChanges = emptySet()) }
    }

    private fun markLlmProviderConfigurationPending(id: String) {
        _state.update {
            it.copy(pendingLlmProviderChanges = it.pendingLlmProviderChanges + id)
        }
    }

    fun refreshProviderStatuses(onDone: (String?) -> Unit = {}) {
        val repo = providerRepo ?: return onDone("provider repository unavailable")
        _state.update { current ->
            current.copy(
                llmProviders = current.llmProviders.map { provider ->
                    if (ProviderSettingsRepository.engineCredentialIdFor(provider) != null) {
                        provider.copy(status = ConnStatus.Testing)
                    } else {
                        provider
                    }
                },
            )
        }
        viewModelScope.launch {
            val result = repo.refreshLlmStatuses(_state.value.llmProviders)
            _state.update {
                it.copy(llmProviders = mergeProviderStatuses(it.llmProviders, result.providers))
            }
            repo.persistProviders(ProviderKind.Llm, _state.value.llmProviders)
            onDone(result.error)
        }
    }

    fun testProviderConnection(
        kind: ProviderKind,
        id: String,
        credentialOverride: String?,
        onDone: (ProviderConnectionTestResult) -> Unit,
    ) {
        val repo = providerRepo
            ?: return onDone(
                ProviderConnectionTestResult(
                    connected = false,
                    reachable = false,
                    authenticated = false,
                    modelAvailable = false,
                    httpStatus = null,
                    latencyMs = 0,
                    message = resolveString(R.string.settings_provider_conn_service_unavailable),
                    usedStoredCredential = credentialOverride.isNullOrBlank(),
                ),
            )
        val provider = _state.value.providers(kind).firstOrNull { it.id == id }
            ?: return onDone(
                ProviderConnectionTestResult(
                    connected = false,
                    reachable = false,
                    authenticated = false,
                    modelAvailable = false,
                    httpStatus = null,
                    latencyMs = 0,
                    message = resolveString(R.string.settings_provider_not_found),
                    usedStoredCredential = credentialOverride.isNullOrBlank(),
                ),
            )
        if (kind != ProviderKind.Llm) {
            return onDone(
                ProviderConnectionTestResult(
                    connected = false,
                    reachable = false,
                    authenticated = false,
                    modelAvailable = false,
                    httpStatus = null,
                    latencyMs = 0,
                    message = resolveString(R.string.settings_provider_test_llm_only),
                    usedStoredCredential = credentialOverride.isNullOrBlank(),
                ),
            )
        }
        _state.update { current ->
            current.copy(
                llmProviders = current.llmProviders.map {
                    if (it.id == id) it.copy(status = ConnStatus.Testing) else it
                },
            )
        }
        viewModelScope.launch {
            val result = repo.testConnection(provider, credentialOverride)
            val stillSameConfiguration = _state.value.llmProviders
                .firstOrNull { it.id == id }
                ?.let {
                    it.url.trim() == provider.url.trim() &&
                        it.model.trim() == provider.model.trim()
                } == true
            if (stillSameConfiguration) {
                _state.update { current ->
                    current.copy(
                        llmProviders = current.llmProviders.map {
                            if (it.id == id) {
                                it.copy(status = if (result.connected) ConnStatus.Connected else ConnStatus.Error)
                            } else {
                                it
                            }
                        },
                    )
                }
                if (result.usedStoredCredential) {
                    repo.persistProviders(ProviderKind.Llm, _state.value.llmProviders)
                }
                onDone(result)
            } else {
                markProviderConnectionUnverified(kind, id)
                onDone(
                    result.copy(
                        connected = false,
                        message = resolveString(R.string.settings_provider_config_changed_retest),
                    ),
                )
            }
        }
    }

    fun markProviderConnectionUnverified(kind: ProviderKind, id: String) {
        val providers = _state.value.providers(kind).map { provider ->
            if (provider.id == id && provider.status in setOf(ConnStatus.Connected, ConnStatus.Error, ConnStatus.Testing)) {
                provider.copy(
                    status = if (provider.credentialConfigured) ConnStatus.Configured else ConnStatus.Idle,
                )
            } else {
                provider
            }
        }
        setProviders(kind, providers)
    }

    fun saveProviderCredential(
        kind: ProviderKind,
        id: String,
        secret: String,
        onDone: (String?) -> Unit = {},
    ) {
        val repo = providerRepo ?: return onDone("provider repository unavailable")
        val provider = _state.value.providers(kind).firstOrNull { it.id == id }
            ?: return onDone("provider not found")
        viewModelScope.launch {
            val snapshot = repo.setCredential(provider, secret)
            if (snapshot.error != null) {
                onDone(snapshot.error)
                return@launch
            }
            val configured = ProviderSettingsRepository.engineCredentialIdFor(provider)
                ?.let(snapshot.configuredProviderIds::contains) == true
            updateProvider(kind, id) {
                it.copy(
                    credentialConfigured = configured,
                    status = ProviderSettingsRepository.statusFor(
                        it,
                        configured,
                        snapshot.storageEncrypted,
                        unavailable = false,
                    ),
                )
            }
            onDone(null)
        }
    }

    fun clearProviderCredential(
        kind: ProviderKind,
        id: String,
        onDone: (String?) -> Unit = {},
    ) {
        val repo = providerRepo ?: return onDone("provider repository unavailable")
        val provider = _state.value.providers(kind).firstOrNull { it.id == id }
            ?: return onDone("provider not found")
        viewModelScope.launch {
            val snapshot = repo.deleteCredential(provider)
            if (snapshot.error != null) {
                onDone(snapshot.error)
                return@launch
            }
            updateProvider(kind, id) {
                it.copy(
                    credentialConfigured = false,
                    status = ProviderSettingsRepository.statusFor(
                        it,
                        credentialConfigured = false,
                        encrypted = snapshot.storageEncrypted,
                        unavailable = false,
                    ),
                )
            }
            onDone(null)
        }
    }

    // --- skills / mcp / dream (A8) ------------------------------------------

    fun setSkillEnabled(id: String, on: Boolean) =
        _state.update { s -> s.copy(skills = s.skills.map { if (it.id == id) it.copy(enabled = on) else it }) }

    fun updateMcp(id: String, mutate: (MCPServer) -> MCPServer) =
        _state.update { s -> s.copy(mcpServers = s.mcpServers.map { if (it.id == id) mutate(it) else it }) }

    fun setMcpStatus(id: String, status: ConnStatus) = updateMcp(id) { it.copy(status = status) }
    /** Replace the whole MCP list — used to mirror the engine's REAL listing in. */
    fun setMcpServers(servers: List<MCPServer>) = _state.update { it.copy(mcpServers = servers) }

    fun removeMcp(id: String) =
        _state.update { s -> s.copy(mcpServers = s.mcpServers.filter { it.id != id }) }

    fun setDream(dream: DreamConfig) = _state.update { it.copy(dream = dream) }

    override fun onCleared() {
        providerRepo?.close()
        super.onCleared()
    }

    companion object {
        fun factory(context: Context): ViewModelProvider.Factory =
            object : ViewModelProvider.Factory {
                override fun <T : ViewModel> create(modelClass: Class<T>): T {
                    @Suppress("UNCHECKED_CAST")
                    val appContext = context.applicationContext
                    return SettingsStore(
                        providerRepo = ProviderSettingsRepository(appContext),
                        voiceRepo = VoiceSettingsRepository(appContext),
                        resolveString = appContext::getString,
                    ) as T
                }
            }
    }
}

internal fun providerLaunchConfigurationChanged(
    before: GenericProvider,
    after: GenericProvider,
): Boolean =
    before.url.trim() != after.url.trim() ||
        before.model.trim() != after.model.trim() ||
        before.enabled != after.enabled

internal fun mergeProviderStatuses(
    current: List<GenericProvider>,
    refreshed: List<GenericProvider>,
): List<GenericProvider> {
    val refreshedById = refreshed.associateBy(GenericProvider::id)
    return current.map { provider ->
        refreshedById[provider.id]?.let { status ->
            provider.copy(
                status = status.status,
                credentialConfigured = status.credentialConfigured,
            )
        } ?: provider
    }
}

/**
 * Removes the secure credential before the persisted provider row. A credential
 * deletion error aborts the operation so the UI never claims a provider was
 * removed while its secret remains in engine storage.
 */
internal suspend fun removeProviderCredentialFirst(
    kind: ProviderKind,
    provider: GenericProvider,
    deleteCredential: suspend (GenericProvider) -> ProviderCredentialSnapshot,
    removePersistedProvider: () -> Unit,
): String? {
    if (kind == ProviderKind.Llm && ProviderSettingsRepository.engineCredentialIdFor(provider) != null) {
        val snapshot = deleteCredential(provider)
        if (snapshot.error != null) return snapshot.error
    }
    removePersistedProvider()
    return null
}
