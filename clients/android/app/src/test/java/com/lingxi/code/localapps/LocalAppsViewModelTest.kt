package com.lingxi.code.localapps

import com.lingxi.code.bindings.AppDesignFieldDto
import com.lingxi.code.bindings.AppDesignFieldTypeDto
import com.lingxi.code.bindings.AppDesignPatchOpDto
import com.lingxi.code.bindings.AppDesignStepDto
import com.lingxi.code.bindings.AppAuthorizationDecisionDto
import com.lingxi.code.bindings.AppCapabilityKindDto
import com.lingxi.code.bindings.AppCapabilityRequestDto
import com.lingxi.code.bindings.AppDetailsDto
import com.lingxi.code.bindings.AppDesignFieldValueDto
import com.lingxi.code.bindings.AppErrorCodeDto
import com.lingxi.code.bindings.AppEventDto
import com.lingxi.code.bindings.AppPlanDto
import com.lingxi.code.bindings.AppRecordDto
import com.lingxi.code.bindings.AppRuntimeDetailsDto
import com.lingxi.code.bindings.AppRuntimeStateDto
import com.lingxi.code.bindings.AppUiActionKindDto
import com.lingxi.code.bindings.AppUiRequestDto
import com.lingxi.code.bindings.AppUiTargetDto
import com.lingxi.code.bindings.AppWorkflowStateDto
import com.lingxi.code.bindings.ClientCommand
import com.lingxi.code.bindings.ClientEvent
import com.lingxi.code.bindings.DesignValueDto
import com.lingxi.code.conversation.ConversationSource
import com.lingxi.code.conversation.ReplyEvent
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.asSharedFlow
import kotlinx.coroutines.flow.emptyFlow
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.test.StandardTestDispatcher
import kotlinx.coroutines.test.advanceTimeBy
import kotlinx.coroutines.test.advanceUntilIdle
import kotlinx.coroutines.test.resetMain
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest
import kotlinx.coroutines.test.setMain
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

@OptIn(ExperimentalCoroutinesApi::class)
class LocalAppsViewModelTest {

    private class RecordingSource : ConversationSource {
        private val events = MutableSharedFlow<ClientEvent>(extraBufferCapacity = 32)
        val commands = mutableListOf<ClientCommand>()

        override val clientEvents: Flow<ClientEvent> = events.asSharedFlow()

        override suspend fun submitClientCommand(command: ClientCommand) {
            commands += command
        }

        override fun submit(text: String): Flow<ReplyEvent> = emptyFlow()

        fun emit(event: ClientEvent) {
            assertTrue("LocalAppsViewModel must subscribe before test events", events.tryEmit(event))
        }
    }

    /// local-apps#questionnaire Task 11 review, Fix 2. The Rust side pinned
    /// this exact fabrication with a NAMED, red-until-fixed test
    /// (`create_app_persists_name_as_brief_until_task_11_adds_a_real_one`,
    /// since renamed once Task 11 landed the real brief). Android's
    /// `createSelectedTemplate()` was given the SAME stopgap
    /// (`brief = name`) with only a `NOTE`, no mechanism forcing anyone to
    /// notice when it should stop being true — this test is that mechanism.
    /// It MUST FAIL until Task 20 replaces the template-picker create flow
    /// with one that collects a real brief from the user; do not "fix" it
    /// by relaxing the assertion — fix it by making Android send a REAL
    /// brief, then update this test the way the Rust tripwire was updated.
    ///
    /// MOVED (local-apps#questionnaire, Task 18): `createSelectedTemplate()`
    /// and `state.templates`/`selectedTemplateKind` are deleted —
    /// `LocalAppsAction.CreateFromBrief(brief)` takes the brief as an
    /// explicit parameter, so the ViewModel itself no longer fabricates
    /// anything. The fabrication moved to the CALLER: `LocalAppsScreen.kt`'s
    /// `CreateAppDialog` (the template picker's minimal replacement) still
    /// collects only a display name and passes that name as the brief too
    /// (see its `onCreate` NOTE). This test now dispatches the exact same
    /// action sequence that dialog's confirm button does — `ChangeCreateName`
    /// then `CreateFromBrief` with the SAME string — so it still fails for
    /// the same reason, just no longer needs the `internal _uiState` seam to
    /// get there.
    @Test
    fun `create app fabricates the brief from the display name until Task 20 fixes it`() = runTest {
        Dispatchers.setMain(StandardTestDispatcher(testScheduler))
        try {
            val source = RecordingSource()
            val viewModel = LocalAppsViewModel(
                sourceFlow = MutableStateFlow<ConversationSource>(source),
                distributionChannel = "store",
            )
            runCurrent()

            viewModel.onAction(LocalAppsAction.ChangeCreateName("My Habit App"))
            // Mirrors `CreateAppDialog`'s confirm button in `LocalAppsScreen.kt`.
            viewModel.onAction(LocalAppsAction.CreateFromBrief("My Habit App"))
            runCurrent()

            val sent = source.commands.filterIsInstance<ClientCommand.CreateApp>().singleOrNull()
                ?: throw AssertionError("CreateFromBrief must submit ClientCommand.CreateApp")
            assertEquals("My Habit App", sent.name)
            assertTrue(
                "the brief must be a real spec, not the display name relabeled — " +
                    "this is the exact fabrication local-apps#questionnaire Tasks 10/11 " +
                    "spent two review rounds eliminating on the Rust side",
                sent.brief != sent.name,
            )
        } finally {
            Dispatchers.resetMain()
        }
    }

    @Test
    fun `a questionnaire event replaces the stored steps`() = runTest {
        Dispatchers.setMain(StandardTestDispatcher(testScheduler))
        try {
            val source = RecordingSource()
            val viewModel = LocalAppsViewModel(
                sourceFlow = MutableStateFlow<ConversationSource>(source),
                distributionChannel = "store",
            )
            runCurrent()

            source.emit(
                ClientEvent.AppEvent(
                    AppEventDto.AppQuestionnaireChanged(
                        appId = APP_ID,
                        revision = 1u,
                        steps = listOf(basicsStepDto(allowsDefer = true)),
                    ),
                ),
            )
            runCurrent()

            assertEquals(1, viewModel.uiState.value.questionnaires[APP_ID]?.size)
            assertTrue(viewModel.uiState.value.questionnaires[APP_ID]!!.first().fields.first().allowsDefer)
        } finally {
            Dispatchers.resetMain()
        }
    }

    @Test
    fun `a null plan event clears the stored plan`() = runTest {
        Dispatchers.setMain(StandardTestDispatcher(testScheduler))
        try {
            val source = RecordingSource()
            val viewModel = LocalAppsViewModel(
                sourceFlow = MutableStateFlow<ConversationSource>(source),
                distributionChannel = "store",
            )
            runCurrent()

            source.emit(ClientEvent.AppEvent(AppEventDto.AppPlanChanged(appId = APP_ID, revision = 2u, plan = onePlanDto())))
            runCurrent()
            assertNotNull(viewModel.uiState.value.plans[APP_ID])

            source.emit(ClientEvent.AppEvent(AppEventDto.AppPlanChanged(appId = APP_ID, revision = 3u, plan = null)))
            runCurrent()
            assertNull("an answer edit voids the plan on the client too", viewModel.uiState.value.plans[APP_ID])
        } finally {
            Dispatchers.resetMain()
        }
    }

