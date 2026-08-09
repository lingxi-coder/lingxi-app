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
import com.lingxi.code.bindings.AppDesignFieldDto
import com.lingxi.code.bindings.AppDesignPatchDto
import com.lingxi.code.bindings.AppDesignPatchOpDto
import com.lingxi.code.bindings.AppDesignFieldTypeDto
import com.lingxi.code.bindings.AppDesignStepDto
import com.lingxi.code.bindings.AppErrorCodeDto
import com.lingxi.code.bindings.AppEventDto
import com.lingxi.code.bindings.AppGenerationJobDto
import com.lingxi.code.bindings.AppGenerationJobStateDto
import com.lingxi.code.bindings.AppPlanDto
import com.lingxi.code.bindings.AppRecordDto
import com.lingxi.code.bindings.AppRuntimeDetailsDto
import com.lingxi.code.bindings.AppRuntimeModeDto
import com.lingxi.code.bindings.AppRuntimeStateDto
import com.lingxi.code.bindings.AppUiActionKindDto
import com.lingxi.code.bindings.AppUiRequestDto
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
    private val strings: LocalAppsStrings = DefaultLocalAppsStrings,
) : ViewModel() {
    // `internal`, not `private`: several same-module tests seed reducer-only
    // state (e.g. `questionnaires`/`plans`, which only ever change through
    // `AppEventDto.AppQuestionnaireChanged`/`AppPlanChanged`) directly rather
    // than replaying a full event sequence — this is that seam, kept as
    // narrow as a single field's visibility.
    internal val _uiState = MutableStateFlow(
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

    /**
     * Every `CreateFromBrief` still awaiting its `AppsChanged` claim, FIFO —
     * one entry per in-flight create, not a single overwritable slot
     * (local-apps#questionnaire, Task 19). A scalar here meant a second
     * create before the first's ack arrived silently replaced the first's
     * claim, so the first app could be claimed by the WRONG pending create
     * (or none at all, if the id-freshness check in `reduceApps` then failed
     * to match it) — see `reduceApps` for how each entry is matched and
     * consumed.
     *
     * Keyed on `brief` alone, not `name`+`brief` (Task 20): `createFromBrief`
     * now sends `name = ""` on every create and lets
     * `AppService::create_app` derive the display name from the brief itself
     * (mirrors iOS's `LocalAppsStore.createApp(brief:)`), so the persisted
     * record's `name` is never the literal string this ViewModel sent —
     * matching on it would never succeed.
     *
     * RESIDUAL AMBIGUITY: `AppRecordDto`/`ClientCommand.CreateApp` carry no
     * correlation id, so `brief` is the best discriminator available. Two
     * concurrent creates with the IDENTICAL brief cannot be told apart by
     * content alone: `reduceApps` matches FIFO by list position against
     * `apps`' sort order, which is `updatedAtMs`-descending, not request
     * order. If two such creates race closely enough that their records tie
     * or invert on `updatedAtMs`, the wrong pending entry can be claimed for
     * a given new id. Nothing is corrupted by this — both apps still exist,
     * independently editable — only WHICH one's designer opens first can be
     * swapped. A real correlation id on the wire is the only way to close
     * this gap completely.
     */
    private val pendingCreates = mutableListOf<String>()
    private val draftEditQueue = mutableListOf<QueuedDraftEdit>()
    private val textEditJobs = mutableMapOf<String, Job>()
    private var draftEditInFlight: InFlightDraftEdit? = null

    /**
     * How long a draft-queue gate may wait on an engine answer that may never
     * come. It bounds both gates in [pumpDraftEditQueue]: the single in-flight
     * slot ([draftEditInFlight]) and the post-conflict details refresh
     * ([draftConflictRefresh]). Far longer than a healthy round trip, so it
     * normally only fires on a failure the event stream could not attribute —
     * but it is a heuristic, not a guarantee: a slow engine or a FORWARD
     * wall-clock step can trip it early. (A backward step cannot: it re-stamps,
     * which delays the deadline rather than pulling it in.) Tripping early is
     * tolerable only because neither gate LOSES anything when it ages out: the
     * in-flight edit is RE-SENT rather than dropped, and the refresh gate holds
     * no edit at all.
     * Settable so a test can reach the path without waiting out the production
     * budget.
     */
    internal var inFlightEditBudgetMs: Long = 15_000

    /**
     * The wall clock the two budget comparisons in [pumpDraftEditQueue] read,
     * and the source of the re-stamp each one writes when it sees a negative
     * elapsed. The two INITIAL `sentAtMs` stamps are taken by the data classes
     * below, so replacing this seam moves "now" — and, through the re-stamp,
     * only stamps this seam itself wrote.
     *
     * It exists because a BACKWARD step is the one direction a `now - sentAtMs
     * >= budget` gate cannot survive on its own: the difference goes negative,
     * satisfies no lower bound, and holds the gate until the clock catches up.
     * Both gates are process-global, so that stalls every app, not just the one
     * that owns the in-flight edit. iOS reads a monotonic clock and has no such
     * failure mode; this reads [System.currentTimeMillis] because a JVM unit
     * test cannot call `SystemClock`, so the sign is checked explicitly instead.
     */
    internal var currentTimeMs: () -> Long = System::currentTimeMillis

    /**
     * The app whose authoritative snapshot a design conflict is waiting on, with
     * the wall clock at which it was asked for. Timestamped for the same reason
     * [draftEditInFlight] is, and more urgently: the only ANSWER that clears
     * this is [reduceDetails], and it runs only for a *successful*
     * `GetAppDetails` for that app.
     *
     * Four sites clear the field in all, and the other three exist precisely
     * because no answer is coming: the `failedConflictRefresh` arm in [reduce]
     * (a refresh answered with `AppOperationFailed` — the app was deleted from
     * another surface, the store is corrupt), the budget in
     * [pumpDraftEditQueue], and the deleted-app prune in [reduceApps]. Without
     * them, a refresh whose answer is simply dropped (an engine rebind or
     * project switch cancels the event collector, so no event arrives at all)
     * would hold the gate for the life of the ViewModel — and this gate, unlike
     * the in-flight slot, blocks the queue for EVERY app, not just this one.
     */
    private var draftConflictRefresh: PendingConflictRefresh? = null
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

    private data class PendingConflictRefresh(
        val appId: String,
        val sentAtMs: Long = System.currentTimeMillis(),
    )

    init {
        viewModelScope.launch {
            sourceFlow.collectLatest { bound ->
                source = bound
                _uiState.update { it.copy(loading = true, error = null) }
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
            // `createName`/`ChangeCreateName` are unused by the real
            // create screen (Task 20): it collects only a brief and dispatches
            // `CreateFromBrief` directly, with no display-name field to hold
            // — `createFromBrief` below sends `name` empty and lets the
            // engine derive one. This action and `state.createName` are kept
            // only because several ViewModel tests still exercise the exact
            // `ChangeCreateName` + `CreateFromBrief` sequence to probe
            // `pendingCreates` matching; deleting either would force
            // rewriting those, for no behavioral gain.
            LocalAppsAction.Create -> _uiState.update { it.copy(createName = "") }
            is LocalAppsAction.Search -> _uiState.update { it.copy(query = action.query) }
            is LocalAppsAction.ChangeCreateName -> _uiState.update { it.copy(createName = action.name) }
            is LocalAppsAction.CreateFromBrief -> createFromBrief(action.brief)
            is LocalAppsAction.UpdateBrief -> submit(ClientCommand.UpdateAppBrief(action.appId, action.brief))
            is LocalAppsAction.RetryQuestionnaire -> submit(ClientCommand.RetryAppQuestionnaire(action.appId))
            is LocalAppsAction.BeginPlanning -> submit(ClientCommand.BeginAppPlanning(action.appId))
            is LocalAppsAction.RetryPlan -> submit(ClientCommand.RetryAppPlan(action.appId))
            is LocalAppsAction.Revise -> submit(ClientCommand.RequestAppRevision(action.appId, action.prompt))
            is LocalAppsAction.CancelDesign -> cancelDesign(action.appId)
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
            .onFailure {
                error(
                    strings.resolve(
                        R.string.local_apps_error_load_apps,
                        "无法加载应用：%1\$s",
                        "${it.message ?: it::class.simpleName}",
                    ),
                )
            }
        // NOTE (local-apps#questionnaire, Task 5): `requestTemplates` /
        // `ClientCommand.ListAppTemplates` were deleted (human-partner ruling:
        // total removal of the static template catalog).
    }

    // NOTE (local-apps#questionnaire, Task 18/20): this replaces the deleted
    // `createSelectedTemplate()`. `brief` is REQUIRED on the wire (it seeds
    // the LLM questionnaire-authoring round trip `create_app` starts in the
    // background) and is a genuine parameter of `CreateFromBrief` — never
    // fabricated from a display name. `name` is sent EMPTY, every time:
    // `AppService::create_app` (service.rs) derives a display name from the
    // brief itself (first 24 chars) whenever the caller's name is empty or
    // blank, so there is no client-side name to collect, guess, or relabel
    // from the brief at all — mirrors iOS's `LocalAppsStore.createApp(brief:)`
    // exactly. `LocalAppsViewModelTest`'s "create app fabricates the brief
    // from the display name" tripwire (Task 11's stopgap, carried through
    // Task 18) is now a permanent guard that this stays true.
    private fun createFromBrief(brief: String) {
        val trimmedBrief = brief.trim()
        if (trimmedBrief.isEmpty()) return
        pendingCreates += trimmedBrief
        submit(
            ClientCommand.CreateApp(
                name = "",
                origin = AppCreateOriginDto.LIBRARY,
                brief = trimmedBrief,
                conversationId = null,
            ),
        )
    }

    private fun openApp(appId: String) {
        val app = _uiState.value.apps.firstOrNull { it.id == appId } ?: return
        _uiState.update { it.copy(selectedAppId = appId) }
        submit(ClientCommand.GetAppDetails(appId))
        when (app.workflow) {
            // `LocalAppDesignerScreen` owns all six of these
            // (local-apps#questionnaire, Task 19) — the busy/failure states
            // get their own rendering there instead of the generic preview
            // view, with retry actions wired to the store. Mirrors iOS's
            // `LocalAppsLibraryView.open(_:)`.
            LocalAppWorkflow.AuthoringQuestionnaire,
            LocalAppWorkflow.QuestionnaireFailed,
            LocalAppWorkflow.CollectingSpec,
            LocalAppWorkflow.Planning,
            LocalAppWorkflow.PlanFailed,
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

    /**
     * Navigates to the designer and refreshes its draft, WITHOUT
     * unconditionally arming the `open_designer` gate (local-apps#questionnaire,
     * Task 19 — mirrors iOS's `LocalAppDesignerView.prepare()`).
     *
     * `open_designer` (state.rs) is legal only from `collecting_spec` /
     * `generation_failed` — but even for `collecting_spec` it is deliberately
     * NOT sent here: `open_designer` unconditionally advances
     * `collecting_spec -> awaiting_spec_confirmation`, arming the LATER
     * plan-confirm gate before the user has even answered a question.
     * `begin_planning` (the questionnaire's own terminal action, dispatched
     * by `BeginPlanning` at the last step) requires exactly `collecting_spec`,
     * so that eager transition would make every 生成方案 tap fail with
     * `workflow_state_invalid` the moment the designer was ever opened.
     *
     * A freshly created app starts in `authoring_questionnaire`, where
     * `open_designer` is illegal outright — sending it unconditionally (the
     * bug this fixes) rejected with `WORKFLOW_STATE_INVALID` on EVERY create
     * tap, surfaced as the raw Rust string in the generic error dialog, with
     * the designer stuck on an infinite spinner underneath because
     * `AppDesignerRequested` — the only event that populates `state.designer`
     * for that path — was never coming.
     *
     * `GetAppDetails` alone is enough for every other case — NOT because
     * `update_draft` skips a workflow check (it does not:
     * `ensure_workflow("update_draft", &DRAFT_EDITABLE_STATES)`, state.rs) but
     * because `GetAppDetails` is a pure read that never calls `update_draft`
     * at all, and `questionnaire_ready` (the LLM-authoring completion)
     * auto-transitions `authoring_questionnaire -> collecting_spec` on its
     * own — which IS inside `DRAFT_EDITABLE_STATES` — with no client command
     * required to get there. So the draft becomes editable server-side
     * without this function ever having to ask for it, and `reduceDetails`
     * below creates `state.designer` the first time a `GetAppDetails` reply
     * lands for the app on this destination — the questionnaire form
     * (`state.questionnaires[appId]`, delivered independently by
     * `AppQuestionnaireChanged`) and the four intermediate/failure states
     * `LocalAppDesignerScreen` renders while `isDesignerEditable(app.workflow)`
     * is false both come from the SAME `state.apps`/`state.questionnaires`
     * the screen already reads, so the designer simply waits rather than
     * erroring.
     *
     * `generation_failed` is the one state where `open_app_designer` is both
     * legal and needed: generation usually fails on the DESIGN itself, and
     * re-arming the confirm gate is what returns the draft to an editable
     * state for a plan re-confirm (mirrors iOS's `.generationFailed` case,
     * reached from `LocalAppPreviewScreen`'s "继续设计" button).
     */
    private fun openDesigner(appId: String) {
        _uiState.update { it.copy(destination = LocalAppsDestination.Designer(appId), selectedAppId = appId) }
        submit(ClientCommand.GetAppDetails(appId))
        val workflow = _uiState.value.apps.firstOrNull { it.id == appId }?.workflow
        if (workflow == LocalAppWorkflow.GenerationFailed) {
            submit(ClientCommand.OpenAppDesigner(appId))
        }
    }

    private fun editField(action: LocalAppsAction.EditField) {
        val designer = _uiState.value.designer ?: return
        val fieldKind = _uiState.value.questionnaires[designer.appId].orEmpty()
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
            val now = currentTimeMs()
            val elapsed = now - stale.sentAtMs
            if (elapsed < 0) {
                // A backward wall-clock step, not a stall: `now` moved and the patch
                // did not, so it is exactly as young as it was a moment ago. RE-STAMP
                // and keep holding the slot. That restarts the budget from the
                // stepped clock — a patch that really is stranded still ages out one
                // budget from here, instead of waiting for the clock to climb back —
                // while a healthy in-flight patch costs nothing. Releasing instead
                // would resend on EVERY backward tick, including the sub-second NTP
                // correction that is by far the common case (see [currentTimeMs]).
                draftEditInFlight = stale.copy(sentAtMs = now)
            } else if (elapsed >= inFlightEditBudgetMs) {
                // Re-queue rather than discard, guarded like the conflict arm so a
                // newer local value for the field wins. Discarding would lose the
                // answer while `designer.values` kept the confirm gate satisfied, so a
                // confirm could ship a spec the engine never received. Resending is
                // safe and is what makes the wall clock acceptable here: a set-op is
                // idempotent if the ack was merely lost, and a patch sent against a
                // moved revision returns AppDesignConflict, which rebases. A forward
                // clock step therefore only shifts WHEN the resend happens, never
                // whether the edit survives.
                if (draftEditQueue.none { it.appId == stale.edit.appId && it.fieldId == stale.edit.fieldId }) {
                    draftEditQueue.add(0, stale.edit.copy(ready = true))
                }
                draftEditInFlight = null
            }
        }
        // The second gate needs the same bound, and needs it more: it is cleared
        // only by a SUCCESSFUL details snapshot, and it is process-global rather
        // than app-scoped, so a refresh that never lands wedges every later edit
        // for every app. Aging it out loses nothing — `reduceDraftConflict` put
        // the rejected patch back on the queue before raising the gate, so the
        // worst case of resuming against the locally known revision is one more
        // AppDesignConflict, which re-queues and re-asks. That is the same state
        // we are in now, not a lost answer, which is why this may be dropped on
        // a wall clock while the in-flight edit may only be re-sent. A backward
        // step (`elapsed < 0`) is handled the same way as at the gate above: the
        // stamp is moved to the stepped clock so the budget runs again from now,
        // which keeps a stepped clock from holding every app's queue until it
        // catches up, without giving up a refresh that may still be answered.
        draftConflictRefresh?.let { pending ->
            val now = currentTimeMs()
            val elapsed = now - pending.sentAtMs
            if (elapsed < 0) {
                draftConflictRefresh = pending.copy(sentAtMs = now)
            } else if (elapsed >= inFlightEditBudgetMs) {
                releaseConflictRefreshUnanswered()
            }
        }
        if (draftEditInFlight != null || draftConflictRefresh != null) return
        val designer = _uiState.value.designer ?: return
        // Take the first SENDABLE edit, not merely the head. This queue is
        // process-global while the designer is one app, so a head that cannot be
        // sent — its app is not the one on screen, because the user moved to
        // another designer or because its app was deleted — used to block every
        // later edit for EVERY app, permanently: the only removal from the queue
        // is right here, and it was gated on that same head matching. iOS never
        // had the failure mode because its queue is keyed by app id and
        // `flushNextEdit(appID:)` already selects "the first READY edit for THIS
        // app" (LocalAppsStore.swift); this is that selector over a flat list.
        //
        // Skipping a not-yet-ready edit for the same app is order-safe too:
        // `editField` keeps at most one entry per (appId, fieldId), so any two
        // queued entries are different fields and their set-ops commute.
        // Skipped entries are kept, not dropped — the user's typed answer for an
        // app whose designer is closed is still owed to that app, and resumes
        // the next time its designer is open and anything pumps.
        val nextIndex = draftEditQueue.indexOfFirst { it.ready && it.appId == designer.appId }
        if (nextIndex < 0) return
        val next = draftEditQueue.removeAt(nextIndex)
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

    /**
     * Give up on the authoritative snapshot the post-conflict gate is waiting
     * for, and take the designer's conflict banner down with it.
     *
     * `designer.conflictRevision` is the sole input to
     * "设计已在其他位置更新到版本 N，已重新加载，请检查后继续。"
     * (`LocalAppsScreen`), and 已重新加载 is a claim only an engine snapshot can
     * make true. Exactly two reducers replace `values` with the engine's stored
     * draft — [reduceDetails] (a successful `GetAppDetails`) and
     * [reduceDraftChanged] (the full field map an `AppDesignDraftChanged`
     * carries) — and both clear `conflictRevision` in the same `copy`, so the
     * banner can never outlive a real reload. ([reduceDesignerRequested] builds
     * a fresh designer from template defaults, whose `conflictRevision` starts
     * null.) Both callers of this function — the budget in [pumpDraftEditQueue]
     * and the `failedConflictRefresh` arm in [reduce] — release the gate
     * precisely because neither of those snapshots is coming, so leaving the
     * field set would assert a reload that provably did not happen, over the
     * pre-conflict local values, and invite the user to keep editing on top of
     * them.
     */
    private fun releaseConflictRefreshUnanswered() {
        val appId = draftConflictRefresh?.appId ?: return
        draftConflictRefresh = null
        _uiState.update { state ->
            if (state.designer?.appId != appId || state.designer.conflictRevision == null) {
                state
            } else {
                state.copy(designer = state.designer.copy(conflictRevision = null))
            }
        }
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
        draftConflictRefresh = PendingConflictRefresh(event.appId)
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
                error(strings.resolve(R.string.local_apps_design_unsaved_retry, "设计尚未保存完成，请重试"))
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
     * The plan-confirmation screen's "返回修改" exit (local-apps#questionnaire,
     * Task 20): `cancel_design` (`awaiting_spec_confirmation -> collecting_spec`)
     * needs only the app id — unlike [confirmDesign] there is no interaction
     * id to read or drafts to drain first.
     *
     * Also requests a details refresh: `reduceDesignerRequested` set
     * `state.designer.values` to each field's bare DEFAULT the moment
     * `plan_ready` armed this gate (it has no other source of answers to
     * seed from — see its own doc), discarding whatever the user had
     * actually last saved from the DISPLAYED draft, even though the
     * engine's own `draft.fields` (what `cancel_design` reverts to editing)
     * was never touched. Landing back on the step form with every answer
     * visually reset to its default — while the real answers are still
     * intact server-side — is exactly the "check what the second call does
     * with the first call's result fed back in" class of bug: without this
     * refresh, the SECOND arrival at `collecting_spec` (via cancel, as
     * opposed to the FIRST, fresh-questionnaire arrival `reduceDesignerRequested`
     * was written for) would show wrong values despite the underlying state
     * being correct. `reduceDetails` overwrites `designer.values` from the
     * reply's authoritative `designFields` for the app currently pinned to
     * `state.designer` — exactly this one, since `destination` never leaves
     * `Designer(appId)` across the whole plan-confirm detour.
     */
    private fun cancelDesign(appId: String) {
        submit(ClientCommand.CancelAppDesign(appId))
        submit(ClientCommand.GetAppDetails(appId))
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
        // `prompt = null` replays the job unchanged. Android has no failure
        // composer yet (iOS grew one so a failed generation can be talked out
        // of rather than only re-run); when it does, the user's words go here.
        submit(ClientCommand.RetryAppGeneration(appId, null))
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
                error(
                    strings.resolve(
                        R.string.local_apps_error_bridge_unsupported_op,
                        "应用请求了不支持的 Bridge 操作：%1\$s",
                        message.operation,
                    ),
                )
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
                // `event.appId` is `Option<String>` on the wire and skipped when
                // absent, so a failure addressing no app is a shape the protocol
                // permits — and `null == null` would match a gate that is not
                // even raised, silently swallowing that failure's banner. Guard
                // the null explicitly, exactly as `failedConflictRefresh` below
                // already does. No engine path emits `app_id: None` with this
                // code today (the only two `None` handlers are `handle_list_apps`
                // and `handle_create_app`, neither of which can produce
                // `RevisionConflict`), so this is a guard, not a bug that was
                // reaching users.
                val recoveringConflict = event.code == AppErrorCodeDto.REVISION_CONFLICT &&
                    event.appId != null &&
                    event.appId == draftConflictRefresh?.appId
                // The details refresh a conflict is waiting on can fail outright —
                // the app was deleted from another surface, or its store is
                // corrupt — and a failure never reaches `reduceDetails`, the one
                // place that clears the gate. Release it here so the deterministic
                // case recovers at once instead of waiting out the budget above.
                // A non-conflict failure carrying that app id is the strongest
                // signal available: `AppOperationFailed` has no correlation id, so
                // this cannot be narrowed to the GetAppDetails that failed. Being
                // wrong is cheap and cannot lose an answer — the rejected patch is
                // already back on the queue, so an early release at worst resends
                // against the local revision and earns one more AppDesignConflict,
                // which re-raises this gate with a fresh deadline. REVISION_CONFLICT
                // is excluded because that is the failure that ACCOMPANIES the
                // conflict which raised the gate, not the refresh answering.
                val failedConflictRefresh = event.appId != null &&
                    event.appId == draftConflictRefresh?.appId &&
                    event.code != AppErrorCodeDto.REVISION_CONFLICT
                if (failedConflictRefresh) releaseConflictRefreshUnanswered()
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
                if (rejectedDraftPatch) draftEditInFlight = null
                if (rejectedDraftPatch || failedConflictRefresh) pumpDraftEditQueue()
                if (!recoveringConflict) error(event.message)
            }
            else -> Unit
        }
    }

    private fun reduceAppEvent(event: AppEventDto) {
        when (event) {
            // NOTE (local-apps#questionnaire, Task 5): `AppEventDto.AppTemplatesChanged`
            // was deleted (human-partner ruling: total removal of the static
            // template catalog) — no case for it exists on the wire enum
            // anymore, so there is nothing to match here.
            is AppEventDto.AppDetailsChanged -> reduceDetails(event.details)
            // The LLM finished (or discarded) authoring the questionnaire.
            // Stores the ordered steps under `questionnaires[appId]`, replacing
            // whatever was there — a full replacement, not a merge, because a
            // re-authored questionnaire (via `UpdateBrief`/`RetryQuestionnaire`)
            // may drop, add, or reorder steps entirely. No Android designer
            // surface renders this yet (Task 19's job); Task 18 only wires the
            // data layer.
            is AppEventDto.AppQuestionnaireChanged -> _uiState.update { state ->
                state.copy(questionnaires = state.questionnaires + (event.appId to event.steps.map { it.toUiStep() }))
            }
            // The LLM finished (or discarded) deriving the plan. `event.plan ==
            // null` means an answer edit voided a previously-derived plan —
            // mirrored here by REMOVING the entry rather than storing null, so
            // `state.plans[appId]` and "a plan exists" stay the same question.
            // No Android plan-confirmation screen renders this yet (Task 20's
            // job); Task 18 only wires the data layer.
            is AppEventDto.AppPlanChanged -> _uiState.update { state ->
                state.copy(
                    plans = if (event.plan != null) {
                        state.plans + (event.appId to event.plan.toUiPlan())
                    } else {
                        state.plans - event.appId
                    },
                )
            }
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
                        title = request.capability.authorizationTitle(strings),
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
        }
    }

    private fun reduceDetails(details: com.lingxi.code.bindings.AppDetailsDto) {
        val state = _uiState.value
        val prior = state.apps.firstOrNull { it.id == details.app.id }
        val app = details.app.toUiApp(
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
            current.copy(
                apps = apps.sortedByDescending { it.updatedAtMs },
                // A full details snapshot carries the questionnaire/plan too
                // (not just live `AppQuestionnaireChanged`/`AppPlanChanged`
                // events) — e.g. reopening an app after its designer/plan was
                // authored while this ViewModel was not collecting events.
                // Mirrors iOS's `LocalAppsStore.handle`'s `.appDetailsChanged`
                // arm.
                questionnaires = current.questionnaires + (app.id to details.questionnaire.map { it.toUiStep() }),
                plans = if (details.plan != null) {
                    current.plans + (app.id to details.plan.toUiPlan())
                } else {
                    current.plans - app.id
                },
                // The gate this snapshot answers goes down unconditionally, so
                // the banner it raised must go down with it — `conflictRevision`
                // is the sole input to "…已重新加载，请检查后继续。", and the only two
                // reducers that can make 已重新加载 true are this one and
                // [reduceDraftChanged], which clears the banner in the same
                // `copy` that replaces `values`. There is no more template
                // lookup gating this (Task 18: the questionnaire is looked up
                // separately, by app id, not carried on the designer), so this
                // snapshot always has enough to replace `values`.
                //
                // The second branch is new in Task 19: `openDesigner` no
                // longer waits for `open_app_designer`/`AppDesignerRequested`
                // to create `state.designer` for the common `collecting_spec`
                // path (see `openDesigner`'s doc — issuing that command there
                // would prematurely arm the plan-confirm gate). This details
                // reply, which `openDesigner` always requests, is therefore
                // the thing that creates the designer for that path — mirrors
                // iOS's `LocalAppsStore.handle`'s `.appDetailsChanged` arm,
                // which unconditionally creates/replaces
                // `designers[summary.id]`. Scoped to "the app this snapshot
                // is FOR is the one currently on the Designer destination" —
                // Android holds one designer slot, not iOS's per-app
                // dictionary — so a details reply for some other app (e.g. a
                // background refresh) cannot spuriously create a designer for
                // it.
                designer = when {
                    currentDesigner?.appId == app.id -> currentDesigner.copy(
                        revision = details.designRevision,
                        values = details.designFields.associate { it.fieldId to it.value.toUiValue() } + pendingValues,
                        conflictRevision = null,
                    )
                    currentDesigner == null && (current.destination as? LocalAppsDestination.Designer)?.appId == app.id ->
                        LocalAppDesigner(
                            appId = app.id,
                            appName = app.name,
                            revision = details.designRevision,
                            values = details.designFields.associate { it.fieldId to it.value.toUiValue() } + pendingValues,
                        )
                    else -> currentDesigner
                },
                generation = details.generationJob?.let { job ->
                    current.generation + (app.id to job.toUiGeneration(strings))
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
        if (draftConflictRefresh?.appId == app.id) {
            draftConflictRefresh = null
            pumpDraftEditQueue()
        }
    }

    private fun reduceGenerationJob(job: AppGenerationJobDto) {
        _uiState.update { state ->
            state.copy(
                generation = state.generation + (job.appId to job.toUiGeneration(strings)),
                destination = if (state.selectedAppId == job.appId) LocalAppsDestination.Preview(job.appId) else state.destination,
            )
        }
    }

    private fun reduceApps(event: ClientEvent.AppsChanged) {
        val oldIds = _uiState.value.apps.mapTo(hashSetOf()) { it.id }
        val apps = event.apps.map { record ->
            val prior = _uiState.value.apps.firstOrNull { it.id == record.id }
            record.toUiApp(fallbackRuntime = prior?.runtime)
        }.sortedByDescending { it.updatedAtMs }
        val liveIds = apps.mapTo(hashSetOf()) { it.id }
        // Release the draft machinery for apps that left the record set. Nothing
        // else does, and every path back INTO such an app already refuses it —
        // `openApp` and `reduceDesignerRequested` both return early for an id
        // absent from `state.apps`. Left in place, a designer still pinned to a
        // deleted app keeps sending patches the engine can only answer NOT_FOUND,
        // a code neither the ack arm nor `rejectedDraftPatch` may attribute, so
        // the in-flight watchdog re-queues them and the pump re-sends them for
        // the life of the ViewModel.
        //
        // `AppsChanged` is authoritative enough to prune on: the protocol defines
        // it as the FULL record set (`ClientEvent::AppsChanged.apps`), and all
        // three emitters build that set the same way — `AppService::announce_apps`
        // (the `ListApps` reply and every post-mutation snapshot),
        // `AppService::create_app`, and `AppService::delete_app`, each
        // snapshotting every record under the emission-order lock it took before
        // mutating. There is no partial
        // snapshot to mistake for a deletion — this reducer already replaces
        // `apps` wholesale and filters details/generation/previews on the same
        // set, so pruning here is no more trusting than what it already does.
        //
        // Discarding those edits is deliberate, and is NOT the "an edit that
        // leaves the in-flight slot must be re-queued" rule the watchdog and the
        // conflict arm obey. That rule exists so a confirm cannot ship a spec the
        // engine never received; here the designer whose `values` would keep the
        // confirm gate satisfied is torn down in the same breath, so
        // `confirmDesign` returns at its own `designer ?: return` and `drained`
        // is never reached for the dead app.
        //
        // What the user is LOOKING at decides whether a deletion is news —
        // never the mere existence of a `designer` object. Nothing but this
        // reducer clears `state.designer`: `navigateBack` and `openApp` move
        // only `destination`, so after the first designer of a session it stays
        // non-null and pinned for the rest of it. Keying a notice off it fired
        // the modal 应用操作失败 dialog on every ordinary library delete — and
        // the library card's ⋮ menu is the only delete surface there is, so
        // that was EVERY delete the user can perform — announcing a failure for
        // an operation that did exactly what was asked, and telling them they
        // had been 已返回应用列表 while they had never left it.
        val displacedFrom = _uiState.value.destination.appIdOnScreen()?.takeIf { it !in liveIds }
        // The one thing a deletion takes that the user cannot get back: answers
        // typed into the designer that the engine never received. The prune
        // below discards them deliberately (see above), so sample first.
        val strandedEdits = displacedFrom != null && (
            draftEditInFlight?.edit?.appId == displacedFrom ||
                draftEditQueue.any { it.appId == displacedFrom }
            )
        val queuePruned = draftEditQueue.removeAll { it.appId !in liveIds }
        val flightPruned = draftEditInFlight?.let { it.edit.appId !in liveIds } == true
        val gatePruned = draftConflictRefresh?.let { it.appId !in liveIds } == true
        if (flightPruned) draftEditInFlight = null
        if (gatePruned) draftConflictRefresh = null
        _uiState.update { state ->
            val destination = state.destination
            state.copy(
                apps = apps,
                details = state.details.filterKeys(liveIds::contains),
                generation = state.generation.filterKeys(liveIds::contains),
                previews = state.previews.filterKeys(liveIds::contains),
                designer = state.designer?.takeIf { it.appId in liveIds },
                // Pruned on the same set as everything above it, and for the
                // same reason: `SelectDetailsTab` and both generation reducers
                // steer off this id, so a stale one rebuilds a Details/Preview
                // destination for an app that no longer exists.
                selectedAppId = state.selectedAppId?.takeIf { it in liveIds },
                // Every per-app screen for a deleted app is now an empty shell
                // with no way forward — the designer above is null, and
                // `details`/`generation`/`previews` were just filtered out from
                // under Details and Preview. Leave the screen we emptied,
                // whichever of the three it was.
                destination = if (destination.appIdOnScreen()?.let { it !in liveIds } == true) {
                    LocalAppsDestination.Library
                } else {
                    destination
                },
                loading = false,
            )
        }
        // Navigation is not a failure, so the rescue above is silent. Losing an
        // answer the user typed is, and it is the only part of a deletion they
        // could not have predicted.
        if (strandedEdits) {
            error(
                strings.resolve(
                    R.string.local_apps_app_deleted_edits_lost,
                    "该应用已被删除，尚未保存的设计修改已丢失，已返回应用列表。",
                ),
            )
        }
        if (queuePruned || flightPruned || gatePruned) pumpDraftEditQueue()

        // Claims every app just created by [createFromBrief], to open its
        // designer. `templateKind` is gone (Task 18), so this matches on the
        // `brief` recorded at create time — `createFromBrief` sends `name`
        // empty on every create (Task 20), so the persisted record's `name`
        // is engine-derived and never equals anything this ViewModel sent;
        // `brief` is the only content still comparable. `it.id !in oldIds`
        // is still checked first and is still doing the real work: it is
        // what stops this from claiming a PRE-EXISTING app that merely
        // happens to share a brief with the one just created (two apps from
        // the same brief, e.g. a retry after a dropped reply) — brief alone
        // cannot tell those apart, since it is not unique.
        //
        // `pendingCreates` is a queue, not a scalar (Task 19: see its own
        // doc) — every freshly-appeared app is checked against it, oldest
        // pending entry first, and each match is consumed (removed) so a
        // later app cannot re-claim it. `openDesigner` is called once per
        // claimed app; the LAST one claimed in this batch is the one left
        // showing (single designer route), but every claim's `pendingCreates`
        // entry is still consumed and its `GetAppDetails`/gate-check still
        // runs, so a batch that claims two new apps at once does not leave
        // either one silently unclaimed. See `pendingCreates`' doc for the
        // residual ambiguity this cannot fully resolve without a real
        // correlation id on the wire.
        val newApps = apps.filter { it.id !in oldIds }
        newApps.forEach { candidate ->
            val matchIndex = pendingCreates.indexOfFirst { it == candidate.brief }
            if (matchIndex >= 0) {
                pendingCreates.removeAt(matchIndex)
                openDesigner(candidate.id)
            }
        }
    }

    private fun reduceDesignerRequested(event: ClientEvent.AppDesignerRequested) {
        val state = _uiState.value
        val app = state.apps.firstOrNull { it.id == event.appId } ?: return
        // `open_app_designer` is only legal from `collecting_spec`/
        // `generation_failed`, both of which require an authored
        // questionnaire — so by the time this event arrives,
        // `questionnaires[app.id]` is expected to already be populated by an
        // earlier `AppQuestionnaireChanged`. Defaulting to empty rather than
        // returning early keeps this reducer from silently dropping the
        // designer-open gate on a race (e.g. a details refresh landing before
        // the questionnaire does); the designer surface (Task 19) fills in
        // once `state.questionnaires[app.id]` catches up.
        val steps = state.questionnaires[app.id].orEmpty()
        _uiState.update {
            it.copy(
                selectedAppId = app.id,
                destination = LocalAppsDestination.Designer(app.id),
                designer = LocalAppDesigner(
                    appId = app.id,
                    appName = app.name,
                    revision = event.revision,
                    interactionId = event.interactionId,
                    values = steps.flatMap { step -> step.fields }
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
            val fields = state.questionnaires[designer.appId].orEmpty().flatMap { it.fields }.associateBy { it.id }
            val changes = event.patch.ops.mapNotNull { op ->
                val set = op as? AppDesignPatchOpDto.Set ?: return@mapNotNull null
                val after = set.value.toUiValue()
                LocalAppSuggestedChange(
                    fieldId = set.fieldId,
                    label = fields[set.fieldId]?.label ?: set.fieldId,
                    before = designer.values[set.fieldId]?.readable(strings).orEmpty(),
                    after = after.readable(strings),
                )
            }
            state.copy(
                designer = designer.copy(
                    suggestion = LocalAppSuggestion(
                        id = event.suggestionId,
                        basedOnRevision = event.basedOnRevision,
                        summary = event.patch.note ?: strings.resolve(
                            R.string.local_apps_suggestion_default_summary,
                            "建议调整 %1\$d 个字段",
                            changes.size,
                        ),
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

        fun factory(
            sourceFlow: StateFlow<ConversationSource>,
            strings: LocalAppsStrings = DefaultLocalAppsStrings,
        ): ViewModelProvider.Factory =
            object : ViewModelProvider.Factory {
                @Suppress("UNCHECKED_CAST")
                override fun <T : ViewModel> create(modelClass: Class<T>): T =
                    LocalAppsViewModel(sourceFlow, strings = strings) as T
            }
    }
}

/**
 * The app a destination is showing, or null on the two app-independent screens.
 * `reduceApps` needs it twice — once to decide whether the user is standing on a
 * screen a deletion just emptied, and once to move them off it — and an
 * exhaustive `when` is what makes a fourth per-app destination a compile error
 * rather than a screen the prune silently forgets.
 */
private fun LocalAppsDestination.appIdOnScreen(): String? = when (this) {
    LocalAppsDestination.Library -> null
    is LocalAppsDestination.Designer -> appId
    is LocalAppsDestination.Preview -> appId
    is LocalAppsDestination.Details -> appId
}

// NOTE (local-apps#questionnaire, Task 5): `AppTemplateKindDto`/`AppTemplateDto`
// and the `toUiTemplateKind`/`toBindingTemplateKind`/`toUiTemplate` conversions
// that used to live here were deleted from client-protocol (human-partner
// ruling: total removal of the static template catalog). `LocalAppTemplate`
// (the native UI model) was deleted in Task 18 alongside them.

// (local-apps#questionnaire, Task 19): AUTHORING_QUESTIONNAIRE /
// QUESTIONNAIRE_FAILED / PLANNING / PLAN_FAILED (core Task 3) now map 1:1 —
// `LocalAppDesignerScreen` renders each as its own intermediate/failure
// state (mirrors iOS's `LocalAppDesignerView.unavailableView(for:)`).
// Explicit branches, not an `else`, so this `when` still breaks the moment a
// real state is removed or renamed.
private fun AppWorkflowStateDto.toUiWorkflow(): LocalAppWorkflow = when (this) {
    AppWorkflowStateDto.AUTHORING_QUESTIONNAIRE -> LocalAppWorkflow.AuthoringQuestionnaire
    AppWorkflowStateDto.QUESTIONNAIRE_FAILED -> LocalAppWorkflow.QuestionnaireFailed
    AppWorkflowStateDto.COLLECTING_SPEC -> LocalAppWorkflow.CollectingSpec
    AppWorkflowStateDto.PLANNING -> LocalAppWorkflow.Planning
    AppWorkflowStateDto.PLAN_FAILED -> LocalAppWorkflow.PlanFailed
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

// NOTE (local-apps#questionnaire, Task 18): `AppRecordDto.template` was
// removed back in Task 2, and this conversion has not compiled since —
// `LocalAppItem` is updated here to carry `brief` (already required on the
// wire) instead of the deleted `templateKind`/`templateName`.
private fun AppRecordDto.toUiApp(
    runtime: LocalAppRuntime? = null,
    fallbackRuntime: LocalAppRuntime? = null,
): LocalAppItem = LocalAppItem(
    id = id,
    name = name,
    brief = brief,
    workflow = workflowState.toUiWorkflow(),
    runtime = runtime ?: fallbackRuntime ?: LocalAppRuntime(),
    updatedAtMs = updatedAtMs.toLong(),
)

/**
 * The LLM-authored questionnaire, one [ClientEvent.AppEvent]/
 * [AppEventDto.AppQuestionnaireChanged] step at a time. Ordered the way
 * `LocalAppTemplate.orderedSteps` used to order the static catalog's steps:
 * by declared `order`, id as tiebreak (mirrors iOS's
 * `LocalAppsProtocolAdapter.questionnaire`).
 */
private fun AppDesignStepDto.toUiStep(): LocalAppDesignStep = LocalAppDesignStep(
    id = id,
    order = order,
    title = title,
    description = description,
    fields = fields.map { it.toUiField() },
)

private fun AppDesignFieldDto.toUiField(): LocalAppDesignField = LocalAppDesignField(
    id = id,
    label = label,
    description = description,
    kind = fieldType.toUiFieldKind(),
    required = required,
    allowsCustom = allowsCustom,
    allowsDefer = allowsDefer,
    defaultValue = defaultValue?.toUiValue(),
    options = options.map { LocalAppFieldOption(value = it.value, label = it.label) },
)

private fun AppPlanDto.toUiPlan(): LocalAppPlan = LocalAppPlan(
    collections = collections.map { collection ->
        LocalAppCollectionSchema(
            id = collection.id,
            label = collection.label,
            fields = collection.fields.map { it.toUiDataField() },
            enabledByDefault = collection.enabledByDefault,
        )
    },
    capabilities = capabilities.map { it.toUiCapabilityKind() },
    domains = domains,
    summary = summary,
)

private fun AppCapabilityKindDto.toUiCapabilityKind(): LocalAppCapabilityKind = when (this) {
    AppCapabilityKindDto.DATA_MUTATION -> LocalAppCapabilityKind.DataMutation
    AppCapabilityKindDto.UI_CONTROL -> LocalAppCapabilityKind.UiControl
    AppCapabilityKindDto.NETWORK_DOMAIN -> LocalAppCapabilityKind.NetworkDomain
    AppCapabilityKindDto.RESTORE_CHECKPOINT -> LocalAppCapabilityKind.RestoreCheckpoint
    AppCapabilityKindDto.CAMERA -> LocalAppCapabilityKind.Camera
    AppCapabilityKindDto.PHOTO_LIBRARY -> LocalAppCapabilityKind.PhotoLibrary
    AppCapabilityKindDto.MICROPHONE -> LocalAppCapabilityKind.Microphone
    AppCapabilityKindDto.LOCATION -> LocalAppCapabilityKind.Location
    AppCapabilityKindDto.NOTIFICATIONS -> LocalAppCapabilityKind.Notifications
    AppCapabilityKindDto.LLM -> LocalAppCapabilityKind.Llm
    AppCapabilityKindDto.AGENT_NOTIFY -> LocalAppCapabilityKind.AgentNotify
}

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

private fun AppGenerationJobDto.toUiGeneration(
    strings: LocalAppsStrings = DefaultLocalAppsStrings,
): LocalAppGeneration = LocalAppGeneration(
    jobId = id,
    state = state.name.lowercase().replace('_', ' '),
    percent = percent?.toInt(),
    // A cold-launch staging timeout leaves this process with no local-app
    // runtime and no way to acquire one (see LocalAppRuntimeAssets), so every
    // generation fails at Building with an engine message that reads like a
    // build-time instruction. Say which state the runtime is actually in, on
    // the preview screen a failed job already routes the user to.
    detail = LocalAppRuntimeAssets.generationDetail(
        detail = detail,
        failed = state == AppGenerationJobStateDto.FAILED,
        strings = strings,
    ),
)

private fun AppCapabilityKindDto.authorizationTitle(strings: LocalAppsStrings): String = when (this) {
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
    AppCapabilityKindDto.LLM ->
        strings.resolve(R.string.local_apps_permission_llm, "允许应用调用 AI 模型？（会消耗你的模型用量）")
    AppCapabilityKindDto.AGENT_NOTIFY ->
        strings.resolve(R.string.local_apps_permission_agent_notify, "允许应用向对话助手发送事件？")
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
    LocalAppDesignValue.Deferred -> DesignValueDto.Deferred
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
    // (local-apps#questionnaire, Task 19): "let the model decide" — a real
    // answer, not an absence. `LocalAppDesignerScreen`'s 「由你决定」 chip
    // sends this back verbatim; see `LocalAppDesignValue.Deferred`'s doc.
    is DesignValueDto.Deferred -> LocalAppDesignValue.Deferred
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
