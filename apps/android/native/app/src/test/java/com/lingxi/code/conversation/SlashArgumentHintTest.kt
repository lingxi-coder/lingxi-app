package com.lingxi.code.conversation

import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.AnnotatedString
import org.json.JSONObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test
import java.io.File

class SlashArgumentHintTest {
    private val reviewHint = "[low|medium|high|xhigh|max|ultra] [--fix] [--comment] [<pr#>|<branch>|<path>] [--max-findings <n>|all]"
    private val commands = listOf(
        ConversationSlashCommand("code-review", listOf("review"), reviewHint),
        ConversationSlashCommand("deploy", argumentHint = "[target] [version]", argumentNames = listOf("target", "version")),
        ConversationSlashCommand("named-only", argumentNames = listOf("first", "second")),
    )

    private fun hint(draft: String, cursor: Int = draft.length, count: Int? = 1): String? =
        slashArgumentHint(draft, cursor, commands) { count }

    @Test fun staticHintPreservesExactBytesAndRequiresOneTrailingSpace() {
        assertEquals(reviewHint, hint("/code-review "))
        assertEquals(reviewHint, hint("/review "))
        for (draft in listOf("/code-review", "/code-review  ", "/code-review high ", "/code-review\t", "/code-review\n")) {
            assertNull(draft, hint(draft))
        }
    }

    @Test fun acceptsOnlyExactCaseSensitiveAsciiCommandTokens() {
        for (draft in listOf(" /code-review ", "/Code-review ", "/code ", "/code-review\t ", "/code-review\u00a0 ", "/未知 ")) {
            assertNull(draft, hint(draft))
        }
        val dotted = listOf(ConversationSlashCommand("plugin:review.v2_test-1", argumentHint = "[input]"))
        assertEquals("[input]", slashArgumentHint("/plugin:review.v2_test-1 ", 1, dotted))
    }

    @Test fun nativeCursorRuleAllowsMiddleButHidesAtStart() {
        assertEquals(reviewHint, hint("/code-review ", cursor = 1))
        assertNull(hint("/code-review ", cursor = 0))
    }

    @Test fun exactPrimaryNameWinsOverAnEarlierAlias() {
        val collisions = listOf(
            ConversationSlashCommand("first", listOf("review"), "[wrong]"),
            ConversationSlashCommand("review", argumentHint = "[right]"),
        )
        assertEquals("[right]", slashArgumentHint("/review ", 8, collisions))
    }

    @Test fun namedHintsUseSharedNativeCountOnlyAfterInitialSpace() {
        var parsed: String? = null
        assertEquals("[version]", slashArgumentHint("/deploy \"two words\" ", 1, commands) {
            parsed = it
            1
        })
        assertEquals("\"two words\" ", parsed)
        assertEquals("[target] [version]", hint("/deploy  ", count = 0))
        assertNull(hint("/deploy one two ", count = 2))
        assertNull(hint("/deploy one", count = 0))
        assertNull(hint("/deploy one ", count = null))
    }

    @Test fun namedArgumentsDoNotRequireAStaticHint() {
        assertEquals("[first] [second]", hint("/named-only ", count = 0))
        assertEquals("[first] [second]", hint("/named-only  ", count = 0))
        assertEquals("[second]", hint("/named-only value ", count = 1))
    }

    @Test fun emptyHintIsAbsentButWhitespaceHintIsPreserved() {
        val empty = listOf(ConversationSlashCommand("empty", argumentHint = ""))
        val whitespace = listOf(ConversationSlashCommand("space", argumentHint = "  "))
        assertNull(slashArgumentHint("/empty ", 7, empty))
        assertEquals("  ", slashArgumentHint("/space ", 7, whitespace))
        assertEquals("[first]", slashArgumentHint(
            "/empty ", 7, listOf(ConversationSlashCommand("empty", argumentHint = "", argumentNames = listOf("first"))),
        ) { 0 })
    }

