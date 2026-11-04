package com.lingxi.code.conversation

import android.content.Context
import android.content.ContextWrapper
import androidx.lifecycle.ViewModelStore
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import com.lingxi.code.model.ConversationScope
import com.lingxi.code.model.SessionMode
import com.lingxi.code.model.SessionRef
import com.lingxi.code.settings.LinuxRuntimeMode
import java.io.File
import java.util.UUID
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.emitAll
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.flow.flow
import kotlinx.coroutines.flow.receiveAsFlow
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withContext
import kotlinx.coroutines.withTimeout
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith

@RunWith(AndroidJUnit4::class)
class SessionHostHandoffLeaseTest {
    @Test fun activeOwnerKeepsItsTurnAndPermissionAcrossModeNavigation() = runBlocking {
        val target = InstrumentationRegistry.getInstrumentation().targetContext
        val directory = File(target.cacheDir, "retained-owner-${UUID.randomUUID()}").apply { mkdirs() }
        val context = object : ContextWrapper(target) {
            override fun getApplicationContext(): Context = this
            override fun getFilesDir(): File = directory
        }
        class FakeSource(val id: String, val mode: SessionMode) : ConversationSource {
            override val recoverySpec = ConversationRecoverySpec(null, null, mode,
                LinuxRuntimeMode.MobileLinux, workspaceKey = "global", transcriptPath = "/guest/$id.jsonl")
            override val activeSessionState = MutableStateFlow<ActivatedSession?>(
                ActivatedSession(id, emptyList(), SessionActivationKind.Resumed, mode),
            )
            override val pendingPermission = MutableStateFlow<PermissionPromptState?>(null)
            val admitted = CompletableDeferred<Unit>()
            val replies = Channel<ReplyEvent>(Channel.UNLIMITED)
            var closed = false
            var cancelled = false
            override fun submit(text: String) = flow {
                admitted.complete(Unit)
                emitAll(replies.receiveAsFlow())
            }
            override suspend fun cancel() { cancelled = true }
            override fun close() { closed = true; replies.close() }
        }
        withContext(Dispatchers.Main.immediate) {
            val store = ViewModelStore()
            val host = SessionHost(context, 1)
            store.put("host", host)
            val a = FakeSource(UUID.randomUUID().toString(), SessionMode.Code)
            val b = FakeSource(UUID.randomUUID().toString(), SessionMode.Chat)
            val aRef = SessionRef(a.id, "Code owner")
            try {
                val first = checkNotNull(host.acquire(ConversationScope.Global, a.mode, aRef, 1) { a })
                host.activate(first)
                withTimeout(10_000) { first.viewModel.state.first { it.sessionReady && it.session.id == a.id } }
                first.viewModel.send("keep working")
                withTimeout(10_000) { a.admitted.await() }
                val second = checkNotNull(host.acquire(ConversationScope.Global, b.mode, SessionRef(b.id, "Chat owner"), 1) { b })
                host.activate(second)
                host.release(first)
                val prompt = PermissionPromptState(41uL, "Permission", "Continue?", originSessionId = a.id)
                a.pendingPermission.value = prompt
                a.replies.send(ReplyEvent.Delta("late output"))
                withTimeout(10_000) { first.viewModel.pendingPermission.first { it == prompt } }
                withTimeout(10_000) { first.viewModel.state.first { it.streamingMessage?.text == "late output" } }
                assertTrue(first.viewModel.state.value.streaming)
                assertNull(second.viewModel.pendingPermission.value)
                assertFalse(a.closed)
                assertFalse(a.cancelled)
                val again = checkNotNull(host.acquire(ConversationScope.Global, a.mode, aRef, 1) {
                    error("Retained owner must not rebuild its source")
                })
                assertTrue(again.viewModel === first.viewModel)
                host.activate(again)
                assertEquals(prompt, again.viewModel.pendingPermission.value)
                a.pendingPermission.value = null
                a.replies.send(ReplyEvent.End)
                withTimeout(10_000) { again.viewModel.state.first { !it.streaming } }
                host.release(again)
                host.release(second)
            } finally {
                store.clear()
            }
        }
    }

    @Test fun rejectedColdHandoffReleasesOnlyItsTemporaryOwner() = runBlocking {
        val target = InstrumentationRegistry.getInstrumentation().targetContext
        val directory = File(target.cacheDir, "handoff-lease-${UUID.randomUUID()}").apply { mkdirs() }
        val context = object : ContextWrapper(target) {
            override fun getApplicationContext(): Context = this
            override fun getFilesDir(): File = directory
        }
        withContext(Dispatchers.Main.immediate) {
            val store = ViewModelStore()
            val host = SessionHost(context, 1)
            store.put("host", host)
            val sessionId = UUID.randomUUID().toString()
            val ref = SessionRef(sessionId, "session")
            class FakeSource(path: String) : ConversationSource {
                override val recoverySpec = ConversationRecoverySpec(null, null, SessionMode.Chat,
                    LinuxRuntimeMode.MobileLinux, workspaceKey = "global", transcriptPath = path)
                var closed = false
                override fun close() { closed = true }
            }
            val first = FakeSource("/a/session.jsonl")
            val replacement = FakeSource("/b/session.jsonl")
            try {
                val lease = checkNotNull(host.acquire(ConversationScope.Global, SessionMode.Chat, ref, 1) { first })
                assertTrue(lease.created)
                assertNotNull(host.find(ConversationScope.Global, sessionId))
                // The selected A row is rejected after its index changes to B,
                // before PrepareSessionHandoff is sent.
                assertFalse(selectedTranscriptPathMatches(first.recoverySpec.transcriptPath, replacement.recoverySpec.transcriptPath))
                host.release(lease, discard = true)
                assertTrue(first.closed)
                assertNull(host.find(ConversationScope.Global, sessionId))
                val next = checkNotNull(host.acquire(ConversationScope.Global, SessionMode.Chat, ref, 1) { replacement })
                assertTrue(next.created)
                assertEquals("/b/session.jsonl", next.viewModel.engineSource.value.recoverySpec?.transcriptPath)
                host.release(next, discard = true)
            } finally {
                store.clear()
            }
        }
    }
}
