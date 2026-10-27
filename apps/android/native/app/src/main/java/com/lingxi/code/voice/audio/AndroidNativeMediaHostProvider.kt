package com.lingxi.code.voice.audio

import android.content.Context
import com.lingxi.code.bindings.android.AndroidNativeMediaHost
import com.lingxi.code.settings.toStorageMap
import com.lingxi.code.vision.AndroidCameraAdapter
import org.json.JSONObject

/**
 * The app's one native media host. Engines, Local App agents, cron workers and
 * Remote Control all receive this same object so microphone/speaker leases,
 * saved routing settings and provider bindings have one owner.
 */
internal object AndroidNativeMediaHostProvider {
    private val lock = Any()

    @Volatile
    private var host: AndroidNativeMediaHost? = null

    fun get(context: Context): AndroidNativeMediaHost {
        host?.let { return it }
        return synchronized(lock) {
            host ?: create(context.applicationContext).also { host = it }
        }
    }

    /** Update the existing facade after a v4 settings save without rebuilding leases. */
    fun updateAudioSettings(configuration: AudioConfigurationV4) {
        val json = configuration.toStorageJson()
        synchronized(lock) {
            host?.updateAudioSettings(json)
        }
    }

    private fun create(context: Context): AndroidNativeMediaHost {
        val audio = AndroidNativeAudioServiceAdapter(context)
        val camera = AndroidCameraAdapter()
        val devicePorts = AndroidNativeMediaDeviceAdapter(context)
        val configuration = AndroidAudioServiceProvider
            .get(context)
            .configurationSnapshot()
            .configuration
            .toStorageJson()
        return AndroidNativeMediaHost(
            audio = audio,
            camera = camera,
            mediaDevices = devicePorts,
            mediaAudioConfigurationJson = configuration,
        )
    }

    private fun AudioConfigurationV4.toStorageJson(): String =
        JSONObject(toStorageMap()).toString()
}
