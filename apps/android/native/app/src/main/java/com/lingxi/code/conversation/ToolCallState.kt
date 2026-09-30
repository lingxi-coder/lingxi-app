package com.lingxi.code.conversation

import androidx.compose.runtime.Immutable
import com.lingxi.code.bindings.client.CodeSegmentDto
import com.lingxi.code.bindings.client.DiffLineKindDto
import com.lingxi.code.bindings.client.DiffRowDto
import com.lingxi.code.bindings.client.HeadlineKindDto
import com.lingxi.code.bindings.client.PlanTaskDto
import com.lingxi.code.bindings.client.PlanTaskStateDto
import com.lingxi.code.bindings.client.StructuredDiffDto
import com.lingxi.code.bindings.client.SyntaxClassDto
import com.lingxi.code.bindings.client.ToolHeaderDto
import com.lingxi.code.bindings.client.ToolResultDisplayDto
import com.lingxi.code.bindings.client.ToolSubLineDto
import com.lingxi.code.bindings.client.ToolVerbDto

/**
 * The client-side render model for a tool call's PRE-DERIVED presentation.
 *
 * The engine now derives — ONCE, in `tui-core::tool_display` — how every tool
 * call should be presented, and ships it on additive wire fields
 * (`ToolUseStarted.header`, `ToolUseResult.display`, `MessageBlockDto.ToolUse.
 * header`, `MessageBlockDto.ToolResult.display`). This file is the Android half:
 * plain Kotlin mirrors of those DTOs plus the DTO→model mappers.
 *
 * NOTHING here re-parses `input_json` / `result_json` to rebuild a header. That
 * four-way drift (TUI / iOS / Android / Electron each summarizing tool JSON its
 * own way) is exactly what the shared derivation deletes. [summarizeToolInput]
 * survives ONLY as the fallback for an older engine that sends no `header`.
 *
 * Deliberately Android-free apart from the `@Immutable` stability annotations
 * (a plain JVM-resolvable Compose annotation, no runtime), so every mapper is
 * exercised on the plain JVM next to `ClientEventMapperTest` — see
 * `ToolCallStateTest`.
 *
 * ### Three contracts that are easy to break
 *
 *  1. **Segments are PRE-SPLIT — never index into a row's string.** Rust indexes
 *     by UTF-8 byte, Kotlin by UTF-16 code unit; that mismatch is precisely why
 *     no offsets cross the wire. Concatenating [DiffRowUi.segments] in order
 *     reproduces the line EXACTLY ([DiffRowUi.text]), and attributed text is
 *     built by APPENDING segments, never by styling a range.
 *  2. **Color from [CodeSegmentUi.syntax], not [CodeSegmentUi.rgb].** `rgb` is
 *     baked against ONE dark terminal theme; painted on a light background it is
 *     unreadable and it cannot follow a theme toggle. See `theme/SyntaxPalette`.
 *  3. **Diff backgrounds are NOT on the wire, deliberately** — the terminal's
 *     are alpha-over-black blends valid only over a black terminal. They are
 *     derived client-side from [DiffRowUi.kind] and [CodeSegmentUi.emph].
 */

// MARK: - Header -----------------------------------------------------------

/**
 * Stable, non-localized verb identity for a tool-call header. The localized
 * label is looked up from this (`chat_tool_verb_*`), NOT from the English
 * [ToolHeaderUi.label] — which exists for surfaces that do not localize.
 */
enum class ToolVerbUi {
    Update,
    Create,
    Read,
    Search,

    /** Counted: its label takes [ToolHeaderUi.count] (`chat_tool_verb_shell_label`). */
    Shell,
    Output,
    Kill,
    Fetch,
    Task,
    Todo,
    Skill,

    /** No verb table entry — the raw tool name in [ToolHeaderUi.label] IS the label. */
    Generic,
}

/** A second header line with its own glyph, e.g. `$ cargo test --all`. */
@Immutable
data class ToolSubLineUi(val prefix: String, val text: String)

/** The parameterized tool-call header — `Update(src/host.rs)`. */
@Immutable
data class ToolHeaderUi(
    val verb: ToolVerbUi,
    /** English label. Localizing clients key off [verb] instead — except [ToolVerbUi.Generic]. */
    val label: String,
    val primary: String? = null,
    val qualifier: String? = null,
    val count: Int? = null,
    val subLine: ToolSubLineUi? = null,
    /** Pre-composed English `label(primary)qualifier`, for non-localizing surfaces. */
    val title: String,
)

// MARK: - Diff -------------------------------------------------------------

