package com.lingxi.code.localapps

import com.lingxi.code.bindings.AppAuthorizationDecisionDto
import com.lingxi.code.bindings.AppBridgeResponseDto
import com.lingxi.code.bindings.AppCapabilityKindDto
import com.lingxi.code.bindings.AppCreateOriginDto
import com.lingxi.code.bindings.AppErrorCodeDto
import com.lingxi.code.bindings.AppCapabilityRequestDto
import com.lingxi.code.bindings.AppRecordDto
import com.lingxi.code.bindings.AppSessionKindDto
import com.lingxi.code.bindings.AppSessionRowDto
import com.lingxi.code.bindings.AppSurfaceDto
import com.lingxi.code.bindings.AppUiActionKindDto
import com.lingxi.code.bindings.AppUiRequestDto
import com.lingxi.code.bindings.AppUiTargetDto
import com.lingxi.code.bindings.AppWorkflowStateDto
import com.lingxi.code.bindings.ClientCommand
import com.lingxi.code.bindings.ClientEvent
import com.lingxi.code.bindings.AppEventDto
import com.lingxi.code.conversation.ConversationSource
import com.lingxi.code.conversation.ReplyEvent
import com.lingxi.code.model.EngineModelState
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.async
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.asSharedFlow
import kotlinx.coroutines.flow.collect
import kotlinx.coroutines.flow.emptyFlow
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch
import kotlinx.coroutines.test.StandardTestDispatcher
import kotlinx.coroutines.test.advanceTimeBy
import kotlinx.coroutines.test.resetMain
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest
import kotlinx.coroutines.test.setMain
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

@OptIn(ExperimentalCoroutinesApi::class)
class LocalAppsViewModelTest {

    private class RecordingSource : ConversationSource {
        private val events = MutableSharedFlow<ClientEvent>(extraBufferCapacity = 32)
        val commands = mutableListOf<ClientCommand>()
        var commandFailure: Throwable? = null
        val models = MutableStateFlow(EngineModelState())

        override val clientEvents: Flow<ClientEvent> = events.asSharedFlow()
        override val modelState = models

        override suspend fun submitClientCommand(command: ClientCommand) {
            commandFailure?.let { throw it }
            commands += command
        }

        override fun submit(text: String): Flow<ReplyEvent> = emptyFlow()

        fun emit(event: ClientEvent) {
            assertTrue("LocalAppsViewModel must subscribe before test events", events.tryEmit(event))
        }
    }

    private class RecordingWebStorageCleanup : LocalAppWebStorageCleanup {
        val prepared = mutableListOf<Pair<String, String?>>()
        val snapshots = mutableListOf<Set<String>>()
        val cancelled = mutableListOf<String>()
        var retried = false
        var prepareSucceeds = true

        override fun prepareDeletion(appId: String, currentUrl: String?): Boolean {
            prepared += appId to currentUrl
            return prepareSucceeds
        }

        override fun cancelDeletion(appId: String) {
            cancelled += appId
        }

        override fun reconcile(liveAppIds: Set<String>) {
            snapshots += liveAppIds
        }

        override fun retryConfirmed() {
            retried = true
        }
    }

    /// The create sheet creates the app OUTRIGHT, carrying the name and the
    /// surface the user confirmed.
    ///
    /// Both are fixed at creation — a surface is immutable once scaffolded and
    /// apps have no rename — so neither may be left to a value the user never
    /// saw, and neither may travel through the model.
    @Test
    fun `create sends CreateApp with the confirmed name and surface`() = runTest {
        Dispatchers.setMain(StandardTestDispatcher(testScheduler))
        try {
            val source = RecordingSource()
            val viewModel = LocalAppsViewModel(
                sourceFlow = MutableStateFlow<ConversationSource>(source),
                distributionChannel = "store",
            )
            runCurrent()

            val brief = "一个能记录每天喝水量的小工具，支持提醒和每周汇总"
            viewModel.onAction(
                LocalAppsAction.CreateFromBrief(
                    brief = brief,
                    name = "喝水记录",
                    surface = LocalAppSurface.CANVAS,
                    gitEnabled = false,
                    addWidget = true,
                ),
            )
            runCurrent()

            val create = source.commands.filterIsInstance<ClientCommand.CreateApp>().single()
            assertEquals("the brief must reach the engine verbatim", brief, create.brief)
            assertEquals("喝水记录", create.name)
            assertEquals(AppSurfaceDto.CANVAS, create.surface)
            assertEquals(false, create.gitEnabled)
            assertEquals(AppCreateOriginDto.LIBRARY, create.origin)
            assertNull(
                "a library create binds no conversation — none exists yet",
                create.conversationId,
            )
        } finally {
            Dispatchers.resetMain()
        }
    }

    /// The routed shape is the default a sheet sends when the user changed
    /// nothing, and it is what an absent surface would have meant anyway.
    @Test
    fun `create defaults to the routed surface`() = runTest {
        Dispatchers.setMain(StandardTestDispatcher(testScheduler))
        try {
            val source = RecordingSource()
            val viewModel = LocalAppsViewModel(
                sourceFlow = MutableStateFlow<ConversationSource>(source),
                distributionChannel = "store",
            )
            runCurrent()

            viewModel.onAction(LocalAppsAction.CreateFromBrief("一个记事本"))
            runCurrent()

            val create = source.commands.filterIsInstance<ClientCommand.CreateApp>().single()
            assertEquals(AppSurfaceDto.DOM, create.surface)
        } finally {
            Dispatchers.resetMain()
        }
    }

