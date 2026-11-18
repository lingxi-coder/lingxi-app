package com.lingxi.code.inbox

import android.content.ContentValues
import android.content.Context
import android.database.sqlite.SQLiteDatabase
import android.database.sqlite.SQLiteOpenHelper
import org.json.JSONArray
import org.json.JSONObject
import java.io.File

/** Local state and pending writes live in no-backup storage, separately from evictable summaries. */
internal class InboxStorage(context: Context) :
    SQLiteOpenHelper(context, File(context.noBackupFilesDir, "conversation-inbox.db").path, null, 1) {
    override fun onCreate(db: SQLiteDatabase) {
        db.execSQL("CREATE TABLE preferences (owner TEXT NOT NULL, name TEXT NOT NULL, value TEXT NOT NULL, PRIMARY KEY(owner, name))")
        db.execSQL("CREATE TABLE pages (owner TEXT NOT NULL, cache_key TEXT NOT NULL, value TEXT NOT NULL, bytes INTEGER NOT NULL, touched INTEGER NOT NULL, PRIMARY KEY(owner, cache_key))")
        db.execSQL("CREATE TABLE outbox (id INTEGER PRIMARY KEY AUTOINCREMENT, owner TEXT NOT NULL, slot TEXT NOT NULL, admission TEXT NOT NULL, value TEXT NOT NULL, bytes INTEGER NOT NULL, failure TEXT)")
        db.execSQL("CREATE INDEX inbox_outbox_owner ON outbox(owner, id)")
        db.execSQL("CREATE TABLE delivery (owner TEXT NOT NULL, event TEXT NOT NULL, expires INTEGER NOT NULL, PRIMARY KEY(owner, event))")
    }

    override fun onUpgrade(db: SQLiteDatabase, oldVersion: Int, newVersion: Int) = Unit

    fun value(owner: String, name: String): String? = readableDatabase.rawQuery(
        "SELECT value FROM preferences WHERE owner = ? AND name = ?", arrayOf(owner, name),
    ).use { if (it.moveToFirst()) it.getString(0) else null }

    fun consent(owner: String): AssistantConsent = AssistantConsent.load(value(owner, "assistantConsent"), value(owner, "assistantOptIn"))

    fun changeConsent(owner: String, enabled: Boolean): AssistantConsent = write { db ->
        val updated = consent(owner).change(enabled)
        db.replaceOrThrow("preferences", null, ContentValues().apply {
            put("owner", owner); put("name", "assistantConsent"); put("value", updated.json().toString())
        })
        updated
    }

    fun conversationConsents(owner: String): Map<String, AssistantConversationConsent> = readableDatabase.rawQuery(
        "SELECT value FROM preferences WHERE owner = ? AND name LIKE 'assistantConversationConsent:%'", arrayOf(owner),
    ).use { rows ->
        buildMap {
            while (rows.moveToNext()) AssistantConversationConsent.parse(rows.getString(0))?.let { put(it.ref.key, it) }
        }
    }

    fun changeConversationConsent(owner: String, ref: InboxRef, sourceRevision: Long, enabled: Boolean): AssistantConversationConsent = write { db ->
        val name = "assistantConversationConsent:${ref.key}"
        val updated = (value(owner, name)?.let(AssistantConversationConsent::parse)
            ?: AssistantConversationConsent(ref.copy(generation = 0))).change(enabled, sourceRevision)
        db.replaceOrThrow("preferences", null, ContentValues().apply {
            put("owner", owner); put("name", name); put("value", updated.json().toString())
        })
        updated
    }

    fun setValue(owner: String, name: String, value: String?) = write { db ->
        if (value == null) db.delete("preferences", "owner = ? AND name = ?", arrayOf(owner, name))
        else db.replaceOrThrow("preferences", null, ContentValues().apply {
            put("owner", owner); put("name", name); put("value", value)
        })
        Unit
    }

    fun rememberLocal(owner: String, item: InboxItem, op: InboxOperation): InboxItem = write { db ->
        val name = "local:${item.key}"
        val current = db.rawQuery("SELECT value FROM preferences WHERE owner = ? AND name = ?", arrayOf(owner, name)).use {
            if (it.moveToFirst()) InboxItem.parse(JSONObject(it.getString(0))) else null
        }
        val currentPersonal = current?.takeIf { it.ref == item.ref }
        val changed = op.apply(item.withLocalState(currentPersonal))
        db.replaceOrThrow("preferences", null, ContentValues().apply {
            put("owner", owner); put("name", name); put("value", changed.json().toString())
        })
        changed
    }

    fun cache(owner: String, cacheKey: String, page: JSONObject, verifiedAt: Long) = write { db ->
        val data = page.toString()
        val bytes = data.toByteArray().size
        if (bytes > CACHE_BYTES) return@write
        db.replaceOrThrow("pages", null, ContentValues().apply {
            put("owner", owner); put("cache_key", cacheKey); put("value", data)
            put("bytes", bytes); put("touched", verifiedAt)
        })
        var excess = db.rawQuery("SELECT COALESCE(SUM(bytes), 0) FROM pages", null).use { it.moveToFirst(); it.getLong(0) } - CACHE_BYTES
        if (excess > 0) db.rawQuery("SELECT rowid, bytes FROM pages ORDER BY touched, rowid", null).use { entries ->
            val evictions = mutableListOf<String>()
            while (excess > 0 && entries.moveToNext()) {
                evictions.add(entries.getLong(0).toString())
                excess -= entries.getLong(1)
            }
            evictions.forEach { db.delete("pages", "rowid = ?", arrayOf(it)) }
        }
    }

    data class VerifiedPage(val value: JSONObject, val verifiedAt: Long)

    fun cached(owner: String, cacheKey: String, now: Long): VerifiedPage? = readableDatabase.rawQuery(
        "SELECT value, touched FROM pages WHERE owner = ? AND cache_key = ?", arrayOf(owner, cacheKey),
    ).use {
        if (it.moveToFirst() && isCloudInboxLeaseFresh(it.getLong(1), now)) VerifiedPage(JSONObject(it.getString(0)), it.getLong(1))
        else null
    }

    fun purgePages(owner: String) {
        writableDatabase.delete("pages", "owner = ?", arrayOf(owner))
    }

    fun enqueue(owner: String, operation: InboxOperation): Boolean = write { db ->
        // Only a compatible operation at this conversation's tail may be replaced.
        // A manual unread between two reads is part of the write order.
        val tail = db.rawQuery(
            "SELECT id, slot, admission, value FROM outbox WHERE owner = ? AND slot LIKE ? ESCAPE '!' AND failure IS NULL ORDER BY id DESC LIMIT 1",
            arrayOf(owner, operation.slot.substringBeforeLast(':').replace("_", "!_") + ":%"),
        ).use {
            if (it.moveToFirst() && it.getString(1) == operation.slot) {
                val previous = InboxOperation(operation.ref, JSONObject(it.getString(3)), it.getString(2))
                previous.coalesce(operation)?.let { merged -> Triple(it.getLong(0), previous, merged) }
            } else null
        }
        val admitted = tail?.third ?: operation
        val data = admitted.body.toString()
        val bytes = data.toByteArray().size
        val totals = db.rawQuery(
            "SELECT COUNT(*), COALESCE(SUM(bytes), 0) FROM outbox WHERE owner = ?", arrayOf(owner),
        ).use { it.moveToFirst(); it.getInt(0) to it.getLong(1) }
        val replacedBytes = tail?.second?.body?.toString()?.toByteArray()?.size ?: 0
        val additionalEntries = if (tail == null) 1 else 0
        if (totals.first + additionalEntries > OUTBOX_COUNT || totals.second - replacedBytes + bytes > OUTBOX_BYTES) return@write false
        tail?.let { db.delete("outbox", "id = ?", arrayOf(it.first.toString())) }
        db.insertOrThrow("outbox", null, ContentValues().apply {
            put("owner", owner); put("slot", admitted.slot); put("admission", admitted.admission); put("value", data); put("bytes", bytes)
        })
        true
    }

    fun pending(owner: String): List<InboxOperation> = readableDatabase.rawQuery(
        "SELECT value, admission FROM outbox WHERE owner = ? AND failure IS NULL ORDER BY id", arrayOf(owner),
    ).use {
        buildList {
            while (it.moveToNext()) {
                val body = JSONObject(it.getString(0))
                val ref = InboxRef.fromKey(body.getString("key"), body.getLong("generation")) ?: error("Invalid persisted inbox key")
                add(InboxOperation(ref, body, it.getString(1)))
            }
        }
    }

    fun acknowledge(owner: String, operation: InboxOperation) {
        writableDatabase.delete("outbox", "owner = ? AND admission = ?", arrayOf(owner, operation.admission))
    }

    fun reject(owner: String, operation: InboxOperation, reason: String) {
        writableDatabase.update("outbox", ContentValues().apply { put("failure", reason) },
            "owner = ? AND admission = ?", arrayOf(owner, operation.admission))
    }

    fun hasRejected(owner: String): Boolean = readableDatabase.rawQuery(
        "SELECT 1 FROM outbox WHERE owner = ? AND failure IS NOT NULL LIMIT 1", arrayOf(owner),
    ).use { it.moveToFirst() }

    fun clearRejected(owner: String) {
        writableDatabase.delete("outbox", "owner = ? AND failure IS NOT NULL", arrayOf(owner))
    }

    fun admitDelivery(owner: String, event: String, now: Long): Boolean = write { db ->
        db.delete("delivery", "expires <= ?", arrayOf(now.toString()))
        db.insertWithOnConflict("delivery", null, ContentValues().apply {
            put("owner", owner); put("event", event); put("expires", now + 7L * 24 * 60 * 60 * 1_000)
        }, SQLiteDatabase.CONFLICT_IGNORE) >= 0
    }

    private fun <T> write(action: (SQLiteDatabase) -> T): T {
        val db = writableDatabase
        db.beginTransaction()
        return try {
            action(db).also { db.setTransactionSuccessful() }
        } finally {
            db.endTransaction()
        }
    }

    companion object {
        private const val CACHE_BYTES = 16 * 1024 * 1024
        internal const val OUTBOX_BYTES = 16 * 1024 * 1024
        internal const val OUTBOX_COUNT = 10_000
    }
}

internal fun InboxItem.withLocalState(previous: InboxItem?): InboxItem {
    if (previous == null || previous.ref != ref) return this
    val through = maxOf(readThrough, previous.readThrough)
    return copy(
        readThrough = through,
        manualUnread = previous.manualUnread,
        manualUnreadRevision = previous.manualUnreadRevision,
        archived = previous.archived, pinOrder = previous.pinOrder, notificationMode = previous.notificationMode,
        preferencesRevision = previous.preferencesRevision,
        unreadCount = if (ref.kind == "assistant") (sourceCursor - maxOf(baselineCursor, through)).coerceIn(0, 100).toInt() else unreadCount,
    )
}

internal fun JSONArray.inboxItems(): List<InboxItem> = (0 until length()).mapNotNull {
    optJSONObject(it)?.let(InboxItem::parse)
}
