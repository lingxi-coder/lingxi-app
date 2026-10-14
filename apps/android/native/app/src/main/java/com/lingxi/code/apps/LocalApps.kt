package com.lingxi.code.apps

import android.content.Context
import com.lingxi.code.market.LocalAppMarket
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import java.io.File

/**
 * The app market's anonymous API. The market's web front end proxies everything under `/api/`
 * to its backend; while that backend is not deployed every call fails with the
 * server's own error code, which the pages show as is.
 */
internal const val LOCAL_APP_MARKET_API_ROOT = "https://local-app-market.luolingfeng-lingxi.workers.dev/api/v1"

/**
 * The process's one Local App Market (its docs ask for exactly one per storage
 * root) and what the pages remember about installed apps. Lives as long as the
 * process, like [com.lingxi.code.cron.AndroidCronRepository].
 */
class LocalApps private constructor(
    val market: LocalAppMarket,
    val info: InstalledAppInfoStore,
    /** Main + SupervisorJob, as the market asks for; the pages' fire-and-forget calls run here too. */
    val scope: CoroutineScope,
) {
    companion object {
        @Volatile
        private var instance: LocalApps? = null

        fun get(context: Context): LocalApps =
            instance ?: synchronized(this) {
                instance ?: create(context.applicationContext).also { instance = it }
            }

        private fun create(context: Context): LocalApps {
            val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate)
            val market = LocalAppMarket.create(context, LOCAL_APP_MARKET_API_ROOT, scope)
            val info = InstalledAppInfoStore(File(context.filesDir, INSTALLED_APP_INFO_FILE), scope)
            // Restores the installed registry and finishes interrupted uninstalls
            // before any page can open an app.
            market.store.initialize()
            info.load()
            return LocalApps(market, info, scope)
        }

        private const val INSTALLED_APP_INFO_FILE = "local-app-info.json"
    }
}
