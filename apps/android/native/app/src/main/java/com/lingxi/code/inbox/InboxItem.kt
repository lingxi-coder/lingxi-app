package com.lingxi.code.inbox

import org.json.JSONObject
import java.util.UUID
import java.util.Locale
import java.text.Normalizer

internal data class InboxRef(val kind: String, val sourceId: String, val conversationId: String, val generation: Long) {
    val key: String get() = "$kind:$sourceId:$conversationId"

    fun isValid(): Boolean = kind in kinds && safeId.matches(sourceId) && safeId.matches(conversationId) && generation >= 0

    companion object {
        private val kinds = setOf("assistant", "team", "service")
        private val safeId = Regex("[A-Za-z0-9_-]{1,128}")

        fun fromKey(key: String, generation: Long): InboxRef? {
            val parts = key.split(':')
            if (parts.size != 3) return null
            return InboxRef(parts[0], parts[1], parts[2], generation).takeIf { it.isValid() }
        }
    }
}

internal enum class InboxFilter(val wire: String) { All("all"), Unread("unread"), Attention("attention"), Archived("archived") }

internal fun inboxTitleSort(text: String): String =
    Normalizer.normalize(text.trim(), Normalizer.Form.NFKC).lowercase(Locale.ROOT)

internal data class InboxQuery(val filter: InboxFilter = InboxFilter.All, val category: String? = null, val search: String = "") {
    val searchPrefix: String get() = inboxTitleSort(search).take(120)
}
internal data class InboxCursor(val lastActivityAt: Long, val key: String, val pinOrder: Long? = null, val titleSort: String? = null) {
    fun json() = JSONObject().put("lastActivityAt", lastActivityAt).put("key", key).put("pinOrder", pinOrder ?: JSONObject.NULL)
        .apply { titleSort?.let { put("titleSort", it) } }
}

internal data class InboxItem(
    val ref: InboxRef,
    val category: String,
    val title: String,
    val preview: String = "",
    val lastActivityAt: Long = 0,
    val sourceCursor: Long = 0,
    val sourceRevision: Long = 0,
    val readThrough: Long = 0,
    val unreadCount: Int = 0,
    val mentionCount: Int = 0,
    val pendingActionCount: Int = 0,
    val pendingActionRevision: Long? = null,
    val pendingActionAt: Long? = null,
    val failureAt: Long? = null,
    val failureRevision: Long? = null,
    val conversationSummarySync: Boolean? = null,
    val attentionKind: String? = null,
    val state: String = "",
    val manualUnread: Boolean = false,
    val manualUnreadRevision: Long = 0,
    val stateRevision: Long = 0,
    val preferencesRevision: Long = 0,
    val pinOrder: Long? = null,
    val notificationMode: String = "mentions",
    val archived: Boolean = false,
    val offline: Boolean = false,
    val localOnly: Boolean = false,
    val context: String = "",
    val peerUid: String? = null,
    val remoteHostId: String? = null,
    val baselineCursor: Long = 0,
) {
    val key get() = ref.key
    val unread get() = manualUnread || unreadCount > 0
    val attention get() = pendingActionCount > 0 || mentionCount > 0 || !attentionKind.isNullOrEmpty()
    val needsBadge get() = !archived && (attention || unread)

    fun matches(query: InboxQuery): Boolean {
        if (query.category == "messages" && category == "assistant") return false
        if (query.category != null && query.category != "messages" && query.category != category) return false
        val matchesFilter = when (query.filter) {
            InboxFilter.All -> !archived
            InboxFilter.Archived -> archived
            InboxFilter.Unread -> unread
            InboxFilter.Attention -> attention
        }
        if (!matchesFilter) return false
        val searchPrefix = query.searchPrefix
        return searchPrefix.isEmpty() || inboxTitleSort(title).startsWith(searchPrefix)
    }

    fun json(): JSONObject = JSONObject()
        .put("schemaVersion", 1)
        .put("key", key)
        .put("ref", JSONObject().put("kind", ref.kind).put("sourceId", ref.sourceId)
            .put("conversationId", ref.conversationId).put("generation", ref.generation))
        .put("category", category).put("title", title).put("preview", preview)
        .put("lastActivityAt", lastActivityAt).put("sourceCursor", sourceCursor).put("sourceRevision", sourceRevision)
        .put("readThrough", readThrough).put("unreadCount", unreadCount).put("mentionCount", mentionCount)
        .put("pendingActionCount", pendingActionCount).put("pendingActionRevision", pendingActionRevision ?: JSONObject.NULL)
        .put("pendingActionAt", pendingActionAt ?: JSONObject.NULL).put("failureAt", failureAt ?: JSONObject.NULL)
        .put("failureRevision", failureRevision ?: JSONObject.NULL).put("attentionKind", attentionKind ?: JSONObject.NULL)
        .put("state", state).put("manualUnread", manualUnread).put("manualUnreadRevision", manualUnreadRevision)
        .put("stateRevision", stateRevision).put("preferencesRevision", preferencesRevision).put("pinOrder", pinOrder ?: JSONObject.NULL)
        .put("notificationMode", notificationMode).put("archived", archived).put("offline", offline)
        .put("peerUid", peerUid ?: JSONObject.NULL).put("remoteHostId", remoteHostId ?: JSONObject.NULL).put("localOnly", localOnly).put("context", context).put("baselineCursor", baselineCursor)

    companion object {
        val ordering = compareBy<InboxItem> { it.pinOrder ?: Long.MAX_VALUE }
            .thenByDescending { it.lastActivityAt }.thenBy { it.key }

        fun parse(json: JSONObject): InboxItem? {
            if (json.optInt("schemaVersion") != 1) return null
            val ref = json.optJSONObject("ref") ?: return null
            val parsed = InboxRef(ref.optString("kind"), ref.optString("sourceId"), ref.optString("conversationId"), ref.optLong("generation", -1))
            if (!parsed.isValid() || json.optString("key") != parsed.key) return null
            val category = json.optString("category")
            if (category !in setOf(parsed.kind, "friend") || category == "friend" && parsed.kind != "team") return null
            return InboxItem(
                ref = parsed, category = category, title = json.optString("title").take(240), preview = json.optString("preview").take(320),
                lastActivityAt = json.optLong("lastActivityAt"), sourceCursor = json.optLong("sourceCursor"),
                sourceRevision = json.optLong("sourceRevision"), readThrough = json.optLong("readThrough"),
                unreadCount = json.optInt("unreadCount").coerceIn(0, 100), mentionCount = json.optInt("mentionCount").coerceIn(0, 100),
                pendingActionCount = json.optInt("pendingActionCount").coerceIn(0, 100),
                pendingActionRevision = json.nullableLong("pendingActionRevision"), pendingActionAt = json.nullableLong("pendingActionAt"),
                failureAt = json.nullableLong("failureAt"), failureRevision = json.nullableLong("failureRevision"),
                attentionKind = json.nullableString("attentionKind"),
                state = json.optString("state"), manualUnread = json.optBoolean("manualUnread"),
                manualUnreadRevision = json.optLong("manualUnreadRevision"), stateRevision = json.optLong("stateRevision"),
                preferencesRevision = json.optLong("preferencesRevision"),
                pinOrder = json.optLong("pinOrder").takeIf { !json.isNull("pinOrder") },
                notificationMode = json.optString("notificationMode", "mentions"), archived = json.optBoolean("archived"),
                offline = json.optBoolean("offline"), localOnly = json.optBoolean("localOnly"), context = json.optString("context").ifBlank { json.optString("sourceLabel") },
                baselineCursor = json.optLong("baselineCursor"), peerUid = json.nullableString("peerUid"), remoteHostId = json.nullableString("remoteHostId"),
            )
        }
    }
}

