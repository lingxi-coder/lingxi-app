package com.lingxi.code.inbox

import org.json.JSONObject
import java.io.File
import java.nio.file.Files
import java.nio.file.LinkOption

internal data class AssistantActivity(
    val sourceId: String,
    val cursor: Long,
    val baseline: Long,
    val at: Long,
    val previewKind: String?,
    val pendingActions: Int,
    val failed: Boolean,
    val revision: Long,
    val headMessageId: String?,
    val pendingActionKind: String?,
    val pendingActionAt: Long?,
    val pendingActionRevision: Long?,
    val failureAt: Long?,
    val failureRevision: Long?,
) {
    companion object {
        fun parse(raw: String?): AssistantActivity? {
            if (raw == null) return null
            return runCatching {
                val value = JSONObject(raw)
                val cursor = value.getLong("cursor")
                val baseline = value.getLong("baselineCursor")
                require(baseline in 0..cursor)
                AssistantActivity(
                    value.getString("sourceId"), cursor, baseline, value.getLong("lastMeaningfulAt"),
                    value.nullableString("previewKind"), value.optInt("pendingActionCount"), value.optBoolean("failed"),
                    value.getLong("sourceRevision"), value.optJSONArray("latestResultUuids")?.let { ids ->
                        ids.optString(ids.length() - 1).takeIf(String::isNotBlank)
                    },
                    value.nullableString("pendingActionKind")?.takeIf { it == "input" || it == "approval" },
                    value.nullableLong("pendingActionAt"), value.nullableLong("pendingActionRevision"),
                    value.nullableLong("failureAt"), value.nullableLong("failureRevision"),
                )
            }.getOrNull()
        }
    }
}

/** Read the runtime's committed activity index. The rendered transcript supplies the visibility proof. */
internal object LocalAssistantActivity {
    fun summary(home: File, session: String): String? = read(home, session, "summary.json")

    fun through(home: File, session: String, sourceId: String, after: Long, head: Long, visible: Set<String>): Long {
        if (after >= head || visible.isEmpty()) return after
        val activity = AssistantActivity.parse(summary(home, session)) ?: return after
        if (activity.sourceId != sourceId || activity.cursor < head) return after
        var through = after
        while (through < head && through - after < 50) {
            val cursor = through + 1
            val raw = read(home, session, "entries/$cursor.json") ?: break
            val item = runCatching { JSONObject(raw) }.getOrNull() ?: break
            val ids = item.optJSONArray("resultUuids") ?: break
            val final = ids.optString(ids.length() - 1)
            if (item.optLong("cursor") != cursor || final !in visible) break
            through = cursor
        }
        return through
    }

    private fun read(home: File, session: String, name: String): String? {
        return runCatching {
            if (java.util.UUID.fromString(session).toString() != session) return null
            val base = File(home, "inbox-activity").toPath()
            val dir = base.resolve(session)
            val path = dir.resolve(name)
            val dirs = if (name.contains('/')) listOf(base, dir, path.parent) else listOf(base, dir)
            if (dirs.any { !Files.isDirectory(it, LinkOption.NOFOLLOW_LINKS) } ||
                !Files.isRegularFile(path, LinkOption.NOFOLLOW_LINKS) || Files.size(path) > 16_384) return null
            Files.newInputStream(path, LinkOption.NOFOLLOW_LINKS).buffered().use { input ->
                val data = ByteArray(16_385)
                var count = 0
                while (count < data.size) {
                    val read = input.read(data, count, data.size - count)
                    if (read < 0) break
                    count += read
                }
                if (count <= 16_384) String(data, 0, count, Charsets.UTF_8) else null
            }
        }.getOrNull()
    }
}


internal data class AssistantConsent(val enabled: Boolean = false, val revision: Long = 1) {
    fun change(enabled: Boolean): AssistantConsent =
        if (enabled == this.enabled) this else AssistantConsent(enabled, revision + 1)

    fun json() = JSONObject().put("enabled", enabled).put("revision", revision)

    companion object {
        fun load(saved: String?, legacy: String?): AssistantConsent {
            if (saved == null) return AssistantConsent(legacy == "true")
            val json = JSONObject(saved)
            return AssistantConsent(json.getBoolean("enabled"), json.getLong("revision").also { require(it > 0) })
        }
    }
}

internal data class VisibleAssistantPage(val through: Long, val more: Boolean)

internal fun applyVisibleAssistantPage(raw: String, after: Long, head: Long, visible: Set<String>): VisibleAssistantPage {
    val page = runCatching { JSONObject(raw) }.getOrNull() ?: return VisibleAssistantPage(after, false)
    val cursor = page.nullableLong("cursor") ?: return VisibleAssistantPage(after, false)
    val baseline = page.nullableLong("baselineCursor") ?: return VisibleAssistantPage(after, false)
    val items = page.optJSONArray("items") ?: return VisibleAssistantPage(after, false)
    if (after < baseline || cursor < after || items.length() > 50) return VisibleAssistantPage(after, false)
    val cap = minOf(head, cursor)
    var through = after
    for (index in 0 until items.length()) {
        if (through >= cap) break
        val result = items.optJSONObject(index) ?: return VisibleAssistantPage(through, false)
        val position = result.nullableLong("cursor") ?: return VisibleAssistantPage(through, false)
        val ids = result.optJSONArray("resultUuids") ?: return VisibleAssistantPage(through, false)
        if (position != through + 1 || ids.optString(ids.length() - 1) !in visible) {
            return VisibleAssistantPage(through, false)
        }
        through = position
    }
    return VisibleAssistantPage(through, through > after && through < cap && page.optBoolean("hasMore"))
}

internal suspend fun visibleRemoteAssistantThrough(
    after: Long, head: Long, rendered: Set<String>, fetch: suspend (ULong) -> String,
): Long {
    if (after >= head || rendered.isEmpty()) return after
    var through = after
    // At most four pages can fit in the active 200-row window.
    repeat(4) {
        val raw = try { fetch(through.toULong()) }
        catch (cancelled: kotlinx.coroutines.CancellationException) { throw cancelled }
        catch (_: Exception) { return through }
        val accepted = applyVisibleAssistantPage(raw, through, head, rendered)
        through = accepted.through
        if (!accepted.more) return through
    }
    return through
}

internal data class AssistantConversationConsent(
    val ref: InboxRef, val consent: AssistantConsent = AssistantConsent(enabled = true), val sourceRevision: Long = 0,
) {
    fun change(enabled: Boolean, sourceRevision: Long) = copy(consent = consent.change(enabled), sourceRevision = maxOf(this.sourceRevision, sourceRevision))

    fun json() = JSONObject().put("key", ref.key).put("consent", consent.json()).put("sourceRevision", sourceRevision)

    companion object {
        fun parse(raw: String): AssistantConversationConsent? = runCatching {
            val json = JSONObject(raw)
            val ref = InboxRef.fromKey(json.getString("key"), 0)?.takeIf { it.kind == "assistant" } ?: return null
            AssistantConversationConsent(ref, AssistantConsent.load(json.getJSONObject("consent").toString(), null), json.getLong("sourceRevision"))
        }.getOrNull()
    }
}