    @Test
    fun `creating an app sends only the brief`() = runTest {
        Dispatchers.setMain(StandardTestDispatcher(testScheduler))
        try {
            val source = RecordingSource()
            val viewModel = LocalAppsViewModel(
                sourceFlow = MutableStateFlow<ConversationSource>(source),
                distributionChannel = "store",
            )
            runCurrent()

            viewModel.onAction(LocalAppsAction.ChangeCreateName("记事本"))
            viewModel.onAction(LocalAppsAction.CreateFromBrief("一个记事本 app"))
            runCurrent()

            val sent = source.commands.filterIsInstance<ClientCommand.CreateApp>().single()
            assertEquals("一个记事本 app", sent.brief)
            // `ClientCommand.CreateApp` has no `template` argument at all any
            // more (it was deleted in Task 5 with `AppTemplateKindDto`) — a
            // compile-time guarantee stronger than a runtime `sent.has(...)`
            // check could give.
        } finally {
            Dispatchers.resetMain()
        }
    }

    @Test
    fun `draft edits debounce serialize and recover revision conflicts`() = runTest {
        Dispatchers.setMain(StandardTestDispatcher(testScheduler))
        try {
            val source = RecordingSource()
            val viewModel = LocalAppsViewModel(
                sourceFlow = MutableStateFlow<ConversationSource>(source),
                distributionChannel = "store",
            )
            runCurrent()

            assertTrue(source.commands.contains(ClientCommand.ListApps))
            // NOTE (local-apps#questionnaire, Task 5): `ClientCommand.ListAppTemplates`
            // was deleted with the static template catalog — startup requests
            // only `ListApps` now; the questionnaire arrives per-app, later,
            // via `AppEventDto.AppQuestionnaireChanged`.
            seedDesigner(source)
            runCurrent()

            viewModel.onAction(
                LocalAppsAction.EditField("purpose", LocalAppDesignValue.Text("first"), debounce = true),
            )
            viewModel.onAction(
                LocalAppsAction.EditField("purpose", LocalAppDesignValue.Text("coalesced"), debounce = true),
            )
            advanceTimeBy(399)
            runCurrent()
            assertEquals(0, source.updateCommands().size)

            advanceTimeBy(1)
            runCurrent()
            assertUpdate(source.updateCommands().single(), expectedRevision = 0u, value = "coalesced")

            viewModel.onAction(
                LocalAppsAction.EditField("purpose", LocalAppDesignValue.Text("queued"), debounce = false),
            )
            runCurrent()
            assertEquals("one patch remains in flight", 1, source.updateCommands().size)

            source.emit(
                ClientEvent.AppDesignDraftChanged(
                    appId = APP_ID,
                    revision = 1u,
                    fields = mapOf("purpose" to DesignValueDto.ShortText("coalesced")),
                ),
            )
            runCurrent()
            assertUpdate(source.updateCommands().last(), expectedRevision = 1u, value = "queued")
            assertEquals(2, source.updateCommands().size)

            source.emit(ClientEvent.AppDesignConflict(APP_ID, expectedRevision = 1u, actualRevision = 2u))
            source.emit(ClientEvent.AppOperationFailed(APP_ID, AppErrorCodeDto.REVISION_CONFLICT, "stale revision"))
            runCurrent()
            assertTrue(source.commands.any { it == ClientCommand.GetAppDetails(APP_ID) })
            assertEquals("retry waits for the authoritative detail snapshot", 2, source.updateCommands().size)
            assertNull(viewModel.uiState.value.error)

            source.emit(
                ClientEvent.AppEvent(
                    AppEventDto.AppDetailsChanged(
                        AppDetailsDto(
                            app = appRecord(),
                            designRevision = 2u,
                            designFields = listOf(
                                AppDesignFieldValueDto("purpose", DesignValueDto.ShortText("external")),
                            ),
                            questionnaire = emptyList(),
                            plan = null,
                            manifest = null,
                            runtime = AppRuntimeDetailsDto(
                                state = AppRuntimeStateDto.STOPPED,
                                mode = null,
                                loopbackUrl = null,
                                suspensionReason = null,
                                recoveryState = null,
                                lastError = null,
                            ),
                            generationJob = null,
                            checkpoints = emptyList(),
                        ),
                    ),
                ),
            )
            runCurrent()

            assertEquals(3, source.updateCommands().size)
            assertUpdate(source.updateCommands().last(), expectedRevision = 2u, value = "queued")
            assertEquals(LocalAppDesignValue.Text("queued"), viewModel.uiState.value.designer?.values?.get("purpose"))
        } finally {
            Dispatchers.resetMain()
        }
    }

    @Test
    fun `confirm drains the debounced draft edit before sending the confirm`() = runTest {
        Dispatchers.setMain(StandardTestDispatcher(testScheduler))
        try {
            val source = RecordingSource()
            val viewModel = LocalAppsViewModel(
                sourceFlow = MutableStateFlow<ConversationSource>(source),
                distributionChannel = "store",
            )
            runCurrent()
            seedDesigner(source)
            runCurrent()

            viewModel.onAction(
                LocalAppsAction.EditField("purpose", LocalAppDesignValue.Text("final answer"), debounce = true),
            )
            // Confirm beats the 400 ms timer: the edit is still only debounced.
            advanceTimeBy(100)
            viewModel.onAction(LocalAppsAction.ConfirmDesign)
            runCurrent()

            assertUpdate(source.updateCommands().single(), expectedRevision = 0u, value = "final answer")
            assertTrue(
                "confirm must wait for the flushed patch",
                source.commands.none { it is ClientCommand.ConfirmAppDesign },
            )

            source.emit(
                ClientEvent.AppDesignDraftChanged(
                    appId = APP_ID,
                    revision = 1u,
                    fields = mapOf("purpose" to DesignValueDto.ShortText("final answer")),
                ),
            )
            runCurrent()
            advanceTimeBy(50)
            runCurrent()

            val confirm = source.commands.filterIsInstance<ClientCommand.ConfirmAppDesign>().single()
            assertEquals(1uL, confirm.revision)
            assertEquals("designer-1", confirm.interactionId)
            assertEquals(1, source.updateCommands().size)
        } finally {
            Dispatchers.resetMain()
        }
    }

