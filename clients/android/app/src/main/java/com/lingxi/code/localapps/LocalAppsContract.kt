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
)

/**
 * Workflow state of an app — mirrors the v3 wire `AppWorkflowStateDto`, which
 * collapsed the whole design/generation pipeline to `draft` / `ready`
 * (local apps are agent-driven now; there is no client-side designer).
 */
enum class LocalAppWorkflow { Draft, Ready }

enum class LocalAppRuntimeState { Stopped, Starting, Running, Stopping, Failed }

enum class LocalAppRuntimeMode { StaticExport, NextProduction }

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
    val uiAction: LocalAppUiAutomationAction? = null,
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
    val createName: String = "",
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
    val bridgeResults: Map<LocalAppBridgeRequestKey, LocalAppBridgeResult> = emptyMap(),
    val pendingUiAction: LocalAppPendingUiAction? = null,
    val error: String? = null,
    val distributionMode: LocalAppRuntimeMode,
) {
    val filteredApps: List<LocalAppItem>
        get() = apps.filter { app ->
            query.isBlank() || app.name.contains(query.trim(), ignoreCase = true)
        }

    /** The app's live preview url — the runtime's loopback url once running. */
    fun previewUrl(appId: String): String? =
        apps.firstOrNull { it.id == appId }?.runtime?.url ?: details[appId]?.runtime?.url
}

sealed interface LocalAppsAction {
    data object Refresh : LocalAppsAction
    data object Create : LocalAppsAction
    data class Search(val query: String) : LocalAppsAction
    data class ChangeCreateName(val name: String) : LocalAppsAction
    /** Creates a new app from a one-line brief. */
    data class CreateFromBrief(val brief: String, val gitEnabled: Boolean = true) : LocalAppsAction
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
    data class UiActionHandled(
        val requestId: String,
        val resultJson: String?,
        val error: String?,
    ) : LocalAppsAction
    data class SelectDetailsTab(val tab: LocalAppDetailsTab) : LocalAppsAction
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