/** Syntax class of one pre-split run. Maps to a palette entry, never to `rgb`. */
enum class SyntaxClassUi {
    Plain,
    Keyword,
    TypeName,
    Function,
    StringLit,
    Number,
    Comment,
    Punctuation,
    Operator,
    Variable,
    Constant,
    Attribute,
}

/** One pre-split run of a diff row's text. */
@Immutable
data class CodeSegmentUi(
    val text: String,
    val syntax: SyntaxClassUi,
    /**
     * Terminal-resolved foreground packed `0x00RRGGBB`, or `null` for the
     * terminal default. Baked against a DARK theme — use only as the dark-mode
     * fallback for [SyntaxClassUi.Plain]. See `theme/SyntaxPalette`.
     */
    val rgb: Int? = null,
    val bold: Boolean = false,
    val italic: Boolean = false,
    val underline: Boolean = false,
    /** A changed word of a word-diffed pair — render the stronger intra-line background. */
    val emph: Boolean = false,
)

enum class DiffLineKindUi { Add, Remove, Context }

/** One diff row: gutter metadata plus its pre-split content runs. */
@Immutable
data class DiffRowUi(
    val kind: DiffLineKindUi,
    /** New-file line number for add/context; old-file for remove. */
    val lineNo: Int,
    /** 0-based hunk index. A CHANGE between consecutive rows is where `⋯` belongs. */
    val hunk: Int,
    val wordDiffed: Boolean,
    val segments: List<CodeSegmentUi>,
) {
    /** The row's exact text — concatenating the pre-split runs, never indexing. */
    val text: String get() = segments.joinToString(separator = "") { it.text }
}

/** A complete structured diff. */
@Immutable
data class StructuredDiffUi(
    val filePath: String? = null,
    val language: String? = null,
    /** Width of the right-aligned line-number gutter, across ALL hunks. */
    val gutterWidth: Int,
    val additions: Int,
    val removals: Int,
    /** Rows dropped by the wire cap; `0` when complete. */
    val truncatedRows: Int,
    val rows: List<DiffRowUi>,
)

// MARK: - Result -----------------------------------------------------------

/**
 * What a result headline says, for clients that localize. The numeric slots in
 * [ToolResultDisplayUi.headlineArgs] are ordered exactly as each kind documents.
 *
 * [Failed] and [Plain] carry their message in [ToolResultDisplayUi.headline]
 * itself — there is no catalog key for them, so it renders verbatim.
 */
enum class HeadlineKindUi {
    /** `args = [additions]`. */
    Added,

    /** `args = [removals]`. */
    Removed,

    /** `args = [additions, removals]`. */
    AddedRemoved,

    /** `args = [read]`. */
    LinesRead,

    /** `args = [read, total]`. */
    LinesReadPartial,

    /** `args = [n]`. */
    FilesFound,

    /** `args = [n]`. */
    FilesFoundTruncated,

    /** `args = [n]`. */
    LinesFound,

    /** `args = [n]`. */
    MatchesFound,
    Interrupted,
    NoContent,

    /** The message is in `headline`. */
    Failed,

    /** Free text; the message is in `headline`. */
    Plain,
}

/** Everything needed to render one completed call's `⎿` block. */
@Immutable
data class ToolResultDisplayUi(
    /** ENGLISH headline. Localizing clients compose from [headlineKind] + [headlineArgs]. */
    val headline: String? = null,
    val headlineKind: HeadlineKindUi? = null,
    val headlineArgs: List<Int> = emptyList(),
    val diff: StructuredDiffUi? = null,
    /** Plain-text body for the expanded view, ALREADY clamped to the wire caps. */
    val body: String? = null,
    /** Line count BEFORE clamping — drives "show N more lines" with no measuring. */
    val bodyLines: Int = 0,
    /** [body] was clamped; the full text remains in `result_json`. */
    val bodyTruncated: Boolean = false,
    /** The body exceeds the inline budget — render it collapsed by default. */
    val collapsed: Boolean = false,
) {
    /** True when there is anything to reveal behind a collapse toggle. */
    val hasExpandableContent: Boolean
        get() = !body.isNullOrEmpty() || (diff?.rows?.isNotEmpty() == true)
}

// MARK: - Plan -------------------------------------------------------------

enum class PlanTaskStateUi {
    Pending,
    InProgress,
    Completed,
    ;

    /** The checklist glyph. Data only — color and strikethrough stay with the renderer. */
    val glyph: String
        get() = when (this) {
            Pending -> "◻"
            InProgress -> "◼"
            Completed -> "✔"
        }
}

