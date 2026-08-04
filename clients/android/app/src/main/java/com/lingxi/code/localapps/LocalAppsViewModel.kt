package com.lingxi.code.localapps

import androidx.lifecycle.ViewModel
import androidx.lifecycle.ViewModelProvider
import androidx.lifecycle.viewModelScope
import com.lingxi.code.BuildConfig
import com.lingxi.code.bindings.AppAuthorizationDecisionDto
import com.lingxi.code.bindings.AppBridgeOperationDto
import com.lingxi.code.bindings.AppBridgeRequestDto
import com.lingxi.code.bindings.AppCapabilityKindDto
import com.lingxi.code.bindings.AppCreateOriginDto
import com.lingxi.code.bindings.AppDataFieldDto
import com.lingxi.code.bindings.AppDataFieldTypeDto
import com.lingxi.code.bindings.AppDesignPatchDto
import com.lingxi.code.bindings.AppDesignPatchOpDto
import com.lingxi.code.bindings.AppDesignFieldTypeDto
import com.lingxi.code.bindings.AppErrorCodeDto
import com.lingxi.code.bindings.AppEventDto
import com.lingxi.code.bindings.AppGenerationJobDto
import com.lingxi.code.bindings.AppRecordDto
import com.lingxi.code.bindings.AppRuntimeDetailsDto
import com.lingxi.code.bindings.AppRuntimeModeDto
import com.lingxi.code.bindings.AppRuntimeStateDto
import com.lingxi.code.bindings.AppTemplateDto
import com.lingxi.code.bindings.AppUiActionKindDto
import com.lingxi.code.bindings.AppUiRequestDto
import com.lingxi.code.bindings.AppTemplateKindDto
import com.lingxi.code.bindings.AppWorkflowStateDto
import com.lingxi.code.bindings.ClientCommand
import com.lingxi.code.bindings.ClientEvent
import com.lingxi.code.bindings.DensityLevelDto
import com.lingxi.code.bindings.DesignValueDto
import com.lingxi.code.conversation.ConversationSource
import kotlinx.coroutines.CoroutineStart
import kotlinx.coroutines.Job
import kotlinx.coroutines.coroutineScope
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.collectLatest
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch
import org.json.JSONObject

