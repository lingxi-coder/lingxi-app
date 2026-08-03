package com.lingxi.code.conversation

import com.lingxi.code.bindings.ClientEvent
import com.lingxi.code.bindings.ErrorKindDto
import com.lingxi.code.bindings.MessageBlockDto
import com.lingxi.code.bindings.MessageDto
import com.lingxi.code.bindings.SessionRowDto
import com.lingxi.code.model.EngineModelState
import com.lingxi.code.model.EngineSessionState
import com.lingxi.code.model.Message
import com.lingxi.code.model.SessionCatalogPhase
import com.lingxi.code.model.SessionCatalog
import com.lingxi.code.model.SessionRow
import com.lingxi.code.model.withCachedRows
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.emptyFlow
import kotlinx.coroutines.test.UnconfinedTestDispatcher
import kotlinx.coroutines.test.resetMain
import kotlinx.coroutines.test.runTest
import kotlinx.coroutines.test.setMain
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertSame
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Test

/**
 * The OUT-OF-BAND SESSION-state path that drives the drawer from the engine's
 * REAL `SessionList` instead of the branded MockData chats — the exact sibling
 * of [ModelStateTest].
 *
 * Three pure layers, none of which build a native engine (`buildAndroidEngine`
 * is never touched):
 *  - [reduceSessionEvent]               — folds one [ClientEvent] into [EngineSessionState].
 *  - the [SessionRow] mapping it drives — wire dto → UI row (title + count + relative time).
 *  - [ChatViewModel] session mirroring  — reflects the engine state + resume/new intents.
 *
 * The fixtures only CONSTRUCT generated UniFFI data classes (no exported call),
 * so no `.so` is loaded — exactly the ModelStateTest pattern.
 */
@OptIn(ExperimentalCoroutinesApi::class)
class SessionStateTest {

    private val dispatcher = UnconfinedTestDispatcher()

    @Before fun setMain() = Dispatchers.setMain(dispatcher)

    @After fun tearDown() = Dispatchers.resetMain()

    // A fixed "now" (2024-06-15T12:00:00Z) so the relative-time buckets are
    // deterministic regardless of when the suite runs.
    private val now = SessionCatalog.parseRfc3339ToEpochSeconds("2024-06-15T12:00:00Z")!!

    private fun dto(
        uuid: String,
        title: String,
        modified: String,
        count: Int,
    ) = SessionRowDto(
        uuid = uuid,
        title = title,
        modifiedRfc3339 = modified,
        messageCount = count.toUInt(),
        path = "/x/$uuid.jsonl",
    )

    // --- reduceSessionEvent: SessionList ----------------------------------

    @Test
    fun sessionList_replacesCatalog_mappingWireRowsToUiRows() {
        val next = reduceSessionEvent(
            EngineSessionState.loading(),
            ClientEvent.SessionList(
                sessions = listOf(
                    dto("u1", "重装 LingXi", "2024-06-15T11:30:00Z", 8),
                    dto("u2", "客户邮件模板", "2024-06-13T12:00:00Z", 4),
                ),
            ),
            nowEpochSeconds = now,
        )
        assertTrue(next.hasSessions)
        assertEquals(SessionCatalogPhase.Ready, next.phase)
        assertEquals(listOf("u1", "u2"), next.rows.map { it.uuid })
        assertEquals(listOf("重装 LingXi", "客户邮件模板"), next.rows.map { it.title })
        assertEquals(listOf(8, 4), next.rows.map { it.messageCount })
        // 30 minutes ago → "30 分钟前"; ~2 days ago → "2 天前".
        assertEquals("30 分钟前", next.rows[0].relativeTime)
        assertEquals("2 天前", next.rows[1].relativeTime)
    }

    @Test
    fun sessionList_replacesAPriorCatalog_wholesale() {
        val prev = EngineSessionState.ready(
            rows = listOf(SessionRow("old", "Old", 1, "刚刚")),
        )
        val next = reduceSessionEvent(
            prev,
            ClientEvent.SessionList(sessions = listOf(dto("new", "New", "2024-06-15T12:00:00Z", 2))),
            nowEpochSeconds = now,
        )
        assertEquals(listOf("new"), next.rows.map { it.uuid })
    }

    @Test
    fun sessionList_empty_marksReadyEmpty_withoutMockFallback() {
        val prev = EngineSessionState.ready(rows = listOf(SessionRow("u", "T", 1, "刚刚")))
        val next = reduceSessionEvent(
            prev,
            ClientEvent.SessionList(sessions = emptyList()),
            nowEpochSeconds = now,
        )
        assertFalse(next.hasSessions)
        assertTrue(next.isEmpty)
        assertEquals(SessionCatalogPhase.Ready, next.phase)
    }

