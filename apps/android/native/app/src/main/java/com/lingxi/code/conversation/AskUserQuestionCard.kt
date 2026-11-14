package com.lingxi.code.conversation

import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.animateContentSize
import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.defaultMinSize
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.OutlinedButton
import androidx.compose.material.icons.rounded.ArrowForward
import androidx.compose.material.icons.rounded.Edit
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
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
import androidx.compose.foundation.selection.selectableGroup
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.rounded.Close
import androidx.compose.material.icons.rounded.KeyboardArrowDown
import androidx.compose.material.icons.rounded.KeyboardArrowUp
import androidx.compose.material3.Button
import androidx.compose.material3.Card
import androidx.compose.material3.CardDefaults
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
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
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import com.lingxi.code.R
import com.lingxi.code.bindings.client.AskQuestionDto
import com.lingxi.code.bindings.client.AskUserQuestionAnswerDto
import com.lingxi.code.bindings.client.AskUserQuestionAnswersDto
import com.lingxi.code.bindings.client.AskUserQuestionRequestDto
import com.lingxi.code.bindings.client.ClientTextDto

/** Toggle an option index in a question's selection list. */
internal fun toggleAskSelection(
    current: List<Int>,
    optionIndex: Int,
    multiSelect: Boolean,
): List<Int> = when {
    multiSelect -> if (optionIndex in current) current - optionIndex else current + optionIndex
    optionIndex in current -> emptyList()
    else -> listOf(optionIndex)
}

/** Assemble the answer payload expected by AnswerAskUserQuestion. */
internal fun assembleAskAnswers(
    request: AskUserQuestionRequestDto,
    selections: Map<Int, List<Int>>,
    freeTexts: Map<Int, String>,
): AskUserQuestionAnswersDto {
    val pairs = request.questions.mapIndexedNotNull { index, question ->
        val parts = selections[index].orEmpty().mapNotNull { optionIndex ->
            question.options.getOrNull(optionIndex)?.label?.codeUnits
        }.toMutableList()
        freeTexts[index]?.trim()?.takeIf { it.isNotEmpty() }?.let {
            parts += it.toClientTextDto().codeUnits
        }
        if (parts.isEmpty()) return@mapIndexedNotNull null
        val joined = mutableListOf<UShort>()
        parts.forEachIndexed { partIndex, units ->
            if (partIndex > 0) joined += listOf(','.code.toUShort(), ' '.code.toUShort())
            joined += units
        }
        AskUserQuestionAnswerDto(
            question = question.question,
            answer = ClientTextDto(codeUnits = joined),
        )
    }
    return AskUserQuestionAnswersDto(pairs = pairs)
}