    @Test
    fun `a rejected draft patch releases the queue for the next edit`() = runTest {
        Dispatchers.setMain(StandardTestDispatcher(testScheduler))
        try {
            val source = RecordingSource()
            val viewModel = LocalAppsViewModel(
                sourceFlow = MutableStateFlow<ConversationSource>(source),
                distributionChannel = "store",
            )
            runCurrent()
            seedDesigner(source)
            runCurrent()

            viewModel.onAction(
                LocalAppsAction.EditField("purpose", LocalAppDesignValue.Text("first"), debounce = false),
            )
            runCurrent()
            assertEquals(1, source.updateCommands().size)

            viewModel.onAction(
                LocalAppsAction.EditField("purpose", LocalAppDesignValue.Text("second"), debounce = false),
            )
            runCurrent()
            assertEquals("the second patch waits behind the in-flight one", 1, source.updateCommands().size)

            // The engine's ingest gate refuses the value outright: no
            // AppDesignDraftChanged ack and no AppDesignConflict will follow, so
            // this is the only event that can release the single-slot queue.
            source.emit(
                ClientEvent.AppOperationFailed(
                    APP_ID,
                    AppErrorCodeDto.INVALID_REQUEST,
                    "invalid HTTPS domain \"API.Example.com\"",
                ),
            )
            runCurrent()

            assertEquals("the queued edit must still reach the engine", 2, source.updateCommands().size)
            assertUpdate(source.updateCommands().last(), expectedRevision = 0u, value = "second")
            assertEquals("invalid HTTPS domain \"API.Example.com\"", viewModel.uiState.value.error)
        } finally {
            Dispatchers.resetMain()
        }
    }

    @Test
    fun `a runtime failure does not release a draft patch that is still in flight`() = runTest {
        Dispatchers.setMain(StandardTestDispatcher(testScheduler))
        try {
            val source = RecordingSource()
            val viewModel = LocalAppsViewModel(
                sourceFlow = MutableStateFlow<ConversationSource>(source),
                distributionChannel = "store",
            )
            runCurrent()
            seedDesigner(source)
            runCurrent()

            viewModel.onAction(
                LocalAppsAction.EditField("purpose", LocalAppDesignValue.Text("first"), debounce = false),
            )
            viewModel.onAction(
                LocalAppsAction.EditField("purpose", LocalAppDesignValue.Text("second"), debounce = false),
            )
            runCurrent()
            assertEquals(1, source.updateCommands().size)

            // A StartApp/RestoreAppCheckpoint/suggestion failure also carries
            // this app id, but says nothing about a patch still in flight —
            // releasing on it would double-send at one revision.
            source.emit(
                ClientEvent.AppOperationFailed(APP_ID, AppErrorCodeDto.IO, "local app runtime start failed"),
            )
            runCurrent()

            assertEquals("only a rejected patch may release the slot", 1, source.updateCommands().size)
            assertEquals("local app runtime start failed", viewModel.uiState.value.error)
        } finally {
            Dispatchers.resetMain()
        }
    }

    /**
     * `update_draft` runs inside `AppService::with_app`, so its own failures arrive
     * as Io / NotFound / StorageCorrupt too — the same codes an unrelated command
     * produces, because `AppOperationFailed` carries no correlation id. Neither arm
     * may release the slot for those, so without the staleness bound the designer
     * stays wedged for the rest of the process.
     */
    @Test
    fun `a draft patch stranded by an unattributable failure stops blocking later edits`() = runTest {
        Dispatchers.setMain(StandardTestDispatcher(testScheduler))
        try {
            val source = RecordingSource()
            val viewModel = LocalAppsViewModel(
                sourceFlow = MutableStateFlow<ConversationSource>(source),
                distributionChannel = "store",
            )
            viewModel.inFlightEditBudgetMs = 0
            runCurrent()
            seedDesigner(source)
            runCurrent()

            viewModel.onAction(
                LocalAppsAction.EditField("purpose", LocalAppDesignValue.Text("first"), debounce = false),
            )
            runCurrent()
            assertEquals(1, source.updateCommands().size)

            // The persist behind THIS patch failed. Indistinguishable on the wire
            // from the runtime failure above, so the slot is still held.
            source.emit(
                ClientEvent.AppOperationFailed(APP_ID, AppErrorCodeDto.IO, "persist draft: disk full"),
            )
            runCurrent()
            assertEquals("the failure itself must not release the slot", 1, source.updateCommands().size)

            // Anything that pumps the queue ages out the stale slot — and must put the
            // stranded edit BACK on it. Dropping it would lose the answer while
            // `designer.values` keeps the confirm gate satisfied, so a later confirm
            // would ship a spec the engine never received.
            viewModel.onAction(LocalAppsAction.ConfirmDesign)
            advanceUntilIdle()

            assertEquals(
                "a stranded patch must not wedge the queue forever",
                2,
                source.updateCommands().size,
            )
            assertUpdate(source.updateCommands().last(), expectedRevision = 0u, value = "first")
            assertTrue(
                "the confirm must not go out while the resent patch is unacked",
                source.commands.none { it is ClientCommand.ConfirmAppDesign },
            )
        } finally {
            Dispatchers.resetMain()
        }
    }