/** One item of the model-managed working plan. */
@Immutable
data class PlanTaskUi(
    /** Stable V2 task id. TodoWrite V1 items have none. */
    val id: String? = null,
    val subject: String,
    /** Present-continuous label, for the status line — not the list row. */
    val activeForm: String? = null,
    val state: PlanTaskStateUi,
)

/** Maximum plan rows shown before the compact overflow line. */
const val MAX_VISIBLE_PLAN_TASKS: Int = 5

/**
 * The visible plan rows plus the per-state counts of the HIDDEN remainder.
 *
 * Mirrors `tui_core::tool_display::plan::overflow_summary`: the visible window
 * is simply the first [MAX_VISIBLE_PLAN_TASKS] rows in wire order, and the
 * overflow clause counts only what is hidden, ordered in-progress → pending →
 * completed with zero clauses omitted.
 */
@Immutable
data class PlanWindow(
    val visible: List<PlanTaskUi>,
    val hiddenInProgress: Int,
    val hiddenPending: Int,
    val hiddenCompleted: Int,
) {
    val hiddenCount: Int get() = hiddenInProgress + hiddenPending + hiddenCompleted
    val hasOverflow: Boolean get() = hiddenCount > 0
}

/** Split [tasks] into the visible window and the hidden per-state counts. PURE. */
fun planWindow(tasks: List<PlanTaskUi>, max: Int = MAX_VISIBLE_PLAN_TASKS): PlanWindow {
    val visible = if (max <= 0) emptyList() else tasks.take(max)
    val hidden = if (max <= 0) tasks else tasks.drop(max)
    return PlanWindow(
        visible = visible,
        hiddenInProgress = hidden.count { it.state == PlanTaskStateUi.InProgress },
        hiddenPending = hidden.count { it.state == PlanTaskStateUi.Pending },
        hiddenCompleted = hidden.count { it.state == PlanTaskStateUi.Completed },
    )
}

// MARK: - One rendered call -------------------------------------------------

/**
 * One tool call as the UI renders it: the pre-derived [header], the pre-derived
 * result [display], and the lifecycle [status] the status dot reads.
 *
 * [id] is the engine's stable tool-use id and is the ONLY collapse key — see
 * `ChatState.expandedToolCalls`, which lives in the ViewModel precisely because
 * both lists that render these recycle their rows.
 *
 * [fallbackSummary] is the OLD `summarizeToolInput` one-liner, kept so an engine
 * that predates `header` still renders something.
 */
@Immutable
data class ToolCallUi(
    val id: String,
    val tool: String,
    val header: ToolHeaderUi? = null,
    val display: ToolResultDisplayUi? = null,
    val status: AgentToolStatus = AgentToolStatus.Running,
    val fallbackSummary: String? = null,
    val planMarkdown: String? = null,
)

/** One ordered piece of a settled transcript message. */
sealed interface MessageContent {
    /** Assistant/user prose (or a thinking / boundary marker), rendered as Markdown. */
    @Immutable
    data class Text(val text: String) : MessageContent

    /** A tool call — header, `⎿` headline, and its collapsible body/diff. */
    @Immutable
    data class Tool(val call: ToolCallUi) : MessageContent
}

// MARK: - DTO → model -------------------------------------------------------

fun ToolVerbDto.toUi(): ToolVerbUi = when (this) {
    ToolVerbDto.UPDATE -> ToolVerbUi.Update
    ToolVerbDto.CREATE -> ToolVerbUi.Create
    ToolVerbDto.READ -> ToolVerbUi.Read
    ToolVerbDto.SEARCH -> ToolVerbUi.Search
    ToolVerbDto.SHELL -> ToolVerbUi.Shell
    ToolVerbDto.OUTPUT -> ToolVerbUi.Output
    ToolVerbDto.KILL -> ToolVerbUi.Kill
    ToolVerbDto.FETCH -> ToolVerbUi.Fetch
    ToolVerbDto.TASK -> ToolVerbUi.Task
    ToolVerbDto.TODO -> ToolVerbUi.Todo
    ToolVerbDto.SKILL -> ToolVerbUi.Skill
    ToolVerbDto.GENERIC -> ToolVerbUi.Generic
}

fun ToolSubLineDto.toUi(): ToolSubLineUi = ToolSubLineUi(prefix = prefix, text = text)

fun ToolHeaderDto.toUi(): ToolHeaderUi = ToolHeaderUi(
    verb = verb.toUi(),
    label = label,
    primary = primary,
    qualifier = qualifier,
    count = count?.toInt(),
    subLine = subLine?.toUi(),
    title = title,
)

