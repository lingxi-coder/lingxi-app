package com.lingxi.code.voice.recorder

import android.Manifest
import android.content.Context
import android.content.pm.PackageManager
import android.media.MediaRecorder
import android.os.Build
import android.util.Log
import androidx.core.content.ContextCompat
import java.io.File

private const val TAG = "RecorderController"

/** Why a recording operation failed; mapped onto `VoiceFfiException` by the adapter. */
sealed interface RecorderFailure {
    /** RECORD_AUDIO has not been granted (or no context is attached). */
    data object PermissionDenied : RecorderFailure

    /** `stop` was called with no active session. */
    data object NotRecording : RecorderFailure

    /** Any other native failure. */
    data class Other(val message: String) : RecorderFailure
}

/** Raised by the controller; the adapter fans it onto `VoiceFfiException`. */
class RecorderException(val failure: RecorderFailure) : Exception(
    when (failure) {
        is RecorderFailure.Other -> failure.message
        else -> failure::class.simpleName ?: "recorder error"
    },
)

/** A finished recording: the encoded bytes + their MIME type. */
data class Recording(val audioBytes: ByteArray, val mimeType: String)

/**
 * Process-global mic recorder bridged to the (Rust-driven)
 * `com.lingxi.code.bindings.AndroidVoice` callback interface. The engine drives
 * `start`/`stop`/`isRecording` through `tool-voice`; the controller owns a
 * single [MediaRecorder] session writing AAC into an `.m4a` cache file.
 *
 * Mirrors [com.lingxi.code.share.ShareController] /
 * [com.lingxi.code.vision.CameraController]: because the engine has no
 * [Context] handle, the host Activity attaches the application context in
 * `onCreate` ([attach]) and clears it in `onDestroy` ([detach]). Unlike the
 * STT hold-to-talk path (which the user triggers from the composer), this raw
 * recorder needs no UI button — it is fully engine-driven.
 *
 * RECORD_AUDIO is already declared in the manifest for STT; [start] checks the
 * runtime grant and reports [RecorderFailure.PermissionDenied] if absent.
 */
object RecorderController {

    /** Application context wired up by the host Activity; null when detached. */
    @Volatile
    private var context: Context? = null

    /** The live session, or null when idle. Guarded by `this`. */
    private var recorder: MediaRecorder? = null

    /** The cache file the live session is writing to. Guarded by `this`. */
    private var outputFile: File? = null

    /** Register the host Activity's (application) context. */
    fun attach(context: Context) {
        this.context = context.applicationContext
    }

    /** Drop the context and tear down any in-flight session. */
    fun detach() {
        synchronized(this) {
            releaseLocked(deleteFile = true)
        }
        context = null
    }

    /** Whether a recording session is currently active. */
    fun isRecording(): Boolean = synchronized(this) { recorder != null }

    /**
     * Configure and start a [MediaRecorder] session writing AAC into an `.m4a`
     * file in the app cache. Throws [RecorderException] with
     * [RecorderFailure.PermissionDenied] when RECORD_AUDIO is not granted,
     * [RecorderFailure.Other] when already recording or on a native failure.
     */
    fun start(sampleRateHz: Int) {
        val ctx = context ?: throw RecorderException(RecorderFailure.PermissionDenied)
        if (ContextCompat.checkSelfPermission(ctx, Manifest.permission.RECORD_AUDIO)
            != PackageManager.PERMISSION_GRANTED
        ) {
            throw RecorderException(RecorderFailure.PermissionDenied)
        }
        synchronized(this) {
            if (recorder != null) {
                throw RecorderException(RecorderFailure.Other("recording already in progress"))
            }
            val dir = File(ctx.cacheDir, "recordings").apply { mkdirs() }
            val file = File(dir, "rec_${System.currentTimeMillis()}.m4a")
            val mr = newMediaRecorder(ctx).apply {
                setAudioSource(MediaRecorder.AudioSource.MIC)
                setOutputFormat(MediaRecorder.OutputFormat.MPEG_4)
                setAudioEncoder(MediaRecorder.AudioEncoder.AAC)
                if (sampleRateHz > 0) setAudioSamplingRate(sampleRateHz)
                setOutputFile(file.absolutePath)
            }
            try {
                mr.prepare()
                mr.start()
            } catch (t: Throwable) {
                runCatching { mr.release() }
                file.delete()
                Log.w(TAG, "failed to start recording: ${t.message}")
                throw RecorderException(RecorderFailure.Other(t.message ?: "start failed"))
            }
            recorder = mr
            outputFile = file
        }
    }

    /**
     * Stop the active session, read the encoded bytes, and return them. Throws
     * [RecorderException] with [RecorderFailure.NotRecording] when idle,
     * [RecorderFailure.Other] on a read/stop failure.
     */
    fun stop(): Recording {
        synchronized(this) {
            val mr = recorder ?: throw RecorderException(RecorderFailure.NotRecording)
            val file = outputFile
            try {
                mr.stop()
            } catch (t: Throwable) {
                // A stop() failure (e.g. too-short clip) leaves no valid file.
                releaseLocked(deleteFile = true)
                Log.w(TAG, "failed to stop recording: ${t.message}")
                throw RecorderException(RecorderFailure.Other(t.message ?: "stop failed"))
            }
            mr.release()
            recorder = null
            outputFile = null
            val bytes = try {
                file?.readBytes() ?: ByteArray(0)
            } catch (t: Throwable) {
                Log.w(TAG, "failed to read recording: ${t.message}")
                throw RecorderException(RecorderFailure.Other(t.message ?: "read failed"))
            } finally {
                file?.delete()
            }
            return Recording(audioBytes = bytes, mimeType = "audio/m4a")
        }
    }

    /** Release any live recorder and optionally delete its file. Caller holds `this`. */
    private fun releaseLocked(deleteFile: Boolean) {
        recorder?.let { runCatching { it.stop() }; runCatching { it.release() } }
        recorder = null
        if (deleteFile) outputFile?.delete()
        outputFile = null
    }

    @Suppress("DEPRECATION")
    private fun newMediaRecorder(ctx: Context): MediaRecorder =
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) MediaRecorder(ctx) else MediaRecorder()
}
