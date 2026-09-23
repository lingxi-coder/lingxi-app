package com.lingxi.code.computeruse

import com.lingxi.code.bindings.AndroidComputerUseFfiException
import com.lingxi.code.voice.audio.AudioOperationException
import com.lingxi.code.voice.audio.DeviceAudioErrorKind
import org.junit.Assert.assertTrue
import org.junit.Test

class ComputerUseAudioErrorMappingTest {
    @Test
    fun microphoneDenialRemainsPermissionDeniedAtComputerUseBoundary() {
        val error = AudioOperationException(DeviceAudioErrorKind.PermissionDenied, "microphone access was denied")
            .toComputerUseAudioFfiException()

        assertTrue(error is AndroidComputerUseFfiException.PermissionDenied)
    }

    @Test
    fun busyAudioReturnsAUsefulRetryMessage() {
        val error = AudioOperationException(DeviceAudioErrorKind.Busy, "another microphone capture is active")
            .toComputerUseAudioFfiException()

        assertTrue(error is AndroidComputerUseFfiException.Other)
        assertTrue(error.message.orEmpty().contains("请稍后重试"))
    }
}
