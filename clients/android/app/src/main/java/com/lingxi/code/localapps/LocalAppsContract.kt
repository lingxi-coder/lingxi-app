package com.lingxi.code.localapps

import android.content.Context
import androidx.compose.runtime.Immutable

/**
 * Resolves a localized string for localapps-package code that runs OUTSIDE a
 * `@Composable` body — [LocalAppsViewModel], [LocalAppRuntimeAssets] and
 * [LocalAppCodeBrowser] — and therefore cannot call `stringResource()`.
 * Mirrors `ConversationStrings` (conversation/ConversationSource.kt): the
 * fallback keeps every JVM unit test that constructs these types directly,
 * with no Android `Context`, asserting the same literal Chinese copy without
 * being touched; the production resolver ([localAppsStrings]) resolves the
 * REAL localized text through [Context.getString] so the app renders in the
 * user's selected language.
 */
fun interface LocalAppsStrings {
    fun resolve(id: Int, fallback: String, vararg args: Any): String
}

/** Test/no-Context fallback: the literal zh-Hans copy, `String.format`-ed. */
val DefaultLocalAppsStrings = LocalAppsStrings { _, fallback, args ->
    if (args.isEmpty()) fallback else String.format(java.util.Locale.getDefault(), fallback, *args)
}

/** Production resolver: real localized text via the app's (locale-wrapped) [Context]. */
fun localAppsStrings(context: Context): LocalAppsStrings =
    LocalAppsStrings { id, _, args -> context.getString(id, *args) }

@Immutable
data class LocalAppItem(
    val id: String,
    val name: String,
    /** One-line description the user gave at creation time. */
    val brief: String,
    val workflow: LocalAppWorkflow,
    val runtime: LocalAppRuntime = LocalAppRuntime(),
    val updatedAtMs: Long,
    val gitEnabled: Boolean = true,
    /**
     * Workspace directory relative to the engine data root (always
     * `apps/<id>/workspace`, forward slashes) — the app's conversation cwd.
     * Carried on the row (not only in [LocalAppDetails]) so scope switching
     * can resolve the workspace without waiting for a details snapshot.
     */
    val workspaceRel: String = "",
    /**
     * The app's pinned "init" session (bare uuid), when the record carries
     * one. Listed first in the session catalog with an 「初始化」 badge.
     */
    val initSessionId: String? = null,
    /**
     * Whether the app's scaffold has landed — the mirror of the wire
     * `AppRecordDto.scaffolded`.
     *
     * `false` is the empty SHELL the "+" button creates before the user has
     * confirmed anything: its [name] is the engine's non-localized `"untitled"`
     * placeholder and its [brief] is empty. Every surface that would render
     * either one branches on this through [localAppCardText] /
     * [localAppDisplayName], and the home-screen widget snapshot drops such an
     * app outright ([appsForWidgetSnapshot]).
     *
     * Deliberately has NO default, mirroring the wire field's "REQUIRED with no
     * serde default": a default would let a construction site silently mint a
     * shell as a formed app, and the placeholder would leak from wherever that
     * site feeds.
    */
    val scaffolded: Boolean,
    /** Last host-derived health snapshot; absent until details are loaded. */
    val runtimeProfileStatus: LocalAppRuntimeProfileStatus? = null,
    /** Last host-derived verification projection; absent until host emits it. */
    val mcpVerification: LocalAppVerificationSummary? = null,
    /** Last host-derived verification projection; absent until host emits it. */
    val uiVerification: LocalAppVerificationSummary? = null,
)

/** Title and subtitle a library card renders for one app. */
@Immutable
data class LocalAppCardText(val title: String, val subtitle: String)

/**
 * What a library card renders for [app] — the ONE draft predicate.
 *
 * A shell ([LocalAppItem.scaffolded] `== false`) has no identity yet: the
 * engine stored the non-localized `"untitled"` placeholder as its name and an
 * empty brief, so rendering either leaks something the user never chose. Every
 * other render point ([localAppDisplayName]) and the widget snapshot
 * ([appsForWidgetSnapshot]) branch on the same field, so there is no second
 * derivation of "is this a draft" to drift out of step.
 *
 * [draftTitle] / [draftSubtitle] are passed in (rather than resolved here) so
 * this stays a pure function that JVM unit tests can call with no `Context`.
 */
