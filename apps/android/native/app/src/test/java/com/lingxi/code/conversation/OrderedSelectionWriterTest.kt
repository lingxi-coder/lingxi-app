package com.lingxi.code.conversation

import com.lingxi.code.model.SessionMode
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.async
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Test

class OrderedSelectionWriterTest {
    @OptIn(kotlinx.coroutines.ExperimentalCoroutinesApi::class)
    @Test fun laterOwnerSelectionCannotBeOverwrittenByAnEarlierSlowWrite() = runTest {
        val writer = OrderedSelectionWriter(backgroundScope)
        val release = CompletableDeferred<Unit>()
        val written = mutableListOf<SessionMode>()
        val save: suspend (SessionMode) -> Unit = { mode ->
            if (mode == SessionMode.Chat) release.await()
            written += mode
        }
        writer.write(SessionMode.Chat, save) { throw it }
        writer.write(SessionMode.Code, save) { throw it }
        val restored = async { writer.read { written.last() } }
        runCurrent()
        assertFalse(restored.isCompleted)
        release.complete(Unit)
        assertEquals(SessionMode.Code, restored.await())
        assertEquals(listOf(SessionMode.Chat, SessionMode.Code), written)
    }
}
