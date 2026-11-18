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
}