class LocalAppsViewModel(
    private val sourceFlow: StateFlow<ConversationSource>,
    distributionChannel: String = BuildConfig.DISTRIBUTION_CHANNEL,
) : ViewModel() {
    private val _uiState = MutableStateFlow(
        LocalAppsUiState(
            distributionMode = if (distributionChannel == "full") {
                LocalAppRuntimeMode.NextProduction
            } else {
                LocalAppRuntimeMode.StaticExport
            },
        ),
    )
    val uiState: StateFlow<LocalAppsUiState> = _uiState.asStateFlow()

    private var source: ConversationSource? = null
    private var pendingCreate: Pair<String, String>? = null
    private val draftEditQueue = mutableListOf<QueuedDraftEdit>()
    private val textEditJobs = mutableMapOf<String, Job>()
    private var draftEditInFlight: InFlightDraftEdit? = null

    /**
     * How long a sent draft patch may hold the single in-flight slot before it is
     * assumed lost and re-queued. Far longer than a healthy ack, so it normally
     * only fires on a failure the event stream could not attribute — but it is a
     * heuristic, not a guarantee: a slow engine or a wall-clock step can trip it
     * early. That is tolerable only because the aged-out edit is RE-SENT rather
     * than dropped, so an early trip costs one idempotent set-op. Settable so a
     * test can reach the path without waiting out the production budget.
     */
    internal var inFlightEditBudgetMs: Long = 15_000

    private var draftConflictRefreshAppId: String? = null
    private var draftEditSequence = 0L
    private val pendingCapabilityKinds = mutableMapOf<String, AppCapabilityKindDto>()
    private val queuedAuthorizations = ArrayDeque<LocalAppAuthorizationRequest>()
    private val uiControlGrants = mutableMapOf<String, LocalAppAuthorizationDecision>()
    private val runtimeLastUsedAt = mutableMapOf<String, Long>()

    private data class QueuedDraftEdit(
        val appId: String,
        val fieldId: String,
        val value: LocalAppDesignValue,
        val bindingValue: DesignValueDto,
        val sequence: Long,
        val ready: Boolean,
    )

    private data class InFlightDraftEdit(
        val edit: QueuedDraftEdit,
        val expectedRevision: ULong,
        val sentAtMs: Long = System.currentTimeMillis(),
    )

    init {
        viewModelScope.launch {
            sourceFlow.collectLatest { bound ->
                source = bound
                _uiState.update { it.copy(loading = true, templatesLoading = true, error = null) }
                coroutineScope {
                    launch(start = CoroutineStart.UNDISPATCHED) {
                        bound.clientEvents.collect(::reduce)
                    }
                    launch { requestSnapshots(bound) }
                }
            }
        }
        viewModelScope.launch {
            LocalAppsMemoryPressure.events.collect {
                val victim = _uiState.value.apps
                    .filter { it.runtime.state == LocalAppRuntimeState.Running }
                    .minByOrNull { runtimeLastUsedAt[it.id] ?: it.updatedAtMs }
                victim?.let { submit(ClientCommand.StopApp(it.id)) }
            }
        }
    }

    fun onAction(action: LocalAppsAction) {
        when (action) {
            LocalAppsAction.Refresh -> submit { requestSnapshots(it) }
            LocalAppsAction.Create -> _uiState.update {
                it.copy(destination = LocalAppsDestination.Templates, createName = "", selectedTemplateKind = null)
            }
            is LocalAppsAction.Search -> _uiState.update { it.copy(query = action.query) }
            is LocalAppsAction.FilterTemplate -> _uiState.update { it.copy(templateFilter = action.kind) }
            is LocalAppsAction.ChangeCreateName -> _uiState.update { it.copy(createName = action.name) }
            is LocalAppsAction.SelectTemplate -> _uiState.update { it.copy(selectedTemplateKind = action.kind) }
            LocalAppsAction.CreateSelectedTemplate -> createSelectedTemplate()
            is LocalAppsAction.OpenApp -> openApp(action.appId)
            is LocalAppsAction.OpenDesigner -> openDesigner(action.appId)
            is LocalAppsAction.ChangeStep -> _uiState.update { state ->
                state.copy(designer = state.designer?.copy(stepIndex = action.index))
            }
            is LocalAppsAction.EditField -> editField(action)
            LocalAppsAction.RequestSuggestion -> requestSuggestion()
            LocalAppsAction.ApplySuggestion -> applySuggestion()
            LocalAppsAction.DismissSuggestion -> dismissSuggestion()
            LocalAppsAction.ConfirmDesign -> confirmDesign()
            is LocalAppsAction.StartRuntime -> {
                runtimeLastUsedAt[action.appId] = System.currentTimeMillis()
                submit(ClientCommand.StartApp(action.appId))
            }
            is LocalAppsAction.StopRuntime -> submit(ClientCommand.StopApp(action.appId))
            is LocalAppsAction.RetryGeneration -> retryGeneration(action.appId)
            is LocalAppsAction.DeleteApp -> submit(ClientCommand.DeleteApp(action.appId))
            is LocalAppsAction.ResetPermissions -> {
                uiControlGrants.remove(action.appId)
                submit(ClientCommand.ResetAppPermissions(action.appId))
            }
            is LocalAppsAction.RestoreCheckpoint -> submit(
                ClientCommand.RestoreAppCheckpoint(action.appId, action.checkpointId),
            )
            is LocalAppsAction.ApprovePreview -> approvePreview(action.appId)
            is LocalAppsAction.SubmitRevision -> submit(ClientCommand.RequestAppRevision(action.appId, action.feedback))
            is LocalAppsAction.BridgeRequest -> executeBridgeRequest(action.message)
            is LocalAppsAction.AcknowledgeBridgeResult -> _uiState.update {
                it.copy(bridgeResults = it.bridgeResults - action.requestId)
            }
            is LocalAppsAction.ResolveAuthorization -> resolveAuthorization(action.decision)
            is LocalAppsAction.UiActionHandled -> resolveCompletedUiAction(action)
            is LocalAppsAction.SelectDetailsTab -> _uiState.update { state ->
                val appId = state.selectedAppId ?: return@update state
                state.copy(
                    selectedDetailsTab = action.tab,
                    destination = LocalAppsDestination.Details(appId, action.tab),
                )
            }
            LocalAppsAction.Back -> navigateBack()
            LocalAppsAction.DismissError -> _uiState.update { it.copy(error = null) }
        }
    }

    private suspend fun requestSnapshots(bound: ConversationSource) {
        runCatching { bound.submitClientCommand(ClientCommand.ListApps) }
            .onFailure { error("无法加载应用：${it.message ?: it::class.simpleName}") }
        requestTemplates(bound)
    }

    private suspend fun requestTemplates(bound: ConversationSource) {
        runCatching { bound.submitClientCommand(ClientCommand.ListAppTemplates) }
            .onFailure { error("无法加载应用模板：${it.message ?: it::class.simpleName}") }
    }

    private fun createSelectedTemplate() {
        val state = _uiState.value
        val template = state.templates.firstOrNull { it.kind == state.selectedTemplateKind } ?: return
        val name = state.createName.trim()
        if (name.isEmpty()) return
        pendingCreate = name to template.kind
        submit(
            ClientCommand.CreateApp(
                name = name,
                template = template.kind.toBindingTemplateKind(),
                origin = AppCreateOriginDto.LIBRARY,
                conversationId = null,
            ),
        )
    }

    private fun openApp(appId: String) {
        val app = _uiState.value.apps.firstOrNull { it.id == appId } ?: return
        _uiState.update { it.copy(selectedAppId = appId) }
        submit(ClientCommand.GetAppDetails(appId))
        when (app.workflow) {
            LocalAppWorkflow.CollectingSpec,
            LocalAppWorkflow.AwaitingSpecConfirmation -> openDesigner(appId)
            LocalAppWorkflow.Generating,
            LocalAppWorkflow.Validating,
            LocalAppWorkflow.AwaitingPreviewConfirmation,
            LocalAppWorkflow.Revising,
            LocalAppWorkflow.GenerationFailed,
            LocalAppWorkflow.ValidationFailed -> _uiState.update {
                it.copy(destination = LocalAppsDestination.Preview(appId))
            }
            LocalAppWorkflow.Ready -> _uiState.update {
                it.copy(destination = LocalAppsDestination.Details(appId), selectedDetailsTab = LocalAppDetailsTab.Preview)
            }
        }
    }

    private fun openDesigner(appId: String) {
        _uiState.update { it.copy(destination = LocalAppsDestination.Designer(appId), selectedAppId = appId) }
        submit(ClientCommand.GetAppDetails(appId))
        submit(ClientCommand.OpenAppDesigner(appId))
    }

    private fun editField(action: LocalAppsAction.EditField) {
        val designer = _uiState.value.designer ?: return
        val fieldKind = designer.template.steps
            .asSequence()
            .flatMap { it.fields.asSequence() }
            .firstOrNull { it.id == action.fieldId }
            ?.kind ?: return
        _uiState.update { state ->
            state.copy(designer = state.designer?.copy(values = state.designer.values + (action.fieldId to action.value)))
        }
        val jobKey = "${designer.appId}:${action.fieldId}"
        textEditJobs.remove(jobKey)?.cancel()
        val edit = QueuedDraftEdit(
            appId = designer.appId,
            fieldId = action.fieldId,
            value = action.value,
            bindingValue = action.value.toBindingValue(fieldKind),
            sequence = ++draftEditSequence,
            ready = !action.debounce,
        )
        val queuedIndex = draftEditQueue.indexOfLast {
            it.appId == edit.appId && it.fieldId == edit.fieldId
        }
        if (queuedIndex >= 0) {
            draftEditQueue[queuedIndex] = edit
        } else {
            draftEditQueue += edit
        }
        if (action.debounce) {
            textEditJobs[jobKey] = viewModelScope.launch {
                delay(400)
                val index = draftEditQueue.indexOfFirst { it.sequence == edit.sequence }
                if (index >= 0) {
                    draftEditQueue[index] = draftEditQueue[index].copy(ready = true)
                    pumpDraftEditQueue()
                }
                textEditJobs.remove(jobKey)
            }
        } else {
            pumpDraftEditQueue()
        }
    }

    /**
     * Sends at most one optimistic draft patch. The next patch is not assigned
     * a revision until the engine confirms the previous one with a full
     * [ClientEvent.AppDesignDraftChanged] snapshot.
     */
    private fun pumpDraftEditQueue() {
        // A patch whose failure could not be attributed (see the AppOperationFailed
        // arm) holds the slot forever, wedging every later edit for the app. Release
        // it lazily, here, rather than from a timer: this runs exactly when the user
        // types again, which is when the wedge starts to matter, and it keeps the
        // queue free of a background coroutine whose delay a virtual test clock would
        // fast-forward.
        draftEditInFlight?.let { stale ->
            if (System.currentTimeMillis() - stale.sentAtMs >= inFlightEditBudgetMs) {
                // Re-queue rather than discard, guarded like the conflict arm so a
                // newer local value for the field wins. Discarding would lose the
                // answer while `designer.values` kept the confirm gate satisfied, so a
                // confirm could ship a spec the engine never received. Resending is
                // safe and is what makes the wall clock acceptable here: a set-op is
                // idempotent if the ack was merely lost, and a patch sent against a
                // moved revision returns AppDesignConflict, which rebases. (iOS uses a
                // monotonic clock; this uses the wall clock because a JVM unit test
                // cannot call SystemClock. A clock step therefore only shifts WHEN the
                // resend happens, never whether the edit survives.)
                if (draftEditQueue.none { it.appId == stale.edit.appId && it.fieldId == stale.edit.fieldId }) {
                    draftEditQueue.add(0, stale.edit.copy(ready = true))
                }
                draftEditInFlight = null
            }
        }
        if (draftEditInFlight != null || draftConflictRefreshAppId != null) return
        val designer = _uiState.value.designer ?: return
        val next = draftEditQueue.firstOrNull() ?: return
        if (!next.ready || next.appId != designer.appId) return
        draftEditQueue.removeAt(0)
        val inFlight = InFlightDraftEdit(next, designer.revision)
        draftEditInFlight = inFlight
        submit(
            ClientCommand.UpdateAppDesignDraft(
                appId = next.appId,
                expectedRevision = inFlight.expectedRevision,
                patch = AppDesignPatchDto(
                    ops = listOf(AppDesignPatchOpDto.Set(next.fieldId, next.bindingValue)),
                    note = null,
                ),
            ),
        )
    }

    private fun pendingDraftValues(appId: String): Map<String, LocalAppDesignValue> = buildMap {
        draftEditInFlight?.edit
            ?.takeIf { it.appId == appId }
            ?.let { put(it.fieldId, it.value) }
        draftEditQueue.asSequence()
            .filter { it.appId == appId }
            .forEach { put(it.fieldId, it.value) }
    }

    private fun reduceDraftChanged(event: ClientEvent.AppDesignDraftChanged) {
        val inFlight = draftEditInFlight
        val acknowledged = inFlight != null &&
            inFlight.edit.appId == event.appId &&
            event.revision > inFlight.expectedRevision &&
            event.fields[inFlight.edit.fieldId] == inFlight.edit.bindingValue
        if (acknowledged) draftEditInFlight = null
        val pendingValues = pendingDraftValues(event.appId)
        _uiState.update { state ->
            if (state.designer?.appId != event.appId) state else state.copy(
                designer = state.designer.copy(
                    revision = event.revision,
                    values = event.fields.mapValues { it.value.toUiValue() } + pendingValues,
                    conflictRevision = null,
                ),
            )
        }
        if (acknowledged || draftEditInFlight == null) pumpDraftEditQueue()
    }

    private fun reduceDraftConflict(event: ClientEvent.AppDesignConflict) {
        val inFlight = draftEditInFlight
        if (inFlight?.edit?.appId == event.appId) {
            draftEditInFlight = null
            // A newer local value for this field supersedes the rejected one.
            if (draftEditQueue.none { it.appId == event.appId && it.fieldId == inFlight.edit.fieldId }) {
                draftEditQueue.add(0, inFlight.edit.copy(ready = true))
            }
        }
        draftConflictRefreshAppId = event.appId
        _uiState.update { state ->
            if (state.designer?.appId != event.appId) state else state.copy(
                designer = state.designer.copy(conflictRevision = event.actualRevision),
            )
        }
        submit(ClientCommand.GetAppDetails(event.appId))
    }

    private fun requestSuggestion() {
        val designer = _uiState.value.designer ?: return
        submit(ClientCommand.RequestAppDesignSuggestion(designer.appId, designer.revision, null))
    }

    private fun applySuggestion() {
        val designer = _uiState.value.designer ?: return
        val suggestion = designer.suggestion ?: return
        submit(ClientCommand.ApplyAgentDesignSuggestion(designer.appId, suggestion.id, designer.revision))
    }

    private fun dismissSuggestion() {
        val designer = _uiState.value.designer ?: return
        designer.suggestion?.let {
            submit(ClientCommand.DismissAppDesignSuggestion(designer.appId, it.id))
        }
        _uiState.update { it.copy(designer = it.designer?.copy(suggestion = null)) }
    }

    /**
     * The last designer step's required answer is a debounced text field, so a
     * confirm that beats the 400 ms timer would freeze a spec missing that
     * answer — and the late patch would then be rejected against `generating`.
     */
    private fun confirmDesign() {
        val appId = _uiState.value.designer?.appId ?: return
        viewModelScope.launch {
            if (!drainDraftEdits(appId)) {
                // Commands are submitted in order, so a confirm sent behind a
                // still-unacked patch reaches the engine at revision R while
                // the patch has already moved it to R+1: a guaranteed
                // RevisionConflict. Keep the user on the designer with their
                // answer intact instead.
                error("设计尚未保存完成，请重试")
                return@launch
            }
            val designer = _uiState.value.designer ?: return@launch
            val interaction = designer.interactionId ?: run {
                openDesigner(designer.appId)
                return@launch
            }
            submit(ClientCommand.ConfirmAppDesign(designer.appId, designer.revision, interaction))
        }
    }

    /**
     * Promote every debounced edit for [appId] and wait for the queue to empty.
     *
     * @return false when the 5 s budget expired with a patch still outstanding.
     */
    private suspend fun drainDraftEdits(appId: String): Boolean {
        val prefix = "$appId:"
        textEditJobs.keys.filter { it.startsWith(prefix) }
            .forEach { textEditJobs.remove(it)?.cancel() }
        draftEditQueue.forEachIndexed { index, edit ->
            if (edit.appId == appId && !edit.ready) {
                draftEditQueue[index] = edit.copy(ready = true)
            }
        }
        pumpDraftEditQueue()
        repeat(100) {
            if (drained(appId)) return true
            delay(50)
        }
        return drained(appId)
    }

    private fun drained(appId: String): Boolean =
        draftEditInFlight == null && draftEditQueue.none { it.appId == appId }

    private fun retryGeneration(appId: String) {
        submit(ClientCommand.RetryAppGeneration(appId))
        _uiState.update { it.copy(destination = LocalAppsDestination.Preview(appId)) }
    }

    private fun approvePreview(appId: String) {
        val preview = _uiState.value.previews[appId] ?: return
        submit(ClientCommand.ConfirmAppPreview(appId, preview.revision, preview.interactionId))
    }

    private fun executeBridgeRequest(message: LocalAppBridgeMessage) {
        if (message.appId.isBlank() || message.requestId.isBlank()) return
        val operation = when (message.operation) {
            "query_data" -> AppBridgeOperationDto.QUERY_DATA
            "mutate_data" -> AppBridgeOperationDto.MUTATE_DATA
            "network_request" -> AppBridgeOperationDto.NETWORK_REQUEST
            "runtime_status" -> AppBridgeOperationDto.RUNTIME_STATUS
            else -> {
                error("应用请求了不支持的 Bridge 操作：${message.operation}")
                return
            }
        }
        submit(
            ClientCommand.ExecuteAppBridgeRequest(
                AppBridgeRequestDto(
                    requestId = message.requestId,
                    appId = message.appId,
                    operation = operation,
                    payloadJson = message.payloadJson,
                ),
            ),
        )
    }

    /**
     * A page can raise a second prompt before the first is answered — two
     * `fetch()` calls to two unauthorized declared domains in one tick produce
     * two `AppCapabilityRequested` events. Overwriting the head would strand
     * the first request until the engine's 5-minute approval timeout with the
     * page's fetch stalled for that whole window, so queue strictly FIFO: the
     * request the page issued first is answered first.
     */
    private fun enqueueAuthorization(request: LocalAppAuthorizationRequest) {
        if (_uiState.value.pendingAuthorization == null) {
            _uiState.update { it.copy(pendingAuthorization = request) }
            return
        }
        if (queuedAuthorizations.size >= MAX_QUEUED_AUTHORIZATIONS) {
            // Drop the newest, not the oldest: the oldest already has a page
            // awaiting it. This is pathological-only (8 unanswered prompts).
            error("应用的授权请求过多，已忽略最新一条")
            return
        }
        queuedAuthorizations.addLast(request)
    }

    private fun resolveAuthorization(decision: LocalAppAuthorizationDecision) {
        val request = _uiState.value.pendingAuthorization ?: return
        val bindingDecision = decision.toBindingDecision()
        if (request.isUiControl) {
            if (decision == LocalAppAuthorizationDecision.Deny || request.uiAction == null) {
                submit(
                    ClientCommand.ResolveAppUiRequest(
                        request.requestId,
                        bindingDecision,
                        null,
                        if (request.uiAction == null && decision != LocalAppAuthorizationDecision.Deny) {
                            "UI 请求缺少有效目标或参数"
                        } else null,
                    ),
                )
            } else if (decision == LocalAppAuthorizationDecision.AllowSession ||
                decision == LocalAppAuthorizationDecision.AllowAlways
            ) {
                uiControlGrants[request.appId] = decision
            }
        } else {
            val capability = pendingCapabilityKinds.remove(request.requestId)
            if (capability == AppCapabilityKindDto.UI_CONTROL && decision != LocalAppAuthorizationDecision.Deny) {
                uiControlGrants[request.appId] = decision
            }
            submit(ClientCommand.ResolveAppCapabilityRequest(request.requestId, bindingDecision))
        }
        // Popped outside `update`: that lambda can re-run under CAS contention,
        // and a second pop would silently drop a queued prompt.
        val nextAuthorization = queuedAuthorizations.removeFirstOrNull()
        _uiState.update {
            it.copy(
                pendingAuthorization = nextAuthorization,
                pendingUiAction = if (
                    request.isUiControl && decision != LocalAppAuthorizationDecision.Deny && request.uiAction != null
                ) {
                    LocalAppPendingUiAction(request.requestId, request.appId, request.uiAction, decision)
                } else it.pendingUiAction,
                selectedAppId = if (request.isUiControl) request.appId else it.selectedAppId,
                destination = if (request.isUiControl && decision != LocalAppAuthorizationDecision.Deny) {
                    LocalAppsDestination.Details(request.appId, LocalAppDetailsTab.Preview)
                } else it.destination,
            )
        }
    }

    private fun resolveCompletedUiAction(action: LocalAppsAction.UiActionHandled) {
        val pending = _uiState.value.pendingUiAction
        if (pending?.requestId != action.requestId) return
        submit(
            ClientCommand.ResolveAppUiRequest(
                requestId = action.requestId,
                decision = pending.decision.toBindingDecision(),
                resultJson = action.resultJson,
                error = action.error,
            ),
        )
        _uiState.update { state -> state.copy(pendingUiAction = null) }
    }

    private fun navigateBack() {
        _uiState.update { state ->
            when (state.destination) {
                LocalAppsDestination.Library -> state
                LocalAppsDestination.Templates -> state.copy(destination = LocalAppsDestination.Library)
                is LocalAppsDestination.Designer,
                is LocalAppsDestination.Preview,
                is LocalAppsDestination.Details -> state.copy(destination = LocalAppsDestination.Library)
            }
        }
    }

    private fun reduce(event: ClientEvent) {
        when (event) {
            is ClientEvent.AppsChanged -> reduceApps(event)
            is ClientEvent.AppEvent -> reduceAppEvent(event.event)
            is ClientEvent.AppDesignerRequested -> reduceDesignerRequested(event)
            is ClientEvent.AppDesignDraftChanged -> reduceDraftChanged(event)
            is ClientEvent.AppDesignSuggestionAvailable -> reduceSuggestion(event)
            is ClientEvent.AppDesignConflict -> reduceDraftConflict(event)
            is ClientEvent.AppWorkflowChanged -> updateApp(event.appId) {
                it.copy(workflow = event.state.toUiWorkflow())
            }
            is ClientEvent.AppGenerationProgress -> _uiState.update { state ->
                state.copy(
                    generation = state.generation + (
                        event.appId to LocalAppGeneration(
                            state = event.stage,
                            percent = event.percent?.toInt(),
                            detail = event.detail,
                        )
                    ),
                    destination = if (state.selectedAppId == event.appId) LocalAppsDestination.Preview(event.appId) else state.destination,
                )
            }
            is ClientEvent.AppRuntimeChanged -> {
                if (event.state == AppRuntimeStateDto.RUNNING) {
                    runtimeLastUsedAt[event.appId] = System.currentTimeMillis()
                }
                var runtime: LocalAppRuntime? = null
                updateApp(event.appId) { app ->
                    (event.details?.toUiRuntime() ?: app.runtime.copy(state = event.state.toUiRuntime()))
                        .copy(detail = event.details?.lastError ?: event.lastError)
                        .also { runtime = it }
                        .let { app.copy(runtime = it) }
                }
                runtime?.let { latest ->
                    _uiState.update { state ->
                        val details = state.details[event.appId] ?: return@update state
                        state.copy(details = state.details + (event.appId to details.copy(runtime = latest)))
                    }
                }
            }
            is ClientEvent.AppPreviewReady -> _uiState.update { state ->
                state.copy(
                    previews = state.previews + (event.appId to LocalAppPreview(event.appId, event.revision, event.interactionId, event.url)),
                    selectedAppId = event.appId,
                    destination = LocalAppsDestination.Preview(event.appId),
                )
            }
            is ClientEvent.AppOperationFailed -> {
                val recoveringConflict = event.code == AppErrorCodeDto.REVISION_CONFLICT &&
                    event.appId == draftConflictRefreshAppId
                // A rejected patch must not wedge the single-slot draft queue:
                // `draftEditInFlight` is otherwise only cleared by an ack or a
                // conflict, so a patch the engine refuses outright leaves
                // `pumpDraftEditQueue`'s guard true for the rest of the process
                // and every later design edit is queued and never sent.
                // These two codes are the ones a rejected patch produces —
                // validate_patch / validate_design_value produce InvalidRequest
                // and ensure_workflow produces WorkflowStateInvalid.
                //
                // They are NOT the only codes `update_draft` can produce: it runs
                // inside `AppService::with_app`, which also yields NotFound,
                // Io (persist_mutation, and a JoinError lowered to Io) and
                // StorageCorrupt. Those cannot be handled here, because
                // `AppOperationFailed` carries only (appId, code, message) — an Io
                // from a failed persist and an Io from a failed runtime start are
                // the same event. Widening the set would drop a live patch on every
                // unrelated failure; the wedge they would otherwise cause is bounded
                // by the in-flight watchdog in `pumpDraftEditQueue` instead. iOS
                // resolves this identically (LocalAppsStore.swift).
                val rejectedDraftPatch = event.appId != null &&
                    event.appId == draftEditInFlight?.edit?.appId &&
                    (
                        event.code == AppErrorCodeDto.INVALID_REQUEST ||
                            event.code == AppErrorCodeDto.WORKFLOW_STATE_INVALID
                        )
                if (rejectedDraftPatch) {
                    draftEditInFlight = null
                    pumpDraftEditQueue()
                }
                if (!recoveringConflict) error(event.message)
            }
            else -> Unit
        }
    }

    private fun reduceAppEvent(event: AppEventDto) {
        when (event) {
            is AppEventDto.AppTemplatesChanged -> {
                val templates = event.templates.map(AppTemplateDto::toUiTemplate).sortedBy { it.name }
                val names = templates.associate { it.kind to it.name }
                _uiState.update { state ->
                    state.copy(
                        templates = templates,
                        templatesLoading = false,
                        apps = state.apps.map { app -> app.copy(templateName = names[app.templateKind] ?: app.templateName) },
                    )
                }
            }
            is AppEventDto.AppDetailsChanged -> reduceDetails(event.details)
            is AppEventDto.AppGenerationJobChanged -> reduceGenerationJob(event.job)
            is AppEventDto.AppBridgeResponse -> {
                val response = event.response
                val payload = response.resultJson ?: response.error?.let(JSONObject::quote)
                _uiState.update { state ->
                    state.copy(
                        bridgeResults = state.bridgeResults + (
                            response.requestId to LocalAppBridgeResult(
                                requestId = response.requestId,
                                appId = response.appId,
                                ok = response.ok,
                                payloadJson = payload,
                                error = response.error,
                            )
                        ),
                    )
                }
            }
            is AppEventDto.AppUiRequest -> {
                val request = event.request
                val action = request.toUiAutomationAction()
                val capabilityDecision = uiControlGrants[request.appId]
                if (capabilityDecision == LocalAppAuthorizationDecision.AllowOnce) {
                    uiControlGrants.remove(request.appId)
                }
                val executionDecision = capabilityDecision
                    ?: LocalAppAuthorizationDecision.AllowOnce.takeIf { request.action == AppUiActionKindDto.INSPECT }
                if (executionDecision != null && action != null) {
                    _uiState.update {
                        it.copy(
                            selectedAppId = request.appId,
                            destination = LocalAppsDestination.Details(request.appId, LocalAppDetailsTab.Preview),
                            pendingUiAction = LocalAppPendingUiAction(
                                requestId = request.requestId,
                                appId = request.appId,
                                action = action,
                                decision = executionDecision,
                            ),
                        )
                    }
                } else if (executionDecision != null) {
                    submit(
                        ClientCommand.ResolveAppUiRequest(
                            requestId = request.requestId,
                            decision = executionDecision.toBindingDecision(),
                            resultJson = null,
                            error = "UI 请求缺少有效目标或参数",
                        ),
                    )
                } else {
                    enqueueAuthorization(
                        LocalAppAuthorizationRequest(
                            requestId = request.requestId,
                            appId = request.appId,
                            title = "允许 Agent 控制应用界面？",
                            reason = "Agent 请求执行 ${request.action.name.lowercase()} 操作。",
                            isUiControl = true,
                            uiAction = action,
                        ),
                    )
                }
            }
            is AppEventDto.AppCapabilityRequested -> {
                val request = event.request
                pendingCapabilityKinds[request.requestId] = request.capability
                enqueueAuthorization(
                    LocalAppAuthorizationRequest(
                        requestId = request.requestId,
                        appId = request.appId,
                        title = request.capability.authorizationTitle(),
                        reason = buildString {
                            append(request.reason)
                            request.domain?.let { append("\n域名：").append(it) }
                        },
                        isUiControl = false,
                    ),
                )
            }
            is AppEventDto.AppCheckpointsChanged -> _uiState.update { state ->
                val details = state.details[event.appId] ?: return@update state
                state.copy(
                    details = state.details + (
                        event.appId to details.copy(
                            checkpoints = event.checkpoints.map { checkpoint ->
                                LocalAppCheckpoint(
                                    id = checkpoint.id,
                                    label = checkpoint.label,
                                    kind = checkpoint.kind.name.lowercase(),
                                    createdAtMs = checkpoint.createdAtMs.toLong(),
                                )
                            },
                        )
                    ),
                )
            }
        }
    }

    private fun reduceDetails(details: com.lingxi.code.bindings.AppDetailsDto) {
        val state = _uiState.value
        val templateNames = state.templates.associate { it.kind to it.name }
        val prior = state.apps.firstOrNull { it.id == details.app.id }
        val app = details.app.toUiApp(
            templateNames = templateNames,
            runtime = details.runtime.toUiRuntime(),
            fallbackRuntime = prior?.runtime,
        )
        val pendingValues = pendingDraftValues(app.id)
        _uiState.update { current ->
            val apps = if (current.apps.any { it.id == app.id }) {
                current.apps.map { if (it.id == app.id) app else it }
            } else {
                current.apps + app
            }
            val currentDesigner = current.designer
            val template = current.templates.firstOrNull { it.kind == app.templateKind }
            current.copy(
                apps = apps.sortedByDescending { it.updatedAtMs },
                designer = if (currentDesigner?.appId == app.id && template != null) {
                    currentDesigner.copy(
                        template = template,
                        revision = details.designRevision,
                        values = details.designFields.associate { it.fieldId to it.value.toUiValue() } + pendingValues,
                        conflictRevision = null,
                    )
                } else currentDesigner,
                generation = details.generationJob?.let { job ->
                    current.generation + (app.id to job.toUiGeneration())
                } ?: current.generation,
                details = current.details + (
                    app.id to LocalAppDetails(
                        appId = app.id,
                        workspaceRelativePath = details.app.workspaceRel,
                        collections = details.manifest?.collections.orEmpty().map { collection ->
                            LocalAppCollectionSchema(
                                id = collection.id,
                                label = collection.label,
                                fields = collection.fields.map { it.toUiDataField() },
                                enabledByDefault = collection.enabledByDefault,
                            )
                        },
                        allowedDomains = details.manifest?.allowedDomains.orEmpty(),
                        checkpoints = details.checkpoints.map { checkpoint ->
                            LocalAppCheckpoint(
                                id = checkpoint.id,
                                label = checkpoint.label,
                                kind = checkpoint.kind.name.lowercase(),
                                createdAtMs = checkpoint.createdAtMs.toLong(),
                            )
                        },
                        runtime = details.runtime.toUiRuntime(),
                    )
                ),
            )
        }
        if (draftConflictRefreshAppId == app.id) {
            draftConflictRefreshAppId = null
            pumpDraftEditQueue()
        }
    }

    private fun reduceGenerationJob(job: AppGenerationJobDto) {
        _uiState.update { state ->
            state.copy(
                generation = state.generation + (job.appId to job.toUiGeneration()),
                destination = if (state.selectedAppId == job.appId) LocalAppsDestination.Preview(job.appId) else state.destination,
            )
        }
    }

    private fun reduceApps(event: ClientEvent.AppsChanged) {
        val oldIds = _uiState.value.apps.mapTo(hashSetOf()) { it.id }
        val templates = _uiState.value.templates.associateBy { it.kind }
        val apps = event.apps.map { record ->
            val prior = _uiState.value.apps.firstOrNull { it.id == record.id }
            record.toUiApp(templates.mapValues { it.value.name }, fallbackRuntime = prior?.runtime)
        }.sortedByDescending { it.updatedAtMs }
        val liveIds = apps.mapTo(hashSetOf()) { it.id }
        _uiState.update {
            it.copy(
                apps = apps,
                details = it.details.filterKeys(liveIds::contains),
                generation = it.generation.filterKeys(liveIds::contains),
                previews = it.previews.filterKeys(liveIds::contains),
                loading = false,
            )
        }

        val pending = pendingCreate ?: return
        val created = apps.firstOrNull { it.id !in oldIds && it.name == pending.first && it.templateKind == pending.second } ?: return
        pendingCreate = null
        openDesigner(created.id)
    }

    private fun reduceDesignerRequested(event: ClientEvent.AppDesignerRequested) {
        val state = _uiState.value
        val app = state.apps.firstOrNull { it.id == event.appId } ?: return
        val template = state.templates.firstOrNull { it.kind == app.templateKind } ?: return
        _uiState.update {
            it.copy(
                selectedAppId = app.id,
                destination = LocalAppsDestination.Designer(app.id),
                designer = LocalAppDesigner(
                    appId = app.id,
                    appName = app.name,
                    template = template,
                    revision = event.revision,
                    interactionId = event.interactionId,
                    values = template.steps.flatMap { step -> step.fields }
                        .mapNotNull { field -> field.defaultValue?.let { field.id to it } }
                        .toMap(),
                ),
            )
        }
    }

    private fun reduceSuggestion(event: ClientEvent.AppDesignSuggestionAvailable) {
        _uiState.update { state ->
            val designer = state.designer ?: return@update state
            if (designer.appId != event.appId) return@update state
            val fields = designer.template.steps.flatMap { it.fields }.associateBy { it.id }
            val changes = event.patch.ops.mapNotNull { op ->
                val set = op as? AppDesignPatchOpDto.Set ?: return@mapNotNull null
                val after = set.value.toUiValue()
                LocalAppSuggestedChange(
                    fieldId = set.fieldId,
                    label = fields[set.fieldId]?.label ?: set.fieldId,
                    before = designer.values[set.fieldId]?.readable().orEmpty(),
                    after = after.readable(),
                )
            }
            state.copy(
                designer = designer.copy(
                    suggestion = LocalAppSuggestion(
                        id = event.suggestionId,
                        basedOnRevision = event.basedOnRevision,
                        summary = event.patch.note ?: "建议调整 ${changes.size} 个字段",
                        changes = changes,
                    ),
                ),
            )
        }
    }

    private fun updateApp(appId: String, transform: (LocalAppItem) -> LocalAppItem) {
        _uiState.update { state ->
            state.copy(apps = state.apps.map { if (it.id == appId) transform(it) else it })
        }
    }

    private fun submit(command: ClientCommand) {
        submit { it.submitClientCommand(command) }
    }

    private fun submit(block: suspend (ConversationSource) -> Unit) {
        val bound = source ?: return
        viewModelScope.launch {
            runCatching { block(bound) }.onFailure { error(it.message ?: it::class.simpleName.orEmpty()) }
        }
    }

    private fun error(message: String) {
        _uiState.update { it.copy(loading = false, error = message) }
    }

    companion object {
        private const val MAX_QUEUED_AUTHORIZATIONS = 8

        fun factory(sourceFlow: StateFlow<ConversationSource>): ViewModelProvider.Factory =
            object : ViewModelProvider.Factory {
                @Suppress("UNCHECKED_CAST")
                override fun <T : ViewModel> create(modelClass: Class<T>): T =
                    LocalAppsViewModel(sourceFlow) as T
            }
    }
}

