package com.lingxi.code.voice.audio

import org.json.JSONObject
import org.junit.Assert.assertEquals
import org.junit.Test

class AndroidProviderAudioCapabilityTest {
    @Test fun canonicalVoiceObjectsRetainExactProfileAndNullableModelScope() {
        val capability = parseProviderAudioCapability(JSONObject("""{
            "supported":true,"readiness":"ready","profileId":"actual-profile","providerId":"provider",
            "modelId":null,"models":[{"id":null,"voices":[{"id":"voice-a","label":"Voice A"},{"id":"voice-b"}]}]
        }"""))
        assertEquals(listOf<String?>(null), capability.modelIds)
        assertEquals(AudioVoiceSelection(AudioSource.PROVIDER, "voice-a", profileId = "actual-profile"), capability.voices[0].selection)
        assertEquals("Voice A", capability.voices[0].label)
        assertEquals("voice-b", capability.voices[1].label)
    }

    @Test fun stringVoiceListIsRejected() = assertInvalid("""{
        "supported":true,"readiness":"ready","models":[{"id":"audio-model","voices":["voice-a"]}]
    }""")

    @Test fun stringModelListIsRejected() = assertInvalid("""{
        "supported":true,"readiness":"ready","models":["audio-model"]
    }""")

    @Test fun flatModelAndVoiceListsAreRejected() = assertInvalid("""{
        "supported":true,"readiness":"ready","modelIds":["audio-model"],"voices":["voice-a"]
    }""")

    @Test fun nestedSupportedReadinessAliasIsRejected() = assertInvalid("""{
        "capabilities":{"supported":true,"readiness":"ready"},"models":[]
    }""")

    private fun assertInvalid(json: String) {
        try {
            parseProviderAudioCapability(JSONObject(json))
            throw AssertionError("obsolete capability shape was accepted")
        } catch (error: AudioOperationException) { assertEquals(DeviceAudioErrorKind.InvalidRequest, error.kind) }
    }
}