internal fun localAppCardText(
    app: LocalAppItem,
    draftTitle: String,
    draftSubtitle: String,
): LocalAppCardText = if (app.scaffolded) {
    LocalAppCardText(title = app.name, subtitle = app.brief)
} else {
    LocalAppCardText(title = draftTitle, subtitle = draftSubtitle)
}

/**
 * The name to show for [app] anywhere a single label is rendered — top bars,
 * the delete confirmation, the drawer's app-scope header.
 *
 * [fallback] covers "the catalog does not know this app (yet)", which is not
 * the same case as a draft and must not borrow the draft copy.
 */
internal fun localAppDisplayName(
    app: LocalAppItem?,
    draftTitle: String,
    fallback: String,
): String = when {
    app == null -> fallback
    !app.scaffolded -> draftTitle
    else -> app.name
}

/**
 * The apps that may appear in the home-screen widget snapshot.
 *
 * A shell is EXCLUDED, not relabelled: it has no scaffold, so its widget would
 * be an un-openable icon captioned with the `"untitled"` placeholder sitting on
 * the user's home screen. Excluding it also keeps the widget DTO free of a new
 * field it would otherwise need in order to say "this one is a draft".
 */
internal fun appsForWidgetSnapshot(apps: List<LocalAppItem>): List<LocalAppItem> =
    apps.filter(LocalAppItem::scaffolded)

/**
 * Publication state of an app — the client projection of the v3 wire
 * `AppWorkflowStateDto`.
 *
 * The Android bindings in this branch still decode the pre-Phase-8 `READY`
 * variant, so the reducer maps that legacy name to
 * [PublishedUnverified]. Once the shared DTO regeneration lands the same UI
 * state already accepts the final `published_*` names with no contract churn.
 */
enum class LocalAppWorkflow {
    Draft,
    PublishedUnverified,
    PublishedVerified,
}

val LocalAppWorkflow.isPublished: Boolean
    get() = this != LocalAppWorkflow.Draft

enum class LocalAppStatusBadgeKind {
    Draft,
    PublishedUnverified,
    PublishedVerified,
    VerificationPending,
    VerificationFailed,
    Error,
}

enum class LocalAppVerificationStatus {
    Pending,
    Passed,
    Failed,
    Unverified,
    Unavailable,
}

@Immutable
data class LocalAppVerificationSummary(
    val status: LocalAppVerificationStatus,
    val summary: String,
    val code: String? = null,
)

internal fun localAppStatusBadges(
    workflow: LocalAppWorkflow,
    runtimeError: String?,
    runtimeProfileStatus: LocalAppRuntimeProfileStatus?,
    mcpVerification: LocalAppVerificationSummary?,
    uiVerification: LocalAppVerificationSummary?,
): List<LocalAppStatusBadgeKind> {
    val badges = mutableListOf(
        when (workflow) {
            LocalAppWorkflow.Draft -> LocalAppStatusBadgeKind.Draft
            LocalAppWorkflow.PublishedUnverified -> LocalAppStatusBadgeKind.PublishedUnverified
            LocalAppWorkflow.PublishedVerified -> LocalAppStatusBadgeKind.PublishedVerified
        },
    )
    if (
        workflow == LocalAppWorkflow.PublishedUnverified ||
        mcpVerification?.status == LocalAppVerificationStatus.Pending ||
        uiVerification?.status == LocalAppVerificationStatus.Pending
    ) {
        badges += LocalAppStatusBadgeKind.VerificationPending
    }
    if (
        mcpVerification?.status == LocalAppVerificationStatus.Failed ||
        uiVerification?.status == LocalAppVerificationStatus.Failed
    ) {
        badges += LocalAppStatusBadgeKind.VerificationFailed
    }
    if (!runtimeError.isNullOrBlank() || runtimeProfileStatus in setOf(
            LocalAppRuntimeProfileStatus.DependenciesDirty,
            LocalAppRuntimeProfileStatus.CoreDependencyDrift,
            LocalAppRuntimeProfileStatus.RebuildRequired,
            LocalAppRuntimeProfileStatus.MigrationAvailable,
            LocalAppRuntimeProfileStatus.RuntimeBundleMissing,
            LocalAppRuntimeProfileStatus.RuntimeContractCorrupt,
        )
    ) {
        badges += LocalAppStatusBadgeKind.Error
    }
    return badges
}

