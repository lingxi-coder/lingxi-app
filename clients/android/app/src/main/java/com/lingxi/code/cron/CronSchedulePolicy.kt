package com.lingxi.code.cron

internal object CronSchedulePolicy {
    const val MIN_RECURRING_INTERVAL_MINUTES = 15
    const val UNSUPPORTED_INTERVAL_REASON = "Android 重复任务最小间隔为 15 分钟"

    /**
     * Conservatively validates the minute field of a standard five-field cron.
     * A single minute is at most hourly. Multiple minute values must have at
     * least 15 minutes between adjacent values, including the hour boundary.
     */
    fun isAndroidSupportedRecurring(cron: String, recurring: Boolean): Boolean {
        if (!recurring) return true
        val fields = cron.trim().split(Regex("\\s+"))
        if (fields.size != 5) return false
        val minutes = parseMinuteField(fields.first()) ?: return false
        if (minutes.size <= 1) return true
        val sorted = minutes.sorted()
        val gaps = sorted.zipWithNext { left, right -> right - left } +
            (60 - sorted.last() + sorted.first())
        return gaps.minOrNull()?.let { it >= MIN_RECURRING_INTERVAL_MINUTES } == true
    }

    internal fun parseMinuteField(field: String): Set<Int>? {
        val values = linkedSetOf<Int>()
        for (part in field.split(',')) {
            if (part.isBlank()) return null
            val (base, step) = if ('/' in part) {
                val pair = part.split('/')
                if (pair.size != 2) return null
                val parsedStep = pair[1].toIntOrNull()?.takeIf { it in 1..59 } ?: return null
                pair[0] to parsedStep
            } else {
                part to 1
            }
            val range = when {
                base == "*" -> 0..59
                '-' in base -> {
                    val ends = base.split('-')
                    if (ends.size != 2) return null
                    val start = ends[0].toIntOrNull()?.takeIf { it in 0..59 } ?: return null
                    val end = ends[1].toIntOrNull()?.takeIf { it in start..59 } ?: return null
                    start..end
                }
                else -> {
                    val minute = base.toIntOrNull()?.takeIf { it in 0..59 } ?: return null
                    minute..minute
                }
            }
            range.step(step).forEach(values::add)
        }
        return values.takeIf { it.isNotEmpty() }
    }
}
