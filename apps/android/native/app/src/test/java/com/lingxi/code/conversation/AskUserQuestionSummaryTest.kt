package com.lingxi.code.conversation
import org.junit.Assert.*
import org.junit.Test
class AskUserQuestionSummaryTest {
    @Test fun preservesQuestionOrderAndMissingAnswers() {
        val rows = parseQuestionAnswers("AskUserQuestion", """{"questions":[{"question":"语音？"},{"question":"Other?"}],"answers":{"语音？":"实时\n打断"}}""")
        assertEquals(listOf(AnsweredQuestion("语音？", "实时\n打断"), AnsweredQuestion("Other?", null)), rows)
    }
    @Test fun rejectsMalformedAndUnrelatedResults() {
        assertNull(parseQuestionAnswers("Other", """{"questions":[{"question":"Q"}],"answers":{}}"""))
        assertNull(parseQuestionAnswers("AskUserQuestion", """{"questions":[{"question":"Q"}],"answers":{"Q":3}}"""))
        assertNull(parseQuestionAnswers("AskUserQuestion", "not json"))
    }
    @Test fun summarySurvivesRunToTranscriptProjection() {
        val rows = listOf(AnsweredQuestion("Q", "A"))
        assertEquals(rows, AgentToolRunState(id = "ask", tool = "AskUserQuestion", status = AgentToolStatus.Completed, questionAnswers = rows).toToolCall().questionAnswers)
    }
}