private fun AppTemplateKindDto.toUiTemplateKind(): String = when (this) {
    AppTemplateKindDto.DASHBOARD -> "dashboard"
    AppTemplateKindDto.CRUD_TRACKER -> "crud_tracker"
    AppTemplateKindDto.CONTENT_SHOWCASE -> "content_showcase"
    AppTemplateKindDto.FORM_UTILITY -> "form_utility"
}

private fun String.toBindingTemplateKind(): AppTemplateKindDto = when (this) {
    "dashboard" -> AppTemplateKindDto.DASHBOARD
    "crud_tracker" -> AppTemplateKindDto.CRUD_TRACKER
    "content_showcase" -> AppTemplateKindDto.CONTENT_SHOWCASE
    "form_utility" -> AppTemplateKindDto.FORM_UTILITY
    else -> error("Unsupported template kind: $this")
}

private fun AppWorkflowStateDto.toUiWorkflow(): LocalAppWorkflow = when (this) {
    AppWorkflowStateDto.COLLECTING_SPEC -> LocalAppWorkflow.CollectingSpec
    AppWorkflowStateDto.AWAITING_SPEC_CONFIRMATION -> LocalAppWorkflow.AwaitingSpecConfirmation
    AppWorkflowStateDto.GENERATING -> LocalAppWorkflow.Generating
    AppWorkflowStateDto.VALIDATING -> LocalAppWorkflow.Validating
    AppWorkflowStateDto.AWAITING_PREVIEW_CONFIRMATION -> LocalAppWorkflow.AwaitingPreviewConfirmation
    AppWorkflowStateDto.REVISING -> LocalAppWorkflow.Revising
    AppWorkflowStateDto.READY -> LocalAppWorkflow.Ready
    AppWorkflowStateDto.GENERATION_FAILED -> LocalAppWorkflow.GenerationFailed
    AppWorkflowStateDto.VALIDATION_FAILED -> LocalAppWorkflow.ValidationFailed
}