enum class LocalAppRuntimeState { Stopped, Starting, Running, Stopping, Failed }

/**
 * Host-derived health of the app's pinned runtime profile. The wire values
 * are intentionally represented as a finite type so cards and details never
 * parse host error prose or invent a status from source files.
 */
enum class LocalAppRuntimeProfileStatus {
    Verified,
    DependenciesDirty,
    CoreDependencyDrift,
    RebuildRequired,
    MigrationAvailable,
    RuntimeBundleMissing,
    RuntimeContractCorrupt,
}

enum class LocalAppRuntimeMode { StaticExport, ViteStatic }

@Immutable
data class LocalAppRuntime(
    val state: LocalAppRuntimeState = LocalAppRuntimeState.Stopped,
    val mode: LocalAppRuntimeMode? = null,
    val url: String? = null,
    val detail: String? = null,
    val recovery: String? = null,
)

enum class LocalAppDataFieldType {
    Text,
    LongText,
    Integer,
    Decimal,
    Boolean,
    DateTime,
    Enum,
    ImageRef,
}

@Immutable
data class LocalAppDataField(
    val id: String,
    val label: String,
    val type: LocalAppDataFieldType,
    val required: Boolean,
    val options: List<String> = emptyList(),
)

@Immutable
data class LocalAppCollectionSchema(
    val id: String,
    val label: String,
    val fields: List<LocalAppDataField>,
    val enabledByDefault: Boolean,
)

@Immutable
data class LocalAppCheckpoint(
    val id: String,
    val label: String,
    val kind: String,
    val createdAtMs: Long,
)

@Immutable
data class LocalAppDetails(
    val appId: String,
    val workspaceRelativePath: String,
    val collections: List<LocalAppCollectionSchema> = emptyList(),
    val allowedDomains: List<String> = emptyList(),
    val checkpoints: List<LocalAppCheckpoint> = emptyList(),
    val runtime: LocalAppRuntime = LocalAppRuntime(),
    /** Host-derived profile health from the same detail snapshot. */
    val runtimeProfileStatus: LocalAppRuntimeProfileStatus? = null,
    /** Host-derived MCP verification projection from the same app lifecycle. */
    val mcpVerification: LocalAppVerificationSummary? = null,
    /** Host-derived UI verification projection from the same app lifecycle. */
    val uiVerification: LocalAppVerificationSummary? = null,
)

/**
 * One row of an app's workspace-scoped session catalog — the UI projection of
 * the wire `AppSessionRowDto`. [relativeTime] is humanized from the wire
 * `modified_rfc3339` at reduce time (same treatment as the drawer's global
 * [com.lingxi.code.model.SessionRow]).
 */
@Immutable
data class LocalAppSessionRow(
    val uuid: String,
    val title: String,
    val relativeTime: String,
    val messageCount: Int,
    /** True for the app's pinned init session — listed first with a badge. */
    val isInit: Boolean,
)

/**
 * The loaded (possibly partial) session catalog for one app. [nextOffset] is
 * the wire `next_offset`: non-null means another `ListAppSessions` page exists
 * (the 「加载更多」 affordance); null means the catalog is complete. [loaded]
 * distinguishes "no reply yet" (loading) from "replied empty".
 */
