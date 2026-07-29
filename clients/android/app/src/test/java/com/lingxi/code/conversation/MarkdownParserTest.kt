package com.lingxi.code.conversation

import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * Coverage of the PURE Markdown parser ([parseMarkdownBlocks] / [parseInline])
 * powering the assistant `MessageBubble`. No Compose / Android dependency, so it
 * runs on the plain JVM. These assert the block + inline structure the renderer
 * consumes — the renderer itself is thin layout over this model.
 */
class MarkdownParserTest {

    // --- inline -----------------------------------------------------------

    @Test
    fun inline_plainText() {
        assertEquals(listOf(MdInline.Text("hello world")), parseInline("hello world"))
    }

    @Test
    fun inline_bold() {
        assertEquals(
            listOf(MdInline.Text("a "), MdInline.Bold("b"), MdInline.Text(" c")),
            parseInline("a **b** c"),
        )
    }

    @Test
    fun inline_code() {
        assertEquals(
            listOf(MdInline.Text("run "), MdInline.Code("ls -la"), MdInline.Text(" now")),
            parseInline("run `ls -la` now"),
        )
    }

    @Test
    fun inline_codeIsLiteral_starsInsideBackticksAreNotBold() {
        // The `**x**` is inside backticks → it stays literal code, not bold.
        assertEquals(listOf(MdInline.Code("**x**")), parseInline("`**x**`"))
    }

    @Test
    fun inline_unterminatedBold_degradesToLiteral() {
        assertEquals(listOf(MdInline.Text("**oops")), parseInline("**oops"))
    }

    @Test
    fun inline_unterminatedCode_degradesToLiteral() {
        assertEquals(listOf(MdInline.Text("`oops")), parseInline("`oops"))
    }

    @Test
    fun inline_empty_yieldsSingleEmptyText() {
        assertEquals(listOf(MdInline.Text("")), parseInline(""))
    }

    @Test
    fun inline_terminalDeepLink_keepsLingxiProtocolAndLabel() {
        assertEquals(
            listOf(
                MdInline.Text("run "),
                MdInline.Link(
                    label = "in terminal",
                    url = "lingxi://open_terminal?sessionId=s1&initCommand=ls",
                ),
            ),
            parseInline(
                "run [in terminal](lingxi://open_terminal?sessionId=s1&initCommand=ls)",
            ),
        )
    }

    // --- paragraphs -------------------------------------------------------

    @Test
    fun paragraph_single() {
        val blocks = parseMarkdownBlocks("just one line")
        assertEquals(1, blocks.size)
        assertEquals(
            MdBlock.Paragraph(listOf(MdInline.Text("just one line"))),
            blocks[0],
        )
    }

    @Test
    fun paragraphs_separatedByBlankLine() {
        val blocks = parseMarkdownBlocks("first\n\nsecond")
        assertEquals(2, blocks.size)
        assertTrue(blocks[0] is MdBlock.Paragraph)
        assertTrue(blocks[1] is MdBlock.Paragraph)
        assertEquals(listOf(MdInline.Text("second")), (blocks[1] as MdBlock.Paragraph).spans)
    }

    @Test
    fun paragraph_consecutiveLinesCoalesce() {
        val blocks = parseMarkdownBlocks("line one\nline two")
        assertEquals(1, blocks.size)
        assertEquals(
            listOf(MdInline.Text("line one\nline two")),
            (blocks[0] as MdBlock.Paragraph).spans,
        )
    }

    // --- fenced code ------------------------------------------------------

    @Test
    fun codeBlock_withLanguage() {
        val md = "```kotlin\nval x = 1\nprintln(x)\n```"
        val blocks = parseMarkdownBlocks(md)
        assertEquals(1, blocks.size)
        val cb = blocks[0] as MdBlock.CodeBlock
        assertEquals("kotlin", cb.language)
        assertEquals("val x = 1\nprintln(x)", cb.code)
    }

    @Test
    fun codeBlock_noLanguage() {
        val blocks = parseMarkdownBlocks("```\nplain\n```")
        val cb = blocks[0] as MdBlock.CodeBlock
        assertEquals("", cb.language)
        assertEquals("plain", cb.code)
    }