    /// `ProposeAppIdentity` is a REAL command, and its answer is routed back to
    /// the ONE caller that asked.
    @Test
    fun `proposeIdentity asks the host and adopts the answer`() = runTest {
        Dispatchers.setMain(StandardTestDispatcher(testScheduler))
        try {
            val source = RecordingSource()
            val viewModel = LocalAppsViewModel(
                sourceFlow = MutableStateFlow<ConversationSource>(source),
                distributionChannel = "store",
            )
            runCurrent()

            val answer = async { viewModel.proposeIdentity("一个打砖块游戏") }
            runCurrent()

            // Reply on the request id the view model actually sent. Echoing a
            // fabricated one would let this pass against a view model that
            // never correlates at all.
            val ask = source.commands
                .filterIsInstance<ClientCommand.ProposeAppIdentity>()
                .single()
            assertEquals("一个打砖块游戏", ask.brief)
            source.emit(
                ClientEvent.AppIdentityProposed(
                    requestId = ask.requestId,
                    name = "打砖块",
                    surface = AppSurfaceDto.CANVAS,
                ),
            )
            runCurrent()

            val proposal = answer.await()
            assertEquals("打砖块", proposal.name)
            assertEquals(LocalAppSurface.CANVAS, proposal.surface)
        } finally {
            Dispatchers.resetMain()
        }
    }

    /// An answer for a DIFFERENT request must not resolve this one.
    ///
    /// A sheet that was retyped and re-submitted has a stale proposal in
    /// flight; adopting it would name the app after the abandoned brief.
    @Test
    fun `proposeIdentity falls back rather than adopt another request's answer`() = runTest {
        Dispatchers.setMain(StandardTestDispatcher(testScheduler))
        try {
            val source = RecordingSource()
            val viewModel = LocalAppsViewModel(
                sourceFlow = MutableStateFlow<ConversationSource>(source),
                distributionChannel = "store",
            )
            runCurrent()

            val answer = async { viewModel.proposeIdentity("一个记事本") }
            runCurrent()
            source.emit(
                ClientEvent.AppIdentityProposed(
                    requestId = "somebody-elses",
                    name = "别的应用",
                    surface = AppSurfaceDto.CANVAS,
                ),
            )
            runCurrent()
            // Nothing resolves it, so the timeout does — the same derived
            // default the host itself falls back to.
            advanceTimeBy(LocalAppsViewModel.IDENTITY_PROPOSAL_TIMEOUT_MS + 1)
            runCurrent()

            val proposal = answer.await()
            assertEquals("一个记事本", proposal.name)
            assertEquals(LocalAppSurface.DOM, proposal.surface)
        } finally {
            Dispatchers.resetMain()
        }
    }

    /// The hand-off into the app's own conversation arms on `AppCreated` and is
    /// completed by the pin that arrives afterwards.
    ///
    /// `AppCreated` is emitted inside the create transaction and the init
    /// session is minted AFTER it, so the record on the create event never
    /// carries one. Waiting for the pin on the create event alone made the
    /// hand-off unreachable — the agent stayed in a conversation rooted outside
    /// the app and every build failed on the workspace.
    @Test
    fun `the landing arms on AppCreated and takes the pin from the record update`() = runTest {
        Dispatchers.setMain(StandardTestDispatcher(testScheduler))
        try {
            val source = RecordingSource()
            val viewModel = LocalAppsViewModel(
                sourceFlow = MutableStateFlow<ConversationSource>(source),
                distributionChannel = "store",
            )
            runCurrent()
            val landings = mutableListOf<LocalAppsViewModel.CreatedAppLanding>()
            val job = launch { viewModel.createdAppLandings.collect { landings += it } }

            viewModel.onAction(LocalAppsAction.CreateFromBrief("一个记事本"))
            runCurrent()
            // `initSessionId = null` is what the ENGINE actually sends here:
            // `AppCreated` is emitted inside the create transaction and the pin
            // is minted afterwards. The shared fixture defaults it to a uuid,
            // which would let this pass against a view model that only ever
            // read the create event.
            val created = appRecord(id = "notes", name = "记事本", brief = "一个记事本")
                .copy(initSessionId = null)
            source.emit(ClientEvent.AppsChanged(listOf(created)))
            source.emit(ClientEvent.AppEvent(AppEventDto.AppCreated(created)))
            runCurrent()

            assertTrue("AppCreated alone must not land: the pin is not minted yet", landings.isEmpty())

            source.emit(
                ClientEvent.AppEvent(
                    AppEventDto.AppRecordChanged(
                        created.copy(initSessionId = "session-9"),
                    ),
                ),
            )
            runCurrent()

            val landing = landings.single()
            assertEquals("notes", landing.appId)
            assertEquals("session-9", landing.initSessionId)
            job.cancel()
        } finally {
            Dispatchers.resetMain()
        }
    }

    /// A create committed by an AGENT in another conversation must not be
    /// claimed by this sheet.
    ///
    /// `AppCreated` carries no correlator back to the client that asked, so an
    /// unkeyed claim opens whichever app committed first — the user lands in
    /// someone else's app and their own create never lands at all.
    @Test
    fun `another conversation's create is not claimed by the sheet`() = runTest {
        Dispatchers.setMain(StandardTestDispatcher(testScheduler))
        try {
            val source = RecordingSource()
            val viewModel = LocalAppsViewModel(
                sourceFlow = MutableStateFlow<ConversationSource>(source),
                distributionChannel = "store",
            )
            runCurrent()
            val landings = mutableListOf<LocalAppsViewModel.CreatedAppLanding>()
            val job = launch { viewModel.createdAppLandings.collect { landings += it } }

            viewModel.onAction(LocalAppsAction.CreateFromBrief("我的记事本"))
            runCurrent()

            // The agent's app commits first.
            val theirs = appRecord(id = "theirs", brief = "代理自己的应用")
                .copy(initSessionId = null)
            source.emit(ClientEvent.AppsChanged(listOf(theirs)))
            source.emit(ClientEvent.AppEvent(AppEventDto.AppCreated(theirs)))
            source.emit(ClientEvent.AppEvent(AppEventDto.AppRecordChanged(theirs)))
            runCurrent()
            assertTrue("not this sheet's app", landings.isEmpty())

            // Mine commits afterwards and is still claimable.
            val mine = appRecord(id = "mine", brief = "我的记事本").copy(initSessionId = null)
            source.emit(ClientEvent.AppsChanged(listOf(theirs, mine)))
            source.emit(ClientEvent.AppEvent(AppEventDto.AppCreated(mine)))
            source.emit(ClientEvent.AppEvent(AppEventDto.AppRecordChanged(mine)))
            runCurrent()

            assertEquals("mine", landings.single().appId)
            job.cancel()
        } finally {
            Dispatchers.resetMain()
        }
    }

