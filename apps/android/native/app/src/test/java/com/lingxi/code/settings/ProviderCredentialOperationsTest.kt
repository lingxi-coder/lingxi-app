package com.lingxi.code.settings

import com.lingxi.code.bindings.client.ClientCommand
import com.lingxi.code.bindings.client.ClientEvent
import com.lingxi.code.bindings.client.ProviderCredentialSecretDto
import com.lingxi.code.conversation.ConversationSource
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.async
import kotlinx.coroutines.cancelAndJoin
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.launch
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest
import org.junit.Assert.*
import org.junit.Test

@OptIn(ExperimentalCoroutinesApi::class)
class ProviderCredentialOperationsTest {
    @Test fun immediateNativeAcknowledgementIsObservedAndUnrelatedReplyIsIgnored() = runTest {
        val source = Source()
        source.beforeReply = { command ->
            source.clientEvents.emit(status(operationID(command) + 1uL, emptyList(), "unrelated failure"))
        }
        val snapshot = runLiveProviderCredentialOperation(source, 100uL, set(100uL, "replacement")) { true }
        assertNull(snapshot.error)
        assertEquals(setOf("anthropic"), snapshot.configuredProviderIds)
        assertEquals(1, source.modelRefreshes)
        assertEquals(0, source.clientEvents.subscriptionCount.value)
    }

    @Test fun canceledOperationRemovesItsCollectorWithoutClaimingSuccess() = runTest {
        val source = Source()
        val gate = CompletableDeferred<Unit>()
        source.beforeReply = { gate.await() }
        var completed = false
        val pending = launch {
            runLiveProviderCredentialOperation(source, 100uL, set(100uL, "replacement")) { true }
            completed = true
        }
        runCurrent()
        assertEquals(1, source.clientEvents.subscriptionCount.value)
        pending.cancelAndJoin()
        assertFalse(completed)
        assertEquals(0, source.clientEvents.subscriptionCount.value)
    }

    @Test fun lateAcknowledgementFromReplacedBindingCannotClaimTheNewEngineWasUpdated() = runTest {
        val source = Source()
        val gate = CompletableDeferred<Unit>()
        var current = true
        source.beforeReply = { gate.await() }
        val pending = async {
            runCatching { runLiveProviderCredentialOperation(source, 100uL, set(100uL, "replacement")) { current } }
        }
        runCurrent()
        current = false
        gate.complete(Unit)
        assertTrue(pending.await().isFailure)
        assertEquals(0, source.clientEvents.subscriptionCount.value)
    }

    @Test fun storageFailureDoesNotAdvertiseNewModelAvailability() = runTest {
        val source = Source().apply { failure = "secure storage unavailable" }
        val snapshot = runLiveProviderCredentialOperation(source, 100uL, set(100uL, "replacement")) { true }
        assertEquals("secure storage unavailable", snapshot.error)
        assertEquals(0, source.modelRefreshes)
    }

    @Test fun deleteAcknowledgementRefreshesModelsWithoutRecreatingTheSource() = runTest {
        val source = Source().apply { keys["anthropic"] = "fixture" }
        val snapshot = runLiveProviderCredentialOperation(source, 100uL, ClientCommand.DeleteProviderCredential(100uL, "anthropic")) { true }
        assertTrue(snapshot.configuredProviderIds.isEmpty())
        assertTrue(source.keys.isEmpty())
        assertEquals(1, source.modelRefreshes)
    }

    @Test fun modelRefreshFailureCannotTurnAConfirmedCredentialWriteIntoAFailedSave() = runTest {
        val source = Source().apply { failRefresh = true }
        val snapshot = runLiveProviderCredentialOperation(source, 100uL, set(100uL, "replacement")) { true }
        assertNull(snapshot.error)
        assertEquals(setOf("anthropic"), snapshot.configuredProviderIds)
    }

    private class Source : ConversationSource {
        override val clientEvents = MutableSharedFlow<ClientEvent>()
        val keys = mutableMapOf<String, String>()
        var modelRefreshes = 0
        var failRefresh = false
        var beforeReply: suspend (ClientCommand) -> Unit = {}
        var failure: String? = null
        override suspend fun submitClientCommand(command: ClientCommand) {
            if (command is ClientCommand.ListModels) {
                modelRefreshes += 1
                check(!failRefresh) { "model refresh transport failure" }
                return
            }
            beforeReply(command)
            if (failure == null) when (command) {
                is ClientCommand.SetProviderCredential -> keys[command.providerId] = command.credential.value
                is ClientCommand.DeleteProviderCredential -> keys.remove(command.providerId)
                else -> error("unexpected command")
            }
            clientEvents.emit(status(operationID(command), keys.keys.toList(), failure))
        }
    }

    private companion object {
        fun set(id: ULong, secret: String) = ClientCommand.SetProviderCredential(id, "anthropic", ProviderCredentialSecretDto(secret))
        fun operationID(command: ClientCommand): ULong = when (command) {
            is ClientCommand.SetProviderCredential -> command.operationId
            is ClientCommand.DeleteProviderCredential -> command.operationId
            else -> error("unexpected command")
        }
        fun status(id: ULong, configured: List<String>, error: String? = null) = ClientEvent.ProviderCredentialStatus(
            operationId = id,
            configuredProviderIds = configured,
            unavailableProviderIds = emptyList(),
            storageEncrypted = true,
            credentialPreviews = emptyMap(),
            error = error,
        )
    }
}
