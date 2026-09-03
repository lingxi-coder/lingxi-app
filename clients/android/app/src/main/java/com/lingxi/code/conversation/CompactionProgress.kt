package com.lingxi.code.conversation

import java.util.Locale
import kotlin.math.exp
import kotlin.math.roundToInt

enum class CompactionProgressStatus {
    Running,
    Completed,
    Failed,
}

data class CompactionProgressUi(
    val status: CompactionProgressStatus,
    val startedAtMillis: Long,
    val messagesBefore: Int? = null,
    val messagesAfter: Int? = null,
    val bytesSaved: Long? = null,
    val detail: String? = null,
)

/** CLI-compatible time estimate. The real summarizer exposes no percent. */
internal fun compactProgressPercent(elapsedMs: Long): Int {
    val elapsedSeconds = elapsedMs.coerceAtLeast(0).toDouble() / 1_000.0
    return ((1.0 - exp(-elapsedSeconds / 90.0)) * 100.0)
        .roundToInt()
        .coerceIn(0, 95)
}

internal fun compactionClockMillis(): Long = System.nanoTime() / 1_000_000L

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
