package com.lingxi.code.localapps

import android.content.Context
import androidx.compose.runtime.Immutable
import com.lingxi.code.R

/**
 * Resolves a localized string for localapps-package code that runs OUTSIDE a
 * `@Composable` body — [LocalAppsViewModel], [LocalAppRuntimeAssets],
 * [LocalAppCodeBrowser] and the pure [readable] mapper below — and therefore
 * cannot call `stringResource()`. Mirrors `ConversationStrings`
 * (conversation/ConversationSource.kt): the fallback keeps every JVM unit test
 * that constructs these types directly, with no Android `Context`, asserting
 * the same literal Chinese copy without being touched; the production
 * resolver ([localAppsStrings]) resolves the REAL localized text through
 * [Context.getString] so the app renders in the user's selected language.
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
    /**
     * One-line description the user gave at creation time — the seed the LLM
     * authors the questionnaire from. Replaces `templateKind`/`templateName`
     * (local-apps#questionnaire, Task 18): there is no more static template
     * catalog to classify an app by.
     */
    val brief: String,
    val workflow: LocalAppWorkflow,
    val runtime: LocalAppRuntime = LocalAppRuntime(),
    val updatedAtMs: Long,
)

enum class LocalAppWorkflow {
    CollectingSpec,
    AwaitingSpecConfirmation,
    Generating,
    Validating,
    AwaitingPreviewConfirmation,
    Revising,
    Ready,
    GenerationFailed,
    ValidationFailed,
}

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

// NOTE (local-apps#questionnaire, Task 18): `LocalAppTemplate` (the static
// per-template wrapper — kind/version/name/description around a `steps`
// list) is deleted here (human-partner ruling: total removal of the static
// template catalog, T2/T5). `LocalAppQuestionnaire` below replaces it — the
// LLM-authored questionnaire is just an ordered bag of steps, with no
// catalog entry wrapping it.
typealias LocalAppQuestionnaire = List<LocalAppDesignStep>

@Immutable
data class LocalAppDesignStep(
    val id: String,
    val order: UInt,
    val title: String,
    val description: String?,
    val fields: List<LocalAppDesignField>,
)

enum class LocalAppFieldKind {
    ShortText,
    LongText,
    SingleChoice,
    MultipleChoice,
    Boolean,
    Color,
    Density,
    ScreenList,
    FeatureList,
    DataFieldList,
    DomainList,
}

@Immutable
data class LocalAppFieldOption(val value: String, val label: String)

