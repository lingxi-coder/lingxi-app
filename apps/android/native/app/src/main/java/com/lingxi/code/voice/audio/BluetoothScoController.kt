package com.lingxi.code.voice.audio

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.media.AudioManager

/**
 * Bluetooth SCO toggle.
 *
 * Routes mic capture + speaker playback through a connected Bluetooth
 * headset's SCO (Synchronous Connection-Oriented) channel during the
 * talk session. SCO is the legacy voice-call profile every Bluetooth
 * headset supports — LE Audio (BLE) provides better fidelity but isn't
 * universally available pre-Android-15, so SCO stays the MVP path.
 *
 * Usage:
 *   val sco = BluetoothScoController(context)
 *   sco.start()   // before opening the mic / speaker
 *   ...session work...
 *   sco.stop()    // before releasing audio
 *
 * No-op when no Bluetooth headset is connected or when the system
 * AudioManager refuses the request (silent failure — talk mode still
 * works through the device's built-in mic/speaker).
 */
class BluetoothScoController(private val context: Context) {

    private val audioManager: AudioManager? =
        context.getSystemService(Context.AUDIO_SERVICE) as? AudioManager

    @Volatile private var started: Boolean = false
    @Volatile private var receiver: BroadcastReceiver? = null

    /**
     * Request SCO routing. Returns true when SCO is supported and the
     * system accepted the request; false when no headset, no SCO support,
     * or AudioManager is missing.
     *
     * Note: SCO connection is asynchronous — the system may take ~1-2s
     * to actually route audio after this call. Speaker.play / mic open
     * should follow immediately; the SCO channel will pick up once
     * connected.
     */
    fun start(): Boolean {
        val am = audioManager ?: return false
        if (!am.isBluetoothScoAvailableOffCall) return false
        if (started) return true
        return try {
            am.mode = AudioManager.MODE_IN_COMMUNICATION
            @Suppress("DEPRECATION")
            am.startBluetoothSco()
            @Suppress("DEPRECATION")
            am.isBluetoothScoOn = true
            // Track connection state so we can clean up if SCO fails to
            // establish (some headsets drop after a few seconds).
            val filter = IntentFilter(AudioManager.ACTION_SCO_AUDIO_STATE_UPDATED)
            val br = object : BroadcastReceiver() {
                override fun onReceive(ctx: Context?, intent: Intent?) {
                    val state = intent?.getIntExtra(
                        AudioManager.EXTRA_SCO_AUDIO_STATE,
                        AudioManager.SCO_AUDIO_STATE_ERROR,
                    ) ?: AudioManager.SCO_AUDIO_STATE_ERROR
                    if (state == AudioManager.SCO_AUDIO_STATE_DISCONNECTED ||
                        state == AudioManager.SCO_AUDIO_STATE_ERROR) {
                        // Headset dropped — leave SCO state as-is; the next
                        // start() call (if any) re-requests.
                    }
                }
            }
            context.registerReceiver(br, filter)
            receiver = br
            started = true
            true
        } catch (_: Throwable) {
            // SecurityException (older APIs), IllegalStateException — silent
            // degradation.
            false
        }
    }

    /** Idempotent. Restores normal audio mode and detaches the listener. */
    fun stop() {
        if (!started) return
        val am = audioManager
        runCatching {
            @Suppress("DEPRECATION")
            am?.stopBluetoothSco()
            @Suppress("DEPRECATION")
            am?.isBluetoothScoOn = false
            am?.mode = AudioManager.MODE_NORMAL
        }
        runCatching { receiver?.let { context.unregisterReceiver(it) } }
        receiver = null
        started = false
    }
}
