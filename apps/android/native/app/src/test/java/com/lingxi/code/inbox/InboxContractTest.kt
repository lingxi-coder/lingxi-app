package com.lingxi.code.inbox

import org.json.JSONArray
import org.json.JSONObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import java.nio.file.Files
import java.util.UUID

class InboxContractTest {
    private val team = InboxRef("team", "pair_42", "chat", 4)
    private val assistant = InboxRef("assistant", "host", "session", 0)

    @Test fun friendUsesTeamKeyAndCanonicalParserRejectsMismatchedProjection() {
        val row = InboxItem(team, "friend", "Friend", peerUid = "peer")
        assertEquals(row, InboxItem.parse(row.json()))
        assertEquals("team:pair_42:chat", row.key)
        assertNull(InboxItem.parse(row.json().put("key", "friend:pair_42:chat")))
        assertNull(InboxRef.fromKey("team:pair_42:../chat", 4))
    }

    @Test fun frozenReadCannotClearALaterManualUnreadOrResolveAnAction() {
        val item = InboxItem(team, "team", "Channel", sourceCursor = 50, readThrough = 5,
            manualUnreadRevision = 3, unreadCount = 12, pendingActionCount = 1)
        val frozen = InboxOperation.read(item, 25)
        val markedUnread = InboxOperation.unread(item).apply(item)
        val result = frozen.apply(markedUnread.copy(sourceCursor = 53))
        assertEquals(25L, result.readThrough)
        assertEquals(4L, result.manualUnreadRevision)
        assertTrue(result.manualUnread)
        assertTrue(result.attention)
        val cleared = InboxOperation.read(result, 53).apply(result)
        assertFalse(cleared.unread)
        assertTrue(cleared.attention)
        assertEquals(4L, cleared.manualUnreadRevision)
        assertEquals(53L, frozen.apply(cleared).readThrough)
        assertEquals(cleared, frozen.apply(cleared.copy(ref = team.copy(generation = 5))).copy(ref = team))
    }

    @Test fun consecutiveReadCoalescesOnlyWithIdenticalObservedRevisions() {
        val first = InboxOperation.read(team, 21, 1, 7)
        assertNull(first.coalesce(InboxOperation.read(team, 20, 2, 7)))
        assertNull(first.coalesce(InboxOperation.read(team, 20, 1, 8)))
        val second = InboxOperation.read(team, 20, 1, 7)
        val merged = requireNotNull(first.coalesce(second))
        assertEquals(21L, merged.body.getLong("through"))
        assertEquals(1L, merged.body.getLong("manualUnreadRevision"))
        assertEquals(7L, merged.body.getLong("observedPendingActionRevision"))
        assertEquals(second.admission, merged.admission)
    }

    @Test fun twoDistinctManualMarksStayOrderedBeforeTheirFrozenRead() {
        val row = InboxItem(team, "team", "Channel", sourceCursor = 71, unreadCount = 1)
        val first = InboxOperation.unread(row)
        val afterFirst = first.apply(row)
        val second = InboxOperation.unread(afterFirst)
        assertNull(first.coalesce(second))
        val afterSecond = second.apply(afterFirst)
        val frozen = InboxOperation.read(afterSecond)
        assertEquals(2L, frozen.body.getLong("manualUnreadRevision"))
        assertTrue(frozen.apply(afterFirst).manualUnread)
        assertFalse(frozen.apply(afterSecond).manualUnread)
    }

    @Test fun preferencePatchesMergeAndPreserveExplicitNull() {
        val item = InboxItem(team, "friend", "Title", pinOrder = 123)
        val mute = InboxOperation.preferences(item, JSONObject().put("notificationMode", "none"))
        val unpin = InboxOperation.preferences(item, JSONObject().put("pinOrder", JSONObject.NULL))
        val output = requireNotNull(mute.coalesce(unpin)).apply(item)
        assertEquals("none", output.notificationMode)
        assertNull(output.pinOrder)
    }

    @Test fun sourceCursorSkipsNothingBetweenVisibleMessages() {
        assertEquals(9L, contiguousVisibleRead(9, listOf(12, 13)))
        assertEquals(11L, contiguousVisibleRead(9, listOf(11, 10, 11, 13)))
        assertEquals(9L, contiguousVisibleRead(9, listOf(7, 8, 9, 30)))
    }