    /// A create whose best-effort init-session mint FAILED must still land.
    ///
    /// The engine announces the record either way. Landing on a fresh
    /// conversation is still correct: the SCOPE, not the session, is what roots
    /// the agent in the app workspace.
    @Test
    fun `the landing still fires when no init session was pinned`() = runTest {
        Dispatchers.setMain(StandardTestDispatcher(testScheduler))
        try {
            val source = RecordingSource()
            val viewModel = LocalAppsViewModel(
                sourceFlow = MutableStateFlow<ConversationSource>(source),
                distributionChannel = "store",
            )
            runCurrent()
            val landings = mutableListOf<LocalAppsViewModel.CreatedAppLanding>()
            val job = launch { viewModel.createdAppLandings.collect { landings += it } }

            viewModel.onAction(LocalAppsAction.CreateFromBrief("一个记事本"))
            runCurrent()
            // No pin on either event — the engine announces the record anyway
            // when the best-effort mint fails, and that announcement is what
            // the hand-off rides.
            val created = appRecord(id = "notes", name = "记事本", brief = "一个记事本")
                .copy(initSessionId = null)
            source.emit(ClientEvent.AppsChanged(listOf(created)))
            source.emit(ClientEvent.AppEvent(AppEventDto.AppCreated(created)))
            source.emit(ClientEvent.AppEvent(AppEventDto.AppRecordChanged(created)))
            runCurrent()

            val landing = landings.single()
            assertEquals("notes", landing.appId)
            assertNull("no pin was minted", landing.initSessionId)
            job.cancel()
        } finally {
            Dispatchers.resetMain()
        }
    }

    /// `AppCreated` names the record the engine just committed, and the claim
    /// is keyed on the brief the sheet sent.
    ///
    /// A previous version pinned `brief = "AGENT REWROTE THIS"` to encode the
    /// deferred flow, where the agent rewrote the brief before creating
    /// anything and matching on it was therefore impossible. Create-first
    /// reverses that: the sheet sends the user's own sentence and
    /// `AppService::create_app` stores it verbatim, so the match is exact — and
    /// it has to be, because `AppCreated` carries no correlator and a
    /// concurrent agent-driven create would otherwise be claimed instead
    /// (see `another conversation's create is not claimed by the sheet`).
    @Test
    fun `AppCreated opens the new app and fires the widget pin`() = runTest {
        Dispatchers.setMain(StandardTestDispatcher(testScheduler))
        try {
            val source = RecordingSource()
            val viewModel = LocalAppsViewModel(
                sourceFlow = MutableStateFlow<ConversationSource>(source),
                distributionChannel = "store",
            )
            runCurrent()

            viewModel.onAction(
                LocalAppsAction.CreateFromBrief("一个带桌面快捷入口的记事本", addWidget = true),
            )
            runCurrent()
            val created = appRecord(
                id = "widget-app", name = "喝水记录", brief = "一个带桌面快捷入口的记事本",
            )
            source.emit(ClientEvent.AppsChanged(listOf(created)))
            source.emit(ClientEvent.AppEvent(AppEventDto.AppCreated(created)))
            runCurrent()

            assertEquals("widget-app", viewModel.widgetPinRequests.first())
        } finally {
            Dispatchers.resetMain()
        }
    }

    /// An app that simply APPEARS is not the one this session asked for.
    ///
    /// Another device, a background flow, or the agent creating a second app can
    /// all put a new row in the catalog. Only `AppCreated` claims one.
    @Test
    fun `a new app that arrives without AppCreated is not claimed`() = runTest {
        Dispatchers.setMain(StandardTestDispatcher(testScheduler))
        try {
            val source = RecordingSource()
            val viewModel = LocalAppsViewModel(
                sourceFlow = MutableStateFlow<ConversationSource>(source),
                distributionChannel = "store",
            )
            runCurrent()

            val pins = mutableListOf<String>()
            val pinJob = launch { viewModel.widgetPinRequests.collect { pins += it } }

            val brief = "一个带桌面快捷入口的记事本"
            viewModel.onAction(LocalAppsAction.CreateFromBrief(brief, addWidget = true))
            runCurrent()
            source.emit(
                ClientEvent.AppsChanged(listOf(appRecord(id = "someone-elses", brief = brief))),
            )
            runCurrent()

            assertTrue(
                "a catalog row alone must not be claimed, however well its brief matches",
                pins.isEmpty(),
            )
            pinJob.cancel()
        } finally {
            Dispatchers.resetMain()
        }
    }

    /// A global failure has to disarm the claim, or the sheet stays wedged: the
    /// agent-driven create emits no client event of its own on failure, so this
    /// is the only signal that can release it.
    @Test
    fun `a global operation failure disarms the in-flight create`() = runTest {
        Dispatchers.setMain(StandardTestDispatcher(testScheduler))
        try {
            val source = RecordingSource()
            val viewModel = LocalAppsViewModel(
                sourceFlow = MutableStateFlow<ConversationSource>(source),
                distributionChannel = "store",
            )
            runCurrent()

            val pins = mutableListOf<String>()
            val pinJob = launch { viewModel.widgetPinRequests.collect { pins += it } }

            viewModel.onAction(LocalAppsAction.CreateFromBrief("一个记事本", addWidget = true))
            runCurrent()
            source.emit(
                ClientEvent.AppOperationFailed(
                    appId = null,
                    code = AppErrorCodeDto.WORKFLOW_STATE_INVALID,
                    message = "创建失败",
                ),
            )
            runCurrent()
            val later = appRecord(id = "assistant-app", brief = "别的应用")
            source.emit(ClientEvent.AppsChanged(listOf(later)))
            source.emit(ClientEvent.AppEvent(AppEventDto.AppCreated(later)))
            runCurrent()

            assertTrue("a disarmed create must not pin a later app", pins.isEmpty())
            pinJob.cancel()
        } finally {
            Dispatchers.resetMain()
        }
    }

