package com.lingxi.code.localapps.widget

import android.content.Context
import android.os.Handler
import android.os.Looper
import com.lingxi.code.localapps.LocalAppItem
import com.lingxi.code.localapps.LocalAppRuntimeState
import com.lingxi.code.localapps.LocalAppWorkflow
import org.json.JSONArray
import org.json.JSONObject

internal data class LocalAppWidgetSnapshot(
    val version: Int = VERSION,
    val apps: List<LocalAppWidgetApp> = emptyList(),
) {
    companion object {
        const val VERSION = 1
    }
}

internal data class LocalAppWidgetApp(
    val id: String,
    val name: String,
    val brief: String,
    val workflow: String,
    val runtimeState: String,
    val updatedAtMs: Long,
)

internal object LocalAppWidgetSnapshotStore {
    const val PREFERENCES_NAME = "local_apps_widget_snapshot_v1"
    const val SNAPSHOT_KEY = "snapshot_json"

    fun read(context: Context): LocalAppWidgetSnapshot {
        val raw = context.applicationContext
            .getSharedPreferences(PREFERENCES_NAME, Context.MODE_PRIVATE)
            .getString(SNAPSHOT_KEY, null)
            ?: return LocalAppWidgetSnapshot()
        return runCatching { decode(raw) }.getOrDefault(LocalAppWidgetSnapshot())
    }

    fun write(context: Context, snapshot: LocalAppWidgetSnapshot) {
        context.applicationContext
            .getSharedPreferences(PREFERENCES_NAME, Context.MODE_PRIVATE)
            .edit()
            .putString(SNAPSHOT_KEY, encode(snapshot))
            .apply()
    }

    private fun encode(snapshot: LocalAppWidgetSnapshot): String =
        JSONObject().apply {
            put("version", snapshot.version)
            put(
                "apps",
                JSONArray().apply {
                    snapshot.apps.forEach { app ->
                        put(
                            JSONObject().apply {
                                put("id", app.id)
                                put("name", app.name)
                                put("brief", app.brief)
                                put("workflow", app.workflow)
                                put("runtimeState", app.runtimeState)
                                put("updatedAtMs", app.updatedAtMs)
                            },
                        )
                    }
                },
            )
        }.toString()

    internal fun decode(raw: String): LocalAppWidgetSnapshot {
        val json = JSONObject(raw)
        if (json.optInt("version") != LocalAppWidgetSnapshot.VERSION) {
            return LocalAppWidgetSnapshot()
        }
        val seenIds = hashSetOf<String>()
        val apps = buildList {
            val array = json.optJSONArray("apps") ?: JSONArray()
            for (index in 0 until array.length()) {
                val item = array.optJSONObject(index) ?: continue
                val id = item.optString("id").trim()
                if (!LocalAppWidgetDeepLink.isValidAppId(id) || !seenIds.add(id)) continue
                add(
                    LocalAppWidgetApp(
                        id = id,
                        name = item.optString("name").trim().ifBlank { id },
                        brief = item.optString("brief").trim(),
                        workflow = item.optString("workflow").trim(),
                        runtimeState = item.optString("runtimeState").trim(),
                        updatedAtMs = item.optLong("updatedAtMs"),
                    ),
                )
            }
        }
        return LocalAppWidgetSnapshot(apps = apps)
    }
}

interface LocalAppWidgetSnapshotSync {
    fun publish(apps: List<LocalAppItem>)
}

object NoopLocalAppWidgetSnapshotSync : LocalAppWidgetSnapshotSync {
    override fun publish(apps: List<LocalAppItem>) = Unit
}

class AndroidLocalAppWidgetSnapshotSync private constructor(
    context: Context,
) : LocalAppWidgetSnapshotSync {
    private val appContext = context.applicationContext
    private val handler = Handler(Looper.getMainLooper())
    private var pendingSnapshot: LocalAppWidgetSnapshot? = null
    private val publishRunnable = Runnable {
        val snapshot = pendingSnapshot ?: return@Runnable
        pendingSnapshot = null
        LocalAppWidgetSnapshotStore.write(appContext, snapshot)
        LocalAppWidgetProvider.refreshAll(appContext)
    }

    override fun publish(apps: List<LocalAppItem>) {
        val snapshot = LocalAppWidgetSnapshot(
            apps = apps.map { app ->
                LocalAppWidgetApp(
                    id = app.id,
                    name = app.name,
                    brief = app.brief,
                    workflow = when (app.workflow) {
                        LocalAppWorkflow.Draft -> "draft"
                        LocalAppWorkflow.PublishedUnverified -> "published_unverified"
                        LocalAppWorkflow.PublishedVerified -> "published_verified"
                    },
                    runtimeState = when (app.runtime.state) {
                        LocalAppRuntimeState.Stopped -> "stopped"
                        LocalAppRuntimeState.Starting -> "starting"
                        LocalAppRuntimeState.Running -> "running"
                        LocalAppRuntimeState.Stopping -> "stopping"
                        LocalAppRuntimeState.Failed -> "failed"
                    },
                    updatedAtMs = app.updatedAtMs,
                )
            },
        )
        handler.post {
            pendingSnapshot = snapshot
            handler.removeCallbacks(publishRunnable)
            handler.postDelayed(publishRunnable, 250L)
        }
    }

    companion object {
        @Volatile
        private var instance: AndroidLocalAppWidgetSnapshotSync? = null

        fun get(context: Context): AndroidLocalAppWidgetSnapshotSync =
            instance ?: synchronized(this) {
                instance ?: AndroidLocalAppWidgetSnapshotSync(context).also { instance = it }
            }
    }
}
