package com.lingxi.code.localapps.widget

import android.app.PendingIntent
import android.appwidget.AppWidgetManager
import android.content.ComponentName
import android.content.Context
import android.content.Intent
import android.net.Uri
import android.os.Bundle
import java.util.concurrent.atomic.AtomicInteger

object LocalAppWidgetPinRequester {
    const val ACTION_PIN_CONFIRMED = "com.lingxi.code.localapps.widget.PIN_CONFIRMED"
    const val EXTRA_APP_ID = "com.lingxi.code.localapps.widget.APP_ID"

    // The launcher fills EXTRA_APPWIDGET_ID via PendingIntent.send(fillIn).
    // FLAG_IMMUTABLE would drop that extra and leave the pinned widget unbound.
    internal const val PIN_CALLBACK_FLAGS =
        PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_MUTABLE

    // Configure is often launched before the pin callback. Re-read this
    // widget's options for a short window so in-app pin can auto-bind
    // without trusting exported Intent extras.
    internal const val OPTIONS_RETRY_ATTEMPTS = 8
    internal const val OPTIONS_RETRY_INTERVAL_MS = 150L

    // Longer than configure's Wait window so a still-registering pin is not
    // retracted before getAppWidgetIds catches up.
    internal const val UNOWNED_BINDING_RETRACT_MS =
        OPTIONS_RETRY_ATTEMPTS * OPTIONS_RETRY_INTERVAL_MS + 500L

    private val nextRequestCode = AtomicInteger(1)
    private val requestCodes = java.util.concurrent.ConcurrentHashMap<String, Int>()

    // Launcher preview extras. Android does not copy these into
    // getAppWidgetOptions(); the non-exported pin receiver writes the
    // trusted copy onto this widget id after the launcher fill-in.
    fun extrasForApp(appId: String): Bundle =
        Bundle().apply {
            appId.trim().takeIf(LocalAppWidgetDeepLink::isValidAppId)?.let {
                putString(EXTRA_APP_ID, it)
            }
        }

    fun appIdFromOptions(options: Bundle?): String? =
        options?.getString(EXTRA_APP_ID)
            ?.trim()
            ?.takeIf(LocalAppWidgetDeepLink::isValidAppId)

    fun writeAppIdToOptions(
        manager: AppWidgetManager,
        widgetId: Int,
        appId: String,
    ) {
        if (widgetId == AppWidgetManager.INVALID_APPWIDGET_ID) return
        if (!LocalAppWidgetDeepLink.isValidAppId(appId.trim())) return
        manager.updateAppWidgetOptions(widgetId, extrasForApp(appId))
    }

    internal fun requestCodeFor(appId: String): Int =
        requestCodes.getOrPut(appId) { nextRequestCode.getAndIncrement() }

    fun request(context: Context, appId: String): Boolean {
        if (!LocalAppWidgetDeepLink.isValidAppId(appId)) return false
        val manager = AppWidgetManager.getInstance(context)
        if (!manager.isRequestPinAppWidgetSupported) return false

        val callbackIntent = Intent(context, LocalAppWidgetPinnedReceiver::class.java)
            .setAction(ACTION_PIN_CONFIRMED)
            .setData(Uri.parse("lingxi-widget://pin/$appId"))
            .putExtra(EXTRA_APP_ID, appId)
        val callback = PendingIntent.getBroadcast(
            context,
            requestCodeFor(appId),
            callbackIntent,
            PIN_CALLBACK_FLAGS,
        )
        return runCatching {
            manager.requestPinAppWidget(
                ComponentName(context, LocalAppWidgetProvider::class.java),
                extrasForApp(appId),
                callback,
            )
        }.getOrDefault(false)
    }
}