    @Test
    fun blankTitle_fallsBackToPlaceholder_neverEmpty() {
        val next = reduceSessionEvent(
            EngineSessionState.loading(),
            ClientEvent.SessionList(sessions = listOf(dto("u", "   ", "2024-06-15T12:00:00Z", 0))),
            nowEpochSeconds = now,
        )
        assertEquals("未命名会话", next.rows.single().title)
    }

    // --- reduceSessionEvent: non-session events are inert ------------------

    @Test
    fun nonSessionEvent_returnsPrevUnchanged_identity() {
        val prev = EngineSessionState.ready(rows = listOf(SessionRow("u", "T", 3, "刚刚")))
        // TextDelta / Error / lifecycle events must NOT disturb the catalog.
        assertSame(prev, reduceSessionEvent(prev, ClientEvent.TextDelta("hi"), now))
        assertSame(prev, reduceSessionEvent(prev, ClientEvent.SessionStarted(sessionId = "x"), now))
        assertSame(
            prev,
            reduceSessionEvent(prev, ClientEvent.SessionResumed(sessionId = "x", messages = emptyList()), now),
        )
        assertSame(prev, reduceSessionEvent(prev, ClientEvent.SessionEnded, now))
        assertSame(
            prev,
            reduceSessionEvent(prev, ClientEvent.Error(kind = ErrorKindDto.TRANSPORT, message = "x"), now),
        )
    }

    // --- ChatViewModel session integration --------------------------------

    /** A source whose session + resume state we drive; submit/streams are inert. */
    private class FakeSessionSource(
        private val sessions: MutableStateFlow<EngineSessionState>,
        private val activated: MutableStateFlow<ActivatedSession?> = MutableStateFlow(null),
    ) : ConversationSource {
        val resumeCalls = mutableListOf<String>()
        var newSessionCalls = 0
        var refreshCalls = 0
        override fun initialMessages(): List<Message> = emptyList()
        override fun submit(text: String): Flow<ReplyEvent> = emptyFlow()
        override val sessionState: StateFlow<EngineSessionState> = sessions.asStateFlow()
        override val activeSessionState: StateFlow<ActivatedSession?> = activated.asStateFlow()
        override suspend fun refreshSessions() { refreshCalls++ }
        override suspend fun resumeSession(uuid: String) { resumeCalls += uuid }
        override suspend fun newSession() { newSessionCalls++ }
    }

    @Test
    fun loadingCatalog_viewModelExposesLoadingState() {
        val vm = ChatViewModel(FakeSessionSource(MutableStateFlow(EngineSessionState.loading())))
        assertTrue(vm.sessions.value.isLoading)
    }

    @Test
    fun engineCatalog_mirrorsIntoViewModelSessions_outOfBand() {
        val flow = MutableStateFlow(EngineSessionState.loading())
        val vm = ChatViewModel(FakeSessionSource(flow))

        flow.value = EngineSessionState.ready(rows = listOf(SessionRow("u1", "会话一", 5, "刚刚")))

        assertEquals(listOf("u1"), vm.sessions.value.rows.map { it.uuid })
        assertEquals("会话一", vm.sessions.value.rows.single().title)
    }

    @Test
    fun resumeSession_selectsLocally_andSubmitsResume() = runTest(dispatcher) {
        val source = FakeSessionSource(MutableStateFlow(EngineSessionState.loading()))
        val vm = ChatViewModel(source)

        val row = SessionRow(uuid = "uuid-42", title = "差旅规划", messageCount = 12, relativeTime = "昨天")
        vm.resumeSession(row)

        // Local select: the title bar reflects the resumed session immediately.
        assertEquals("uuid-42", vm.state.value.session.id)
        assertEquals("差旅规划", vm.state.value.session.title)
        // The transcript was reset to the source's (empty) initialMessages.
        assertTrue(vm.state.value.messages.isEmpty())
        assertFalse(vm.state.value.isNew)
        // AND the engine was told to resume by the REAL wire uuid.
        assertEquals(listOf("uuid-42"), source.resumeCalls)
    }

    @Test
    fun legacyPrefixedSessionId_isCanonicalBeforeResume() = runTest(dispatcher) {
        val source = FakeSessionSource(MutableStateFlow(EngineSessionState.loading()))
        val vm = ChatViewModel(source)
        val uuid = "19587a33-0725-48db-abca-8a2aed345f6b"

        vm.resumeSession(
            SessionRow(
                uuid = "sess:$uuid",
                title = "旧版会话",
                messageCount = 1,
                relativeTime = "刚刚",
            ),
        )

        assertEquals(uuid, vm.state.value.session.id)
        assertEquals(listOf(uuid), source.resumeCalls)
    }

    // --- sessionActivationFrom: the out-of-band session activation path -------

    private fun userDto(text: String) =
        MessageDto(role = "user", blocks = listOf(MessageBlockDto.Text(text)))

