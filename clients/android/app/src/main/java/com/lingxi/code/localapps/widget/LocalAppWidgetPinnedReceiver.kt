package com.lingxi.code.localapps.widget

import android.appwidget.AppWidgetManager
import android.content.BroadcastReceiver
import android.content.ComponentName
import android.content.Context
import android.content.Intent
import android.os.Handler
import android.os.Looper

class LocalAppWidgetPinnedReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        if (intent.action != LocalAppWidgetPinRequester.ACTION_PIN_CONFIRMED) return
        val bind = resolvePinnedWidgetBind(
            appId = intent.getStringExtra(LocalAppWidgetPinRequester.EXTRA_APP_ID),
            widgetId = intent.getIntExtra(
                AppWidgetManager.EXTRA_APPWIDGET_ID,
                AppWidgetManager.INVALID_APPWIDGET_ID,
            ),
        ) ?: return

        val appContext = context.applicationContext
        val manager = AppWidgetManager.getInstance(appContext)
        val provider = ComponentName(appContext, LocalAppWidgetProvider::class.java)
        val ownedIds = manager.getAppWidgetIds(provider)
        val ownership = widgetOwnership(bind.widgetId, ownedIds)
        if (ownership == WidgetOwnership.Reject) return

        // Prefs, options, and RemoteViews do not wait for getAppWidgetIds.
        // The id is already allocated; skipping the refresh leaves a blank pin.
        LocalAppWidgetSelectionStore.writeAppId(appContext, bind.widgetId, bind.appId)
        runCatching {
            LocalAppWidgetPinRequester.writeAppIdToOptions(manager, bind.widgetId, bind.appId)
        }
        runCatching {
            LocalAppWidgetProvider.updateAppWidget(appContext, manager, bind.widgetId)
        }
        if (ownership == WidgetOwnership.Accept) return

        val pending = goAsync()
        Handler(Looper.getMainLooper()).postDelayed({
            try {
                retractIfStillUnowned(appContext, bind)
            } finally {
                pending.finish()
            }
        }, LocalAppWidgetPinRequester.UNOWNED_BINDING_RETRACT_MS)
    }
}

private fun retractIfStillUnowned(context: Context, bind: LocalAppWidgetPinBind) {
    val manager = AppWidgetManager.getInstance(context)
    val ownedIds = manager.getAppWidgetIds(
        ComponentName(context, LocalAppWidgetProvider::class.java),
    )
    val storedAppId = LocalAppWidgetSelectionStore.readAppId(context, bind.widgetId)
    if (!shouldRetractPinBinding(widgetOwnership(bind.widgetId, ownedIds), storedAppId, bind.appId)) {
        return
    }
    LocalAppWidgetSelectionStore.deleteAppId(context, bind.widgetId)
}
