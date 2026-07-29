package com.lingxi.code.conversation

import com.lingxi.code.bindings.ClientEvent
import com.lingxi.code.bindings.ErrorKindDto
import com.lingxi.code.model.EngineModelCatalog
import com.lingxi.code.model.EngineModelState
import com.lingxi.code.model.Message
import com.lingxi.code.model.ModelProviderStatus
import com.lingxi.code.model.ConnStatus
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
        val ids = listOf(
            "anthropic/claude-opus-4-20250514",
            "openrouter/anthropic/claude-3-5-sonnet-latest",
        )
        val opts = EngineModelCatalog.options(ids)
        // ModelOption.id MUST remain the complete qualified ref (SetModel sends it).
        assertEquals(ids, opts.map { it.id })
        assertEquals(listOf("anthropic", "openrouter"), opts.map { it.providerId })
        assertEquals(
            listOf("claude-opus-4-20250514", "anthropic/claude-3-5-sonnet-latest"),
            opts.map { it.desc },
        )
    }

    @Test
    fun options_emptyForEmptyIds() {
        assertTrue(EngineModelCatalog.options(emptyList()).isEmpty())
    }

    @Test
    fun displayName_dropsDateStamp_andTitleCases() {
        assertEquals("Claude Opus 4", EngineModelCatalog.displayName("claude-opus-4-20250514"))
        assertEquals("Claude Sonnet 4", EngineModelCatalog.displayName("claude-sonnet-4-20250514"))
        assertEquals("GPT 5.2", EngineModelCatalog.displayName("openai/gpt-5.2"))
        assertEquals("DeepSeek V3.2", EngineModelCatalog.displayName("deepseek/deepseek-v3.2"))
        assertEquals("Kimi K3", EngineModelCatalog.displayName("kimi/kimi-k3"))
    }

    @Test
    fun displayName_unknownShape_stillRenders_neverBlank() {
        // An unfamiliar id is title-cased segment-by-segment — never blank.
        assertEquals("GPT Ish Model", EngineModelCatalog.displayName("gpt-ish-model"))
        // A single-token id with no recognizable shape still renders (capitalized).
        assertEquals("Mymodel", EngineModelCatalog.displayName("mymodel"))
    }

    @Test
    fun providerDisplayName_hasStableBrands_andCustomFallback() {
        assertEquals("OpenAI", EngineModelCatalog.providerDisplayName("openai"))
        assertEquals("DeepSeek", EngineModelCatalog.providerDisplayName("deepseek"))
        assertEquals("Kimi", EngineModelCatalog.providerDisplayName("kimi"))
        assertEquals("Kimi Code", EngineModelCatalog.providerDisplayName("kimi-code"))
        assertEquals("GitHub Copilot", EngineModelCatalog.providerDisplayName("github-copilot"))
        assertEquals("Team Proxy", EngineModelCatalog.providerDisplayName("team-proxy"))
    }

    @Test
    fun groups_onlyIncomingCuratedModels_preservingProviderAndModelOrder() {
        val ids = listOf(
            "openai/gpt-5.2",
            "anthropic/claude-sonnet-4-5-20250929",
            "openai/o3",
            "deepseek/deepseek-v3.2",
        )
        val groups = EngineModelCatalog.groups(EngineModelCatalog.options(ids))

        assertEquals(listOf("openai", "anthropic", "deepseek"), groups.map { it.id })
        assertEquals(listOf("OpenAI", "Anthropic", "DeepSeek"), groups.map { it.name })
        assertEquals(
            listOf("openai/gpt-5.2", "openai/o3"),
            groups.first().models.map { it.id },
        )
        assertEquals(ids.toSet(), groups.flatMap { it.models }.map { it.id }.toSet())
        assertEquals(ids.size, groups.sumOf { it.models.size })
    }

    @Test
    fun sameRequestModelFromDifferentProviders_remainsDistinctAndSelectable() {
        val ids = listOf("openai/gpt-5.2", "openrouter/gpt-5.2")
        val groups = EngineModelCatalog.groups(EngineModelCatalog.options(ids))

        assertEquals(2, groups.size)
        assertEquals(ids, groups.flatMap { it.models }.map { it.id })
        assertEquals(listOf("GPT 5.2", "GPT 5.2"), groups.flatMap { it.models }.map { it.name })
    }

    @Test
    fun curatedModelMetadata_exposesThinkingContextAndPublishedSize() {
        val options = EngineModelCatalog.options(
            listOf(
                "deepseek/deepseek-v4-flash",
                "anthropic/claude-sonnet-5",
                "openrouter/openrouter/auto",
            ),
        )

        assertEquals("Thinking", options[0].metadata.thinking)
        assertEquals("1M 上下文", options[0].metadata.contextWindow)
        assertEquals("284B / 13B 激活", options[0].metadata.parameterSize)

        assertEquals("自适应 Thinking", options[1].metadata.thinking)
        assertEquals("1M 上下文", options[1].metadata.contextWindow)
        assertEquals("参数未公开", options[1].metadata.parameterSize)

        assertEquals("动态路由", options[2].metadata.thinking)
        assertEquals("规格随实际模型", options[2].metadata.contextWindow)
    }

    @Test
    fun filter_matchesModelProviderWireIdAndMetadata() {
        val options = EngineModelCatalog.options(
            listOf(
                "deepseek/deepseek-v4-flash",
                "anthropic/claude-sonnet-5",
                "openrouter/openrouter/auto",
            ),
        )

        assertEquals(
            listOf("deepseek/deepseek-v4-flash"),
            EngineModelCatalog.filter(options, "284B").map { it.id },
        )
        assertEquals(
            listOf("anthropic/claude-sonnet-5"),
            EngineModelCatalog.filter(options, "Anthropic").map { it.id },
        )
        assertEquals(
            listOf("deepseek/deepseek-v4-flash", "anthropic/claude-sonnet-5"),
            EngineModelCatalog.filter(options, "thinking").map { it.id },
        )
        assertEquals(options, EngineModelCatalog.filter(options, "  "))
    }

    @Test
    fun providerStatus_prefersEnabledProfile_andOnlyConfiguredOrConnectedAreSelectable() {
        val statuses = listOf(
            ModelProviderStatus(
                profileId = "deepseek",
                settingsId = "disabled",
                name = "DeepSeek",
                status = ConnStatus.Connected,
                enabled = false,
                credentialConfigured = true,
            ),
            ModelProviderStatus(
                profileId = "deepseek",
                settingsId = "enabled",
                name = "DeepSeek",
                status = ConnStatus.Configured,
                enabled = true,
                credentialConfigured = true,
            ),
        )

        val resolved = EngineModelCatalog.providerStatus("deepseek", statuses)
        assertEquals("enabled", resolved?.settingsId)
        assertEquals("已配置", resolved?.displayLabel)
        assertTrue(resolved?.canSelect == true)

        assertFalse(
            ModelProviderStatus(
                profileId = "openrouter",
                settingsId = "failed",
                name = "OpenRouter",
                status = ConnStatus.Error,
                enabled = true,
                credentialConfigured = true,
            ).canSelect,
        )
        assertEquals(
            "未配置",
            ModelProviderStatus(
                profileId = "anthropic",
                settingsId = "missing-key",
                name = "Anthropic",
                status = ConnStatus.Idle,
                enabled = true,
                credentialConfigured = false,
            ).displayLabel,
        )
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
    fun emptyCatalog_staysExplicitlyPending_withoutFakeModels() {
        val vm = ChatViewModel(FakeModelSource(MutableStateFlow(EngineModelState())))
        assertTrue(vm.state.value.availableModels.isEmpty())
        assertEquals(EngineModelCatalog.pending, vm.state.value.model)
    }

    @Test
    fun engineCatalog_drivesPicker_withRealIds_andActiveSelection() {
        val flow = MutableStateFlow(EngineModelState())
        val vm = ChatViewModel(FakeModelSource(flow))

        // The engine reports its real catalog out-of-band (the ListModels reply).
        flow.value = EngineModelState(
            available = listOf(
                "anthropic/claude-opus-4-20250514",
                "anthropic/claude-sonnet-4-20250514",
            ),
            active = "anthropic/claude-sonnet-4-20250514",
        )

        val s = vm.state.value
        assertEquals(
            listOf(
                "anthropic/claude-opus-4-20250514",
                "anthropic/claude-sonnet-4-20250514",
            ),
            s.availableModels.map { it.id },
        )
        // The active row is the engine's qualified id — never a branded mock id.
        assertEquals("anthropic/claude-sonnet-4-20250514", s.model.id)
    }

    @Test
    fun modelChanged_reSelectsActiveRow_fromTheLiveCatalog() {
        val flow = MutableStateFlow(
            EngineModelState(
                available = listOf(
                    "anthropic/claude-opus-4-20250514",
                    "anthropic/claude-sonnet-4-20250514",
                ),
                active = "anthropic/claude-opus-4-20250514",
            ),
        )
        val vm = ChatViewModel(FakeModelSource(flow))
        assertEquals("anthropic/claude-opus-4-20250514", vm.state.value.model.id)

        // The engine confirms a switch (ModelChanged folds into active).
        flow.value = flow.value.copy(active = "anthropic/claude-sonnet-4-20250514")
        assertEquals("anthropic/claude-sonnet-4-20250514", vm.state.value.model.id)
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
                available = listOf(
                    "anthropic/claude-opus-4-20250514",
                    "anthropic/claude-sonnet-4-20250514",
                ),
                active = "anthropic/claude-opus-4-20250514",
            ),
        )
        val source = FakeModelSource(flow)
        val vm = ChatViewModel(source)

        val sonnet = vm.state.value.availableModels.first {
            it.id == "anthropic/claude-sonnet-4-20250514"
        }
        vm.selectModel(sonnet)

        // The pick reflects locally AND submits the complete qualified ref.
        assertEquals("anthropic/claude-sonnet-4-20250514", vm.state.value.model.id)
        assertEquals(listOf("anthropic/claude-sonnet-4-20250514"), source.setModelCalls)
    }
}