    /**
     * Both budget gates ask `now - sentAtMs >= budget`, and both stamps come from
     * the wall clock. An NTP or user step BACKWARD makes that difference
     * negative, and a negative elapsed satisfies no lower bound: with neither a
     * sign check nor a re-stamp the gate is held until the clock catches up.
     *
     * The answer to a negative elapsed is to RE-STAMP, not to release: only `now`
     * moved, so the patch is exactly as healthy as it was, and releasing it would
     * buy an avoidable resend on every backward tick — including the sub-second
     * NTP correction that is the common case. Both halves are asserted, because
     * either alone is also satisfied by a weaker gate: the step itself must not
     * resend (a release would), and one budget measured FROM THE RE-STAMP must
     * still release (merely ignoring the sign would not, since the original stamp
     * stays an hour in the future). Nothing here touches the budget — it stays at
     * its production 15 s — so the only thing under test is the clock.
     */
    @Test
    fun `a backward clock step re-stamps the in-flight draft slot instead of releasing it`() = runTest {
        Dispatchers.setMain(StandardTestDispatcher(testScheduler))
        try {
            val source = RecordingSource()
            val viewModel = LocalAppsViewModel(
                sourceFlow = MutableStateFlow<ConversationSource>(source),
                distributionChannel = "store",
            )
            runCurrent()
            seedDesigner(source)
            runCurrent()

            viewModel.onAction(
                LocalAppsAction.EditField("purpose", LocalAppDesignValue.Text("first"), debounce = false),
            )
            runCurrent()
            assertEquals(1, source.updateCommands().size)

            // Unattributable, so no arm may release the slot — the watchdog is
            // the only thing that can, exactly as in the test above.
            source.emit(
                ClientEvent.AppOperationFailed(APP_ID, AppErrorCodeDto.IO, "persist draft: disk full"),
            )
            runCurrent()
            assertEquals("the failure itself must not release the slot", 1, source.updateCommands().size)

            // The clock steps back an hour. `sentAtMs` was stamped moments ago and
            // therefore at or BEFORE the real `now` this is derived from, so the
            // elapsed the gate computes is at most a few milliseconds above
            // -3_600_000 — negative unless this test itself runs for an hour.
            val stepped = System.currentTimeMillis() - 3_600_000
            viewModel.currentTimeMs = { stepped }
            assertEquals(
                "the budget must stay at its production value: only the clock moved",
                15_000L,
                viewModel.inFlightEditBudgetMs,
            )

            viewModel.onAction(LocalAppsAction.ConfirmDesign)
            advanceUntilIdle()
            assertEquals(
                "a healthy patch must not be resent merely because the clock stepped back",
                1,
                source.updateCommands().size,
            )

            // One millisecond short of the budget, measured from the re-stamp the
            // pump above wrote: still held, and now for the ordinary reason.
            viewModel.currentTimeMs = { stepped + 14_999 }
            viewModel.onAction(LocalAppsAction.ConfirmDesign)
            advanceUntilIdle()
            assertEquals(
                "the budget restarted at the step; it has not run yet",
                1,
                source.updateCommands().size,
            )

            // At the budget it releases. Measured from the re-stamp: the original
            // stamp is still an hour ahead of this clock, so a gate that only
            // ignored the sign would hold here until the clock climbed back.
            viewModel.currentTimeMs = { stepped + 15_000 }
            viewModel.onAction(LocalAppsAction.ConfirmDesign)
            advanceUntilIdle()

            assertEquals(
                "a stranded patch must still age out one budget after the step",
                2,
                source.updateCommands().size,
            )
            // Released, never dropped: the answer the user typed goes back out.
            assertUpdate(source.updateCommands().last(), expectedRevision = 0u, value = "first")
            assertTrue(
                "the confirm must not go out while the resent patch is unacked",
                source.commands.none { it is ClientCommand.ConfirmAppDesign },
            )
        } finally {
            Dispatchers.resetMain()
        }
    }

    /**
     * The same backward step against the second gate, which matters more: it is
     * process-global rather than app-scoped, so wedging it stalls later edits for
     * EVERY app. It re-stamps for the same reason the slot above does — the step
     * says nothing about the refresh, which may still be answered — and one
     * budget later it does release, because by then the answer really is overdue.
     * Releasing loses nothing (`reduceDraftConflict` put the rejected patch back
     * on the queue before raising the gate) but it must also take the "已重新加载"
     * banner down, because it is released precisely when the authoritative
     * snapshot is not known to be coming.
     */
    @Test
    fun `a backward clock step re-stamps the conflict refresh gate instead of releasing it`() = runTest {
        Dispatchers.setMain(StandardTestDispatcher(testScheduler))
        try {
            val source = RecordingSource()
            val viewModel = LocalAppsViewModel(
                sourceFlow = MutableStateFlow<ConversationSource>(source),
                distributionChannel = "store",
            )
            runCurrent()
            seedDesigner(source)
            runCurrent()

            viewModel.onAction(
                LocalAppsAction.EditField("purpose", LocalAppDesignValue.Text("first"), debounce = false),
            )
            runCurrent()
            assertEquals(1, source.updateCommands().size)

            source.emit(ClientEvent.AppDesignConflict(APP_ID, expectedRevision = 0u, actualRevision = 2u))
            runCurrent()
            assertTrue(source.commands.any { it == ClientCommand.GetAppDetails(APP_ID) })
            assertEquals(2uL, viewModel.uiState.value.designer?.conflictRevision)

            val stepped = System.currentTimeMillis() - 3_600_000
            viewModel.currentTimeMs = { stepped }
            assertEquals(
                "the budget must stay at its production value: only the clock moved",
                15_000L,
                viewModel.inFlightEditBudgetMs,
            )

            viewModel.onAction(
                LocalAppsAction.EditField("purpose", LocalAppDesignValue.Text("second"), debounce = false),
            )
            runCurrent()

            assertEquals(
                "a backward step must not abandon a refresh that may still be answered",
                1,
                source.updateCommands().size,
            )
            assertEquals(
                "the banner may only come down with the gate",
                2uL,
                viewModel.uiState.value.designer?.conflictRevision,
            )

            // One budget from the RE-STAMP, not from the original stamp, which is
            // still an hour ahead of this clock: a gate that only ignored the sign
            // would keep every app's queue closed here.
            viewModel.currentTimeMs = { stepped + 15_000 }
            viewModel.onAction(LocalAppsAction.ConfirmDesign)
            advanceUntilIdle()

            assertEquals(
                "an overdue refresh must not wedge the process-global gate",
                2,
                source.updateCommands().size,
            )
            assertUpdate(source.updateCommands().last(), expectedRevision = 0u, value = "second")
            assertNull(
                "a gate released without its snapshot must not claim the designer reloaded",
                viewModel.uiState.value.designer?.conflictRevision,
            )
        } finally {
            Dispatchers.resetMain()
        }
    }

    /**
     * `AppOperationFailed.app_id` is `Option<String>` on the wire and skipped when
     * absent, so a failure addressing no app is a shape the protocol permits. The
     * suppression guard compared it to `draftConflictRefresh?.appId` with `==`,
     * which is true when BOTH are null — no refresh pending and no app addressed
     * — so such a failure's banner was swallowed entirely. The sibling guard one
     * line below already spells `event.appId != null &&`.
     *
     * Reachability: no engine path emits this today. The only two handlers that
     * pass `app_id: None` are `handle_list_apps` and `handle_create_app`, and
     * neither can produce `RevisionConflict` — they yield the boot-time load
     * error, a DTO raise failure, or a create/persist failure. This is a guard
     * against a protocol-legal event, not a closed production bug.
     */
    @Test
    fun `a failure addressing no app is not a conflict recovery and still raises its banner`() = runTest {
        Dispatchers.setMain(StandardTestDispatcher(testScheduler))
        try {
            val source = RecordingSource()
            val viewModel = LocalAppsViewModel(
                sourceFlow = MutableStateFlow<ConversationSource>(source),
                distributionChannel = "store",
            )
            runCurrent()
            seedDesigner(source)
            runCurrent()
            assertNull("no refresh is pending", viewModel.uiState.value.error)

            source.emit(
                ClientEvent.AppOperationFailed(null, AppErrorCodeDto.REVISION_CONFLICT, "创建应用失败"),
            )
            runCurrent()

            assertEquals(
                "null == null must not be read as a conflict this client is recovering from",
                "创建应用失败",
                viewModel.uiState.value.error,
            )
        } finally {
            Dispatchers.resetMain()
        }
    }

