package com.lingxi.code.conversation

import com.lingxi.code.bindings.android.RemoteImageRefFfi
import com.lingxi.code.bindings.android.RemoteOutgoingStatusFfi
import com.lingxi.code.bindings.android.RemoteOutgoingPromptFfi
import com.lingxi.code.bindings.android.RemotePendingFfi
import com.lingxi.code.bindings.android.RemotePermissionModeFfi
import com.lingxi.code.bindings.android.RemotePromptOriginFfi
import com.lingxi.code.bindings.android.RemoteRecordFfi
import com.lingxi.code.bindings.android.RemoteSessionPhaseFfi
import com.lingxi.code.bindings.client.AskUserQuestionAnswerDto
import com.lingxi.code.bindings.client.AskUserQuestionAnswersDto
import com.lingxi.code.bindings.client.ClientCommand
import com.lingxi.code.bindings.client.ClientEvent
import com.lingxi.code.bindings.client.ImageRefDto
import com.lingxi.code.bindings.client.MessageImageDto
import com.lingxi.code.bindings.client.PermissionKindDto
import com.lingxi.code.bindings.client.PermissionRequest
import com.lingxi.code.bindings.client.PermissionResponseDto
import java.util.Base64
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.flow.toList
import kotlinx.coroutines.launch
import kotlinx.coroutines.test.UnconfinedTestDispatcher
import kotlinx.coroutines.test.runTest
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Assert.fail
import org.junit.Test

@OptIn(ExperimentalCoroutinesApi::class)
class RemoteConversationSourceTest {
    private val mapper = RemoteFeedMapper(DefaultConversationStrings, thisDeviceName = "Pixel 9")

    private fun prompt(turn: ULong, origin: RemotePromptOriginFfi, device: String? = null, images: List<RemoteImageRefFfi> = emptyList()) =
        RemoteRecordFfi.Prompt(turn, "hello $turn", images, origin, device)

    // --- deltas ----------------------------------------------------------

    @Test fun replaceStartsANewBaseFromTheSnapshotMessages() {
        val items = mapper.map(
            remoteUpdate(
                replace = true,
                messages = listOf(textMessage("user", "hi"), textMessage("assistant", "hello")),
            ),
        )
        val base = items.filterIsInstance<ExternalFeedItem.Base>().single()
        assertEquals("s1", base.session.sessionId)
        assertEquals(listOf("hi", "hello"), base.session.transcript.map { it.text })
        assertTrue(items.none { it is ExternalFeedItem.OlderMessages })
    }

    @Test fun anAppendDoesNotReplaceAndKeepsRecordOrder() {
        val items = mapper.map(
            remoteUpdate(
                records = listOf(
                    prompt(1uL, RemotePromptOriginFfi.DESKTOP),
                    RemoteRecordFfi.Event(ClientEvent.TurnStarted(1uL), false),
                    prompt(2uL, RemotePromptOriginFfi.DESKTOP),
                ),
            ),
        )
        assertTrue(items.none { it is ExternalFeedItem.Base })
        val kinds = items.take(3).map { it::class.simpleName }
        assertEquals(listOf("UserPrompt", "Event", "UserPrompt"), kinds)
    }

    @Test fun olderMessagesArePrependedNotTreatedAsABase() {
        val items = mapper.map(remoteUpdate(older = listOf(textMessage("user", "earlier")), hasOlder = true))
        val older = items.filterIsInstance<ExternalFeedItem.OlderMessages>().single()
        assertEquals(listOf("earlier"), older.messages.map { it.text })
        assertTrue(items.none { it is ExternalFeedItem.Base })
    }

    @Test fun aLoadingUpdateOnlyReportsNotReadyAndLeavesTheHeldTranscriptAlone() {
        val items = mapper.map(remoteUpdate(phase = RemoteSessionPhaseFfi.LOADING, replace = true, messages = listOf(textMessage("user", "x"))))
        assertEquals(1, items.size)
        val readiness = items.single() as ExternalFeedItem.Readiness
        assertFalse(readiness.ready)
    }

