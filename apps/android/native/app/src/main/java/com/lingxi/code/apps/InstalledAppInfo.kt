package com.lingxi.code.apps

import com.lingxi.code.market.MarketApp
import com.lingxi.code.project.DefaultProjectAtomicWriter
import com.lingxi.code.project.ProjectAtomicWriter
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.withContext
import org.json.JSONArray
import org.json.JSONObject
import java.io.File

/**
 * What the pages remember about an installed app. The market's registry keeps
 * only ids and versions, so without this an app opened offline would have no
 * name, and nothing would know which apps were used last.
 */
data class InstalledAppInfo(
    val appId: String,
    val name: String,
    val summary: String = "",
    val category: String = "",
    val lastOpenedAtMillis: Long? = null,
)

internal fun encodeInstalledAppInfo(items: Collection<InstalledAppInfo>): String =
    JSONObject().put(
        "apps",
        JSONArray(
            items.sortedBy(InstalledAppInfo::appId).map { item ->
                JSONObject()
                    .put("appId", item.appId)
                    .put("name", item.name)
                    .put("summary", item.summary)
                    .put("category", item.category)
                    .apply { item.lastOpenedAtMillis?.let { put("lastOpenedAtMillis", it) } }
            },
        ),
    ).toString()

internal fun decodeInstalledAppInfo(text: String): Map<String, InstalledAppInfo> {
    val apps = JSONObject(text).getJSONArray("apps")
    return (0 until apps.length()).associate { index ->
        val item = apps.getJSONObject(index)
        val appId = item.getString("appId")
        appId to InstalledAppInfo(
            appId = appId,
            name = item.getString("name"),
            summary = item.optString("summary"),
            category = item.optString("category"),
            lastOpenedAtMillis = if (item.has("lastOpenedAtMillis")) item.getLong("lastOpenedAtMillis") else null,
        )
    }
}

/**
 * [InstalledAppInfo] per app id, as state for the pages and as one JSON file
 * written with the projects' atomic write-validate-rename discipline.
 */
class InstalledAppInfoStore internal constructor(
    private val file: File,
    private val scope: CoroutineScope,
    private val writer: ProjectAtomicWriter = DefaultProjectAtomicWriter(),
) {
    private val mutable = MutableStateFlow<Map<String, InstalledAppInfo>>(emptyMap())
    val state: StateFlow<Map<String, InstalledAppInfo>> = mutable.asStateFlow()
    private val writes = Mutex()

    fun load(): Job = scope.launch {
        val stored = withContext(Dispatchers.IO) {
            if (!file.isFile) emptyMap()
            else runCatching { decodeInstalledAppInfo(file.readText(Charsets.UTF_8)) }.getOrDefault(emptyMap())
        }
        // Anything recorded while the file was still loading is newer.
        mutable.update { current -> stored + current }
    }

    /** Keeps the market's name, summary and category of an app being installed or already installed. */
    fun remember(app: MarketApp) = change { current ->
        val known = current[app.id] ?: InstalledAppInfo(app.id, app.name)
        current + (app.id to known.copy(name = app.name, summary = app.summary, category = app.category))
    }

    fun markOpened(appId: String, atMillis: Long) = change { current ->
        val known = current[appId] ?: InstalledAppInfo(appId, name = appId)
        current + (appId to known.copy(lastOpenedAtMillis = atMillis))
    }

    fun forget(appId: String) = change { it - appId }

    private fun change(transform: (Map<String, InstalledAppInfo>) -> Map<String, InstalledAppInfo>) {
        mutable.update(transform)
        scope.launch {
            writes.withLock {
                // The newest state, not the one this change produced: writes queue
                // up behind each other and the last one must win.
                val text = encodeInstalledAppInfo(mutable.value.values)
                withContext(Dispatchers.IO) { writer.write(file, text) { decodeInstalledAppInfo(it) } }
            }
        }
    }
}
