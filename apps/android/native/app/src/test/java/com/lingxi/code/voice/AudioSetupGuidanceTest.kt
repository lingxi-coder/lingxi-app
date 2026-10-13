package com.lingxi.code.voice

import org.junit.Assert.*
import org.junit.Test

class AudioSetupGuidanceTest {
    @Test fun classifiesRecoveryWithoutExposingRawProviderMessages() {
        assertEquals(AudioSetupGuidance.Quota, AudioSetupGuidance.fromError(message = "HTTP 429 secret-token"))
        assertEquals(AudioSetupGuidance.Permission, AudioSetupGuidance.fromError("permission_denied"))
        assertEquals(AudioSetupGuidance.Configuration, AudioSetupGuidance.fromError("unsupported"))
        assertEquals(AudioSetupGuidance.Configuration, AudioSetupGuidance.fromError("model_missing"))
        assertEquals(AudioSetupGuidance.Configuration, AudioSetupGuidance.fromError(message = "systemUnavailable"))
        assertEquals(AudioSetupGuidance.Retry, AudioSetupGuidance.fromError("no_speech"))
        assertNull(AudioSetupGuidance.fromError("cancelled"))
    }
}
