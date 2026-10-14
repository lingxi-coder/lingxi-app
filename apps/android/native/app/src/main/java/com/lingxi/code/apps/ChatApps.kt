package com.lingxi.code.apps

import android.content.Context
import com.lingxi.code.bindings.runtime.LocalAppDto
import com.lingxi.code.bindings.runtime.LocalAppHost
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

/**
 * An app the agent made in a conversation with the `LocalApp` tool. The engine
 * keeps the library; this is its row as the pages show it.
 */
data class ChatApp(
    val id: String,
    val name: String,
    val summary: String,
    val revision: Int,
    val updatedAtMillis: Long,
)

/**
 * Chat apps share the pages' app ids with the market's, so they carry a prefix
 * no market id has: `chat:<id>`. [InstalledAppInfo] keeps their last-opened
 * time under the same key.
 */
internal const val CHAT_APP_KEY_PREFIX = "chat:"

internal fun chatAppKey(id: String): String = CHAT_APP_KEY_PREFIX + id

/** The engine id of a `chat:<id>` key, or null for a market app id. */
internal fun chatAppIdOf(key: String): String? =
    key.takeIf { it.startsWith(CHAT_APP_KEY_PREFIX) }?.removePrefix(CHAT_APP_KEY_PREFIX)?.takeIf { it.isNotEmpty() }

internal fun LocalAppDto.toChatApp() = ChatApp(
    id = id,
    name = name,
    summary = summary,
    revision = revision.toInt(),
    updatedAtMillis = updatedAtMs.toLong(),
)

/**
 * The engine's app library as state for the pages, and the runtime that runs
 * its apps. The host arrives with the engine ([attach]); until then the list is
 * empty and nothing opens.
 *
 * Every host call blocks on the engine, so each one runs on [Dispatchers.IO].
 */
class ChatApps internal constructor(context: Context, private val scope: CoroutineScope) {
    @Volatile
    private var host: LocalAppHost? = null
    private val mutable = MutableStateFlow<List<ChatApp>>(emptyList())

    /** The library, most recently updated first. */
    val state: StateFlow<List<ChatApp>> = mutable.asStateFlow()
    val runtime = ChatAppRuntime(context) { host }

    /** Use [next] from now on; every engine of this process shares one library. */
    fun attach(next: LocalAppHost?) {
        if (next == null || next === host) return
        host = next
        refresh()
    }

    fun refresh(): Job = scope.launch {
        val current = host ?: return@launch
        val apps = withContext(Dispatchers.IO) { runCatching { current.list() }.getOrNull() } ?: return@launch
        mutable.value = apps.map { it.toChatApp() }
    }

    /** True or false once the library answered; null without an engine. */
    suspend fun exists(id: String): Boolean? {
        val current = host ?: return null
        return withContext(Dispatchers.IO) { runCatching { current.get(id) != null }.getOrNull() }
    }

    /**
     * Deletes the app, then its WebView storage. The storage stays when the
     * library could not delete the app, so it never outlives its data.
     */
    fun delete(id: String): Job = scope.launch {
        val current = host ?: return@launch
        runtime.close(id)
        val deleted = withContext(Dispatchers.IO) { runCatching { current.delete(id) }.getOrDefault(false) }
        if (deleted) runtime.clearData(id)
        refresh().join()
    }
}