private fun AppRuntimeStateDto.toUiRuntime(): LocalAppRuntimeState = when (this) {
    AppRuntimeStateDto.STOPPED -> LocalAppRuntimeState.Stopped
    AppRuntimeStateDto.STARTING -> LocalAppRuntimeState.Starting
    AppRuntimeStateDto.RUNNING -> LocalAppRuntimeState.Running
    AppRuntimeStateDto.STOPPING -> LocalAppRuntimeState.Stopping
    AppRuntimeStateDto.FAILED -> LocalAppRuntimeState.Failed
}

private fun AppRuntimeDetailsDto.toUiRuntime(): LocalAppRuntime = LocalAppRuntime(
    state = state.toUiRuntime(),
    mode = when (mode) {
        AppRuntimeModeDto.STATIC_EXPORT -> LocalAppRuntimeMode.StaticExport
        AppRuntimeModeDto.NEXT_PRODUCTION -> LocalAppRuntimeMode.NextProduction
        null -> null
    },
    url = loopbackUrl,
    detail = lastError ?: suspensionReason?.name?.lowercase(),
    recovery = recoveryState?.name?.lowercase(),
)

private fun AppRecordDto.toUiApp(
    templateNames: Map<String, String>,
    runtime: LocalAppRuntime? = null,
    fallbackRuntime: LocalAppRuntime? = null,
): LocalAppItem {
    val kind = template.toUiTemplateKind()
    return LocalAppItem(
        id = id,
        name = name,
        templateKind = kind,
        templateName = templateNames[kind] ?: kind,
        workflow = workflowState.toUiWorkflow(),
        runtime = runtime ?: fallbackRuntime ?: LocalAppRuntime(),
        updatedAtMs = updatedAtMs.toLong(),
    )
}

