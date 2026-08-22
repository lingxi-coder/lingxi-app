package com.lingxi.code.localapps

import androidx.lifecycle.ViewModel
import androidx.lifecycle.ViewModelProvider
import androidx.lifecycle.viewModelScope
import com.lingxi.code.BuildConfig
import com.lingxi.code.R
import com.lingxi.code.bindings.AppAuthorizationDecisionDto
import com.lingxi.code.bindings.AppBridgeOperationDto
import com.lingxi.code.bindings.AppBridgeRequestDto
import com.lingxi.code.bindings.AppCapabilityKindDto
import com.lingxi.code.bindings.AppCreateOriginDto
import com.lingxi.code.bindings.AppDataFieldDto
import com.lingxi.code.bindings.AppDataFieldTypeDto
import com.lingxi.code.bindings.AppEventDto
import com.lingxi.code.bindings.AppRecordDto
import com.lingxi.code.bindings.AppRuntimeDetailsDto
import com.lingxi.code.bindings.AppRuntimeModeDto
import com.lingxi.code.bindings.AppRuntimeStateDto
import com.lingxi.code.bindings.AppSessionKindDto
import com.lingxi.code.bindings.AppSessionRowDto
import com.lingxi.code.bindings.AppUiActionKindDto
import com.lingxi.code.bindings.AppUiRequestDto
import com.lingxi.code.bindings.AppWorkflowStateDto
import com.lingxi.code.bindings.ClientCommand
import com.lingxi.code.bindings.ClientEvent
import com.lingxi.code.conversation.ConversationSource
import com.lingxi.code.localapps.widget.LocalAppWidgetSnapshotSync
import com.lingxi.code.localapps.widget.NoopLocalAppWidgetSnapshotSync
import com.lingxi.code.model.DefaultSessionCatalogStrings
import com.lingxi.code.model.EngineModelCatalog
import com.lingxi.code.model.SessionCatalog
import com.lingxi.code.model.SessionCatalogStrings
import kotlinx.coroutines.CoroutineStart
import kotlinx.coroutines.coroutineScope
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.collectLatest
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.flow.receiveAsFlow
import org.json.JSONObject

/**
 * Local apps are agent-driven (protocol v3): the record set collapsed to
 * `draft`/`ready`, and every design/plan/generation-pipeline command and event
 * left the wire. What remains here is the library (list/create/delete), the
 * runtime (start/stop + WebView bridge + capability gates), the checkpoint
 * history, and the NEW per-app session catalog (`ListAppSessions` /
 * `AppSessionsChanged`) that turns each app into a conversation scope.
 */
