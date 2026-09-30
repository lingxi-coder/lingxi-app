package com.lingxi.code.computeruse

import android.content.Context
import org.json.JSONObject
import java.io.File
import java.time.Instant
import java.time.temporal.ChronoUnit

internal class ComputerUseAuditStore(context: Context) {
    private val directory = File(context.filesDir, "computer-use/audit").apply { mkdirs() }
    private val file = File(directory, "events.jsonl")
    private val lock = Any()

    fun append(
        targetPackage: String,
        actionType: String,
        risk: ComputerUseRisk,
        confirmation: String,
        result: String,
    ) {
        val record = JSONObject()
            .put("timestamp_ms", System.currentTimeMillis())
            .put("target_package", targetPackage)
            .put("action_type", actionType)
            .put("risk", risk.name.lowercase())
            .put("confirmation", confirmation)
            .put("result", result)
        synchronized(lock) {
            directory.mkdirs()
            file.appendText(record.toString() + "\n")
            pruneLocked()
        }
    }

    fun clear() {
        synchronized(lock) {
            if (file.exists()) file.delete()
        }
    }

    private fun pruneLocked() {
        if (!file.exists()) return
        val cutoff = Instant.now().minus(30, ChronoUnit.DAYS).toEpochMilli()
        val retained = file.useLines { lines ->
            lines.mapNotNull { line ->
                runCatching { JSONObject(line) }.getOrNull()
                    ?.takeIf { it.optLong("timestamp_ms") >= cutoff }
                    ?.toString()
            }.toList()
        }
        val temp = File(directory, "events.jsonl.tmp")
        temp.writeText(retained.joinToString(separator = "\n", postfix = if (retained.isEmpty()) "" else "\n"))
        if (!temp.renameTo(file)) {
            file.delete()
            temp.renameTo(file)
        }
    }
}
