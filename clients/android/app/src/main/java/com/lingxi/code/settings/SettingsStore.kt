package com.lingxi.code.settings

import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import com.lingxi.code.model.ConnStatus
import com.lingxi.code.model.DreamConfig
import com.lingxi.code.model.GenericProvider
import com.lingxi.code.model.MCPServer
import com.lingxi.code.model.NotifConfig
import com.lingxi.code.model.ProviderKind
import com.lingxi.code.model.SettingsMock
import com.lingxi.code.model.Skill
import com.lingxi.code.model.VoiceConfig
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch

/**
 * Mutable settings state — the Android analog of the iOS `SettingsStore`
 * (`ObservableObject`). Holds the mock provider / skill / MCP / Dream / voice
 * data plus the simple toggles (language, notifications, privacy switches) and
 * exposes them as a single [StateFlow]. UI mutates via the typed setters; no
 * network or engine work happens here — `test connection` / `reconnect` are
 * local animations driven by the pages.
 *
 * A6 reads [SettingsUiState] for the main-list counts and drives the simple
 * pages' toggles/radios; A7/A8 build the provider / skills / MCP / Dream editors
 * on the same state. Appearance is *not* held here — it lives in the
 * DataStore-backed `AppearanceStore` so theme + accent persist and stay live.
 */
data class SettingsUiState(
    val llmProviders: List<GenericProvider> = SettingsMock.llmProviders,
    val searchProviders: List<GenericProvider> = SettingsMock.searchProviders,
    val fetchProviders: List<GenericProvider> = SettingsMock.fetchProviders,
    val voice: VoiceConfig = VoiceConfig(),
    val linuxRuntime: LinuxRuntimeUiState = LinuxRuntimeUiState(),
    val skills: List<Skill> = SettingsMock.skills,
    val mcpServers: List<MCPServer> = SettingsMock.mcpServers,
    val dream: DreamConfig = DreamConfig(),
    val language: String = "zh-CN",
    val notifs: NotifConfig = NotifConfig(),
    val smartRouting: Boolean = true,
    val streamingDefault: Boolean = true,
    val bioLock: Boolean = true,
    val telemetry: Boolean = false,
    val autoUpdate: Boolean = true,
) {
    /** Providers for a [ProviderKind] (used by A7's list page). */
    fun providers(kind: ProviderKind): List<GenericProvider> = when (kind) {
        ProviderKind.Llm -> llmProviders
        ProviderKind.Search -> searchProviders
        ProviderKind.Fetch -> fetchProviders
    }
}

class SettingsStore : ViewModel() {
    private val _state = MutableStateFlow(SettingsUiState())
    val state: StateFlow<SettingsUiState> = _state.asStateFlow()

    // --- simple toggles / radios (A6) --------------------------------------

    fun setLanguage(code: String) = _state.update { it.copy(language = code) }
    fun setBioLock(on: Boolean) = _state.update { it.copy(bioLock = on) }
    fun setTelemetry(on: Boolean) = _state.update { it.copy(telemetry = on) }
    fun setAutoUpdate(on: Boolean) = _state.update { it.copy(autoUpdate = on) }
    fun setNotifs(notifs: NotifConfig) = _state.update { it.copy(notifs = notifs) }
    fun setVoice(voice: VoiceConfig) = _state.update { it.copy(voice = voice) }
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
                summary = "Mobile Linux 操作失败",
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
    fun setSmartRouting(on: Boolean) = _state.update { it.copy(smartRouting = on) }
    fun setStreamingDefault(on: Boolean) = _state.update { it.copy(streamingDefault = on) }

    // --- provider store access by kind (A7) --------------------------------

    private fun setProviders(kind: ProviderKind, value: List<GenericProvider>) =
        _state.update {
            when (kind) {
                ProviderKind.Llm -> it.copy(llmProviders = value)
                ProviderKind.Search -> it.copy(searchProviders = value)
                ProviderKind.Fetch -> it.copy(fetchProviders = value)
            }
        }

    fun updateProvider(kind: ProviderKind, id: String, mutate: (GenericProvider) -> GenericProvider) {
        val updated = _state.value.providers(kind).map { if (it.id == id) mutate(it) else it }
        setProviders(kind, updated)
    }

    fun setDefaultProvider(kind: ProviderKind, id: String) {
        setProviders(kind, _state.value.providers(kind).map { it.copy(isDefault = it.id == id) })
    }

    fun removeProvider(kind: ProviderKind, id: String) {
        setProviders(kind, _state.value.providers(kind).filter { it.id != id })
    }

    /** Adds a provider from a preset; returns the new id. */
    fun addProvider(kind: ProviderKind, presetId: String): String {
        val next = SettingsMock.newProvider(kind, presetId)
        setProviders(kind, _state.value.providers(kind) + next)
        return next.id
    }

    /**
     * Local "test connection" animation (no network): flip to [ConnStatus.Testing],
     * then settle to [ConnStatus.Connected] after ~1.1s — the Android analog of the
     * iOS `asyncAfter`. Driven from [viewModelScope] so the page stays stateless.
     */
    fun testProviderConnection(kind: ProviderKind, id: String) {
        updateProvider(kind, id) { it.copy(status = ConnStatus.Testing) }
        viewModelScope.launch {
            delay(1100)
            updateProvider(kind, id) { it.copy(status = ConnStatus.Connected) }
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
}