    private fun assistantDto(vararg blocks: MessageBlockDto) =
        MessageDto(role = "assistant", blocks = blocks.toList())

    @Test
    fun sessionActivationFrom_sessionResumed_lowersTranscriptOldestFirst() {
        val restored = sessionActivationFrom(
            ClientEvent.SessionResumed(
                sessionId = "22222222-2222-4222-8222-222222222222",
                messages = listOf(
                    userDto("第一条问题"),
                    assistantDto(
                        MessageBlockDto.Thinking(thinking = "推理…", signature = null),
                        MessageBlockDto.Text("第一条回答"),
                    ),
                ),
            ),
        )
        assertNotNull(restored)
        assertEquals("22222222-2222-4222-8222-222222222222", restored!!.sessionId)
        assertEquals(SessionActivationKind.Resumed, restored.kind)
        // Two messages, OLDEST-FIRST, role-mapped.
        assertEquals(2, restored.transcript.size)
        assertEquals(com.lingxi.code.model.Role.User, restored.transcript[0].role)
        assertEquals("第一条问题", restored.transcript[0].text)
        assertEquals(com.lingxi.code.model.Role.Ai, restored.transcript[1].role)
        // The assistant body folds thinking + text into one block-joined string.
        assertTrue(restored.transcript[1].text.contains("推理…"))
        assertTrue(restored.transcript[1].text.contains("第一条回答"))
    }

    @Test
    fun sessionActivationFrom_canonicalizesLegacyDisplayPrefix() {
        val uuid = "19587a33-0725-48db-abca-8a2aed345f6b"

        val started = sessionActivationFrom(ClientEvent.SessionStarted(sessionId = "sess:$uuid"))
        val resumed = sessionActivationFrom(
            ClientEvent.SessionResumed(sessionId = "sess:$uuid", messages = emptyList()),
        )

        assertEquals(uuid, started?.sessionId)
        assertEquals(uuid, resumed?.sessionId)
    }

    @Test
    fun cachedStartedSessionRemainsVisibleUntilLiveCatalogCatchesUp() {
        val cached = SessionRow(
            uuid = "19587a33-0725-48db-abca-8a2aed345f6b",
            title = "新对话",
            messageCount = 0,
            relativeTime = "刚刚",
        )

        val waiting = EngineSessionState.ready(emptyList()).withCachedRows(listOf(cached))
        assertEquals(listOf(cached), waiting.rows)

        val live = cached.copy(title = "真实标题", messageCount = 2)
        val caughtUp = EngineSessionState.ready(listOf(live)).withCachedRows(listOf(cached))
        assertEquals(listOf(live), caughtUp.rows)
    }

    @Test
    fun sessionActivationFrom_sessionStarted_yieldsEmptyTranscript() {
        val restored = sessionActivationFrom(
            ClientEvent.SessionStarted(sessionId = "s"),
        )
        assertNotNull(restored)
        assertTrue(restored!!.transcript.isEmpty())
        assertEquals(SessionActivationKind.Started, restored.kind)
        assertEquals("s", restored.sessionId)
    }

    @Test
    fun sessionActivationFrom_emptyTranscript_isNonNullWithNoMessages() {
        val restored = sessionActivationFrom(
            ClientEvent.SessionResumed(sessionId = "s", messages = emptyList()),
        )
        assertNotNull(restored)
        assertTrue(restored!!.transcript.isEmpty())
        assertEquals("s", restored.sessionId)
    }

    @Test
    fun sessionActivationFrom_nonActivationEvent_isNull() {
        assertNull(sessionActivationFrom(ClientEvent.TextDelta("hi")))
        assertNull(sessionActivationFrom(ClientEvent.SessionList(sessions = emptyList())))
    }

    @Test
    fun messageDtoText_foldsEveryBlockKind_droppingBlanks() {
        val text = messageDtoText(
            listOf(
                MessageBlockDto.Text("正文"),
                MessageBlockDto.Text("   "), // blank → dropped
                MessageBlockDto.ToolUse(id = "t1", tool = "bash", inputJson = "{}"),
                MessageBlockDto.ToolResult(
                    id = "t1", tool = "bash", resultJson = "ok", isError = false,
                    oldString = null, newString = null, filePath = null,
                ),
                MessageBlockDto.RedactedThinking(data = "opaque"),
                MessageBlockDto.CompactBoundary(
                    messagesBefore = 8u,
                    messagesAfter = 2u,
                    summary = "hidden compact summary",
                ),
            ),
        )
        assertTrue(text.contains("正文"))
        assertTrue(text.contains("bash")) // the tool-use activity line
        assertTrue(text.contains("工具结果"))
        assertTrue(text.contains("已折叠的思考"))
        assertTrue(text.contains("对话已压缩"))
        assertFalse(text.contains("hidden compact summary"))
        // The blank text block left no dangling double-blank run.
        assertFalse(text.contains("\n\n\n"))
    }

