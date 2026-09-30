package com.lingxi.code.conversation

import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.animateContentSize
import androidx.compose.foundation.BorderStroke
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
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.selection.selectable
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.rounded.Close
import androidx.compose.material.icons.rounded.KeyboardArrowDown
import androidx.compose.material.icons.rounded.KeyboardArrowUp
import androidx.compose.material3.Button
import androidx.compose.material3.Card
import androidx.compose.material3.CardDefaults
import androidx.compose.material3.Checkbox
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.RadioButton
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateMapOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import com.lingxi.code.R
import com.lingxi.code.bindings.AskQuestionDto
import com.lingxi.code.bindings.AskUserQuestionRequestDto

/** Toggle [label] in a question's selection list. */
internal fun toggleAskSelection(
    current: List<String>,
    label: String,
    multiSelect: Boolean,
): List<String> = when {
    multiSelect -> if (label in current) current - label else current + label
    label in current -> emptyList()
    else -> listOf(label)
}

/** Assemble the answer payload expected by AnswerAskUserQuestion. */
internal fun assembleAskAnswers(
    request: AskUserQuestionRequestDto,
    selections: Map<Int, List<String>>,
    freeTexts: Map<Int, String>,
): Map<String, String> = buildMap {
    request.questions.forEachIndexed { index, question ->
        val parts = selections[index].orEmpty() +
            listOfNotNull(freeTexts[index]?.trim()?.takeIf { it.isNotEmpty() })
        if (parts.isNotEmpty()) put(question.question, parts.joinToString(", "))
    }
}

/** True when every question has a selected option or non-blank custom answer. */
internal fun askAnswersComplete(
    request: AskUserQuestionRequestDto,
    selections: Map<Int, List<String>>,
    freeTexts: Map<Int, String>,
): Boolean = request.questions.indices.all { index ->
    selections[index].orEmpty().isNotEmpty() ||
        !freeTexts[index]?.trim().isNullOrEmpty()
}

/**
 * Multi-question AskUserQuestion content shown inside a native bottom sheet.
 * Every question stays visible as an accordion row; completed rows summarize
 * the user's answer and can be reopened to edit it.
 */
@Composable
fun AskUserQuestionCard(
    request: AskUserQuestionRequestDto,
    onSubmit: (requestId: ULong, answers: Map<String, String>) -> Unit,
    onCancel: (requestId: ULong) -> Unit,
    modifier: Modifier = Modifier,
) {
    var expandedQuestion by remember(request.requestId) { mutableIntStateOf(0) }
    val selections = remember(request.requestId) { mutableStateMapOf<Int, List<String>>() }
    val freeTexts = remember(request.requestId) { mutableStateMapOf<Int, String>() }
    val questions = request.questions
    val answeredCount = questions.indices.count { index ->
        selections[index].orEmpty().isNotEmpty() || !freeTexts[index]?.trim().isNullOrEmpty()
    }

    Card(
        shape = RoundedCornerShape(24.dp),
        colors = CardDefaults.cardColors(containerColor = MaterialTheme.colorScheme.surfaceContainerLow),
        elevation = CardDefaults.cardElevation(defaultElevation = 2.dp),
        modifier = modifier.fillMaxWidth(),
    ) {
        Column(
            verticalArrangement = Arrangement.spacedBy(14.dp),
            modifier = Modifier.fillMaxWidth().padding(16.dp),
        ) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                Surface(
                    shape = CircleShape,
                    color = MaterialTheme.colorScheme.secondaryContainer,
                    modifier = Modifier.size(38.dp),
                ) {
                    Box(contentAlignment = Alignment.Center) {
                        Text(
                            text = "?",
                            color = MaterialTheme.colorScheme.onSecondaryContainer,
                            style = MaterialTheme.typography.titleMedium,
                            fontWeight = FontWeight.SemiBold,
                        )
                    }
                }
                Text(
                    text = androidx.compose.ui.res.stringResource(R.string.chat_ask_user_question_title),
                    color = MaterialTheme.colorScheme.onSurface,
                    style = MaterialTheme.typography.titleMedium,
                    fontWeight = FontWeight.SemiBold,
                    modifier = Modifier.weight(1f).padding(start = 10.dp).semantics { heading() },
                )
                if (questions.size > 1) {
                    Text(
                        text = androidx.compose.ui.res.stringResource(
                            R.string.chat_question_answered_progress,
                            answeredCount,
                            questions.size,
                        ),
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                        style = MaterialTheme.typography.labelMedium,
                    )
                }
                IconButton(onClick = { onCancel(request.requestId) }) {
                    Icon(
                        imageVector = Icons.Rounded.Close,
                        contentDescription = androidx.compose.ui.res.stringResource(R.string.common_cancel),
                        tint = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                }
            }

            Column(
                verticalArrangement = Arrangement.spacedBy(8.dp),
                modifier = Modifier.heightIn(max = 420.dp).verticalScroll(rememberScrollState()),
            ) {
                questions.forEachIndexed { index, question ->
                    val answer = answerSummary(index, selections, freeTexts)
                    AskQuestionAccordionRow(
                        index = index,
                        question = question,
                        expanded = expandedQuestion == index,
                        answer = answer,
                        selected = selections[index].orEmpty(),
                        freeText = freeTexts[index].orEmpty(),
                        onExpand = {
                            expandedQuestion = if (expandedQuestion == index) -1 else index
                        },
                        onToggleOption = { label ->
                            val next = toggleAskSelection(
                                selections[index].orEmpty(),
                                label,
                                question.multiSelect,
                            )
                            selections[index] = next
                            if (!question.multiSelect && next.isNotEmpty()) {
                                expandedQuestion = questions.indices.firstOrNull { candidate ->
                                    candidate != index &&
                                        selections[candidate].orEmpty().isEmpty() &&
                                        freeTexts[candidate]?.trim().isNullOrEmpty()
                                } ?: -1
                            }
                        },
                        onFreeTextChange = { freeTexts[index] = it },
                    )
                }
            }

            Button(
                enabled = askAnswersComplete(request, selections, freeTexts),
                onClick = { onSubmit(request.requestId, assembleAskAnswers(request, selections, freeTexts)) },
                modifier = Modifier.fillMaxWidth(),
            ) {
                Text(androidx.compose.ui.res.stringResource(R.string.chat_question_submit))
            }
        }
    }
}

