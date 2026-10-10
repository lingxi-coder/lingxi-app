package com.lingxi.code.voice.audio

import org.json.JSONObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class AndroidProviderAudioRequestTest {
    private val request = DeviceAudioRequest(AudioOperationIdentity("request", 1, 1), AudioOwnerKey.ui("preview"),
        null, 1024, DeviceAudioOperation.Status(null))
    private val session = JSONObject("""{"sessionId":"actual-session","profileId":"actual-profile","accountScope":"actual-account","region":"international"}""")

    @Test fun localSpeechVoiceDoesNotPolluteProviderCapabilityProbe() {
        val config = AudioConfigurationV4(speech = AudioSpeechPreference(AudioSource.SYSTEM,
            voice = AudioVoiceSelection(AudioSource.SYSTEM, "local-device-voice")))
        val json = JSONObject(providerAudioRequestJson(request, config, "speech", session))
        assertTrue(json.isNull("voice"))
        assertEquals("actual-profile", json.getJSONObject("session").getString("profileId"))
        assertEquals("actual-account", json.getJSONObject("session").getString("accountScope"))
        assertFalse(json.getJSONObject("session").has("region"))
    }

    @Test fun modelLessProviderVoiceKeepsNullCatalogModel() {
        val config = AudioConfigurationV4(speech = AudioSpeechPreference(AudioSource.PROVIDER,
            voice = AudioVoiceSelection(AudioSource.PROVIDER, "provider-voice", profileId = "actual-profile")))
        val json = JSONObject(providerAudioRequestJson(request, config, "speech", session))
        assertEquals("provider-voice", json.getString("voice"))
        assertTrue(json.getJSONObject("cloud").isNull("modelId"))
    }

    @Test fun voiceFromAnotherProfileFailsBeforeProviderDispatch() {
        val config = AudioConfigurationV4(speech = AudioSpeechPreference(AudioSource.PROVIDER,
            voice = AudioVoiceSelection(AudioSource.PROVIDER, "provider-voice", profileId = "another-profile")))
        try {
            providerAudioRequestJson(request, config, "speech", session)
            throw AssertionError("foreign profile voice was accepted")
        } catch (error: AudioOperationException) { assertEquals(DeviceAudioErrorKind.InvalidRequest, error.kind) }
    }
}
