package com.lingxi.code.voice

import com.lingxi.code.voice.audio.AudioSource
import com.lingxi.code.voice.audio.AudioSpeechPreference
import com.lingxi.code.voice.audio.AudioVoiceSelection
import com.lingxi.code.voice.audio.AudioConfigurationV3
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class AndroidVoiceRuntimeVoiceKeyTest {
    @Test
    fun configuredOfflineKeyResolvesItsModelAndVoice() {
        assertEquals(
            AudioVoiceSelection(
                source = AudioSource.OFFLINE,
                id = "expr-voice-2-f",
                modelId = "sherpa.kitten-nano-en",
            ),
            parseExplicitVoiceSelection("offline:sherpa.kitten-nano-en:expr-voice-2-f"),
        )
    }

    @Test
    fun legacySherpaKeyRemainsSupported() {
        assertEquals(
            AudioVoiceSelection(
                source = AudioSource.OFFLINE,
                id = "expr-voice-2-f",
                modelId = "sherpa.kitten-nano-en",
            ),
            parseExplicitVoiceSelection("sherpa:sherpa.kitten-nano-en:expr-voice-2-f"),
        )
    }

    @Test
    fun incompleteOfflineKeysAreNotTreatedAsExplicitVoiceSelections() {
        assertNull(parseExplicitVoiceSelection("offline:sherpa.kitten-nano-en"))
        assertNull(parseExplicitVoiceSelection("offline::voice"))
        assertNull(parseExplicitVoiceSelection("offline:model:"))
    }

    @Test
    fun defaultAndAutoCallOverridesClearTheFixedVoiceButRetainTheConfiguredSource() {
        val fixedOfflineVoice = AudioVoiceSelection(AudioSource.OFFLINE, "kitten-default-en", "sherpa.kitten-nano-en")
        val preferences = AudioConfigurationV3(
            speech = AudioSpeechPreference(
                source = AudioSource.OFFLINE,
                offlineModelId = "sherpa.kitten-nano-en",
                voice = fixedOfflineVoice,
            ),
        )

        listOf("default", "auto", " DEFAULT ").forEach { override ->
            val resolved = resolveSpeechVoiceOverride(override, preferences)

            assertEquals(AudioSource.OFFLINE, resolved.preference.source)
            assertEquals("sherpa.kitten-nano-en", resolved.preference.offlineModelId)
            assertNull(resolved.preference.voice)
            assertNull(resolved.voiceOverride)
        }
        assertEquals("call override must not mutate the persisted preference", fixedOfflineVoice, preferences.speech.voice)
    }
}