private fun AppTemplateDto.toUiTemplate(): LocalAppTemplate = LocalAppTemplate(
    kind = kind.toUiTemplateKind(),
    version = version,
    name = name,
    description = description,
    steps = steps.map { step ->
        LocalAppDesignStep(
            id = step.id,
            order = step.order,
            title = step.title,
            description = step.description,
            fields = step.fields.map { field ->
                LocalAppDesignField(
                    id = field.id,
                    label = field.label,
                    description = field.description,
                    kind = field.fieldType.toUiFieldKind(),
                    required = field.required,
                    defaultValue = field.defaultValue?.toUiValue(),
                    options = field.options.map { LocalAppFieldOption(it.value, it.label) },
                )
            },
        )
    },
)

private fun AppDesignFieldTypeDto.toUiFieldKind(): LocalAppFieldKind = when (this) {
    AppDesignFieldTypeDto.SHORT_TEXT -> LocalAppFieldKind.ShortText
    AppDesignFieldTypeDto.LONG_TEXT -> LocalAppFieldKind.LongText
    AppDesignFieldTypeDto.SINGLE_CHOICE -> LocalAppFieldKind.SingleChoice
    AppDesignFieldTypeDto.MULTIPLE_CHOICE -> LocalAppFieldKind.MultipleChoice
    AppDesignFieldTypeDto.BOOLEAN -> LocalAppFieldKind.Boolean
    AppDesignFieldTypeDto.COLOR -> LocalAppFieldKind.Color
    AppDesignFieldTypeDto.DENSITY -> LocalAppFieldKind.Density
    AppDesignFieldTypeDto.SCREEN_LIST -> LocalAppFieldKind.ScreenList
    AppDesignFieldTypeDto.FEATURE_LIST -> LocalAppFieldKind.FeatureList
    AppDesignFieldTypeDto.DATA_FIELD_LIST -> LocalAppFieldKind.DataFieldList
    AppDesignFieldTypeDto.DOMAIN_LIST -> LocalAppFieldKind.DomainList
}