    @Test fun assistantBaselineCountsOnlyAfterBarrierAndDispatchesNoActionClear() {
        val row = InboxItem(assistant, "assistant", "Build", baselineCursor = 100, readThrough = 100,
            sourceCursor = 103, unreadCount = 3, pendingActionCount = 2)
        val partial = InboxOperation.read(row, 101).apply(row)
        assertEquals(2, partial.unreadCount)
        assertTrue(partial.matches(InboxQuery(InboxFilter.Attention, "assistant", "build")))
        val latest = InboxOperation.read(partial).apply(partial)
        assertFalse(latest.matches(InboxQuery(InboxFilter.Unread)))
        assertTrue(latest.matches(InboxQuery(InboxFilter.Attention)))
        assertFalse(latest.copy(archived = true).needsBadge)
    }

    @Test fun mutedArchivedConversationStaysArchivedWhileUnreadOrAttentionMakesItDiscoverable() {
        val row = InboxItem(assistant, "assistant", "Build", sourceCursor = 2, readThrough = 1,
            unreadCount = 1, pendingActionCount = 1, archived = true, notificationMode = "none")

        assertFalse(row.matches(InboxQuery(InboxFilter.All)))
        assertTrue(row.matches(InboxQuery(InboxFilter.Archived)))
        assertTrue(row.matches(InboxQuery(InboxFilter.Unread, "assistant", "build")))
        assertTrue(row.matches(InboxQuery(InboxFilter.Attention, "assistant", "build")))
        assertFalse(row.needsBadge)

        val read = InboxOperation.read(row).apply(row)
        assertFalse(read.matches(InboxQuery(InboxFilter.Unread)))
        assertTrue(read.matches(InboxQuery(InboxFilter.Attention)))
        assertTrue(read.matches(InboxQuery(InboxFilter.Archived)))
        assertEquals("none", read.notificationMode)
        assertFalse(read.copy(pendingActionCount = 0).matches(InboxQuery(InboxFilter.Attention)))
    }

    @Test fun globalReadFreezesArchivedAssistantUnreadAndLeavesNewActivityUnseen() {
        val archived = InboxItem(assistant, "assistant", "Build", sourceCursor = 2, readThrough = 1,
            unreadCount = 1, pendingActionCount = 1, pendingActionRevision = 3, archived = true, notificationMode = "none")
        val alreadyRead = InboxItem(assistant.copy(conversationId = "read"), "assistant", "Read", sourceCursor = 1, readThrough = 1)
        val (frozenRow, operation) = freezeLocalAssistantReads(listOf(archived, alreadyRead)).single()
        assertEquals(archived, frozenRow)
        assertEquals(2L, operation.body.getLong("through"))
        assertEquals(3L, operation.body.getLong("observedPendingActionRevision"))
        assertFalse(operation.apply(archived).unread)

        val result = operation.apply(archived.copy(sourceCursor = 3, unreadCount = 2, pendingActionRevision = 4))
        assertTrue(result.unread)
        assertTrue(result.archived)
        assertEquals("none", result.notificationMode)
        assertEquals(1, result.pendingActionCount)
        assertEquals(4L, result.pendingActionRevision)
    }

    @Test fun runtimeOrdinalsRequireVisibleFinalIdentityWithoutSkippingUnseenMiddle() {
        val home = Files.createTempDirectory("assistant-inbox-contract").toFile()
        try {
            val session = UUID.randomUUID().toString()
            val indexed = home.resolve("inbox-activity/$session")
            val entries = indexed.resolve("entries").apply { mkdirs() }
            indexed.resolve("summary.json").writeText(JSONObject().put("sourceId", "host").put("cursor", 7)
                .put("baselineCursor", 4).put("lastMeaningfulAt", 1234).put("previewKind", "reply")
                .put("sourceRevision", 7).put("latestResultUuids", JSONArray(listOf("head"))).toString())
            for ((cursor, ids) in listOf(5 to listOf("draft", "final"), 6 to listOf("middle"), 7 to listOf("head"))) {
                entries.resolve("$cursor.json").writeText(JSONObject().put("cursor", cursor).put("resultUuids", JSONArray(ids)).toString())
            }
            assertEquals("head", AssistantActivity.parse(LocalAssistantActivity.summary(home, session))?.headMessageId)
            assertEquals(4L, LocalAssistantActivity.through(home, session, "host", 4, 7, setOf("draft", "head")))
            assertEquals(5L, LocalAssistantActivity.through(home, session, "host", 4, 7, setOf("final", "head")))
            assertEquals(7L, LocalAssistantActivity.through(home, session, "host", 4, 7, setOf("final", "middle", "head")))
            assertEquals(4L, LocalAssistantActivity.through(home, session, "other-source", 4, 7, setOf("final", "middle", "head")))
        } finally {
            home.deleteRecursively()
        }
    }

