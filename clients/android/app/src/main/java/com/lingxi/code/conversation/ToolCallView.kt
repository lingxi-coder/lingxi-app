package com.lingxi.code.conversation

import androidx.annotation.StringRes
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.role
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.lingxi.code.R
import com.lingxi.code.components.UiTags
import com.lingxi.code.theme.LingXiTheme

/**
 * One tool call, rendered from the engine's PRE-DERIVED presentation.
 *
 * ```
 * ● Update(src/host.rs)            ← header (localized from verb + primary + qualifier)
 *   $ cargo test --all             ← optional sub-line
 * ⎿ Added 18 lines, removed 4      ← result headline (localized from kind + args)
 *   ┌ the diff, or the plain body ┐
 *   └ Show 42 more lines          ┘
 * ```
 *
 * This composable NEVER touches `input_json` / `result_json`. Everything it
 * shows arrives on the wire already derived once, engine-side; re-deriving it
 * here is the four-way drift this whole change exists to delete. The one
 * exception is [ToolCallUi.fallbackSummary] — the OLD `summarizeToolInput`
 * one-liner, rendered only when an older engine sent no `header` at all.
 *
 * ### Collapse state is NOT held here
 *
 * [expanded] is passed in and [onToggleExpanded] is hoisted all the way to
 * `ChatState.expandedToolCalls`. Both surfaces that render this — the transcript
 * `LazyColumn` and the run timeline's row list — RECYCLE their rows, so a
 * `rememberSaveable` here would silently lose the user's expansion the moment
 * the row scrolled out of view.
 */