internal fun JSONObject.nullableString(key: String): String? = if (isNull(key)) null else optString(key).takeIf { it.isNotEmpty() }
internal fun JSONObject.nullableLong(key: String): Long? = (opt(key) as? Number)?.toLong()?.takeIf { it >= 0 }

internal const val CLOUD_INBOX_AUTHORIZATION_MS = 5 * 60 * 1_000L
internal fun isCloudInboxLeaseFresh(verifiedAt: Long, now: Long): Boolean =
    verifiedAt > 0 && now >= verifiedAt && now - verifiedAt < CLOUD_INBOX_AUTHORIZATION_MS

internal data class InboxOperation(val ref: InboxRef, val body: JSONObject, val admission: String = UUID.randomUUID().toString()) {
    val action: String get() = body.getString("action")
    val slot: String get() = "${ref.key}:${ref.generation}:$action"

    fun coalesce(new: InboxOperation): InboxOperation? {
        if (slot != new.slot) return null
        if (action == "markRead" && (
                body.getLong("manualUnreadRevision") != new.body.getLong("manualUnreadRevision") ||
                    body.nullableLong("observedPendingActionRevision") != new.body.nullableLong("observedPendingActionRevision") ||
                    body.nullableLong("observedFailureRevision") != new.body.nullableLong("observedFailureRevision")
                )) return null
        if (action == "preferences" &&
            body.nullableLong("expectedPreferencesRevision") != new.body.nullableLong("expectedPreferencesRevision")) return null
        return when (action) {
            "markRead" -> new.copy(body = JSONObject(new.body.toString())
                .put("through", maxOf(body.getLong("through"), new.body.getLong("through"))))
            "preferences" -> new.copy(body = JSONObject(new.body.toString()).put("patch",
                JSONObject(body.getJSONObject("patch").toString()).apply {
                    val patch = new.body.getJSONObject("patch")
                    for (key in patch.keys()) put(key, patch.get(key))
                }))
            "markUnread" -> new.takeIf { body.getString("operationId") == new.body.getString("operationId") }
            else -> null
        }
    }

