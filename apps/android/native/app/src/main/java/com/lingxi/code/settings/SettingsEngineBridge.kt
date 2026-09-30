package com.lingxi.code.settings

import com.lingxi.code.bindings.*
import com.lingxi.code.conversation.ConversationSource
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.coroutineScope
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.withTimeout
import kotlinx.coroutines.launch
import org.json.JSONArray
import org.json.JSONObject

/** One connection, authoritative snapshots, no optimistic claim that a dispatch saved data. */
class SettingsEngineBridge {
    data class State(
        val connected: Boolean = false,
        val generation: Long = 0,
        val snapshot: ClientEvent.SettingsSnapshot? = null,
        val catalogs: Map<String, String> = emptyMap(),
        val notice: String? = null,
        val pending: ULong? = null,
        val models: List<ModelDetailsDto> = emptyList(),
        val operation: ClientEvent.ConfigurationOperation? = null,
        val credentials: ClientEvent.ProviderCredentialStatus? = null,
        val requiresReconnect: Boolean = false,
        val savingSettings: Boolean = false,
    )
    private val mutable = MutableStateFlow(State())
    val state = mutable.asStateFlow()
    private var source: ConversationSource? = null
    private var serial = System.currentTimeMillis().toULong() * 1000uL
    private var generation = 0L
    private var pendingPatch: Pair<String, JSONObject>? = null
    private var confirmationArmed = false

    suspend fun bind(connection: ConversationSource?) = coroutineScope {
        val bindingGeneration = ++generation
        source = connection
        pendingPatch = null
        mutable.value = State(connected = connection != null, generation = bindingGeneration)
        if (connection == null) return@coroutineScope
        val listener = launch(start = kotlinx.coroutines.CoroutineStart.UNDISPATCHED) {
            connection.clientEvents.collect { if (generation == bindingGeneration) accept(it) }
        }
        try { runCatching { refresh() }; listener.join() } finally {
            listener.cancel()
            if (generation == bindingGeneration) { source = null; mutable.value = State(generation = bindingGeneration) }
        }
    }