    /// A PER-APP failure belongs to some other app and must leave this claim
    /// alone.
    @Test
    fun `a per-app operation failure does not disarm an in-flight create`() = runTest {
        Dispatchers.setMain(StandardTestDispatcher(testScheduler))
        try {
            val source = RecordingSource()
            val viewModel = LocalAppsViewModel(
                sourceFlow = MutableStateFlow<ConversationSource>(source),
                distributionChannel = "store",
            )
            runCurrent()

            viewModel.onAction(LocalAppsAction.CreateFromBrief("一个记事本", addWidget = true))
            runCurrent()
            source.emit(
                ClientEvent.AppOperationFailed(
                    appId = "another-app",
                    code = AppErrorCodeDto.WORKFLOW_STATE_INVALID,
                    message = "别的应用失败了",
                ),
            )
            runCurrent()
            val created = appRecord(id = "mine", brief = "一个记事本")
            source.emit(ClientEvent.AppsChanged(listOf(created)))
            source.emit(ClientEvent.AppEvent(AppEventDto.AppCreated(created)))
            runCurrent()

            assertEquals("mine", viewModel.widgetPinRequests.first())
        } finally {
            Dispatchers.resetMain()
        }
    }

    @Test
    fun `a second create while one is in flight is rejected`() = runTest {
        Dispatchers.setMain(StandardTestDispatcher(testScheduler))
        try {
            val source = RecordingSource()
            val viewModel = LocalAppsViewModel(
                sourceFlow = MutableStateFlow<ConversationSource>(source),
                distributionChannel = "store",
            )
            runCurrent()

            viewModel.onAction(LocalAppsAction.CreateFromBrief("第一个"))
            runCurrent()
            viewModel.onAction(LocalAppsAction.CreateFromBrief("第二个"))
            runCurrent()

            val creates = source.commands.filterIsInstance<ClientCommand.CreateApp>()
            assertEquals("only one create may be in flight at a time", 1, creates.size)
            assertEquals("第一个", creates.single().brief)
        } finally {
            Dispatchers.resetMain()
        }
    }

    /**
     * The POSITIVE half of the workflow-model contract. Without it the only
     * surviving assertion is the rejection below, which still holds when the
     * field is dropped ENTIRELY — so a refactor that stopped forwarding the
     * user's chosen build model would ship green, and every build would
     * silently fall back to the session default. The value must arrive
     * provider-QUALIFIED: a bare wire id in this slot is a 404 at the provider.
     */
    @Test
    fun `create app forwards an explicit provider-qualified workflow model`() = runTest {
        Dispatchers.setMain(StandardTestDispatcher(testScheduler))
        try {
            val source = RecordingSource().apply {
                models.value = EngineModelState(
                    available = listOf("deepseek/deepseek-v3.2", "anthropic/claude-sonnet-4-6"),
                    active = "deepseek/deepseek-v3.2",
                )
            }
            val viewModel = LocalAppsViewModel(
                sourceFlow = MutableStateFlow<ConversationSource>(source),
                distributionChannel = "store",
            )
            runCurrent()

            viewModel.onAction(
                LocalAppsAction.CreateFromBrief(
                    brief = "一个小游戏",
                    workflowModel = "anthropic/claude-sonnet-4-6",
                ),
            )
            runCurrent()

            val create = source.commands.filterIsInstance<ClientCommand.CreateApp>().single()
            assertEquals("anthropic/claude-sonnet-4-6", create.workflowModel)
        } finally {
            Dispatchers.resetMain()
        }
    }

    @Test
    fun `create app rejects an explicit model that is not in the live catalog`() = runTest {
        Dispatchers.setMain(StandardTestDispatcher(testScheduler))
        try {
            val source = RecordingSource().apply {
                models.value = EngineModelState(
                    available = listOf("deepseek/deepseek-v3.2"),
                    active = "deepseek/deepseek-v3.2",
                )
            }
            val viewModel = LocalAppsViewModel(
                sourceFlow = MutableStateFlow<ConversationSource>(source),
                distributionChannel = "store",
            )
            runCurrent()

            viewModel.onAction(
                LocalAppsAction.CreateFromBrief(
                    brief = "一个小游戏",
                    workflowModel = "anthropic/removed-model",
                ),
            )
            runCurrent()

            assertTrue(source.commands.none { it is ClientCommand.CreateApp })
            assertTrue(viewModel.uiState.value.error?.isNotBlank() == true)
        } finally {
            Dispatchers.resetMain()
        }
    }

    @Test
    fun `opening an app lands on the sessions tab and requests details plus the first catalog page`() = runTest {
        Dispatchers.setMain(StandardTestDispatcher(testScheduler))
        try {
            val source = RecordingSource()
            val viewModel = LocalAppsViewModel(
                sourceFlow = MutableStateFlow<ConversationSource>(source),
                distributionChannel = "store",
            )
            runCurrent()
            source.emit(ClientEvent.AppsChanged(listOf(appRecord())))
            runCurrent()

            viewModel.onAction(LocalAppsAction.OpenApp(APP_ID))
            runCurrent()

            assertEquals(
                LocalAppsDestination.Details(APP_ID, LocalAppDetailsTab.Sessions),
                viewModel.uiState.value.destination,
            )
            assertEquals(LocalAppDetailsTab.Sessions, viewModel.uiState.value.selectedDetailsTab)
            assertTrue(source.commands.any { it == ClientCommand.GetAppDetails(APP_ID) })
            val listRequest = source.commands.filterIsInstance<ClientCommand.ListAppSessions>().single()
            assertEquals(APP_ID, listRequest.appId)
            assertNull("the first page starts at the catalog head", listRequest.offset)
        } finally {
            Dispatchers.resetMain()
        }
    }