    @Test fun cloudLeaseExpiresAtFiveMinutesAndRejectsBackwardsClock() {
        val verifiedAt = 10_000L
        assertTrue(isCloudInboxLeaseFresh(verifiedAt, verifiedAt + CLOUD_INBOX_AUTHORIZATION_MS - 1))
        assertFalse(isCloudInboxLeaseFresh(verifiedAt, verifiedAt + CLOUD_INBOX_AUTHORIZATION_MS))
        assertFalse(isCloudInboxLeaseFresh(verifiedAt, verifiedAt - 1))
        assertFalse(isCloudInboxLeaseFresh(0, verifiedAt))
    }

    @Test fun consentRestoresWithAnOptOutRevisionThatSupersedesAnUnconfirmedEnable() {
        val enabling = AssistantConsent().change(true)
        val restored = AssistantConsent.load(enabling.json().toString(), null)
        val disabling = restored.change(false)
        assertFalse(disabling.enabled)
        assertTrue(disabling.revision > enabling.revision)
        assertEquals(disabling, AssistantConsent.load(disabling.json().toString(), "true").change(false))
    }

    @Test fun prefixAndSearchCursorShareTheWireNormalization() {
        val row = InboxItem(team, "team", "ＢＵＩＬＤ notes")
        assertTrue(row.matches(InboxQuery(search = " ｂｕｉ ")))
        assertFalse(row.matches(InboxQuery(search = "notes")))
        val after = InboxCursor(18, row.key, titleSort = inboxTitleSort(row.title)).json()
        assertEquals("build notes", after.getString("titleSort"))
        assertEquals(row.key, after.getString("key"))
    }

    @Test fun pendingApprovalHasSeparateClockAndFrozenReadObservation() {
        val raw = JSONObject().put("sourceId", "host").put("cursor", 0)
            .put("baselineCursor", 0).put("lastMeaningfulAt", 0).put("sourceRevision", 1)
            .put("pendingActionKind", "approval").put("pendingActionAt", 456).put("pendingActionCount", 1)
            .put("pendingActionRevision", 3).toString()
        val activity = requireNotNull(AssistantActivity.parse(raw))
        assertEquals("approval", activity.pendingActionKind)
        assertEquals(456L, activity.pendingActionAt)
        assertEquals(0L, activity.at)
        val row = InboxItem(assistant, "assistant", "Assistant", pendingActionCount = 1, pendingActionRevision = activity.pendingActionRevision)
        val read = InboxOperation.read(row)
        assertEquals(3L, read.body.getLong("observedPendingActionRevision"))
        assertTrue(read.apply(row).attention)
    }

    @Test fun remoteOrdinalReaderPagesAndStopsAtAnUnseenResult() = kotlinx.coroutines.runBlocking {
        fun page(after: Long, stop: Long) = JSONObject().put("cursor", 130).put("baselineCursor", 3)
            .put("items", JSONArray((after + 1..stop).map { n ->
                JSONObject().put("cursor", n).put("resultUuids", JSONArray(listOf("draft-$n", "final-$n")))
            })).put("hasMore", stop < 130).toString()
        val seen = (4L..130).filterNot { it == 59L }.map { "final-$it" }.toSet()
        val queries = mutableListOf<ULong>()
        val through = visibleRemoteAssistantThrough(3, 130, seen) { after ->
            queries += after
            page(after.toLong(), minOf(130, after.toLong() + 50))
        }
        assertEquals(listOf(3UL, 53UL), queries)
        assertEquals(58L, through)
    }

