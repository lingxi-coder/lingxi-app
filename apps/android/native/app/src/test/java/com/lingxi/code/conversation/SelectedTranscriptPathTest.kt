package com.lingxi.code.conversation

import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.CoroutineStart
import kotlinx.coroutines.async
import kotlinx.coroutines.runBlocking
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class SelectedTranscriptPathTest {
    @Test fun catalogChangeDuringNavigationCannotConstructTheReplacementPath() = runBlocking {
        val selected = "/a/session.jsonl"
        var indexed = selected
        val gate = CompletableDeferred<Unit>()
        val constructed = mutableListOf<String?>()
        val navigation = async(start = CoroutineStart.UNDISPATCHED) {
            assertTrue(selectedTranscriptPathMatches(selected, indexed))
            gate.await()
            runCatching {
                val path = selectedTranscriptPathForCreation(selected, indexed, required = true)
                constructed += path
            }
        }
        indexed = "/b/session.jsonl"
        gate.complete(Unit)
        assertTrue(navigation.await().isFailure)
        assertTrue(constructed.isEmpty())
        assertEquals(selected, selectedTranscriptPathForCreation(selected, selected, required = true))
    }
}