    @Test
    fun `an app sessions reply pins the init row first and keeps the rest modified-descending`() = runTest {
        Dispatchers.setMain(StandardTestDispatcher(testScheduler))
        try {
            val source = RecordingSource()
            val viewModel = LocalAppsViewModel(
                sourceFlow = MutableStateFlow<ConversationSource>(source),
                distributionChannel = "store",
                nowEpochSeconds = { NOW_EPOCH_SECONDS },
            )
            runCurrent()

            source.emit(
                ClientEvent.AppSessionsChanged(
                    appId = APP_ID,
                    sessions = listOf(
                        sessionRow("s-newest", "最新讨论"),
                        sessionRow("s-init", "初始化会话", kind = AppSessionKindDto.INIT),
                        sessionRow("s-older", "老会话"),
                    ),
                    nextOffset = null,
                ),
            )
            runCurrent()

            val page = viewModel.uiState.value.appSessions[APP_ID]
                ?: throw AssertionError("the reply must populate the app's session page")
            assertTrue(page.loaded)
            assertEquals(
                "the pinned init session lists FIRST, ahead of newer conversations",
                listOf("s-init", "s-newest", "s-older"),
                page.rows.map { it.uuid },
            )
            assertTrue(page.rows.first().isInit)
            assertNull("no further page exists", page.nextOffset)
        } finally {
            Dispatchers.resetMain()
        }
    }

    @Test
    fun `a load-more reply appends to the cached catalog and a fresh reload replaces it`() = runTest {
        Dispatchers.setMain(StandardTestDispatcher(testScheduler))
        try {
            val source = RecordingSource()
            val viewModel = LocalAppsViewModel(
                sourceFlow = MutableStateFlow<ConversationSource>(source),
                distributionChannel = "store",
                nowEpochSeconds = { NOW_EPOCH_SECONDS },
            )
            runCurrent()

            viewModel.onAction(LocalAppsAction.LoadAppSessions(APP_ID, offset = null))
            runCurrent()
            source.emit(
                ClientEvent.AppSessionsChanged(
                    appId = APP_ID,
                    sessions = listOf(
                        sessionRow("s-init", "初始化会话", kind = AppSessionKindDto.INIT),
                        sessionRow("s-1", "第一页"),
                    ),
                    nextOffset = 2u,
                ),
            )
            runCurrent()
            assertEquals(2uL, viewModel.uiState.value.appSessions[APP_ID]?.nextOffset)

            // 「加载更多」: request the reported next offset — the reply APPENDS.
            viewModel.onAction(LocalAppsAction.LoadAppSessions(APP_ID, offset = 2u))
            runCurrent()
            val pageRequest = source.commands.filterIsInstance<ClientCommand.ListAppSessions>().last()
            assertEquals(2uL, pageRequest.offset)
            source.emit(
                ClientEvent.AppSessionsChanged(
                    appId = APP_ID,
                    sessions = listOf(sessionRow("s-2", "第二页")),
                    nextOffset = null,
                ),
            )
            runCurrent()
            assertEquals(
                listOf("s-init", "s-1", "s-2"),
                viewModel.uiState.value.appSessions[APP_ID]?.rows?.map { it.uuid },
            )
            assertNull(viewModel.uiState.value.appSessions[APP_ID]?.nextOffset)

            // A fresh first-page load REPLACES the accumulated rows (a deleted
            // session must not survive as a stale cached row).
            viewModel.onAction(LocalAppsAction.LoadAppSessions(APP_ID, offset = null))
            runCurrent()
            source.emit(
                ClientEvent.AppSessionsChanged(
                    appId = APP_ID,
                    sessions = listOf(sessionRow("s-init", "初始化会话", kind = AppSessionKindDto.INIT)),
                    nextOffset = null,
                ),
            )
            runCurrent()
            assertEquals(
                listOf("s-init"),
                viewModel.uiState.value.appSessions[APP_ID]?.rows?.map { it.uuid },
            )
        } finally {
            Dispatchers.resetMain()
        }
    }

    @Test
    fun `a workflow change event moves an app between draft and ready`() = runTest {
        Dispatchers.setMain(StandardTestDispatcher(testScheduler))
        try {
            val source = RecordingSource()
            val viewModel = LocalAppsViewModel(
                sourceFlow = MutableStateFlow<ConversationSource>(source),
                distributionChannel = "store",
            )
            runCurrent()
            source.emit(ClientEvent.AppsChanged(listOf(appRecord(workflow = AppWorkflowStateDto.DRAFT))))
            runCurrent()
            assertEquals(LocalAppWorkflow.Draft, viewModel.uiState.value.apps.single().workflow)

            source.emit(ClientEvent.AppWorkflowChanged(APP_ID, AppWorkflowStateDto.READY, detail = null))
            runCurrent()
            assertEquals(LocalAppWorkflow.Ready, viewModel.uiState.value.apps.single().workflow)
        } finally {
            Dispatchers.resetMain()
        }
    }

    @Test
    fun `delete queues web storage before command and confirms against AppsChanged`() = runTest {
        Dispatchers.setMain(StandardTestDispatcher(testScheduler))
        try {
            val source = RecordingSource()
            val cleanup = RecordingWebStorageCleanup()
            val viewModel = LocalAppsViewModel(
                sourceFlow = MutableStateFlow<ConversationSource>(source),
                distributionChannel = "store",
                webStorageCleanup = cleanup,
            )
            runCurrent()
            assertTrue(cleanup.retried)

            source.emit(ClientEvent.AppsChanged(listOf(appRecord())))
            runCurrent()
            viewModel._uiState.update { state ->
                state.copy(
                    apps = state.apps.map { app ->
                        app.copy(runtime = app.runtime.copy(url = "http://127.0.0.1:43100/preview"))
                    },
                )
            }
            viewModel.onAction(LocalAppsAction.DeleteApp(APP_ID))
            runCurrent()

            assertEquals(listOf(APP_ID to "http://127.0.0.1:43100/preview"), cleanup.prepared)
            assertTrue(source.commands.last() is ClientCommand.DeleteApp)

            source.emit(ClientEvent.AppsChanged(emptyList()))
            runCurrent()
            assertEquals(emptySet<String>(), cleanup.snapshots.last())
        } finally {
            Dispatchers.resetMain()
        }
    }