@Immutable
data class LocalAppDesignField(
    val id: String,
    val label: String,
    val description: String?,
    val kind: LocalAppFieldKind,
    val required: Boolean,
    /** Renders an `Other…` free-text box (mirrors `AppDesignFieldDto.allowsCustom`). */
    val allowsCustom: Boolean = false,
    /** Renders "let the model decide" (mirrors `AppDesignFieldDto.allowsDefer`). */
    val allowsDefer: Boolean = false,
    val defaultValue: LocalAppDesignValue?,
    val options: List<LocalAppFieldOption>,
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

sealed interface LocalAppDesignValue {
    @Immutable
    data class Text(val value: String) : LocalAppDesignValue

    @Immutable
    data class Choice(val value: String) : LocalAppDesignValue

    @Immutable
    data class Choices(val values: List<String>) : LocalAppDesignValue

    @Immutable
    data class Toggle(val value: Boolean) : LocalAppDesignValue

    @Immutable
    data class Density(val compact: Boolean) : LocalAppDesignValue

    @Immutable
    data class StringList(val values: List<String>) : LocalAppDesignValue

    @Immutable
    data class DataFields(val values: List<LocalAppDataField>) : LocalAppDesignValue
}

@Immutable
data class LocalAppSuggestion(
    val id: String,
    val basedOnRevision: ULong,
    val summary: String,
    val changes: List<LocalAppSuggestedChange>,
)

@Immutable
data class LocalAppSuggestedChange(
    val fieldId: String,
    val label: String,
    val before: String,
    val after: String,
)

@Immutable
data class LocalAppDesigner(
    val appId: String,
    val appName: String,
    val stepIndex: Int = 0,
    val revision: ULong = 0u,
    val interactionId: String? = null,
    val values: Map<String, LocalAppDesignValue> = emptyMap(),
    val suggestion: LocalAppSuggestion? = null,
    val conflictRevision: ULong? = null,
)

@Immutable
data class LocalAppGeneration(
    val jobId: String? = null,
    val state: String,
    val percent: Int? = null,
    val detail: String? = null,
)

@Immutable
data class LocalAppCollectionSchema(
    val id: String,
    val label: String,
    val fields: List<LocalAppDataField>,
    val enabledByDefault: Boolean,
)

/**
 * One capability kind the plan asks the user to grant, mirrors
 * `AppCapabilityKindDto`. Distinct from [LocalAppAuthorizationDecision]
 * (once/session/always/deny), which is the user's ANSWER to a capability
 * prompt, not the capability itself.
 */
enum class LocalAppCapabilityKind { DataMutation, UiControl, NetworkDomain, RestoreCheckpoint }

/**
 * The LLM-derived plan awaiting confirmation (local-apps#questionnaire, Task
 * 1/18). Replaces the deleted `LocalAppTemplate`: a template was a
 * human-authored, static catalog entry; a plan is authored per-app from the
 * questionnaire answers, and is voided the moment an answer changes
 * underneath it (mirrors `LocalAppsUiState.plans`'s reducer in
 * [LocalAppsViewModel]).
 */
@Immutable
data class LocalAppPlan(
    val collections: List<LocalAppCollectionSchema>,
    val capabilities: List<LocalAppCapabilityKind>,
    /** External HTTPS host names the app may request. */
    val domains: List<String>,
    /** Human-readable summary, including what every deferred field was finally decided as. */
    val summary: String,
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

@Immutable
data class LocalAppPreview(
    val appId: String,
    val revision: ULong,
    val interactionId: String,
    val url: String?,
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
data class LocalAppBridgeResult(
    val requestId: String,
    val appId: String,
    val ok: Boolean,
    val payloadJson: String?,
    val error: String?,
)

enum class LocalAppDetailsTab { Preview, Data, Code, History, PermissionsLogs }

sealed interface LocalAppsDestination {
    data object Library : LocalAppsDestination
    data class Designer(val appId: String) : LocalAppsDestination
    data class Preview(val appId: String) : LocalAppsDestination
    data class Details(val appId: String, val tab: LocalAppDetailsTab = LocalAppDetailsTab.Preview) : LocalAppsDestination
}

@Immutable
data class LocalAppsUiState(
    val loading: Boolean = true,
    val apps: List<LocalAppItem> = emptyList(),
    /**
     * The LLM-authored questionnaire per app (local-apps#questionnaire, Task
     * 18). Replaces the deleted static `templates` cache — the questionnaire
     * is authored per-app from its brief, not looked up from a catalog.
     */
    val questionnaires: Map<String, LocalAppQuestionnaire> = emptyMap(),
    /**
     * The LLM-derived plan awaiting confirmation, keyed by app id. Cleared by
     * the engine (and mirrored here) the moment an answer edit invalidates a
     * previously-derived plan.
     */
    val plans: Map<String, LocalAppPlan> = emptyMap(),
    val destination: LocalAppsDestination = LocalAppsDestination.Library,
    val query: String = "",
    val createName: String = "",
    val designer: LocalAppDesigner? = null,
    val generation: Map<String, LocalAppGeneration> = emptyMap(),
    val details: Map<String, LocalAppDetails> = emptyMap(),
    val previews: Map<String, LocalAppPreview> = emptyMap(),
    val selectedAppId: String? = null,
    val selectedDetailsTab: LocalAppDetailsTab = LocalAppDetailsTab.Preview,
    val pendingAuthorization: LocalAppAuthorizationRequest? = null,
    val bridgeResults: Map<String, LocalAppBridgeResult> = emptyMap(),
    val pendingUiAction: LocalAppPendingUiAction? = null,
    val error: String? = null,
    val distributionMode: LocalAppRuntimeMode,
) {
    val filteredApps: List<LocalAppItem>
        get() = apps.filter { app ->
            query.isBlank() || app.name.contains(query.trim(), ignoreCase = true)
        }

    /**
     * The preview gate announces `url = null` until the engine serves preview
     * urls itself, so every preview surface follows the runtime url that
     * `start_preview` already brought up before the gate opened.
     */
    fun previewUrl(appId: String): String? =
        apps.firstOrNull { it.id == appId }?.runtime?.url ?: previews[appId]?.url
}

sealed interface LocalAppsAction {
    data object Refresh : LocalAppsAction
    data object Create : LocalAppsAction
    data class Search(val query: String) : LocalAppsAction
    data class ChangeCreateName(val name: String) : LocalAppsAction
    /** Creates a new app from a one-line brief — replaces the deleted template picker. */
    data class CreateFromBrief(val brief: String) : LocalAppsAction
    /** Replaces an app's brief and re-authors its questionnaire from scratch. */
    data class UpdateBrief(val appId: String, val brief: String) : LocalAppsAction
    /** Retries questionnaire authoring after it failed, reusing the same brief. */
    data class RetryQuestionnaire(val appId: String) : LocalAppsAction
    /** Begins planning from the collected answers. */
    data class BeginPlanning(val appId: String) : LocalAppsAction
    /** Retries planning after it failed, reusing the same answers. */
    data class RetryPlan(val appId: String) : LocalAppsAction
    /** Requests a revision pass with a free-text prompt (the persistent iteration input). */
    data class Revise(val appId: String, val prompt: String) : LocalAppsAction
    data class OpenApp(val appId: String) : LocalAppsAction
    data class OpenDesigner(val appId: String) : LocalAppsAction
    data class ChangeStep(val index: Int) : LocalAppsAction
    data class EditField(val fieldId: String, val value: LocalAppDesignValue, val debounce: Boolean) : LocalAppsAction
    data object RequestSuggestion : LocalAppsAction
    data object ApplySuggestion : LocalAppsAction
    data object DismissSuggestion : LocalAppsAction
    data object ConfirmDesign : LocalAppsAction
    data class StartRuntime(val appId: String) : LocalAppsAction
    data class StopRuntime(val appId: String) : LocalAppsAction
    data class RetryGeneration(val appId: String) : LocalAppsAction
    data class DeleteApp(val appId: String) : LocalAppsAction
    data class ResetPermissions(val appId: String) : LocalAppsAction
    data class RestoreCheckpoint(val appId: String, val checkpointId: String) : LocalAppsAction
    data class ApprovePreview(val appId: String) : LocalAppsAction
    data class SubmitRevision(val appId: String, val feedback: String) : LocalAppsAction
    data class BridgeRequest(val message: LocalAppBridgeMessage) : LocalAppsAction
    data class AcknowledgeBridgeResult(val requestId: String) : LocalAppsAction
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

internal fun LocalAppDesignValue.readable(strings: LocalAppsStrings = DefaultLocalAppsStrings): String = when (this) {
    is LocalAppDesignValue.Text -> value
    is LocalAppDesignValue.Choice -> value
    is LocalAppDesignValue.Choices -> values.joinToString("、")
    is LocalAppDesignValue.Toggle -> if (value) {
        strings.resolve(R.string.common_yes, "是")
    } else {
        strings.resolve(R.string.common_no, "否")
    }
    is LocalAppDesignValue.Density -> if (compact) {
        strings.resolve(R.string.settings_density_compact, "紧凑")
    } else {
        strings.resolve(R.string.settings_density_comfortable, "舒适")
    }
    is LocalAppDesignValue.StringList -> values.joinToString("、")
    is LocalAppDesignValue.DataFields -> values.joinToString("、") { it.label }
}
