package com.lingxi.code.cron

import org.json.JSONArray
import org.json.JSONObject
import java.io.File
import java.io.FileOutputStream
import java.nio.file.AtomicMoveNotSupportedException
import java.nio.file.Files
import java.nio.file.StandardCopyOption
import java.security.MessageDigest
import java.util.UUID

internal const val MAX_CRON_RUNS_PER_TASK = 20
internal const val MAX_CRON_RUNS_TOTAL = 500
internal const val MAX_CRON_RESULT_BYTES = 256 * 1024

/**
 * Cap on the generated-session sidecar.
 *
 * 🚨 The sidecar deliberately outlives run-history retention, but "outlives" is
 * not "forever": it is rebuilt from the union of itself and the retained records
 * on EVERY run-state transition — roughly six per run — and each rebuild
 * re-parses, re-sorts, re-encodes, fsyncs and reads the whole file back to
 * verify its checksum. Without a cap a task on a 15-minute schedule adds ~35,000
 * rows a year, so the per-transition cost grows without bound for the life of
 * the install. The cap is generous relative to `MAX_CRON_RUNS_TOTAL` because a
 * session outlives the run that created it.
 */
internal const val MAX_CRON_SESSION_ROWS = 2_000

/**
 * App-private, atomic run-history index. History is intentionally separate from
 * Engine session transcripts. Each result links to its real conversation; the
 * compact generated-session index outlives bounded run-history retention.
 */