    @Test
    fun codeBlock_starsInsideAreLiteral_notBold() {
        val blocks = parseMarkdownBlocks("```\na = **b**\n```")
        val cb = blocks[0] as MdBlock.CodeBlock
        assertEquals("a = **b**", cb.code)
    }

    @Test
    fun codeBlock_unterminated_consumesToEnd() {
        val blocks = parseMarkdownBlocks("```\nno close")
        val cb = blocks[0] as MdBlock.CodeBlock
        assertEquals("no close", cb.code)
    }

    @Test
    fun codeBlock_betweenParagraphs() {
        val md = "before\n```\ncode\n```\nafter"
        val blocks = parseMarkdownBlocks(md)
        assertEquals(3, blocks.size)
        assertTrue(blocks[0] is MdBlock.Paragraph)
        assertTrue(blocks[1] is MdBlock.CodeBlock)
        assertTrue(blocks[2] is MdBlock.Paragraph)
        assertEquals("code", (blocks[1] as MdBlock.CodeBlock).code)
    }

    // --- bullet lists -----------------------------------------------------

    @Test
    fun bulletList_dash() {
        val blocks = parseMarkdownBlocks("- one\n- two\n- three")
        assertEquals(1, blocks.size)
        val list = blocks[0] as MdBlock.BulletList
        assertEquals(3, list.items.size)
        assertEquals(listOf(MdInline.Text("one")), list.items[0])
        assertEquals(listOf(MdInline.Text("three")), list.items[2])
    }

    @Test
    fun bulletList_starAndPlusMarkers() {
        val star = parseMarkdownBlocks("* a\n* b")[0] as MdBlock.BulletList
        assertEquals(2, star.items.size)
        val plus = parseMarkdownBlocks("+ a\n+ b")[0] as MdBlock.BulletList
        assertEquals(2, plus.items.size)
    }

    @Test
    fun bulletList_itemsCarryInlineFormatting() {
        val list = parseMarkdownBlocks("- run `cmd`\n- be **bold**")[0] as MdBlock.BulletList
        assertEquals(
            listOf(MdInline.Text("run "), MdInline.Code("cmd")),
            list.items[0],
        )
        assertEquals(
            listOf(MdInline.Text("be "), MdInline.Bold("bold")),
            list.items[1],
        )
    }

    // --- numbered lists ---------------------------------------------------

    @Test
    fun numberedList_dotMarker() {
        val blocks = parseMarkdownBlocks("1. first\n2. second\n3. third")
        assertEquals(1, blocks.size)
        val list = blocks[0] as MdBlock.NumberedList
        assertEquals(3, list.items.size)
        assertEquals("1.", list.items[0].marker)
        assertEquals(listOf(MdInline.Text("first")), list.items[0].spans)
        assertEquals("3.", list.items[2].marker)
    }

    @Test
    fun numberedList_parenMarker() {
        val list = parseMarkdownBlocks("1) a\n2) b")[0] as MdBlock.NumberedList
        assertEquals(2, list.items.size)
        // The displayed marker normalizes to "N.".
        assertEquals("1.", list.items[0].marker)
    }

    // --- mixed document ---------------------------------------------------

    @Test
    fun mixedDocument_paragraphCodeAndLists_inOrder() {
        val md = """
            Here is a plan:

            1. install deps
            2. run the build

            ```bash
            ./gradlew assembleDebug
            ```

            Notes:
            - it is **fast**
            - uses `gradle`
        """.trimIndent()

        val blocks = parseMarkdownBlocks(md)
        // Paragraph, NumberedList, CodeBlock, Paragraph, BulletList.
        assertTrue(blocks[0] is MdBlock.Paragraph)
        assertTrue(blocks[1] is MdBlock.NumberedList)
        assertTrue(blocks[2] is MdBlock.CodeBlock)
        assertTrue(blocks[3] is MdBlock.Paragraph)
        assertTrue(blocks[4] is MdBlock.BulletList)

        assertEquals(2, (blocks[1] as MdBlock.NumberedList).items.size)
        assertEquals("./gradlew assembleDebug", (blocks[2] as MdBlock.CodeBlock).code)
        assertEquals(2, (blocks[4] as MdBlock.BulletList).items.size)
    }

    @Test
    fun emptyInput_yieldsNoBlocks() {
        assertEquals(emptyList<MdBlock>(), parseMarkdownBlocks(""))
    }
}