private fun AppGenerationJobDto.toUiGeneration(): LocalAppGeneration = LocalAppGeneration(
    jobId = id,
    state = state.name.lowercase().replace('_', ' '),
    percent = percent?.toInt(),
    detail = detail,
)

private fun AppCapabilityKindDto.authorizationTitle(): String = when (this) {
    AppCapabilityKindDto.DATA_MUTATION -> "允许修改应用数据？"
    AppCapabilityKindDto.UI_CONTROL -> "允许 Agent 控制应用界面？"
    AppCapabilityKindDto.NETWORK_DOMAIN -> "允许应用联网？"
    AppCapabilityKindDto.RESTORE_CHECKPOINT -> "允许恢复代码检查点？"
}

private fun LocalAppAuthorizationDecision.toBindingDecision(): AppAuthorizationDecisionDto = when (this) {
    LocalAppAuthorizationDecision.Deny -> AppAuthorizationDecisionDto.DENY
    LocalAppAuthorizationDecision.AllowOnce -> AppAuthorizationDecisionDto.ALLOW_ONCE
    LocalAppAuthorizationDecision.AllowSession -> AppAuthorizationDecisionDto.ALLOW_SESSION
    LocalAppAuthorizationDecision.AllowAlways -> AppAuthorizationDecisionDto.ALLOW_ALWAYS
}