@Composable
internal fun ToolCallView(
    call: ToolCallUi,
    expanded: Boolean,
    onToggleExpanded: () -> Unit,
    modifier: Modifier = Modifier,
    /** Trailing metadata for the LIVE timeline (elapsed time). Null in the transcript. */
    trailing: String? = null,
) {
    val t = LingXiTheme.palette
    val display = call.display
    val title = toolCallTitle(call)
    val statusLabel = toolCallStatusLabel(call.status)
    // `collapsed` is the ENGINE's verdict that the body exceeds the inline
    // budget. A body it did not mark collapsed renders inline with no toggle.
    val collapsible = display != null && display.collapsed && display.hasExpandableContent
    val showsContent = display != null &&
        display.hasExpandableContent &&
        (!display.collapsed || expanded)

    Column(
        modifier = modifier.fillMaxWidth(),
        verticalArrangement = Arrangement.spacedBy(3.dp),
    ) {
        Row(
            // The status is a colored DOT, which is invisible to a screen reader —
            // so the row announces the status word alongside the title.
            modifier = Modifier
                .fillMaxWidth()
                .semantics(mergeDescendants = true) {
                    contentDescription = "$statusLabel $title"
                },
            verticalAlignment = Alignment.Top,
        ) {
            Box(
                modifier = Modifier
                    .padding(top = 6.dp)
                    .size(7.dp)
                    .background(toolCallStatusColor(call.status), CircleShape),
            )
            Spacer(Modifier.width(9.dp))
            Column(modifier = Modifier.weight(1f)) {
                Text(
                    text = title,
                    color = t.text,
                    fontSize = 13.5f.sp,
                    fontWeight = FontWeight.Medium,
                    maxLines = 2,
                    overflow = TextOverflow.Ellipsis,
                )
                call.header?.subLine?.let { sub ->
                    Text(
                        text = "${sub.prefix} ${sub.text}",
                        color = t.text3,
                        fontSize = 12.5f.sp,
                        fontFamily = FontFamily.Monospace,
                        maxLines = 2,
                        overflow = TextOverflow.Ellipsis,
                    )
                }
                // Older engine, no header: the legacy one-line input summary is
                // all we have. Kept deliberately as the compatibility floor.
                if (call.header == null) {
                    call.fallbackSummary?.let {
                        Text(
                            text = it,
                            color = t.text3,
                            fontSize = 12.5f.sp,
                            fontFamily = FontFamily.Monospace,
                            maxLines = 3,
                            overflow = TextOverflow.Ellipsis,
                        )
                    }
                }
            }
            trailing?.let {
                Spacer(Modifier.width(8.dp))
                Text(
                    text = it,
                    color = toolCallStatusColor(call.status),
                    fontSize = 11.5f.sp,
                )
            }
        }

        val headline = display?.let { resultHeadline(it) }
        if (headline != null) {
            Row(verticalAlignment = Alignment.Top) {
                Text(
                    text = "⎿",
                    color = t.text4,
                    fontSize = 12.5f.sp,
                    fontFamily = FontFamily.Monospace,
                    modifier = Modifier.width(16.dp),
                )
                Text(
                    text = headline,
                    color = if (call.status == AgentToolStatus.Failed) t.danger else t.text2,
                    fontSize = 12.5f.sp,
                    modifier = Modifier.weight(1f),
                )
            }
        }

        if (showsContent && display != null) {
            Column(
                modifier = Modifier.padding(start = 16.dp),
                verticalArrangement = Arrangement.spacedBy(4.dp),
            ) {
                display.diff?.takeIf { it.rows.isNotEmpty() }?.let { DiffView(diff = it) }
                display.body?.takeIf { it.isNotEmpty() }?.let { body ->
                    Text(
                        text = body,
                        color = t.text2,
                        fontSize = 12.sp,
                        lineHeight = 17.sp,
                        fontFamily = FontFamily.Monospace,
                        modifier = Modifier
                            .fillMaxWidth()
                            .clip(RoundedCornerShape(8.dp))
                            .background(t.diffSurface)
                            .padding(horizontal = 10.dp, vertical = 7.dp),
                    )
                }
                if (display.bodyTruncated) {
                    Text(
                        text = stringResource(R.string.chat_tool_body_truncated),
                        color = t.text4,
                        fontSize = 11.5f.sp,
                    )
                }
            }
        }

        if (collapsible && display != null) {
            // `bodyLines` is the count BEFORE clamping, so the affordance can say
            // how much is hidden without this side measuring anything.
            val hiddenLines = when {
                !display.body.isNullOrEmpty() -> display.bodyLines
                else -> display.diff?.rows?.size ?: 0
            }
            Text(
                text = if (expanded) {
                    stringResource(R.string.chat_tool_show_less)
                } else {
                    stringResource(R.string.chat_tool_show_more_label, hiddenLines)
                },
                color = t.accent,
                fontSize = 12.sp,
                modifier = Modifier
                    .padding(start = 16.dp)
                    .clip(RoundedCornerShape(6.dp))
                    .clickable(onClick = onToggleExpanded)
                    .semantics { role = Role.Button }
                    .testTag(UiTags.TOOL_CALL_TOGGLE)
                    .heightIn(min = 32.dp)
                    .padding(horizontal = 6.dp, vertical = 7.dp),
            )
        }
    }
}

/**
 * The localized header line — `修改(src/host.rs)`.
 *
 * Composed from [toolVerbLabel] + `primary` + `qualifier`, NOT from the
 * pre-composed English [ToolHeaderUi.title]: `title` exists for surfaces that do
 * not localize.
 */
@Composable
internal fun toolCallTitle(call: ToolCallUi): String {
    val header = call.header ?: return call.tool
    return composeToolTitle(
        label = toolVerbLabel(header),
        primary = header.primary,
        qualifier = header.qualifier,
    )
}

