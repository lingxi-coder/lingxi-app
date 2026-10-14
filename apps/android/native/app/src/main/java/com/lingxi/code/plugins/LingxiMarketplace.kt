package com.lingxi.code.plugins

import android.content.Context
import android.util.Log
import androidx.datastore.core.DataStore
import androidx.datastore.preferences.core.Preferences
import androidx.datastore.preferences.core.booleanPreferencesKey
import androidx.datastore.preferences.core.edit
import androidx.datastore.preferences.preferencesDataStore
import com.lingxi.code.bindings.client.ClientCommand
import com.lingxi.code.bindings.client.ClientEvent
import com.lingxi.code.bindings.client.ConfigurationDomainDto
import com.lingxi.code.bindings.client.ConfigurationOperationStatusDto
import com.lingxi.code.bindings.client.PluginAdminCommandDto
import com.lingxi.code.conversation.ConversationSource
import com.lingxi.code.settings.ProviderSettingsRepository
import java.util.concurrent.atomic.AtomicLong
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineStart
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.coroutineScope
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import kotlinx.coroutines.withTimeoutOrNull
import org.json.JSONObject

private val Context.lingxiMarketplaceDataStore: DataStore<Preferences> by preferencesDataStore(name = "lingxi_marketplace")

/**
 * LingXi's plugin marketplace: the Local App Builder plugin and whatever joins
 * it. The app only registers it, so its plugins are listed under Settings →
 * Plugins for the user to install; the user can remove it like any other.
 */
object LingxiMarketplace {
    /** The marketplace's own `name`: its plugins are `<plugin>@lingxi`. */
    const val NAME = "lingxi"

    const val GITHUB_SOURCE = "lingxi-coder/local-app-builder"

    /** The mirror for mainland China. There is none yet; until then GitHub serves every region. */
    val CHINA_SOURCE: String? = null

    /**
     * The source to add the marketplace from, or null when it should not be
     * added: it is registered already, or it was once ([seen]) and the user
     * removed it.
     */
    fun sourceToAdd(catalog: PluginCatalog, seen: Boolean, providerRegion: String?): String? = when {
        seen || catalog.marketplace(NAME) != null -> null
        providerRegion == "china_mainland" -> CHINA_SOURCE ?: GITHUB_SOURCE
        else -> GITHUB_SOURCE
    }
}

/**
 * Registers [LingxiMarketplace] through the plugin system's own
 * `marketplace_add` once an engine is up, and remembers that it is there
 * (DataStore) so a marketplace the user removed stays removed. It installs
 * nothing, so the engine needs no reconnect. A failure, such as no network,
 * is retried on the next launch.
 */
class LingxiMarketplaceSeeder(private val context: Context) {
    private val store = context.applicationContext.lingxiMarketplaceDataStore

    suspend fun ensure(source: ConversationSource) = withContext(Dispatchers.IO) {
        try {
            if (store.data.first()[SEEN] == true) return@withContext
            PluginAdminSession(source).open { session ->
                var catalog = session.catalog()
                val region = ProviderSettingsRepository(context).use { it.audioProviderRegion() }
                LingxiMarketplace.sourceToAdd(catalog, seen = false, providerRegion = region)?.let { marketplace ->
                    val revision = catalog.revisions["user"] ?: error("The plugin catalog has no user revision")
                    session.apply(JSONObject().put("action", "marketplace_add").put("source", marketplace), revision)
                    catalog = session.catalog()
                }
                if (catalog.marketplace(LingxiMarketplace.NAME) != null) store.edit { it[SEEN] = true }
            }
        } catch (error: CancellationException) {
            throw error
        } catch (error: Exception) {
            Log.w(TAG, "The LingXi plugin marketplace was not added", error)
        }
    }

    private companion object {
        const val TAG = "LingxiMarketplace"
        val SEEN = booleanPreferencesKey("marketplace_seen")
    }
}

/**
 * The seeder's own line to the engine: plugin admin commands with operation
 * ids outside the settings pages' range, and the events that answer them.
 */
private class PluginAdminSession(private val source: ConversationSource) {
    private val events = Channel<ClientEvent>(Channel.UNLIMITED)

    /** Listens before the first command goes out, so no answer is missed. */
    suspend fun <T> open(block: suspend (PluginAdminSession) -> T): T = coroutineScope {
        val listener = launch(start = CoroutineStart.UNDISPATCHED) {
            source.clientEvents.collect { event ->
                if (event is ClientEvent.PluginCatalog || event is ClientEvent.ConfigurationOperation) events.send(event)
            }
        }
        try { block(this@PluginAdminSession) } finally { listener.cancel() }
    }

    /** Asks for the catalog; any catalog that arrives after the request is current. */
    suspend fun catalog(): PluginCatalog {
        source.submitClientCommand(ClientCommand.PluginAdmin(PluginAdminCommandDto("get_catalog", null, null, null, null, null)))
        return withTimeoutOrNull(CATALOG_TIMEOUT_MS) {
            var catalog: PluginCatalog? = null
            while (catalog == null) {
                val event = events.receive()
                if (event is ClientEvent.PluginCatalog) catalog = PluginCatalog.parse(event.catalogJson)
            }
            catalog
        } ?: error("The engine did not send the plugin catalog")
    }

    /** Applies one user-scope operation, confirmed, and waits for it to finish. */
    suspend fun apply(payload: JSONObject, revision: String) {
        val id = (OPERATION_BASE + operations.incrementAndGet()).toULong()
        val body = payload.put("scope", "user").put("confirmed", true).toString()
        source.submitClientCommand(ClientCommand.PluginAdmin(PluginAdminCommandDto("apply_operation", id, null, "user", revision, body)))
        withTimeoutOrNull(APPLY_TIMEOUT_MS) {
            var finished = false
            while (!finished) {
                val event = events.receive() as? ClientEvent.ConfigurationOperation ?: continue
                if (event.domain != ConfigurationDomainDto.PLUGIN || event.operationId != id) continue
                check(event.status != ConfigurationOperationStatusDto.FAILED) { "$payload failed: ${event.message.orEmpty()}" }
                finished = event.status == ConfigurationOperationStatusDto.SUCCEEDED
            }
        } ?: error("The engine did not finish $payload")
    }

    private companion object {
        const val CATALOG_TIMEOUT_MS = 30_000L
        const val APPLY_TIMEOUT_MS = 120_000L
        /** Far above the settings pages' time-based ids, so the two never collide. */
        const val OPERATION_BASE = 0x7000_0000_0000_0000L
        val operations = AtomicLong(0)
    }
}