@Immutable
data class LocalAppSessionPage(
    val rows: List<LocalAppSessionRow> = emptyList(),
    val nextOffset: ULong? = null,
    val loaded: Boolean = false,
)

enum class LocalAppAuthorizationDecision { Deny, AllowOnce, AllowSession, AllowAlways }

@Immutable
data class LocalAppAuthorizationRequest(
    val requestId: String,
    val appId: String,
    val title: String,
    val reason: String,
    val isUiControl: Boolean,
    val allowsPersistentGrant: Boolean = true,
    val uiAction: LocalAppUiAutomationAction? = null,
)

enum class LocalAppRuntimeProfileFamily {
    ReactDom,
    Canvas2d,
    Three3d,
    Phaser2d,
    Babylon3d,
}

enum class LocalAppRuntimeProfileSurface {
    Dom,
    Canvas,
}

@Immutable
data class LocalAppRuntimeProfilePackage(
    val name: String,
    val version: String,
)

@Immutable
data class LocalAppRuntimeProfileOption(
    val family: LocalAppRuntimeProfileFamily,
    val revision: UInt,
    val contractSha256: String,
    val surface: LocalAppRuntimeProfileSurface,
    val corePackages: List<LocalAppRuntimeProfilePackage>,
    val cacheStatus: String,
    val downloadStatus: String,
    val available: Boolean,
    val reason: String? = null,
)

enum class LocalAppDependencyChangeKind {
    Add,
    Update,
    Remove,
}

@Immutable
data class LocalAppDependencyChange(
    val kind: LocalAppDependencyChangeKind,
    val packageName: String,
    val version: String?,
    val cacheStatus: String,
    val downloadStatus: String,
)

@Immutable
data class LocalAppDependencyChangeConfirmationRequest(
    val requestId: String,
    val appId: String,
    val reason: String,
    val changes: List<LocalAppDependencyChange>,
    val licenseRisk: String,
    val sbomRisk: String,
    val lifecycleScriptsBlocked: Boolean,
    val nativeAddonsBlocked: Boolean,
    val rollbackPolicy: String,
)

@Immutable
data class LocalAppPendingUiAction(
    val requestId: String,
    val appId: String,
    val action: LocalAppUiAutomationAction,
    val decision: LocalAppAuthorizationDecision,
)

@Immutable
data class LocalAppBridgeRequestKey(
    val appId: String,
    val requestId: String,
)

@Immutable
data class LocalAppBridgeResult(
    val requestId: String,
    val appId: String,
    val ok: Boolean,
    val payloadJson: String?,
    val error: String?,
    val errorCode: String? = null,
)

enum class LocalAppApprovalReceiptState {
    Pending,
    Expired,
    Superseded,
}

@Immutable
data class LocalAppApprovalDependency(
    val packageName: String,
    val version: String? = null,
    val downloadStatus: String? = null,
)

@Immutable
data class LocalAppApprovalInitialTool(
    val name: String,
    val summary: String,
    val permissionCeiling: String? = null,
)

@Immutable
data class LocalAppApprovalGate(
    val name: String,
    val status: LocalAppVerificationStatus,
    val available: Boolean,
    val detail: String? = null,
)

@Immutable
data class LocalAppApprovalToolSurface(
    val name: String,
    val title: String? = null,
    val description: String? = null,
    val inputSchemaJson: String,
    val outputSchemaJson: String? = null,
    val annotationsJson: String? = null,
    val executionJson: String? = null,
    val visibleMetaJson: String? = null,
    val semanticFlowJson: String,
    val permissionCeiling: String,
)

sealed interface LocalAppApprovalSheet {
    val appId: String
    val requestId: String
    val receiptId: String
    val state: LocalAppApprovalReceiptState
    val expiresAtMs: Long?
}

enum class LocalAppApprovalToolField {
    Name,
    Title,
    Description,
    InputSchema,
    OutputSchema,
    Annotations,
    Execution,
    VisibleMeta,
    SemanticFlow,
    PermissionCeiling,
}