/**
 * The localized verb — or the engine's OVERRIDE, verbatim, when it set one.
 *
 * The engine picks a label in this precedence (`tool_display/header.rs`): an
 * arm's explicit override, else the verb's English label, else the raw tool
 * name. An override is a proper noun no catalog key can express — `REPL`,
 * `Web Search`, a subagent type — and translating it away is a WRONG label, not
 * a missing translation: `WebSearch` and `WebFetch` share [ToolVerbUi.Fetch] and
 * are told apart by nothing else, and a `Task` loses which agent ran.
 *
 * So: a label equal to the verb's canonical English is the engine's default and
 * gets localized; anything else survives as sent. [ToolVerbUi.Generic] has no
 * canonical English by design (its label IS the tool name) and [ToolVerbUi.Shell]
 * is decided by [ToolHeaderUi.count] instead — see below.
 *
 * Mirrors iOS `ToolDisplayText.verbLabel`. Both sides hand-copy [canonicalVerbEnglish]
 * from `ToolVerb::english()`; an engine-side `label_overridden` flag on the wire
 * would delete both copies, and is the better fix once that DTO can change.
 */
@Composable
internal fun toolVerbLabel(header: ToolHeaderUi): String {
    val key = toolVerbLabelRes(header) ?: return header.label
    val count = header.count
    return if (header.verb == ToolVerbUi.Shell && count != null) {
        stringResource(key, count)
    } else {
        stringResource(key)
    }
}

/**
 * The catalog key [toolVerbLabel] should render, or `null` when the engine's
 * own [ToolHeaderUi.label] must be shown verbatim instead.
 *
 * PURE, so the whole decision — which is the part that can be WRONG — is
 * testable on the plain JVM without a Compose runtime.
 */
@StringRes
internal fun toolVerbLabelRes(header: ToolHeaderUi): Int? = when (header.verb) {
    // `Bash`/`Shell`/`PowerShell` carry a count; `REPL` shares the verb, sets no
    // count and overrides the label. `count ?: 1` used to fabricate
    // "Running 1 shell command…" for a call that ran no shell command at all.
    ToolVerbUi.Shell -> R.string.chat_tool_verb_shell_label.takeIf { header.count != null }
    // No catalog key by design — the raw tool name IS the label.
    ToolVerbUi.Generic -> null
    else -> verbCatalogKey(header.verb)?.takeIf { header.label == canonicalVerbEnglish(header.verb) }
}

/**
 * The verb's own English label, mirroring Rust `ToolVerb::english()`.
 *
 * `null` where the engine has no canonical English to compare against:
 * [ToolVerbUi.Generic] (the tool name is the label) and [ToolVerbUi.Shell]
 * (whose `Bash` arm overrides the label as well, so [toolVerbLabelRes] keys off
 * `count` instead). `null` never equals a non-null `label`, so both fall
 * through to the engine's string.
 */
internal fun canonicalVerbEnglish(verb: ToolVerbUi): String? = when (verb) {
    ToolVerbUi.Update -> "Update"
    ToolVerbUi.Create -> "Write"
    ToolVerbUi.Read -> "Read"
    ToolVerbUi.Search -> "Search"
    ToolVerbUi.Output -> "Output"
    ToolVerbUi.Kill -> "Kill"
    ToolVerbUi.Fetch -> "Fetch"
    ToolVerbUi.Task -> "Task"
    ToolVerbUi.Todo -> "Update Todos"
    ToolVerbUi.Skill -> "Skill"
    ToolVerbUi.Shell, ToolVerbUi.Generic -> null
}

/**
 * The catalog entry for a verb the engine did NOT override. `null` for the two
 * verbs that have none — [ToolVerbUi.Shell] is COUNTED (its key takes a `%d`
 * argument, so it may only be resolved through the counted branch above) and
 * [ToolVerbUi.Generic] has no catalog entry at all.
 */
