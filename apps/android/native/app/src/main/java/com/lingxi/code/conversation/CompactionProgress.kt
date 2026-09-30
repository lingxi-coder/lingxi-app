package com.lingxi.code.conversation

import java.util.Locale
import kotlin.math.exp
import kotlin.math.roundToInt

enum class CompactionProgressStatus {
    Running,
    Completed,
    Failed,
    Skipped,
}

data class CompactionProgressUi(
    val status: CompactionProgressStatus,
    val startedAtMillis: Long? = null,
    val phase: String = "queued",
    val unknownPhase: Boolean = false,
    val phaseStartedAtMillis: Long? = null,
    val messagesBefore: Int? = null,
    val messagesAfter: Int? = null,
    val bytesSaved: Long? = null,
    val detail: String? = null,
)

/** Phase-bounded estimate; only the engine's success event can reach 100%. */
internal fun compactProgressPercent(phase: String, elapsedMs: Long): Int? {
    if (phase == "complete") return 100
    val (base, span, tau, cap) = when (phase) {
        "preparing" -> listOf(0.0, 10.0, 5.0, 9.0)
        "summarizing" -> listOf(10.0, 75.0, 90.0, 84.0)
        "restoring" -> listOf(85.0, 14.0, 10.0, 99.0)
        else -> return null
    }
    return (base.toInt() + (span * (1 - exp(-elapsedMs.coerceAtLeast(0).toDouble() / 1000 / tau))).roundToInt())
        .coerceAtMost(cap.toInt())
}

internal fun compactionClockMillis(): Long = System.nanoTime() / 1_000_000L

internal fun reduceCompactionStatus(
    previous: CompactionProgressUi?,
    phase: String,
    error: String?,
    nowMillis: Long = compactionClockMillis(),
): CompactionProgressUi? = when (phase) {
    "preparing", "summarizing", "restoring" -> {
        val active = previous?.takeIf { it.status == CompactionProgressStatus.Running }
        val phases = listOf("preparing", "summarizing", "restoring")
        if (active != null && phases.indexOf(phase) <= phases.indexOf(active.phase)) active.copy(unknownPhase = false)
        else (active ?: CompactionProgressUi(CompactionProgressStatus.Running)).copy(
            startedAtMillis = active?.startedAtMillis ?: nowMillis,
            phase = phase,
            unknownPhase = false,
            phaseStartedAtMillis = nowMillis,
        )
    }
    "complete" -> if (previous?.status == CompactionProgressStatus.Skipped) previous else (previous ?: CompactionProgressUi(status = CompactionProgressStatus.Completed))
        .copy(status = CompactionProgressStatus.Completed, detail = null)
    "error" -> (previous ?: CompactionProgressUi(status = CompactionProgressStatus.Failed))
        .copy(status = CompactionProgressStatus.Failed, detail = error)
    "cancelled" -> null
    "skipped" -> CompactionProgressUi(CompactionProgressStatus.Skipped)
    else -> if (previous?.status == CompactionProgressStatus.Running) previous.copy(unknownPhase = true)
        else previous ?: CompactionProgressUi(CompactionProgressStatus.Running, phase = phase, unknownPhase = true)
}

internal fun isManualCompactCommand(text: String): Boolean =
    text.trim().equals("/compact", ignoreCase = true)

internal fun compactFailureDetail(message: String): String =
    message
        .replaceFirst(Regex("^force_compact failed:\\s*", RegexOption.IGNORE_CASE), "")
        .replaceFirst(Regex("^handle action failed:\\s*", RegexOption.IGNORE_CASE), "")
        .trim()
        .ifEmpty { "Unknown error" }

internal fun formatCompactBytes(bytes: Long): String = when {
    bytes >= 1_048_576L -> String.format(Locale.ROOT, "%.1f MB", bytes / 1_048_576.0)
    bytes >= 1_024L -> String.format(Locale.ROOT, "%.0f KB", bytes / 1_024.0)
    else -> "$bytes B"
}