    @Test
    fun `app disappearance without delete does not journal browser cleanup`() = runTest {
        Dispatchers.setMain(StandardTestDispatcher(testScheduler))
        try {
            val source = RecordingSource()
            val cleanup = RecordingWebStorageCleanup()
            LocalAppsViewModel(
                sourceFlow = MutableStateFlow<ConversationSource>(source),
                distributionChannel = "store",
                webStorageCleanup = cleanup,
            )
            runCurrent()

            source.emit(ClientEvent.AppsChanged(listOf(appRecord())))
            runCurrent()
            source.emit(ClientEvent.AppsChanged(emptyList()))
            runCurrent()

            assertTrue(cleanup.prepared.isEmpty())
            assertEquals(emptySet<String>(), cleanup.snapshots.last())
        } finally {
            Dispatchers.resetMain()
        }
    }

    @Test
    fun `failed delete submission cancels unconfirmed browser cleanup journal`() = runTest {
        Dispatchers.setMain(StandardTestDispatcher(testScheduler))
        try {
            val source = RecordingSource()
            val cleanup = RecordingWebStorageCleanup()
            val viewModel = LocalAppsViewModel(
                sourceFlow = MutableStateFlow<ConversationSource>(source),
                distributionChannel = "store",
                webStorageCleanup = cleanup,
            )
            runCurrent()
            source.emit(ClientEvent.AppsChanged(listOf(appRecord())))
            runCurrent()
            source.commandFailure = IllegalStateException("delete rejected")

            viewModel.onAction(LocalAppsAction.DeleteApp(APP_ID))
            runCurrent()

            assertEquals(listOf(APP_ID), cleanup.cancelled)
            assertEquals("delete rejected", viewModel.uiState.value.error)
        } finally {
            Dispatchers.resetMain()
        }
    }

    @Test
    fun `delete is not submitted when browser cleanup cannot be journaled`() = runTest {
        Dispatchers.setMain(StandardTestDispatcher(testScheduler))
        try {
            val source = RecordingSource()
            val cleanup = RecordingWebStorageCleanup().apply { prepareSucceeds = false }
            val viewModel = LocalAppsViewModel(
                sourceFlow = MutableStateFlow<ConversationSource>(source),
                distributionChannel = "store",
                webStorageCleanup = cleanup,
            )
            runCurrent()
            source.emit(ClientEvent.AppsChanged(listOf(appRecord())))
            runCurrent()

            viewModel.onAction(LocalAppsAction.DeleteApp(APP_ID))
            runCurrent()

            assertTrue(source.commands.none { it is ClientCommand.DeleteApp })
            assertTrue(viewModel.uiState.value.error?.isNotBlank() == true)
        } finally {
            Dispatchers.resetMain()
        }
    }

    /**
     * A user standing on a deleted app's Details screen (now the Sessions tab)
     * has nothing left to render — the prune moves them back to the Library
     * and drops the dead app's selection, details, and cached sessions.
     */
    @Test
    fun `a deleted app cannot leave the user on its details screen`() = runTest {
        Dispatchers.setMain(StandardTestDispatcher(testScheduler))
        try {
            val source = RecordingSource()
            val viewModel = LocalAppsViewModel(
                sourceFlow = MutableStateFlow<ConversationSource>(source),
                distributionChannel = "store",
            )
            runCurrent()
            source.emit(
                ClientEvent.AppsChanged(
                    listOf(
                        appRecord(workflow = AppWorkflowStateDto.READY),
                        appRecord(OTHER_APP_ID, "订单", AppWorkflowStateDto.READY),
                    ),
                ),
            )
            runCurrent()

            viewModel.onAction(LocalAppsAction.OpenApp(APP_ID))
            runCurrent()
            source.emit(
                ClientEvent.AppSessionsChanged(
                    appId = APP_ID,
                    sessions = listOf(sessionRow("s-init", "初始化会话", kind = AppSessionKindDto.INIT)),
                    nextOffset = null,
                ),
            )
            runCurrent()
            assertEquals(
                LocalAppsDestination.Details(APP_ID, LocalAppDetailsTab.Sessions),
                viewModel.uiState.value.destination,
            )

            source.emit(ClientEvent.AppsChanged(listOf(appRecord(OTHER_APP_ID, "订单", AppWorkflowStateDto.READY))))
            runCurrent()

            assertEquals(LocalAppsDestination.Library, viewModel.uiState.value.destination)
            assertNull(viewModel.uiState.value.selectedAppId)
            assertNull(
                "a deleted app's cached session catalog must not survive it",
                viewModel.uiState.value.appSessions[APP_ID],
            )
        } finally {
            Dispatchers.resetMain()
        }
    }

    @Test
    fun `concurrent capability requests queue instead of clobbering`() = runTest {
        Dispatchers.setMain(StandardTestDispatcher(testScheduler))
        try {
            val source = RecordingSource()
            val viewModel = LocalAppsViewModel(
                sourceFlow = MutableStateFlow<ConversationSource>(source),
                distributionChannel = "full",
            )
            runCurrent()

            // One page, two fetch() calls to two unauthorized declared domains
            // in the same tick.
            source.emit(ClientEvent.AppEvent(AppEventDto.AppCapabilityRequested(domainRequest("cap-a", "a.example.com"))))
            source.emit(ClientEvent.AppEvent(AppEventDto.AppCapabilityRequested(domainRequest("cap-b", "b.example.com"))))
            runCurrent()

            assertEquals(
                "the first prompt must survive the second request",
                "cap-a",
                viewModel.uiState.value.pendingAuthorization?.requestId,
            )

            viewModel.onAction(LocalAppsAction.ResolveAuthorization(LocalAppAuthorizationDecision.AllowOnce))
            runCurrent()
            assertEquals("cap-b", viewModel.uiState.value.pendingAuthorization?.requestId)

            viewModel.onAction(LocalAppsAction.ResolveAuthorization(LocalAppAuthorizationDecision.AllowOnce))
            runCurrent()
            assertNull(viewModel.uiState.value.pendingAuthorization)

            assertEquals(
                listOf("cap-a", "cap-b"),
                source.commands.filterIsInstance<ClientCommand.ResolveAppCapabilityRequest>().map { it.requestId },
            )
        } finally {
            Dispatchers.resetMain()
        }
    }

