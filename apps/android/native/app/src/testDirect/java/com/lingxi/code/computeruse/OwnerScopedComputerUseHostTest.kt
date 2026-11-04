package com.lingxi.code.computeruse

import com.lingxi.code.bindings.android.AndroidComputerUseHost
import com.lingxi.code.bindings.android.AndroidScreenshotFfi
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.async
import kotlinx.coroutines.currentCoroutineContext
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest
import org.junit.Assert.assertEquals
import org.junit.Test

class OwnerScopedComputerUseHostTest {
    @OptIn(kotlinx.coroutines.ExperimentalCoroutinesApi::class)
    @Test fun callbackOriginSurvivesUntilAudioAndStop() = runTest {
        val registry = ComputerUseOwnerRegistry()
        val entered = CompletableDeferred<Unit>()
        val finish = CompletableDeferred<Unit>()
        val delegate = object : AndroidComputerUseHost {
            override suspend fun performJson(actionJson: String): String {
                entered.complete(Unit)
                finish.await()
                return "done"
            }
            override suspend fun statusJson(): String = ""
            override suspend fun requestAccessJson(requestJson: String): String = ""
            override suspend fun listGrantedAppsJson(): String = ""
            override suspend fun screenshot(): AndroidScreenshotFfi = error("unused")
            override suspend fun uiTreeJson(): String = ""
            override suspend fun findNodesJson(queryJson: String): String = ""
            override suspend fun inspectNodeJson(nodeId: String): String = ""
            override suspend fun waitForJson(conditionJson: String, timeoutMs: ULong): String = ""
            override suspend fun listenJson(requestJson: String): String = ""
            override suspend fun speakJson(requestJson: String): String =
                currentCoroutineContext()[ComputerUseCallOwner]?.value.orEmpty()
            override suspend fun stopAudio() = Unit
            override suspend fun stop() = Unit
        }
        val code = OwnerScopedComputerUseHost(delegate, "code-engine", registry)
        val call = async { code.performJson("{}") }
        runCurrent()
        entered.await()
        assertEquals(setOf("code-engine"), registry.owners())
        finish.complete(Unit)
        assertEquals("done", call.await())
        assertEquals(emptySet<String>(), registry.owners())
        val chat = OwnerScopedComputerUseHost(delegate, "chat-engine", registry)
        assertEquals("code-engine", code.speakJson("{}"))
        assertEquals("chat-engine", chat.speakJson("{}"))
        assertEquals(emptySet<String>(), registry.owners())
    }
}
