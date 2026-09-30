package com.lingxi.code.localapps.widget

import android.content.Context
import android.content.Intent
import android.net.Uri
import com.lingxi.code.MainActivity

private val localAppIdPattern = Regex("^[a-z0-9][a-z0-9-]{0,63}$")

data class LocalAppLaunchRequest(
    val appId: String,
    val autostart: Boolean,
    val source: String,
)

object LocalAppWidgetDeepLink {
    const val EXTRA_OPEN_LOCAL_APPS = "lingxi.open_local_apps"
    private val supportedQueryNames = setOf("appId", "destination", "autostart", "source")

    fun isValidAppId(appId: String): Boolean = localAppIdPattern.matches(appId)

    fun parse(intent: Intent?): LocalAppLaunchRequest? {
        val uri = intent?.data ?: return null
        val parameters = uri.queryParameterNames.associateWith(uri::getQueryParameter)
        return parseParts(uri.scheme, uri.host, uri.path, parameters)
    }

    internal fun parseParts(
        scheme: String?,
        host: String?,
        path: String?,
        parameters: Map<String, String?>,
    ): LocalAppLaunchRequest? {
        if (!scheme.equals("lingxi", ignoreCase = true) ||
            !host.equals("open_local_app", ignoreCase = true)
        ) return null
        if (!path.isNullOrEmpty() && path != "/") return null
        if (parameters.keys.any { it !in supportedQueryNames }) return null
        if (parameters["destination"]?.trim()?.let { it != "preview" } == true) return null
        val appId = parameters["appId"]?.trim().orEmpty()
        if (!isValidAppId(appId)) return null
        val autostart = when (parameters["autostart"]?.trim()) {
            null, "1" -> true
            "0" -> false
            else -> return null
        }
        val source = parameters["source"]?.trim().takeUnless { it.isNullOrEmpty() }
            ?: "widget"
        return LocalAppLaunchRequest(appId = appId, autostart = autostart, source = source)
    }

    fun previewUri(appId: String): Uri =
        Uri.Builder()
            .scheme("lingxi")
            .authority("open_local_app")
            .appendQueryParameter("appId", appId)
            .appendQueryParameter("destination", "preview")
            .appendQueryParameter("autostart", "1")
            .appendQueryParameter("source", "widget")
            .build()

    fun previewIntent(context: Context, appId: String): Intent =
        Intent(Intent.ACTION_VIEW, previewUri(appId), context, MainActivity::class.java)
            .addCategory(Intent.CATEGORY_BROWSABLE)
            .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_CLEAR_TOP)

    fun libraryIntent(context: Context): Intent =
        Intent(context, MainActivity::class.java)
            .putExtra(EXTRA_OPEN_LOCAL_APPS, true)
            .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_CLEAR_TOP)
}