    @Test fun metaFieldsFollowTheRecordsSoTheyWin() {
        val items = mapper.map(
            remoteUpdate(
                records = listOf(RemoteRecordFfi.Event(ClientEvent.TurnStarted(1uL), false)),
                running = false,
            ),
        )
        val turnIndex = items.indexOfFirst { it is ExternalFeedItem.Turn }
        assertTrue(turnIndex > items.indexOfFirst { it is ExternalFeedItem.Event })
        assertFalse((items[turnIndex] as ExternalFeedItem.Turn).running)
    }

    // --- origins ---------------------------------------------------------

    @Test fun promptOriginsGetLabels() {
        val items = mapper.map(
            remoteUpdate(
                records = listOf(
                    prompt(1uL, RemotePromptOriginFfi.DESKTOP),
                    prompt(2uL, RemotePromptOriginFfi.SCHEDULED),
                    prompt(3uL, RemotePromptOriginFfi.DEVICE, "Pixel 9"),
                    prompt(4uL, RemotePromptOriginFfi.DEVICE, "iPad"),
                    prompt(5uL, RemotePromptOriginFfi.DEVICE, null),
                ),
            ),
        ).filterIsInstance<ExternalFeedItem.UserPrompt>()
        assertEquals(5, items.size)
        assertEquals("来自桌面", items[0].originLabel)
        assertEquals("定时任务", items[1].originLabel)
        assertEquals("来自此设备", items[2].originLabel)
        assertEquals("来自 iPad", items[3].originLabel)
        assertEquals("来自其他设备", items[4].originLabel)
        assertEquals(listOf(1L, 2L, 3L, 4L, 5L), items.map { it.turnId })
    }

    @Test fun aBotsPromptIsLabelledWithTheBotsName() {
        // A Bot's prompt still arrives as a scheduled one; the connection knows whose it was.
        val named = RemoteFeedMapper(DefaultConversationStrings, thisDeviceName = "Pixel 9", botNameOf = { turn -> "Daily digest".takeIf { turn == 2uL } })
        val items = named.map(
            remoteUpdate(
                records = listOf(
                    prompt(1uL, RemotePromptOriginFfi.SCHEDULED),
                    prompt(2uL, RemotePromptOriginFfi.SCHEDULED),
                    prompt(3uL, RemotePromptOriginFfi.DESKTOP),
                ),
            ),
        ).filterIsInstance<ExternalFeedItem.UserPrompt>()
        assertEquals("定时任务", items[0].originLabel)
        assertEquals("来自 Bot「Daily digest」", items[1].originLabel)
        assertEquals("来自桌面", items[2].originLabel)
    }

    @Test fun onlyAScheduledPromptIsLookedUpForABot() {
        val asked = mutableListOf<ULong>()
        val named = RemoteFeedMapper(DefaultConversationStrings, thisDeviceName = "Pixel 9", botNameOf = { turn -> asked += turn; "Bot" })
        named.map(
            remoteUpdate(
                records = listOf(
                    prompt(1uL, RemotePromptOriginFfi.DESKTOP),
                    prompt(2uL, RemotePromptOriginFfi.DEVICE, "iPad"),
                    prompt(3uL, RemotePromptOriginFfi.SCHEDULED),
                ),
            ),
        )
        assertEquals(listOf(3uL), asked)
    }

    @Test fun theSourceAsksItsPortWhoseBotAPromptWas() = runTest(UnconfinedTestDispatcher()) {
        val port = FakeRemoteSessionPort()
        port.botNames[5uL] = "Inbox triage"
        val source = RemoteConversationSource(port, "Pixel 9")
        val items = mutableListOf<ExternalFeedItem>()
        val job = launch { source.externalFeed.collect { items += it } }
        port.updates.emit(
            remoteUpdate(records = listOf(prompt(4uL, RemotePromptOriginFfi.SCHEDULED), prompt(5uL, RemotePromptOriginFfi.SCHEDULED))),
        )
        job.cancel()
        assertEquals(
            listOf("定时任务", "来自 Bot「Inbox triage」"),
            items.filterIsInstance<ExternalFeedItem.UserPrompt>().map { it.originLabel },
        )
    }