private fun AppUiRequestDto.toUiAutomationAction(): LocalAppUiAutomationAction? {
    val uiTarget = target?.toUiTarget()
    return when (action) {
        AppUiActionKindDto.INSPECT -> LocalAppUiAutomationAction.Inspect
        AppUiActionKindDto.CLICK -> uiTarget?.let(LocalAppUiAutomationAction::Click)
        AppUiActionKindDto.FILL -> uiTarget?.let { LocalAppUiAutomationAction.Fill(it, value.orEmpty()) }
        AppUiActionKindDto.SELECT -> uiTarget?.let { LocalAppUiAutomationAction.Select(it, value.orEmpty()) }
        AppUiActionKindDto.TOGGLE -> uiTarget?.let { LocalAppUiAutomationAction.Toggle(it, value.toBoolean()) }
        AppUiActionKindDto.SCROLL -> value.orEmpty().split(',', limit = 2).let { parts ->
            LocalAppUiAutomationAction.Scroll(
                x = parts.getOrNull(0)?.trim()?.toIntOrNull() ?: 0,
                y = parts.getOrNull(1)?.trim()?.toIntOrNull() ?: 0,
            )
        }
        // `inspect` hands the agent back absolute loopback urls, and the tool
        // schema constrains `value` no further. Origin is enforced in the
        // WebView, the only layer that knows the live one.
        AppUiActionKindDto.NAVIGATE ->
            value?.takeIf { it.startsWith('/') || it.contains("://") }
                ?.let(LocalAppUiAutomationAction::Navigate)
        AppUiActionKindDto.BACK -> LocalAppUiAutomationAction.Back
        AppUiActionKindDto.RELOAD -> LocalAppUiAutomationAction.Reload
    }
}

