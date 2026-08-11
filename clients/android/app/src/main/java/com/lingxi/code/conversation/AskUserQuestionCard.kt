package com.lingxi.code.conversation

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.material3.Button
import androidx.compose.material3.Card
import androidx.compose.material3.CardDefaults
import androidx.compose.material3.FilterChip
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateMapOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import com.lingxi.code.R
import com.lingxi.code.bindings.AskQuestionDto
import com.lingxi.code.bindings.AskUserQuestionRequestDto

/**
 * Toggle [label] in a question's selection list. Multi-select questions toggle
 * membership; single-select replaces the selection (tapping the selected chip
 * clears it). Pure so the chip semantics are unit-testable.
 */
internal fun toggleAskSelection(
    current: List<String>,
    label: String,
    multiSelect: Boolean,
): List<String> = when {
    multiSelect -> if (label in current) current - label else current + label
    label in current -> emptyList()
    else -> listOf(label)
}

/**
 * Assemble the `AnswerAskUserQuestion` answer map: each answered question's
 * full text maps to its selected label(s), comma-joined, with any free-text
 * 「其他」 entry appended as one more answer segment. Questions with no
 * selection and no free text are omitted. Pure — the exact payload contract
 * is unit-tested.
 */
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

/** True when every question carries at least one answer (chip or free text). */
internal fun askAnswersComplete(
    request: AskUserQuestionRequestDto,
    selections: Map<Int, List<String>>,
    freeTexts: Map<Int, String>,
): Boolean = request.questions.indices.all { index ->
    selections[index].orEmpty().isNotEmpty() ||
        !freeTexts[index]?.trim().isNullOrEmpty()
}

/**
 * The interactive `AskUserQuestion` content hosted by a native modal sheet: one question
 * shown at a time with 上一题/下一题 across the request's 1–4 questions,
 * options as selectable chips (multi-select per the question's flag), an
 * always-present free-text 「其他」 row, and 提交 / 取消 resolving the whole
 * request through `AnswerAskUserQuestion` / `CancelAskUserQuestion`.
 */
@OptIn(ExperimentalLayoutApi::class)
@Composable
fun AskUserQuestionCard(
    request: AskUserQuestionRequestDto,
    onSubmit: (requestId: ULong, answers: Map<String, String>) -> Unit,
    onCancel: (requestId: ULong) -> Unit,
    modifier: Modifier = Modifier,
) {
    // Draft state is keyed to the request id: a NEW request (after this one
    // resolves) starts clean, while recompositions of the same request keep
    // the user's picks.
    var questionIndex by remember(request.requestId) { mutableIntStateOf(0) }
    val selections = remember(request.requestId) { mutableStateMapOf<Int, List<String>>() }
    val freeTexts = remember(request.requestId) { mutableStateMapOf<Int, String>() }

    val questions = request.questions
    val index = questionIndex.coerceIn(0, (questions.size - 1).coerceAtLeast(0))
    val question = questions.getOrNull(index)

    Card(
        colors = CardDefaults.cardColors(containerColor = MaterialTheme.colorScheme.surfaceContainerLow),
        modifier = modifier.fillMaxWidth().padding(vertical = 4.dp),
    ) {
        Column(
            verticalArrangement = Arrangement.spacedBy(10.dp),
            modifier = Modifier.fillMaxWidth().padding(14.dp),
        ) {
            Row(verticalAlignment = androidx.compose.ui.Alignment.CenterVertically) {
                Text(
                    question?.header?.takeIf { it.isNotBlank() }
                        ?: stringResource(R.string.chat_question_card_title),
                    style = MaterialTheme.typography.titleSmall,
                    fontWeight = FontWeight.SemiBold,
                    modifier = Modifier.weight(1f).semantics { heading() },
                )
                if (questions.size > 1) {
                    Text(
                        stringResource(R.string.chat_question_progress, index + 1, questions.size),
                        style = MaterialTheme.typography.labelMedium,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                }
            }
            question?.let { q ->
                QuestionBody(
                    question = q,
                    selected = selections[index].orEmpty(),
                    freeText = freeTexts[index].orEmpty(),
                    onToggleOption = { label ->
                        selections[index] = toggleAskSelection(selections[index].orEmpty(), label, q.multiSelect)
                    },
                    onFreeTextChange = { freeTexts[index] = it },
                )
            }
            if (questions.size > 1) {
                Row(horizontalArrangement = Arrangement.spacedBy(8.dp), modifier = Modifier.fillMaxWidth()) {
                    OutlinedButton(
                        enabled = index > 0,
                        onClick = { questionIndex = index - 1 },
                        modifier = Modifier.weight(1f),
                    ) { Text(stringResource(R.string.chat_question_previous)) }
                    OutlinedButton(
                        enabled = index < questions.lastIndex,
                        onClick = { questionIndex = index + 1 },
                        modifier = Modifier.weight(1f),
                    ) { Text(stringResource(R.string.chat_question_next)) }
                }
            }
            Row(
                horizontalArrangement = Arrangement.spacedBy(8.dp),
                verticalAlignment = androidx.compose.ui.Alignment.CenterVertically,
                modifier = Modifier.fillMaxWidth(),
            ) {
                TextButton(onClick = { onCancel(request.requestId) }) {
                    Text(stringResource(R.string.common_cancel))
                }
                Spacer(Modifier.weight(1f))
                Button(
                    enabled = askAnswersComplete(request, selections, freeTexts),
                    onClick = {
                        onSubmit(request.requestId, assembleAskAnswers(request, selections, freeTexts))
                    },
                ) { Text(stringResource(R.string.chat_question_submit)) }
            }
        }
    }
}

@OptIn(ExperimentalLayoutApi::class)
@Composable
private fun QuestionBody(
    question: AskQuestionDto,
    selected: List<String>,
    freeText: String,
    onToggleOption: (String) -> Unit,
    onFreeTextChange: (String) -> Unit,
) {
    Column(verticalArrangement = Arrangement.spacedBy(8.dp), modifier = Modifier.fillMaxWidth()) {
        Text(question.question, style = MaterialTheme.typography.bodyMedium)
        if (question.options.isNotEmpty()) {
            // A generic chip row — the same FilterChip treatment the deleted
            // designer used for option fields, kept as the one reusable style
            // for selectable answers.
            FlowRow(
                horizontalArrangement = Arrangement.spacedBy(8.dp),
                verticalArrangement = Arrangement.spacedBy(6.dp),
                modifier = Modifier.fillMaxWidth(),
            ) {
                question.options.forEach { option ->
                    FilterChip(
                        selected = option.label in selected,
                        onClick = { onToggleOption(option.label) },
                        label = { Text(option.label) },
                    )
                }
            }
            // Surface the explanation of what the user has picked — chips
            // alone would hide the option descriptions entirely.
            question.options
                .filter { it.label in selected && it.description.isNotBlank() }
                .forEach { option ->
                    Text(
                        "${option.label}：${option.description}",
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                }
        }
        // The automatic free-text row, always offered (clients synthesize the
        // `Other` affordance — the wire options never include one).
        OutlinedTextField(
            value = freeText,
            onValueChange = onFreeTextChange,
            placeholder = { Text(stringResource(R.string.chat_question_other_placeholder)) },
            singleLine = true,
            modifier = Modifier.fillMaxWidth(),
        )
    }
}