    @Test
    fun `confirm is refused when the draft edit never acks`() = runTest {
        Dispatchers.setMain(StandardTestDispatcher(testScheduler))
        try {
            val source = RecordingSource()
            val viewModel = LocalAppsViewModel(
                sourceFlow = MutableStateFlow<ConversationSource>(source),
                distributionChannel = "store",
            )
            runCurrent()
            seedDesigner(source)
            runCurrent()

            viewModel.onAction(
                LocalAppsAction.EditField("purpose", LocalAppDesignValue.Text("final answer"), debounce = true),
            )
            advanceTimeBy(100)
            viewModel.onAction(LocalAppsAction.ConfirmDesign)
            runCurrent()
            assertUpdate(source.updateCommands().single(), expectedRevision = 0u, value = "final answer")

            // No AppDesignDraftChanged ever arrives, so the drain burns its full
            // 100 x 50 ms budget with the patch still outstanding.
            advanceTimeBy(6_000)
            runCurrent()

            assertTrue(
                "a confirm behind an unacked patch can only come back as a revision conflict",
                source.commands.none { it is ClientCommand.ConfirmAppDesign },
            )
            assertEquals("设计尚未保存完成，请重试", viewModel.uiState.value.error)
        } finally {
            Dispatchers.resetMain()
        }
    }

    /**
     * The post-conflict gate is cleared in exactly one place — `reduceDetails`,
     * which runs only for a SUCCESSFUL `GetAppDetails` for that app. An engine
     * rebind or project switch re-binds `ConversationSource` and cancels the
     * in-flight event collector, so the snapshot is simply never delivered and no
     * failure event arrives either. Unlike the in-flight slot this gate is
     * process-global, so without a bound it wedges later edits for EVERY app for
     * the life of the activity-scoped ViewModel.
     */
    @Test
    fun `a design conflict whose details refresh never answers stops blocking later edits`() = runTest {
        Dispatchers.setMain(StandardTestDispatcher(testScheduler))
        try {
            val source = RecordingSource()
            val viewModel = LocalAppsViewModel(
                sourceFlow = MutableStateFlow<ConversationSource>(source),
                distributionChannel = "store",
            )
            runCurrent()
            seedDesigner(source)
            runCurrent()

            viewModel.onAction(
                LocalAppsAction.EditField("purpose", LocalAppDesignValue.Text("first"), debounce = false),
            )
            runCurrent()
            assertEquals(1, source.updateCommands().size)

            source.emit(ClientEvent.AppDesignConflict(APP_ID, expectedRevision = 0u, actualRevision = 2u))
            runCurrent()
            assertTrue(source.commands.any { it == ClientCommand.GetAppDetails(APP_ID) })

            // Inside the budget the gate must still do what it exists to do: hold
            // the retry back until the authoritative snapshot lands. Removing the
            // gate is not the fix.
            viewModel.onAction(
                LocalAppsAction.EditField("purpose", LocalAppDesignValue.Text("second"), debounce = false),
            )
            runCurrent()
            assertEquals(
                "inside the budget the retry still waits for the authoritative snapshot",
                1,
                source.updateCommands().size,
            )

            // The snapshot never arrives, and no failure event arrives either.
            viewModel.inFlightEditBudgetMs = 0
            viewModel.onAction(
                LocalAppsAction.EditField("purpose", LocalAppDesignValue.Text("third"), debounce = false),
            )
            runCurrent()

            assertEquals(
                "an unanswered refresh must not wedge the queue forever",
                2,
                source.updateCommands().size,
            )
            assertUpdate(source.updateCommands().last(), expectedRevision = 0u, value = "third")
            assertEquals(
                "the typed answer must still be the one that goes out",
                LocalAppDesignValue.Text("third"),
                viewModel.uiState.value.designer?.values?.get("purpose"),
            )
            // The gate aged out because the snapshot never came, so the designer
            // must not go on rendering "…已重新加载，请检查后继续。" over values
            // nothing ever reloaded.
            assertNull(
                "an aged-out gate must not claim the designer reloaded",
                viewModel.uiState.value.designer?.conflictRevision,
            )
        } finally {
            Dispatchers.resetMain()
        }
    }

    /**
     * The refresh a conflict asks for can be answered with a failure instead of a
     * snapshot — the app was deleted from another surface, or its store is
     * corrupt. `reduceDetails` then never runs for that app again (no later
     * openApp can reach a deleted id), so nothing else would clear the gate.
     */
    @Test
    fun `a details refresh that fails outright releases the conflict gate at once`() = runTest {
        Dispatchers.setMain(StandardTestDispatcher(testScheduler))
        try {
            val source = RecordingSource()
            val viewModel = LocalAppsViewModel(
                sourceFlow = MutableStateFlow<ConversationSource>(source),
                distributionChannel = "store",
            )
            runCurrent()
            seedDesigner(source)
            runCurrent()

            viewModel.onAction(
                LocalAppsAction.EditField("purpose", LocalAppDesignValue.Text("first"), debounce = false),
            )
            runCurrent()
            assertEquals(1, source.updateCommands().size)

            source.emit(ClientEvent.AppDesignConflict(APP_ID, expectedRevision = 0u, actualRevision = 2u))
            runCurrent()
            assertTrue(source.commands.any { it == ClientCommand.GetAppDetails(APP_ID) })
            assertEquals("the retry waits for the snapshot", 1, source.updateCommands().size)

            // The GetAppDetails the conflict submitted is what failed.
            source.emit(ClientEvent.AppOperationFailed(APP_ID, AppErrorCodeDto.NOT_FOUND, "应用不存在"))
            runCurrent()

            // No clock involved: the budget is still the production 15 s.
            assertEquals(
                "a failed refresh must release the gate without waiting out the budget",
                2,
                source.updateCommands().size,
            )
            assertUpdate(source.updateCommands().last(), expectedRevision = 0u, value = "first")
            assertEquals("应用不存在", viewModel.uiState.value.error)
            // The gate was released precisely BECAUSE the authoritative snapshot
            // is not coming, so the designer must not keep asserting one landed:
            // `conflictRevision` renders "…已重新加载，请检查后继续。" over the
            // pre-conflict local values.
            assertNull(
                "a gate released without its snapshot must not claim the designer reloaded",
                viewModel.uiState.value.designer?.conflictRevision,
            )
        } finally {
            Dispatchers.resetMain()
        }
    }

