package com.lingxi.code.conversation

import com.lingxi.code.bindings.client.MessageDto
import com.lingxi.code.bindings.client.MessageBlockDto
import org.junit.Assert.*
import org.junit.Test

class UserMessageMetadataTest {
    @Test fun repeatedPromptsRetainTheirOwnHistoricalClocks() {
        val first = MessageDto(transcriptUuid = null, role = "user", blocks = listOf(MessageBlockDto.Text("same prompt".toClientTextDto())), sentAtMs = 1_700_000_000_000L)
        val second = first.copy(sentAtMs = 1_700_000_060_000L)
        assertEquals(listOf(first.sentAtMs, second.sentAtMs), transcriptFromDtos(listOf(first, second)).map { it.sentAtMs })
        assertEquals(first.sentAtMs, messageDtoToMessage(first).sentAtMs)
    }

    @Test fun missingHistoricalClockIsNotReplacedWithCurrentTime() {
        val dto = MessageDto(transcriptUuid = null, role = "user", blocks = listOf(MessageBlockDto.Text("old prompt".toClientTextDto())))
        assertNull(messageDtoToMessage(dto).sentAtMs)
        assertNull(transcriptFromDtos(listOf(dto)).single().sentAtMs)
    }
}
