package com.lingxi.code.conversation

import com.lingxi.code.bindings.ClientEvent
import com.lingxi.code.bindings.ErrorKindDto
import com.lingxi.code.model.EngineModelCatalog
import com.lingxi.code.model.EngineModelState
import com.lingxi.code.model.Message
import com.lingxi.code.model.MockData
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
import org.junit.Assert.assertSame
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Test

/**
 * SHIP-BLOCKER #2 coverage: the OUT-OF-BAND model-state path that drives the
 * picker from the engine's REAL `ModelList` / `ModelChanged` instead of the
 * branded `lx-*` mock catalog.
 *
 * Three pure layers, none of which build a native engine (`buildAndroidEngine`
 * is never touched):
 *  - [reduceModelEvent]      — folds one [ClientEvent] into [EngineModelState].
 *  - [EngineModelCatalog]    — turns real wire ids into friendly picker rows.
 *  - [ChatViewModel.applyModelState] — reflects the engine state into [ChatState].
 *
 * The fixtures only CONSTRUCT generated UniFFI data classes (no exported call),
 * so no `.so` is loaded.
 */
@OptIn(ExperimentalCoroutinesApi::class)
class ModelStateTest {

    private val dispatcher = UnconfinedTestDispatcher()

    @Before fun setMain() = Dispatchers.setMain(dispatcher)

    @After fun tearDown() = Dispatchers.resetMain()

    // --- reduceModelEvent: ModelList --------------------------------------

    @Test
    fun modelList_replacesCatalog_andAdoptsCurrent() {
        val next = reduceModelEvent(
            EngineModelState(),
            ClientEvent.ModelList(
                models = listOf("claude-opus-4-20250514", "claude-sonnet-4-20250514"),
                current = "claude-sonnet-4-20250514",
            ),
        )
        assertEquals(listOf("claude-opus-4-20250514", "claude-sonnet-4-20250514"), next.available)
        assertEquals("claude-sonnet-4-20250514", next.active)
        assertTrue(next.hasCatalog)
    }

    @Test
    fun modelList_replacesAPriorCatalog_wholesale() {
        val prev = EngineModelState(
            available = listOf("old-a", "old-b"),
            active = "old-a",
        )
        val next = reduceModelEvent(
            prev,
            ClientEvent.ModelList(models = listOf("claude-haiku-4"), current = "claude-haiku-4"),
        )
        assertEquals(listOf("claude-haiku-4"), next.available)
        assertEquals("claude-haiku-4", next.active)
    }

    // --- reduceModelEvent: ModelChanged -----------------------------------

    @Test
    fun modelChanged_swapsActive_keepingCatalog() {
        val prev = EngineModelState(
            available = listOf("claude-opus-4", "claude-sonnet-4"),
            active = "claude-opus-4",
        )
        val next = reduceModelEvent(prev, ClientEvent.ModelChanged(model = "claude-sonnet-4"))
        assertEquals(prev.available, next.available) // catalog untouched
        assertEquals("claude-sonnet-4", next.active)
    }

    @Test
    fun modelChanged_beforeAnyList_leavesCatalogEmpty() {
        val next = reduceModelEvent(EngineModelState(), ClientEvent.ModelChanged(model = "claude-opus-4"))
        assertEquals("claude-opus-4", next.active)
        assertFalse(next.hasCatalog) // a changed-without-list still has no catalog
    }

    // --- reduceModelEvent: non-model events are inert ---------------------

    @Test
    fun nonModelEvent_returnsPrevUnchanged_identity() {
        val prev = EngineModelState(available = listOf("claude-opus-4"), active = "claude-opus-4")
        // TextDelta / Error / TurnStarted must NOT disturb the model state.
        assertSame(prev, reduceModelEvent(prev, ClientEvent.TextDelta("hi")))
        assertSame(prev, reduceModelEvent(prev, ClientEvent.TurnStarted(turnId = null)))
        assertSame(
            prev,
            reduceModelEvent(prev, ClientEvent.Error(kind = ErrorKindDto.TRANSPORT, message = "x")),
        )
    }

    // --- EngineModelCatalog: ids -> friendly rows (id IS the wire id) -----

    @Test
    fun options_preserveWireId_verbatim() {
        val ids = listOf("claude-opus-4-20250514", "claude-3-5-sonnet-latest")
        val opts = EngineModelCatalog.options(ids)
        // The id carried by each ModelOption MUST be the real wire id (SetModel sends it).
        assertEquals(ids, opts.map { it.id })
    }

