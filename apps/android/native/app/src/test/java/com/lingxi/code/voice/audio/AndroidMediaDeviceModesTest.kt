package com.lingxi.code.voice.audio

import com.lingxi.code.bindings.client.MediaSessionModeDto
import com.lingxi.code.bindings.client.MediaSessionIntentDto
import org.junit.Assert.assertEquals
import org.junit.Test

class AndroidMediaDeviceModesTest {
    @Test
    fun nativeMediaAdapterAdvertisesOnlyItsImplementedSessionModes() {
        assertEquals(
            listOf(MediaSessionModeDto.AUDIO, MediaSessionModeDto.VIDEO, MediaSessionModeDto.AUDIO_VIDEO),
            androidNativeMediaSupportedSessionModes(),
        )
    }

    @Test
    fun observationAudioCaptureDoesNotAcquirePlayback() {
        assertEquals(false, androidMediaNeedsPlayback(MediaSessionModeDto.AUDIO, MediaSessionIntentDto.OBSERVATION))
        assertEquals(false, androidMediaNeedsPlayback(MediaSessionModeDto.AUDIO_VIDEO, MediaSessionIntentDto.OBSERVATION))
        assertEquals(true, androidMediaNeedsPlayback(MediaSessionModeDto.AUDIO, MediaSessionIntentDto.CONVERSATION))
        assertEquals(false, androidMediaNeedsPlayback(MediaSessionModeDto.VIDEO, MediaSessionIntentDto.CONVERSATION))
    }
}
