package com.lingxi.code.settings

import com.lingxi.code.voice.audio.AudioConfigurationV4
import com.lingxi.code.voice.audio.AudioReadiness
import com.lingxi.code.voice.audio.AudioRecognitionPreference
import com.lingxi.code.voice.audio.AudioSource
import com.lingxi.code.voice.audio.AudioSpeechPreference
import com.lingxi.code.voice.audio.AudioVoiceSelection
import com.lingxi.code.voice.offline.ModelState
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class VoiceSettingsContractTest {

    @Test
    fun automaticRecognitionUsesSystemWhenAvailableEvenWithOfflineModelsInstalled() {
        val capability = VoiceSettingsCapabilityResolver.resolve(
            preferences = AudioConfigurationV4(
                recognition = AudioRecognitionPreference(source = AudioSource.AUTOMATIC),
                language = "zh-CN",
            ),
            platform = platformSnapshot(
                recognizerAvailable = true,
                modelStates = mapOf("sherpa.zipformer-zh-14m-mobile" to ModelState.Ready),
            ),
        )

        assertEquals(AudioSource.SYSTEM.value, capability.effectiveRecognitionBackend)
        assertEquals(AudioReadiness.AVAILABLE, capability.recognitionReadiness)
        assertTrue(capability.blockingIssues.isEmpty())
    }

    @Test
    fun explicitOfflineRecognitionRequiresItsInstalledCompatibleModel() {
        val capability = VoiceSettingsCapabilityResolver.resolve(
            preferences = AudioConfigurationV4(
                recognition = AudioRecognitionPreference(source = AudioSource.OFFLINE),
                language = "zh-CN",
            ),
            platform = platformSnapshot(recognizerAvailable = true),
        )

        assertEquals("unavailable", capability.effectiveRecognitionBackend)
        assertEquals(AudioReadiness.UNAVAILABLE, capability.recognitionReadiness)
        assertTrue(capability.blockingIssues.contains(VoiceBlockingIssue.OfflineRecognitionModelRequired))
    }

    @Test
    fun explicitUnknownOfflineVoiceRemainsUnavailableInsteadOfFallingBack() {
        val voice = AudioVoiceSelection(AudioSource.OFFLINE, "missing", "sherpa.kitten-nano-en")
        val capability = VoiceSettingsCapabilityResolver.resolve(
            preferences = AudioConfigurationV4(
                speech = AudioSpeechPreference(
                    source = AudioSource.OFFLINE,
                    offlineModelId = voice.modelId,
                    voice = voice,
                ),
                language = "en-US",
            ),
            platform = platformSnapshot(
                modelStates = mapOf("sherpa.kitten-nano-en" to ModelState.Ready),
            ),
        )

        assertEquals(voice, capability.requestedVoice?.selection)
        assertEquals(null, capability.effectiveVoice)
        assertTrue(capability.blockingIssues.contains(VoiceBlockingIssue.RequestedVoiceUnavailable))
    }

    @Test
    fun microphoneDenialDoesNotHidePlaybackVoiceSupport() {
        val capability = VoiceSettingsCapabilityResolver.resolve(
            preferences = AudioConfigurationV4(language = "en-US"),
            platform = platformSnapshot(
                microphonePermission = VoicePermissionStatus.Denied,
                recognizerAvailable = true,
            ),
        )

        assertEquals(AudioReadiness.PERMISSION_REQUIRED, capability.recognitionReadiness)
        assertEquals(AudioReadiness.AVAILABLE, capability.speechReadiness)
        assertTrue(capability.blockingIssues.contains(VoiceBlockingIssue.MicrophonePermissionRequired))
        assertFalse(capability.systemVoiceOptions.isEmpty())
    }

    private fun platformSnapshot(
        microphonePermission: VoicePermissionStatus = VoicePermissionStatus.Granted,
        recognizerAvailable: Boolean = true,
        modelStates: Map<String, ModelState> = emptyMap(),
    ) = VoicePlatformSnapshot(
        localeTag = "en-US",
        microphonePermission = microphonePermission,
        platformRecognizerAvailable = recognizerAvailable,
        systemVoices = listOf(
            VoiceOption(
                id = "system:default",
                label = "System default",
                languageTag = "en-US",
                source = VoiceOptionSource.System,
                familyId = "system",
                isDefault = true,
                selection = AudioVoiceSelection(AudioSource.SYSTEM, "default"),
            ),
            VoiceOption(
                id = "system:android-en-1",
                label = "Android English 1",
                languageTag = "en-US",
                source = VoiceOptionSource.System,
                familyId = "system",
                selection = AudioVoiceSelection(AudioSource.SYSTEM, "android-en-1"),
            ),
        ),
        defaultSystemVoiceId = "default",
        modelStates = modelStates,
    )
}
