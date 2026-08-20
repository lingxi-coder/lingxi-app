package com.lingxi.code.settings

import com.lingxi.code.model.VoiceConfig
import com.lingxi.code.voice.offline.ModelState
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class VoiceSettingsContractTest {

    @Test
    fun migratesLegacyPreferencesAndAppearanceVoiceLang() {
        val config = resolveVoiceConfig(
            schemaVersion = 0,
            recognitionMode = null,
            language = null,
            voiceSelection = null,
            rate = null,
            autoPlayReplies = null,
            legacyInputLanguage = "auto",
            legacyVoiceId = "zh-CN-XiaoxiaoNeural",
            legacySpeed = 1.4f,
            legacyAutoPlay = true,
            legacyVoiceLang = "en",
        )

        assertEquals(VoiceConfig.CURRENT_SCHEMA_VERSION, config.schemaVersion)
        assertEquals(VoiceConfig.MODE_AUTOMATIC, config.recognitionMode)
        assertEquals("en-US", config.language)
        assertEquals(VoiceConfig.DEFAULT_VOICE_SELECTION, config.voiceSelection)
        assertEquals(1.4f, config.rate, 1e-4f)
        assertTrue(config.autoPlayReplies)
    }

    @Test
    fun automaticPrefersSystemRecognizerWhenAvailable() {
        val capability = VoiceSettingsCapabilityResolver.resolve(
            preferences = VoiceConfig(
                recognitionMode = VoiceConfig.MODE_AUTOMATIC,
                language = "zh-CN",
                voiceSelection = VoiceConfig.DEFAULT_VOICE_SELECTION,
            ),
            platform = platformSnapshot(
                recognizerAvailable = true,
                modelStates = mapOf("sherpa.zipformer-zh-14m-mobile" to ModelState.Ready),
            ),
        )

        assertEquals("system", capability.effectiveRecognitionBackend)
        assertTrue(capability.blockingIssues.isEmpty())
    }

    @Test
    fun localOnlyRequiresReadySherpaRecognitionModel() {
        val capability = VoiceSettingsCapabilityResolver.resolve(
            preferences = VoiceConfig(
                recognitionMode = VoiceConfig.MODE_LOCAL_ONLY,
                language = "zh-CN",
                voiceSelection = VoiceConfig.DEFAULT_VOICE_SELECTION,
            ),
            platform = platformSnapshot(recognizerAvailable = true),
        )

        assertEquals("unavailable", capability.effectiveRecognitionBackend)
        assertTrue(capability.blockingIssues.contains(VoiceBlockingIssue.OfflineRecognitionModelRequired))
    }

    @Test
    fun runtimeRouteUsesSameStrictLocalAndAutomaticFallbackRules() {
        val readyStates = mapOf("sherpa.zipformer-zh-14m-mobile" to ModelState.Ready)
        val strictMissing = resolveVoiceExecutionRoute(
            preferences = VoiceConfig(
                recognitionMode = VoiceConfig.MODE_LOCAL_ONLY,
                language = "zh-CN",
            ),
            localeTag = "en-US",
            platformRecognizerAvailable = true,
            modelStates = emptyMap(),
        )
        assertEquals(VoiceRecognitionBackend.Unavailable, strictMissing.recognitionBackend)

        val strictReady = resolveVoiceExecutionRoute(
            preferences = VoiceConfig(
                recognitionMode = VoiceConfig.MODE_LOCAL_ONLY,
                language = "zh-CN",
            ),
            localeTag = "en-US",
            platformRecognizerAvailable = true,
            modelStates = readyStates,
        )
        assertEquals(VoiceRecognitionBackend.Sherpa, strictReady.recognitionBackend)

        val automaticFallback = resolveVoiceExecutionRoute(
            preferences = VoiceConfig(
                recognitionMode = VoiceConfig.MODE_AUTOMATIC,
                language = "zh-CN",
            ),
            localeTag = "en-US",
            platformRecognizerAvailable = false,
            modelStates = readyStates,
        )
        assertEquals(VoiceRecognitionBackend.Sherpa, automaticFallback.recognitionBackend)
    }

    @Test
    fun missingRequestedVoiceFallsBackToSystemDefaultButKeepsRequestedSelection() {
        val requestedSelection = VoiceConfig.sherpaVoiceSelection("sherpa.kitten-nano-en", "missing")
        val capability = VoiceSettingsCapabilityResolver.resolve(
            preferences = VoiceConfig(
                recognitionMode = VoiceConfig.MODE_AUTOMATIC,
                language = "en-US",
                voiceSelection = requestedSelection,
            ),
            platform = platformSnapshot(
                recognizerAvailable = true,
                modelStates = mapOf("sherpa.kitten-nano-en" to ModelState.Ready),
            ),
        )

        assertEquals(requestedSelection, capability.requestedVoice?.id)
        assertEquals(
            VoiceConfig.sherpaVoiceSelection("sherpa.kitten-nano-en", "expr-voice-2-f"),
            capability.effectiveVoice?.id,
        )
        assertTrue(capability.blockingIssues.contains(VoiceBlockingIssue.RequestedVoiceUnavailable))
    }

    @Test
    fun incompatibleSherpaVoiceFallsBackToSystemDefaultForSettingsAndRuntime() {
        val requestedSelection = VoiceConfig.sherpaVoiceSelection("sherpa.kitten-nano-en", "expr-voice-2-f")
        val capability = VoiceSettingsCapabilityResolver.resolve(
            preferences = VoiceConfig(
                recognitionMode = VoiceConfig.MODE_AUTOMATIC,
                language = "ja-JP",
                voiceSelection = requestedSelection,
            ),
            platform = platformSnapshot(
                recognizerAvailable = true,
                modelStates = mapOf("sherpa.kitten-nano-en" to ModelState.Ready),
            ),
        )

        assertEquals(requestedSelection, capability.requestedVoice?.id)
        assertEquals(VoiceConfig.DEFAULT_VOICE_SELECTION, capability.effectiveVoice?.id)
        assertTrue(capability.blockingIssues.contains(VoiceBlockingIssue.RequestedVoiceUnavailable))

        val route = resolveVoiceExecutionRoute(
            preferences = VoiceConfig(
                recognitionMode = VoiceConfig.MODE_AUTOMATIC,
                language = "ja-JP",
                voiceSelection = requestedSelection,
            ),
            localeTag = "en-US",
            platformRecognizerAvailable = true,
            modelStates = mapOf("sherpa.kitten-nano-en" to ModelState.Ready),
        )
        assertEquals(VoiceSpeechBackend.System, route.speechBackend)
        assertEquals(null, route.systemVoiceId)
    }

    @Test
    fun microphoneDenialBlocksRecognitionButNotPlaybackCatalog() {
        val capability = VoiceSettingsCapabilityResolver.resolve(
            preferences = VoiceConfig(language = "en-US"),
            platform = platformSnapshot(
                microphonePermission = VoicePermissionStatus.Denied,
                recognizerAvailable = true,
            ),
        )

        assertEquals("unavailable", capability.effectiveRecognitionBackend)
        assertTrue(capability.blockingIssues.contains(VoiceBlockingIssue.MicrophonePermissionRequired))
        assertFalse(capability.systemVoiceOptions.isEmpty())
    }

    private fun platformSnapshot(
        microphonePermission: VoicePermissionStatus = VoicePermissionStatus.Granted,
        recognizerAvailable: Boolean = true,
        modelStates: Map<String, ModelState> = emptyMap(),
    ): VoicePlatformSnapshot = VoicePlatformSnapshot(
        localeTag = "en-US",
        microphonePermission = microphonePermission,
        platformRecognizerAvailable = recognizerAvailable,
        systemVoices = listOf(
            VoiceOption(
                id = VoiceConfig.DEFAULT_VOICE_SELECTION,
                label = "System default",
                languageTag = "en-US",
                source = VoiceOptionSource.System,
                familyId = "system",
                isDefault = true,
            ),
            VoiceOption(
                id = VoiceConfig.systemVoiceSelection("android-en-1"),
                label = "Android English 1",
                languageTag = "en-US",
                source = VoiceOptionSource.System,
                familyId = "system",
            ),
        ),
        defaultSystemVoiceId = VoiceConfig.DEFAULT_VOICE_SELECTION,
        modelStates = modelStates,
    )
}