    fun accept(event: ClientEvent) {
        when (event) {
            is ClientEvent.SettingsSnapshot -> {
                val valid = runCatching {
                    JSONObject(event.effectiveJson); JSONObject(event.provenanceJson)
                    JSONObject(event.layersJson ?: "{}"); JSONArray(event.filesJson ?: "[]")
                }.isSuccess
                if (!valid) {
                    pendingPatch = null
                    mutable.value = mutable.value.copy(savingSettings = false, notice = "Engine returned an invalid settings snapshot; refresh to retry.")
                    return
                }
                val pending = if (confirmationArmed) pendingPatch else null
                val message = pending?.let { (layer, patch) ->
                    val raw = JSONObject(event.layersJson ?: "{}").optJSONObject(layer) ?: JSONObject()
                    if (patch.keys().asSequence().all { key ->
                        if (key == "permissions") permissionSnapshotMatches(patch.optJSONObject(key) ?: JSONObject(),raw.optJSONObject(key) ?: JSONObject())
                        else if (patch.isNull(key)) !raw.has(key) else jsonEquivalent(patch.opt(key), raw.opt(key))
                    }) "Saved to $layer. Effective values may be overridden by another layer."
                    else "Engine snapshot received; requested values were not confirmed. Review the layer and retry."
                }
                if (confirmationArmed) pendingPatch = null
                mutable.value = mutable.value.copy(snapshot = event, notice = message ?: mutable.value.notice,
                    savingSettings = if (confirmationArmed) false else mutable.value.savingSettings,
                    requiresReconnect = mutable.value.requiresReconnect || (message?.startsWith("Saved to ") == true && event.activeJson == null))
            }
            is ClientEvent.ProviderCredentialStatus -> {
                mutable.value = mutable.value.copy(credentials = event,
                    notice = event.error ?: "Credential storage status refreshed")
            }
            is ClientEvent.ModelList -> mutable.value = mutable.value.copy(models = event.details)
            is ClientEvent.Error -> {
                pendingPatch = null
                mutable.value = mutable.value.copy(pending = null, savingSettings = false, notice = event.message)
            }
            is ClientEvent.McpConfigurationSnapshot -> catalog("mcp", event.snapshotJson)
            is ClientEvent.SkillCatalog -> catalog("skills", event.catalogJson)
            is ClientEvent.SkillDocument -> catalog("document", event.documentJson)
            is ClientEvent.PluginCatalog -> catalog("plugins", event.catalogJson)
            is ClientEvent.ConfigurationOperation -> {
                if (event.domain == ConfigurationDomainDto.HOOK && event.detailsJson != null) catalog("hooks", event.detailsJson)
                if (event.operationId == mutable.value.pending) {
                    val finished = event.status == ConfigurationOperationStatusDto.SUCCEEDED || event.status == ConfigurationOperationStatusDto.FAILED
                    mutable.value = mutable.value.copy(pending = if (finished) null else event.operationId, operation = event,
                        requiresReconnect = mutable.value.requiresReconnect || (event.status == ConfigurationOperationStatusDto.SUCCEEDED && event.effect == ConfigurationEffectDto.RESTART_REQUIRED),
                        notice = "${event.status.name}: ${event.effect.name}. ${event.message.orEmpty()}")
                }
            }
            else -> Unit
        }
    }
    private fun catalog(domain: String, json: String) {
        mutable.value = mutable.value.copy(catalogs = mutable.value.catalogs + (domain to json))
    }
    suspend fun refresh() = dispatch(ClientCommand.RefreshListings(listOf(ListingKindDto.SETTINGS, ListingKindDto.MODELS)))
    suspend fun update(layer: String, patch: JSONObject) {
        val requestSource = source ?: error("Connect an engine to manage these settings")
        val requestGeneration = generation
        check(pendingPatch == null) { "Wait for the current settings save to finish" }
        val snapshot = state.value.snapshot ?: error("Waiting for engine settings")
        validateSettingsPatch(snapshot, layer, patch)
        require(!patch.has("permissions")) { "Use dedicated permission commands" }
        pendingPatch = layer to patch
        confirmationArmed = false
        mutable.value = mutable.value.copy(notice = "Waiting for engine confirmation…", savingSettings = true)
        dispatch(ClientCommand.UpdateSettings(WritableScopeDto.valueOf(layer.uppercase()), patch.toString()), requestSource, requestGeneration)
        check(requestGeneration == generation && requestSource === source) { "Engine source changed; reload settings" }
        confirmationArmed = true
        dispatch(ClientCommand.RefreshListings(listOf(ListingKindDto.SETTINGS)), requestSource, requestGeneration)
    }
    suspend fun updatePermissions(layer: String, permissions: JSONObject) {
        val requestSource = source ?: error("Connect an engine to manage these settings")
        val requestGeneration = generation
        check(pendingPatch == null) { "Wait for the current settings save to finish" }
        val snapshot = state.value.snapshot ?: error("Waiting for engine settings")
        val patch = JSONObject().put("permissions",permissions)
        validateSettingsPatch(snapshot, layer, patch)
        val before = JSONObject(snapshot.layersJson ?: "{}").getJSONObject(layer).optJSONObject("permissions") ?: JSONObject()
        val commands = permissionSettingsCommands(layer,before,permissions)
        if (commands.isEmpty()) return
        pendingPatch = layer to patch
        confirmationArmed = false
        mutable.value = mutable.value.copy(notice = "Waiting for engine confirmation…", savingSettings = true)
        commands.forEach { dispatch(it,requestSource,requestGeneration) }
        check(requestGeneration == generation && requestSource === source) { "Engine source changed; reload settings" }
        confirmationArmed = true
        dispatch(ClientCommand.RefreshListings(listOf(ListingKindDto.SETTINGS)),requestSource,requestGeneration)
    }
    internal suspend fun importProviders(layer: String, expectedProviders: JSONObject, entries: List<ProviderImportEntry>) {
        val requestSource = source ?: error("Connect an engine to import providers")
        val requestGeneration = generation
        val providers = mergeProviderImport(expectedProviders, entries)
        fun validateTarget() {
            check(requestGeneration == generation && requestSource === source) { "Engine source changed; preview the import again" }
            val snapshot = state.value.snapshot ?: error("Waiting for engine settings")
            validateSettingsPatch(snapshot,layer,JSONObject().put("providers",providers))
            val current = JSONObject(snapshot.layersJson ?: "{}").optJSONObject(layer)?.optJSONObject("providers") ?: JSONObject()
            check(jsonEquivalent(current,expectedProviders)) { "This layer changed since preview. Review the import again before saving." }
        }
        validateTarget()
        val needsStoredCredential = entries.filter { it.credential.isNullOrBlank() && it.definition.optString("apiKeyEnv").isBlank() && it.definition.optString("type") != "bedrock-claude" }
        if (needsStoredCredential.isNotEmpty()) {
            val id = ++serial
            dispatch(ClientCommand.ListProviderCredentials(id,needsStoredCredential.map { it.id },emptyList()),requestSource,requestGeneration)
            val status = withTimeout(15_000) { state.first { it.generation != requestGeneration || it.credentials?.operationId == id } }
            validateTarget()
            check(status.credentials?.error == null && needsStoredCredential.all { it.id in status.credentials?.configuredProviderIds.orEmpty() }) {
                "Provide an API key, an apiKeyEnv name, or an existing stored credential for every selected provider."
            }
        }
        for (entry in entries) {
            val credential = entry.credential ?: continue
            val id = ++serial
            dispatch(ClientCommand.SetProviderCredential(id,entry.id,ProviderCredentialSecretDto(credential)),requestSource,requestGeneration)
            val result = withTimeout(15_000) { state.first { it.generation != requestGeneration || it.credentials?.operationId == id } }
            check(result.generation == requestGeneration) { "Engine source changed; preview the import again" }
            check(result.credentials?.error == null && entry.id in result.credentials?.configuredProviderIds.orEmpty()) {
                "An imported credential could not be stored. Provider settings were not written; previously stored credentials are retained."
            }
        }
        validateTarget()
        val snapshotBeforeWrite = state.value.snapshot
        update(layer,JSONObject().put("providers",providers))
        val confirmed = withTimeout(15_000) { state.first {
            it.generation != requestGeneration || it.snapshot !== snapshotBeforeWrite || (pendingPatch == null && it.notice != "Waiting for engine confirmation…")
        } }
        check(confirmed.generation == requestGeneration) { "Engine source changed; preview the import again" }
        val stored = JSONObject(confirmed.snapshot?.layersJson ?: "{}").optJSONObject(layer)?.optJSONObject("providers") ?: JSONObject()
        check(jsonEquivalent(stored,providers)) { "The engine did not confirm the provider import. Review is retained; refresh settings before retrying." }
    }
    suspend fun refreshCredentials(providerIds: List<String>) = dispatch(ClientCommand.ListProviderCredentials(++serial,providerIds,emptyList()))
    suspend fun setCredential(providerId: String, secret: String) {
        require(providerId.isNotBlank() && secret.isNotBlank()) { "Provider and credential are required" }
        mutable.value = mutable.value.copy(notice = "Waiting for secure credential storage…")
        dispatch(ClientCommand.SetProviderCredential(++serial,providerId,ProviderCredentialSecretDto(secret)))
    }
    suspend fun deleteCredential(providerId: String) {
        mutable.value = mutable.value.copy(notice = "Waiting for secure credential storage…")
        dispatch(ClientCommand.DeleteProviderCredential(++serial,providerId))
    }
    suspend fun admin(domain: String, action: String, target: String? = null, scope: String? = null,
                      revision: String? = null, payload: String? = null) {
        val requestSource = source ?: error("Connect an engine to manage these settings")
        val requestGeneration = generation
        val id = if (action.startsWith("get_")) null else ++serial
        if (id != null) {
            check(mutable.value.pending == null) { "Wait for the current configuration operation" }
            mutable.value = mutable.value.copy(pending = id, notice = "Waiting for engine confirmation…")
        }
        val command = when (domain) {
            "mcp" -> ClientCommand.McpAdmin(McpAdminCommandDto(action,id,target,scope,revision,payload))
            "skills" -> ClientCommand.SkillAdmin(SkillAdminCommandDto(action,id,target,scope,revision,payload))
            "plugins" -> ClientCommand.PluginAdmin(PluginAdminCommandDto(action,id,target,scope,revision,payload))
            "hooks" -> ClientCommand.HookAdmin(HookAdminCommandDto(action,id,target,scope,revision,payload))
            else -> error("Unknown configuration domain")
        }
        dispatch(command, requestSource, requestGeneration)
    }
    private suspend fun dispatch(command: ClientCommand, requestSource: ConversationSource? = source, requestGeneration: Long = generation) {
        check(requestGeneration == generation && requestSource === source) { "Engine source changed; reload settings" }
        try { (requestSource ?: error("Connect an engine to manage these settings")).submitClientCommand(command) }
        catch (error: Exception) {
            if (error is CancellationException) throw error
            if (requestGeneration == generation && requestSource === source) {
                pendingPatch = null
                mutable.value = mutable.value.copy(pending = null, savingSettings = false, notice = error.message ?: "Engine request failed")
            }
            throw error
        }
    }
}

internal fun jsonEquivalent(a: Any?, b: Any?): Boolean = when {
    a is JSONObject && b is JSONObject -> a.keySet() == b.keySet() && a.keySet().all { jsonEquivalent(a.opt(it), b.opt(it)) }
    a is JSONArray && b is JSONArray -> a.length() == b.length() && (0 until a.length()).all { jsonEquivalent(a.opt(it), b.opt(it)) }
    else -> a == b
}
private fun JSONObject.keySet(): Set<String> = keys().asSequence().toSet()
internal fun validateSettingsPatch(snapshot: ClientEvent.SettingsSnapshot, layer: String, patch: JSONObject) {
    require(layer in listOf("user", "project", "local")) { "Managed settings are read-only" }
    val files = JSONArray(snapshot.filesJson ?: "[]")
    for (i in 0 until files.length()) {
        val file = files.getJSONObject(i)
        require(file.optString("layer") != layer || file.optBoolean("parsed", true)) { file.optString("parse_error", "Settings file cannot be parsed") }
    }
    require(JSONObject(snapshot.layersJson ?: "{}").has(layer)) { "Selected layer has not been loaded" }
    require(patch.keys().asSequence().none { it in snapshot.locked.orEmpty() }) { "A managed policy locks this setting" }
}
