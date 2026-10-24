package com.lingxi.code.conversation

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class LocalAppCardTest {
    @Test
    fun aPublishResultIsTheCard() {
        assertEquals(
            PublishedLocalApp("a0123456789abcdef0123456789abcdef", "喝水记录", 2),
            parseLocalAppResult(
                "LocalApp",
                """{"action":"publish","id":"a0123456789abcdef0123456789abcdef","revision":2,"name":"喝水记录","warnings":[]}""",
            ),
        )
    }

    @Test
    fun listReadOtherToolsAndMalformedResultsHaveNoCard() {
        assertNull(parseLocalAppResult("LocalApp", """{"action":"list","apps":[]}"""))
        assertNull(parseLocalAppResult("LocalApp", """{"action":"read","id":"a1","name":"T","revision":1,"bytes":9}"""))
        assertNull(parseLocalAppResult("Visualization", """{"action":"publish","id":"a1","revision":1,"name":"T"}"""))
        assertNull(parseLocalAppResult("LocalApp", """{"action":"publish","id":"../x","revision":1,"name":"T"}"""))
        assertNull(parseLocalAppResult("LocalApp", """{"action":"publish","id":"a1","revision":0,"name":"T"}"""))
        assertNull(parseLocalAppResult("LocalApp", """{"action":"publish","id":"a1","revision":1,"name":" "}"""))
        assertNull(parseLocalAppResult("LocalApp", "Published \"T\" as local app a1"))
    }

    @Test
    fun legacyPublishIsAnArchiveRatherThanAnExecutableV2Card() {
        assertNull(parseLocalAppResult("LocalApp", """{"action":"publish","id":"a1","revision":1,"name":"Old app"}"""))
    }

    @Test
    fun theCardSurvivesRunToTranscriptProjection() {
        val app = PublishedLocalApp("a0123456789abcdef0123456789abcdef", "Timer", 1)
        assertEquals(
            app,
            AgentToolRunState(id = "t", tool = "LocalApp", status = AgentToolStatus.Completed, localApp = app)
                .toToolCall()
                .localApp,
        )
    }
}