    @Test
    fun `bridge errors preserve machine code and unsupported calls resolve locally`() = runTest {
        Dispatchers.setMain(StandardTestDispatcher(testScheduler))
        try {
            val source = RecordingSource()
            val viewModel = LocalAppsViewModel(
                sourceFlow = MutableStateFlow<ConversationSource>(source),
                distributionChannel = "full",
            )
            runCurrent()

            source.emit(
                ClientEvent.AppEvent(
                    AppEventDto.AppBridgeResponse(
                        AppBridgeResponseDto(
                            requestId = "engine-error",
                            appId = APP_ID,
                            ok = false,
                            resultJson = null,
                            error = "Location unavailable",
                            errorCode = "location_unavailable",
                        ),
                    ),
                ),
            )
            runCurrent()
            assertEquals(
                "location_unavailable",
                viewModel.uiState.value.bridgeResults[LocalAppBridgeRequestKey(APP_ID, "engine-error")]?.errorCode,
            )

            viewModel.onAction(
                LocalAppsAction.BridgeRequest(
                    LocalAppBridgeMessage(APP_ID, "bad-op", "not_supported", "{}"),
                ),
            )
            runCurrent()
            val localFailure = viewModel.uiState.value.bridgeResults[LocalAppBridgeRequestKey(APP_ID, "bad-op")]
            assertEquals(false, localFailure?.ok)
            assertEquals("operation_unsupported", localFailure?.errorCode)
            assertTrue(source.commands.none { it is ClientCommand.ExecuteAppBridgeRequest })
        } finally {
            Dispatchers.resetMain()
        }
    }

    @Test
    fun `same request id from two apps remains isolated through acknowledgement`() = runTest {
        Dispatchers.setMain(StandardTestDispatcher(testScheduler))
        try {
            val source = RecordingSource()
            val viewModel = LocalAppsViewModel(
                sourceFlow = MutableStateFlow<ConversationSource>(source),
                distributionChannel = "full",
            )
            runCurrent()

            listOf(APP_ID, OTHER_APP_ID).forEach { appId ->
                source.emit(
                    ClientEvent.AppEvent(
                        AppEventDto.AppBridgeResponse(
                            AppBridgeResponseDto(
                                requestId = "shared-id",
                                appId = appId,
                                ok = true,
                                resultJson = "{}",
                                error = null,
                                errorCode = null,
                            ),
                        ),
                    ),
                )
            }
            runCurrent()

            assertEquals(2, viewModel.uiState.value.bridgeResults.size)
            viewModel.onAction(LocalAppsAction.AcknowledgeBridgeResult(APP_ID, "shared-id"))
            assertFalse(
                viewModel.uiState.value.bridgeResults.containsKey(
                    LocalAppBridgeRequestKey(APP_ID, "shared-id"),
                ),
            )
            assertTrue(
                viewModel.uiState.value.bridgeResults.containsKey(
                    LocalAppBridgeRequestKey(OTHER_APP_ID, "shared-id"),
                ),
            )
        } finally {
            Dispatchers.resetMain()
        }
    }

    private fun domainRequest(requestId: String, domain: String) = AppCapabilityRequestDto(
        requestId = requestId,
        appId = APP_ID,
        capability = AppCapabilityKindDto.NETWORK_DOMAIN,
        domain = domain,
        reason = "页面请求访问 $domain",
    )

    @Test
    fun `ui capability approval executes following structured request and returns result`() = runTest {
        Dispatchers.setMain(StandardTestDispatcher(testScheduler))
        try {
            val source = RecordingSource()
            val viewModel = LocalAppsViewModel(
                sourceFlow = MutableStateFlow<ConversationSource>(source),
                distributionChannel = "full",
            )
            runCurrent()

            source.emit(
                ClientEvent.AppEvent(
                    AppEventDto.AppCapabilityRequested(
                        AppCapabilityRequestDto(
                            requestId = "cap-1",
                            appId = APP_ID,
                            capability = AppCapabilityKindDto.UI_CONTROL,
                            domain = null,
                            reason = "Agent needs to click Save",
                        ),
                    ),
                ),
            )
            runCurrent()
            assertEquals("cap-1", viewModel.uiState.value.pendingAuthorization?.requestId)

            viewModel.onAction(LocalAppsAction.ResolveAuthorization(LocalAppAuthorizationDecision.AllowSession))
            runCurrent()
            val capabilityResolution = source.commands
                .filterIsInstance<ClientCommand.ResolveAppCapabilityRequest>()
                .single()
            assertEquals("cap-1", capabilityResolution.requestId)
            assertEquals(AppAuthorizationDecisionDto.ALLOW_SESSION, capabilityResolution.decision)

            source.emit(
                ClientEvent.AppEvent(
                    AppEventDto.AppUiRequest(
                        AppUiRequestDto(
                            requestId = "ui-1",
                            appId = APP_ID,
                            action = AppUiActionKindDto.CLICK,
                            target = AppUiTargetDto(elementId = null, role = "button", name = "Save"),
                            value = null,
                        ),
                    ),
                ),
            )
            runCurrent()

            assertNull("capability approval must not prompt a second time", viewModel.uiState.value.pendingAuthorization)
            assertEquals(
                LocalAppUiAutomationAction.Click(
                    LocalAppUiTarget(elementId = null, role = "button", name = "Save"),
                ),
                viewModel.uiState.value.pendingUiAction?.action,
            )
            viewModel.onAction(LocalAppsAction.UiActionHandled("ui-1", resultJson = "{\"clicked\":true}", error = null))
            runCurrent()

            val uiResolution = source.commands.filterIsInstance<ClientCommand.ResolveAppUiRequest>().single()
            assertEquals("ui-1", uiResolution.requestId)
            assertEquals(AppAuthorizationDecisionDto.ALLOW_SESSION, uiResolution.decision)
            assertEquals("{\"clicked\":true}", uiResolution.resultJson)
            assertNull(uiResolution.error)

            source.emit(
                ClientEvent.AppEvent(
                    AppEventDto.AppUiRequest(
                        AppUiRequestDto(
                            requestId = "ui-2",
                            appId = APP_ID,
                            action = AppUiActionKindDto.RELOAD,
                            target = null,
                            value = null,
                        ),
                    ),
                ),
            )
            runCurrent()
            assertEquals("ui-2", viewModel.uiState.value.pendingUiAction?.requestId)
            assertNull(viewModel.uiState.value.pendingAuthorization)
        } finally {
            Dispatchers.resetMain()
        }
    }

