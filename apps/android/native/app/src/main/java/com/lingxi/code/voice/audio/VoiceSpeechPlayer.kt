package com.lingxi.code.voice.audio

import android.content.Context
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.launch
import java.util.UUID

/** Small compatibility facade; playback itself is owned by the app-scoped audio service. */
internal class VoiceSpeechPlayer(context: Context) {
    private val appContext = context.applicationContext
    private val owner = AudioOwnerKey.ui("speech-player-${UUID.randomUUID()}")
    private val lifecycleScope = CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate)

    suspend fun speak(
        text: String,
        voice: String? = null,
        speed: Float? = null,
        foregroundUserInitiated: Boolean = false,
        flowDuplex: Boolean = false,
    ): Boolean = when (
        val result = AndroidAudioServiceProvider.perform(
            context = appContext,
            owner = owner,
            operation = DeviceAudioOperation.Speak(
                text = text,
                language = null,
                rate = speed,
                voice = voice,
                foregroundUserInitiated = foregroundUserInitiated,
                flowDuplex = flowDuplex,
            ),
        )
    ) {
        is DeviceAudioResult.PlaybackCompleted -> true
        is DeviceAudioResult.Failed -> false
        else -> false
    }

    fun stop() {
        lifecycleScope.launch {
            AndroidAudioServiceProvider.perform(appContext, owner, DeviceAudioOperation.EndOwner)
        }
    }
}
