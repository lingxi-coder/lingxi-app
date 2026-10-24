package com.lingxi.code.apps

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.os.Build
import android.os.IBinder
import androidx.core.content.ContextCompat
import com.lingxi.code.MainActivity
import kotlinx.coroutines.launch
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.delay
import com.lingxi.code.R

/** A user-started app lease. Process death never silently relaunches application code. */
class LocalAppProcessService : Service() {
    private val monitoring = CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate)
    override fun onCreate() {
        super.onCreate()
        monitoring.launch {
            while (true) {
                delay(1500)
                for (id in synchronized(active) { ready.toList() }) {
                    val status = runCatching { UnifiedLocalApps.get(this@LocalAppProcessService).request("status", id) as org.json.JSONObject }.getOrNull() ?: continue
                    val phase = status.optJSONObject("run")?.optString("phase")
                    if (phase !in setOf("preparing", "starting", "running", "stopping")) release(this@LocalAppProcessService, id)
                }
            }
        }
    }
    override fun onDestroy() {
        monitoring.cancel()
        val abandoned = synchronized(active) { active.toList().also { active.clear(); ready.clear() } }
        UnifiedLocalApps.get(this).scope.launch { abandoned.forEach { id -> runCatching { UnifiedLocalApps.get(this@LocalAppProcessService).suspend(id) } } }
        super.onDestroy()
    }
    override fun onBind(intent: Intent?): IBinder? = null
    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        if (intent?.action == STOP) {
            UnifiedLocalApps.get(this).scope.launch {
                for (id in synchronized(active) { active.toList() }) runCatching { UnifiedLocalApps.get(this@LocalAppProcessService).stop(id) }
                stopSelf()
            }
            return START_NOT_STICKY
        }
        val manager = getSystemService(NotificationManager::class.java)
        manager.createNotificationChannel(NotificationChannel(CHANNEL, "本地应用运行", NotificationManager.IMPORTANCE_LOW))
        val open = PendingIntent.getActivity(this, 0, Intent(this, MainActivity::class.java), PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE)
        val stop = PendingIntent.getService(this, 1, Intent(this, LocalAppProcessService::class.java).setAction(STOP), PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE)
        val notification = Notification.Builder(this, CHANNEL).setSmallIcon(android.R.drawable.stat_notify_sync)
            .setContentTitle("本地应用正在运行").setContentText("后台服务仅在本机运行，可随时停止。")
            .setContentIntent(open).setOngoing(true).addAction(Notification.Action.Builder(null, "停止应用", stop).build()).build()
        if (Build.VERSION.SDK_INT >= 34) startForeground(NOTIFICATION, notification, ServiceInfo.FOREGROUND_SERVICE_TYPE_SPECIAL_USE)
        else startForeground(NOTIFICATION, notification)
        if (synchronized(active) { active.isEmpty() }) stopSelf()
        return START_NOT_STICKY
    }
    companion object {
        private const val CHANNEL = "local-app-v2-runtime"
        private const val NOTIFICATION = 7242
        private const val STOP = "com.lingxi.code.apps.STOP"
        private val active = mutableSetOf<String>()
        private val ready = mutableSetOf<String>()
        internal fun owns(appId: String): Boolean = synchronized(active) { appId in active }
        fun ready(appId: String) { synchronized(active) { if (appId in active) ready.add(appId) } }
        fun acquire(context: Context, appId: String) {
            synchronized(active) { active.add(appId); ready.remove(appId) }
            try { ContextCompat.startForegroundService(context, Intent(context, LocalAppProcessService::class.java)) }
            catch (error: Exception) { synchronized(active) { active.remove(appId) }; throw error }
        }
        fun release(context: Context, appId: String) {
            val empty = synchronized(active) { active.remove(appId); ready.remove(appId); active.isEmpty() }
            if (empty) context.stopService(Intent(context, LocalAppProcessService::class.java))
        }
    }
}