@Immutable
data class LocalAppApprovalToolDiff(
    val name: String,
    val before: LocalAppApprovalToolSurface? = null,
    val after: LocalAppApprovalToolSurface? = null,
    val changedFields: List<LocalAppApprovalToolField> = emptyList(),
)

@Immutable
data class LocalAppCreateApprovalSheet(
    override val appId: String,
    override val requestId: String,
    override val receiptId: String,
    override val state: LocalAppApprovalReceiptState = LocalAppApprovalReceiptState.Pending,
    override val expiresAtMs: Long? = null,
    val appName: String,
    val brief: String,
    val templateName: String,
    val runtimeProfile: LocalAppRuntimeProfileOption,
    val reason: String,
    val rejectedCandidates: List<String> = emptyList(),
    val dependencies: List<LocalAppApprovalDependency> = emptyList(),
    val initialTools: List<LocalAppApprovalInitialTool> = emptyList(),
    val permissionCeilings: List<String> = emptyList(),
    val gates: List<LocalAppApprovalGate> = emptyList(),
) : LocalAppApprovalSheet

@Immutable
data class LocalAppMcpProposalApprovalSheet(
    override val appId: String,
    override val requestId: String,
    override val receiptId: String,
    override val state: LocalAppApprovalReceiptState = LocalAppApprovalReceiptState.Pending,
    override val expiresAtMs: Long? = null,
    val summary: String,
    val toolDiffs: List<LocalAppApprovalToolDiff> = emptyList(),
    val requiredChanges: List<String> = emptyList(),
    val excludedCapabilities: List<String> = emptyList(),
    val pendingGates: List<LocalAppApprovalGate> = emptyList(),
) : LocalAppApprovalSheet

@Immutable
data class LocalAppProfileApprovalSheet(
    override val appId: String,
    override val requestId: String,
    override val receiptId: String,
    override val state: LocalAppApprovalReceiptState = LocalAppApprovalReceiptState.Pending,
    override val expiresAtMs: Long? = null,
    val baseRevision: ULong,
    val currentRevision: ULong,
    val instructions: String,
    val reason: String,
) : LocalAppApprovalSheet

/**
 * Details tabs. [Sessions] is FIRST and the default: an app is a conversation
 * scope now, so its session catalog is the primary surface; the runtime
 * preview / data / code / history / permissions tabs stay reachable behind it.
 */
enum class LocalAppDetailsTab { Sessions, Preview, Data, Code, History, PermissionsLogs }

sealed interface LocalAppsDestination {
    data object Library : LocalAppsDestination
    data class Preview(val appId: String) : LocalAppsDestination
    data class Details(val appId: String, val tab: LocalAppDetailsTab = LocalAppDetailsTab.Sessions) : LocalAppsDestination
}

@Immutable
data class LocalAppsUiState(
    val loading: Boolean = true,
    val apps: List<LocalAppItem> = emptyList(),
    val destination: LocalAppsDestination = LocalAppsDestination.Library,
    val query: String = "",
    val details: Map<String, LocalAppDetails> = emptyMap(),
    /**
     * Per-app workspace-scoped session catalogs — the accumulated
     * `AppSessionsChanged` pages, init row pinned first. Feeds the Details
     * screen's Sessions tab and the drawer's active-app section.
     */
    val appSessions: Map<String, LocalAppSessionPage> = emptyMap(),
    val selectedAppId: String? = null,
    val selectedDetailsTab: LocalAppDetailsTab = LocalAppDetailsTab.Sessions,
    val pendingAuthorization: LocalAppAuthorizationRequest? = null,
    val pendingDependencyChangeConfirmation: LocalAppDependencyChangeConfirmationRequest? = null,
    val pendingApprovalSheet: LocalAppApprovalSheet? = null,
    val bridgeResults: Map<LocalAppBridgeRequestKey, LocalAppBridgeResult> = emptyMap(),
    val pendingUiAction: LocalAppPendingUiAction? = null,
    val error: String? = null,
    val distributionMode: LocalAppRuntimeMode,
) {
    /**
     * The library rows for the current [query].
     *
     * A draft is listed while the search box is empty and drops out as soon as
     * the user types: its stored name is the `"untitled"` placeholder, so
     * matching on it would both surface the placeholder indirectly (「为什么搜
     * unt 出来一张『新应用』卡片」) and pretend a shell has a searchable
     * identity it has not been given yet.
     */
    val filteredApps: List<LocalAppItem>
        get() = apps.filter { app ->
            query.isBlank() ||
                (app.scaffolded && app.name.contains(query.trim(), ignoreCase = true))
        }

    /** The app's live preview url — the runtime's loopback url once running. */
    fun previewUrl(appId: String): String? =
        apps.firstOrNull { it.id == appId }?.runtime?.url ?: details[appId]?.runtime?.url
}