    @Test fun manualUnreadReplayIsRejectedByFrozenRevision() {
        val row = InboxItem(team, "team", "Channel", manualUnreadRevision = 9)
        val mark = InboxOperation.unread(row)
        assertEquals(9L, mark.body.getLong("expectedManualUnreadRevision"))
        val accepted = mark.apply(row)
        assertEquals(10L, accepted.manualUnreadRevision)
        assertEquals(accepted, mark.apply(accepted))
        val later = accepted.copy(manualUnread = false, manualUnreadRevision = 11)
        assertEquals(later, mark.apply(later))
    }

    @Test fun preferencePatchDoesNotRebaseOverAnotherDevicesUpdate() {
        val row = InboxItem(team, "team", "Channel", preferencesRevision = 4)
        val mute = InboxOperation.preferences(row, JSONObject().put("notificationMode", "none"))
        assertEquals(4L, mute.body.getLong("expectedPreferencesRevision"))
        val changed = mute.apply(row)
        assertEquals(5L, changed.preferencesRevision)
        val archive = InboxOperation.preferences(changed, JSONObject().put("archived", true))
        assertNull(mute.coalesce(archive))
        assertEquals(changed.copy(notificationMode = "all"), mute.apply(changed.copy(notificationMode = "all")))
    }

    @Test fun conversationConsentDefaultsOnAndPersistsAnIndependentTombstoneRevision() {
        val initial = AssistantConversationConsent(assistant, sourceRevision = 10)
        assertTrue(initial.consent.enabled)
        val disabled = initial.change(false, 9)
        assertFalse(disabled.consent.enabled)
        assertEquals(10L, disabled.sourceRevision)
        val restored = requireNotNull(AssistantConversationConsent.parse(disabled.json().toString()))
        assertEquals(disabled, restored)
        assertTrue(restored.change(true, 11).consent.revision > disabled.consent.revision)
    }

    @Test fun firstTurnFailureHasItsOwnIdentityBeforeAnyResult() {
        val raw = JSONObject().put("sourceId", "host").put("cursor", 0).put("baselineCursor", 0)
            .put("lastMeaningfulAt", 0).put("sourceRevision", 2).put("failed", true)
            .put("failureRevision", 1).put("failureAt", 8_000).toString()
        val activity = requireNotNull(AssistantActivity.parse(raw))
        assertTrue(activity.failed)
        assertEquals(0L, activity.cursor)
        assertEquals(1L, activity.failureRevision)
        assertEquals(8_000L, activity.failureAt)
        val row = InboxItem(assistant, "assistant", "Assistant", failureRevision = activity.failureRevision, attentionKind = "failure")
        assertEquals(1L, InboxOperation.read(row).body.getLong("observedFailureRevision"))
        assertFalse(InboxOperation.read(row, observeFailure = false).body.has("observedFailureRevision"))
        assertTrue(InboxOperation.read(row).apply(row).attention)
    }

    @Test fun notificationPreviewRequiresFreshExactTargetAndExpiry() {
        val target = InboxNoticeTarget(team, "a".repeat(64), "mention", 12, 1_000, messageId = "message_123")
        val verified = JSONObject().put("key", team.key).put("generation", team.generation).put("id", target.id)
            .put("through", target.through).put("kind", target.kind).put("messageId", target.messageId)
        assertEquals(target, target.verifiedBy(verified, 999))
        assertNull(target.verifiedBy(verified, 1_000))
        assertNull(target.verifiedBy(JSONObject(verified.toString()).put("through", 13), 999))
        assertNull(target.verifiedBy(JSONObject(verified.toString()).apply { remove("messageId") }, 999))
        val local = target.copy(ref = assistant, id = "${assistant.key}:2:reply", messageId = null)
        assertFalse(local.ledgerKey() == local.copy(through = 13).ledgerKey())
    }

    @Test fun assistantNoticePreviewDecodesUtf16TextAndOmitsToolBlocks() {
        val serialized = JSONObject().put("available", true).put("messages", JSONArray().put(JSONObject().put("blocks", JSONArray()
            .put(JSONObject().put("type", "text").put("text", JSONObject().put("code_units", JSONArray(listOf(0xd83d, 0xde00)))))
            .put(JSONObject().put("type", "tool_result").put("result_json", "private tool output")))))
        assertEquals(listOf("😀"), assistantNoticeText(serialized.toString()))
        assertNull(assistantNoticeText(serialized.put("available", false).toString()))
    }
}