/** True when every question has a selected option or non-blank custom answer. */
internal fun askAnswersComplete(
    request: AskUserQuestionRequestDto,
    selections: Map<Int, List<Int>>,
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
    onSubmit: (requestId: ULong, answers: AskUserQuestionAnswersDto) -> Unit,
    onCancel: (requestId: ULong) -> Unit,
    modifier: Modifier = Modifier,
) {
    var expandedQuestion by remember(request.requestId) { mutableIntStateOf(0) }
    val selections = remember(request.requestId) { mutableStateMapOf<Int, List<Int>>() }
    val freeTexts = remember(request.requestId) { mutableStateMapOf<Int, String>() }
    val questions = request.questions
    val answeredCount = questions.indices.count { index ->
        selections[index].orEmpty().isNotEmpty() || !freeTexts[index]?.trim().isNullOrEmpty()
    }

    Card(
        shape = RoundedCornerShape(24.dp),
        border = BorderStroke(1.dp, MaterialTheme.colorScheme.outlineVariant),
        colors = CardDefaults.cardColors(containerColor = MaterialTheme.colorScheme.surface),
        elevation = CardDefaults.cardElevation(defaultElevation = 0.dp),
        modifier = modifier.fillMaxWidth(),
    ) {
        Column(
            verticalArrangement = Arrangement.spacedBy(10.dp),
            modifier = Modifier.fillMaxWidth().padding(16.dp),
        ) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                Icon(
                    painter = androidx.compose.ui.res.painterResource(R.drawable.ic_codex_question),
                    contentDescription = null,
                    tint = MaterialTheme.colorScheme.onSurfaceVariant,
                    modifier = Modifier.size(20.dp),
                )
                Text(
                    text = androidx.compose.ui.res.stringResource(R.string.chat_ask_user_question_title),
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    style = MaterialTheme.typography.bodyLarge,
                    modifier = Modifier.weight(1f).padding(start = 10.dp).semantics { heading() },
                )
                if (questions.size > 1) {
                    Surface(
                        shape = RoundedCornerShape(50),
                        color = MaterialTheme.colorScheme.surfaceContainerHigh,
                    ) {
                        Text(
                            text = androidx.compose.ui.res.stringResource(
                                R.string.chat_question_answered_progress,
                                answeredCount,
                                questions.size,
                            ),
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                            style = MaterialTheme.typography.labelSmall,
                            modifier = Modifier.padding(horizontal = 8.dp, vertical = 5.dp),
                        )
                    }
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
                    val answer = answerSummary(index, question, selections, freeTexts)
                    AskQuestionAccordionRow(
                        question = question,
                        expanded = expandedQuestion == index,
                        showDisclosure = questions.size > 1,
                        answer = answer,
                        selected = selections[index].orEmpty(),
                        onExpand = {
                            expandedQuestion = if (expandedQuestion == index && questions.size > 1) -1 else index
                        },
                        onToggleOption = { optionIndex ->
                            val next = toggleAskSelection(
                                selections[index].orEmpty(),
                                optionIndex,
                                question.multiSelect,
                            )
                            selections[index] = next
                            if (questions.size > 1 && !question.multiSelect && next.isNotEmpty()) {
                                expandedQuestion = questions.indices.firstOrNull { candidate ->
                                    candidate != index &&
                                        selections[candidate].orEmpty().isEmpty() &&
                                        freeTexts[candidate]?.trim().isNullOrEmpty()
                                } ?: -1
                            }
                        },
                    )
                }
            }

            val questionInputLabel = androidx.compose.ui.res.stringResource(R.string.chat_ask_other_option)
            val composerIndex = expandedQuestion.takeIf { it in questions.indices } ?: 0
            Surface(
                shape = RoundedCornerShape(18.dp),
                border = BorderStroke(1.dp, MaterialTheme.colorScheme.onSurface),
                color = MaterialTheme.colorScheme.surface,
            ) {
                Column(Modifier.fillMaxWidth().padding(8.dp)) {
                    if (expandedQuestion in questions.indices) {
                        Row(verticalAlignment = Alignment.CenterVertically) {
                            Icon(Icons.Rounded.Edit, contentDescription = null,
                                tint = MaterialTheme.colorScheme.onSurfaceVariant,
                                modifier = Modifier.padding(horizontal = 8.dp).size(20.dp))
                            BasicTextField(
                                value = freeTexts[composerIndex].orEmpty(),
                                onValueChange = { freeTexts[composerIndex] = it },
                                textStyle = MaterialTheme.typography.bodyLarge.copy(color = MaterialTheme.colorScheme.onSurface),
                                cursorBrush = SolidColor(MaterialTheme.colorScheme.onSurface),
                                maxLines = 4,
                                modifier = Modifier.weight(1f).defaultMinSize(minHeight = 44.dp)
                                    .semantics { contentDescription = questionInputLabel },
                                decorationBox = { inner ->
                                    Box(contentAlignment = Alignment.CenterStart) {
                                        if (freeTexts[composerIndex].isNullOrEmpty()) {
                                            Text(androidx.compose.ui.res.stringResource(R.string.chat_question_other_placeholder),
                                                color = MaterialTheme.colorScheme.onSurfaceVariant,
                                                style = MaterialTheme.typography.bodyLarge)
                                        }
                                        inner()
                                    }
                                },
                            )
                        }
                    }
                    Row(
                        horizontalArrangement = Arrangement.spacedBy(8.dp, Alignment.End),
                        modifier = Modifier.fillMaxWidth(),
                    ) {
                        OutlinedButton(onClick = { onCancel(request.requestId) }) {
                            Text(androidx.compose.ui.res.stringResource(R.string.chat_question_skip))
                        }
                        Button(
                            enabled = askAnswersComplete(request, selections, freeTexts),
                            colors = ButtonDefaults.buttonColors(
                                containerColor = MaterialTheme.colorScheme.onSurface,
                                contentColor = MaterialTheme.colorScheme.surface,
                            ),
                            onClick = { onSubmit(request.requestId, assembleAskAnswers(request, selections, freeTexts)) },
                        ) {
                            Text(androidx.compose.ui.res.stringResource(R.string.composer_send))
                        }
                    }
                }
            }
        }
    }
}