private fun com.lingxi.code.bindings.AppUiTargetDto.toUiTarget(): LocalAppUiTarget? {
    val elementId = elementId?.trim().takeUnless { it.isNullOrEmpty() }
    val role = role?.trim().takeUnless { it.isNullOrEmpty() }
    val name = name?.trim().takeUnless { it.isNullOrEmpty() }
    return if (elementId != null || role != null || name != null) {
        LocalAppUiTarget(elementId = elementId, role = role, name = name)
    } else {
        null
    }
}

private fun LocalAppDesignValue.toBindingValue(kind: LocalAppFieldKind): DesignValueDto = when (this) {
    is LocalAppDesignValue.Text -> when (kind) {
        LocalAppFieldKind.LongText -> DesignValueDto.LongText(value)
        LocalAppFieldKind.Color -> DesignValueDto.Color(value)
        else -> DesignValueDto.ShortText(value)
    }
    is LocalAppDesignValue.Choice -> DesignValueDto.SingleChoice(value)
    is LocalAppDesignValue.Choices -> DesignValueDto.MultipleChoice(values)
    is LocalAppDesignValue.Toggle -> DesignValueDto.Boolean(value)
    is LocalAppDesignValue.Density -> DesignValueDto.Density(if (compact) DensityLevelDto.COMPACT else DensityLevelDto.COMFORTABLE)
    is LocalAppDesignValue.StringList -> when (kind) {
        LocalAppFieldKind.ScreenList -> DesignValueDto.ScreenList(values)
        LocalAppFieldKind.DomainList -> DesignValueDto.DomainList(values)
        else -> DesignValueDto.FeatureList(values)
    }
    is LocalAppDesignValue.DataFields -> DesignValueDto.DataFieldList(values.map { it.toBindingDataField() })
}

private fun DesignValueDto.toUiValue(): LocalAppDesignValue = when (this) {
    is DesignValueDto.ShortText -> LocalAppDesignValue.Text(value)
    is DesignValueDto.LongText -> LocalAppDesignValue.Text(value)
    is DesignValueDto.SingleChoice -> LocalAppDesignValue.Choice(value)
    is DesignValueDto.MultipleChoice -> LocalAppDesignValue.Choices(value)
    is DesignValueDto.Boolean -> LocalAppDesignValue.Toggle(value)
    is DesignValueDto.Color -> LocalAppDesignValue.Text(value)
    is DesignValueDto.Density -> LocalAppDesignValue.Density(value == DensityLevelDto.COMPACT)
    is DesignValueDto.ScreenList -> LocalAppDesignValue.StringList(value)
    is DesignValueDto.FeatureList -> LocalAppDesignValue.StringList(value)
    is DesignValueDto.DataFieldList -> LocalAppDesignValue.DataFields(value.map { it.toUiDataField() })
    is DesignValueDto.DomainList -> LocalAppDesignValue.StringList(value)
}

private fun LocalAppDataField.toBindingDataField(): AppDataFieldDto = AppDataFieldDto(
    id = id,
    label = label,
    fieldType = type.toBindingDataFieldType(),
    required = required,
    options = options,
)

private fun AppDataFieldDto.toUiDataField(): LocalAppDataField = LocalAppDataField(
    id = id,
    label = label,
    type = fieldType.toUiDataFieldType(),
    required = required,
    options = options,
)

private fun LocalAppDataFieldType.toBindingDataFieldType(): AppDataFieldTypeDto =
    AppDataFieldTypeDto.valueOf(name.uppercase().replace("LONGTEXT", "LONG_TEXT").replace("DATETIME", "DATE_TIME").replace("IMAGEREF", "IMAGE_REF"))

private fun AppDataFieldTypeDto.toUiDataFieldType(): LocalAppDataFieldType = when (this) {
    AppDataFieldTypeDto.TEXT -> LocalAppDataFieldType.Text
    AppDataFieldTypeDto.LONG_TEXT -> LocalAppDataFieldType.LongText
    AppDataFieldTypeDto.INTEGER -> LocalAppDataFieldType.Integer
    AppDataFieldTypeDto.DECIMAL -> LocalAppDataFieldType.Decimal
    AppDataFieldTypeDto.BOOLEAN -> LocalAppDataFieldType.Boolean
    AppDataFieldTypeDto.DATE_TIME -> LocalAppDataFieldType.DateTime
    AppDataFieldTypeDto.ENUM -> LocalAppDataFieldType.Enum
    AppDataFieldTypeDto.IMAGE_REF -> LocalAppDataFieldType.ImageRef
}