sealed interface LocalAppsAction {
    data object Refresh : LocalAppsAction

    /**
     * The 「+」 button. Creates an empty SHELL app immediately and hands the
     * user into its own conversation — there is no form: the shape, the name
     * and the requirements are settled by talking to the agent, and
     * `LocalAppScaffold` lands the scaffold once the user confirms.
     */
    data object Create : LocalAppsAction
    data class Search(val query: String) : LocalAppsAction

    /**
     * Ask Android to pin a home-screen Widget for an EXISTING app.
     *
     * The permanent home for a request that used to live only as a checkbox
     * inside the create dialog. Deleting that dialog without this would quietly
     * remove the feature: nothing else in the app sets `pendingWidgetPin`.
     */
    data class RequestWidget(val appId: String) : LocalAppsAction
    data class OpenApp(val appId: String) : LocalAppsAction
    /**
     * (Re)load one page of the app's session catalog. `offset == null` asks
     * for the first page and REPLACES the cached rows on reply; a non-null
     * offset (the cached `nextOffset`) appends — the 「加载更多」 tap.
     */
    data class LoadAppSessions(val appId: String, val offset: ULong? = null) : LocalAppsAction
    data class StartRuntime(val appId: String) : LocalAppsAction
    data class StopRuntime(val appId: String) : LocalAppsAction
    data class DeleteApp(val appId: String) : LocalAppsAction
    data class ResetPermissions(val appId: String) : LocalAppsAction
    data class RestoreCheckpoint(val appId: String, val checkpointId: String) : LocalAppsAction
    data class BridgeRequest(val message: LocalAppBridgeMessage) : LocalAppsAction
    data class AcknowledgeBridgeResult(val appId: String, val requestId: String) : LocalAppsAction
    data class ResolveAuthorization(val decision: LocalAppAuthorizationDecision) : LocalAppsAction
    data class ResolveDependencyChangeConfirmation(val approved: Boolean) : LocalAppsAction
    data class ResolveApprovalSheet(val approved: Boolean) : LocalAppsAction
    data class UiActionHandled(
        val requestId: String,
        val resultJson: String?,
        val error: String?,
    ) : LocalAppsAction
    data class SelectDetailsTab(val tab: LocalAppDetailsTab) : LocalAppsAction

    /**
     * 「打开应用」 on the details page: push the full-bleed run surface.
     *
     * Exists because [LocalAppsViewModel.openFromWidget] used to be the ONLY
     * writer of [LocalAppsDestination.Preview], which made the whole run
     * experience — hidden host bar, floating run control, one-level pop —
     * reachable only by tapping a home-screen widget. iOS opens the same
     * surface from the app's detail page; this is Android's equivalent.
     */
    data class OpenRunSurface(val appId: String) : LocalAppsAction
    data object Back : LocalAppsAction
    data object DismissError : LocalAppsAction
}

/**
 * Catalog rows in display order: the pinned init row first, everything else in
 * the order the engine reported (modified-descending). Pure so the pinning
 * rule is unit-testable without a ViewModel.
 */
internal fun sessionRowsForDisplay(rows: List<LocalAppSessionRow>): List<LocalAppSessionRow> =
    rows.filter { it.isInit } + rows.filterNot { it.isInit }