    // --- ChatViewModel session activation rehydration -----------------------

    @Test
    fun activeSessionState_rehydratesTranscript_andSetsActiveSession_outOfBand() {
        val resumed = MutableStateFlow<ActivatedSession?>(null)
        val source = FakeSessionSource(MutableStateFlow(EngineSessionState.loading()), resumed)
        val vm = ChatViewModel(source)

        // A live SessionResumed lands with 3 messages, oldest-first.
        resumed.value = ActivatedSession(
            sessionId = "uuid-99",
            transcript = listOf(
                Message(role = com.lingxi.code.model.Role.User, text = "q1"),
                Message(role = com.lingxi.code.model.Role.Ai, text = "a1"),
                Message(role = com.lingxi.code.model.Role.User, text = "q2"),
            ),
            kind = SessionActivationKind.Resumed,
        )

        // N messages rendered, in order.
        assertEquals(3, vm.state.value.messages.size)
        assertEquals(listOf("q1", "a1", "q2"), vm.state.value.messages.map { it.text })
        // Active session id swapped to the REAL resumed uuid.
        assertEquals("uuid-99", vm.state.value.session.id)
        assertFalse(vm.state.value.isNew)
        assertFalse(vm.state.value.streaming)
    }

    @Test
    fun activeSessionState_replacesCurrentTranscript_wholesale() {
        val resumed = MutableStateFlow<ActivatedSession?>(null)
        val source = FakeSessionSource(MutableStateFlow(EngineSessionState.loading()), resumed)
        val vm = ChatViewModel(source)

        // Seed a stale transcript (a prior turn), then resume swaps it wholesale.
        vm.reduce(ReplyEvent.Delta("stale turn"))
        assertEquals("stale turn", vm.state.value.streamingMessage?.text)

        resumed.value = ActivatedSession(
            sessionId = "uuid-77",
            transcript = listOf(Message(role = com.lingxi.code.model.Role.User, text = "restored")),
            kind = SessionActivationKind.Resumed,
        )

        assertEquals(listOf("restored"), vm.state.value.messages.map { it.text })
        assertNull(vm.state.value.streamingMessage)
    }

    @Test
    fun applyActivatedSession_preservesDrawerSelectedTitle_whenIdMatches() {
        val source = FakeSessionSource(MutableStateFlow(EngineSessionState.loading()))
        val vm = ChatViewModel(source)

        // The drawer's optimistic local select set the title before resume landed.
        vm.resumeSession(SessionRow(uuid = "uuid-55", title = "差旅规划", messageCount = 3, relativeTime = "昨天"))
        // The engine confirms with the rehydrated transcript (id matches the select).
        vm.applyActivatedSession(
            ActivatedSession(
                sessionId = "uuid-55",
                transcript = listOf(Message(role = com.lingxi.code.model.Role.User, text = "hi")),
                kind = SessionActivationKind.Resumed,
            ),
        )

        assertEquals("uuid-55", vm.state.value.session.id)
        assertEquals("差旅规划", vm.state.value.session.title) // preserved, not a placeholder
        assertEquals(listOf("hi"), vm.state.value.messages.map { it.text })
    }

    @Test
    fun startNewSession_resetsTranscriptLocally_andSubmitsNewSession() = runTest(dispatcher) {
        val source = FakeSessionSource(MutableStateFlow(EngineSessionState.loading()))
        val vm = ChatViewModel(source)

        // Seed a non-empty transcript, then start a new session.
        vm.reduce(ReplyEvent.Delta("old turn"))
        vm.startNewSession()

        assertTrue("a fresh session starts empty", vm.state.value.messages.isEmpty())
        assertTrue(vm.state.value.isNew)
        assertEquals(1, source.newSessionCalls)
    }

    @Test
    fun refreshSessions_drivesSourceRefresh() = runTest(dispatcher) {
        val source = FakeSessionSource(MutableStateFlow(EngineSessionState.loading()))
        val vm = ChatViewModel(source)
        vm.refreshSessions()
        assertEquals(1, source.refreshCalls)
    }

    // A no-op smoke: the default ConversationSource session surface starts in
    // loading, not in a mock-filled state.
    @Test
    fun defaultSource_startsLoadingSessionState() {
        val inert = object : ConversationSource {
            override fun initialMessages(): List<Message> = emptyList()
            override fun submit(text: String): Flow<ReplyEvent> = emptyFlow()
        }
        assertTrue(inert.sessionState.value.isLoading)
        // And reduceModelEvent / reduceSessionEvent remain independent surfaces.
        assertFalse(EngineModelState().hasCatalog)
    }
}
