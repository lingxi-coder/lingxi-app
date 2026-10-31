package com.lingxi.code.voice.audio

import android.content.ContextWrapper
import com.lingxi.code.bindings.client.MediaDeviceOpenRequestDto
import com.lingxi.code.bindings.client.MediaDeviceRequestDto
import com.lingxi.code.bindings.client.MediaDeviceRequestEnvelopeDto
import com.lingxi.code.bindings.client.MediaDeviceResultDto
import com.lingxi.code.bindings.client.MediaErrorKindDto
import com.lingxi.code.bindings.client.MediaOperationIdDto
import com.lingxi.code.bindings.client.MediaOwnerDto
import com.lingxi.code.bindings.client.MediaSessionIntentDto
import com.lingxi.code.bindings.client.MediaSessionModeDto
import com.lingxi.code.bindings.client.MediaSessionRequestDto
import kotlinx.coroutines.runBlocking
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test
import java.util.UUID

class AndroidNativeMediaOpenReceiptTest {
    private fun adapter(): AndroidNativeMediaDeviceAdapter {
        val context = object : ContextWrapper(null) {
            override fun getApplicationContext() = this
        }
        return AndroidNativeMediaDeviceAdapter(context)
    }

    @Test
    fun invalidOpenBeforeNativeDispatchReturnsCorrelatedNoResourceReceipt() = runBlocking {
        val adapter = adapter()
        val requestId = UUID.randomUUID().toString()
        val response = adapter.request(
            MediaDeviceRequestEnvelopeDto(
                requestId = requestId,
                request = MediaDeviceRequestDto.Open(
                    MediaDeviceOpenRequestDto(
                        identity = MediaOperationIdDto(UUID.randomUUID().toString(), 1uL, 1uL),
                        owner = MediaOwnerDto.Session("open-preflight"),
                        initiator = null,
                        timeoutBudgetMs = null,
                        maxPayloadBytes = 0uL,
                        request = MediaSessionRequestDto(
                            mode = MediaSessionModeDto.AUDIO,
                            intent = MediaSessionIntentDto.OBSERVATION,
                            audioSource = null,
                            videoSource = null,
                        ),
                    ),
                ),
            ),
        ).response
        assertEquals(requestId, response.requestId)
        assertTrue(response.result is MediaDeviceResultDto.OpenFailedWithoutResource)
        val failure = response.result as MediaDeviceResultDto.OpenFailedWithoutResource
        assertEquals(MediaErrorKindDto.INVALID_REQUEST, failure.error.kind)
    }

    @Test
    fun unknownStopDoesNotClaimClosedLeaseReceipt() = runBlocking {
        val requestId = UUID.randomUUID().toString()
        val response = adapter().request(
            MediaDeviceRequestEnvelopeDto(
                requestId = requestId,
                request = MediaDeviceRequestDto.Stop(UUID.randomUUID().toString()),
            ),
        ).response
        assertEquals(requestId, response.requestId)
        assertTrue(response.result is MediaDeviceResultDto.Error)
        assertEquals(MediaErrorKindDto.NOT_FOUND, (response.result as MediaDeviceResultDto.Error).error.kind)
    }
}