@Composable
private fun AskQuestionAccordionRow(
    index: Int,
    question: AskQuestionDto,
    expanded: Boolean,
    answer: String?,
    selected: List<String>,
    freeText: String,
    onExpand: () -> Unit,
    onToggleOption: (String) -> Unit,
    onFreeTextChange: (String) -> Unit,
) {
    val colors = MaterialTheme.colorScheme
    val shape = RoundedCornerShape(18.dp)
    Column(
        modifier = Modifier
            .fillMaxWidth()
            .animateContentSize(),
    ) {
        Row(
            verticalAlignment = Alignment.CenterVertically,
            modifier = Modifier
                .fillMaxWidth()
                .semantics { heading() }
                .clickable(onClick = onExpand)
                .padding(horizontal = 8.dp, vertical = 10.dp),
        ) {
            Surface(
                shape = CircleShape,
                color = if (answer != null) colors.primaryContainer else colors.surfaceContainerHigh,
                modifier = Modifier.size(40.dp),
            ) {
                Box(contentAlignment = Alignment.Center) {
                    Text(
                        text = (index + 1).toString(),
                        style = MaterialTheme.typography.titleSmall,
                        color = if (answer != null) colors.onPrimaryContainer else colors.onSurfaceVariant,
                        fontWeight = FontWeight.Medium,
                    )
                }
            }
            Column(
                verticalArrangement = Arrangement.spacedBy(3.dp),
                modifier = Modifier.weight(1f).padding(horizontal = 12.dp),
            ) {
                Text(
                    text = question.question,
                    style = MaterialTheme.typography.bodyLarge,
                    color = colors.onSurface,
                    fontWeight = if (expanded) FontWeight.Medium else FontWeight.Normal,
                )
                Text(
                    text = answer ?: androidx.compose.ui.res.stringResource(R.string.chat_question_tap_to_answer),
                    style = MaterialTheme.typography.bodySmall,
                    color = if (answer != null) colors.primary else colors.onSurfaceVariant,
                )
            }
            Icon(
                imageVector = if (expanded) Icons.Rounded.KeyboardArrowUp else Icons.Rounded.KeyboardArrowDown,
                contentDescription = null,
                tint = colors.onSurfaceVariant,
            )
        }

        AnimatedVisibility(visible = expanded) {
            Column(
                verticalArrangement = Arrangement.spacedBy(10.dp),
                modifier = Modifier.fillMaxWidth().padding(start = 50.dp, end = 4.dp, bottom = 10.dp),
            ) {
                question.options.forEach { option ->
                    val active = option.label in selected
                    Surface(
                        shape = shape,
                        color = if (active) colors.secondaryContainer.copy(alpha = 0.7f) else colors.surface,
                        border = BorderStroke(
                            width = 1.dp,
                            color = if (active) colors.primary else colors.outlineVariant,
                        ),
                        modifier = Modifier
                            .fillMaxWidth()
                            .selectable(
                                selected = active,
                                role = if (question.multiSelect) Role.Checkbox else Role.RadioButton,
                                onClick = { onToggleOption(option.label) },
                            ),
                    ) {
                        Row(
                            verticalAlignment = Alignment.CenterVertically,
                            modifier = Modifier.padding(horizontal = 10.dp, vertical = 8.dp),
                        ) {
                            if (question.multiSelect) {
                                Checkbox(checked = active, onCheckedChange = null)
                            } else {
                                RadioButton(selected = active, onClick = null)
                            }
                            Spacer(Modifier.width(8.dp))
                            Column(verticalArrangement = Arrangement.spacedBy(2.dp)) {
                                Text(
                                    text = option.label,
                                    style = MaterialTheme.typography.bodyMedium,
                                    color = colors.onSurface,
                                    fontWeight = FontWeight.Medium,
                                )
                                if (option.description.isNotBlank()) {
                                    Text(
                                        text = option.description,
                                        style = MaterialTheme.typography.bodySmall,
                                        color = colors.onSurfaceVariant,
                                    )
                                }
                            }
                        }
                    }
                }

                Text(
                    text = androidx.compose.ui.res.stringResource(R.string.chat_ask_other_option),
                    style = MaterialTheme.typography.labelLarge,
                    color = colors.onSurfaceVariant,
                )
                OutlinedTextField(
                    value = freeText,
                    onValueChange = onFreeTextChange,
                    placeholder = { Text(androidx.compose.ui.res.stringResource(R.string.chat_question_other_placeholder)) },
                    minLines = 1,
                    maxLines = 3,
                    modifier = Modifier.fillMaxWidth(),
                )
            }
        }
    }
}

private fun answerSummary(
    index: Int,
    selections: Map<Int, List<String>>,
    freeTexts: Map<Int, String>,
): String? {
    val parts = selections[index].orEmpty() +
        listOfNotNull(freeTexts[index]?.trim()?.takeIf { it.isNotEmpty() })
    return parts.takeIf { it.isNotEmpty() }?.joinToString(", ")
}