class LocalAppsViewModel(
    private val sourceFlow: StateFlow<ConversationSource>,
    distributionChannel: String = BuildConfig.DISTRIBUTION_CHANNEL,
    private val strings: LocalAppsStrings = DefaultLocalAppsStrings,
    private val sessionStrings: SessionCatalogStrings = DefaultSessionCatalogStrings,
    private val webStorageCleanup: LocalAppWebStorageCleanup = NoopLocalAppWebStorageCleanup,
    private val widgetSnapshotSync: LocalAppWidgetSnapshotSync = NoopLocalAppWidgetSnapshotSync,
    /** Injectable "now" so relative-time bucketing is deterministic in tests. */
    internal var nowEpochSeconds: () -> Long = { System.currentTimeMillis() / 1000L },
) : ViewModel() {
    internal val _uiState = MutableStateFlow(
        LocalAppsUiState(
            distributionMode = if (distributionChannel == "full") {
                LocalAppRuntimeMode.ViteStatic
            } else {
                LocalAppRuntimeMode.StaticExport
            },
        ),
    )
    val uiState: StateFlow<LocalAppsUiState> = _uiState.asStateFlow()

    private var source: ConversationSource? = null

    /**
     * The in-flight `CreateFromBrief` waiting for its `AppsChanged` claim.
     * Only one create may be armed at a time (same as iOS). The claim is
     * matched by brief among newly announced ids because `createFromBrief`
     * sends `name = ""` and `AppService::create_app` derives the display name.
     */
    private data class PendingCreate(
        val brief: String,
        val addWidget: Boolean,
    )

    private val pendingCreates = mutableListOf<PendingCreate>()
    private val widgetPinRequestChannel = Channel<String>(Channel.BUFFERED)
    val widgetPinRequests = widgetPinRequestChannel.receiveAsFlow()
    private val pendingCapabilityKinds = mutableMapOf<String, AppCapabilityKindDto>()
    private val queuedAuthorizations = ArrayDeque<LocalAppAuthorizationRequest>()
    private val uiControlGrants = mutableMapOf<String, LocalAppAuthorizationDecision>()
    private val runtimeLastUsedAt = mutableMapOf<String, Long>()
    private val runtimeStartsInFlight = mutableSetOf<String>()

    /**
     * The offset each app's most recent `ListAppSessions` asked for.
     * `AppSessionsChanged` carries no request correlator, so this is how the
     * reducer knows whether the reply REPLACES the cached rows (a first-page
     * request, or an unsolicited push) or APPENDS them (a 「加载更多」 page).
     */
    private val sessionRequestOffsets = mutableMapOf<String, ULong?>()

    init {
        webStorageCleanup.retryConfirmed()
        viewModelScope.launch {
            sourceFlow.collectLatest { bound ->
                source = bound
                _uiState.update { it.copy(loading = true, error = null) }
                coroutineScope {
                    launch(start = CoroutineStart.UNDISPATCHED) {
                        bound.clientEvents.collect(::reduce)
                    }
                    launch(start = CoroutineStart.UNDISPATCHED) {
                        bound.modelState.collect { engine ->
                            _uiState.update {
                                it.copy(
                                    workflowModels = EngineModelCatalog.options(engine.available),
                                    currentWorkflowModelId = engine.active.takeIf(String::isNotBlank),
                                )
                            }
                        }
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
            LocalAppsAction.Create -> _uiState.update { it.copy(createName = "") }
            is LocalAppsAction.Search -> _uiState.update { it.copy(query = action.query) }
            is LocalAppsAction.ChangeCreateName -> _uiState.update { it.copy(createName = action.name) }
            is LocalAppsAction.CreateFromBrief -> createFromBrief(
                brief = action.brief,
                gitEnabled = action.gitEnabled,
                workflowModel = action.workflowModel,
                addWidget = action.addWidget,
            )
            is LocalAppsAction.OpenApp -> openApp(action.appId)
            is LocalAppsAction.LoadAppSessions -> requestSessions(action.appId, action.offset)
            is LocalAppsAction.StartRuntime -> startRuntimeIfNeeded(action.appId)
            is LocalAppsAction.StopRuntime -> {
                runtimeStartsInFlight.remove(action.appId)
                submit(ClientCommand.StopApp(action.appId))
            }
            is LocalAppsAction.DeleteApp -> deleteApp(action.appId)
            is LocalAppsAction.ResetPermissions -> {
                uiControlGrants.remove(action.appId)
                submit(ClientCommand.ResetAppPermissions(action.appId))
            }
            is LocalAppsAction.RestoreCheckpoint -> submit(
                ClientCommand.RestoreAppCheckpoint(action.appId, action.checkpointId),
            )
            is LocalAppsAction.BridgeRequest -> executeBridgeRequest(action.message)
            is LocalAppsAction.AcknowledgeBridgeResult -> _uiState.update {
                it.copy(
                    bridgeResults = it.bridgeResults - LocalAppBridgeRequestKey(action.appId, action.requestId),
                )
            }
            is LocalAppsAction.ResolveAuthorization -> resolveAuthorization(action.decision)
            is LocalAppsAction.ResolveProfileProposal -> resolveProfileProposal(action.approved)
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
            .onFailure {
                error(
                    strings.resolve(
                        R.string.local_apps_error_load_apps,
                        "无法加载应用：%1\$s",
                        "${it.message ?: it::class.simpleName}",
                    ),
                )
            }
    }

    /**
     * `brief` is REQUIRED on the wire (it seeds the agent conversation the
     * engine starts for the new app) and `name` is sent EMPTY, every time:
     * `AppService::create_app` derives a display name from the brief whenever
     * the caller's name is blank, so there is no client-side name to collect
     * or fabricate (mirrors iOS's `LocalAppsStore.createApp(brief:)`).
     */
    private fun createFromBrief(
        brief: String,
        gitEnabled: Boolean,
        workflowModel: String?,
        addWidget: Boolean,
    ) {
        val trimmedBrief = brief.trim()
        if (trimmedBrief.isEmpty()) return
        val selectedModel = workflowModel?.trim()?.takeIf(String::isNotEmpty)
        if (selectedModel != null && _uiState.value.workflowModels.none { it.id == selectedModel }) {
            // An explicit selection must still belong to the engine's live catalog.
            // Do not silently turn a stale/forged id into "follow current".
            error(
                strings.resolve(
                    R.string.local_apps_error_workflow_model_unavailable,
                    "所选 Workflow 模型已不可用，请重新选择。",
                ),
            )
            return
        }
        if (pendingCreates.isNotEmpty()) {
            error(
                strings.resolve(
                    R.string.local_apps_error_create_in_progress,
                    "已有一个本地应用正在创建中，请稍候。",
                ),
            )
            return
        }
        val pending = PendingCreate(trimmedBrief, addWidget)
        pendingCreates += pending
        submit(
            ClientCommand.CreateApp(
                name = "",
                origin = AppCreateOriginDto.LIBRARY,
                brief = trimmedBrief,
                gitEnabled = gitEnabled,
                workflowModel = selectedModel,
                conversationId = null,
            ),
        ) { pendingCreates.remove(pending) }
    }

    /**
     * Every app opens onto its Details screen with the Sessions tab selected:
     * an app is a conversation scope, so its session catalog is the primary
     * surface for `draft` and `ready` alike. The details snapshot and the
     * first catalog page are requested together.
     */
    private fun openApp(appId: String) {
        if (_uiState.value.apps.none { it.id == appId }) return
        _uiState.update {
            it.copy(
                selectedAppId = appId,
                selectedDetailsTab = LocalAppDetailsTab.Sessions,
                destination = LocalAppsDestination.Details(appId, LocalAppDetailsTab.Sessions),
            )
        }
        submit(ClientCommand.GetAppDetails(appId))
        requestSessions(appId, offset = null)
    }

    fun openFromWidget(appId: String, autostart: Boolean) {
        val app = _uiState.value.apps.firstOrNull { it.id == appId }
        if (app == null) {
            _uiState.update {
                it.copy(
                    selectedAppId = null,
                    destination = LocalAppsDestination.Library,
                    error = strings.resolve(R.string.local_apps_not_found, "应用不存在"),
                )
            }
            return
        }
        submit(ClientCommand.GetAppDetails(appId))
        requestSessions(appId, offset = null)
        if (app.workflow != LocalAppWorkflow.Ready) {
            _uiState.update {
                it.copy(
                    selectedAppId = appId,
                    selectedDetailsTab = LocalAppDetailsTab.Sessions,
                    destination = LocalAppsDestination.Details(appId, LocalAppDetailsTab.Sessions),
                    error = strings.resolve(
                        R.string.local_apps_preview_not_ready,
                        "预览尚未准备好",
                    ),
                )
            }
            return
        }
        _uiState.update {
            it.copy(
                selectedAppId = appId,
                selectedDetailsTab = LocalAppDetailsTab.Preview,
                destination = LocalAppsDestination.Preview(appId),
            )
        }
        if (autostart) startRuntimeIfNeeded(appId)
    }

    fun openLibrary() {
        _uiState.update {
            it.copy(
                selectedAppId = null,
                destination = LocalAppsDestination.Library,
            )
        }
    }

    private fun requestSessions(appId: String, offset: ULong?) {
        sessionRequestOffsets[appId] = offset
        submit(ClientCommand.ListAppSessions(appId = appId, offset = offset, limit = null))
    }

    private fun deleteApp(appId: String) {
        val runtimeUrl = _uiState.value.apps.firstOrNull { it.id == appId }?.runtime?.url
            ?: _uiState.value.details[appId]?.runtime?.url
        if (!webStorageCleanup.prepareDeletion(appId, runtimeUrl)) {
            error(
                strings.resolve(
                    R.string.local_apps_error_web_storage_cleanup_queue,
                    "无法安全记录应用浏览数据清理任务，应用尚未删除。",
                ),
            )
            return
        }
        val bound = source
        if (bound == null) {
            webStorageCleanup.cancelDeletion(appId)
            error("Local-app engine is unavailable")
            return
        }
        viewModelScope.launch {
            runCatching { bound.submitClientCommand(ClientCommand.DeleteApp(appId)) }
                .onFailure { failure ->
                    // Submission did not reach the engine, so the app still
                    // owns its origin. Release only the unconfirmed journal;
                    // a confirmed deletion is intentionally irreversible here.
                    webStorageCleanup.cancelDeletion(appId)
                    error(failure.message ?: failure::class.simpleName.orEmpty())
                }
        }
    }

    private fun executeBridgeRequest(message: LocalAppBridgeMessage) {
        if (message.appId.isBlank() || message.requestId.isBlank()) return
        val operation = bridgeOperationFor(message.operation)
        if (operation == null) {
            publishBridgeFailure(
                message = message,
                error = strings.resolve(
                    R.string.local_apps_error_bridge_unsupported_op,
                    "应用请求了不支持的 Bridge 操作：%1\$s",
                    message.operation,
                ),
                errorCode = "operation_unsupported",
            )
            return
        }
        val bound = source
        if (bound == null) {
            publishBridgeFailure(message, "The local-app engine is unavailable", "engine_unavailable")
            return
        }
        viewModelScope.launch {
            runCatching {
                bound.submitClientCommand(
                    ClientCommand.ExecuteAppBridgeRequest(
                        AppBridgeRequestDto(
                            requestId = message.requestId,
                            appId = message.appId,
                            operation = operation,
                            payloadJson = message.payloadJson,
                        ),
                    ),
                )
            }.onFailure { failure ->
                publishBridgeFailure(
                    message,
                    failure.message ?: "The local-app engine rejected the Bridge request",
                    "engine_rejected",
                )
            }
        }
    }

    private fun publishBridgeFailure(message: LocalAppBridgeMessage, error: String, errorCode: String) {
        _uiState.update { state ->
            state.copy(
                bridgeResults = state.bridgeResults + (
                    LocalAppBridgeRequestKey(message.appId, message.requestId) to LocalAppBridgeResult(
                        requestId = message.requestId,
                        appId = message.appId,
                        ok = false,
                        payloadJson = null,
                        error = error,
                        errorCode = errorCode,
                    )
                ),
            )
        }
    }

    /**
     * A page can raise a second prompt before the first is answered — two
     * `fetch()` calls to two unauthorized declared domains in one tick produce
     * two `AppCapabilityRequested` events. Overwriting the head would strand
     * the first request until the engine's approval timeout with the page's
     * fetch stalled for that whole window, so queue strictly FIFO.
     */
    private fun enqueueAuthorization(request: LocalAppAuthorizationRequest) {
        if (_uiState.value.pendingAuthorization == null) {
            _uiState.update { it.copy(pendingAuthorization = request) }
            return
        }
        if (queuedAuthorizations.size >= MAX_QUEUED_AUTHORIZATIONS) {
            // Drop the newest, not the oldest: the oldest already has a page
            // awaiting it. This is pathological-only (8 unanswered prompts).
            error(strings.resolve(R.string.local_apps_authorization_queue_overflow, "应用的授权请求过多，已忽略最新一条"))
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
                            strings.resolve(R.string.local_apps_ui_action_missing_target, "UI 请求缺少有效目标或参数")
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
                selectedDetailsTab = if (request.isUiControl && decision != LocalAppAuthorizationDecision.Deny) {
                    LocalAppDetailsTab.Preview
                } else it.selectedDetailsTab,
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
                is LocalAppsDestination.Preview,
                is LocalAppsDestination.Details -> state.copy(destination = LocalAppsDestination.Library)
            }
        }
    }

    private fun reduce(event: ClientEvent) {
        when (event) {
            is ClientEvent.AppsChanged -> reduceApps(event)
            is ClientEvent.AppEvent -> reduceAppEvent(event.event)
            is ClientEvent.AppWorkflowChanged -> updateApp(event.appId) {
                it.copy(workflow = event.state.toUiWorkflow())
            }
            is ClientEvent.AppSessionsChanged -> reduceAppSessions(event)
            is ClientEvent.AppRuntimeChanged -> {
                if (event.state != AppRuntimeStateDto.STARTING) {
                    runtimeStartsInFlight.remove(event.appId)
                }
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
            is ClientEvent.AppOperationFailed -> {
                error(event.message)
                // CreateApp failures carry no app id. Fail-closed: drop
                // in-flight create claims so a later AppsChanged cannot pin
                // or open the wrong app. Per-app failures keep their claims.
                if (event.appId == null) {
                    pendingCreates.clear()
                }
            }
            else -> Unit
        }
    }

    private fun reduceAppEvent(event: AppEventDto) {
        when (event) {
            is AppEventDto.AppDetailsChanged -> reduceDetails(event.details)
            is AppEventDto.AppRecordChanged -> {
                val prior = _uiState.value.apps.firstOrNull { it.id == event.record.id }
                val app = event.record.toUiApp(fallbackRuntime = prior?.runtime)
                // RecordChanged may race the initial full catalog snapshot.
                // Upsert it and restore the canonical newest-first ordering so
                // an incremental init-session pin cannot be dropped or leave
                // the list sorted differently from AppsChanged.
                upsertApp(app)
            }
            is AppEventDto.AppBridgeResponse -> {
                val response = event.response
                _uiState.update { state ->
                    state.copy(
                        bridgeResults = state.bridgeResults + (
                            LocalAppBridgeRequestKey(response.appId, response.requestId) to LocalAppBridgeResult(
                                requestId = response.requestId,
                                appId = response.appId,
                                ok = response.ok,
                                payloadJson = response.resultJson,
                                error = response.error,
                                errorCode = response.errorCode,
                            )
                        ),
                    )
                }
            }
            // Stream frames are consumed by the app bridge/session stream
            // owner. Keep the library reducer exhaustive without treating a
            // frame as a one-shot bridge result.
            is AppEventDto.AppBridgeStreamFrame -> {
                val appId = runCatching { JSONObject(event.frameJson).optString("appId") }
                    .getOrNull()
                    ?.takeIf { it.isNotBlank() }
                if (appId != null) {
                    LocalAppWebViewRegistry.deliverStreamFrame(appId, event.frameJson)
                }
            }
            is AppEventDto.AppUiRequest -> {
                val request = event.request
                val action = request.toUiAutomationAction()
                val capabilityDecision = uiControlGrants[request.appId]
                if (capabilityDecision == LocalAppAuthorizationDecision.AllowOnce) {
                    uiControlGrants.remove(request.appId)
                }
                // CAPTURE_VIEW rides with INSPECT because it is read-only in the
                // same sense — the engine does not gate it on `ui_control` at
                // all (`capture_ui` never calls `authorize_capability`), and the
                // agent already had to clear the `LocalAppCaptureUi` prompt,
                // which is DenyByDefault. Routing it to the `ui_control` prompt
                // instead was worse than redundant: the title asks to CONTROL
                // the interface, and an AllowSession/AllowAlways answer is
                // recorded in `uiControlGrants` — so approving a screenshot
                // silently authorized click/fill/navigate for the whole session.
                val readOnlyAction = request.action == AppUiActionKindDto.INSPECT ||
                    request.action == AppUiActionKindDto.CAPTURE_VIEW
                val executionDecision = capabilityDecision
                    ?: LocalAppAuthorizationDecision.AllowOnce.takeIf { readOnlyAction }
                if (executionDecision != null && action != null) {
                    _uiState.update {
                        it.copy(
                            selectedAppId = request.appId,
                            destination = LocalAppsDestination.Details(request.appId, LocalAppDetailsTab.Preview),
                            selectedDetailsTab = LocalAppDetailsTab.Preview,
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
                            error = strings.resolve(R.string.local_apps_ui_action_missing_target, "UI 请求缺少有效目标或参数"),
                        ),
                    )
                } else {
                    enqueueAuthorization(
                        LocalAppAuthorizationRequest(
                            requestId = request.requestId,
                            appId = request.appId,
                            title = strings.resolve(R.string.local_apps_permission_ui_control, "允许 Agent 控制应用界面？"),
                            reason = strings.resolve(
                                R.string.local_apps_ui_action_reason,
                                "Agent 请求执行 %1\$s 操作。",
                                request.action.name.lowercase(),
                            ),
                            isUiControl = true,
                            uiAction = action,
                        ),
                    )
                }
            }
            is AppEventDto.AppCapabilityRequested -> {
                val request = event.request
                pendingCapabilityKinds[request.requestId] = request.capability
                val domainSuffix = request.domain?.let {
                    strings.resolve(R.string.local_apps_capability_domain_suffix, "\n域名：%1\$s", it)
                }
                enqueueAuthorization(
                    LocalAppAuthorizationRequest(
                        requestId = request.requestId,
                        appId = request.appId,
                        title = request.capability.authorizationTitle(strings, request.reason),
                        reason = buildString {
                            append(request.reason)
                            domainSuffix?.let(::append)
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
            // iOS-first: Android renders neither the "calling AI" indicator
            // nor the mailbox badge yet, but the wire enum is exhaustive here
            // and silence would be a compile error, not a no-op.
            is AppEventDto.AppLlmActivityChanged -> Unit
            is AppEventDto.AppAgentEventPosted -> Unit
            is AppEventDto.AppBackgroundTaskChanged -> Unit
            is AppEventDto.AppProfileProposal -> _uiState.update { state ->
                state.copy(
                    pendingProfileProposal = LocalAppProfileProposal(
                        appId = event.proposal.appId,
                        approvalToken = event.proposal.approvalToken,
                        baseRevision = event.proposal.baseRevision,
                        currentRevision = event.proposal.currentRevision,
                        instructions = event.proposal.instructions,
                        reason = event.proposal.reason,
                    ),
                )
            }
        }
    }

    private fun resolveProfileProposal(approved: Boolean) {
        val proposal = _uiState.value.pendingProfileProposal ?: return
        _uiState.update { it.copy(pendingProfileProposal = null) }
        submit(
            ClientCommand.ResolveAppProfileProposal(
                appId = proposal.appId,
                approvalToken = proposal.approvalToken,
                approved = approved,
            ),
        )
    }

    private fun reduceDetails(details: com.lingxi.code.bindings.AppDetailsDto) {
        val prior = _uiState.value.apps.firstOrNull { it.id == details.app.id }
        val app = details.app.toUiApp(
            runtime = details.runtime.toUiRuntime(),
            fallbackRuntime = prior?.runtime,
        )
        _uiState.update { current ->
            val apps = if (current.apps.any { it.id == app.id }) {
                current.apps.map { if (it.id == app.id) app else it }
            } else {
                current.apps + app
            }
            current.copy(
                apps = apps.sortedByDescending { it.updatedAtMs },
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
        publishWidgetSnapshot()
    }

    /**
     * Fold one `AppSessionsChanged` page into the app's cached catalog. The
     * event carries no request correlator, so [sessionRequestOffsets] (stamped
     * at request time) decides whether this reply replaces the cached rows (a
     * first-page request — or an unsolicited engine push, which always carries
     * a fresh first page) or appends them (a 「加载更多」 page). Rows are
     * deduped by uuid with the newest copy winning, and the pinned init row is
     * always displayed first.
     */
    private fun reduceAppSessions(event: ClientEvent.AppSessionsChanged) {
        val requestedOffset = sessionRequestOffsets.remove(event.appId)
        val now = nowEpochSeconds()
        val incoming = event.sessions.map { it.toUiRow(now) }
        _uiState.update { state ->
            val existing = state.appSessions[event.appId]
            val merged = if (requestedOffset == null || existing == null || !existing.loaded) {
                incoming
            } else {
                existing.rows.filter { old -> incoming.none { it.uuid == old.uuid } } + incoming
            }
            state.copy(
                appSessions = state.appSessions + (
                    event.appId to LocalAppSessionPage(
                        rows = sessionRowsForDisplay(merged),
                        nextOffset = event.nextOffset,
                        loaded = true,
                    )
                ),
            )
        }
    }

    private fun AppSessionRowDto.toUiRow(nowEpochSeconds: Long): LocalAppSessionRow {
        val base = SessionCatalog.rowFrom(
            uuid = uuid,
            title = title,
            messageCount = messageCount.toInt(),
            modifiedRfc3339 = modifiedRfc3339,
            nowEpochSeconds = nowEpochSeconds,
            strings = sessionStrings,
        )
        return LocalAppSessionRow(
            uuid = base.uuid,
            title = base.title,
            relativeTime = base.relativeTime,
            messageCount = base.messageCount,
            isInit = kind == AppSessionKindDto.INIT,
        )
    }

    private fun reduceApps(event: ClientEvent.AppsChanged) {
        val oldIds = _uiState.value.apps.mapTo(hashSetOf()) { it.id }
        val apps = event.apps.map { record ->
            val prior = _uiState.value.apps.firstOrNull { it.id == record.id }
            record.toUiApp(fallbackRuntime = prior?.runtime)
        }.sortedByDescending { it.updatedAtMs }
        val liveIds = apps.mapTo(hashSetOf()) { it.id }
        runtimeStartsInFlight.retainAll(liveIds)
        // Only an explicit DeleteApp action may journal browser-data removal.
        // A source/profile rebind can also make ids disappear from this local
        // reducer, and must never be interpreted as user-authorized deletion.
        webStorageCleanup.reconcile(liveIds)
        sessionRequestOffsets.keys.retainAll(liveIds)
        _uiState.update { state ->
            val destination = state.destination
            state.copy(
                apps = apps,
                details = state.details.filterKeys(liveIds::contains),
                appSessions = state.appSessions.filterKeys(liveIds::contains),
                bridgeResults = state.bridgeResults.filterValues { it.appId in liveIds },
                selectedAppId = state.selectedAppId?.takeIf { it in liveIds },
                // Every per-app screen for a deleted app is now an empty shell
                // with no way forward — leave the screen we emptied.
                destination = if (destination.appIdOnScreen()?.let { it !in liveIds } == true) {
                    LocalAppsDestination.Library
                } else {
                    destination
                },
                loading = false,
            )
        }
        publishWidgetSnapshot()

        // At most one create is armed. Claim it only when a newly announced
        // id carries the same brief; `id !in oldIds` stops a pre-existing app
        // that happens to share that brief from being claimed.
        val newApps = apps.filter { it.id !in oldIds }
        newApps.forEach { candidate ->
            val matchIndex = pendingCreates.indexOfFirst { it.brief == candidate.brief }
            if (matchIndex >= 0) {
                val pendingCreate = pendingCreates.removeAt(matchIndex)
                openApp(candidate.id)
                if (pendingCreate.addWidget) {
                    widgetPinRequestChannel.trySend(candidate.id)
                }
            }
        }
    }

    private fun updateApp(appId: String, transform: (LocalAppItem) -> LocalAppItem) {
        _uiState.update { state ->
            state.copy(apps = state.apps.map { if (it.id == appId) transform(it) else it })
        }
        publishWidgetSnapshot()
    }

    private fun upsertApp(app: LocalAppItem) {
        _uiState.update { state ->
            val updated = state.apps.toMutableList()
            val index = updated.indexOfFirst { it.id == app.id }
            if (index >= 0) {
                updated[index] = app
            } else {
                updated += app
            }
            state.copy(apps = updated.sortedByDescending { it.updatedAtMs })
        }
        publishWidgetSnapshot()
    }

    private fun startRuntimeIfNeeded(appId: String) {
        runtimeLastUsedAt[appId] = System.currentTimeMillis()
        val runtimeState = _uiState.value.apps.firstOrNull { it.id == appId }?.runtime?.state
            ?: _uiState.value.details[appId]?.runtime?.state
        if (runtimeState == LocalAppRuntimeState.Running ||
            runtimeState == LocalAppRuntimeState.Starting
        ) {
            return
        }
        if (!runtimeStartsInFlight.add(appId)) return
        val bound = source
        if (bound == null) {
            runtimeStartsInFlight.remove(appId)
            error(strings.resolve(R.string.local_apps_error_engine_unavailable, "此构建未包含本地应用引擎。"))
            return
        }
        viewModelScope.launch {
            runCatching {
                bound.submitClientCommand(ClientCommand.StartApp(appId))
            }.onFailure {
                runtimeStartsInFlight.remove(appId)
                error(it.message ?: it::class.simpleName.orEmpty())
            }
        }
    }

    private fun publishWidgetSnapshot() {
        widgetSnapshotSync.publish(_uiState.value.apps)
    }

    private fun submit(command: ClientCommand, onFailure: (() -> Unit)? = null) {
        submit(onFailure = onFailure) { it.submitClientCommand(command) }
    }

    private fun submit(
        onFailure: (() -> Unit)? = null,
        block: suspend (ConversationSource) -> Unit,
    ) {
        val bound = source
        if (bound == null) {
            onFailure?.invoke()
            return
        }
        viewModelScope.launch {
            runCatching { block(bound) }.onFailure {
                onFailure?.invoke()
                error(it.message ?: it::class.simpleName.orEmpty())
            }
        }
    }

    private fun error(message: String) {
        _uiState.update { it.copy(loading = false, error = message) }
    }

    companion object {
        private const val MAX_QUEUED_AUTHORIZATIONS = 8

        fun factory(
            sourceFlow: StateFlow<ConversationSource>,
            strings: LocalAppsStrings = DefaultLocalAppsStrings,
            sessionStrings: SessionCatalogStrings = DefaultSessionCatalogStrings,
            webStorageCleanup: LocalAppWebStorageCleanup = NoopLocalAppWebStorageCleanup,
            widgetSnapshotSync: LocalAppWidgetSnapshotSync = NoopLocalAppWidgetSnapshotSync,
        ): ViewModelProvider.Factory =
            object : ViewModelProvider.Factory {
                @Suppress("UNCHECKED_CAST")
                override fun <T : ViewModel> create(modelClass: Class<T>): T =
                    LocalAppsViewModel(
                        sourceFlow,
                        strings = strings,
                        sessionStrings = sessionStrings,
                        webStorageCleanup = webStorageCleanup,
                        widgetSnapshotSync = widgetSnapshotSync,
                    ) as T
            }
    }
}

/**
 * Exhaustive enum-to-wire mapping. Adding an operation to the UniFFI enum now
 * breaks this `when` instead of silently leaving Android behind iOS.
 */
internal fun AppBridgeOperationDto.bridgeWireName(): String = when (this) {
    AppBridgeOperationDto.QUERY_DATA -> "query_data"
    AppBridgeOperationDto.MUTATE_DATA -> "mutate_data"
    AppBridgeOperationDto.NETWORK_REQUEST -> "network_request"
    AppBridgeOperationDto.RUNTIME_STATUS -> "runtime_status"
    AppBridgeOperationDto.CAPTURE_PHOTO -> "capture_photo"
    AppBridgeOperationDto.PICK_IMAGE -> "pick_image"
    AppBridgeOperationDto.RECORD_AUDIO_START -> "record_audio_start"
    AppBridgeOperationDto.RECORD_AUDIO_STOP -> "record_audio_stop"
    AppBridgeOperationDto.GET_LOCATION -> "get_location"
    AppBridgeOperationDto.TRANSCRIBE_SPEECH -> "transcribe_speech"
    AppBridgeOperationDto.POST_NOTIFICATION -> "post_notification"
    AppBridgeOperationDto.CLIPBOARD_GET_TEXT -> "clipboard_get_text"
    AppBridgeOperationDto.CLIPBOARD_SET_TEXT -> "clipboard_set_text"
    AppBridgeOperationDto.SHARE -> "share"
    AppBridgeOperationDto.SYNTHESIZE_SPEECH -> "synthesize_speech"
    AppBridgeOperationDto.FILE_READ -> "file_read"
    AppBridgeOperationDto.FILE_WRITE -> "file_write"
    AppBridgeOperationDto.DEVICE_STATUS -> "device_status"
    AppBridgeOperationDto.HAPTICS -> "haptics"
    AppBridgeOperationDto.DEEP_LINK -> "deep_link"
    AppBridgeOperationDto.LLM_CHAT -> "llm_chat"
    AppBridgeOperationDto.LLM_STREAM -> "llm_stream"
    AppBridgeOperationDto.AGENT_POST -> "agent_post"
    AppBridgeOperationDto.AGENT_SESSION_CREATE -> "agent_session_create"
    AppBridgeOperationDto.AGENT_SESSION_LIST -> "agent_session_list"
    AppBridgeOperationDto.AGENT_SESSION_RESUME -> "agent_session_resume"
    AppBridgeOperationDto.AGENT_SESSION_CLOSE -> "agent_session_close"
    AppBridgeOperationDto.AGENT_SEND -> "agent_send"
    AppBridgeOperationDto.AGENT_STREAM -> "agent_stream"
    AppBridgeOperationDto.AGENT_CANCEL -> "agent_cancel"
    AppBridgeOperationDto.AGENT_PROFILE_PROPOSE_UPDATE -> "agent_profile_propose_update"
    AppBridgeOperationDto.BACKGROUND_SCHEDULE -> "background_schedule"
    AppBridgeOperationDto.BACKGROUND_LIST -> "background_list"
    AppBridgeOperationDto.BACKGROUND_STATUS -> "background_status"
    AppBridgeOperationDto.BACKGROUND_CANCEL -> "background_cancel"
    AppBridgeOperationDto.BACKGROUND_RETRY -> "background_retry"
    AppBridgeOperationDto.CALENDAR_LIST_EVENTS -> "calendar_list_events"
    AppBridgeOperationDto.CONTACTS_SEARCH -> "contacts_search"
    AppBridgeOperationDto.MEDIA_GET -> "media_get"
}

internal fun bridgeOperationFor(wireName: String): AppBridgeOperationDto? =
    AppBridgeOperationDto.entries.firstOrNull { it.bridgeWireName() == wireName }

/**
 * The app a destination is showing, or null on the app-independent Library.
 * `reduceApps` needs it to move the user off a screen a deletion just emptied;
 * an exhaustive `when` makes a new per-app destination a compile error rather
 * than a screen the prune silently forgets.
 */
private fun LocalAppsDestination.appIdOnScreen(): String? = when (this) {
    LocalAppsDestination.Library -> null
    is LocalAppsDestination.Preview -> appId
    is LocalAppsDestination.Details -> appId
}

/**
 * The v3 wire workflow is exactly `draft`/`ready`. Explicit branches, not an
 * `else`, so this `when` breaks the moment a state is added or renamed.
 */
private fun AppWorkflowStateDto.toUiWorkflow(): LocalAppWorkflow = when (this) {
    AppWorkflowStateDto.DRAFT -> LocalAppWorkflow.Draft
    AppWorkflowStateDto.READY -> LocalAppWorkflow.Ready
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
        // Old persisted runtimes are migrated by the engine on their next
        // start. Never project that legacy wire value back into current UI.
        AppRuntimeModeDto.NEXT_PRODUCTION -> LocalAppRuntimeMode.ViteStatic
        null -> null
    },
    url = loopbackUrl,
    detail = lastError ?: suspensionReason?.name?.lowercase(),
    recovery = recoveryState?.name?.lowercase(),
)

private fun AppRecordDto.toUiApp(
    runtime: LocalAppRuntime? = null,
    fallbackRuntime: LocalAppRuntime? = null,
): LocalAppItem = LocalAppItem(
    id = id,
    name = name,
    brief = brief,
    gitEnabled = gitEnabled,
    workflow = workflowState.toUiWorkflow(),
    runtime = runtime ?: fallbackRuntime ?: LocalAppRuntime(),
    updatedAtMs = updatedAtMs.toLong(),
    workspaceRel = workspaceRel,
    initSessionId = initSessionId,
)

private fun AppCapabilityKindDto.authorizationTitle(
    strings: LocalAppsStrings,
    reason: String,
): String = when (this) {
    AppCapabilityKindDto.DATA_MUTATION ->
        strings.resolve(R.string.local_apps_permission_data_mutation_plain, "允许修改应用数据？")
    AppCapabilityKindDto.UI_CONTROL ->
        strings.resolve(R.string.local_apps_permission_ui_control, "允许 Agent 控制应用界面？")
    AppCapabilityKindDto.NETWORK_DOMAIN ->
        strings.resolve(R.string.local_apps_permission_network_short, "允许应用联网？")
    AppCapabilityKindDto.RESTORE_CHECKPOINT ->
        strings.resolve(R.string.local_apps_permission_restore, "允许恢复代码检查点？")
    AppCapabilityKindDto.CAMERA ->
        strings.resolve(R.string.local_apps_permission_camera, "允许应用使用相机拍照？")
    AppCapabilityKindDto.PHOTO_LIBRARY ->
        strings.resolve(R.string.local_apps_permission_photo_library, "允许应用从相册选择图片？")
    AppCapabilityKindDto.MICROPHONE ->
        strings.resolve(R.string.local_apps_permission_microphone, "允许应用使用麦克风录音？")
    AppCapabilityKindDto.LOCATION ->
        strings.resolve(R.string.local_apps_permission_location, "允许应用获取当前位置？")
    AppCapabilityKindDto.NOTIFICATIONS ->
        strings.resolve(R.string.local_apps_permission_notifications, "允许应用发送本地通知？")
    AppCapabilityKindDto.CLIPBOARD ->
        "允许应用读取或写入系统剪贴板？"
    AppCapabilityKindDto.SHARE ->
        "允许应用打开系统分享面板？"
    AppCapabilityKindDto.TEXT_TO_SPEECH ->
        "允许应用将文字转换为语音？"
    AppCapabilityKindDto.FILES_READ ->
        "允许应用读取自己的私有文件？"
    AppCapabilityKindDto.FILES_WRITE ->
        "允许应用写入自己的私有文件？"
    AppCapabilityKindDto.FILES -> when {
        reason.contains("写入") -> "允许应用写入自己的私有文件？"
        reason.contains("读取") -> "允许应用读取自己的私有文件？"
        else -> "允许应用访问自己的私有文件？"
    }
    AppCapabilityKindDto.DEVICE_STATUS ->
        "允许应用读取设备状态？"
    AppCapabilityKindDto.HAPTICS ->
        "允许应用触发触觉反馈？"
    AppCapabilityKindDto.DEEP_LINK ->
        "允许应用打开外部链接？"
    AppCapabilityKindDto.LLM ->
        strings.resolve(R.string.local_apps_permission_llm, "允许应用调用 AI 模型？（会消耗你的模型用量）")
    AppCapabilityKindDto.AGENT_NOTIFY ->
        strings.resolve(R.string.local_apps_permission_agent_notify, "允许应用向对话助手发送事件？")
    AppCapabilityKindDto.BACKGROUND_SCHEDULE ->
        "允许应用在系统后台按计划运行流程？"
    AppCapabilityKindDto.CALENDAR ->
        "允许应用读取指定范围内的日历事件？"
    AppCapabilityKindDto.CONTACTS ->
        "允许应用搜索联系人？"
    AppCapabilityKindDto.MEDIA ->
        "允许应用读取自己刚获取的媒体？"
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
        AppUiActionKindDto.CAPTURE_VIEW -> LocalAppUiAutomationAction.CaptureView
        // `"x,y"` / `"x,y,phase"`, the same comma-packed `value` convention
        // SCROLL already uses. The wire variant is fieldless on purpose: a
        // data-carrying uniffi variant renders this enum as a Kotlin sealed
        // class and renames every existing constant.
        AppUiActionKindDto.POINTER -> value.orEmpty().split(',').map { it.trim() }.let { parts ->
            // `toDoubleOrNull`, not `toIntOrNull`: the documented unit is CSS
            // pixels and an agent reading a centre off `getBoundingClientRect()`
            // sends "207.5,320". `toIntOrNull` rejected that outright and the
            // whole action was dropped with a generic "missing target" error,
            // while iOS (which parses with `Number`) accepted it — one wire
            // contract, two answers.
            val x = parts.getOrNull(0)?.toDoubleOrNull()?.let { Math.round(it).toInt() }
            val y = parts.getOrNull(1)?.toDoubleOrNull()?.let { Math.round(it).toInt() }
            // The phase is passed through UNVALIDATED on purpose; the injected
            // script rejects an unknown one, so both platforms answer the same
            // way. Coercing it to "tap" here turned an intended hold into a
            // tap-and-release that still reported success.
            val phase = parts.getOrNull(2)?.lowercase().orEmpty().ifEmpty { "tap" }
            if (x == null || y == null) null else LocalAppUiAutomationAction.Pointer(x, y, phase)
        }
        // Split from the RIGHT and only when the tail is a known phase, so the
        // key `,` itself still works.
        AppUiActionKindDto.KEY -> value.orEmpty().trim().let { raw ->
            val comma = raw.lastIndexOf(',')
            val tail = if (comma >= 0) raw.substring(comma + 1).trim().lowercase() else ""
            val hasPhase = tail in setOf("press", "down", "up")
            val key = if (hasPhase) raw.substring(0, comma).trim() else raw
            if (key.isEmpty()) null else LocalAppUiAutomationAction.Key(key, if (hasPhase) tail else "press")
        }
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

private fun AppDataFieldDto.toUiDataField(): LocalAppDataField = LocalAppDataField(
    id = id,
    label = label,
    type = fieldType.toUiDataFieldType(),
    required = required,
    options = options,
)

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
