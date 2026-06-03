package com.lingxi.code.voice.audio

import android.content.Context
import android.media.AudioAttributes
import android.media.AudioFocusRequest
import android.media.AudioManager
import android.os.Build

/**
 * Talk-session transitions an [AudioFocusController] drives in response to
 * system audio-focus events. The owning session implements this; extracting
 * the controller from the source project's `TalkSessionController` to a small
 * listener keeps the audio layer free of the agent/session framework.
 */
interface AudioFocusListener {
    /** Focus restored (`AUDIOFOCUS_GAIN`). */
    fun onResume()

    /** Transient loss (incoming call, alarm) — `AUDIOFOCUS_LOSS_TRANSIENT`. */
    fun onPause()

    /** Permanent loss (another media app took over) — `AUDIOFOCUS_LOSS`. */
    fun onStop()
}

/**
 * AudioFocus listener.
 *
 * Maps system audio-focus events to [AudioFocusListener] transitions:
 *  - `AUDIOFOCUS_LOSS_TRANSIENT` (incoming call, alarm) → [AudioFocusListener.onPause]
 *  - `AUDIOFOCUS_GAIN` (focus restored) → [AudioFocusListener.onResume]
 *  - `AUDIOFOCUS_LOSS` (another media app takes over) → [AudioFocusListener.onStop]
 *
 * Designed for a single short-lived owner: the talk-mode session creates
 * one [AudioFocusController], calls [register] on session start, and
 * [unregister] on session stop. Re-registering before unregistering is
 * idempotent.
 */
class AudioFocusController(
    private val context: Context,
    private val listener: AudioFocusListener,
) {

    private val audioManager: AudioManager? =
        context.getSystemService(Context.AUDIO_SERVICE) as? AudioManager

    @Volatile private var focusRequest: AudioFocusRequest? = null

    private val focusListener = AudioManager.OnAudioFocusChangeListener { change ->
        when (change) {
            AudioManager.AUDIOFOCUS_GAIN -> listener.onResume()
            AudioManager.AUDIOFOCUS_LOSS_TRANSIENT,
            AudioManager.AUDIOFOCUS_LOSS_TRANSIENT_CAN_DUCK -> listener.onPause()
            AudioManager.AUDIOFOCUS_LOSS -> listener.onStop()
            else -> { /* ignore */ }
        }
    }

    /** Idempotent. Returns true if focus was granted. */
    fun register(): Boolean {
        val am = audioManager ?: return false
        if (focusRequest != null) return true
        return if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            val req = AudioFocusRequest.Builder(AudioManager.AUDIOFOCUS_GAIN_TRANSIENT)
                .setAudioAttributes(
                    AudioAttributes.Builder()
                        .setUsage(AudioAttributes.USAGE_ASSISTANT)
                        .setContentType(AudioAttributes.CONTENT_TYPE_SPEECH)
                        .build()
                )
                .setOnAudioFocusChangeListener(focusListener)
                .setAcceptsDelayedFocusGain(false)
                .build()
            focusRequest = req
            am.requestAudioFocus(req) == AudioManager.AUDIOFOCUS_REQUEST_GRANTED
        } else {
            @Suppress("DEPRECATION")
            val rc = am.requestAudioFocus(
                focusListener,
                AudioManager.STREAM_VOICE_CALL,
                AudioManager.AUDIOFOCUS_GAIN_TRANSIENT,
            )
            rc == AudioManager.AUDIOFOCUS_REQUEST_GRANTED
        }
    }

    /** Idempotent. Releases focus and detaches the listener. */
    fun unregister() {
        val am = audioManager ?: return
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            focusRequest?.let { am.abandonAudioFocusRequest(it) }
        } else {
            @Suppress("DEPRECATION")
            am.abandonAudioFocus(focusListener)
        }
        focusRequest = null
    }
}