    fun apply(item: InboxItem): InboxItem {
        if (ref != item.ref) return item
        return when (action) {
            "markRead" -> {
                val through = maxOf(item.readThrough, body.getLong("through"))
                val clearManual = body.getLong("manualUnreadRevision") == item.manualUnreadRevision
                item.copy(
                    readThrough = through,
                    unreadCount = when {
                        through >= item.sourceCursor -> 0
                        item.category == "assistant" -> (item.sourceCursor - maxOf(through, item.baselineCursor)).coerceIn(0, 100).toInt()
                        else -> item.unreadCount
                    },
                    mentionCount = if (through >= item.sourceCursor) 0 else item.mentionCount,
                    manualUnread = item.manualUnread && !clearManual,
                )
            }
            "markUnread" -> {
                val expected = body.nullableLong("expectedManualUnreadRevision")
                if (expected != null && expected != item.manualUnreadRevision) item
                else item.copy(manualUnread = true, manualUnreadRevision = item.manualUnreadRevision + 1)
            }
            "preferences" -> {
                val expected = body.nullableLong("expectedPreferencesRevision")
                if (expected != null && expected != item.preferencesRevision) return item
                val patch = body.getJSONObject("patch")
                item.copy(
                    pinOrder = if (patch.has("pinOrder")) patch.optLong("pinOrder").takeUnless { patch.isNull("pinOrder") } else item.pinOrder,
                    notificationMode = patch.optString("notificationMode", item.notificationMode),
                    archived = patch.optBoolean("archived", item.archived),
                    preferencesRevision = item.preferencesRevision + 1,
                )
            }
            else -> item
        }
    }

    companion object {
        fun read(item: InboxItem, through: Long = item.sourceCursor, observeFailure: Boolean = true): InboxOperation =
            read(item.ref, through, item.manualUnreadRevision, item.pendingActionRevision, item.failureRevision.takeIf { observeFailure })

        fun read(
            ref: InboxRef, through: Long, manualRevision: Long,
            observedPendingActionRevision: Long? = null, observedFailureRevision: Long? = null,
        ): InboxOperation = forRef(ref, "markRead") {
            put("through", through).put("manualUnreadRevision", manualRevision).put("operationId", UUID.randomUUID().toString())
            observedPendingActionRevision?.let { put("observedPendingActionRevision", it) }
            observedFailureRevision?.let { put("observedFailureRevision", it) }
        }
        fun unread(item: InboxItem): InboxOperation = forRef(item.ref, "markUnread") {
            put("operationId", UUID.randomUUID().toString())
            put("expectedManualUnreadRevision", item.manualUnreadRevision)
        }
        fun preferences(item: InboxItem, patch: JSONObject): InboxOperation = forRef(item.ref, "preferences") {
            put("patch", patch).put("expectedPreferencesRevision", item.preferencesRevision)
        }

        private fun forRef(ref: InboxRef, action: String, fields: JSONObject.() -> Unit): InboxOperation =
            InboxOperation(ref, JSONObject().put("key", ref.key).put("generation", ref.generation).put("action", action).apply(fields))
    }
}

internal fun freezeLocalAssistantReads(rows: List<InboxItem>): List<Pair<InboxItem, InboxOperation>> =
    rows.filter { it.unread }.map { row -> row to InboxOperation.read(row) }

/** The rows must contain every source position between the last read cursor and each advancement. */
internal fun contiguousVisibleRead(readThrough: Long, visible: List<Long>): Long {
    var through = readThrough
    for (cursor in visible.distinct().sorted()) {
        if (cursor > through + 1) break
        if (cursor == through + 1) through = cursor
    }
    return through
}

internal data class InboxNoticeTarget(
    val ref: InboxRef, val id: String, val kind: String, val through: Long, val expiresAt: Long,
    val messageId: String? = null, val taskId: String? = null,
) {
    fun isValid(now: Long): Boolean = ref.isValid() && through in 0L..9_007_199_254_740_991L &&
        expiresAt > now && kind in kinds &&
        (if (ref.generation == 0L) ref.kind == "assistant" && id.startsWith("${ref.key}:") && id.length <= 512
         else cloudNoticeId.matches(id)) &&
        (messageId == null || safeId.matches(messageId)) && (taskId == null || safeId.matches(taskId))

    fun ledgerKey(): String = if (ref.generation > 0) id else "$id|$kind|$through|$expiresAt"

    fun verifiedBy(document: JSONObject, now: Long): InboxNoticeTarget? {
        if (!isValid(now) || document.optString("id") != id || document.optString("key") != ref.key ||
            document.optLong("generation", -1) != ref.generation || document.optLong("through", -1) != through ||
            document.optString("kind") != kind) return null
        val candidate = copy(messageId = document.nullableString("messageId"), taskId = document.nullableString("taskId"))
        if (messageId != null && candidate.messageId != messageId || taskId != null && candidate.taskId != taskId) return null
        return candidate.takeIf { it.isValid(now) }
    }

    companion object {
        private val safeId = Regex("[A-Za-z0-9_-]{1,128}")
        private val cloudNoticeId = Regex("[a-f0-9]{64}")
        private val kinds = setOf("dm", "mention", "reply", "input", "approval", "failure", "completion", "message")
    }
}

internal data class InboxNoticePreview(
    val target: InboxNoticeTarget,
    val row: InboxItem,
    val messages: List<String> = emptyList(),
)
