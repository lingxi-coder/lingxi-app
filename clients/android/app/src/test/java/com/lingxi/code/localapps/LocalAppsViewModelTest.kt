package com.lingxi.code.localapps

import com.lingxi.code.bindings.AppAuthorizationDecisionDto
import com.lingxi.code.bindings.AppBridgeResponseDto
import com.lingxi.code.bindings.AppCapabilityKindDto
import com.lingxi.code.bindings.AppCreateModeDto
import com.lingxi.code.bindings.AppCreateOriginDto
import com.lingxi.code.bindings.AppErrorCodeDto
import com.lingxi.code.bindings.AppCapabilityRequestDto
import com.lingxi.code.bindings.AppRecordDto
import com.lingxi.code.bindings.AppSessionKindDto
import com.lingxi.code.bindings.AppSessionRowDto
import com.lingxi.code.bindings.AppUiActionKindDto
import com.lingxi.code.bindings.AppUiRequestDto
import com.lingxi.code.bindings.AppUiTargetDto
import com.lingxi.code.bindings.AppWorkflowStateDto
import com.lingxi.code.bindings.ClientCommand
import com.lingxi.code.bindings.ClientEvent
import com.lingxi.code.bindings.AppEventDto
import com.lingxi.code.conversation.ConversationSource
import com.lingxi.code.localapps.widget.LocalAppWidgetSnapshotSync
import com.lingxi.code.conversation.ReplyEvent
import com.lingxi.code.model.EngineModelState
import kotlinx.coroutines.Dispatchers
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
import kotlinx.coroutines.test.TestScope
import kotlinx.coroutines.test.advanceUntilIdle
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

    /**
     * Unwind `Dispatchers.Main` at the end of a test — the DRAIN FIRST, then
     * the reset. Every `finally` in this file calls this and none of them
     * calls `Dispatchers.resetMain()` directly.
     *
     * ⚠️ The order is the whole point, and getting it wrong does not fail on
     * what the test asserts. `LocalAppsViewModel.createShellApp` arms a
     * stop-loss — `viewModelScope.launch { delay(CREATE_RESULT_TIMEOUT_MS) }`,
     * i.e. a task scheduled on `Dispatchers.Main` — and a test that leaves a
     * create unresolved (or resolves one and starts another) still has that
     * task in the scheduler when its body returns. `runTest` then drains the
     * scheduler as part of its OWN teardown, which runs AFTER this `finally`.
     * Dispatching that task onto a Main the `finally` has already unset throws
     *
     *     IllegalStateException: Dispatchers.Main was accessed when the
     *     platform dispatcher was absent and the test dispatcher was unset
     *
     * — a harness error attributed to the test, with no assertion involved.
     * Five tests here failed exactly that way, all of them create-flow tests,
     * and the production stop-loss they tripped over is deliberate: a sibling
     * test (`a landed create is not timed out afterwards`) exists to assert it.
     *
     * `advanceUntilIdle()` cannot weaken anything: it runs after the body's
     * assertions have already been evaluated. It only lets the armed timeout
     * complete while Main is still installed.
     */
    private fun TestScope.releaseMain() {
        advanceUntilIdle()
        Dispatchers.resetMain()
    }

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

    /** Captures every widget-snapshot publish so exclusions can be asserted. */
    private class RecordingWidgetSnapshotSync : LocalAppWidgetSnapshotSync {
        val published = mutableListOf<List<LocalAppItem>>()

        override fun publish(apps: List<LocalAppItem>) {
            published += apps
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

    /// The 「+」 button creates an empty SHELL and carries a correlation key.
    ///
    /// Every value here is load-bearing. `mode = SHELL` is the fork that decides
    /// `scaffolded = false`; `surface = null` is mandatory in that mode (the
    /// engine rejects a shell create that names one); an empty `name`/`brief`
    /// is what makes the record a blank the conversation fills in; and
    /// `requestId` is the only thing that will identify this create's outcome
    /// among the events of every other creation path.
    @Test
    fun `the plus button creates a shell carrying a request id`() = runTest {
        Dispatchers.setMain(StandardTestDispatcher(testScheduler))
        try {
            val source = RecordingSource()
            val viewModel = LocalAppsViewModel(
                sourceFlow = MutableStateFlow<ConversationSource>(source),
                distributionChannel = "store",
            )
            runCurrent()

            viewModel.onAction(LocalAppsAction.Create)
            runCurrent()

            val create = source.commands.filterIsInstance<ClientCommand.CreateApp>().single()
            assertEquals(AppCreateModeDto.SHELL, create.mode)
            assertNull("a shell create must not name a surface", create.surface)
            assertEquals("", create.name)
            assertEquals("", create.brief)
            assertEquals(AppCreateOriginDto.LIBRARY, create.origin)
            assertEquals(true, create.gitEnabled)
            assertNull(create.workflowModel)
            assertNull(
                "a library create binds no conversation — the engine discards one anyway",
                create.conversationId,
            )
            assertTrue(
                "the create must carry a correlation key, not an empty slot",
                create.requestId?.isNotBlank() == true,
            )
        } finally {
            releaseMain()
        }
    }

    /// The key must be FRESH per create, not a constant.
    ///
    /// A key computed once (or hard-coded) would still match its own event and
    /// pass every single-create test here, while making the second create in a
    /// session claimable by the first one's leftovers.
    @Test
    fun `each create generates a new request id`() = runTest {
        Dispatchers.setMain(StandardTestDispatcher(testScheduler))
        try {
            val source = RecordingSource()
            val viewModel = LocalAppsViewModel(
                sourceFlow = MutableStateFlow<ConversationSource>(source),
                distributionChannel = "store",
            )
            runCurrent()

            viewModel.onAction(LocalAppsAction.Create)
            runCurrent()
            val first = source.commands.filterIsInstance<ClientCommand.CreateApp>()
                .single().requestId
            // Resolve the first one so the second is not refused as in-flight.
            val created = appRecord(id = "first", scaffolded = false)
            source.emit(ClientEvent.AppsChanged(listOf(created)))
            source.emit(ClientEvent.AppEvent(AppEventDto.AppCreated(created, first)))
            runCurrent()

            viewModel.onAction(LocalAppsAction.Create)
            runCurrent()
            val second = source.commands.filterIsInstance<ClientCommand.CreateApp>()
                .last().requestId

            assertTrue(first?.isNotBlank() == true)
            assertTrue("the second create must not reuse the first key", first != second)
        } finally {
            releaseMain()
        }
    }

    /// The hand-off arms on a KEY-MATCHED `AppCreated` and completes on the pin.
    ///
    /// `AppCreated` is emitted inside the create transaction and the init
    /// session is minted AFTER it, so the record on the create event never
    /// carries one. Waiting for the pin on the create event alone made the
    /// hand-off unreachable — the agent stayed in a conversation rooted outside
    /// the app and every build failed on the workspace.
    @Test
    fun `a matching AppCreated arms the landing and the record update brings the pin`() = runTest {
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

            viewModel.onAction(LocalAppsAction.Create)
            runCurrent()
            val requestId = source.commands.filterIsInstance<ClientCommand.CreateApp>()
                .single().requestId
            // `initSessionId = null` is what the ENGINE actually sends here.
            val created = appRecord(id = "shell", scaffolded = false).copy(initSessionId = null)
            source.emit(ClientEvent.AppsChanged(listOf(created)))
            source.emit(ClientEvent.AppEvent(AppEventDto.AppCreated(created, requestId)))
            runCurrent()

            assertTrue("AppCreated alone must not land: the pin is not minted yet", landings.isEmpty())

            source.emit(
                ClientEvent.AppEvent(
                    AppEventDto.AppRecordChanged(created.copy(initSessionId = "session-9")),
                ),
            )
            runCurrent()

            val landing = landings.single()
            assertEquals("shell", landing.appId)
            assertEquals("session-9", landing.initSessionId)
            job.cancel()
        } finally {
            releaseMain()
        }
    }

    /// An `AppCreated` for a DIFFERENT request must be ignored outright.
    ///
    /// The engine emits this event for BOTH creation paths, so an agent
    /// committing its own `LocalAppCreate` in this window is routine. Claiming
    /// it would open someone else's app and leave this create with no landing —
    /// which is exactly why a one-shot boolean was rejected for this job.
    @Test
    fun `an AppCreated for another request is ignored`() = runTest {
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

            viewModel.onAction(LocalAppsAction.Create)
            runCurrent()
            val mine = source.commands.filterIsInstance<ClientCommand.CreateApp>()
                .single().requestId

            val theirs = appRecord(id = "theirs").copy(initSessionId = null)
            source.emit(ClientEvent.AppsChanged(listOf(theirs)))
            source.emit(
                ClientEvent.AppEvent(AppEventDto.AppCreated(theirs, "somebody-elses-request")),
            )
            source.emit(ClientEvent.AppEvent(AppEventDto.AppRecordChanged(theirs)))
            runCurrent()
            assertTrue("not this session's create", landings.isEmpty())

            // Mine arrives afterwards and is still claimable — the claim was
            // not consumed by the impostor.
            val ours = appRecord(id = "ours", scaffolded = false).copy(initSessionId = null)
            source.emit(ClientEvent.AppsChanged(listOf(theirs, ours)))
            source.emit(ClientEvent.AppEvent(AppEventDto.AppCreated(ours, mine)))
            source.emit(ClientEvent.AppEvent(AppEventDto.AppRecordChanged(ours)))
            runCurrent()

            assertEquals("ours", landings.single().appId)
            job.cancel()
        } finally {
            releaseMain()
        }
    }

    /// A create with NO key at all (an agent-tool create, a backfill) is not
    /// this session's, and a null key must not be read as a wildcard.
    @Test
    fun `an AppCreated with no request id is ignored`() = runTest {
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

            viewModel.onAction(LocalAppsAction.Create)
            runCurrent()
            val unkeyed = appRecord(id = "agent-made").copy(initSessionId = null)
            source.emit(ClientEvent.AppsChanged(listOf(unkeyed)))
            source.emit(ClientEvent.AppEvent(AppEventDto.AppCreated(unkeyed, null)))
            source.emit(ClientEvent.AppEvent(AppEventDto.AppRecordChanged(unkeyed)))
            runCurrent()

            assertTrue("a key-less create belongs to nobody here", landings.isEmpty())
            job.cancel()
        } finally {
            releaseMain()
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

            viewModel.onAction(LocalAppsAction.Create)
            runCurrent()
            val requestId = source.commands.filterIsInstance<ClientCommand.CreateApp>()
                .single().requestId
            val created = appRecord(id = "shell", scaffolded = false).copy(initSessionId = null)
            source.emit(ClientEvent.AppsChanged(listOf(created)))
            source.emit(ClientEvent.AppEvent(AppEventDto.AppCreated(created, requestId)))
            source.emit(ClientEvent.AppEvent(AppEventDto.AppRecordChanged(created)))
            runCurrent()

            val landing = landings.single()
            assertEquals("shell", landing.appId)
            assertNull("no pin was minted", landing.initSessionId)
            job.cancel()
        } finally {
            releaseMain()
        }
    }

    /// The stop-loss. After 30 seconds the claim is released and the user is
    /// told the outcome is unknown — never that the create failed, because the
    /// app has almost certainly been created.
    @Test
    fun `an unresolved create times out and reports the result as unknown`() = runTest {
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

            viewModel.onAction(LocalAppsAction.Create)
            runCurrent()
            val requestId = source.commands.filterIsInstance<ClientCommand.CreateApp>()
                .single().requestId

            advanceTimeBy(LocalAppsViewModel.CREATE_RESULT_TIMEOUT_MS - 1)
            runCurrent()
            assertNull("the claim must survive right up to the deadline", viewModel.uiState.value.error)

            advanceTimeBy(2)
            runCurrent()
            assertEquals(
                "创建结果未知，请在应用库确认。",
                viewModel.uiState.value.error,
            )

            // And the released claim must not be re-claimable by a late event.
            val late = appRecord(id = "late", scaffolded = false).copy(initSessionId = null)
            source.emit(ClientEvent.AppsChanged(listOf(late)))
            source.emit(ClientEvent.AppEvent(AppEventDto.AppCreated(late, requestId)))
            source.emit(ClientEvent.AppEvent(AppEventDto.AppRecordChanged(late)))
            runCurrent()
            assertTrue("a timed-out claim is gone, not merely quiet", landings.isEmpty())
            job.cancel()
        } finally {
            releaseMain()
        }
    }

    /// A create that LANDED must not be "timed out" afterwards.
    ///
    /// The timeout is a coroutine armed at submit time; if clearing the claim
    /// did not also cancel it, every successful create would raise a bogus
    /// "result unknown" banner thirty seconds later.
    @Test
    fun `a landed create is not timed out afterwards`() = runTest {
        Dispatchers.setMain(StandardTestDispatcher(testScheduler))
        try {
            val source = RecordingSource()
            val viewModel = LocalAppsViewModel(
                sourceFlow = MutableStateFlow<ConversationSource>(source),
                distributionChannel = "store",
            )
            runCurrent()

            viewModel.onAction(LocalAppsAction.Create)
            runCurrent()
            val requestId = source.commands.filterIsInstance<ClientCommand.CreateApp>()
                .single().requestId
            val created = appRecord(id = "shell", scaffolded = false).copy(initSessionId = null)
            source.emit(ClientEvent.AppsChanged(listOf(created)))
            source.emit(ClientEvent.AppEvent(AppEventDto.AppCreated(created, requestId)))
            runCurrent()

            advanceTimeBy(LocalAppsViewModel.CREATE_RESULT_TIMEOUT_MS * 2)
            runCurrent()

            assertNull("a landed create has nothing to time out", viewModel.uiState.value.error)
        } finally {
            releaseMain()
        }
    }

    /// A failure carrying OUR key releases the claim (and is shown).
    @Test
    fun `a failure with the matching request id releases the create`() = runTest {
        Dispatchers.setMain(StandardTestDispatcher(testScheduler))
        try {
            val source = RecordingSource()
            val viewModel = LocalAppsViewModel(
                sourceFlow = MutableStateFlow<ConversationSource>(source),
                distributionChannel = "store",
            )
            runCurrent()

            viewModel.onAction(LocalAppsAction.Create)
            runCurrent()
            val requestId = source.commands.filterIsInstance<ClientCommand.CreateApp>()
                .single().requestId
            source.emit(
                ClientEvent.AppOperationFailed(
                    appId = null,
                    code = AppErrorCodeDto.WORKFLOW_STATE_INVALID,
                    message = "创建失败",
                    requestId = requestId,
                ),
            )
            runCurrent()
            assertEquals("创建失败", viewModel.uiState.value.error)

            // Released, so the next create is accepted rather than refused as
            // "one already in flight".
            viewModel.onAction(LocalAppsAction.Create)
            runCurrent()
            assertEquals(
                2,
                source.commands.filterIsInstance<ClientCommand.CreateApp>().size,
            )
        } finally {
            releaseMain()
        }
    }

    /// A failure that is not ours must leave the claim alone.
    ///
    /// This replaces the old "any global failure disarms the create" rule,
    /// which was an unkeyed claim in the other direction: with two creation
    /// paths live, an unrelated failure would drop a still-valid claim and the
    /// user's own app would never open.
    @Test
    fun `a failure for another request leaves the create claimable`() = runTest {
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

            viewModel.onAction(LocalAppsAction.Create)
            runCurrent()
            val requestId = source.commands.filterIsInstance<ClientCommand.CreateApp>()
                .single().requestId
            source.emit(
                ClientEvent.AppOperationFailed(
                    appId = "another-app",
                    code = AppErrorCodeDto.WORKFLOW_STATE_INVALID,
                    message = "别的应用失败了",
                    requestId = "somebody-elses-request",
                ),
            )
            // A key-less failure the engine synthesized is likewise not ours.
            source.emit(
                ClientEvent.AppOperationFailed(
                    appId = null,
                    code = AppErrorCodeDto.WORKFLOW_STATE_INVALID,
                    message = "无主的失败",
                    requestId = null,
                ),
            )
            runCurrent()

            val created = appRecord(id = "ours", scaffolded = false).copy(initSessionId = null)
            source.emit(ClientEvent.AppsChanged(listOf(created)))
            source.emit(ClientEvent.AppEvent(AppEventDto.AppCreated(created, requestId)))
            source.emit(ClientEvent.AppEvent(AppEventDto.AppRecordChanged(created)))
            runCurrent()

            assertEquals("ours", landings.single().appId)
            job.cancel()
        } finally {
            releaseMain()
        }
    }

    /// A reconnect (or a scope switch) clears the claim and says so.
    ///
    /// The create event is one-shot on the source that was just cancelled, so
    /// the claim can never resolve. Nothing may be claimed after a reconnect —
    /// not even an event carrying the old key, which is what this asserts.
    @Test
    fun `a reconnect clears the pending create and never claims afterwards`() = runTest {
        Dispatchers.setMain(StandardTestDispatcher(testScheduler))
        try {
            val first = RecordingSource()
            val sources = MutableStateFlow<ConversationSource>(first)
            val viewModel = LocalAppsViewModel(
                sourceFlow = sources,
                distributionChannel = "store",
            )
            runCurrent()
            val landings = mutableListOf<LocalAppsViewModel.CreatedAppLanding>()
            val job = launch { viewModel.createdAppLandings.collect { landings += it } }

            viewModel.onAction(LocalAppsAction.Create)
            runCurrent()
            val requestId = first.commands.filterIsInstance<ClientCommand.CreateApp>()
                .single().requestId

            val second = RecordingSource()
            sources.value = second
            runCurrent()

            assertEquals(
                "创建结果未知，请在应用库确认。",
                viewModel.uiState.value.error,
            )

            val created = appRecord(id = "shell", scaffolded = false).copy(initSessionId = null)
            second.emit(ClientEvent.AppsChanged(listOf(created)))
            second.emit(ClientEvent.AppEvent(AppEventDto.AppCreated(created, requestId)))
            second.emit(ClientEvent.AppEvent(AppEventDto.AppRecordChanged(created)))
            runCurrent()

            assertTrue("nothing may be claimed after a reconnect", landings.isEmpty())
            job.cancel()
        } finally {
            releaseMain()
        }
    }

    /// One create at a time; the refusal is visible, not silent.
    @Test
    fun `a second create while one is in flight is refused`() = runTest {
        Dispatchers.setMain(StandardTestDispatcher(testScheduler))
        try {
            val source = RecordingSource()
            val viewModel = LocalAppsViewModel(
                sourceFlow = MutableStateFlow<ConversationSource>(source),
                distributionChannel = "store",
            )
            runCurrent()

            viewModel.onAction(LocalAppsAction.Create)
            runCurrent()
            viewModel.onAction(LocalAppsAction.Create)
            runCurrent()

            assertEquals(
                "only one create may be in flight at a time",
                1,
                source.commands.filterIsInstance<ClientCommand.CreateApp>().size,
            )
            assertTrue(
                "the refusal has to be visible, not silent",
                viewModel.uiState.value.error?.isNotBlank() == true,
            )
        } finally {
            releaseMain()
        }
    }

    /// A shell must never reach the home screen.
    ///
    /// The snapshot is what the widget renders from, so a shell in it becomes a
    /// tile captioned with the engine's `"untitled"` placeholder that opens
    /// nothing. This asserts the exclusion at the ViewModel's ONE publish point,
    /// which every catalog mutation funnels through.
    @Test
    fun `the widget snapshot excludes shells`() = runTest {
        Dispatchers.setMain(StandardTestDispatcher(testScheduler))
        try {
            val source = RecordingSource()
            val snapshots = RecordingWidgetSnapshotSync()
            val viewModel = LocalAppsViewModel(
                sourceFlow = MutableStateFlow<ConversationSource>(source),
                distributionChannel = "store",
                widgetSnapshotSync = snapshots,
            )
            runCurrent()

            source.emit(
                ClientEvent.AppsChanged(
                    listOf(
                        appRecord(id = "formed", scaffolded = true),
                        appRecord(id = "shell", name = "untitled", brief = "", scaffolded = false),
                    ),
                ),
            )
            runCurrent()

            assertEquals(
                listOf("formed"),
                snapshots.published.last().map { it.id },
            )
        } finally {
            releaseMain()
        }
    }

    /// The widget request's permanent home: an action on an existing app.
    ///
    /// It used to exist ONLY as a checkbox inside the create dialog, so without
    /// this the feature would have been retired along with the form. A shell is
    /// refused for the same reason it is left out of the snapshot.
    @Test
    fun `RequestWidget pins a formed app and refuses a shell`() = runTest {
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

            source.emit(
                ClientEvent.AppsChanged(
                    listOf(
                        appRecord(id = "formed", scaffolded = true),
                        appRecord(id = "shell", scaffolded = false),
                    ),
                ),
            )
            runCurrent()

            viewModel.onAction(LocalAppsAction.RequestWidget("shell"))
            viewModel.onAction(LocalAppsAction.RequestWidget("does-not-exist"))
            runCurrent()
            assertTrue("neither a shell nor a stranger may be pinned", pins.isEmpty())

            viewModel.onAction(LocalAppsAction.RequestWidget("formed"))
            runCurrent()
            assertEquals(listOf("formed"), pins)
            pinJob.cancel()
        } finally {
            releaseMain()
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
            releaseMain()
        }
    }

    /// §D.3: tapping a DRAFT card resumes its pinned init session, not Details.
    ///
    /// One assertion alone ("draftSessionLandings fired") cannot tell "the
    /// `scaffolded` fork was read" from "it always takes this branch" — the
    /// sibling test below on a FORMED record is the other half that does.
    @Test
    fun `opening a draft app resumes its pinned init session instead of details`() = runTest {
        Dispatchers.setMain(StandardTestDispatcher(testScheduler))
        try {
            val source = RecordingSource()
            val viewModel = LocalAppsViewModel(
                sourceFlow = MutableStateFlow<ConversationSource>(source),
                distributionChannel = "store",
            )
            runCurrent()
            val landings = mutableListOf<LocalAppsViewModel.DraftSessionLanding>()
            val job = launch { viewModel.draftSessionLandings.collect { landings += it } }

            val draft = appRecord(scaffolded = false)
            source.emit(ClientEvent.AppsChanged(listOf(draft)))
            runCurrent()

            viewModel.onAction(LocalAppsAction.OpenApp(APP_ID))
            runCurrent()

            val landing = landings.single()
            assertEquals(APP_ID, landing.appId)
            assertEquals(
                "the shell's own pinned init session id must ride along",
                draft.initSessionId,
                landing.sessionId,
            )
            assertEquals(
                "a draft card must never open the details screen",
                LocalAppsDestination.Library,
                viewModel.uiState.value.destination,
            )
            assertTrue(
                "a draft has no details worth fetching",
                source.commands.none { it is ClientCommand.GetAppDetails },
            )
            job.cancel()
        } finally {
            releaseMain()
        }
    }

    /// The other half of the §D.3 fork: a FORMED app still opens Details, and
    /// tapping it raises nothing on the draft-landing channel.
    @Test
    fun `opening a formed app still lands on details and emits no draft landing`() = runTest {
        Dispatchers.setMain(StandardTestDispatcher(testScheduler))
        try {
            val source = RecordingSource()
            val viewModel = LocalAppsViewModel(
                sourceFlow = MutableStateFlow<ConversationSource>(source),
                distributionChannel = "store",
            )
            runCurrent()
            val landings = mutableListOf<LocalAppsViewModel.DraftSessionLanding>()
            val job = launch { viewModel.draftSessionLandings.collect { landings += it } }

            source.emit(ClientEvent.AppsChanged(listOf(appRecord(scaffolded = true))))
            runCurrent()

            viewModel.onAction(LocalAppsAction.OpenApp(APP_ID))
            runCurrent()

            assertEquals(
                LocalAppsDestination.Details(APP_ID, LocalAppDetailsTab.Sessions),
                viewModel.uiState.value.destination,
            )
            assertTrue("a formed app must not resume any draft session", landings.isEmpty())
            job.cancel()
        } finally {
            releaseMain()
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
            releaseMain()
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
            releaseMain()
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
            releaseMain()
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
            releaseMain()
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
            releaseMain()
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
            releaseMain()
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
            releaseMain()
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
            releaseMain()
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
            releaseMain()
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
            releaseMain()
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
            releaseMain()
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
            releaseMain()
        }
    }

    /**
     * Task 9: `CaptureView` moved from a fieldless `data object` to a `data
     * class` carrying the opaque crop request, so `AppUiRequestDto.value`
     * (already wired for FILL/SELECT/etc.) must now also reach CAPTURE_VIEW.
     * `toUiAutomationAction()` is `private` to `LocalAppsViewModel.kt` (a
     * file-scoped, not just package-scoped, Kotlin visibility), so this
     * drives it the same way the CLICK/RELOAD test above does: through the
     * public event pipeline down to `pendingUiAction?.action`.
     */
    @Test
    fun `a capture_view ui request threads the opaque rect value into the automation action`() = runTest {
        Dispatchers.setMain(StandardTestDispatcher(testScheduler))
        try {
            val source = RecordingSource()
            val viewModel = LocalAppsViewModel(
                sourceFlow = MutableStateFlow<ConversationSource>(source),
                distributionChannel = "full",
            )
            runCurrent()

            val rectValue = """{"rect":{"x":10,"y":20,"width":120,"height":80}}"""
            source.emit(
                ClientEvent.AppEvent(
                    AppEventDto.AppUiRequest(
                        AppUiRequestDto(
                            requestId = "ui-capture-rect",
                            appId = APP_ID,
                            action = AppUiActionKindDto.CAPTURE_VIEW,
                            target = null,
                            value = rectValue,
                        ),
                    ),
                ),
            )
            runCurrent()

            // CAPTURE_VIEW is read-only (rides with INSPECT — see the comment
            // in the reducer), so it needs no prior `ui_control` grant.
            assertNull(viewModel.uiState.value.pendingAuthorization)
            assertEquals(
                LocalAppUiAutomationAction.CaptureView(rectValue),
                viewModel.uiState.value.pendingUiAction?.action,
            )

            // A whole-view capture (no crop) must thread a null value, not an
            // empty string — `parseRequestedCaptureRect(null)` and
            // `parseRequestedCaptureRect("")` are NOT the same thing.
            source.emit(
                ClientEvent.AppEvent(
                    AppEventDto.AppUiRequest(
                        AppUiRequestDto(
                            requestId = "ui-capture-whole",
                            appId = APP_ID,
                            action = AppUiActionKindDto.CAPTURE_VIEW,
                            target = null,
                            value = null,
                        ),
                    ),
                ),
            )
            runCurrent()
            assertEquals(
                LocalAppUiAutomationAction.CaptureView(null),
                viewModel.uiState.value.pendingUiAction?.action,
            )
        } finally {
            releaseMain()
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
            releaseMain()
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
        /**
         * Defaults to a FORMED app, matching the fixture's populated name and
         * brief. A shell fixture has to say so — and should also blank the two
         * fields, the way the engine's shell record actually looks.
         */
        scaffolded: Boolean = true,
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
        scaffolded = scaffolded,
    )

    private companion object {
        const val APP_ID = "tracker"
        const val OTHER_APP_ID = "orders"

        /** 2026-08-09T13:00:00Z — one hour after every fixture row's mtime. */
        const val NOW_EPOCH_SECONDS = 1_786_453_200L
    }
}