    @Test
    fun options_emptyForEmptyIds() {
        assertTrue(EngineModelCatalog.options(emptyList()).isEmpty())
    }

    @Test
    fun displayName_dropsDateStamp_andTitleCases() {
        assertEquals("Claude Opus 4", EngineModelCatalog.displayName("claude-opus-4-20250514"))
        assertEquals("Claude Sonnet 4", EngineModelCatalog.displayName("claude-sonnet-4-20250514"))
    }

    @Test
    fun displayName_unknownShape_stillRenders_neverBlank() {
        // An unfamiliar id is title-cased segment-by-segment — never blank.
        assertEquals("Gpt Ish Model", EngineModelCatalog.displayName("gpt-ish-model"))
        // A single-token id with no recognizable shape still renders (capitalized).
        assertEquals("Mymodel", EngineModelCatalog.displayName("mymodel"))
    }

    // --- ChatViewModel.applyModelState integration ------------------------

    /** A source whose model state we can drive; submit/streams are inert. */
    private class FakeModelSource(
        private val models: MutableStateFlow<EngineModelState>,
    ) : ConversationSource {
        val setModelCalls = mutableListOf<String>()
        override fun initialMessages(): List<Message> = emptyList()
        override fun submit(text: String): Flow<ReplyEvent> = emptyFlow()
        override val modelState: StateFlow<EngineModelState> = models.asStateFlow()
        override suspend fun setModel(id: String) { setModelCalls += id }
    }

    @Test
    fun emptyCatalog_keepsMockModels_asTheUiFallback() {
        val vm = ChatViewModel(FakeModelSource(MutableStateFlow(EngineModelState())))
        // No engine catalog → the picker still shows the branded mock list.
        assertEquals(MockData.models, vm.state.value.availableModels)
        assertEquals(MockData.models.first(), vm.state.value.model)
    }

    @Test
    fun engineCatalog_drivesPicker_withRealIds_andActiveSelection() {
        val flow = MutableStateFlow(EngineModelState())
        val vm = ChatViewModel(FakeModelSource(flow))

        // The engine reports its real catalog out-of-band (the ListModels reply).
        flow.value = EngineModelState(
            available = listOf("claude-opus-4-20250514", "claude-sonnet-4-20250514"),
            active = "claude-sonnet-4-20250514",
        )

        val s = vm.state.value
        assertEquals(
            listOf("claude-opus-4-20250514", "claude-sonnet-4-20250514"),
            s.availableModels.map { it.id },
        )
        // The active row is the engine's current id — a REAL wire id, not "lx-72b".
        assertEquals("claude-sonnet-4-20250514", s.model.id)
    }

    @Test
    fun modelChanged_reSelectsActiveRow_fromTheLiveCatalog() {
        val flow = MutableStateFlow(
            EngineModelState(
                available = listOf("claude-opus-4-20250514", "claude-sonnet-4-20250514"),
                active = "claude-opus-4-20250514",
            ),
        )
        val vm = ChatViewModel(FakeModelSource(flow))
        assertEquals("claude-opus-4-20250514", vm.state.value.model.id)

        // The engine confirms a switch (ModelChanged folds into active).
        flow.value = flow.value.copy(active = "claude-sonnet-4-20250514")
        assertEquals("claude-sonnet-4-20250514", vm.state.value.model.id)
    }

    @Test
    fun activeIdAbsentFromCatalog_fallsBackToFirstRow() {
        val flow = MutableStateFlow(
            EngineModelState(available = listOf("claude-opus-4"), active = "ghost-id"),
        )
        val vm = ChatViewModel(FakeModelSource(flow))
        // A stale/unknown active id must not crash — it selects the first row.
        assertEquals("claude-opus-4", vm.state.value.model.id)
    }

    @Test
    fun selectModel_submitsRealWireId_toEngine() = runTest {
        val flow = MutableStateFlow(
            EngineModelState(
                available = listOf("claude-opus-4-20250514", "claude-sonnet-4-20250514"),
                active = "claude-opus-4-20250514",
            ),
        )
        val source = FakeModelSource(flow)
        val vm = ChatViewModel(source)

        val sonnet = vm.state.value.availableModels.first { it.id == "claude-sonnet-4-20250514" }
        vm.selectModel(sonnet)

        // The pick reflects locally AND submits the REAL wire id via SetModel.
        assertEquals("claude-sonnet-4-20250514", vm.state.value.model.id)
        assertEquals(listOf("claude-sonnet-4-20250514"), source.setModelCalls)
    }
}
