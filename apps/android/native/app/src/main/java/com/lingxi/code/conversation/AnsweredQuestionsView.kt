package com.lingxi.code.conversation

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.*
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.graphics.luminance
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.res.pluralStringResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.semantics.stateDescription
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.lingxi.code.R
import com.lingxi.code.components.LXIcon
import com.lingxi.code.components.LXIconName
import com.lingxi.code.theme.LingXiTheme
import org.json.JSONObject

/** Verbatim structured content, decoded at live and persisted result boundaries. */
data class AnsweredQuestion(val question: String, val answer: String?)

internal fun parseQuestionAnswers(tool: String, json: String): List<AnsweredQuestion>? {
    if (tool != "AskUserQuestion") return null
    return runCatching {
        val output = JSONObject(json)
        val questions = output.getJSONArray("questions")
        val answers = output.getJSONObject("answers")
        require(questions.length() > 0)
        (0 until questions.length()).map { index ->
            val question = questions.getJSONObject(index).get("question")
            require(question is String)
            val answer = if (answers.has(question)) answers.get(question) else null
            require(answer == null || answer is String)
            AnsweredQuestion(question, (answer as? String)?.takeIf { it.isNotBlank() })
        }
    }.getOrNull()
}

@Composable
internal fun AnsweredQuestionsView(rows: List<AnsweredQuestion>, expanded: Boolean, onToggle: () -> Unit, modifier: Modifier = Modifier) {
    val t = LingXiTheme.palette
    val summaryColor = if (t.appBg.luminance() < 0.5f) t.text2 else t.text3
    val state = stringResource(if (expanded) R.string.chat_question_expanded else R.string.chat_question_collapsed)
    Column(modifier.fillMaxWidth(), verticalArrangement = Arrangement.spacedBy(10.dp)) {
        Row(
            modifier = Modifier.heightIn(min = 44.dp).clickable(role = Role.Button, onClick = onToggle)
                .semantics { stateDescription = state },
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            Icon(painterResource(R.drawable.ic_codex_question), contentDescription = null, tint = summaryColor, modifier = Modifier.size(18.dp))
            Text(pluralStringResource(R.plurals.chat_asked_questions, rows.size, rows.size), color = summaryColor.copy(alpha = 0.85f), fontSize = 14.sp, modifier = Modifier.weight(1f, fill = false))
            LXIcon(if (expanded) LXIconName.Chevron else LXIconName.ChevronR, size = 12.dp, color = summaryColor)
        }
        if (expanded) rows.forEach { row ->
            Column(verticalArrangement = Arrangement.spacedBy(4.dp)) {
                Text(row.question, color = summaryColor.copy(alpha = 0.85f), fontSize = 14.sp, lineHeight = 23.sp)
                Text(row.answer ?: stringResource(R.string.chat_question_no_answer), color = summaryColor.copy(alpha = 0.5f), fontSize = 14.sp, lineHeight = 23.sp)
            }
        }
    }
}