    /**
     * The ledger chain. A conflict re-queues the rejected patch and raises the
     * refresh gate; the app was deleted from another surface, so the refresh
     * fails, and the resend the release triggers earns NOT_FOUND — a code
     * neither `rejectedDraftPatch` nor the ack arm may attribute, so it holds
     * the process-global in-flight slot. Nothing released that slot, and nothing
     * released the designer pinned to the deleted app, so every later edit for
     * every OTHER app was queued and never sent.
     */
    @Test
    fun `a deleted app releases the draft slot instead of wedging every other designer`() = runTest {
        Dispatchers.setMain(StandardTestDispatcher(testScheduler))
        try {
            val source = RecordingSource()
            val viewModel = LocalAppsViewModel(
                sourceFlow = MutableStateFlow<ConversationSource>(source),
                distributionChannel = "store",
            )
            runCurrent()
            seedDesigner(source)
            runCurrent()

            viewModel.onAction(
                LocalAppsAction.EditField("purpose", LocalAppDesignValue.Text("first"), debounce = false),
            )
            runCurrent()
            source.emit(ClientEvent.AppDesignConflict(APP_ID, expectedRevision = 0u, actualRevision = 2u))
            runCurrent()
            assertEquals(2uL, viewModel.uiState.value.designer?.conflictRevision)

            // The refresh the conflict asked for comes back NOT_FOUND, the gate
            // is released, and the re-queued patch goes out again…
            source.emit(ClientEvent.AppOperationFailed(APP_ID, AppErrorCodeDto.NOT_FOUND, "应用不存在"))
            runCurrent()
            assertEquals(2, source.updateCommands().size)
            // …to be answered NOT_FOUND a second time, which no arm may attribute,
            // so the resend keeps the single slot.
            source.emit(ClientEvent.AppOperationFailed(APP_ID, AppErrorCodeDto.NOT_FOUND, "应用不存在"))
            runCurrent()
            assertEquals("the resend stays stuck in the single slot", 2, source.updateCommands().size)

            // The delete's own snapshot lands. `AppsChanged` is the FULL record
            // set, so an app missing from it is deleted, not elided.
            source.emit(ClientEvent.AppsChanged(listOf(appRecord(OTHER_APP_ID, "订单"))))
            runCurrent()

            assertNull("a designer for a deleted app can never be reached again", viewModel.uiState.value.designer)
            assertEquals(LocalAppsDestination.Library, viewModel.uiState.value.destination)

            // `drained()` must not be able to answer for an app whose queued
            // edits were discarded: the confirm gate goes with the designer, so
            // no ConfirmAppDesign can ship a spec the engine never received.
            viewModel.onAction(LocalAppsAction.ConfirmDesign)
            advanceUntilIdle()
            assertTrue(
                "a deleted app must never confirm",
                source.commands.none { it is ClientCommand.ConfirmAppDesign },
            )

            // The surviving app's designer opens and its edits reach the engine
            // on the production 15 s budget — no watchdog, no clock manipulation.
            // Each app is authored its OWN questionnaire now (Task 18: no more
            // shared static catalog every CRUD_TRACKER app resolved fields
            // from), so OTHER_APP_ID needs its own `AppQuestionnaireChanged`
            // before its designer can be edited.
            source.emit(
                ClientEvent.AppEvent(
                    AppEventDto.AppQuestionnaireChanged(appId = OTHER_APP_ID, revision = 0u, steps = listOf(basicsStepDto())),
                ),
            )
            source.emit(ClientEvent.AppDesignerRequested(OTHER_APP_ID, interactionId = "designer-2", revision = 0u))
            runCurrent()
            viewModel.onAction(
                LocalAppsAction.EditField("purpose", LocalAppDesignValue.Text("b-typed"), debounce = false),
            )
            runCurrent()

            assertEquals("the deleted app must not wedge the surviving one", 3, source.updateCommands().size)
            val sent = source.updateCommands().last()
            assertEquals(OTHER_APP_ID, sent.appId)
            assertEquals(DesignValueDto.ShortText("b-typed"), sent.sentValue())
        } finally {
            Dispatchers.resetMain()
        }
    }

    /**
     * The delete affordance is the library card's ⋮ menu and nothing else, so a
     * user-initiated delete always arrives with the user standing on the library.
     * `state.designer` is still pinned to the last app they opened — nothing but
     * `reduceApps` ever clears it — so a teardown keyed off it announced a
     * FAILURE for the operation the user had just asked for, and claimed to have
     * returned them to a list they had never left.
     */
    @Test
    fun `deleting an app the user already navigated away from is not a failure`() = runTest {
        Dispatchers.setMain(StandardTestDispatcher(testScheduler))
        try {
            val source = RecordingSource()
            val viewModel = LocalAppsViewModel(
                sourceFlow = MutableStateFlow<ConversationSource>(source),
                distributionChannel = "store",
            )
            runCurrent()
            seedDesigner(source)
            source.emit(ClientEvent.AppsChanged(listOf(appRecord(), appRecord(OTHER_APP_ID, "订单"))))
            runCurrent()

            viewModel.onAction(LocalAppsAction.Back)
            runCurrent()
            assertEquals(LocalAppsDestination.Library, viewModel.uiState.value.destination)
            assertEquals(
                "navigation leaves the designer pinned, which is what made this reachable",
                APP_ID,
                viewModel.uiState.value.designer?.appId,
            )

            viewModel.onAction(LocalAppsAction.DeleteApp(APP_ID))
            runCurrent()
            source.emit(ClientEvent.AppsChanged(listOf(appRecord(OTHER_APP_ID, "订单"))))
            runCurrent()

            assertNull(
                "a delete the user asked for must not raise the 应用操作失败 dialog",
                viewModel.uiState.value.error,
            )
            assertNull("the teardown itself must still happen", viewModel.uiState.value.designer)
            assertEquals(LocalAppsDestination.Library, viewModel.uiState.value.destination)
        } finally {
            Dispatchers.resetMain()
        }
    }

    /**
     * The case that IS worth a dialog: the app went away underneath a designer
     * the user was standing on, and took an answer they typed with it. The prune
     * discards that patch deliberately, so this is the only place the loss can be
     * reported.
     */
    @Test
    fun `an app deleted under an open designer says what the deletion took`() = runTest {
        Dispatchers.setMain(StandardTestDispatcher(testScheduler))
        try {
            val source = RecordingSource()
            val viewModel = LocalAppsViewModel(
                sourceFlow = MutableStateFlow<ConversationSource>(source),
                distributionChannel = "store",
            )
            runCurrent()
            seedDesigner(source)
            runCurrent()
            assertEquals(LocalAppsDestination.Designer(APP_ID), viewModel.uiState.value.destination)

            viewModel.onAction(
                LocalAppsAction.EditField("purpose", LocalAppDesignValue.Text("typed"), debounce = false),
            )
            runCurrent()
            assertEquals("the answer is in flight and never acked", 1, source.updateCommands().size)

            // Deleted from another surface while that designer is on screen.
            source.emit(ClientEvent.AppsChanged(listOf(appRecord(OTHER_APP_ID, "订单"))))
            runCurrent()

            assertEquals(
                "该应用已被删除，尚未保存的设计修改已丢失，已返回应用列表。",
                viewModel.uiState.value.error,
            )
            assertEquals(LocalAppsDestination.Library, viewModel.uiState.value.destination)
        } finally {
            Dispatchers.resetMain()
        }
    }

