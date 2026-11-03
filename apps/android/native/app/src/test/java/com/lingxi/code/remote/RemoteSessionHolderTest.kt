package com.lingxi.code.remote

import com.lingxi.code.bindings.client.ImageRefDto
import com.lingxi.code.conversation.DefaultConversationStrings
import com.lingxi.code.conversation.ExternalSendFailure
import com.lingxi.code.conversation.FakeRemoteSessionPort
import com.lingxi.code.conversation.RemoteConversationSource
import com.lingxi.code.conversation.remoteUpdate
import com.lingxi.code.conversation.sendComposerIfAdmitted
import com.lingxi.code.model.SessionRef
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.test.UnconfinedTestDispatcher
import kotlinx.coroutines.test.resetMain
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest
import kotlinx.coroutines.test.setMain
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotSame
import org.junit.Assert.assertSame
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Test

@OptIn(ExperimentalCoroutinesApi::class)
class RemoteSessionHolderTest {
    private val dispatcher = UnconfinedTestDispatcher()

    @Before fun setMain() = Dispatchers.setMain(dispatcher)
    @After fun resetMain() = Dispatchers.resetMain()

    private fun open(holder: RemoteSessionHolder, port: FakeRemoteSessionPort) = holder.open(
        key = "account/host/s1",
        create = { RemoteConversationSource(port, "s1", "Pixel") },
        onCreated = { it.chat.beginExternalLoading(SessionRef("s1", "session")) },
        strings = DefaultConversationStrings,
    )

    @Test fun leavingDoesNotCancelAnAdmittedSendAndKeepsItsFailedDraft() = runTest(dispatcher) {
        val holder = RemoteSessionHolder()
        val port = FakeRemoteSessionPort()
        val gate = CompletableDeferred<Unit>()
        port.sendGate = gate
        val owner = open(holder, port)
        port.updates.emit(remoteUpdate(replace = true))
        val image = ImageRefDto("image/png", "AQID")
        owner.chat.send("draft", listOf(image))
        assertEquals(1, owner.chat.externalSendInFlight.value)
        holder.release("account/host/s1")
        gate.completeExceptionally(IllegalStateException("offline"))
        runCurrent()
        val reopened = open(holder, port)
        assertSame(owner, reopened)
        assertTrue(reopened.needsReattach)
        assertEquals(listOf(ExternalSendFailure("draft", listOf(image))), reopened.chat.externalSendFailures.value)
    }

    @Test fun unsentDraftPinsOwnerAndAnIdleOwnerIsReleased() = runTest(dispatcher) {
        val holder = RemoteSessionHolder()
        val port = FakeRemoteSessionPort()
        val owner = open(holder, port)
        owner.updateDraft("unsent")
        holder.release("account/host/s1")
        val reopened = open(holder, port)
        assertSame(owner, reopened)
        assertTrue(reopened.needsReattach)
        owner.updateDraft("")
        holder.release("account/host/s1")
        runCurrent()
        assertNotSame(owner, open(holder, port))
    }

    @Test fun sendAndImmediateLeaveCannotRestoreTheAlreadySubmittedDraft() = runTest(dispatcher) {
        val holder = RemoteSessionHolder()
        val port = FakeRemoteSessionPort()
        port.sendGate = CompletableDeferred()
        val owner = open(holder, port)
        port.updates.emit(remoteUpdate(replace = true))
        var draft = "submitted"
        owner.updateDraft(draft)
        sendComposerIfAdmitted(
            admitted = owner.chat.admitComposerSend(draft, hasImages = false),
            send = { owner.chat.send(draft) },
            clearDraft = { owner.updateDraft(""); draft = "" },
            removeAttachment = {},
            followTail = {},
        )
        holder.release("account/host/s1")
        val reopened = open(holder, port)
        assertSame(owner, reopened)
        assertEquals("", reopened.savedDraft)
        port.sendGate!!.complete(Unit)
    }
}
