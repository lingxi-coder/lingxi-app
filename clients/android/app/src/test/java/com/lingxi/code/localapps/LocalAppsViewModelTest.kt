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
import com.lingxi.code.bindings.AppRecordDto
import com.lingxi.code.bindings.AppRuntimeDetailsDto
import com.lingxi.code.bindings.AppRuntimeStateDto
import com.lingxi.code.bindings.AppTemplateDto
import com.lingxi.code.bindings.AppTemplateKindDto
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
import kotlinx.coroutines.test.StandardTestDispatcher
import kotlinx.coroutines.test.advanceTimeBy
import kotlinx.coroutines.test.advanceUntilIdle
import kotlinx.coroutines.test.resetMain
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest
import kotlinx.coroutines.test.setMain
import org.junit.Assert.assertEquals
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
            assertTrue(source.commands.contains(ClientCommand.ListAppTemplates))
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
        source.emit(
            ClientEvent.AppEvent(
                AppEventDto.AppTemplatesChanged(
                    listOf(
                        AppTemplateDto(
                            kind = AppTemplateKindDto.CRUD_TRACKER,
                            version = 1u,
                            name = "CRUD Tracker",
                            description = "",
                            steps = listOf(
                                AppDesignStepDto(
                                    id = "basics",
                                    order = 1u,
                                    title = "基础",
                                    description = null,
                                    fields = listOf(
                                        AppDesignFieldDto(
                                            id = "purpose",
                                            label = "用途",
                                            description = null,
                                            fieldType = AppDesignFieldTypeDto.SHORT_TEXT,
                                            required = true,
                                            defaultValue = DesignValueDto.ShortText(""),
                                            options = emptyList(),
                                        ),
                                    ),
                                ),
                            ),
                            collections = emptyList(),
                        ),
                    ),
                ),
            ),
        )
        source.emit(ClientEvent.AppsChanged(listOf(appRecord())))
        source.emit(ClientEvent.AppDesignerRequested(APP_ID, interactionId = "designer-1", revision = 0u))
    }

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

    private fun appRecord() = AppRecordDto(
        id = APP_ID,
        name = "客户跟进",
        template = AppTemplateKindDto.CRUD_TRACKER,
        createdAtMs = 1u,
        updatedAtMs = 2u,
        workflowState = AppWorkflowStateDto.COLLECTING_SPEC,
        conversationId = null,
        workspaceRel = "apps/$APP_ID/workspace",
    )

    private companion object {
        const val APP_ID = "tracker"
    }
}
