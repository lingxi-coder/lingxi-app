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
 * App-private, atomic run-history index. History is intentionally separate from
 * Engine sessions: background Agent output never appears as a chat transcript.
 */
internal class CronRunHistoryStore(
    private val historyRoot: File,
    private val now: () -> Long = System::currentTimeMillis,
    private val newId: () -> String = { UUID.randomUUID().toString().lowercase() },
) {
    private val indexFile = File(historyRoot, "index.json")
    private val backupFile = File(historyRoot, "index.last-good.json")
    private val lock = Any()

    init {
        historyRoot.mkdirs()
    }

    fun records(): List<CronRunRecord> = synchronized(lock) { readLocked() }

    fun record(runId: String): CronRunRecord? =
        synchronized(lock) { readLocked().firstOrNull { it.runId == runId } }

    /**
     * Atomically claims an occurrence. A task may have only one unfinished run,
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
        if (current.any { it.scopeId == scope.scopeId && it.taskId == taskId && !it.status.isTerminal }) {
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
                attempt = attempt,
                errorMessage = null,
            )
        }

    fun markRetry(runId: String, attempt: Int, message: String): CronRunRecord? =
        update(runId) {
            if (it.status.isTerminal) it else it.copy(
                status = CronRunStatus.Queued,
                attempt = attempt,
                errorMessage = truncateUtf8(message, MAX_CRON_RESULT_BYTES),
            )
        }

    fun markTerminal(
        runId: String,
        status: CronRunStatus,
        resultText: String? = null,
        errorMessage: String? = null,
    ): CronRunRecord? {
        require(status.isTerminal)
        return update(runId) {
            if (it.status.isTerminal) it else it.copy(
                status = status,
                startedAtMs = it.startedAtMs ?: now(),
                finishedAtMs = now(),
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
        atomicWrite(indexFile, text)
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
