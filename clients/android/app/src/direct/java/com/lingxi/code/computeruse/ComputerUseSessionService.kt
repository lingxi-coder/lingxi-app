package com.lingxi.code.computeruse

import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.content.pm.ServiceInfo
import android.os.Build
import android.os.Handler
import android.os.IBinder
import android.os.Looper
import androidx.core.app.NotificationCompat
import androidx.core.app.ServiceCompat
import com.lingxi.code.MainActivity
import com.lingxi.code.R

class ComputerUseSessionService : Service() {
    private val handler = Handler(Looper.getMainLooper())
    private var projectionCapture: MediaProjectionCapture? = null
    private val stopAtMaximum = Runnable {
        ComputerUseFeatureProvider.stop(this, "maximum-duration")
    }
    private val stopWhenIdle = object : Runnable {
        override fun run() {
            val idleFor = System.currentTimeMillis() - ComputerUseFeatureProvider.lastInteractionMs()
            if (idleFor >= IDLE_TIMEOUT_MS) {
                ComputerUseFeatureProvider.stop(this@ComputerUseSessionService, "idle-timeout")
            } else {
                handler.postDelayed(this, IDLE_TIMEOUT_MS - idleFor)
            }
        }
    }
    private val screenReceiver = object : BroadcastReceiver() {
        override fun onReceive(context: Context, intent: Intent) {
            if (intent.action == Intent.ACTION_SCREEN_OFF) {
                ComputerUseFeatureProvider.stop(context, "screen-locked")
            }
        }
    }

    override fun onCreate() {
        super.onCreate()
        createChannel()
        registerReceiver(screenReceiver, IntentFilter(Intent.ACTION_SCREEN_OFF))
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        if (intent?.action == ACTION_STOP) {
            ComputerUseFeatureProvider.stop(this, "notification-stop")
            return START_NOT_STICKY
        }
        if (!ComputerUseFeatureProvider.hasInMemoryGrants()) {
            stopSelf()
            return START_NOT_STICKY
        }
        val foregroundType = if (Build.VERSION.SDK_INT < Build.VERSION_CODES.R) {
            ServiceInfo.FOREGROUND_SERVICE_TYPE_MEDIA_PROJECTION
        } else {
            ServiceInfo.FOREGROUND_SERVICE_TYPE_SPECIAL_USE
        }
        ServiceCompat.startForeground(
            this,
            NOTIFICATION_ID,
            buildNotification(),
            foregroundType,
        )
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.R) {
            val resultCode = intent?.getIntExtra(EXTRA_PROJECTION_RESULT_CODE, Int.MIN_VALUE)
                ?: Int.MIN_VALUE
            @Suppress("DEPRECATION")
            val data = if (Build.VERSION.SDK_INT >= 33) {
                intent?.getParcelableExtra(EXTRA_PROJECTION_DATA, Intent::class.java)
            } else {
                intent?.getParcelableExtra(EXTRA_PROJECTION_DATA)
            }
            if (resultCode == Int.MIN_VALUE || data == null) {
                ComputerUseFeatureProvider.failAndStop(this, "缺少屏幕捕获授权")
                return START_NOT_STICKY
            }
            runCatching {
                MediaProjectionCapture(this).also {
                    it.start(resultCode, data)
                    projectionCapture = it
                }
            }.onFailure {
                ComputerUseFeatureProvider.failAndStop(this, it.message ?: "屏幕捕获启动失败")
                return START_NOT_STICKY
            }
        }
        ComputerUseFeatureProvider.onServiceStarted(projectionCapture)
        handler.removeCallbacks(stopAtMaximum)
        handler.removeCallbacks(stopWhenIdle)
        handler.postDelayed(stopAtMaximum, MAX_SESSION_MS)
        handler.postDelayed(stopWhenIdle, IDLE_TIMEOUT_MS)
        return START_NOT_STICKY
    }

    override fun onDestroy() {
        handler.removeCallbacks(stopAtMaximum)
        handler.removeCallbacks(stopWhenIdle)
        runCatching { unregisterReceiver(screenReceiver) }
        projectionCapture?.close()
        projectionCapture = null
        ComputerUseFeatureProvider.onServiceDestroyed()
        super.onDestroy()
    }

    override fun onBind(intent: Intent?): IBinder? = null

    private fun buildNotification() = NotificationCompat.Builder(this, CHANNEL_ID)
        .setSmallIcon(R.mipmap.ic_launcher_foreground)
        .setContentTitle("Computer Use 正在控制设备")
        .setContentText("仅允许本次会话选中的应用。点击可返回灵犀。")
        .setOngoing(true)
        .setSilent(true)
        .setCategory(NotificationCompat.CATEGORY_SERVICE)
        .setContentIntent(
            PendingIntent.getActivity(
                this,
                0,
                Intent(this, MainActivity::class.java)
                    .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_SINGLE_TOP),
                PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
            ),
        )
        .addAction(
            0,
            "立即停止",
            PendingIntent.getService(
                this,
                1,
                Intent(this, ComputerUseSessionService::class.java).setAction(ACTION_STOP),
                PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
            ),
        )
        .build()

    private fun createChannel() {
        val manager = getSystemService(NotificationManager::class.java)
        manager.createNotificationChannel(
            NotificationChannel(
                CHANNEL_ID,
                "Computer Use 控制会话",
                NotificationManager.IMPORTANCE_LOW,
            ).apply {
                description = "显示正在进行的 Android Computer Use 控制会话"
                setShowBadge(false)
            },
        )
    }

    companion object {
        internal const val ACTION_START = "com.lingxi.code.computeruse.START"
        internal const val ACTION_STOP = "com.lingxi.code.computeruse.STOP"
        internal const val EXTRA_PROJECTION_RESULT_CODE = "projection_result_code"
        internal const val EXTRA_PROJECTION_DATA = "projection_data"
        internal const val IDLE_TIMEOUT_MS = 30 * 60 * 1000L
        internal const val MAX_SESSION_MS = 2 * 60 * 60 * 1000L
        private const val CHANNEL_ID = "computer_use_session"
        private const val NOTIFICATION_ID = 0x4355
    }
}