internal class CronRunHistoryStore(
    private val historyRoot: File,
    private val now: () -> Long = System::currentTimeMillis,
    private val newId: () -> String = { UUID.randomUUID().toString().lowercase() },
) {
    private val indexFile = File(historyRoot, "index.json")
    private val backupFile = File(historyRoot, "index.last-good.json")
    private val sessionsFile = File(historyRoot, "sessions.json")
    private val lock = Any()

    init {
        historyRoot.mkdirs()
    }

    fun records(): List<CronRunRecord> = synchronized(lock) { readLocked() }

    /** Session discovery is independent of bounded execution history. */
    fun generatedSessions(): List<CronRunRecord> = synchronized(lock) {
        (readSessionIndex() + readLocked()).filter { it.sessionId != null }
            .sortedByDescending(CronRunRecord::triggeredAtMs)
            .distinctBy { it.scopeId to it.sessionId }
    }

    private fun readSessionIndex(): List<CronRunRecord> =
        sessionsFile.takeIf(File::isFile)?.let { runCatching { decode(it.readText(Charsets.UTF_8)) }.getOrNull() }.orEmpty()


    fun record(runId: String): CronRunRecord? =
        synchronized(lock) { readLocked().firstOrNull { it.runId == runId } }

    /**
     * Atomically claims an occurrence. A task may have one running and one coalesced queued run,
     * so repeated Alarm broadcasts/watchdog passes cannot build a duplicate.
     */
    fun enqueue(
        scope: CronScope,
        taskId: String,
        prompt: String,
        scheduledAtMs: Long,
        triggeredAtMs: Long = now(),
        manual: Boolean = false,
    ): CronRunRecord? = synchronized(lock) {
        val current = readLocked()
        if (current.any {
                it.scopeId == scope.scopeId && it.taskId == taskId &&
                    (it.status == CronRunStatus.Queued || it.scheduledAtMs == scheduledAtMs)
            }) {
            return@synchronized null
        }
        val record = CronRunRecord(
            runId = newId(),
            taskId = taskId,
            scopeId = scope.scopeId,
            projectId = scope.projectId,
            projectName = scope.projectName,
            prompt = prompt,
            scheduledAtMs = scheduledAtMs,
            triggeredAtMs = triggeredAtMs,
            manual = manual,
        )
        writeLocked(current + record)
        record
    }

    fun markRunning(runId: String, attempt: Int): CronRunRecord? =
        update(runId) {
            if (it.status.isTerminal) it else it.copy(
                status = CronRunStatus.Running,
                startedAtMs = it.startedAtMs ?: now(),
                attempt = maxOf(it.attempt, attempt),
                errorMessage = null,
            )
        }

    fun markRetry(runId: String, attempt: Int, message: String): CronRunRecord? = synchronized(lock) {
        val current = readLocked()
        val target = current.firstOrNull { it.runId == runId } ?: return@synchronized null
        if (target.status.isTerminal) return@synchronized target
        val queued = target.copy(
            status = CronRunStatus.Queued,
            // WorkManager attempt counts restart for each busy wake's new job.
            // The persisted attempt also guards stale wakes and cannot rewind.
            attempt = maxOf(target.attempt, attempt),
            errorMessage = truncateUtf8(message, MAX_CRON_RESULT_BYTES),
        )
        // A later occurrence may have queued while this run was checking its
        // target. Match native busy coalescing: retry this stable run ID once.
        writeLocked(current.map { record ->
            when {
                record.runId == runId -> queued
                record.scopeId == target.scopeId && record.taskId == target.taskId && record.status == CronRunStatus.Queued ->
                    record.copy(status = CronRunStatus.Cancelled, finishedAtMs = now(), errorMessage = "Coalesced with an earlier queued execution")
                else -> record
            }
        })
        queued
    }

    fun markTerminal(
        runId: String,
        status: CronRunStatus,
        resultText: String? = null,
        errorMessage: String? = null,
        finishedAtMs: Long? = null,
    ): CronRunRecord? {
        require(status.isTerminal)
        return update(runId) {
            if (it.status.isTerminal) it else it.copy(
                status = status,
                startedAtMs = it.startedAtMs ?: now(),
                finishedAtMs = finishedAtMs ?: now(),
                resultText = resultText?.let { text -> truncateUtf8(text, MAX_CRON_RESULT_BYTES) },
                errorMessage = errorMessage?.let { text -> truncateUtf8(text, MAX_CRON_RESULT_BYTES) },
            )
        }
    }

    fun unfinished(scopeId: String, taskId: String): CronRunRecord? =
        synchronized(lock) {
            readLocked().firstOrNull {
                it.scopeId == scopeId && it.taskId == taskId && !it.status.isTerminal
            }
        }

    fun cancelUnfinished(scopeId: String, taskId: String, message: String): List<CronRunRecord> =
        synchronized(lock) {
            val current = readLocked()
            val cancelled = mutableListOf<CronRunRecord>()
            val next = current.map { record ->
                if (record.scopeId == scopeId && record.taskId == taskId && !record.status.isTerminal) {
                    record.copy(
                        status = CronRunStatus.Cancelled,
                        startedAtMs = record.startedAtMs ?: now(),
                        finishedAtMs = now(),
                        errorMessage = truncateUtf8(message, MAX_CRON_RESULT_BYTES),
                    ).also(cancelled::add)
                } else {
                    record
                }
            }
            if (cancelled.isNotEmpty()) writeLocked(next)
            cancelled
        }

    fun attachExecution(runId: String, sessionId: String? = null, model: String? = null, notificationPolicy: String? = null): CronRunRecord? = update(runId) {
        it.copy(sessionId = sessionId ?: it.sessionId, model = model ?: it.model, notificationPolicy = notificationPolicy ?: it.notificationPolicy)
    }

    /** Reserve delivery durably before posting; process recovery never repeats a notification. */
    fun claimNotification(runId: String): Boolean = synchronized(lock) {
        val record = readLocked().firstOrNull { it.runId == runId } ?: return@synchronized false
        if (!record.status.isTerminal || record.notificationDelivered) return@synchronized false
        update(runId) { it.copy(notificationDelivered = true) }
        true
    }

    /** Undo a claim whose notification never reached the shade, so recovery can retry it. */
    fun releaseNotificationClaim(runId: String) {
        synchronized(lock) { update(runId) { it.copy(notificationDelivered = false) } }
    }

    /** Recover notification delivery without dispatching or consulting mutable task settings. */
    fun postPendingNotifications(
        runId: String? = null,
        onFailure: (Exception) -> Unit = { throw it },
        post: (CronRunRecord) -> Unit,
    ) {
        records().filter { runId == null || it.runId == runId }.forEach { record ->
            try {
                if (shouldNotifyCronRun(record.notificationPolicy, record.status) &&
                    claimNotification(record.runId)) {
                    // The claim is durable, so `post` MUST release it again when
                    // the notification does not actually reach the shade —
                    // otherwise the result is marked delivered forever and this
                    // very recovery path skips it (see `releaseNotificationClaim`).
                    try {
                        post(record)
                    } catch (error: Exception) {
                        releaseNotificationClaim(record.runId)
                        throw error
                    }
                }
            } catch (error: Exception) {
                onFailure(error)
            }
        }
    }

    fun cancelQueued(scopeId: String, taskId: String) {
        records().filter { it.scopeId == scopeId && it.taskId == taskId && it.status == CronRunStatus.Queued }
            .forEach { markTerminal(it.runId, CronRunStatus.Cancelled, errorMessage = "Task is paused or completed") }
    }

    private fun update(
        runId: String,
        transform: (CronRunRecord) -> CronRunRecord,
    ): CronRunRecord? = synchronized(lock) {
        val current = readLocked()
        val index = current.indexOfFirst { it.runId == runId }
        if (index < 0) return@synchronized null
        val nextRecord = transform(current[index])
        val next = current.toMutableList().also { it[index] = nextRecord }
        writeLocked(next)
        nextRecord
    }

    private fun readLocked(): List<CronRunRecord> {
        val primaryText = indexFile.takeIf(File::isFile)
            ?.let { runCatching { it.readText(Charsets.UTF_8) }.getOrNull() }
        val primary = primaryText?.let { runCatching { decode(it) }.getOrNull() }
        if (primary != null) return primary

        val backupText = backupFile.takeIf(File::isFile)
            ?.let { runCatching { it.readText(Charsets.UTF_8) }.getOrNull() }
        val backup = backupText?.let { runCatching { decode(it) }.getOrNull() }
            ?: return emptyList()
        if (indexFile.exists()) {
            val quarantine = File(
                historyRoot,
                "index.corrupt.${System.currentTimeMillis()}.json",
            )
            runCatching {
                Files.move(
                    indexFile.toPath(),
                    quarantine.toPath(),
                    StandardCopyOption.REPLACE_EXISTING,
                )
            }
        }
        runCatching { atomicWrite(indexFile, backupText) }
        return backup
    }

    private fun writeLocked(records: List<CronRunRecord>) {
        historyRoot.mkdirs()
        val retained = retainCronHistory(records)
        val text = encode(retained)
        decode(text)
        // The primary index goes first and unguarded: it carries the run-state
        // transition this call exists to persist. The sessions sidecar is a
        // derived convenience, so it is written after and its failure must not
        // strand a run as `Running` forever.
        atomicWrite(indexFile, text)
        val sessions = (records.filter { it.sessionId != null } + readSessionIndex())
            .sortedByDescending(CronRunRecord::triggeredAtMs)
            .distinctBy { it.scopeId to it.sessionId }
            .take(MAX_CRON_SESSION_ROWS)
            .map { it.copy(resultText = null, errorMessage = null) }
        runCatching { atomicWrite(sessionsFile, encode(sessions)) }
        // A second atomic file gives checksum-verified recovery if external
        // storage corruption damages the primary after its rename.
        runCatching { atomicWrite(backupFile, text) }
    }

    private fun atomicWrite(target: File, text: String) {
        val temp = File(historyRoot, ".${target.name}.${UUID.randomUUID()}.tmp")
        try {
            FileOutputStream(temp).use { output ->
                output.write(text.toByteArray(Charsets.UTF_8))
                output.fd.sync()
            }
            decode(temp.readText(Charsets.UTF_8))
            try {
                Files.move(
                    temp.toPath(),
                    target.toPath(),
                    StandardCopyOption.ATOMIC_MOVE,
                    StandardCopyOption.REPLACE_EXISTING,
                )
            } catch (_: AtomicMoveNotSupportedException) {
                Files.move(
                    temp.toPath(),
                    target.toPath(),
                    StandardCopyOption.REPLACE_EXISTING,
                )
            }
        } finally {
            temp.delete()
        }
    }

    private fun encode(records: List<CronRunRecord>): String {
        val payload = JSONObject()
            .put("version", 1)
            .put(
                "runs",
                JSONArray().also { array ->
                records.sortedByDescending(CronRunRecord::triggeredAtMs).forEach { run ->
                    array.put(
                        JSONObject()
                            .put("runId", run.runId)
                            .put("taskId", run.taskId)
                            .put("scopeId", run.scopeId)
                            .put("projectId", run.projectId)
                            .put("projectName", run.projectName)
                            .put("prompt", run.prompt)
                            .put("scheduledAtMs", run.scheduledAtMs)
                            .put("triggeredAtMs", run.triggeredAtMs)
                            .put("startedAtMs", run.startedAtMs)
                            .put("finishedAtMs", run.finishedAtMs)
                            .put("status", run.status.name)
                            .put("attempt", run.attempt)
                            .put("resultText", run.resultText)
                            .put("errorMessage", run.errorMessage)
                            .put("sessionId", run.sessionId)
                            .put("model", run.model)
                            .put("notificationPolicy", run.notificationPolicy)
                            .put("notificationDelivered", run.notificationDelivered)
                            .put("manual", run.manual),
                    )
                }
            },
            )
            .toString()
        return JSONObject()
            .put("version", 2)
            .put("payload", payload)
            .put("sha256", sha256(payload))
            .toString(2)
    }

    private fun decode(text: String): List<CronRunRecord> {
        val envelope = JSONObject(text)
        val root = if (envelope.optInt("version") == 2) {
            val payload = envelope.getString("payload")
            require(envelope.getString("sha256") == sha256(payload)) {
                "cron history checksum mismatch"
            }
            JSONObject(payload)
        } else {
            // One-time migration for development builds that wrote the original
            // atomic-but-unchecksummed v1 format.
            envelope
        }
        require(root.getInt("version") == 1)
        val array = root.getJSONArray("runs")
        return (0 until array.length()).map { index ->
            val run = array.getJSONObject(index)
            CronRunRecord(
                runId = run.getString("runId"),
                taskId = run.getString("taskId"),
                scopeId = run.getString("scopeId"),
                projectId = run.nullableString("projectId"),
                projectName = run.getString("projectName"),
                prompt = run.getString("prompt"),
                scheduledAtMs = run.getLong("scheduledAtMs"),
                triggeredAtMs = run.getLong("triggeredAtMs"),
                startedAtMs = run.nullableLong("startedAtMs"),
                finishedAtMs = run.nullableLong("finishedAtMs"),
                status = CronRunStatus.valueOf(run.getString("status")),
                attempt = run.getInt("attempt"),
                resultText = run.nullableString("resultText"),
                errorMessage = run.nullableString("errorMessage"),
                manual = run.optBoolean("manual", false),
                sessionId = run.nullableString("sessionId"),
                model = run.nullableString("model"),
                notificationPolicy = run.optString("notificationPolicy", "all"),
                // A record written before this field existed has already been
                // notified (or deliberately not). Defaulting it to `false` would
                // replay the entire retained history as fresh notifications on
                // the first launch after upgrading — and the tag is now per-run,
                // so they no longer collapse into one.
                notificationDelivered = run.optBoolean("notificationDelivered", !run.has("notificationPolicy")),
            )
        }.distinctBy(CronRunRecord::runId)
    }
}