    @Test fun promptImagesAreListedForFetching() {
        val update = remoteUpdate(
            records = listOf(prompt(7uL, RemotePromptOriginFfi.DESKTOP, images = listOf(RemoteImageRefFfi("ref-1", "image/png")))),
        )
        assertEquals(listOf(7L to listOf("ref-1")), mapper.imageRefs(update))
        assertEquals(1, (mapper.map(update).first() as ExternalFeedItem.UserPrompt).imageCount)
    }

    // --- pending actions, outgoing, notices ------------------------------

    @Test fun outgoingPromptsReportFailure() {
        val items = mapper.map(
            remoteUpdate(
                outgoing = listOf(
                    RemoteOutgoingPromptFfi("a", "one", emptyList(), RemoteOutgoingStatusFfi.SENDING, null, null),
                    RemoteOutgoingPromptFfi("b", "two", emptyList(), RemoteOutgoingStatusFfi.FAILED, null, "x"),
                ),
            ),
        )
        val outgoing = items.filterIsInstance<ExternalFeedItem.Outgoing>().single().prompts
        assertEquals(listOf(false, true), outgoing.map { it.failed })
    }

    @Test fun omittedRecordsNoticeOnlyOnAppend() {
        val omitted = RemoteRecordFfi.Omitted("big", 10uL)
        assertTrue(mapper.map(remoteUpdate(records = listOf(omitted))).any { it is ExternalFeedItem.Notice })
        assertTrue(mapper.map(remoteUpdate(replace = true, records = listOf(omitted))).none { it is ExternalFeedItem.Notice })
    }

    @Test fun answeredElsewhereIsReported() {
        assertTrue(mapper.map(remoteUpdate(answeredElsewhere = true)).any { it is ExternalFeedItem.Notice })
    }

    @Test fun offlineAndClosedSessionsAreNotReady() {
        for (phase in listOf(RemoteSessionPhaseFfi.OFFLINE, RemoteSessionPhaseFfi.CLOSED, RemoteSessionPhaseFfi.ERROR)) {
            val readiness = mapper.map(remoteUpdate(phase = phase)).filterIsInstance<ExternalFeedItem.Readiness>().single()
            assertFalse(phase.name, readiness.ready)
        }
        val live = mapper.map(remoteUpdate()).filterIsInstance<ExternalFeedItem.Readiness>().single()
        assertTrue(live.ready)
        assertNull(live.statusLine)
    }

    private fun bypassRequest() = PermissionRequest(
        requestId = 9uL,
        kind = PermissionKindDto.BypassPermissionsMode,
        worker = null,
        owner = null,
        suppressAlwaysAllowRule = false,
        autoModePrompt = null,
    )

    private fun toolRequest() = PermissionRequest(
        requestId = 3uL,
        kind = PermissionKindDto.ToolUseConfirm("Bash", "{}", false),
        worker = null,
        owner = null,
        suppressAlwaysAllowRule = false,
        autoModePrompt = null,
    )

    @Test fun permissionPromptNeverOffersAlwaysAllow() {
        val prompt = mapper.permissionPrompt(remoteUpdate(pending = RemotePendingFfi(listOf(toolRequest()), emptyList(), emptyList())))!!
        assertEquals(3uL, prompt.requestId)
        assertTrue(prompt.suppressAlwaysAllowRule)
        assertNull(prompt.autoModePrompt)
    }

    @Test fun aBypassRequestIsNotShownAndExplainsItself() {
        val update = remoteUpdate(pending = RemotePendingFfi(listOf(bypassRequest()), emptyList(), emptyList()))
        assertNull(mapper.permissionPrompt(update))
        val readiness = mapper.map(update).filterIsInstance<ExternalFeedItem.Readiness>().single()
        assertTrue(readiness.ready)
        assertEquals("要切换到“绕过权限”模式，只能在电脑上批准。", readiness.statusLine)
    }

    // --- the source ------------------------------------------------------