@StringRes
private fun verbCatalogKey(verb: ToolVerbUi): Int? = when (verb) {
    ToolVerbUi.Update -> R.string.chat_tool_verb_update
    ToolVerbUi.Create -> R.string.chat_tool_verb_create
    ToolVerbUi.Read -> R.string.chat_tool_verb_read
    ToolVerbUi.Search -> R.string.chat_tool_verb_search
    ToolVerbUi.Output -> R.string.chat_tool_verb_output
    ToolVerbUi.Kill -> R.string.chat_tool_verb_kill
    ToolVerbUi.Fetch -> R.string.chat_tool_verb_fetch
    ToolVerbUi.Task -> R.string.chat_tool_verb_task
    ToolVerbUi.Todo -> R.string.chat_tool_verb_todo
    ToolVerbUi.Skill -> R.string.chat_tool_verb_skill
    ToolVerbUi.Shell, ToolVerbUi.Generic -> null
}

/** `label(primary)qualifier` — the same shape the engine's `title` uses. PURE. */
internal fun composeToolTitle(label: String, primary: String?, qualifier: String?): String =
    buildString {
        append(label)
        if (!primary.isNullOrEmpty()) {
            append('(')
            append(primary)
            append(')')
        }
        if (!qualifier.isNullOrEmpty()) append(qualifier)
    }

/**
 * The localized `⎿` headline.
 *
 * [ToolResultDisplayUi.headlineKind] + [ToolResultDisplayUi.headlineArgs] are the
 * localizable form; the English [ToolResultDisplayUi.headline] is the fallback
 * and, for [HeadlineKindUi.Failed] / [HeadlineKindUi.Plain], the ONLY form —
 * those two carry a free-text message that has no catalog key.
 */
@Composable
internal fun resultHeadline(display: ToolResultDisplayUi): String? {
    val kind = display.headlineKind ?: return display.headline
    fun arg(index: Int): Int = display.headlineArgs.getOrElse(index) { 0 }
    return when (kind) {
        HeadlineKindUi.Added -> stringResource(R.string.chat_result_added_label, arg(0))
        HeadlineKindUi.Removed -> stringResource(R.string.chat_result_removed_label, arg(0))
        HeadlineKindUi.AddedRemoved ->
            stringResource(R.string.chat_result_added_removed_label, arg(0), arg(1))
        HeadlineKindUi.LinesRead -> stringResource(R.string.chat_result_lines_read_label, arg(0))
        HeadlineKindUi.LinesReadPartial ->
            stringResource(R.string.chat_result_lines_read_partial_label, arg(0), arg(1))
        HeadlineKindUi.FilesFound -> stringResource(R.string.chat_result_files_found_label, arg(0))
        HeadlineKindUi.FilesFoundTruncated ->
            stringResource(R.string.chat_result_files_found_truncated_label, arg(0))
        HeadlineKindUi.LinesFound -> stringResource(R.string.chat_result_lines_found_label, arg(0))
        HeadlineKindUi.MatchesFound ->
            stringResource(R.string.chat_result_matches_found_label, arg(0))
        HeadlineKindUi.Interrupted -> stringResource(R.string.chat_result_interrupted)
        HeadlineKindUi.NoContent -> stringResource(R.string.chat_result_no_content)
        // Free text from the engine — no catalog key exists, by design.
        HeadlineKindUi.Failed, HeadlineKindUi.Plain -> display.headline
    }
}

/** The spoken status word — the colored dot alone tells a screen reader nothing. */
@Composable
internal fun toolCallStatusLabel(status: AgentToolStatus): String = when (status) {
    AgentToolStatus.Running -> stringResource(R.string.chat_status_running)
    AgentToolStatus.Completed -> stringResource(R.string.chat_tool_status_completed)
    AgentToolStatus.Failed -> stringResource(R.string.chat_status_failed)
    AgentToolStatus.Cancelled -> stringResource(R.string.chat_status_cancelled)
}

@Composable
internal fun toolCallStatusColor(status: AgentToolStatus): Color {
    val t = LingXiTheme.palette
    return when (status) {
        AgentToolStatus.Running -> t.accent
        AgentToolStatus.Completed -> t.ok
        AgentToolStatus.Failed -> t.danger
        AgentToolStatus.Cancelled -> t.text4
    }
}