fun SyntaxClassDto.toUi(): SyntaxClassUi = when (this) {
    SyntaxClassDto.PLAIN -> SyntaxClassUi.Plain
    SyntaxClassDto.KEYWORD -> SyntaxClassUi.Keyword
    SyntaxClassDto.TYPE_NAME -> SyntaxClassUi.TypeName
    SyntaxClassDto.FUNCTION -> SyntaxClassUi.Function
    SyntaxClassDto.STRING_LIT -> SyntaxClassUi.StringLit
    SyntaxClassDto.NUMBER -> SyntaxClassUi.Number
    SyntaxClassDto.COMMENT -> SyntaxClassUi.Comment
    SyntaxClassDto.PUNCTUATION -> SyntaxClassUi.Punctuation
    SyntaxClassDto.OPERATOR -> SyntaxClassUi.Operator
    SyntaxClassDto.VARIABLE -> SyntaxClassUi.Variable
    SyntaxClassDto.CONSTANT -> SyntaxClassUi.Constant
    SyntaxClassDto.ATTRIBUTE -> SyntaxClassUi.Attribute
}

fun CodeSegmentDto.toUi(): CodeSegmentUi = CodeSegmentUi(
    text = text,
    syntax = `class`.toUi(),
    rgb = rgb?.toInt(),
    bold = bold,
    italic = italic,
    underline = underline,
    emph = emph,
)

fun DiffLineKindDto.toUi(): DiffLineKindUi = when (this) {
    DiffLineKindDto.ADD -> DiffLineKindUi.Add
    DiffLineKindDto.REMOVE -> DiffLineKindUi.Remove
    DiffLineKindDto.CONTEXT -> DiffLineKindUi.Context
}

fun DiffRowDto.toUi(): DiffRowUi = DiffRowUi(
    kind = kind.toUi(),
    lineNo = lineNo.toInt(),
    hunk = hunk.toInt(),
    wordDiffed = wordDiffed,
    segments = segments.map { it.toUi() },
)

fun StructuredDiffDto.toUi(): StructuredDiffUi = StructuredDiffUi(
    filePath = filePath,
    language = language,
    gutterWidth = gutterWidth.toInt(),
    additions = additions.toInt(),
    removals = removals.toInt(),
    truncatedRows = truncatedRows.toInt(),
    rows = rows.map { it.toUi() },
)

fun HeadlineKindDto.toUi(): HeadlineKindUi = when (this) {
    HeadlineKindDto.ADDED -> HeadlineKindUi.Added
    HeadlineKindDto.REMOVED -> HeadlineKindUi.Removed
    HeadlineKindDto.ADDED_REMOVED -> HeadlineKindUi.AddedRemoved
    HeadlineKindDto.LINES_READ -> HeadlineKindUi.LinesRead
    HeadlineKindDto.LINES_READ_PARTIAL -> HeadlineKindUi.LinesReadPartial
    HeadlineKindDto.FILES_FOUND -> HeadlineKindUi.FilesFound
    HeadlineKindDto.FILES_FOUND_TRUNCATED -> HeadlineKindUi.FilesFoundTruncated
    HeadlineKindDto.LINES_FOUND -> HeadlineKindUi.LinesFound
    HeadlineKindDto.MATCHES_FOUND -> HeadlineKindUi.MatchesFound
    HeadlineKindDto.INTERRUPTED -> HeadlineKindUi.Interrupted
    HeadlineKindDto.NO_CONTENT -> HeadlineKindUi.NoContent
    HeadlineKindDto.FAILED -> HeadlineKindUi.Failed
    HeadlineKindDto.PLAIN -> HeadlineKindUi.Plain
}

fun ToolResultDisplayDto.toUi(): ToolResultDisplayUi = ToolResultDisplayUi(
    headline = headline,
    headlineKind = headlineKind?.toUi(),
    headlineArgs = headlineArgs.map { it.toInt() },
    diff = diff?.toUi(),
    body = body,
    bodyLines = bodyLines.toInt(),
    bodyTruncated = bodyTruncated,
    collapsed = collapsed,
)

fun PlanTaskStateDto.toUi(): PlanTaskStateUi = when (this) {
    PlanTaskStateDto.PENDING -> PlanTaskStateUi.Pending
    PlanTaskStateDto.IN_PROGRESS -> PlanTaskStateUi.InProgress
    PlanTaskStateDto.COMPLETED -> PlanTaskStateUi.Completed
}

fun PlanTaskDto.toUi(): PlanTaskUi = PlanTaskUi(
    id = id,
    subject = subject,
    activeForm = activeForm,
    state = state.toUi(),
)
