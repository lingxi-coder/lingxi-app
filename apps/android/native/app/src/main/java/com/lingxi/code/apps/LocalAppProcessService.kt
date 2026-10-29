package com.lingxi.code.apps

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.net.Uri
import android.os.Build
import android.os.IBinder
import androidx.core.content.ContextCompat
import com.lingxi.code.MainActivity
import kotlinx.coroutines.launch
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.NonCancellable
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.delay
import kotlinx.coroutines.withContext
import kotlinx.coroutines.withTimeout
import com.lingxi.code.R

/** A user-started app lease. Process death never silently relaunches application code. */
class LocalAppProcessService : Service() {
    private val monitoring = CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate)
    @Volatile private var foregroundReady = false
    override fun onCreate() {
        super.onCreate()
        synchronized(active) { instance = this }
        monitoring.launch {
            while (true) {
                delay(1500)
                val ownersToMonitor = synchronized(active) {
                    ready.mapNotNull { id -> owners[id]?.let { owner -> id to owner } }
                }
                for ((id, owner) in ownersToMonitor) {
                    val status = runCatching { owner.request("status", id) as org.json.JSONObject }.getOrNull() ?: continue
                    val phase = status.optJSONObject("run")?.optString("phase")
                    if (phase !in setOf("preparing", "starting", "running", "stopping")) release(this@LocalAppProcessService, id)
                }
            }
        }
    }
    override fun onDestroy() {
        foregroundReady = false
        monitoring.cancel()
        val abandoned = synchronized(active) {
            active.keys.mapNotNull { id -> owners.remove(id)?.let { owner -> id to owner } }
                .also { active.clear(); ready.clear() }
        }
        synchronized(active) { if (instance === this) instance = null }
        abandoned.forEach { (id, owner) ->
            owner.scope.launch { runCatching { owner.suspend(id) } }
        }
        super.onDestroy()
    }
    override fun onBind(intent: Intent?): IBinder? = null
    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        if (intent?.action == STOP_APP) {
            val appId = intent.getStringExtra(APP_ID)
            if (appId == null || !owns(appId)) {
                if (synchronized(active) { active.isEmpty() }) stopSelf(startId)
                return START_NOT_STICKY
            }
            val owner = synchronized(active) { owners[appId] } ?: UnifiedLocalApps.get(this)
            owner.scope.launch {
                runCatching { owner.stop(appId) }
                if (owns(appId)) updateForegroundNotification()
                else if (synchronized(active) { active.isEmpty() }) stopSelf()
                else updateForegroundNotification()
            }
            return START_NOT_STICKY
        }
        val manager = getSystemService(NotificationManager::class.java)
        manager.createNotificationChannel(NotificationChannel(CHANNEL, "本地应用运行", NotificationManager.IMPORTANCE_LOW))
        updateForegroundNotification()
        return START_NOT_STICKY
    }

    private fun updateForegroundNotification() {
        val (appId, appName, count) = synchronized(active) {
            val first = active.entries.firstOrNull()
            Triple(first?.key, first?.value, active.size)
        }
        if (appId == null || appName == null) {
            foregroundReady = false
            stopForeground(STOP_FOREGROUND_REMOVE)
            stopSelf()
            return
        }
        val requestCode = appId.hashCode()
        val route = Uri.Builder().scheme("lingxi").authority("apps").appendPath(appId)
            .appendQueryParameter("review", "agent").build()
        val openIntent = Intent(this, MainActivity::class.java).setAction(Intent.ACTION_VIEW).setData(route)
        val open = PendingIntent.getActivity(this, requestCode, openIntent, PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE)
        val stopData = Uri.Builder().scheme("lingxi-localapp").authority("stop").appendPath(appId).build()
        val stopIntent = Intent(this, LocalAppProcessService::class.java).setAction(STOP_APP).setData(stopData).putExtra(APP_ID, appId)
        val stop = PendingIntent.getService(this, requestCode xor 0x51f4, stopIntent, PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE)
        val text = if (count > 1) "另有 ${count - 1} 个应用运行；此操作仅停止 $appName 及其 Agent 任务。"
        else "浏览器继续使用此应用；可停止 $appName 及其 Agent 任务。"
        val notification = Notification.Builder(this, CHANNEL).setSmallIcon(android.R.drawable.stat_notify_sync)
            .setContentTitle("$appName 正在运行").setContentText(text)
            .setContentIntent(open).setOngoing(true)
            .addAction(Notification.Action.Builder(null, "查看应用", open).build())
            .addAction(Notification.Action.Builder(null, "停止 $appName", stop).build()).build()
        if (Build.VERSION.SDK_INT >= 34) startForeground(NOTIFICATION, notification, ServiceInfo.FOREGROUND_SERVICE_TYPE_SPECIAL_USE)
        else startForeground(NOTIFICATION, notification)
        foregroundReady = true
        synchronized(active) { foregroundGeneration += 1 }
    }
    companion object {
        private const val CHANNEL = "local-app-v2-runtime"
        private const val NOTIFICATION = 7242
        private const val STOP_APP = "com.lingxi.code.apps.STOP_APP"
        private const val APP_ID = "appId"
        private var instance: LocalAppProcessService? = null
        private val active = linkedMapOf<String, String>()
        private val owners = mutableMapOf<String, UnifiedLocalApps>()
        private val ready = mutableSetOf<String>()
        private var foregroundGeneration = 0L
        internal fun owns(appId: String): Boolean = synchronized(active) { appId in active }
        internal fun notificationReady(appId: String): Boolean = synchronized(active) {
            appId in active && instance?.foregroundReady == true
        }
        fun ready(appId: String) { synchronized(active) { if (appId in active) ready.add(appId) } }
        suspend fun acquire(
            context: Context,
            appId: String,
            appName: String = appId,
            owner: UnifiedLocalApps = UnifiedLocalApps.get(context),
        ) {
            val generationBefore = synchronized(active) {
                active[appId] = appName
                owners[appId] = owner
                ready.remove(appId)
                foregroundGeneration
            }
            try {
                withContext(Dispatchers.Main.immediate) {
                    // Also starts a fresh callback if an older Service instance
                    // was already stopping. The generation wait below confirms
                    // that this app's current actions reached a notification.
                    ContextCompat.startForegroundService(context, Intent(context, LocalAppProcessService::class.java))
                }
                withTimeout(5_000) {
                    while (synchronized(active) {
                            appId !in active || instance?.foregroundReady != true || foregroundGeneration <= generationBefore
                        }) delay(20)
                }
            } catch (error: Throwable) {
                withContext(NonCancellable + Dispatchers.Main.immediate) { release(context, appId) }
                throw error
            }
        }
        fun release(context: Context, appId: String) {
            val (empty, service) = synchronized(active) {
                active.remove(appId); owners.remove(appId); ready.remove(appId)
                active.isEmpty() to instance
            }
            if (empty) context.stopService(Intent(context, LocalAppProcessService::class.java))
            else service?.updateForegroundNotification()
        }
    }
}