@Composable
private fun AskQuestionAccordionRow(
    question: AskQuestionDto,
    expanded: Boolean,
    showDisclosure: Boolean,
    answer: String?,
    selected: List<Int>,
    onExpand: () -> Unit,
    onToggleOption: (Int) -> Unit,
) {
    val colors = MaterialTheme.colorScheme
    val shape = RoundedCornerShape(12.dp)
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
                .padding(vertical = 10.dp),
        ) {
            Column(
                verticalArrangement = Arrangement.spacedBy(3.dp),
                modifier = Modifier.weight(1f),
            ) {
                Text(
                    text = question.question.displayText(),
                    style = MaterialTheme.typography.bodyLarge,
                    color = colors.onSurface,
                    fontWeight = FontWeight.SemiBold,
                )
                if (!expanded) {
                    Text(
                        text = answer ?: androidx.compose.ui.res.stringResource(R.string.chat_question_tap_to_answer),
                        style = MaterialTheme.typography.bodyMedium,
                        color = colors.onSurfaceVariant,
                    )
                }
            }
            if (showDisclosure) {
                Icon(
                    imageVector = if (expanded) Icons.Rounded.KeyboardArrowUp else Icons.Rounded.KeyboardArrowDown,
                    contentDescription = null,
                    tint = colors.onSurfaceVariant,
                )
            }
        }

        AnimatedVisibility(visible = expanded) {
            Column(
                verticalArrangement = Arrangement.spacedBy(8.dp),
                modifier = Modifier.fillMaxWidth().selectableGroup().padding(bottom = 6.dp),
            ) {
                question.options.forEachIndexed { optionIndex, option ->
                    val active = optionIndex in selected
                    Surface(
                        shape = shape,
                        color = if (active) colors.surfaceContainerHigh else colors.surface,
                        modifier = Modifier
                            .fillMaxWidth()
                            .selectable(
                                selected = active,
                                role = if (question.multiSelect) Role.Checkbox else Role.RadioButton,
                                onClick = { onToggleOption(optionIndex) },
                            ),
                    ) {
                        Row(
                            verticalAlignment = Alignment.CenterVertically,
                            modifier = Modifier.heightIn(min = 56.dp).padding(horizontal = 12.dp, vertical = 8.dp),
                        ) {
                            Surface(
                                shape = RoundedCornerShape(6.dp),
                                color = colors.surfaceContainerHigh,
                                border = BorderStroke(1.dp, colors.outlineVariant),
                            ) {
                                Box(Modifier.size(26.dp), contentAlignment = Alignment.Center) {
                                    Text(if (question.multiSelect && active) "✓" else "${optionIndex + 1}",
                                        style = MaterialTheme.typography.bodyMedium,
                                        color = colors.onSurfaceVariant)
                                }
                            }
                            Spacer(Modifier.width(8.dp))
                            Column(verticalArrangement = Arrangement.spacedBy(2.dp), modifier = Modifier.weight(1f)) {
                                Text(
                                    text = option.label.displayText(),
                                    style = MaterialTheme.typography.bodyMedium,
                                    color = colors.onSurface,
                                    fontWeight = FontWeight.SemiBold,
                                )
                                if (option.description.displayText().isNotBlank()) {
                                    Text(
                                        text = option.description.displayText(),
                                        style = MaterialTheme.typography.bodySmall,
                                        color = colors.onSurfaceVariant,
                                    )
                                }
                            }
                            if (active) {
                                Icon(Icons.Rounded.ArrowForward, contentDescription = null,
                                    tint = colors.onSurfaceVariant, modifier = Modifier.size(20.dp))
                            }
                        }
                    }
                }


            }
        }
    }
}

private fun answerSummary(
    index: Int,
    question: AskQuestionDto,
    selections: Map<Int, List<Int>>,
    freeTexts: Map<Int, String>,
): String? {
    val parts = selections[index].orEmpty().mapNotNull { optionIndex ->
        question.options.getOrNull(optionIndex)?.label?.displayText()
    } + listOfNotNull(freeTexts[index]?.trim()?.takeIf { it.isNotEmpty() })
    return parts.takeIf { it.isNotEmpty() }?.joinToString(", ")
}