    /**
     * `designer` was the only per-app state the teardown rescued the destination
     * for, so a user standing on a deleted app's Details or Preview screen kept it
     * while `details`/`previews` were filtered out from under them — and
     * `selectedAppId`, which `SelectDetailsTab` and both generation reducers steer
     * off, went on naming the dead app.
     */
    @Test
    fun `a deleted app cannot leave the user on its details or preview screen`() = runTest {
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
            assertEquals(LocalAppsDestination.Details(APP_ID), viewModel.uiState.value.destination)
            assertEquals(APP_ID, viewModel.uiState.value.selectedAppId)

            source.emit(ClientEvent.AppsChanged(listOf(appRecord(OTHER_APP_ID, "订单", AppWorkflowStateDto.READY))))
            runCurrent()

            assertEquals(
                "a details screen for a deleted app has nothing left to render",
                LocalAppsDestination.Library,
                viewModel.uiState.value.destination,
            )
            assertNull(
                "the selection the details tab steers off must not name a deleted app",
                viewModel.uiState.value.selectedAppId,
            )
            assertNull("nothing was lost, so nothing to report", viewModel.uiState.value.error)

            source.emit(
                ClientEvent.AppPreviewReady(
                    appId = OTHER_APP_ID,
                    interactionId = "preview-1",
                    revision = 1u,
                    url = null,
                ),
            )
            runCurrent()
            assertEquals(LocalAppsDestination.Preview(OTHER_APP_ID), viewModel.uiState.value.destination)

            source.emit(ClientEvent.AppsChanged(emptyList()))
            runCurrent()
            assertEquals(
                "a preview screen for a deleted app has nothing left to render",
                LocalAppsDestination.Library,
                viewModel.uiState.value.destination,
            )
            assertNull(viewModel.uiState.value.selectedAppId)
        } finally {
            Dispatchers.resetMain()
        }
    }

    /**
     * The gate is released by a details snapshot unconditionally, so the
     * banner it raised has to come down with it. `conflictRevision` renders
     * "…已重新加载，请检查后继续。".
     *
     * Was `a details snapshot with no matching template still takes the
     * conflict banner down`: the "no matching template" half of that scenario
     * no longer exists (local-apps#questionnaire, Task 18) — `reduceDetails`
     * does not look up a template before replacing `values` any more (the
     * questionnaire is looked up separately, by app id, not carried on the
     * designer), so there is no "snapshot arrived but could not reload"
     * branch left to cover. This migrates the surviving assertion: a details
     * snapshot always takes the banner down.
     */
    @Test
    fun `a details snapshot always takes the conflict banner down`() = runTest {
        Dispatchers.setMain(StandardTestDispatcher(testScheduler))
        try {
            val source = RecordingSource()
            val viewModel = LocalAppsViewModel(
                sourceFlow = MutableStateFlow<ConversationSource>(source),
                distributionChannel = "store",
            )
            runCurrent()
            seedDesigner(source)
            runCurrent()

            viewModel.onAction(
                LocalAppsAction.EditField("purpose", LocalAppDesignValue.Text("first"), debounce = false),
            )
            runCurrent()
            assertEquals(1, source.updateCommands().size)

            source.emit(ClientEvent.AppDesignConflict(APP_ID, expectedRevision = 0u, actualRevision = 2u))
            runCurrent()
            assertEquals(2uL, viewModel.uiState.value.designer?.conflictRevision)

            source.emit(ClientEvent.AppEvent(AppEventDto.AppDetailsChanged(appDetails(designRevision = 2u))))
            runCurrent()

            assertNull(
                "the gate came down, so the banner it raised must not survive it",
                viewModel.uiState.value.designer?.conflictRevision,
            )
            assertEquals(
                "the gate really did come down: the re-queued patch went out",
                2,
                source.updateCommands().size,
            )
        } finally {
            Dispatchers.resetMain()
        }
    }

    /**
     * Head-of-line blocking needs no deleted app at all: the queue is
     * process-global while the designer is one app, so an edit left behind by a
     * designer the user closed sits at the head, and the only removal from the
     * queue used to be gated on that head matching the designer on screen.
     */
    @Test
    fun `an edit left behind by another designer does not block the one on screen`() = runTest {
        Dispatchers.setMain(StandardTestDispatcher(testScheduler))
        try {
            val source = RecordingSource()
            val viewModel = LocalAppsViewModel(
                sourceFlow = MutableStateFlow<ConversationSource>(source),
                distributionChannel = "store",
            )
            runCurrent()
            seedDesigner(source)
            // Both apps stay live for the whole test.
            source.emit(ClientEvent.AppsChanged(listOf(appRecord(), appRecord(OTHER_APP_ID, "订单"))))
            runCurrent()

            viewModel.onAction(
                LocalAppsAction.EditField("purpose", LocalAppDesignValue.Text("a-typed"), debounce = true),
            )
            runCurrent()

            // The user leaves app A's designer for app B's before the 400 ms
            // debounce fires. Each app is authored its OWN questionnaire now
            // (Task 18), so OTHER_APP_ID needs its own
            // `AppQuestionnaireChanged` before its designer can be edited.
            source.emit(
                ClientEvent.AppEvent(
                    AppEventDto.AppQuestionnaireChanged(appId = OTHER_APP_ID, revision = 0u, steps = listOf(basicsStepDto())),
                ),
            )
            source.emit(ClientEvent.AppDesignerRequested(OTHER_APP_ID, interactionId = "designer-2", revision = 0u))
            runCurrent()
            assertEquals(OTHER_APP_ID, viewModel.uiState.value.designer?.appId)

            advanceTimeBy(400)
            runCurrent()
            assertEquals("A's designer is closed, so nothing may go out for A", 0, source.updateCommands().size)

            viewModel.onAction(
                LocalAppsAction.EditField("purpose", LocalAppDesignValue.Text("b-typed"), debounce = false),
            )
            runCurrent()

            val sent = source.updateCommands().single()
            assertEquals("the designer on screen must not queue behind another app", OTHER_APP_ID, sent.appId)
            assertEquals(DesignValueDto.ShortText("b-typed"), sent.sentValue())

            source.emit(
                ClientEvent.AppDesignDraftChanged(
                    appId = OTHER_APP_ID,
                    revision = 1u,
                    fields = mapOf("purpose" to DesignValueDto.ShortText("b-typed")),
                ),
            )
            runCurrent()
            assertEquals("nothing else is sendable while B's designer is open", 1, source.updateCommands().size)

            // A's edit was skipped, not dropped: it is still owed to A and goes
            // out as soon as A's designer is the one on screen again.
            source.emit(ClientEvent.AppDesignerRequested(APP_ID, interactionId = "designer-1", revision = 0u))
            runCurrent()
            viewModel.onAction(LocalAppsAction.ConfirmDesign)
            advanceUntilIdle()

            val resumed = source.updateCommands().last()
            assertEquals(APP_ID, resumed.appId)
            assertEquals(DesignValueDto.ShortText("a-typed"), resumed.sentValue())
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
    fun `structured ui targets preserve semantic button and labelled input requests`() = runTest {
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
                            requestId = "cap-2",
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
                            requestId = "ui-button",
                            appId = APP_ID,
                            action = AppUiActionKindDto.CLICK,
                            target = AppUiTargetDto(elementId = null, role = "button", name = "Save"),
                            value = null,
                        ),
                    ),
                ),
            )
            runCurrent()
            assertEquals(
                LocalAppUiAutomationAction.Click(LocalAppUiTarget(role = "button", name = "Save")),
                viewModel.uiState.value.pendingUiAction?.action,
            )

            viewModel.onAction(
                LocalAppsAction.UiActionHandled("ui-button", resultJson = "{\"ok\":true}", error = null),
            )
            runCurrent()

            source.emit(
                ClientEvent.AppEvent(
                    AppEventDto.AppUiRequest(
                        AppUiRequestDto(
                            requestId = "ui-fill",
                            appId = APP_ID,
                            action = AppUiActionKindDto.FILL,
                            target = AppUiTargetDto(elementId = null, role = "textbox", name = "Title"),
                            value = "Orders",
                        ),
                    ),
                ),
            )
            runCurrent()
            assertEquals(
                LocalAppUiAutomationAction.Fill(
                    LocalAppUiTarget(role = "textbox", name = "Title"),
                    "Orders",
                ),
                viewModel.uiState.value.pendingUiAction?.action,
            )
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

    @Test
    fun `navigate ui requests accept an absolute same-origin url`() = runTest {
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
                            requestId = "cap-4",
                            appId = APP_ID,
                            capability = AppCapabilityKindDto.UI_CONTROL,
                            domain = null,
                            reason = "Agent needs to navigate",
                        ),
                    ),
                ),
            )
            runCurrent()
            viewModel.onAction(LocalAppsAction.ResolveAuthorization(LocalAppAuthorizationDecision.AllowSession))
            runCurrent()

            // `inspect` hands the agent back exactly this shape; iOS accepts it.
            source.emit(
                ClientEvent.AppEvent(
                    AppEventDto.AppUiRequest(
                        AppUiRequestDto(
                            requestId = "ui-nav",
                            appId = APP_ID,
                            action = AppUiActionKindDto.NAVIGATE,
                            target = null,
                            value = "http://127.0.0.1:41234/items",
                        ),
                    ),
                ),
            )
            runCurrent()
            assertEquals(
                LocalAppUiAutomationAction.Navigate("http://127.0.0.1:41234/items"),
                viewModel.uiState.value.pendingUiAction?.action,
            )
            viewModel.onAction(
                LocalAppsAction.UiActionHandled("ui-nav", resultJson = "{\"ok\":true}", error = null),
            )
            runCurrent()

            source.emit(
                ClientEvent.AppEvent(
                    AppEventDto.AppUiRequest(
                        AppUiRequestDto(
                            requestId = "ui-nav-empty",
                            appId = APP_ID,
                            action = AppUiActionKindDto.NAVIGATE,
                            target = null,
                            value = null,
                        ),
                    ),
                ),
            )
            runCurrent()
            val emptyResolution = source.commands
                .filterIsInstance<ClientCommand.ResolveAppUiRequest>()
                .last()
            assertEquals("ui-nav-empty", emptyResolution.requestId)
            assertEquals("UI 请求缺少有效目标或参数", emptyResolution.error)
        } finally {
            Dispatchers.resetMain()
        }
    }

    private fun seedDesigner(source: RecordingSource) {
        // NOTE (local-apps#questionnaire, Task 18): the static template
        // catalogue (`AppEventDto.AppTemplatesChanged`) is gone — a
        // questionnaire is authored per-app and arrives as
        // `AppEventDto.AppQuestionnaireChanged`, an ordered bag of steps with
        // no catalogue wrapper (no `kind`/`version`/`name`/`description`).
        source.emit(
            ClientEvent.AppEvent(
                AppEventDto.AppQuestionnaireChanged(
                    appId = APP_ID,
                    revision = 0u,
                    steps = listOf(basicsStepDto()),
                ),
            ),
        )
        source.emit(ClientEvent.AppsChanged(listOf(appRecord())))
        source.emit(ClientEvent.AppDesignerRequested(APP_ID, interactionId = "designer-1", revision = 0u))
    }

    private fun basicsStepDto(fieldId: String = "purpose", allowsDefer: Boolean = false) = AppDesignStepDto(
        id = "basics",
        order = 1u,
        title = "基础",
        description = null,
        fields = listOf(
            AppDesignFieldDto(
                id = fieldId,
                label = "用途",
                description = null,
                fieldType = AppDesignFieldTypeDto.SHORT_TEXT,
                required = true,
                allowsCustom = false,
                allowsDefer = allowsDefer,
                defaultValue = DesignValueDto.ShortText(""),
                options = emptyList(),
            ),
        ),
    )

    private fun onePlanDto() = AppPlanDto(
        collections = emptyList(),
        capabilities = emptyList(),
        domains = emptyList(),
        summary = "一个客户跟进应用",
    )

    private fun RecordingSource.updateCommands(): List<ClientCommand.UpdateAppDesignDraft> =
        commands.filterIsInstance<ClientCommand.UpdateAppDesignDraft>()

    private fun assertUpdate(
        command: ClientCommand.UpdateAppDesignDraft,
        expectedRevision: ULong,
        value: String,
    ) {
        assertEquals(expectedRevision, command.expectedRevision)
        val operation = command.patch.ops.single() as AppDesignPatchOpDto.Set
        assertEquals("purpose", operation.fieldId)
        assertEquals(DesignValueDto.ShortText(value), operation.value)
    }

    private fun appRecord(
        id: String = APP_ID,
        name: String = "客户跟进",
        workflow: AppWorkflowStateDto = AppWorkflowStateDto.COLLECTING_SPEC,
        brief: String = "记录客户跟进情况",
    ) = AppRecordDto(
        id = id,
        name = name,
        brief = brief,
        createdAtMs = 1u,
        updatedAtMs = 2u,
        workflowState = workflow,
        conversationId = null,
        workspaceRel = "apps/$id/workspace",
    )

    private fun appDetails(designRevision: ULong) = AppDetailsDto(
        app = appRecord(),
        designRevision = designRevision,
        designFields = listOf(AppDesignFieldValueDto("purpose", DesignValueDto.ShortText("external"))),
        questionnaire = listOf(basicsStepDto()),
        plan = null,
        manifest = null,
        runtime = AppRuntimeDetailsDto(
            state = AppRuntimeStateDto.STOPPED,
            mode = null,
            loopbackUrl = null,
            suspensionReason = null,
            recoveryState = null,
            lastError = null,
        ),
        generationJob = null,
        checkpoints = emptyList(),
    )

    private fun ClientCommand.UpdateAppDesignDraft.sentValue(): DesignValueDto =
        (patch.ops.single() as AppDesignPatchOpDto.Set).value

    private companion object {
        const val APP_ID = "tracker"
        const val OTHER_APP_ID = "orders"
    }
}
