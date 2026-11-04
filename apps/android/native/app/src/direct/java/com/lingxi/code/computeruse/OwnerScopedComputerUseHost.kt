package com.lingxi.code.computeruse

import com.lingxi.code.bindings.android.AndroidComputerUseHost
import com.lingxi.code.bindings.android.AndroidScreenshotFfi
import kotlinx.coroutines.withContext

/** Captures the engine origin before entering any shared native control callback. */
internal class OwnerScopedComputerUseHost(
    private val delegate: AndroidComputerUseHost,
    private val owner: String,
    private val owners: ComputerUseOwnerRegistry,
) : AndroidComputerUseHost {
    private suspend fun <T> owned(action: suspend () -> T): T {
        owners.enter(owner)
        try { return withContext(ComputerUseCallOwner(owner)) { action() } } finally { owners.leave(owner) }
    }

    override suspend fun statusJson(): String = owned { delegate.statusJson() }
    override suspend fun requestAccessJson(requestJson: String): String = owned { delegate.requestAccessJson(requestJson) }
    override suspend fun listGrantedAppsJson(): String = owned { delegate.listGrantedAppsJson() }
    override suspend fun screenshot(): AndroidScreenshotFfi = owned { delegate.screenshot() }
    override suspend fun uiTreeJson(): String = owned { delegate.uiTreeJson() }
    override suspend fun findNodesJson(queryJson: String): String = owned { delegate.findNodesJson(queryJson) }
    override suspend fun inspectNodeJson(nodeId: String): String = owned { delegate.inspectNodeJson(nodeId) }
    override suspend fun performJson(actionJson: String): String = owned { delegate.performJson(actionJson) }
    override suspend fun waitForJson(conditionJson: String, timeoutMs: ULong): String = owned { delegate.waitForJson(conditionJson, timeoutMs) }
    override suspend fun listenJson(requestJson: String): String = owned { delegate.listenJson(requestJson) }
    override suspend fun speakJson(requestJson: String): String = owned { delegate.speakJson(requestJson) }
    override suspend fun stopAudio() = owned { delegate.stopAudio() }
    override suspend fun stop() = owned { delegate.stop() }
}