    @Test fun theFeedFollowsThePortAndPublishesMeta() = runTest(UnconfinedTestDispatcher()) {
        val port = FakeRemoteSessionPort()
        val source = RemoteConversationSource(port, "Pixel 9")
        val received = mutableListOf<ExternalFeedItem>()
        val job = launch { source.externalFeed.toList(received) }
        port.updates.emit(remoteUpdate(replace = true, messages = listOf(textMessage("user", "hi")), hasOlder = true))
        assertTrue(received.first() is ExternalFeedItem.Base)
        assertEquals(RemoteSessionPhaseFfi.LIVE, source.meta.value.phase)
        assertTrue(source.meta.value.hasOlder)
        job.cancel()
    }

    @Test fun promptImagesAreFetchedOnceAndDeliveredWithTheirTurn() = runTest(UnconfinedTestDispatcher()) {
        val port = FakeRemoteSessionPort()
        val source = RemoteConversationSource(port, "Pixel 9")
        val received = mutableListOf<ExternalFeedItem>()
        val job = launch { source.externalFeed.toList(received) }
        val record = prompt(4uL, RemotePromptOriginFfi.DESKTOP, images = listOf(RemoteImageRefFfi("ref", "image/png")))
        port.updates.emit(remoteUpdate(records = listOf(record)))
        port.updates.emit(remoteUpdate(replace = true, records = listOf(record)))
        val images = received.filterIsInstance<ExternalFeedItem.PromptImages>()
        assertEquals(2, images.size)
        assertEquals(4L, images.first().turnId)
        assertEquals(ImageRefDto("image/png", Base64.getEncoder().encodeToString(byteArrayOf(1, 2, 3))), images.first().images.single())
        assertEquals(1, port.calls.count { it == "fetch:ref" })
        job.cancel()
    }

    @Test fun sendingForwardsTextAndDecodedImages() = runTest {
        val port = FakeRemoteSessionPort()
        val source = RemoteConversationSource(port, "Pixel 9")
        source.sendExternalPrompt("look", listOf(ImageRefDto("image/jpeg", Base64.getEncoder().encodeToString(byteArrayOf(9, 8)))))
        assertEquals(listOf("send:look"), port.calls)
        val image = port.sentImages.single().single()
        assertEquals("image/jpeg", image.mediaType)
        assertEquals(listOf<Byte>(9, 8), image.bytes.toList())
    }

    @Test fun sendingRejectsMoreThanFourImagesOrOneOverTwoMiB() = runTest {
        val port = FakeRemoteSessionPort()
        val source = RemoteConversationSource(port, "Pixel 9")
        val small = ImageRefDto("image/png", Base64.getEncoder().encodeToString(byteArrayOf(1)))
        try {
            source.sendExternalPrompt("x", List(5) { small })
            fail("five images")
        } catch (_: IllegalArgumentException) {}
        val big = ImageRefDto("image/png", Base64.getEncoder().encodeToString(ByteArray(REMOTE_MAX_PROMPT_IMAGE_BYTES + 1)))
        try {
            source.sendExternalPrompt("x", listOf(big))
            fail("big image")
        } catch (_: IllegalArgumentException) {}
        assertTrue(port.calls.isEmpty())
        source.sendExternalPrompt("x", List(4) { small })
        assertEquals(listOf("send:x"), port.calls)
    }

    @Test fun onlyAllowOnceAndDenyReachTheDesktop() = runTest {
        val port = FakeRemoteSessionPort()
        val source = RemoteConversationSource(port, "Pixel 9")
        source.approvePermission(5uL, PermissionResponseDto.ALLOW_ONCE)
        source.denyPermission(6uL)
        for (response in PermissionResponseDto.values().filter { it != PermissionResponseDto.ALLOW_ONCE }) {
            try {
                source.approvePermission(7uL, response)
                fail(response.name)
            } catch (_: IllegalArgumentException) {}
        }
        assertEquals(listOf("permission:5:true", "permission:6:false"), port.calls)
    }

    @Test fun questionAnswersAndCancelAreForwarded() = runTest {
        val port = FakeRemoteSessionPort()
        val source = RemoteConversationSource(port, "Pixel 9")
        source.submitClientCommand(
            ClientCommand.AnswerAskUserQuestion(
                2uL,
                AskUserQuestionAnswersDto(listOf(AskUserQuestionAnswerDto("Which?".toClientTextDto(), "A".toClientTextDto()))),
            ),
        )
        source.submitClientCommand(ClientCommand.CancelAskUserQuestion(3uL))
        assertEquals(listOf("answer:2:{Which?=A}", "cancelQuestion:3"), port.calls)
    }

