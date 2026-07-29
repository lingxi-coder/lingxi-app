/*
 * Copyright (C) 2026 OpenMinis contributors
 * Copyright (C) 2026 LingXi contributors
 * SPDX-License-Identifier: GPL-3.0-only
 *
 * Adapted from OpenMinis commit 9cf3a855.
 */
package com.lingxi.code.shell

/** Builds a trusted reminder from untrusted submitted shell source. */
object BashismReminder {
    fun sanitize(value: String): String {
        val neutralized = value
            .replace("<", "‹")
            .replace(">", "›")
            .filter { it.code >= 0x20 || it == ' ' }
        return if (neutralized.length > 120) neutralized.take(120) + "…" else neutralized
    }

    fun build(
        hits: List<BashismDetector.Hit>,
        installFailure: String?,
    ): String? {
        if (hits.isEmpty()) return null
        val unique = hits.distinctBy { "${it.line}:${it.ruleName}" }
        val shown = unique.take(8)
        val overflow = unique.size - shown.size
        return buildString {
            appendLine("<system-reminder>")
            if (installFailure != null) {
                appendLine(
                    "This command was executed by BusyBox sh (not Bash), because Bash " +
                        "installation failed: ${sanitize(installFailure)}.",
                )
            } else {
                appendLine("This command was executed by BusyBox sh (not Bash).")
            }
            appendLine("The script contains Bash-only syntax that BusyBox sh may handle incorrectly.")
            appendLine("Detected:")
            appendLine()
            shown.forEach { hit ->
                appendLine("  - line ${hit.line}: `${sanitize(hit.matchedText)}`")
                appendLine("    rule: ${sanitize(hit.ruleName)} — ${sanitize(hit.behaviorNote)}")
                appendLine("    fix:  ${sanitize(hit.fixHint)}")
            }
            if (overflow > 0) appendLine("  … and $overflow more.")
            appendLine()
            appendLine("Detected lines are quoted from the submitted script for locating only.")
            appendLine("Rewrite using the POSIX forms above and retry with sh, or retry unchanged")
            append("later when Bash may be installable.\n</system-reminder>")
        }
    }
}