private fun sha256(value: String): String =
    MessageDigest.getInstance("SHA-256")
        .digest(value.toByteArray(Charsets.UTF_8))
        .joinToString("") { byte ->
            val valueByte = byte.toInt() and 0xff
            "${HEX_DIGITS[valueByte ushr 4]}${HEX_DIGITS[valueByte and 0x0f]}"
        }

private const val HEX_DIGITS = "0123456789abcdef"

internal fun retainCronHistory(
    records: List<CronRunRecord>,
    perTaskLimit: Int = MAX_CRON_RUNS_PER_TASK,
    totalLimit: Int = MAX_CRON_RUNS_TOTAL,
): List<CronRunRecord> {
    require(perTaskLimit > 0 && totalLimit > 0)
    val active = records
        .filterNot { it.status.isTerminal }
        .distinctBy(CronRunRecord::runId)
        .sortedByDescending(CronRunRecord::triggeredAtMs)
    val perTask = mutableMapOf<Pair<String, String>, Int>()
    active.forEach { record ->
        val key = record.scopeId to record.taskId
        perTask[key] = (perTask[key] ?: 0) + 1
    }
    val terminalLimit = maxOf(0, totalLimit - active.size)
    val retainedTerminal = ArrayList<CronRunRecord>(minOf(records.size, terminalLimit))
    for (record in records.asSequence()
        .filter { it.status.isTerminal }
        .distinctBy(CronRunRecord::runId)
        .sortedByDescending(CronRunRecord::triggeredAtMs)
    ) {
        if (retainedTerminal.size >= terminalLimit) break
        val key = record.scopeId to record.taskId
        val count = perTask[key] ?: 0
        if (count >= perTaskLimit) continue
        retainedTerminal += record
        perTask[key] = count + 1
    }
    return (active + retainedTerminal).sortedByDescending(CronRunRecord::triggeredAtMs)
}

internal fun truncateUtf8(value: String, maxBytes: Int): String {
    require(maxBytes >= 0)
    if (value.toByteArray(Charsets.UTF_8).size <= maxBytes) return value
    var low = 0
    var high = value.length
    while (low < high) {
        val middle = (low + high + 1) ushr 1
        val candidate = value.substring(0, middle)
        if (candidate.toByteArray(Charsets.UTF_8).size <= maxBytes) {
            low = middle
        } else {
            high = middle - 1
        }
    }
    var end = low
    if (end > 0 && end < value.length &&
        Character.isHighSurrogate(value[end - 1]) &&
        Character.isLowSurrogate(value[end])
    ) {
        end -= 1
    }
    return value.substring(0, end)
}

private fun JSONObject.nullableString(name: String): String? =
    if (!has(name) || isNull(name)) null else getString(name)

private fun JSONObject.nullableLong(name: String): Long? =
    if (!has(name) || isNull(name)) null else getLong(name)
