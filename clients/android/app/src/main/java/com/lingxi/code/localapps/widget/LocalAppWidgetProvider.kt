package com.lingxi.code.localapps.widget

import android.app.PendingIntent
import android.appwidget.AppWidgetManager
import android.appwidget.AppWidgetProvider
import android.content.ComponentName
import android.content.Context
import android.widget.RemoteViews
import com.lingxi.code.R

class LocalAppWidgetProvider : AppWidgetProvider() {
    override fun onUpdate(
        context: Context,
        appWidgetManager: AppWidgetManager,
        appWidgetIds: IntArray,
    ) {
        appWidgetIds.forEach { appWidgetId ->
            updateAppWidget(context, appWidgetManager, appWidgetId)
        }
    }

    override fun onDeleted(context: Context, appWidgetIds: IntArray) {
        super.onDeleted(context, appWidgetIds)
        appWidgetIds.forEach { appWidgetId ->
            LocalAppWidgetSelectionStore.deleteAppId(context, appWidgetId)
        }
    }

    override fun onRestored(context: Context, oldWidgetIds: IntArray, newWidgetIds: IntArray) {
        super.onRestored(context, oldWidgetIds, newWidgetIds)
        LocalAppWidgetSelectionStore.remap(context, oldWidgetIds, newWidgetIds)
    }

    companion object {
        fun refreshAll(context: Context) {
            val appWidgetManager = AppWidgetManager.getInstance(context)
            val component = ComponentName(context, LocalAppWidgetProvider::class.java)
            val appWidgetIds = appWidgetManager.getAppWidgetIds(component)
            if (appWidgetIds.isEmpty()) return
            appWidgetIds.forEach { appWidgetId ->
                updateAppWidget(context, appWidgetManager, appWidgetId)
            }
        }

        fun updateAppWidget(
            context: Context,
            appWidgetManager: AppWidgetManager,
            appWidgetId: Int,
        ) {
            val snapshot = LocalAppWidgetSnapshotStore.read(context)
            val selectedAppId = LocalAppWidgetSelectionStore.readAppId(context, appWidgetId)
            val app = snapshot.apps.firstOrNull { it.id == selectedAppId }
            val views = RemoteViews(context.packageName, R.layout.local_app_widget)

            if (app == null) {
                views.setTextViewText(R.id.widget_title, context.getString(R.string.local_apps_not_found))
                views.setTextViewText(R.id.widget_brief, selectedAppId.orEmpty())
                views.setTextViewText(R.id.widget_status, unavailableLabel(context))
            } else {
                views.setTextViewText(R.id.widget_title, app.name)
                views.setTextViewText(
                    R.id.widget_brief,
                    app.brief.ifBlank { context.getString(R.string.local_apps_preview_title) },
                )
                views.setTextViewText(R.id.widget_status, statusLabel(context, app))
            }

            views.setOnClickPendingIntent(
                R.id.widget_root,
                launchPendingIntent(context, appWidgetId, selectedAppId),
            )
            appWidgetManager.updateAppWidget(appWidgetId, views)
        }

        private fun launchPendingIntent(
            context: Context,
            appWidgetId: Int,
            selectedAppId: String?,
        ): PendingIntent {
            val intent = if (selectedAppId != null && LocalAppWidgetDeepLink.isValidAppId(selectedAppId)) {
                LocalAppWidgetDeepLink.previewIntent(context, selectedAppId)
            } else {
                LocalAppWidgetDeepLink.libraryIntent(context)
            }
            return PendingIntent.getActivity(
                context,
                appWidgetId,
                intent,
                PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
            )
        }

        private fun statusLabel(context: Context, app: LocalAppWidgetApp): String =
            when (app.runtimeState) {
                "running" -> context.getString(R.string.local_apps_runtime_running)
                "starting" -> context.getString(R.string.settings_cu_state_starting)
                "stopping" -> context.getString(R.string.settings_cu_state_stopping)
                "failed" -> context.getString(R.string.chat_run_status_failed)
                else -> {
                    if (app.workflow == "ready") {
                        context.getString(R.string.local_apps_runtime_stopped)
                    } else {
                        context.getString(R.string.local_apps_workflow_draft)
                    }
                }
            }

        private fun unavailableLabel(context: Context): String =
            context.getString(R.string.settings_linux_unavailable)
    }
}

internal object LocalAppWidgetSelectionStore {
    private const val PREFERENCES_NAME = "local_apps_widget_bindings_v1"

    fun readAppId(context: Context, appWidgetId: Int): String? =
        context.applicationContext
            .getSharedPreferences(PREFERENCES_NAME, Context.MODE_PRIVATE)
            .getString(appKey(appWidgetId), null)
            ?.takeIf(LocalAppWidgetDeepLink::isValidAppId)

    fun writeAppId(context: Context, appWidgetId: Int, appId: String) {
        if (!LocalAppWidgetDeepLink.isValidAppId(appId)) return
        context.applicationContext
            .getSharedPreferences(PREFERENCES_NAME, Context.MODE_PRIVATE)
            .edit()
            .putString(appKey(appWidgetId), appId)
            .apply()
    }

    fun deleteAppId(context: Context, appWidgetId: Int) {
        context.applicationContext
            .getSharedPreferences(PREFERENCES_NAME, Context.MODE_PRIVATE)
            .edit()
            .remove(appKey(appWidgetId))
            .apply()
    }

    fun remap(context: Context, oldWidgetIds: IntArray, newWidgetIds: IntArray) {
        val current = buildMap {
            oldWidgetIds.forEach { widgetId ->
                readAppId(context, widgetId)?.let { put(widgetId, it) }
            }
        }
        val remapped = remappedWidgetBindings(oldWidgetIds, newWidgetIds, current)
        oldWidgetIds.filterNot { it in remapped }.forEach { deleteAppId(context, it) }
        remapped.forEach { (widgetId, appId) ->
            writeAppId(context, widgetId, appId)
        }
    }

    private fun appKey(appWidgetId: Int): String = "widget_app_id_$appWidgetId"
}