    @Test fun visualSuffixCannotBeSelectedOrInsertedIntoTheDraft() {
        val draft = AnnotatedString("/code-review ")
        val transformed = SlashArgumentHintTransformation(reviewHint, Color.Gray).filter(draft)
        assertEquals("/code-review $reviewHint", transformed.text.text)
        assertEquals("/code-review ", draft.text)
        for (offset in 0..draft.length) {
            assertEquals(offset, transformed.offsetMapping.originalToTransformed(offset))
            assertEquals(offset, transformed.offsetMapping.transformedToOriginal(offset))
        }
        for (offset in draft.length..transformed.text.length) {
            assertEquals(draft.length, transformed.offsetMapping.transformedToOriginal(offset))
        }
        assertEquals(Color.Gray, transformed.text.spanStyles.single().item.color)
    }

    @Test fun absentVisualSuffixLeavesTextAndOffsetsUnchanged() {
        val draft = AnnotatedString("ordinary text")
        val transformed = SlashArgumentHintTransformation(null, Color.Gray).filter(draft)
        assertEquals(draft, transformed.text)
        assertEquals(draft.length, transformed.offsetMapping.originalToTransformed(draft.length))
    }

    @Test fun narrowComposerClipsHintWithoutGrowingTheDraftLine() {
        val widthOf: (String) -> Int = { it.codePointCount(0, it.length) }
        assertEquals("[target]", clipSlashArgumentHint("[target]", 8, widthOf))
        assertEquals("[ta…", clipSlashArgumentHint("[target]", 4, widthOf))
        assertEquals("…", clipSlashArgumentHint("[target]", 1, widthOf))
        assertNull(clipSlashArgumentHint("[target]", 0, widthOf))
        assertEquals("😀…", clipSlashArgumentHint("😀target", 2, widthOf))
    }

    @Test fun replaysNativeClaudeSelectorFixtureForThePromptComposer() {
        val relative = "packages/bridge-client/test/fixtures/slash-argument-hints-2.1.299.json"
        val file = generateSequence(File(requireNotNull(System.getProperty("user.dir")))) { it.parentFile }
            .map { File(it, relative) }.first { it.isFile }
        val fixture = JSONObject(file.readText())
        fun JSONObject.strings(key: String): List<String> = optJSONArray(key)?.let { values ->
            (0 until values.length()).map(values::getString)
        }.orEmpty()
        val rows = fixture.getJSONArray("commands")
        val catalog = (0 until rows.length()).map { index ->
            val row = rows.getJSONObject(index)
            // The product catalog sends named arguments only for prompt commands.
            ConversationSlashCommand(
                name = row.getString("name"),
                aliases = row.strings("aliases"),
                argumentHint = if (row.has("argumentHint")) row.getString("argumentHint") else null,
                argumentNames = if (row.getString("type") == "prompt") row.strings("argNames") else emptyList(),
            )
        }
        val argumentRows = fixture.getJSONArray("argumentCases")
        val nativeCounts = (0 until argumentRows.length()).associate { index ->
            val row = argumentRows.getJSONObject(index)
            row.getString("input") to row.getInt("count")
        }
        val cases = fixture.getJSONArray("cases")
        for (index in 0 until cases.length()) {
            val row = cases.getJSONObject(index)
            // Bash mode belongs to the terminal; this native Composer is prompt-only.
            if (row.getString("mode") != "prompt") continue
            val input = row.getString("input")
            // The wire catalog has no separate displayName, and the argument
            // oracle has no four-word sample for this injected-count JVM test.
            if (input == "/display.name " || input == "/named a b c d ") continue
            val expected = if (row.isNull("expected")) null else row.getString("expected")
            assertEquals(input, expected, slashArgumentHint(input, row.getInt("cursor"), catalog) { args ->
                nativeCounts.getValue(args)
            })
        }
    }
}
