package com.lingxi.code.apps

import android.content.ActivityNotFoundException
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.net.Uri
import android.os.Build

/** Opens a Local App through Android's installed, system-selected browser handlers. */
internal object LocalAppBrowser {
    fun intent(url: String): Intent = Intent(Intent.ACTION_VIEW, Uri.parse(url))
        .addCategory(Intent.CATEGORY_BROWSABLE)
        .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)

    internal fun chooserIntent(url: String): Intent = Intent.createChooser(intent(url), null)
        .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)

    fun isAvailable(context: Context, url: String): Boolean = handlers(context, intent(url)).isNotEmpty()

    internal fun installedHandlerPackages(context: Context, url: String): Set<String> =
        handlers(context, intent(url)).mapNotNull { it.activityInfo?.packageName }.toSet()

    fun open(context: Context, url: String, chooseHandler: Boolean = false) {
        val request = intent(url)
        check(handlers(context, request).isNotEmpty()) {
            "No installed browser can open this application page. Install or enable a browser, then try again."
        }
        try {
            // Keep the intent implicit so Android honors the user's default browser
            // and shows its resolver when the user has not chosen one.
            context.startActivity(if (chooseHandler) chooserIntent(url) else request)
        } catch (_: ActivityNotFoundException) {
            throw IllegalStateException("No installed browser can open this application page. Install or enable a browser, then try again.")
        }
    }

    private fun handlers(context: Context, request: Intent) = if (Build.VERSION.SDK_INT >= 33) {
        context.packageManager.queryIntentActivities(
            request,
            PackageManager.ResolveInfoFlags.of(PackageManager.MATCH_DEFAULT_ONLY.toLong()),
        )
    } else {
        @Suppress("DEPRECATION")
        context.packageManager.queryIntentActivities(request, PackageManager.MATCH_DEFAULT_ONLY)
    }
}
