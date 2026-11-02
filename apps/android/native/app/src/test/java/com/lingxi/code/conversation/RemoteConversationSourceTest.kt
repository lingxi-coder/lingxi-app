package com.lingxi.code.conversation

import com.lingxi.code.bindings.android.RemoteImageRefFfi
import com.lingxi.code.bindings.android.RemoteOutgoingStatusFfi
import com.lingxi.code.bindings.android.RemoteOutgoingPromptFfi
import com.lingxi.code.bindings.android.RemotePendingFfi
import com.lingxi.code.bindings.android.RemotePermissionModeFfi
import com.lingxi.code.bindings.android.RemotePromptOriginFfi
import com.lingxi.code.bindings.android.RemoteRecordFfi
import com.lingxi.code.bindings.android.RemoteSessionErrorFfi
import com.lingxi.code.bindings.android.RemoteSessionPhaseFfi
import com.lingxi.code.bindings.client.AccessTierDto
import com.lingxi.code.bindings.client.AskUserQuestionAnswerDto
import com.lingxi.code.bindings.client.AskUserQuestionAnswersDto
import com.lingxi.code.bindings.client.AskUserQuestionRequestDto
import com.lingxi.code.bindings.client.ClientCommand
import com.lingxi.code.bindings.client.ClientEvent
import com.lingxi.code.bindings.client.ComputerAccessRequestDto
import com.lingxi.code.bindings.client.ConversationControlsDto
import com.lingxi.code.bindings.client.ImageRefDto
import com.lingxi.code.bindings.client.MessageImageDto
import com.lingxi.code.bindings.client.PermissionKindDto
import com.lingxi.code.bindings.client.PermissionRequest
import com.lingxi.code.bindings.client.PermissionResponseDto
import com.lingxi.code.bindings.client.PermissionControlStateDto
import com.lingxi.code.bindings.client.ReasoningControlSpecDto
import com.lingxi.code.bindings.client.ReasoningControlStateDto
import com.lingxi.code.bindings.client.ReasoningSelectionDto
import java.util.Base64
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.flow.toList
import kotlinx.coroutines.launch
import kotlinx.coroutines.test.TestScope
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

    private suspend fun TestScope.startLive(source: RemoteConversationSource, port: FakeRemoteSessionPort) {
        backgroundScope.launch(UnconfinedTestDispatcher(testScheduler)) { source.externalFeed.collect {} }
        port.updates.emit(remoteUpdate())
    }

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

    @Test fun snapshotPreservesRemoteChatMode() {
        val snapshot = mapper.map(remoteUpdate(replace = true, sessionMode = com.lingxi.code.bindings.client.SessionModeDto.CHAT))
        assertEquals(com.lingxi.code.model.SessionMode.Chat, snapshot.filterIsInstance<ExternalFeedItem.Base>().single().session.mode)
    }

    @Test fun unknownModeWaitsAndReplaysTheOriginalSnapshotAndEvents() {
        val early = mapper.map(remoteUpdate(replace = true, sessionMode = null,
            messages = listOf(textMessage("user", "kept")),
            records = listOf(RemoteRecordFfi.Event(ClientEvent.TurnStarted(1uL), false))))
        assertTrue(early.none { it is ExternalFeedItem.Base || it is ExternalFeedItem.Event })
        assertFalse(early.filterIsInstance<ExternalFeedItem.Readiness>().single().ready)

        val recovered = mapper.map(remoteUpdate(sessionMode = com.lingxi.code.bindings.client.SessionModeDto.CHAT))
        val base = recovered.filterIsInstance<ExternalFeedItem.Base>().single()
        assertEquals(com.lingxi.code.model.SessionMode.Chat, base.session.mode)
        assertEquals(listOf("kept"), base.session.transcript.map { it.text })
        assertTrue(recovered.any { it is ExternalFeedItem.Event && it.event is ClientEvent.TurnStarted })
        assertEquals(2, mapper.takeAcceptedUpdates().size)
    }

    @Test fun lifecycleModeConfirmationNeverReplacesBufferedBaseOrPrompt() {
        mapper.map(remoteUpdate(replace = true, sessionMode = null,
            messages = listOf(textMessage("user", "saved"))))
        mapper.map(remoteUpdate(sessionMode = null,
            records = listOf(prompt(7uL, RemotePromptOriginFfi.DESKTOP))))
        val replayed = mapper.map(remoteUpdate(sessionMode = null, records = listOf(
            RemoteRecordFfi.Event(ClientEvent.SessionStarted("s1", com.lingxi.code.bindings.client.SessionModeDto.CHAT), false),
            RemoteRecordFfi.Event(ClientEvent.SessionResumed("other", com.lingxi.code.bindings.client.SessionModeDto.CODE, emptyList()), false),
        )))

        assertEquals("saved", replayed.filterIsInstance<ExternalFeedItem.Base>().single().session.transcript.single().text)
        assertEquals("hello 7", replayed.filterIsInstance<ExternalFeedItem.UserPrompt>().single().text)
        assertTrue(replayed.none { it is ExternalFeedItem.Event &&
            (it.event is ClientEvent.SessionStarted || it.event is ClientEvent.SessionResumed) })
        assertEquals(com.lingxi.code.model.SessionMode.Chat, mapper.ownerMode)
    }

    @Test fun missingLaterModeKeepsOwnerAndConflictingModeIsRejected() {
        mapper.map(remoteUpdate(replace = true, sessionMode = com.lingxi.code.bindings.client.SessionModeDto.CHAT))
        val later = mapper.map(remoteUpdate(replace = true, sessionMode = null))
        assertEquals(com.lingxi.code.model.SessionMode.Chat, later.filterIsInstance<ExternalFeedItem.Base>().single().session.mode)
        val conflict = mapper.map(remoteUpdate(replace = true, sessionMode = com.lingxi.code.bindings.client.SessionModeDto.CODE))
        assertTrue(conflict.none { it is ExternalFeedItem.Base })
        assertFalse(conflict.filterIsInstance<ExternalFeedItem.Readiness>().single().ready)
        assertEquals(com.lingxi.code.model.SessionMode.Chat, mapper.ownerMode)
        val afterConflict = mapper.map(remoteUpdate(replace = true, sessionMode = com.lingxi.code.bindings.client.SessionModeDto.CHAT))
        assertTrue(afterConflict.none { it is ExternalFeedItem.Base })
        assertEquals("SESSION_MODE_MISMATCH", mapper.failure?.code)
    }

    @Test fun overflowingUnknownModeRequiresANewClient() {
        repeat(129) { mapper.map(remoteUpdate(sessionMode = null)) }
        val modeWithoutSnapshot = mapper.map(remoteUpdate(sessionMode = com.lingxi.code.bindings.client.SessionModeDto.CHAT))
        assertFalse(modeWithoutSnapshot.filterIsInstance<ExternalFeedItem.Readiness>().single().ready)
        assertTrue(mapper.takeAcceptedUpdates().isEmpty())

        val replacement = mapper.map(remoteUpdate(replace = true, sessionMode = com.lingxi.code.bindings.client.SessionModeDto.CHAT,
            messages = listOf(textMessage("user", "complete snapshot"))))
        assertTrue(replacement.none { it is ExternalFeedItem.Base })
        assertEquals("SESSION_MODE_OVERFLOW", mapper.failure?.code)
        assertTrue(mapper.takeAcceptedUpdates().isEmpty())
    }

    @Test fun oversizedUnknownSnapshotRequiresANewClient() {
        val oversized = textMessage("x".repeat(1_500_000), "body")
        val blocked = mapper.map(remoteUpdate(replace = true, sessionMode = null, messages = listOf(oversized)))
        assertFalse(blocked.filterIsInstance<ExternalFeedItem.Readiness>().single().ready)
        assertTrue(mapper.takeAcceptedUpdates().isEmpty())
        val stillBlocked = mapper.map(remoteUpdate(sessionMode = com.lingxi.code.bindings.client.SessionModeDto.CHAT))
        assertTrue(stillBlocked.none { it is ExternalFeedItem.Base })
        val replacement = mapper.map(remoteUpdate(replace = true, sessionMode = com.lingxi.code.bindings.client.SessionModeDto.CHAT))
        assertTrue(replacement.none { it is ExternalFeedItem.Base })
        assertEquals("SESSION_MODE_OVERFLOW", mapper.failure?.code)
    }

    @Test fun sharedClientOverflowImmediatelyClosesTheUnconfirmedOwner() = runTest(UnconfinedTestDispatcher()) {
        val port = FakeRemoteSessionPort()
        val source = RemoteConversationSource(port, "s1", "Pixel 9")
        val received = mutableListOf<ExternalFeedItem>()
        val job = launch { source.externalFeed.toList(received) }
        port.updates.emit(remoteUpdate(sessionMode = null))
        port.updates.emit(remoteUpdate(sessionMode = null, phase = RemoteSessionPhaseFfi.ERROR).copy(
            error = RemoteSessionErrorFfi("SESSION_MODE_OVERFLOW", "too much held data")))
        assertEquals(RemoteSessionPhaseFfi.ERROR, source.meta.value.phase)
        assertEquals("SESSION_MODE_OVERFLOW", source.meta.value.error?.code)
        port.updates.emit(remoteUpdate(replace = true))
        assertEquals(RemoteSessionPhaseFfi.ERROR, source.meta.value.phase)
        assertTrue(received.none { it is ExternalFeedItem.Base })
        try { source.sendExternalPrompt("draft", emptyList()); fail("overflowed owner") }
        catch (_: IllegalStateException) {}
        assertTrue(port.calls.isEmpty())
        job.cancel()
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
        val source = RemoteConversationSource(port, "s1", "Pixel 9")
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

    @Test fun teamApprovalRequiresAnExactAsciiBase64UrlNonce() {
        val valid = "team-approval:ABCDEFGHIJKLMNOPQRSTUVWXYZabcdef"
        assertTrue(isTeamApprovalSessionId(valid))
        assertTrue(isTeamApprovalSessionId("team-approval:0123456789_-abcdefghijklmnopqrst"))
        for (invalid in listOf(
            "team-approval:short",
            "team-approval:ABCDEFGHIJKLMNOPQRSTUVWXYZabcdef-",
            "team-approval:ABCDEFGHIJKLMNOPQRSTUVWXYZabcde!",
            "TEAM-approval:ABCDEFGHIJKLMNOPQRSTUVWXYZabcdef",
            "sess:$valid",
            "$valid\n",
            "team-approval:ABCDEFGHIJKLMNOPQRSTUVWXYZabcdeé",
        )) assertFalse(invalid, isTeamApprovalSessionId(invalid))
    }

    @Test fun teamApprovalPublishesOnlyLivePermissionAndRejectsOtherUpdates() = runTest(UnconfinedTestDispatcher()) {
        val id = "team-approval:ABCDEFGHIJKLMNOPQRSTUVWXYZabcdef"
        val port = FakeRemoteSessionPort()
        val source = RemoteConversationSource(port, id, "Pixel 9")
        val received = mutableListOf<ExternalFeedItem>()
        val job = launch { source.externalFeed.toList(received) }
        val update = remoteUpdate(sessionId = id, sessionMode = null,
            pending = RemotePendingFfi(listOf(toolRequest()), emptyList(), emptyList()))
        port.updates.emit(update)
        assertEquals(RemoteSessionPhaseFfi.LIVE, source.meta.value.phase)
        assertNull(source.meta.value.sessionMode)
        assertEquals(3uL, source.pendingPermission.value?.requestId)
        assertTrue(source.pendingPermission.value!!.suppressAlwaysAllowRule)
        assertNull(source.pendingPermission.value!!.autoModePrompt)
        assertTrue(received.all { it is ExternalFeedItem.Readiness && !it.ready })
        assertEquals(0, port.modelListRequests)

        port.updates.emit(update.copy(phase = RemoteSessionPhaseFfi.OFFLINE))
        assertNull(source.pendingPermission.value)
        port.updates.emit(update.copy(sessionMode = com.lingxi.code.bindings.client.SessionModeDto.CODE))
        assertEquals(RemoteSessionPhaseFfi.OFFLINE, source.meta.value.phase)
        assertNull(source.pendingPermission.value)
        port.updates.emit(update.copy(sessionId = "team-approval:0123456789_-abcdefghijklmnopqrst"))
        assertEquals(RemoteSessionPhaseFfi.OFFLINE, source.meta.value.phase)
        assertNull(source.pendingPermission.value)
        job.cancel()
    }

    @Test fun teamApprovalRejectsConversationControlsAndOtherPendingActions() = runTest(UnconfinedTestDispatcher()) {
        val id = "team-approval:ABCDEFGHIJKLMNOPQRSTUVWXYZabcdef"
        val port = FakeRemoteSessionPort()
        val source = RemoteConversationSource(port, id, "Pixel 9")
        val job = launch { source.externalFeed.collect {} }
        val pending = RemotePendingFfi(listOf(toolRequest()), emptyList(), emptyList())
        val valid = remoteUpdate(sessionId = id, sessionMode = null, pending = pending)
        val automatic = ReasoningSelectionDto.Automatic
        val controls = ConversationControlsDto("model", PermissionControlStateDto("default", "default", emptyList()),
            ReasoningControlStateDto(automatic, automatic,
                ReasoningControlSpecDto(emptyList(), null, automatic, false, true, null)), null)
        val invalid = listOf(
            valid.copy(controls = controls),
            valid.copy(outgoing = listOf(RemoteOutgoingPromptFfi("id", "text", emptyList(), RemoteOutgoingStatusFfi.SENDING, null, null))),
            valid.copy(pending = pending.copy(questions = listOf(AskUserQuestionRequestDto(1uL, emptyList(), null)))),
            valid.copy(pending = pending.copy(computerAccess = listOf(
                ComputerAccessRequestDto(1uL, "reason", emptyList(), AccessTierDto.READ, false, false, false, null)))),
            valid.copy(turn = valid.turn.copy(running = true)),
            valid.copy(turn = valid.turn.copy(turnId = 1uL)),
            valid.copy(firstMessageIndex = 1uL),
            valid.copy(hasOlder = true),
            valid.copy(loadingOlder = true),
        )
        for (update in invalid) {
            port.updates.emit(valid)
            assertEquals(3uL, source.pendingPermission.value?.requestId)
            port.updates.emit(update)
            assertNull(source.pendingPermission.value)
            assertEquals(RemoteSessionPhaseFfi.OFFLINE, source.meta.value.phase)
        }
        job.cancel()
    }

    @Test fun teamApprovalOnlyForwardsAllowOnceAndDeny() = runTest {
        val port = FakeRemoteSessionPort()
        val id = "team-approval:ABCDEFGHIJKLMNOPQRSTUVWXYZabcdef"
        val source = RemoteConversationSource(port, id, "Pixel 9")
        val forbidden: List<suspend () -> Unit> = listOf(
            { source.sendExternalPrompt("do not send", emptyList()) },
            { source.retryExternalPrompt("p") },
            { source.dismissExternalPrompt("p") },
            { source.cancel() },
            { source.submitClientCommand(ClientCommand.CancelAskUserQuestion(1uL)) },
            { source.setModel("model") },
            { source.setReasoningSelection("auto") },
            { source.setPermissionMode("plan") },
            { source.loadOlder() },
        )
        forbidden.forEach { action ->
            try { action(); fail("conversation operation reached an approval capability") }
            catch (_: IllegalStateException) {}
        }
        try { source.approvePermission(3uL, PermissionResponseDto.ALLOW_ALWAYS); fail("allow always") }
        catch (_: IllegalArgumentException) {}
        try { source.approvePermission(3uL, PermissionResponseDto.ALLOW_ONCE); fail("not live") }
        catch (_: IllegalStateException) {}
        backgroundScope.launch(UnconfinedTestDispatcher(testScheduler)) { source.externalFeed.collect {} }
        port.updates.emit(remoteUpdate(sessionId = id, sessionMode = null,
            pending = RemotePendingFfi(listOf(toolRequest()), emptyList(), emptyList())))
        try { source.denyPermission(4uL); fail("wrong request") }
        catch (_: IllegalStateException) {}
        port.permissionFailure = IllegalStateException("temporarily offline")
        try { source.approvePermission(3uL, PermissionResponseDto.ALLOW_ONCE); fail("failed response") }
        catch (_: IllegalStateException) {}
        port.permissionFailure = null
        source.approvePermission(3uL, PermissionResponseDto.ALLOW_ONCE)
        try { source.denyPermission(3uL); fail("already answered") }
        catch (_: IllegalStateException) {}
        assertEquals(listOf("permission:3:true"), port.calls)
    }

    @Test fun teamApprovalMayDenyItsPendingRequestOnce() = runTest {
        val port = FakeRemoteSessionPort()
        val id = "team-approval:ABCDEFGHIJKLMNOPQRSTUVWXYZabcdef"
        val source = RemoteConversationSource(port, id, "Pixel 9")
        backgroundScope.launch(UnconfinedTestDispatcher(testScheduler)) { source.externalFeed.collect {} }
        port.updates.emit(remoteUpdate(sessionId = id, sessionMode = null,
            pending = RemotePendingFfi(listOf(toolRequest()), emptyList(), emptyList())))
        source.denyPermission(3uL)
        assertEquals(listOf("permission:3:false"), port.calls)
        assertNull(source.pendingPermission.value)
    }

    @Test fun aMalformedTeamApprovalNameStillWaitsForAnAuthoritativeMode() {
        val invalid = "team-approval:ABCDEFGHIJKLMNOPQRSTUVWXYZabcde!"
        val items = mapper.map(remoteUpdate(sessionId = invalid, sessionMode = null,
            pending = RemotePendingFfi(listOf(toolRequest()), emptyList(), emptyList())))
        assertTrue(items.all { it is ExternalFeedItem.Readiness && !it.ready })
        assertTrue(mapper.takeAcceptedUpdates().isEmpty())
    }

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
        val source = RemoteConversationSource(port, "s1", "Pixel 9")
        val received = mutableListOf<ExternalFeedItem>()
        val job = launch { source.externalFeed.toList(received) }
        port.updates.emit(remoteUpdate(replace = true, messages = listOf(textMessage("user", "hi")), hasOlder = true))
        assertTrue(received.first() is ExternalFeedItem.Base)
        assertEquals(RemoteSessionPhaseFfi.LIVE, source.meta.value.phase)
        assertTrue(source.meta.value.hasOlder)
        job.cancel()
    }

    @Test fun theFeedProjectsEachAcceptedScheduledPromptOnce() = runTest(UnconfinedTestDispatcher()) {
        val port = FakeRemoteSessionPort()
        port.botNames[4uL] = "Daily"
        val source = RemoteConversationSource(port, "s1", "Pixel 9")
        val received = mutableListOf<ExternalFeedItem>()
        val job = launch { source.externalFeed.toList(received) }
        port.updates.emit(remoteUpdate(records = listOf(prompt(4uL, RemotePromptOriginFfi.SCHEDULED))))
        assertEquals(listOf(4uL), port.botNameQueries)
        assertEquals("来自 Bot「Daily」", received.filterIsInstance<ExternalFeedItem.UserPrompt>().single().originLabel)
        job.cancel()
    }

    @Test fun offlinePermissionCannotBeAnsweredAndReturnsWhenLive() = runTest(UnconfinedTestDispatcher()) {
        val port = FakeRemoteSessionPort()
        val source = RemoteConversationSource(port, "s1", "Pixel 9")
        val job = launch { source.externalFeed.collect {} }
        val update = remoteUpdate(pending = RemotePendingFfi(listOf(toolRequest()), emptyList(), emptyList()))
        port.updates.emit(update)
        assertEquals(3uL, source.pendingPermission.value?.requestId)
        port.updates.emit(update.copy(phase = RemoteSessionPhaseFfi.OFFLINE))
        assertNull(source.pendingPermission.value)
        try { source.denyPermission(3uL); fail("offline") } catch (_: IllegalStateException) {}
        assertTrue(port.calls.isEmpty())
        port.updates.emit(update)
        assertEquals(3uL, source.pendingPermission.value?.requestId)
        job.cancel()
    }

    @Test fun rejectedModeCannotLeaveRemoteControlsOnline() = runTest(UnconfinedTestDispatcher()) {
        val port = FakeRemoteSessionPort()
        val source = RemoteConversationSource(port, "s1", "Pixel 9")
        val received = mutableListOf<ExternalFeedItem>()
        val job = launch { source.externalFeed.toList(received) }
        port.updates.emit(remoteUpdate(replace = true, sessionMode = com.lingxi.code.bindings.client.SessionModeDto.CHAT))
        assertEquals(RemoteSessionPhaseFfi.LIVE, source.meta.value.phase)
        port.updates.emit(remoteUpdate(replace = true, sessionMode = com.lingxi.code.bindings.client.SessionModeDto.CODE))
        assertEquals(RemoteSessionPhaseFfi.ERROR, source.meta.value.phase)
        assertEquals(com.lingxi.code.model.SessionMode.Chat, source.meta.value.sessionMode)
        assertEquals("SESSION_MODE_MISMATCH", source.meta.value.error?.code)
        port.updates.emit(remoteUpdate(replace = true, sessionMode = com.lingxi.code.bindings.client.SessionModeDto.CHAT))
        assertEquals(RemoteSessionPhaseFfi.ERROR, source.meta.value.phase)
        try { source.sendExternalPrompt("blocked", emptyList()); fail("conflicting owner") }
        catch (_: IllegalStateException) {}
        assertEquals(1, received.filterIsInstance<ExternalFeedItem.Base>().size)
        job.cancel()
    }

    @Test fun unconfirmedOwnerCannotReadImagesOrControlSession() = runTest(UnconfinedTestDispatcher()) {
        val port = FakeRemoteSessionPort()
        val source = RemoteConversationSource(port, "s1", "Pixel 9")
        val received = mutableListOf<ExternalFeedItem>()
        val job = launch { source.externalFeed.toList(received) }
        val image = textMessage("user", "picture").copy(images = listOf(MessageImageDto("image/png", "ref")))
        port.updates.emit(remoteUpdate(replace = true, sessionMode = null, messages = listOf(image)))
        assertEquals(RemoteSessionPhaseFfi.LOADING, source.meta.value.phase)
        assertTrue(port.calls.isEmpty())
        try { source.sendExternalPrompt("draft", emptyList()); fail("unconfirmed owner") }
        catch (_: IllegalStateException) {}
        try { source.loadOlder(); fail("unconfirmed owner") }
        catch (_: IllegalStateException) {}
        assertTrue(port.calls.isEmpty())
        assertTrue(received.none { it is ExternalFeedItem.Base })
        port.updates.emit(remoteUpdate(sessionMode = com.lingxi.code.bindings.client.SessionModeDto.CHAT))
        assertEquals(listOf("fetch:ref"), port.calls)
        assertEquals(listOf(ImageRefDto("image/png", "AQID")),
            received.filterIsInstance<ExternalFeedItem.Base>().single().session.transcript.single().images)
        job.cancel()
    }

    @Test fun promptImagesAreFetchedOnceAndDeliveredWithTheirTurn() = runTest(UnconfinedTestDispatcher()) {
        val port = FakeRemoteSessionPort()
        val source = RemoteConversationSource(port, "s1", "Pixel 9")
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
        val source = RemoteConversationSource(port, "s1", "Pixel 9")
        startLive(source, port)
        source.sendExternalPrompt("look", listOf(ImageRefDto("image/jpeg", Base64.getEncoder().encodeToString(byteArrayOf(9, 8)))))
        assertEquals(listOf("send:look"), port.calls)
        val image = port.sentImages.single().single()
        assertEquals("image/jpeg", image.mediaType)
        assertEquals(listOf<Byte>(9, 8), image.bytes.toList())
    }

    @Test fun sendingRejectsMoreThanFourImagesOrOneOverTwoMiB() = runTest {
        val port = FakeRemoteSessionPort()
        val source = RemoteConversationSource(port, "s1", "Pixel 9")
        startLive(source, port)
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
        val source = RemoteConversationSource(port, "s1", "Pixel 9")
        startLive(source, port)
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
        val source = RemoteConversationSource(port, "s1", "Pixel 9")
        startLive(source, port)
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
        val source = RemoteConversationSource(port, "s1", "Pixel 9")
        startLive(source, port)
        try {
            source.setPermissionMode("bypassPermissions")
            fail("bypass")
        } catch (_: IllegalArgumentException) {}
        source.setPermissionMode("plan")
        assertEquals(listOf("mode:PLAN"), port.calls)
    }

    @Test fun cancelModelAndPagingGoToThePort() = runTest {
        val port = FakeRemoteSessionPort()
        val source = RemoteConversationSource(port, "s1", "Pixel 9")
        startLive(source, port)
        source.cancel()
        source.setModel("m")
        source.loadOlder()
        source.setForeground(true)
        source.dismissExternalPrompt("p")
        assertEquals(listOf("cancel", "model:m", "loadOlder", "foreground:true", "dismiss:p"), port.calls)
    }
    @Test fun snapshotHistoryAndCompletedMessageImagesAreHydrated() = runTest(UnconfinedTestDispatcher()) {
        val port = FakeRemoteSessionPort()
        val source = RemoteConversationSource(port, "s1", "Pixel 9")
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

    @Test fun slowSnapshotImageDoesNotDelayModeConflictOrPublishStaleBase() = runTest(UnconfinedTestDispatcher()) {
        val port = FakeRemoteSessionPort()
        val gate = CompletableDeferred<Unit>()
        port.fetchGate = gate
        val source = RemoteConversationSource(port, "s1", "Pixel 9")
        val received = mutableListOf<ExternalFeedItem>()
        val job = launch { source.externalFeed.toList(received) }
        val image = textMessage("user", "picture").copy(images = listOf(MessageImageDto("image/png", "ref")))
        port.updates.emit(remoteUpdate(replace = true, messages = listOf(image)))
        assertTrue(port.calls.contains("fetch:ref"))
        assertTrue(received.none { it is ExternalFeedItem.Base })

        port.updates.emit(remoteUpdate(replace = true, sessionMode = com.lingxi.code.bindings.client.SessionModeDto.CHAT))
        assertEquals(RemoteSessionPhaseFfi.ERROR, source.meta.value.phase)
        assertEquals("SESSION_MODE_MISMATCH", source.meta.value.error?.code)
        assertTrue(received.last() is ExternalFeedItem.Readiness && !(received.last() as ExternalFeedItem.Readiness).ready)
        gate.complete(Unit)
        assertTrue(received.none { it is ExternalFeedItem.Base })
        job.cancel()
    }

    @Test fun projectionWaitsForEarlierImageBeforePublishingLaterRecords() = runTest(UnconfinedTestDispatcher()) {
        val port = FakeRemoteSessionPort()
        val gate = CompletableDeferred<Unit>()
        port.fetchGate = gate
        val source = RemoteConversationSource(port, "s1", "Pixel 9")
        val received = mutableListOf<ExternalFeedItem>()
        val job = launch { source.externalFeed.toList(received) }
        val image = textMessage("user", "first").copy(images = listOf(MessageImageDto("image/png", "ref")))
        port.updates.emit(remoteUpdate(replace = true, messages = listOf(image)))
        port.updates.emit(remoteUpdate(records = listOf(prompt(7uL, RemotePromptOriginFfi.DESKTOP))))
        assertTrue(received.none { it is ExternalFeedItem.Base || it is ExternalFeedItem.UserPrompt })
        gate.complete(Unit)
        val base = received.indexOfFirst { it is ExternalFeedItem.Base }
        val prompt = received.indexOfFirst { it is ExternalFeedItem.UserPrompt }
        assertTrue(base >= 0 && prompt > base)
        job.cancel()
    }

    @Test fun stalledProjectionQueueFailsClosedAtItsBound() = runTest(UnconfinedTestDispatcher()) {
        val port = FakeRemoteSessionPort()
        val gate = CompletableDeferred<Unit>()
        port.fetchGate = gate
        val source = RemoteConversationSource(port, "s1", "Pixel 9")
        val received = mutableListOf<ExternalFeedItem>()
        val job = launch { source.externalFeed.toList(received) }
        val image = textMessage("user", "first").copy(images = listOf(MessageImageDto("image/png", "ref")))
        port.updates.emit(remoteUpdate(replace = true, messages = listOf(image)))
        repeat(128) { index ->
            port.updates.emit(remoteUpdate(records = listOf(prompt(index.toULong(), RemotePromptOriginFfi.DESKTOP))))
        }
        assertEquals(RemoteSessionPhaseFfi.ERROR, source.meta.value.phase)
        assertEquals("SESSION_PROJECTION_OVERFLOW", source.meta.value.error?.code)
        gate.complete(Unit)
        assertTrue(received.none { it is ExternalFeedItem.Base || it is ExternalFeedItem.UserPrompt })
        job.cancel()
    }

    @Test fun imageFetchFromBeforeReloadCannotRefillTheCacheOrUpdateRows() = runTest(UnconfinedTestDispatcher()) {
        val port = FakeRemoteSessionPort()
        val gate = CompletableDeferred<Unit>()
        port.fetchGate = gate
        val source = RemoteConversationSource(port, "s1", "Pixel 9")
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

    @Test fun imageFetchFromBeforeModeConflictCannotRefillTheCacheOrUpdateRows() = runTest(UnconfinedTestDispatcher()) {
        for (phase in listOf(RemoteSessionPhaseFfi.LIVE, RemoteSessionPhaseFfi.ERROR)) {
            val port = FakeRemoteSessionPort()
            val gate = CompletableDeferred<Unit>()
            port.fetchGate = gate
            val source = RemoteConversationSource(port, "s1", "Pixel 9")
            val received = mutableListOf<ExternalFeedItem>()
            val job = launch { source.externalFeed.toList(received) }
            val record = prompt(4uL, RemotePromptOriginFfi.DESKTOP, images = listOf(RemoteImageRefFfi("ref", "image/png")))
            port.updates.emit(remoteUpdate(records = listOf(record)))
            assertTrue(port.calls.contains("fetch:ref"))
            port.updates.emit(remoteUpdate(phase = phase,
                sessionMode = com.lingxi.code.bindings.client.SessionModeDto.CHAT))
            assertEquals(RemoteSessionPhaseFfi.ERROR, source.meta.value.phase)
            port.fetchGate = null
            gate.complete(Unit)
            assertTrue(received.none { it is ExternalFeedItem.PromptImages })
            // A conflicting client cannot become live again or fetch more images.
            port.updates.emit(remoteUpdate(records = listOf(record)))
            assertEquals(1, port.calls.count { it == "fetch:ref" })
            assertTrue(received.none { it is ExternalFeedItem.PromptImages })
            job.cancel()
        }
    }

}