    @Test
    fun `missing target and explicit ui execution errors resolve app ui requests as failures`() = runTest {
        Dispatchers.setMain(StandardTestDispatcher(testScheduler))
        try {
            val source = RecordingSource()
            val viewModel = LocalAppsViewModel(
                sourceFlow = MutableStateFlow<ConversationSource>(source),
                distributionChannel = "full",
            )
            runCurrent()

            source.emit(
                ClientEvent.AppEvent(
                    AppEventDto.AppCapabilityRequested(
                        AppCapabilityRequestDto(
                            requestId = "cap-3",
                            appId = APP_ID,
                            capability = AppCapabilityKindDto.UI_CONTROL,
                            domain = null,
                            reason = "Agent needs semantic controls",
                        ),
                    ),
                ),
            )
            runCurrent()
            viewModel.onAction(LocalAppsAction.ResolveAuthorization(LocalAppAuthorizationDecision.AllowSession))
            runCurrent()

            source.emit(
                ClientEvent.AppEvent(
                    AppEventDto.AppUiRequest(
                        AppUiRequestDto(
                            requestId = "ui-missing-target",
                            appId = APP_ID,
                            action = AppUiActionKindDto.CLICK,
                            target = null,
                            value = null,
                        ),
                    ),
                ),
            )
            runCurrent()

            val missingTargetResolution = source.commands
                .filterIsInstance<ClientCommand.ResolveAppUiRequest>()
                .last()
            assertEquals("ui-missing-target", missingTargetResolution.requestId)
            assertNull(missingTargetResolution.resultJson)
            assertEquals("UI 请求缺少有效目标或参数", missingTargetResolution.error)

            source.emit(
                ClientEvent.AppEvent(
                    AppEventDto.AppUiRequest(
                        AppUiRequestDto(
                            requestId = "ui-noop",
                            appId = APP_ID,
                            action = AppUiActionKindDto.CLICK,
                            target = AppUiTargetDto(elementId = "save", role = null, name = null),
                            value = null,
                        ),
                    ),
                ),
            )
            runCurrent()
            viewModel.onAction(
                LocalAppsAction.UiActionHandled(
                    requestId = "ui-noop",
                    resultJson = null,
                    error = "WebView UI automation returned no result",
                ),
            )
            runCurrent()

            val noopResolution = source.commands
                .filterIsInstance<ClientCommand.ResolveAppUiRequest>()
                .last()
            assertEquals("ui-noop", noopResolution.requestId)
            assertNull(noopResolution.resultJson)
            assertEquals("WebView UI automation returned no result", noopResolution.error)
        } finally {
            Dispatchers.resetMain()
        }
    }

    /**
     * Concurrent creates are rejected (same as iOS). The first claim still
     * opens Details/Sessions; a later unrelated app must not steal landing.
     */
    /// The second create is REFUSED, not queued.
    ///
    /// This used to assert that two briefs could be in flight without clobbering
    /// each other's claim — a shape that only made sense while claims were keyed
    /// by brief string. There is one intake at a time now, and the engine names
    /// the record it creates, so there is nothing left to clobber.
    @Test
    fun `a second create in flight is refused and leaves the first intact`() = runTest {
        Dispatchers.setMain(StandardTestDispatcher(testScheduler))
        try {
            val source = RecordingSource()
            val viewModel = LocalAppsViewModel(
                sourceFlow = MutableStateFlow<ConversationSource>(source),
                distributionChannel = "store",
            )
            runCurrent()

            viewModel.onAction(LocalAppsAction.CreateFromBrief("第一个应用", addWidget = true))
            runCurrent()
            viewModel.onAction(LocalAppsAction.CreateFromBrief("第二个应用", addWidget = false))
            runCurrent()

            val creates = source.commands.filterIsInstance<ClientCommand.CreateApp>()
            assertEquals(1, creates.size)
            assertEquals("第一个应用", creates.single().brief)
            assertTrue(
                "the refusal has to be visible, not silent",
                viewModel.uiState.value.error?.isNotBlank() == true,
            )

            // The first claim still owns the widget it asked for.
            val created = appRecord(id = "first-app", brief = "第一个应用")
            source.emit(ClientEvent.AppsChanged(listOf(created)))
            source.emit(ClientEvent.AppEvent(AppEventDto.AppCreated(created)))
            runCurrent()
            assertEquals("first-app", viewModel.widgetPinRequests.first())
        } finally {
            Dispatchers.resetMain()
        }
    }

    private fun sessionRow(
        uuid: String,
        title: String,
        kind: AppSessionKindDto = AppSessionKindDto.CONVERSATION,
    ) = AppSessionRowDto(
        uuid = uuid,
        title = title,
        modifiedRfc3339 = "2026-08-09T12:00:00Z",
        messageCount = 3u,
        kind = kind,
    )

    private fun appRecord(
        id: String = APP_ID,
        name: String = "客户跟进",
        workflow: AppWorkflowStateDto = AppWorkflowStateDto.DRAFT,
        brief: String = "记录客户跟进情况",
    ) = AppRecordDto(
        id = id,
        name = name,
        brief = brief,
        gitEnabled = true,
        createdAtMs = 1u,
        updatedAtMs = 2u,
        workflowState = workflow,
        conversationId = null,
        initSessionId = "00000000-0000-4000-8000-00000000$id".take(36),
        workspaceRel = "apps/$id/workspace",
    )

    private companion object {
        const val APP_ID = "tracker"
        const val OTHER_APP_ID = "orders"

        /** 2026-08-09T13:00:00Z — one hour after every fixture row's mtime. */
        const val NOW_EPOCH_SECONDS = 1_786_453_200L
    }
}
