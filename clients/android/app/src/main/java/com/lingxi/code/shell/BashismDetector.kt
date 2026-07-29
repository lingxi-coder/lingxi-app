/*
 * Copyright (C) 2026 OpenMinis contributors
 * Copyright (C) 2026 LingXi contributors
 *
 * SPDX-License-Identifier: GPL-3.0-only
 *
 * Adapted from OpenMinis commit 9cf3a855. The matching rules remain in the
 * bundled bashism/bashism_rules.json asset so Android has one rule source.
 */
package com.lingxi.code.shell

import android.content.Context
import java.util.regex.Pattern

fun interface ElapsedClock {
    fun nowMs(): Long
}

private val systemElapsedClock = ElapsedClock { System.nanoTime() / 1_000_000L }

/**
 * Detects shell syntax that requires Bash instead of the Alpine BusyBox shell.
 *
 * Heredoc bodies are deliberately excluded because they are data for another
 * interpreter. Scanning is bounded by both a per-line cap and a monotonic-time
 * fuse so an untrusted command cannot turn rule matching into a CPU sink.
 */
class BashismDetector private constructor(
    private val rules: List<Rule>,
    private val clock: ElapsedClock,
) {
    enum class Tier { S, E, T1 }

    data class Rule(
        val name: String,
        val tier: Tier,
        val regex: Pattern,
        val behaviorNote: String,
        val fixHint: String,
    )

    data class Hit(
        val line: Int,
        val ruleName: String,
        val tier: Tier,
        val matchedText: String,
        val behaviorNote: String,
        val fixHint: String,
    )

    data class Result(
        val hits: List<Hit>,
        val scanTimedOut: Boolean = false,
    ) {
        val needsBash: Boolean get() = hits.isNotEmpty()
        val mustSwitchInterpreter: Boolean get() = hits.any { it.tier == Tier.S || it.tier == Tier.E }
        val hasSilent: Boolean get() = hits.any { it.tier == Tier.S }
    }

    fun rulesByName(): Map<String, Rule> = rules.associateBy(Rule::name)

    fun shellLayerLines(script: String): List<Pair<Int, String?>> {
        val lines = script.split('\n')
        val result = ArrayList<Pair<Int, String?>>(lines.size)
        var index = 0
        while (index < lines.size) {
            val line = lines[index]
            result += (index + 1) to line
            val delimiters = HEREDOC_OPEN.matcher(line).let { matcher ->
                buildList {
                    while (matcher.find()) {
                        matcher.group(1)?.let(::add)
                    }
                }
            }
            index++
            for (delimiter in delimiters) {
                while (index < lines.size && lines[index].trim() != delimiter) {
                    result += (index + 1) to null
                    index++
                }
                if (index < lines.size) {
                    result += (index + 1) to null
                    index++
                }
            }
        }
        return result
    }

    fun detect(
        script: String,
        fuseMs: Long = DEFAULT_FUSE_MS,
    ): Result {
        if (rules.isEmpty()) return Result(emptyList())
        val startedAt = clock.nowMs()
        val hits = ArrayList<Hit>()
        for ((lineNumber, text) in shellLayerLines(script)) {
            if (text.isNullOrEmpty()) continue
            val scan = text.take(MAX_LINE_CHARS)
            for (rule in rules) {
                if (clock.nowMs() - startedAt > fuseMs) {
                    return Result(hits = hits, scanTimedOut = true)
                }
                if (rule.regex.matcher(scan).find()) {
                    hits += Hit(
                        line = lineNumber,
                        ruleName = rule.name,
                        tier = rule.tier,
                        matchedText = scan.trim(),
                        behaviorNote = rule.behaviorNote,
                        fixHint = rule.fixHint,
                    )
                }
            }
        }
        return Result(hits)
    }

    companion object {
        private const val DEFAULT_FUSE_MS = 50L
        private const val MAX_LINE_CHARS = 4_096
        private val HEREDOC_OPEN = Pattern.compile("<<-?\\s*[\"']?(\\w+)[\"']?")

        fun fromAssets(
            context: Context,
            clock: ElapsedClock = systemElapsedClock,
        ): BashismDetector {
            val json = context.assets.open("bashism/bashism_rules.json")
                .bufferedReader()
                .use { it.readText() }
            return fromJson(json, clock)
        }

        fun fromJson(
            json: String,
            clock: ElapsedClock = systemElapsedClock,
        ): BashismDetector = BashismDetector(
            rules = BashismRuleJson.parse(json),
            clock = clock,
        )

        fun fromRules(
            rules: List<Rule>,
            clock: ElapsedClock = systemElapsedClock,
        ): BashismDetector = BashismDetector(rules.toList(), clock)
    }
}
