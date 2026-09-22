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
import com.lingxi.code.bindings.AppCreateModeDto
import com.lingxi.code.bindings.AppCreateOriginDto
import com.lingxi.code.bindings.AppDataFieldDto
import com.lingxi.code.bindings.AppDataFieldTypeDto
import com.lingxi.code.bindings.AppDependencyChangeConfirmationRequestDto
import com.lingxi.code.bindings.AppDependencyChangeKindDto
import com.lingxi.code.bindings.AppDetailsDto
import com.lingxi.code.bindings.AppEventDto
import com.lingxi.code.bindings.AppRecordDto
import com.lingxi.code.bindings.AppRuntimeDetailsDto
import com.lingxi.code.bindings.AppRuntimeModeDto
import com.lingxi.code.bindings.AppRuntimeProfileStatusDto
import com.lingxi.code.bindings.AppRuntimeStateDto
import com.lingxi.code.bindings.AppSessionKindDto
import com.lingxi.code.bindings.AppSessionRowDto
import com.lingxi.code.bindings.AppUiActionKindDto
import com.lingxi.code.bindings.AppUiRequestDto
import com.lingxi.code.bindings.AppWorkflowStateDto
import com.lingxi.code.bindings.ClientCommand
import com.lingxi.code.bindings.ClientEvent
import com.lingxi.code.bindings.LocalAppGateStatusDto
import com.lingxi.code.bindings.LocalAppMcpProposalApprovalRequestDto
import com.lingxi.code.bindings.LocalAppMcpToolDiffDto
import com.lingxi.code.bindings.LocalAppMcpToolFieldDto
import com.lingxi.code.bindings.LocalAppMcpToolSurfaceDto
import com.lingxi.code.bindings.LocalAppPluginErrorCodeDto
import com.lingxi.code.bindings.LocalAppVerificationStatusDto
import com.lingxi.code.bindings.LocalAppVerificationSummaryDto
import com.lingxi.code.bindings.PluginCommandDto
import com.lingxi.code.conversation.ConversationSource
import com.lingxi.code.localapps.widget.LocalAppWidgetSnapshotSync
import com.lingxi.code.localapps.widget.NoopLocalAppWidgetSnapshotSync
import com.lingxi.code.model.DefaultSessionCatalogStrings
import com.lingxi.code.model.SessionCatalog
import com.lingxi.code.model.SessionCatalogStrings
import com.lingxi.code.model.SessionMode
import com.lingxi.code.model.toUi
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
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.flow.receiveAsFlow
import java.util.UUID
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
    private val currentConversationId: () -> String? = { null },
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
     * Bumped every time [source] is rebound. Callbacks that outlive their own
     * submit capture this and refuse to touch UI state once it has moved on:
     * `submit` reports its failure asynchronously, so a failure lambda can
     * fire long after the source that raised the request is gone. Comparing
     * the ConversationSource instances instead would not be enough — the same
     * instance can legitimately be re-emitted, and a rebind still invalidates
     * everything that was in flight against it.
     */
    private var sourceGeneration = 0

    /**
     * Where a freshly created app hands the user off: into the app's OWN
     * conversation, whose cwd is the app workspace.
     *
     * [initSessionId] is null when the engine's best-effort init-session mint
     * failed. Landing on a fresh conversation is still correct — the SCOPE, not
     * the session, is what roots the agent in the app workspace.
     *
     * Carries no brief any more: the "+" button creates a shell with an EMPTY
     * brief, and the kickoff it sends is a placeholder-free sentence.
     */
    data class CreatedAppLanding(
        val appId: String,
        val initSessionId: String?,
    )

    /**
     * The `request_id` of the `CreateApp` this session sent and has not yet
     * seen resolve — the correlation key the whole hand-off hangs on.
     *
     * A one-shot boolean is NOT sufficient and was explicitly rejected: the
     * engine emits `AppCreated` for BOTH creation paths, so under a concurrent
     * agent-driven create a flag would claim someone else's record and hijack
     * the user into the wrong session. The previous code worked around that by
     * matching the brief EXACTLY, which this flow cannot do — a shell's brief is
     * the empty string, identical for every create ever made.
     *
     * Events whose `request_id` does not match this are IGNORED, including
     * events carrying no key at all (an agent-tool create, a backfill).
     */
    private var pendingCreateRequestId: String? = null

    /**
     * Stop-loss for [pendingCreateRequestId]; see [CREATE_RESULT_TIMEOUT_MS].
     * Cancelled whenever the pending create is cleared, so a create that
     * resolves normally cannot be "timed out" by a stale job afterwards.
     */
    private var pendingCreateTimeout: Job? = null

    /**
     * Whether the create identified by [pendingCreateRequestId] may leave the
     * library parked on the new app's Details page.
     *
     * [openApp] is the library's FALLBACK landing — a screen behind the
     * hand-off in case the scope switch is refused — and its only consumer is
     * the apps cover. A create started from the DRAWER runs with that cover
     * down, so the destination it writes is never rendered and never popped: it
     * survives on this Activity-scoped ViewModel until the user's next
     * unrelated 「打开应用库」, which would then open on this app's Details page
     * instead of the library. Same hazard iOS spells `armLibraryFallback`
     * (`LocalAppsStore.createShellApp`), handled the same way — only the
     * library's own 「+」 arms it.
     *
     * Recorded WITH the correlation key rather than read at event time from the
     * UI state: by the time `AppCreated` lands the cover may have been opened or
     * closed for unrelated reasons, and what decides this is where the create
     * STARTED.
     */
    private var pendingCreateArmsLibraryFallback = false
    private val widgetPinRequestChannel = Channel<String>(Channel.BUFFERED)
    val widgetPinRequests = widgetPinRequestChannel.receiveAsFlow()
    private val createdAppLandingChannel = Channel<CreatedAppLanding>(Channel.BUFFERED)

    /** Where to take the user once an app exists; see [CreatedAppLanding]. */
    val createdAppLandings = createdAppLandingChannel.receiveAsFlow()

    /**
     * Puts a landing BACK on the channel after its collector was cancelled
     * mid-body rather than having actually finished with it.
     *
     * `receiveAsFlow()` hands the element to the collector body before that
     * body runs, so a `repeatOnLifecycle(RESUMED)` cancellation partway
     * through (the retry loop's `delay`, or either `withTimeoutOrNull` wait —
     * all normal to hit if the user backgrounds mid hand-off, which can take
     * several seconds) drops the element for good: nothing re-delivers it,
     * and the app the user just created is stranded with no landing and no
     * retry. The collector's `catch (c: CancellationException)` calls this
     * before rethrowing, so the NEXT `RESUMED` re-collects the same landing.
     */
    fun rearmCreatedAppLanding(landing: CreatedAppLanding) {
        createdAppLandingChannel.trySend(landing)
    }

    /**
     * A kickoff message parked for [appId] because the freshly-switched
     * conversation had not reported ready within `RootScreen`'s own
     * `SESSION_READY_TIMEOUT_MS` wait.
     *
     * A [Channel] (as [createdAppLandingChannel] uses) is wrong here: this is
     * re-checked against every later state the conversation passes through
     * until it fires or its own stop-loss gives up, not handed to a single
     * collector once. [StateFlow] is what lets `RootScreen`'s consumer
     * re-observe the same value across a `repeatOnLifecycle(RESUMED)`
     * restart (e.g. the user backgrounds the app mid-wait) instead of losing
     * it the way a one-shot channel element would be lost to a cancelled
     * collector body.
     *
     * This is the "park it" half of iOS's `pendingInitKickoff`
     * (`RootView.swift`): that latch survives past its own switch-side wait
     * too, fired by whichever session-adoption event lands next in the same
     * scope, rather than dropping the brief the instant one bounded wait
     * expires.
     */
    private val _pendingAppKickoff = MutableStateFlow<String?>(null)

    /** The [appId] with a parked kickoff, or null; see [_pendingAppKickoff]. */
    val pendingAppKickoff: StateFlow<String?> = _pendingAppKickoff.asStateFlow()

    /** Arms (or re-arms) the park for [appId]; see [_pendingAppKickoff]. */
    fun armPendingAppKickoff(appId: String) {
        _pendingAppKickoff.value = appId
    }

    /**
     * Clears the park for [appId] — called by the consumer whether the
     * kickoff actually fired or its stop-loss gave up, either way exactly
     * once, so a later unrelated visit to this app's conversation can never
     * re-fire it.
     */
    fun clearPendingAppKickoff(appId: String) {
        _pendingAppKickoff.compareAndSet(appId, null)
    }

    /**
     * A created app whose init-session pin has not arrived yet.
     *
     * `AppCreated` is emitted inside the create transaction and the pin is
     * minted AFTERWARDS (announced by `AppRecordChanged`), so the record on the
     * create event NEVER carries one. Arming here and emitting on the record
     * update is what keeps the hand-off from being silently dropped.
     */
    private var landingAwaitingPin: CreatedAppLanding? = null

    /**
     * Stop-loss for [landingAwaitingPin]; see [PIN_WAIT_TIMEOUT_MS].
     *
     * `AppRecordChanged` is the ONLY thing that publishes a held landing, and
     * the engine emits it as a best-effort follow-up to the mint — so a mint
     * that never reports back (a crash between the two events, a source
     * dropped mid-handshake) left the landing held forever: no hand-off, no
     * error, the freshly created app simply never opened. Expiring publishes
     * the held landing WITH A NULL SESSION ID rather than raising an error:
     * the app record is not in question (`AppCreated` already committed it),
     * only the pin is, and a landing with no pin is an outcome the consumer
     * already handles — it opens a fresh conversation in the app's scope,
     * exactly the fallback a mint that failed outright produces. iOS spells
     * the same pair `armPinWaitTimeout` / `reportPinWaitTimedOut`.
     */
    private var pinWaitTimeout: Job? = null

    /**
     * Where tapping a DRAFT card in the library lands: the shell's own pinned
     * init conversation — the interview that is going to define it.
     *
     * [sessionId] is null when the engine's best-effort init-session mint
     * failed; a fresh conversation in the app's scope is still correct, for the
     * same reason as [CreatedAppLanding] — the SCOPE roots the agent in the
     * workspace, not the session.
     *
     * Deliberately a SEPARATE channel from [createdAppLandings]: that one's
     * consumer sends the kickoff prompt, and re-sending it every time the user
     * walks back into a half-finished interview would restart the interview.
     */
    data class DraftSessionLanding(
        val appId: String,
        val sessionId: String?,
    )

    private val draftSessionLandingChannel = Channel<DraftSessionLanding>(Channel.BUFFERED)

    /** Where to take the user when a draft card is tapped; see [DraftSessionLanding]. */
    val draftSessionLandings = draftSessionLandingChannel.receiveAsFlow()

    /** As [rearmCreatedAppLanding], for the draft-card hand-off's own channel. */
    fun rearmDraftSessionLanding(landing: DraftSessionLanding) {
        draftSessionLandingChannel.trySend(landing)
    }

    private val pendingCapabilityKinds = mutableMapOf<String, AppCapabilityKindDto>()
    private val queuedAuthorizations = ArrayDeque<LocalAppAuthorizationRequest>()
    private val queuedDependencyChangeConfirmations = ArrayDeque<LocalAppDependencyChangeConfirmationRequest>()
    private val queuedApprovalSheets = ArrayDeque<LocalAppApprovalSheet>()
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

    /**
     * The level BELOW the full-screen run surface — where [navigateBack] returns
     * when [LocalAppsDestination.Preview] is popped.
     *
     * Exists because [LocalAppsDestination] holds one destination rather than a
     * stack, so "pop one level" has no structure to read and must be RECORDED by
     * whoever pushes Preview. Every site that navigates into Preview must write
     * it, and must write it from live state via [originBelowPreview] — there is
     * exactly one such site today ([openFromWidget]).
     *
     * ⚠️ The initializer is a cold-start placeholder, not the policy. Recording
     * the same value the initializer already holds is indistinguishable from not
     * recording at all: the pop then behaves exactly like the constant it was
     * meant to replace, with nothing red to say so. [originBelowPreview] is
     * where the value actually comes from, and the paired tests
     * `the run surface returns to the details page it was pushed from` /
     * `a run surface pushed over the library still pops to the library` exist to
     * keep the two answers apart.
     */
    private var previewReturnDestination: LocalAppsDestination = LocalAppsDestination.Library

    /**
     * The level a run surface for [appId] is about to be pushed ON TOP OF —
     * read from the live destination at push time, which is the only moment it
     * is knowable.
     *
     * Two rules, and both of them earn their place:
     *
     * - A Details page for THIS app is a real level below: the user descended
     *   from it, and iOS's twin pops back to it (`path.append(.preview(appID))`
     *   from the detail route, so `dismiss()` lands on the detail page). This is
     *   reachable today — the widget deep link does not reset the destination
     *   (RootScreen.kt flips `showingApps` on in the same effect and calls
     *   [openFromWidget] directly), so tapping an app's widget while standing on
     *   that same app's Details page pushes the run surface straight over it.
     * - ANY OTHER destination collapses to [LocalAppsDestination.Library].
     *   A widget deep link arrives from OUTSIDE this surface, so a Details page
     *   for a DIFFERENT app underneath it is an unrelated leftover from a visit
     *   the user finished long ago; returning there would be worse than
     *   returning to the library, which is also where iOS's widget-seeded stack
     *   pops to (`path = [shouldOpenPreview ? .preview(id) : .details(id)]` in
     *   LocalAppsLibraryView.swift). The not-ready branch of [openFromWidget]
     *   lands on Details and pops to Library as well, so both widget outcomes
     *   still take exactly one back press to reach the library.
     */
    private fun originBelowPreview(appId: String): LocalAppsDestination {
        val current = _uiState.value.destination
        val below = when (current) {
            // A run surface REPLACES a run surface rather than stacking on one
            // (a widget tap arriving while one is already up), so the level
            // below is still whatever the surface being replaced recorded — not
            // that surface itself, which would make the pop a no-op.
            is LocalAppsDestination.Preview -> previewReturnDestination
            else -> current
        }
        return when (below) {
            is LocalAppsDestination.Details ->
                below.takeIf { it.appId == appId } ?: LocalAppsDestination.Library
            // Unreachable: `below` is either a non-Preview destination or a
            // previously recorded origin, and only Library/Details are ever
            // recorded. Named rather than folded into an `else` so a fourth
            // destination breaks this compile instead of silently picking one.
            is LocalAppsDestination.Preview -> LocalAppsDestination.Library
            LocalAppsDestination.Library -> LocalAppsDestination.Library
        }
    }

    init {
        webStorageCleanup.retryConfirmed()
        viewModelScope.launch {
            sourceFlow.collectLatest { bound ->
                source = bound
                // Requests submitted to the previous source can never resolve:
                // collectLatest cancels its event collector and RootScreen then
                // closes that engine. Do not let their offsets suppress the new
                // source's first-page requests.
                sessionRequestOffsets.clear()
                // The create claim does NOT survive a scope switch or a
                // reconnect. `AppCreated` is a one-shot event on the source
                // `collectLatest` just cancelled, so a create still in flight
                // can never resolve its claim here — and nothing else clears
                // it. Left armed, `pendingCreateRequestId` is a permanent latch
                // on an Activity-scoped ViewModel: every later create is refused
                // with `local_apps_error_create_in_progress`.
                //
                // Reporting it is the other half of the contract: after a
                // reconnect this client must NOT claim an unmatched event, so
                // the honest outcome is "the result is unknown, check the app
                // library" — the app usually IS there, which is why the copy
                // says to look rather than to retry.
                sourceGeneration += 1
                val abandonedCreate = pendingCreateRequestId != null
                // A sheet discarded below is just as much an unfinished
                // promise as an abandoned create: the user answered (or was
                // about to answer) a prompt whose engine is gone, and the
                // outcome is genuinely unknown. Captured BEFORE the clears.
                val abandonedSheet = _uiState.value.pendingApprovalSheet != null ||
                    queuedApprovalSheets.isNotEmpty()
                clearPendingCreate()
                landingAwaitingPin = null
                clearPinWaitTimeout()
                // A pending (or queued) approval sheet is a promise to resolve
                // ITS request against the source that raised it. That source is
                // gone the instant `collectLatest` moves past this point — a
                // resolve from here on would submit into `bound`, the NEW
                // source, using an old requestId it never issued (the same class
                // of bug `rejectSupersededApprovalSheet` would commit if called
                // here, which is why this does not route through it). Drop the
                // sheet and its queue outright rather than resolve into the
                // wrong engine.
                queuedApprovalSheets.clear()
                _uiState.update {
                    it.copy(
                        loading = true,
                        pendingApprovalSheet = null,
                        // Set INSIDE this update, not via `error(...)` after it:
                        // this same update clears `error`, so a message raised
                        // before it would be wiped and one raised after it would
                        // race the snapshot reply.
                        error = if (abandonedCreate || abandonedSheet) {
                            strings.resolve(
                                R.string.local_apps_creation_result_unknown,
                                "创建结果未知，请在应用库确认。",
                            )
                        } else {
                            null
                        },
                    )
                }
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
            // The library's own 「+」: the cover IS mounted, so it may arm the
            // fallback landing. See [pendingCreateArmsLibraryFallback].
            LocalAppsAction.Create -> createShellApp(armLibraryFallback = true)
            is LocalAppsAction.Search -> _uiState.update { it.copy(query = action.query) }
            is LocalAppsAction.RequestWidget -> requestWidgetPin(action.appId)
            is LocalAppsAction.OpenApp -> openAppFromLibrary(action.appId)
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
            is LocalAppsAction.ResolveDependencyChangeConfirmation -> resolveDependencyChangeConfirmation(action.approved)
            is LocalAppsAction.ResolveApprovalSheet -> resolveApprovalSheet(action.requestId, action.approved)
            is LocalAppsAction.UiActionHandled -> resolveCompletedUiAction(action)
            is LocalAppsAction.UpdateMcpGoal -> _uiState.update { state ->
                state.copy(
                    mcpDrafts = state.mcpDrafts + (action.appId to LocalAppMcpDraft(action.userGoal)),
                    mcpErrorByApp = state.mcpErrorByApp - action.appId,
                )
            }
            is LocalAppsAction.StartMcpAuthoring -> startManagedMcpAuthoring(action.appId)
            is LocalAppsAction.SetMcpEnabled -> setManagedMcpEnabled(action.appId, action.enabled)
            is LocalAppsAction.SetMcpToolEnabled -> setManagedMcpToolEnabled(
                action.appId,
                action.toolName,
                action.enabled,
            )
            is LocalAppsAction.SetMcpPinnedToConversation -> setManagedMcpPinnedToConversation(
                action.appId,
                action.pinned,
            )
            is LocalAppsAction.SelectDetailsTab -> _uiState.update { state ->
                val appId = state.selectedAppId ?: return@update state
                state.copy(
                    selectedDetailsTab = action.tab,
                    destination = LocalAppsDestination.Details(appId, action.tab),
                )
            }
            is LocalAppsAction.OpenRunSurface -> openRunSurface(action.appId)
            LocalAppsAction.Back -> navigateBack()
            LocalAppsAction.DismissError -> _uiState.update { it.copy(error = null) }
        }
    }

    /**
     * The two snapshot commands a freshly bound source is asked for.
     *
     * `GetManagedMcpInventory` also carries the REATTACH half of the native
     * approval flow: the engine re-announces every approval it is still
     * blocked on from this command's handler (`host.rs`'s
     * `PluginCommandDto::GetManagedMcpInventory` arm ->
     * `reemit_pending_native_approvals`). This ViewModel is Activity-scoped,
     * so an Activity destroyed while the engine stays alive headlessly loses
     * the pending create-confirmation sheet outright; this is the only channel
     * that brings it back. Dropping the command here — or sending it before
     * the event collector is subscribed — silently restores the old failure,
     * where the engine blocked for its whole approval timeout with the user
     * never seeing a prompt.
     */
    private suspend fun requestSnapshots(bound: ConversationSource) {
        runCatching {
            bound.submitClientCommand(ClientCommand.ListApps)
            bound.submitClientCommand(ClientCommand.PluginCommand(PluginCommandDto.GetManagedMcpInventory))
        }
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
     * The 「+」 button: create an empty SHELL app and hand the user into its own
     * conversation.
     *
     * Everything the old create form collected is now settled by talking to the
     * agent, so this sends the neutral values the engine itself documents for
     * that case:
     *
     * - `mode = SHELL` — the record is written with `scaffolded = false` and NO
     *   scaffold is laid down. `LocalAppScaffold` lands the shape later.
     * - `surface = null` — required by the wire in shell mode (`host.rs` fails
     *   a shell create that names one); the shape is not known yet.
     * - `name = ""` / `brief = ""` — the service derives nothing from either and
     *   stores its `"untitled"` placeholder, which no surface renders (see
     *   [localAppCardText]).
     * - `conversationId = null` — a LIBRARY-origin create binds no conversation:
     *   `AppCreateOrigin::conversation_binding` (`local_apps_bridge.rs`) returns
     *   `None` for `Library` regardless of what is sent, so passing the current
     *   session id here would be a value the engine provably discards.
     * - `gitEnabled = true` — the wire's own documented default
     *   (`default_git_version_control`), which is what the deleted checkbox
     *   started at.
     * - `workflowModel = null` — follow the session model; there is no picker
     *   any more and no other caller of that field on Android.
     *
     * [armLibraryFallback] says whether the apps cover is mounted to render the
     * Details page [openApp] would park it on; see
     * [pendingCreateArmsLibraryFallback]. Every caller states it explicitly —
     * there is no default — because getting it wrong is invisible until an
     * unrelated 「打开应用库」 lands on the wrong screen.
     */
    private fun createShellApp(armLibraryFallback: Boolean) {
        if (pendingCreateRequestId != null) {
            error(
                strings.resolve(
                    R.string.local_apps_error_create_in_progress,
                    "已有一个本地应用正在创建中，请稍候。",
                ),
            )
            return
        }
        if (source == null) {
            // `submit` returns silently when nothing is bound, which would make
            // the 「+」 button look dead. Say so instead — same treatment
            // `startRuntimeIfNeeded` gives an absent engine.
            error(
                strings.resolve(
                    R.string.local_apps_error_engine_unavailable,
                    "此构建未包含本地应用引擎。",
                ),
            )
            return
        }
        val requestId = UUID.randomUUID().toString()
        pendingCreateRequestId = requestId
        pendingCreateArmsLibraryFallback = armLibraryFallback
        // Armed BEFORE `submit`, alongside the key it mirrors: `submit`
        // launches into `viewModelScope` and the engine can answer from inside
        // that call, so publishing the flag afterwards would leave the button
        // live across the window this exists to cover.
        publishCreateInFlight(true)
        armCreateTimeout(requestId)
        submit(
            ClientCommand.CreateApp(
                name = "",
                origin = AppCreateOriginDto.LIBRARY,
                brief = "",
                gitEnabled = true,
                workflowModel = null,
                conversationId = null,
                surface = null,
                mode = AppCreateModeDto.SHELL,
                requestId = requestId,
            ),
        ) {
            // The command never reached the engine, so no event will ever carry
            // this key. Releasing here (rather than waiting out the timeout)
            // keeps the button usable; `submit` raises the failure itself.
            //
            // Keyed release: `submit`'s failure lambda runs asynchronously
            // (after the coroutine it launched fails), so by the time it fires
            // a NEWER create may already own `pendingCreateRequestId`. An
            // unkeyed `clearPendingCreate()` here would drop that newer claim
            // instead of this failed one.
            clearPendingCreateIfMatches(requestId)
        }
    }

    /**
     * The drawer's 「创建应用」 row: the same shell create, started with the apps
     * cover DOWN.
     *
     * A public function rather than a [LocalAppsAction], because
     * [LocalAppsAction] is the cover's own intent vocabulary — `onAction` is
     * only ever reached from `LocalAppsScreen` — and the drawer already talks to
     * this ViewModel directly the way [openLibrary] and [openFromWidget] do
     * (`RootScreen.kt`). It also keeps `LocalAppsAction.Create` a `data object`,
     * which is what the cover's two call sites pass.
     *
     * No landing logic here on purpose: the hand-off is [createdAppLandings],
     * already collected in `RootScreen.kt`, which switches the conversation into
     * the app's scope and sends `R.string.local_apps_kickoff`. This path adds
     * nothing to it — it only starts the create.
     *
     * `armLibraryFallback = false`: nothing is mounted to render [openApp]'s
     * Details destination, and leaving it written would hijack the next
     * 「打开应用库」. See [pendingCreateArmsLibraryFallback].
     *
     * Failures still reach the user: they land on `uiState.error`, which
     * `RootScreen` now presents itself while the cover is down.
     */
    fun createAppFromDrawer() {
        createShellApp(armLibraryFallback = false)
    }

    /**
     * Ask Android to pin a home-screen Widget for an existing, SCAFFOLDED app.
     *
     * Gated on `scaffolded`, not merely on existence: a shell is excluded from
     * the widget snapshot ([appsForWidgetSnapshot]), so a widget pinned for one
     * would render an empty tile bound to an id the snapshot never mentions.
     */
    private fun requestWidgetPin(appId: String) {
        val app = _uiState.value.apps.firstOrNull { it.id == appId } ?: return
        if (!app.scaffolded) return
        widgetPinRequestChannel.trySend(appId)
    }

    /**
     * Arm the stop-loss for one pending create.
     *
     * The create itself is a local file operation, but it also mints a pinned
     * session, so a cold device can be slow; [CREATE_RESULT_TIMEOUT_MS] is a
     * ceiling, not an expectation. Expiring only tells the user the outcome is
     * unknown — the app is very likely in the library either way — and above all
     * it releases the claim so the next event cannot be matched against a key
     * whose UI moment has passed.
     */
    private fun armCreateTimeout(requestId: String) {
        pendingCreateTimeout?.cancel()
        pendingCreateTimeout = viewModelScope.launch {
            delay(CREATE_RESULT_TIMEOUT_MS)
            // Re-check the key rather than trusting the job's own liveness: a
            // create that resolved and was immediately followed by another one
            // must not be timed out by the previous job.
            if (pendingCreateRequestId != requestId) return@launch
            // Released by hand rather than through `clearPendingCreate()`: this
            // coroutine IS the stop-loss, and that helper would cancel the job
            // currently executing this line.
            pendingCreateRequestId = null
            pendingCreateArmsLibraryFallback = false
            pendingCreateTimeout = null
            publishCreateInFlight(false)
            error(
                strings.resolve(
                    R.string.local_apps_creation_result_unknown,
                    "创建结果未知，请在应用库确认。",
                ),
            )
        }
    }

    /**
     * Arm the stop-loss for one landing still waiting on its init-session pin.
     *
     * Keyed by [appId] for the same reason [armCreateTimeout] re-checks its
     * request id: a second create can arm its own landing before this job
     * wakes, and timing THAT one out would publish a hand-off the engine is
     * still about to complete.
     */
    private fun armPinWaitTimeout(appId: String) {
        pinWaitTimeout?.cancel()
        pinWaitTimeout = viewModelScope.launch {
            delay(PIN_WAIT_TIMEOUT_MS)
            val armed = landingAwaitingPin?.takeIf { it.appId == appId } ?: return@launch
            landingAwaitingPin = null
            // Cleared by hand rather than through `clearPinWaitTimeout()`:
            // this coroutine IS the stop-loss, and that helper would cancel
            // the job currently executing this line.
            pinWaitTimeout = null
            // No error. The record exists and the SCOPE is what roots the
            // agent in the workspace; a landing with no pin opens a fresh
            // conversation there, which is the same fallback the engine's own
            // failed mint produces.
            createdAppLandingChannel.trySend(armed.copy(initSessionId = null))
        }
    }

    /**
     * Disarm the pin-wait stop-loss WITHOUT touching [landingAwaitingPin] --
     * every caller already knows how it is resolving the latch (a matching
     * `AppRecordChanged`, a rebind) and only needs the timer to stop.
     */
    private fun clearPinWaitTimeout() {
        pinWaitTimeout?.cancel()
        pinWaitTimeout = null
    }

    /**
     * The created-app hand-off ran out of retries: the record exists and the
     * landing was already taken off its one-shot channel, so nothing else will
     * carry the user into the new app's conversation.
     *
     * Reuses the abandoned-create copy above on purpose. From the user's side
     * the two situations are the same — the app was created and is in the
     * library, this client just could not take them to it — and a second string
     * would need a new key in the `clients/translations` JSON catalogs (the real
     * source behind the generated `strings.xml`) for a message that reads
     * identically.
     *
     * NOTE: do not write that path with a glob. Kotlin block comments NEST, so
     * a `slash-star` sequence inside this doc comment opens a second comment and
     * swallows the rest of the file — the closing delimiter here then closes
     * only the inner one. It costs 33 cascading "unresolved reference" errors
     * across three files and one real syntax error 1800 lines away.
     */
    fun reportCreatedAppLandingExhausted() {
        _uiState.update {
            it.copy(
                error = strings.resolve(
                    R.string.local_apps_creation_result_unknown,
                    "创建结果未知，请在应用库确认。",
                ),
            )
        }
    }

    /** Release the create claim and its stop-loss together — always both. */
    private fun clearPendingCreate() {
        pendingCreateRequestId = null
        // Cleared WITH the key it qualifies. Not a live bug fix — the flag is
        // only ever read under `pendingCreateRequestId != null`, and every
        // create writes it before arming the key — but the invariant it holds
        // up ("this is meaningful only alongside a pending key") is what makes
        // that argument checkable at a glance. The hand-rolled release inside
        // `armCreateTimeout` clears it for the same reason.
        pendingCreateArmsLibraryFallback = false
        pendingCreateTimeout?.cancel()
        pendingCreateTimeout = null
        publishCreateInFlight(false)
    }

    /**
     * Publish [LocalAppsUiState.createInFlight] — the OBSERVED twin of
     * [pendingCreateRequestId].
     *
     * The latch itself is a plain field, so no composition ever recomposes on
     * it. Without this twin the 「+」 button could only fake an in-flight window
     * around its own `submit` call, which spans the round trip to the engine
     * and NOT the ~30s until `AppCreated` / `AppOperationFailed` / the
     * stop-loss actually resolves the create — so the button re-enabled
     * immediately and the user's second tap was answered with
     * `local_apps_error_create_in_progress` instead of being prevented. iOS
     * spells the same twin `LocalAppsStore.isCreateInFlight`.
     *
     * Written ONLY beside a write to [pendingCreateRequestId] — the arm in
     * [createShellApp], [clearPendingCreate], and the hand-rolled release
     * inside [armCreateTimeout] (which must not call [clearPendingCreate]
     * because that cancels the very job executing it) — so it cannot drift
     * from the latch it mirrors.
     */
    private fun publishCreateInFlight(value: Boolean) {
        _uiState.update { it.copy(createInFlight = value) }
    }

    /**
     * As [clearPendingCreate], but only when [requestId] still owns the
     * claim. A `submit` failure callback can fire after the claim has already
     * moved on — the command errors out asynchronously, and by the time that
     * lands a NEWER create may have armed its own [pendingCreateRequestId].
     * An unkeyed release there would drop the newer claim instead of the
     * failed one, leaving that create's own timeout as the only thing left to
     * eventually free it. Every other release site is already keyed by its
     * own enclosing check (the event handlers compare `event.requestId`
     * before calling [clearPendingCreate]); this is the one release that
     * previously was not.
     */
    private fun clearPendingCreateIfMatches(requestId: String) {
        if (pendingCreateRequestId != requestId) return
        clearPendingCreate()
    }

    /**
     * Tapping a card in the library.
     *
     * A SHELL has no detail page worth showing — no brief, no surface, no
     * runtime — and the one thing the user wants from it is the conversation
     * that is going to define it. So a draft card RESUMES the app's pinned init
     * session (§D.3 「点击进 pin 会话而非预览」), which is what makes leaving an
     * interview half-finished and tapping back in land in the SAME
     * conversation. A formed app opens its Details as before.
     *
     * Deliberately NOT folded into [openApp]: the `AppCreated` handler calls
     * that directly while the record is still a shell whose pin has not been
     * minted yet, so routing it through here would start a FRESH session
     * without the kickoff and orphan the session the engine is about to pin —
     * the same race iOS's `openCreatedAppIfNeeded` documents.
     */
    private fun openAppFromLibrary(appId: String) {
        val app = _uiState.value.apps.firstOrNull { it.id == appId } ?: return
        if (app.scaffolded) {
            openApp(appId)
            return
        }
        draftSessionLandingChannel.trySend(
            DraftSessionLanding(appId = appId, sessionId = app.initSessionId),
        )
    }

    /**
     * Every FORMED app opens onto its Details screen with the Sessions tab
     * selected: an app is a conversation scope, so its session catalog is the
     * primary surface. The details snapshot and the first catalog page are
     * requested together.
     *
     * Still reachable for a shell, but only from the create path
     * ([openAppFromLibrary] routes a user's tap elsewhere): the freshly created
     * record needs a screen behind the hand-off in case the scope switch is
     * refused.
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
        if (!app.workflow.isPublished) {
            _uiState.update {
                it.copy(
                    selectedAppId = appId,
                    selectedDetailsTab = LocalAppDetailsTab.Sessions,
                    destination = LocalAppsDestination.Details(appId, LocalAppDetailsTab.Sessions),
                    error = strings.resolve(
                        R.string.local_apps_preview_not_ready,
                        "应用尚未准备好",
                    ),
                )
            }
            return
        }
        pushRunSurface(appId)
        if (autostart) startRuntimeIfNeeded(appId)
    }

    /**
     * The ONLY writer of [LocalAppsDestination.Preview], shared by the widget
     * entrance and the details page's 「打开应用」 so the
     * [previewReturnDestination] discipline has exactly one implementation.
     *
     * Read the origin from the LIVE destination, never a constant: a push while
     * the user is standing on this app's Details page must give that page back
     * on exit. Assigning a constant here — any constant — makes the line
     * indistinguishable from the property's own initializer and turns the whole
     * mechanism back into the collapse-to-Library it replaced. See
     * [originBelowPreview] for which origins survive and why.
     *
     * The assignment happens BEFORE the `update` that writes Preview,
     * deliberately: afterwards the destination it has to read is already gone.
     */
    private fun pushRunSurface(appId: String) {
        previewReturnDestination = originBelowPreview(appId)
        _uiState.update {
            it.copy(
                selectedAppId = appId,
                selectedDetailsTab = LocalAppDetailsTab.Preview,
                destination = LocalAppsDestination.Preview(appId),
            )
        }
    }

    /**
     * 「打开应用」 on the details page.
     *
     * Refuses a app that is not published with the same message
     * the widget path uses, rather than pushing a surface whose only content
     * would be the not-running placeholder.
     */
    private fun openRunSurface(appId: String) {
        val app = _uiState.value.apps.firstOrNull { it.id == appId } ?: return
        if (!app.workflow.isPublished) {
            error(strings.resolve(R.string.local_apps_preview_not_ready, "应用尚未准备好"))
            return
        }
        pushRunSurface(appId)
        startRuntimeIfNeeded(appId)
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
        if (sessionRequestOffsets.containsKey(appId) && sessionRequestOffsets[appId] == offset) return
        sessionRequestOffsets[appId] = offset
        submit(
            ClientCommand.ListAppSessions(appId = appId, offset = offset, limit = null),
            onFailure = { sessionRequestOffsets.remove(appId) },
        )
    }

    private fun startManagedMcpAuthoring(appId: String) {
        val goal = _uiState.value.mcpDraft(appId).userGoal.trim().ifBlank {
            _uiState.value.apps.firstOrNull { it.id == appId }?.brief?.trim().orEmpty()
        }
        if (goal.isBlank()) {
            setManagedMcpError(
                appId,
                strings.resolve(
                    R.string.local_apps_mcp_goal_required,
                    "请先描述希望 LLM 通过这个应用完成什么工作。",
                ),
            )
            return
        }
        val command = PluginCommandDto.StartLocalAppMcpAuthoring(
            appId = appId,
            userGoal = goal,
        )
        submitManagedMcpCommand(
            appId = appId,
            pendingMessage = strings.resolve(R.string.local_apps_mcp_pending_authoring, "正在生成 MCP 方案…"),
            command = command,
            refreshInventoryAfterSubmit = false,
        )
    }

    private fun setManagedMcpEnabled(appId: String, enabled: Boolean) {
        val managed = _uiState.value.managedMcp(appId)
        val revision = managed.settingsRevision ?: 0uL
        submitManagedMcpCommand(
            appId = appId,
            pendingMessage = strings.resolve(R.string.local_apps_mcp_pending_service, "正在更新 MCP 服务…"),
            command = PluginCommandDto.SetLocalAppMcpEnabled(
                appId = appId,
                enabled = enabled,
                expectedRevision = revision,
            ),
        )
    }

    private fun setManagedMcpToolEnabled(appId: String, toolName: String, enabled: Boolean) {
        val managed = _uiState.value.managedMcp(appId)
        val revision = managed.settingsRevision ?: 0uL
        submitManagedMcpCommand(
            appId = appId,
            pendingMessage = strings.resolve(R.string.local_apps_mcp_pending_tool, "正在更新工具开关…"),
            command = PluginCommandDto.SetLocalAppMcpToolEnabled(
                appId = appId,
                toolName = toolName,
                enabled = enabled,
                expectedRevision = revision,
            ),
        )
    }

    private fun setManagedMcpPinnedToConversation(appId: String, pinned: Boolean) {
        val conversationId = currentConversationId()?.takeUnless { it.isBlank() || it == "new" }
        if (conversationId == null) {
            setManagedMcpError(
                appId,
                strings.resolve(
                    R.string.local_apps_mcp_conversation_required,
                    "请先进入一个真实会话，再把这个应用的 MCP 暴露给当前对话。",
                ),
            )
            return
        }
        submitManagedMcpCommand(
            appId = appId,
            pendingMessage = strings.resolve(R.string.local_apps_mcp_pending_pin, "正在更新当前对话暴露状态…"),
            command = PluginCommandDto.SetLocalAppMcpConversationPinned(
                conversationId = conversationId,
                appId = appId,
                pinned = pinned,
            ),
        )
    }

    private fun submitManagedMcpCommand(
        appId: String,
        pendingMessage: String,
        command: PluginCommandDto,
        refreshInventoryAfterSubmit: Boolean = true,
    ) {
        _uiState.update { state ->
            state.copy(
                mcpPendingByApp = state.mcpPendingByApp + (appId to pendingMessage),
                mcpErrorByApp = state.mcpErrorByApp - appId,
            )
        }
        submit(
            onFailure = {
                clearManagedMcpPending(appId)
            },
        ) {
            it.submitClientCommand(ClientCommand.PluginCommand(command))
            if (refreshInventoryAfterSubmit) {
                it.submitClientCommand(ClientCommand.PluginCommand(PluginCommandDto.GetManagedMcpInventory))
            }
        }
    }

    private fun clearManagedMcpPending(appId: String) {
        _uiState.update { state ->
            state.copy(mcpPendingByApp = state.mcpPendingByApp - appId)
        }
    }

    private fun setManagedMcpError(appId: String, message: String) {
        _uiState.update { state ->
            state.copy(
                mcpPendingByApp = state.mcpPendingByApp - appId,
                mcpErrorByApp = state.mcpErrorByApp + (appId to message),
                error = message,
            )
        }
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
        val effectiveDecision = when {
            request.allowsPersistentGrant -> decision
            decision == LocalAppAuthorizationDecision.Deny -> decision
            else -> LocalAppAuthorizationDecision.AllowOnce
        }
        val bindingDecision = effectiveDecision.toBindingDecision()
        if (request.isUiControl) {
            if (effectiveDecision == LocalAppAuthorizationDecision.Deny || request.uiAction == null) {
                submit(
                    ClientCommand.ResolveAppUiRequest(
                        request.requestId,
                        bindingDecision,
                        null,
                        if (request.uiAction == null && effectiveDecision != LocalAppAuthorizationDecision.Deny) {
                            strings.resolve(R.string.local_apps_ui_action_missing_target, "UI 请求缺少有效目标或参数")
                        } else null,
                    ),
                )
            } else if (effectiveDecision == LocalAppAuthorizationDecision.AllowSession ||
                effectiveDecision == LocalAppAuthorizationDecision.AllowAlways
            ) {
                uiControlGrants[request.appId] = effectiveDecision
            }
        } else {
            val capability = pendingCapabilityKinds.remove(request.requestId)
            if (capability == AppCapabilityKindDto.UI_CONTROL && effectiveDecision != LocalAppAuthorizationDecision.Deny) {
                uiControlGrants[request.appId] = effectiveDecision
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
                    request.isUiControl && effectiveDecision != LocalAppAuthorizationDecision.Deny && request.uiAction != null
                ) {
                    LocalAppPendingUiAction(request.requestId, request.appId, request.uiAction, effectiveDecision)
                } else it.pendingUiAction,
                selectedAppId = if (request.isUiControl) request.appId else it.selectedAppId,
                destination = if (request.isUiControl && effectiveDecision != LocalAppAuthorizationDecision.Deny) {
                    LocalAppsDestination.Details(request.appId, LocalAppDetailsTab.Preview)
                } else it.destination,
                selectedDetailsTab = if (request.isUiControl && effectiveDecision != LocalAppAuthorizationDecision.Deny) {
                    LocalAppDetailsTab.Preview
                } else it.selectedDetailsTab,
            )
        }
    }

    private fun enqueueDependencyChangeConfirmation(request: LocalAppDependencyChangeConfirmationRequest) {
        if (_uiState.value.pendingDependencyChangeConfirmation == null) {
            _uiState.update { it.copy(pendingDependencyChangeConfirmation = request) }
            return
        }
        queuedDependencyChangeConfirmations.addLast(request)
    }

    private fun resolveDependencyChangeConfirmation(approved: Boolean) {
        val request = _uiState.value.pendingDependencyChangeConfirmation ?: return
        submit(
            ClientCommand.ResolveAppDependencyChangeConfirmation(
                requestId = request.requestId,
                approved = approved,
            ),
        )
        val next = queuedDependencyChangeConfirmations.removeFirstOrNull()
        _uiState.update { it.copy(pendingDependencyChangeConfirmation = next) }
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

    /**
     * Leave the current destination by popping exactly ONE level, the way iOS's
     * `dismiss()` pops one `NavigationStack` route.
     *
     * [LocalAppsDestination] is a single value rather than a stack, so the run
     * surface's "one level down" has to be recorded when it is entered — that is
     * [previewReturnDestination], written from live state by
     * [originBelowPreview]. Collapsing Preview straight to
     * [LocalAppsDestination.Library] discarded whatever screen the run surface
     * had been opened ON TOP of; iOS's twin returns to it (its detail route
     * pushes the run route with `path.append(.preview(appID))`, so `dismiss()`
     * lands back on the detail page).
     *
     * Details still pops to Library because Library IS the level below it: it is
     * pushed from a library card ([openApp]), from a create's fallback landing,
     * from the ui-control prompt, and from [openFromWidget]'s not-ready branch —
     * never from another Details page and never from the run surface.
     *
     * Reached only through [LocalAppsAction.Back], which three call sites
     * dispatch — RootScreen.kt's system-back `BackHandler`, the run surface's
     * RunPill exit button, and the Details top bar's back chevron (plus the
     * not-running run surface's own top bar, restored to mirror iOS). All four
     * mean the same thing, so all four go through this one function.
     */
    private fun navigateBack() {
        _uiState.update { state ->
            when (state.destination) {
                LocalAppsDestination.Library -> state
                is LocalAppsDestination.Preview -> {
                    val origin = previewReturnDestination
                    state.copy(
                        destination = origin,
                        // The tab travels with the page. `LocalAppDetailsScreen`
                        // renders `state.selectedDetailsTab`, NOT the
                        // destination's own `tab`, and pushing the run surface
                        // set that field to Preview — so restoring the origin
                        // alone would drop the user back on the page they came
                        // from with a tab they never chose, and leave the two
                        // fields disagreeing about the same screen.
                        selectedDetailsTab = when (origin) {
                            is LocalAppsDestination.Details -> origin.tab
                            else -> state.selectedDetailsTab
                        },
                    )
                }
                is LocalAppsDestination.Details ->
                    state.copy(destination = LocalAppsDestination.Library)
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
                // Every app-operation failure is user-visible; that is unchanged.
                error(event.message)
                // Releasing the create claim, though, is KEYED. The engine
                // echoes the originating `request_id` on failure precisely so
                // this client can tell its own failed create from any other
                // app operation's. A key-less failure (one the engine
                // synthesized with no originating request) releases NOTHING —
                // the old "any global failure disarms the create" rule was an
                // unkeyed claim in the other direction, and with two creation
                // paths live it would drop a still-valid claim on a failure
                // that had nothing to do with it. If the engine ever fails our
                // create without echoing the key, the 30s stop-loss is what
                // releases it.
                val failedRequestId = event.requestId
                if (failedRequestId != null && failedRequestId == pendingCreateRequestId) {
                    clearPendingCreate()
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
                val app = event.record.toUiApp(
                    fallbackRuntime = prior?.runtime,
                    runtimeProfileStatus = prior?.runtimeProfileStatus,
                )
                // RecordChanged may race the initial full catalog snapshot.
                // Upsert it and restore the canonical newest-first ordering so
                // an incremental init-session pin cannot be dropped or leave
                // the list sorted differently from AppsChanged.
                upsertApp(app)
                // The create handshake's second half. The engine announces this
                // record EITHER WAY — with the pin it minted, or without one
                // when the best-effort mint failed — so the hand-off fires in
                // both cases rather than waiting for something that never comes.
                landingAwaitingPin?.takeIf { it.appId == event.record.id }?.let { armed ->
                    landingAwaitingPin = null
                    // Hygiene, not the guarantee: the stop-loss re-checks the
                    // latch by app id before it publishes anything, so clearing
                    // the latch above is already what makes a second, pin-less
                    // hand-off impossible. This stops the job from sitting in
                    // the scheduler for the rest of its ten seconds. Break BOTH
                    // and `a pinned landing is not re-published by the pin
                    // stop-loss` goes red at two landings for one create.
                    clearPinWaitTimeout()
                    createdAppLandingChannel.trySend(
                        armed.copy(initSessionId = event.record.initSessionId),
                    )
                }
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
                            allowsPersistentGrant = true,
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
                        allowsPersistentGrant = request.capability.allowsPersistentGrant(),
                    ),
                )
            }
            is AppEventDto.AppDependencyChangeConfirmationRequested -> {
                enqueueDependencyChangeConfirmation(event.request.toUiDependencyChangeConfirmation())
            }
            is AppEventDto.ManagedMcpInventoryChanged -> reduceManagedMcpInventory(event.servers)
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
            is AppEventDto.AppCreated -> {
                // The engine names the record it just committed, for BOTH
                // create paths, and echoes the originating `request_id`.
                // Emitted after `AppsChanged`, so the catalog this opens into
                // already contains the record.
                //
                // Claim ONLY on an exact key match. A record created by an
                // agent elsewhere arrives here with a different key or none at
                // all; claiming it would open someone else's app and leave this
                // create with no landing.
                val createdRequestId = event.requestId
                val pending = pendingCreateRequestId
                if (createdRequestId != null && pending != null && createdRequestId == pending) {
                    // Read BEFORE `clearPendingCreate`, which resets it along
                    // with the key it belongs to.
                    val armsLibraryFallback = pendingCreateArmsLibraryFallback
                    clearPendingCreate()
                    // Only when a cover is mounted to render it. A drawer create
                    // skips this: `openApp` writes a Details destination that
                    // nothing would draw and nothing would pop, and the next
                    // 「打开应用库」 would open onto it. Skipping cannot cost the
                    // hand-off — the pin arrives on the engine's own pushed
                    // `AppEvent::RecordChanged` (`local_apps_bridge.rs`), not as
                    // a reply to the `GetAppDetails` this issues.
                    if (armsLibraryFallback) openApp(event.record.id)
                    // Arm the hand-off; the init-session pin is minted AFTER
                    // this event and arrives on `AppRecordChanged`.
                    landingAwaitingPin = CreatedAppLanding(
                        appId = event.record.id,
                        initSessionId = event.record.initSessionId,
                    )
                    armPinWaitTimeout(event.record.id)
                }
            }
            is AppEventDto.PluginStatusChanged -> Unit
            is AppEventDto.PluginInventoryChanged -> Unit
            is AppEventDto.CreateConfirmationRequested -> Unit
            is AppEventDto.McpProposalApprovalRequested -> {
                clearManagedMcpPending(event.request.appId)
                enqueueApprovalSheet(event.request.toUiMcpProposalApprovalSheet())
            }
            is AppEventDto.VerificationSummaryChanged -> reduceVerificationSummary(
                appId = event.appId,
                workflow = event.publicationState.toUiWorkflow(),
                mcpVerification = event.mcpVerification.toUiVerificationSummary(),
                uiVerification = event.uiVerification.toUiVerificationSummary(),
            )
            is AppEventDto.LocalAppOperationFailed -> {
                error(event.code.localizedPluginError(strings, event.message))
                val current = _uiState.value.pendingApprovalSheet
                if (current != null && current.requestId == event.requestId) {
                    shiftToNextApprovalSheet()
                }
            }
            is AppEventDto.AppProfileProposal -> enqueueApprovalSheet(
                LocalAppProfileApprovalSheet(
                    appId = event.proposal.appId,
                    requestId = event.proposal.approvalToken,
                    receiptId = event.proposal.approvalToken,
                    baseRevision = event.proposal.baseRevision,
                    currentRevision = event.proposal.currentRevision,
                    instructions = event.proposal.instructions,
                    reason = event.proposal.reason,
                ),
            )
        }
    }

    private fun enqueueApprovalSheet(sheet: LocalAppApprovalSheet) {
        val current = _uiState.value.pendingApprovalSheet
        if (current == null) {
            _uiState.update { it.copy(pendingApprovalSheet = sheet) }
            return
        }
        // A RE-EMISSION, not a supersession. The engine re-announces a pending
        // approval with its ORIGINAL request id (see `requestSnapshots`), so
        // the sheet the user is looking at can arrive a second time; falling
        // through to the supersede branch below would reject the very request
        // they are being asked about.
        if (current.requestId == sheet.requestId || current.receiptId == sheet.receiptId) return
        if (current.appId == sheet.appId) {
            rejectSupersededApprovalSheet(current)
            _uiState.update { it.copy(pendingApprovalSheet = sheet) }
            return
        }
        val queuedIndex = queuedApprovalSheets.indexOfFirst { queued ->
            queued.appId == sheet.appId && queued.requestId != sheet.requestId
        }
        if (queuedIndex >= 0) {
            rejectSupersededApprovalSheet(queuedApprovalSheets.removeAt(queuedIndex))
        }
        if (queuedApprovalSheets.any { it.requestId == sheet.requestId || it.receiptId == sheet.receiptId }) {
            return
        }
        queuedApprovalSheets.addLast(sheet)
    }

    private fun rejectSupersededApprovalSheet(sheet: LocalAppApprovalSheet) {
        when (sheet) {
            is LocalAppProfileApprovalSheet -> submit(
                ClientCommand.ResolveAppProfileProposal(
                    appId = sheet.appId,
                    approvalToken = sheet.receiptId,
                    approved = false,
                ),
            )
            is LocalAppMcpProposalApprovalSheet -> submit(
                ClientCommand.PluginCommand(
                    PluginCommandDto.ResolveMcpProposalApproval(
                        requestId = sheet.requestId,
                        approved = false,
                    ),
                ),
            )
        }
    }

    private fun resolveApprovalSheet(requestId: String, approved: Boolean) {
        val sheet = _uiState.value.pendingApprovalSheet ?: return
        // Refuse a stale resolution. A tap's action carries the id of the
        // sheet it was rendered against; if an in-place supersede or a queue
        // shift already swapped in a different sheet by the time the action
        // arrives, resolving `approved` into `sheet` (now unrelated to what
        // the user tapped) would approve/reject a request the user never saw.
        if (sheet.requestId != requestId) return
        // Restore the sheet on a failed submit rather than losing the user's
        // decision: if nothing has taken its place, resurface it immediately;
        // otherwise put it back at the head of the queue so it surfaces right
        // after whatever is now current, instead of clobbering it.
        val generationAtResolve = sourceGeneration
        val restoreOnFailure = {
            // Never resurrect a sheet the rebind handler deliberately dropped.
            // `submit` fails asynchronously, so the most likely reason this
            // runs at all is that the source went away — which is exactly when
            // the rebind handler has already cleared this sheet (and its
            // queue) because its requestId belongs to an engine that no longer
            // exists. Restoring here would re-prompt the user with a dead
            // request and submit an unknown id into the NEW source.
            if (sourceGeneration != generationAtResolve) {
                Unit
            } else if (_uiState.value.pendingApprovalSheet == null) {
                _uiState.update { it.copy(pendingApprovalSheet = sheet) }
            } else {
                queuedApprovalSheets.addFirst(sheet)
            }
        }
        when (sheet) {
            is LocalAppProfileApprovalSheet -> submit(
                ClientCommand.ResolveAppProfileProposal(
                    appId = sheet.appId,
                    approvalToken = sheet.receiptId,
                    approved = approved,
                ),
                onFailure = restoreOnFailure,
            )
            is LocalAppMcpProposalApprovalSheet -> submit(
                ClientCommand.PluginCommand(
                    PluginCommandDto.ResolveMcpProposalApproval(
                        requestId = sheet.requestId,
                        approved = approved,
                    ),
                ),
                onFailure = restoreOnFailure,
            )
        }
        shiftToNextApprovalSheet()
    }

    private fun shiftToNextApprovalSheet() {
        _uiState.update { it.copy(pendingApprovalSheet = queuedApprovalSheets.removeFirstOrNull()) }
    }

    private fun reduceVerificationSummary(
        appId: String,
        workflow: LocalAppWorkflow,
        mcpVerification: LocalAppVerificationSummary,
        uiVerification: LocalAppVerificationSummary,
    ) {
        _uiState.update { state ->
            val apps = state.apps.map { app ->
                if (app.id != appId) app else app.copy(
                    workflow = workflow,
                    mcpVerification = mcpVerification,
                    uiVerification = uiVerification,
                )
            }
            val details = state.details[appId]?.let { detail ->
                state.details + (
                    appId to detail.copy(
                        mcpVerification = mcpVerification,
                        uiVerification = uiVerification,
                    )
                )
            } ?: state.details
            val managedMcp = state.managedMcp[appId]?.let { managed ->
                state.managedMcp + (
                    appId to managed.copy(
                        mcpVerification = mcpVerification,
                        uiVerification = uiVerification,
                    )
                )
            } ?: state.managedMcp
            state.copy(apps = apps, details = details)
                .copy(managedMcp = managedMcp)
        }
    }

    private fun reduceManagedMcpInventory(servers: List<com.lingxi.code.bindings.ManagedLocalAppMcpServerDto>) {
        val byAppId = servers.associateBy { it.appId }
        _uiState.update { state ->
            state.copy(
                managedMcp = byAppId.mapValues { (_, server) -> server.toUiManagedMcpServer() },
                mcpPendingByApp = emptyMap(),
                mcpErrorByApp = state.mcpErrorByApp - byAppId.keys,
            )
        }
    }

    private fun reduceDetails(details: com.lingxi.code.bindings.AppDetailsDto) {
        val prior = _uiState.value.apps.firstOrNull { it.id == details.app.id }
        val runtimeProfileStatus = details.runtimeProfileStatus?.toUiRuntimeProfileStatus()
        val app = details.app.toUiApp(
            runtime = details.runtime.toUiRuntime(),
            fallbackRuntime = prior?.runtime,
            runtimeProfileStatus = runtimeProfileStatus,
            mcpVerification = prior?.mcpVerification,
            uiVerification = prior?.uiVerification,
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
                        runtimeProfileStatus = runtimeProfileStatus,
                        mcpVerification = prior?.mcpVerification,
                        uiVerification = prior?.uiVerification,
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
            mode = mode.toUi(),
            modifiedRfc3339 = modifiedRfc3339,
            nowEpochSeconds = nowEpochSeconds,
            strings = sessionStrings,
        )
        return LocalAppSessionRow(
            uuid = base.uuid,
            title = base.title,
            mode = base.mode,
            modifiedAtEpochSeconds = base.modifiedAtEpochSeconds,
            relativeTime = base.relativeTime,
            messageCount = base.messageCount,
            isInit = kind == AppSessionKindDto.INIT,
        )
    }

    private fun reduceApps(event: ClientEvent.AppsChanged) {
        val apps = event.apps.map { record ->
            val prior = _uiState.value.apps.firstOrNull { it.id == record.id }
            record.toUiApp(
                fallbackRuntime = prior?.runtime,
                runtimeProfileStatus = prior?.runtimeProfileStatus,
                mcpVerification = prior?.mcpVerification,
                uiVerification = prior?.uiVerification,
            )
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
                managedMcp = state.managedMcp.filterKeys(liveIds::contains),
                mcpDrafts = state.mcpDrafts.filterKeys(liveIds::contains),
                mcpPendingByApp = state.mcpPendingByApp.filterKeys(liveIds::contains),
                mcpErrorByApp = state.mcpErrorByApp.filterKeys(liveIds::contains),
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
        apps.asSequence()
            .map { it.id }
            .filter { appId ->
                appId !in _uiState.value.appSessions && !sessionRequestOffsets.containsKey(appId)
            }
            .forEach { appId -> requestSessions(appId, offset = null) }
        publishWidgetSnapshot()

        // No claim here any more.
        //
        // Identifying "the app I asked for" by diffing the catalog and matching
        // briefs was only ever an inference, and the deferred create flow
        // invalidates it outright: the agent rewrites the brief before it
        // creates anything, and minutes may pass, during which the user can
        // create something else. `AppEventDto.AppCreated` names the record the
        // engine just committed, so the answer arrives instead of being guessed.
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

    /**
     * Republish the home-screen widget snapshot.
     *
     * Shells are filtered out here, at the ONE place the snapshot is produced,
     * rather than at each of the four call sites — see [appsForWidgetSnapshot]
     * for why exclusion (not relabelling) is the right treatment.
     */
    private fun publishWidgetSnapshot() {
        widgetSnapshotSync.publish(appsForWidgetSnapshot(_uiState.value.apps))
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
        /**
         * How long a `CreateApp` may stay unresolved before this client stops
         * waiting for its `request_id` (iOS's `createResultTimeout`).
         *
         * A NEW constant, deliberately not anchored to the 20s
         * `IDENTITY_PROPOSAL_TIMEOUT_MS` it replaces: that one was sized for a
         * single model call and is deleted with the rest of the proposal
         * feature. 30s is sized for what a create actually does — local file
         * work, plus minting a pinned session (and, for a chat-origin create,
         * forking the source conversation's history), which can be slow on a
         * cold device.
         *
         * Expiring is stop-loss, not a verdict: the app has almost certainly
         * been created, which is why the copy sends the user to the library
         * instead of asking them to retry.
         */
        internal const val CREATE_RESULT_TIMEOUT_MS = 30_000L

        /**
         * How long a created app's landing waits for the engine's init-session
         * pin to arrive on `AppRecordChanged` before handing off without one
         * (iOS's `pinWaitTimeout`).
         *
         * Much shorter than [CREATE_RESULT_TIMEOUT_MS]: by this point the
         * record already exists and this is one local follow-up on it, not the
         * create itself. Expiring is not a failure — see [armPinWaitTimeout].
         */
        internal const val PIN_WAIT_TIMEOUT_MS = 10_000L
        private const val MAX_QUEUED_AUTHORIZATIONS = 8

        fun factory(
            sourceFlow: StateFlow<ConversationSource>,
            strings: LocalAppsStrings = DefaultLocalAppsStrings,
            sessionStrings: SessionCatalogStrings = DefaultSessionCatalogStrings,
            webStorageCleanup: LocalAppWebStorageCleanup = NoopLocalAppWebStorageCleanup,
            widgetSnapshotSync: LocalAppWidgetSnapshotSync = NoopLocalAppWidgetSnapshotSync,
            currentConversationId: () -> String? = { null },
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
                        currentConversationId = currentConversationId,
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
 * The generated Android bindings may still expose the pre-Phase-8 `READY`
 * enum name while the shared protocol branch converges on
 * `PUBLISHED_UNVERIFIED` / `PUBLISHED_VERIFIED`. Match on the stable enum name
 * string so this reducer accepts both shapes without a second UI refactor.
 */
private fun AppWorkflowStateDto.toUiWorkflow(): LocalAppWorkflow = when (this) {
    AppWorkflowStateDto.DRAFT -> LocalAppWorkflow.Draft
    AppWorkflowStateDto.PUBLISHED_UNVERIFIED -> LocalAppWorkflow.PublishedUnverified
    AppWorkflowStateDto.PUBLISHED_VERIFIED -> LocalAppWorkflow.PublishedVerified
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

private fun AppRuntimeProfileStatusDto.toUiRuntimeProfileStatus(): LocalAppRuntimeProfileStatus = when (this) {
    AppRuntimeProfileStatusDto.VERIFIED -> LocalAppRuntimeProfileStatus.Verified
    AppRuntimeProfileStatusDto.DEPENDENCIES_DIRTY -> LocalAppRuntimeProfileStatus.DependenciesDirty
    AppRuntimeProfileStatusDto.CORE_DEPENDENCY_DRIFT -> LocalAppRuntimeProfileStatus.CoreDependencyDrift
    AppRuntimeProfileStatusDto.REBUILD_REQUIRED -> LocalAppRuntimeProfileStatus.RebuildRequired
    AppRuntimeProfileStatusDto.MIGRATION_AVAILABLE -> LocalAppRuntimeProfileStatus.MigrationAvailable
    AppRuntimeProfileStatusDto.RUNTIME_BUNDLE_MISSING -> LocalAppRuntimeProfileStatus.RuntimeBundleMissing
    AppRuntimeProfileStatusDto.RUNTIME_CONTRACT_CORRUPT -> LocalAppRuntimeProfileStatus.RuntimeContractCorrupt
}

private fun AppRecordDto.toUiApp(
    runtime: LocalAppRuntime? = null,
    fallbackRuntime: LocalAppRuntime? = null,
    runtimeProfileStatus: LocalAppRuntimeProfileStatus? = null,
    mcpVerification: LocalAppVerificationSummary? = null,
    uiVerification: LocalAppVerificationSummary? = null,
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
    // The one hop where the wire's shell/formed fact becomes a UI fact. Every
    // draft branch in this module reads it from here and nowhere else — no
    // surface re-derives "is this a draft" by sniffing the name or the brief.
    scaffolded = scaffolded,
    runtimeProfileStatus = runtimeProfileStatus,
    mcpVerification = mcpVerification,
    uiVerification = uiVerification,
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
    AppCapabilityKindDto.DEPENDENCY_CHANGE ->
        strings.resolve(R.string.local_apps_permission_dependency_change, "允许应用更新依赖吗？")
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

private fun AppCapabilityKindDto.allowsPersistentGrant(): Boolean = when (this) {
    AppCapabilityKindDto.DEPENDENCY_CHANGE -> false
    else -> true
}

private fun LocalAppAuthorizationDecision.toBindingDecision(): AppAuthorizationDecisionDto = when (this) {
    LocalAppAuthorizationDecision.Deny -> AppAuthorizationDecisionDto.DENY
    LocalAppAuthorizationDecision.AllowOnce -> AppAuthorizationDecisionDto.ALLOW_ONCE
    LocalAppAuthorizationDecision.AllowSession -> AppAuthorizationDecisionDto.ALLOW_SESSION
    LocalAppAuthorizationDecision.AllowAlways -> AppAuthorizationDecisionDto.ALLOW_ALWAYS
}

private fun LocalAppVerificationStatusDto.toUiVerificationStatus(): LocalAppVerificationStatus = when (this) {
    LocalAppVerificationStatusDto.PENDING -> LocalAppVerificationStatus.Pending
    LocalAppVerificationStatusDto.PASSED -> LocalAppVerificationStatus.Passed
    LocalAppVerificationStatusDto.FAILED -> LocalAppVerificationStatus.Failed
    LocalAppVerificationStatusDto.UNVERIFIED -> LocalAppVerificationStatus.Unverified
    LocalAppVerificationStatusDto.UNAVAILABLE -> LocalAppVerificationStatus.Unavailable
}

private fun LocalAppVerificationSummaryDto.toUiVerificationSummary(): LocalAppVerificationSummary =
    LocalAppVerificationSummary(
        status = status.toUiVerificationStatus(),
        summary = summary,
        code = code,
    )

private fun com.lingxi.code.bindings.ManagedLocalAppMcpServerDto.toUiManagedMcpServer(): LocalAppManagedMcpServer {
    val reflectedEnabled = reflectBoolean("getEnabled")
    val reflectedStatus = reflectStatus("getStatus")
    val reflectedRevision = reflectULong("getSettingsRevision")
    val reflectedEnabledTools = reflectStringList("getEnabledTools").toSet()
    val reflectedPinned = reflectBoolean("getPinnedToCurrentConversation")
        ?: reflectBoolean("getConversationPinned")
        ?: false
    val reflectedWidget = reflectWidget("getWidget")
    val effectiveEnabled = reflectedEnabled ?: false
    val effectiveStatus = reflectedStatus ?: when {
        toolCount.toInt() == 0 -> LocalAppManagedMcpStatus.NeedsSetup
        effectiveEnabled -> LocalAppManagedMcpStatus.Enabled
        else -> LocalAppManagedMcpStatus.Disabled
    }
    val effectiveEnabledTools = if (reflectedEnabledTools.isEmpty() && effectiveEnabled) {
        tools.mapTo(linkedSetOf()) { it.name }
    } else {
        reflectedEnabledTools
    }
    return LocalAppManagedMcpServer(
        serverName = serverName,
        appId = appId,
        appName = appName,
        enabled = effectiveEnabled,
        status = effectiveStatus,
        settingsRevision = reflectedRevision,
        toolCount = toolCount.toInt(),
        authoringRevision = authoringRevision,
        mcpVerification = mcpVerification.toUiVerificationSummary(),
        uiVerification = uiVerification.toUiVerificationSummary(),
        tools = tools.map { tool ->
            LocalAppManagedMcpTool(
                name = tool.name,
                title = tool.title,
                description = tool.description,
                permissionCeiling = tool.permissionCeiling,
                enabled = tool.name in effectiveEnabledTools,
            )
        },
        enabledTools = effectiveEnabledTools,
        pinnedToCurrentConversation = reflectedPinned,
        widget = reflectedWidget,
    )
}

private fun Any.reflectBoolean(methodName: String): Boolean? =
    runCatching { javaClass.methods.firstOrNull { it.name == methodName }?.invoke(this) as? Boolean }.getOrNull()

private fun Any.reflectULong(methodName: String): ULong? =
    runCatching {
        when (val value = javaClass.methods.firstOrNull { it.name == methodName }?.invoke(this)) {
            is ULong -> value
            is UInt -> value.toULong()
            is Long -> value.toULong()
            is Int -> value.toULong()
            else -> null
        }
    }.getOrNull()

private fun Any.reflectStringList(methodName: String): List<String> =
    runCatching {
        @Suppress("UNCHECKED_CAST")
        (javaClass.methods.firstOrNull { it.name == methodName }?.invoke(this) as? List<Any?>)
            ?.mapNotNull { it?.toString()?.takeIf(String::isNotBlank) }
            .orEmpty()
    }.getOrDefault(emptyList())

private fun Any.reflectStatus(methodName: String): LocalAppManagedMcpStatus? =
    runCatching {
        javaClass.methods.firstOrNull { it.name == methodName }?.invoke(this)?.toString()?.let { raw ->
            when (raw.lowercase()) {
                "disabled" -> LocalAppManagedMcpStatus.Disabled
                "needs_setup", "needssetup" -> LocalAppManagedMcpStatus.NeedsSetup
                "authoring" -> LocalAppManagedMcpStatus.Authoring
                "enabled" -> LocalAppManagedMcpStatus.Enabled
                "needs_revalidation", "needsrevalidation" -> LocalAppManagedMcpStatus.NeedsRevalidation
                "error" -> LocalAppManagedMcpStatus.Error
                else -> null
            }
        }
    }.getOrNull()

private fun Any.reflectWidget(methodName: String): LocalAppManagedMcpWidget? =
    runCatching {
        val value = javaClass.methods.firstOrNull { it.name == methodName }?.invoke(this) ?: return@runCatching null
        val label = value.javaClass.methods.firstOrNull { it.name == "getLabel" }?.invoke(value) as? String
        val detail = (value.javaClass.methods.firstOrNull { it.name == "getSummary" }?.invoke(value) as? String)
            ?: (value.javaClass.methods.firstOrNull { it.name == "getResourceUri" }?.invoke(value) as? String)
        LocalAppManagedMcpWidget(available = true, label = label, detail = detail)
    }.getOrNull()

private fun LocalAppGateStatusDto.toUiApprovalGate(): LocalAppApprovalGate =
    LocalAppApprovalGate(
        // Carried, not dropped. `label` and `detail` arrive as fixed English
        // from `pending_verification_gates`, so `gateId` is the only field the
        // sheet can localize on — see `localAppGateLabelRes`.
        gateId = gateId,
        name = label,
        status = status.toUiVerificationStatus(),
        available = available,
        detail = detail,
    )

private fun LocalAppMcpToolSurfaceDto.toUiApprovalToolSurface(): LocalAppApprovalToolSurface =
    LocalAppApprovalToolSurface(
        name = name,
        title = title,
        description = description,
        inputSchemaJson = inputSchemaJson,
        outputSchemaJson = outputSchemaJson,
        annotationsJson = annotationsJson,
        executionJson = executionJson,
        visibleMetaJson = visibleMetaJson,
        semanticFlowJson = semanticFlowJson,
        permissionCeiling = permissionCeiling,
    )

private fun LocalAppMcpToolFieldDto.toUiApprovalToolField(): LocalAppApprovalToolField = when (this) {
    LocalAppMcpToolFieldDto.NAME -> LocalAppApprovalToolField.Name
    LocalAppMcpToolFieldDto.TITLE -> LocalAppApprovalToolField.Title
    LocalAppMcpToolFieldDto.DESCRIPTION -> LocalAppApprovalToolField.Description
    LocalAppMcpToolFieldDto.INPUT_SCHEMA -> LocalAppApprovalToolField.InputSchema
    LocalAppMcpToolFieldDto.OUTPUT_SCHEMA -> LocalAppApprovalToolField.OutputSchema
    LocalAppMcpToolFieldDto.ANNOTATIONS -> LocalAppApprovalToolField.Annotations
    LocalAppMcpToolFieldDto.EXECUTION -> LocalAppApprovalToolField.Execution
    LocalAppMcpToolFieldDto.VISIBLE_META -> LocalAppApprovalToolField.VisibleMeta
    LocalAppMcpToolFieldDto.SEMANTIC_FLOW -> LocalAppApprovalToolField.SemanticFlow
    LocalAppMcpToolFieldDto.PERMISSION_CEILING -> LocalAppApprovalToolField.PermissionCeiling
}

private fun LocalAppMcpToolDiffDto.toUiApprovalToolDiff(): LocalAppApprovalToolDiff =
    LocalAppApprovalToolDiff(
        name = name,
        before = before?.toUiApprovalToolSurface(),
        after = after?.toUiApprovalToolSurface(),
        changedFields = changedFields.map(LocalAppMcpToolFieldDto::toUiApprovalToolField),
    )

private fun LocalAppMcpProposalApprovalRequestDto.toUiMcpProposalApprovalSheet(): LocalAppMcpProposalApprovalSheet =
    LocalAppMcpProposalApprovalSheet(
        appId = appId,
        requestId = requestId,
        // r1-backlog-native-confirmation-13 removed the wire `receipt`: it never
        // had a producer, so these three were ALWAYS the `?:` fallback arm.
        // `receiptId` is a non-defaulted member of `LocalAppApprovalSheet` and is
        // read live (sheet de-duplication, and `approvalToken = sheet.receiptId`),
        // so it is replaced, not dropped.
        receiptId = requestId,
        state = LocalAppApprovalReceiptState.Pending,
        expiresAtMs = null,
        summary = summary,
        toolDiffs = toolDiffs.map(LocalAppMcpToolDiffDto::toUiApprovalToolDiff),
        requiredChanges = requiredFlowChanges,
        excludedCapabilities = excludedCapabilities,
        pendingGates = pendingGates.map(LocalAppGateStatusDto::toUiApprovalGate),
    )

private fun LocalAppPluginErrorCodeDto.localizedPluginError(
    strings: LocalAppsStrings,
    fallback: String,
): String = when (this) {
    LocalAppPluginErrorCodeDto.PLUGIN_DISABLED ->
        strings.resolve(R.string.local_apps_error_plugin_disabled, fallback)
    LocalAppPluginErrorCodeDto.BUILTIN_BUNDLE_UNAVAILABLE ->
        strings.resolve(R.string.local_apps_error_builtin_bundle_unavailable, fallback)
    LocalAppPluginErrorCodeDto.TEMPLATE_UNAVAILABLE ->
        strings.resolve(R.string.local_apps_error_template_unavailable, fallback)
    LocalAppPluginErrorCodeDto.PROPOSAL_INVALID ->
        strings.resolve(R.string.local_apps_error_proposal_invalid, fallback)
    LocalAppPluginErrorCodeDto.CATALOG_STALE ->
        strings.resolve(R.string.local_apps_error_catalog_stale, fallback)
    LocalAppPluginErrorCodeDto.ACTIVE_STATE_CORRUPT ->
        strings.resolve(R.string.local_apps_error_active_state_corrupt, fallback)
    LocalAppPluginErrorCodeDto.REVISION_CONFLICT ->
        strings.resolve(R.string.local_apps_error_catalog_stale, fallback)
    LocalAppPluginErrorCodeDto.INVALID_MCP_SETTINGS ->
        strings.resolve(R.string.local_apps_error_proposal_invalid, fallback)
    LocalAppPluginErrorCodeDto.MCP_AUTHORING_REQUIRED ->
        strings.resolve(R.string.local_apps_error_mcp_authoring_required, fallback)
    LocalAppPluginErrorCodeDto.REPAIR_BUDGET_EXHAUSTED ->
        strings.resolve(R.string.local_apps_error_repair_budget_exhausted, fallback)
    LocalAppPluginErrorCodeDto.EXPOSURE_CAPACITY_REACHED ->
        strings.resolve(R.string.local_apps_error_exposure_capacity_reached, fallback)
}

private fun AppDependencyChangeKindDto.toUiDependencyChangeKind(): LocalAppDependencyChangeKind = when (this) {
    AppDependencyChangeKindDto.ADD -> LocalAppDependencyChangeKind.Add
    AppDependencyChangeKindDto.UPDATE -> LocalAppDependencyChangeKind.Update
    AppDependencyChangeKindDto.REMOVE -> LocalAppDependencyChangeKind.Remove
}

private fun AppDependencyChangeConfirmationRequestDto.toUiDependencyChangeConfirmation(): LocalAppDependencyChangeConfirmationRequest =
    LocalAppDependencyChangeConfirmationRequest(
        requestId = requestId,
        appId = appId,
        reason = reason,
        changes = changes.map { change ->
            LocalAppDependencyChange(
                kind = change.kind.toUiDependencyChangeKind(),
                packageName = change.`package`,
                version = change.version,
                cacheStatus = change.cacheStatus,
                downloadStatus = change.downloadStatus,
            )
        },
        licenseRisk = licenseRisk,
        sbomRisk = sbomRisk,
        lifecycleScriptsBlocked = lifecycleScriptsBlocked,
        nativeAddonsBlocked = nativeAddonsBlocked,
        rollbackPolicy = rollbackPolicy,
    )

internal fun AppUiRequestDto.toUiAutomationAction(): LocalAppUiAutomationAction? {
    val qaEnvelope = parseLocalAppQaEnvelope(value, requestId)
    if (isLocalAppQaRequestId(requestId) && qaEnvelope == null) return null
    val actionValue = if (qaEnvelope == null) value else qaEnvelope.actionValue
    val uiTarget = target?.toUiTarget()
    val parsed = when (action) {
        AppUiActionKindDto.INSPECT -> LocalAppUiAutomationAction.Inspect
        AppUiActionKindDto.CLICK -> uiTarget?.let(LocalAppUiAutomationAction::Click)
        AppUiActionKindDto.FILL -> uiTarget?.let { LocalAppUiAutomationAction.Fill(it, actionValue.orEmpty()) }
        AppUiActionKindDto.SELECT -> uiTarget?.let { LocalAppUiAutomationAction.Select(it, actionValue.orEmpty()) }
        AppUiActionKindDto.TOGGLE -> uiTarget?.let { LocalAppUiAutomationAction.Toggle(it, actionValue.toBoolean()) }
        AppUiActionKindDto.SCROLL -> actionValue.orEmpty().split(',', limit = 2).let { parts ->
            LocalAppUiAutomationAction.Scroll(
                x = parts.getOrNull(0)?.trim()?.toIntOrNull() ?: 0,
                y = parts.getOrNull(1)?.trim()?.toIntOrNull() ?: 0,
            )
        }
        // `inspect` hands the agent back absolute loopback urls, and the tool
        // schema constrains `value` no further. Origin is enforced in the
        // WebView, the only layer that knows the live one.
        AppUiActionKindDto.NAVIGATE ->
            actionValue?.takeIf { it.startsWith('/') || it.contains("://") }
                ?.let(LocalAppUiAutomationAction::Navigate)
        AppUiActionKindDto.BACK -> LocalAppUiAutomationAction.Back
        AppUiActionKindDto.RELOAD -> LocalAppUiAutomationAction.Reload
        // The opaque `value` carries an optional crop request
        // (`{"rect":{"x","y","width","height"}}`) straight through; the
        // WebView layer parses it, clamps it to the real viewport, and
        // reports what it actually captured back as `capture_rect`.
        AppUiActionKindDto.CAPTURE_VIEW -> LocalAppUiAutomationAction.CaptureView(actionValue)
        // `"x,y"` / `"x,y,phase"`, the same comma-packed `value` convention
        // SCROLL already uses. The wire variant is fieldless on purpose: a
        // data-carrying uniffi variant renders this enum as a Kotlin sealed
        // class and renames every existing constant.
        AppUiActionKindDto.POINTER -> actionValue.orEmpty().split(',').map { it.trim() }.let { parts ->
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
        AppUiActionKindDto.KEY -> actionValue.orEmpty().trim().let { raw ->
            val comma = raw.lastIndexOf(',')
            val tail = if (comma >= 0) raw.substring(comma + 1).trim().lowercase() else ""
            val hasPhase = tail in setOf("press", "down", "up")
            val key = if (hasPhase) raw.substring(0, comma).trim() else raw
            if (key.isEmpty()) null else LocalAppUiAutomationAction.Key(key, if (hasPhase) tail else "press")
        }
    }
    return if (qaEnvelope != null && parsed != null) {
        LocalAppUiAutomationAction.Qa(qaEnvelope.expectedRuntimeUrl, parsed)
    } else {
        parsed
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