    @Test fun permissionModesNeverIncludeBypass() = runTest {
        assertEquals(RemotePermissionModeFfi.ACCEPT_EDITS, remotePermissionMode("acceptEdits"))
        assertEquals(RemotePermissionModeFfi.DONT_ASK, remotePermissionMode("dontAsk"))
        assertNull(remotePermissionMode("bypassPermissions"))
        assertNull(remotePermissionMode("anything"))
        val port = FakeRemoteSessionPort()
        val source = RemoteConversationSource(port, "Pixel 9")
        try {
            source.setPermissionMode("bypassPermissions")
            fail("bypass")
        } catch (_: IllegalArgumentException) {}
        source.setPermissionMode("plan")
        assertEquals(listOf("mode:PLAN"), port.calls)
    }

    @Test fun cancelModelAndPagingGoToThePort() = runTest {
        val port = FakeRemoteSessionPort()
        val source = RemoteConversationSource(port, "Pixel 9")
        source.cancel()
        source.setModel("m")
        source.loadOlder()
        source.setForeground(true)
        source.dismissExternalPrompt("p")
        assertEquals(listOf("cancel", "model:m", "loadOlder", "foreground:true", "dismiss:p"), port.calls)
    }
    @Test fun snapshotHistoryAndCompletedMessageImagesAreHydrated() = runTest(UnconfinedTestDispatcher()) {
        val port = FakeRemoteSessionPort()
        val source = RemoteConversationSource(port, "Pixel 9")
        val received = mutableListOf<ExternalFeedItem>()
        val job = launch { source.externalFeed.toList(received) }
        fun message(ref: String) = textMessage("user", "picture").copy(
            images = listOf(MessageImageDto(mediaType = "image/png", url = ref)),
        )
        port.updates.emit(remoteUpdate(replace = true, messages = listOf(message("s1:m:0:0"))))
        port.updates.emit(remoteUpdate(older = listOf(message("s1:m:1:0"))))
        port.updates.emit(remoteUpdate(records = listOf(RemoteRecordFfi.Event(
            ClientEvent.MessageComplete(stopReason = "end_turn", message = message("s1:e:2:0")), false,
        ))))
        val expected = listOf(ImageRefDto("image/png", "AQID"))
        assertEquals(expected, received.filterIsInstance<ExternalFeedItem.Base>().single().session.transcript.single().images)
        assertEquals(expected, received.filterIsInstance<ExternalFeedItem.OlderMessages>().single().messages.single().images)
        val completed = received.filterIsInstance<ExternalFeedItem.Event>().single().event as ClientEvent.MessageComplete
        assertEquals("data:image/png;base64,AQID", completed.message!!.images.single().url)
        assertEquals(3, port.calls.count { it.startsWith("fetch:") })
        job.cancel()
    }

    @Test fun imageFetchFromBeforeReloadCannotRefillTheCacheOrUpdateRows() = runTest(UnconfinedTestDispatcher()) {
        val port = FakeRemoteSessionPort()
        val gate = CompletableDeferred<Unit>()
        port.fetchGate = gate
        val source = RemoteConversationSource(port, "Pixel 9")
        val received = mutableListOf<ExternalFeedItem>()
        val job = launch { source.externalFeed.toList(received) }
        val record = prompt(4uL, RemotePromptOriginFfi.DESKTOP, images = listOf(RemoteImageRefFfi("ref", "image/png")))
        port.updates.emit(remoteUpdate(records = listOf(record)))
        port.updates.emit(remoteUpdate(phase = RemoteSessionPhaseFfi.LOADING))
        port.fetchGate = null
        gate.complete(Unit)
        assertTrue(received.none { it is ExternalFeedItem.PromptImages })
        port.updates.emit(remoteUpdate(replace = true, records = listOf(record)))
        assertEquals(1, received.filterIsInstance<ExternalFeedItem.PromptImages>().size)
        assertEquals(2, port.calls.count { it == "fetch:ref" })
        job.cancel()
    }

}
