package com.lingxi.code.localapps

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.LazyRow
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.rounded.ArrowBack
import androidx.compose.material.icons.rounded.Add
import androidx.compose.material.icons.rounded.Apps
import androidx.compose.material.icons.rounded.Close
import androidx.compose.material.icons.rounded.Menu
import androidx.compose.material.icons.rounded.MoreVert
import androidx.compose.material.icons.rounded.PlayArrow
import androidx.compose.material.icons.rounded.Refresh
import androidx.compose.material.icons.rounded.Stop
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.AssistChip
import androidx.compose.material3.Button
import androidx.compose.material3.Card
import androidx.compose.material3.CardDefaults
import androidx.compose.material3.Checkbox
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.FilterChip
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Surface
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TopAppBar
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.platform.LocalContext
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import com.lingxi.code.R
import java.text.DateFormat
import java.util.Date
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

@Composable
fun LocalAppsRoute(
    viewModel: LocalAppsViewModel,
    onOpenDrawer: () -> Unit,
    onExternalNavigation: (String) -> Unit,
    modifier: Modifier = Modifier,
) {
    val state by viewModel.uiState.collectAsStateWithLifecycle()
    LocalAppsScreen(
        state = state,
        onAction = viewModel::onAction,
        onOpenDrawer = onOpenDrawer,
        onExternalNavigation = onExternalNavigation,
        modifier = modifier,
    )
}

@Composable
fun LocalAppsScreen(
    state: LocalAppsUiState,
    onAction: (LocalAppsAction) -> Unit,
    onOpenDrawer: () -> Unit,
    onExternalNavigation: (String) -> Unit,
    modifier: Modifier = Modifier,
) {
    Box(modifier.fillMaxSize()) {
        when (val destination = state.destination) {
            LocalAppsDestination.Library -> LocalAppsLibraryScreen(
                state = state,
                onAction = onAction,
                onOpenDrawer = onOpenDrawer,
            )

            is LocalAppsDestination.Designer -> LocalAppDesignerScreen(
                appId = destination.appId,
                state = state,
                onAction = onAction,
            )

            is LocalAppsDestination.Preview -> LocalAppPreviewScreen(
                appId = destination.appId,
                state = state,
                onAction = onAction,
                onExternalNavigation = onExternalNavigation,
            )

            is LocalAppsDestination.Details -> LocalAppDetailsScreen(
                appId = destination.appId,
                state = state,
                onAction = onAction,
                onExternalNavigation = onExternalNavigation,
            )
        }

        state.error?.let { message ->
            AlertDialog(
                onDismissRequest = { onAction(LocalAppsAction.DismissError) },
                title = { Text(stringResource(R.string.local_apps_action_failed_title)) },
                text = { Text(message) },
                confirmButton = {
                    TextButton(onClick = { onAction(LocalAppsAction.DismissError) }) {
                        Text(stringResource(R.string.common_got_it))
                    }
                },
            )
        }
        state.pendingAuthorization?.let { request ->
            AuthorizationDialog(request = request, onAction = onAction)
        }
    }
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun LocalAppsLibraryScreen(
    state: LocalAppsUiState,
    onAction: (LocalAppsAction) -> Unit,
    onOpenDrawer: () -> Unit,
) {
    // Pure Compose-local UI state, not part of `LocalAppsUiState` — the
    // template-picker route this replaces (local-apps#questionnaire, Task
    // 18) is gone, and there is no dedicated create destination to hold
    // "is the create dialog open" instead. This is the minimal working
    // replacement, same spirit as iOS's Task 13 `.create` route note; Task 20
    // owns the real create-entry UI.
    var showCreateDialog by remember { mutableStateOf(false) }
    Scaffold(
        topBar = {
            TopAppBar(
                title = { Text(stringResource(R.string.local_apps_title)) },
                navigationIcon = {
                    IconButton(onClick = onOpenDrawer) {
                        Icon(Icons.Rounded.Menu, contentDescription = stringResource(R.string.chat_open_side_drawer))
                    }
                },
                actions = {
                    IconButton(onClick = { onAction(LocalAppsAction.Refresh) }) {
                        Icon(Icons.Rounded.Refresh, contentDescription = stringResource(R.string.local_apps_refresh))
                    }
                    IconButton(onClick = { onAction(LocalAppsAction.Create); showCreateDialog = true }) {
                        Icon(Icons.Rounded.Add, contentDescription = stringResource(R.string.local_apps_create))
                    }
                },
            )
        },
    ) { padding ->
        Column(
            modifier = Modifier
                .fillMaxSize()
                .padding(padding)
                .padding(horizontal = 16.dp),
        ) {
            RuntimeModeBanner(state.distributionMode)
            OutlinedTextField(
                value = state.query,
                onValueChange = { onAction(LocalAppsAction.Search(it)) },
                label = { Text(stringResource(R.string.local_apps_search)) },
                singleLine = true,
                modifier = Modifier.fillMaxWidth().padding(vertical = 12.dp),
            )

            when {
                state.loading -> Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) {
                    CircularProgressIndicator()
                }

                state.filteredApps.isEmpty() -> EmptyApps(onCreate = {
                    onAction(LocalAppsAction.Create)
                    showCreateDialog = true
                })

                else -> LazyColumn(
                    verticalArrangement = Arrangement.spacedBy(10.dp),
                    modifier = Modifier.fillMaxSize(),
                ) {
                    items(state.filteredApps, key = { it.id }) { app ->
                        LocalAppCard(app = app, onAction = onAction)
                    }
                    item { Spacer(Modifier.height(20.dp)) }
                }
            }
        }
    }
    if (showCreateDialog) {
        CreateAppDialog(
            name = state.createName,
            onNameChange = { onAction(LocalAppsAction.ChangeCreateName(it)) },
            onDismiss = { showCreateDialog = false },
            onCreate = {
                // NOTE (local-apps#questionnaire, Task 18/20): this dialog only
                // collects a display name, so it sends that name as the brief
                // too — the SAME stopgap `createSelectedTemplate()` used to
                // make (see `LocalAppsViewModel.createFromBrief`'s doc and the
                // `LocalAppsViewModelTest` tripwire pinning this exact
                // fabrication). Task 20 replaces this dialog with the real
                // brief-collecting create entry.
                onAction(LocalAppsAction.CreateFromBrief(state.createName))
                showCreateDialog = false
            },
        )
    }
}

@Composable
private fun CreateAppDialog(
    name: String,
    onNameChange: (String) -> Unit,
    onDismiss: () -> Unit,
    onCreate: () -> Unit,
) {
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text(stringResource(R.string.local_apps_create)) },
        text = {
            OutlinedTextField(
                value = name,
                onValueChange = onNameChange,
                label = { Text(stringResource(R.string.local_apps_name)) },
                supportingText = { Text(stringResource(R.string.local_apps_create_name_hint)) },
                singleLine = true,
                modifier = Modifier.fillMaxWidth(),
            )
        },
        confirmButton = {
            Button(enabled = name.isNotBlank(), onClick = onCreate) {
                Text(stringResource(R.string.local_apps_create_and_design))
            }
        },
        dismissButton = { TextButton(onClick = onDismiss) { Text(stringResource(R.string.common_cancel)) } },
    )
}

@Composable
private fun RuntimeModeBanner(mode: LocalAppRuntimeMode) {
    val direct = mode == LocalAppRuntimeMode.NextProduction
    Surface(
        color = MaterialTheme.colorScheme.secondaryContainer,
        shape = RoundedCornerShape(12.dp),
        modifier = Modifier
            .fillMaxWidth()
            .padding(bottom = 12.dp),
    ) {
        Row(
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(10.dp),
            modifier = Modifier.padding(12.dp),
        ) {
            Icon(Icons.Rounded.Apps, contentDescription = null)
            Column {
                Text(
                    if (direct) {
                        stringResource(R.string.local_apps_mode_direct)
                    } else {
                        stringResource(R.string.local_apps_mode_play)
                    },
                    fontWeight = FontWeight.SemiBold,
                )
                Text(
                    if (direct) {
                        stringResource(R.string.local_apps_mode_direct_detail)
                    } else {
                        stringResource(R.string.local_apps_mode_play_detail)
                    },
                    style = MaterialTheme.typography.bodySmall,
                )
            }
        }
    }
}

@Composable
private fun EmptyApps(onCreate: () -> Unit) {
    Column(
        horizontalAlignment = Alignment.CenterHorizontally,
        verticalArrangement = Arrangement.Center,
        modifier = Modifier.fillMaxSize(),
    ) {
        Icon(Icons.Rounded.Apps, contentDescription = null, modifier = Modifier.size(48.dp))
        Spacer(Modifier.height(12.dp))
        Text(stringResource(R.string.local_apps_empty), style = MaterialTheme.typography.titleMedium)
        Text(stringResource(R.string.local_apps_empty_detail), style = MaterialTheme.typography.bodyMedium)
        Spacer(Modifier.height(16.dp))
        Button(onClick = onCreate) { Text(stringResource(R.string.local_apps_create)) }
    }
}

@Composable
private fun LocalAppCard(app: LocalAppItem, onAction: (LocalAppsAction) -> Unit) {
    var menuExpanded by remember(app.id) { mutableStateOf(false) }
    var confirmDelete by remember(app.id) { mutableStateOf(false) }
    Card(
        onClick = { onAction(LocalAppsAction.OpenApp(app.id)) },
        colors = CardDefaults.cardColors(containerColor = MaterialTheme.colorScheme.surfaceContainer),
        modifier = Modifier.fillMaxWidth(),
    ) {
        Column(Modifier.padding(16.dp)) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                Column(Modifier.weight(1f)) {
                    Text(app.name, style = MaterialTheme.typography.titleMedium, maxLines = 1, overflow = TextOverflow.Ellipsis)
                    Text(
                        app.brief,
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                    )
                }
                AssistChip(onClick = {}, label = { Text(app.workflow.label()) })
                Box {
                    IconButton(onClick = { menuExpanded = true }) {
                        Icon(Icons.Rounded.MoreVert, contentDescription = stringResource(R.string.local_apps_card_menu_a11y))
                    }
                    DropdownMenu(expanded = menuExpanded, onDismissRequest = { menuExpanded = false }) {
                        DropdownMenuItem(
                            text = { Text(stringResource(R.string.local_apps_delete), color = MaterialTheme.colorScheme.error) },
                            onClick = { menuExpanded = false; confirmDelete = true },
                        )
                    }
                }
            }
            app.runtime.detail?.let {
                Text(it, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.error)
            }
            Row(
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(8.dp),
                modifier = Modifier.padding(top = 10.dp),
            ) {
                Text(app.runtime.state.label(), style = MaterialTheme.typography.labelMedium)
                Spacer(Modifier.weight(1f))
                if (app.runtime.state == LocalAppRuntimeState.Running || app.runtime.state == LocalAppRuntimeState.Starting) {
                    OutlinedButton(onClick = { onAction(LocalAppsAction.StopRuntime(app.id)) }) {
                        Icon(Icons.Rounded.Stop, contentDescription = null, modifier = Modifier.size(18.dp))
                        Spacer(Modifier.width(6.dp))
                        Text(stringResource(R.string.composer_stop))
                    }
                } else if (app.workflow == LocalAppWorkflow.Ready) {
                    Button(onClick = { onAction(LocalAppsAction.StartRuntime(app.id)) }) {
                        Icon(Icons.Rounded.PlayArrow, contentDescription = null, modifier = Modifier.size(18.dp))
                        Spacer(Modifier.width(6.dp))
                        Text(stringResource(R.string.common_start))
                    }
                }
            }
        }
    }
    if (confirmDelete) {
        AlertDialog(
            onDismissRequest = { confirmDelete = false },
            title = { Text(stringResource(R.string.local_apps_delete_confirm, app.name)) },
            text = { Text(stringResource(R.string.local_apps_delete_confirm_detail)) },
            confirmButton = {
                Button(onClick = {
                    confirmDelete = false
                    onAction(LocalAppsAction.DeleteApp(app.id))
                }) { Text(stringResource(R.string.local_apps_delete)) }
            },
            dismissButton = { TextButton(onClick = { confirmDelete = false }) { Text(stringResource(R.string.common_cancel)) } },
        )
    }
}

/**
 * The questionnaire's steps for [appId], ordered the way the engine declares
 * them. Pure so a test can assert the sort without a Composable host —
 * [LocalAppDesignerScreen] calls this instead of inlining the sort (mirrors
 * iOS's `LocalAppDesignerView.steps`).
 */
internal fun designerSteps(state: LocalAppsUiState, appId: String): List<LocalAppDesignStep> =
    state.questionnaires[appId].orEmpty().sortedBy { it.order }

/**
 * Whether the questionnaire's answering form is interactive for [workflow] —
 * `false` for every state but [LocalAppWorkflow.CollectingSpec], in
 * particular for [LocalAppWorkflow.AuthoringQuestionnaire] and
 * [LocalAppWorkflow.Planning], where an LLM round trip owns the draft and a
 * concurrent user edit would race it. Mirrors iOS's
 * `LocalAppDesignerView.isEditable`; a pure function of workflow so it is
 * directly testable without a live view.
 */
internal fun isDesignerEditable(workflow: LocalAppWorkflow): Boolean =
    workflow == LocalAppWorkflow.CollectingSpec

/**
 * A chip in a field's chip row: one of the field's declared options, or the
 * 「由你决定」 chip when [LocalAppDesignField.allowsDefer]. Mirrors iOS's
 * `DesignerFieldChips.Chip`.
 */
sealed interface DesignerChip {
    data class Option(val value: String) : DesignerChip
    data object Defer : DesignerChip
}

/**
 * The chip row for [field]: every declared option, then the defer chip last
 * when [LocalAppDesignField.allowsDefer]. `allowsCustom` does NOT add a chip
 * here — [showsCustomInput] renders it as an always-visible text box instead
 * (mirrors iOS's `DesignerFieldChips.chipValues`).
 */
internal fun chipsFor(field: LocalAppDesignField): List<DesignerChip> =
    field.options.map { DesignerChip.Option(it.value) } +
        if (field.allowsDefer) listOf(DesignerChip.Defer) else emptyList()

/** Whether [field] renders the always-visible `Other…` free-text box. */
internal fun showsCustomInput(field: LocalAppDesignField): Boolean = field.allowsCustom

/**
 * Applies tapping [chip] against [field]'s [currentValue]. `Defer` always
 * sends [LocalAppDesignValue.Deferred] — an ANSWER, not an absence
 * (local-apps#questionnaire, Task 1/13/19: the core gate treats `Deferred` as
 * satisfying a required field the same way `isPresent` below does). An
 * `Option` toggles into/out of a `MultipleChoice` selection, or replaces any
 * other field kind's value outright. Mirrors iOS's `DesignerFieldChips.select(_:)`.
 */
internal fun selectChip(
    field: LocalAppDesignField,
    chip: DesignerChip,
    currentValue: LocalAppDesignValue? = null,
    onChange: (LocalAppDesignValue) -> Unit,
) {
    when (chip) {
        DesignerChip.Defer -> onChange(LocalAppDesignValue.Deferred)
        is DesignerChip.Option -> onChange(toggledChipOption(field, chip.value, currentValue))
    }
}

private fun toggledChipOption(
    field: LocalAppDesignField,
    optionValue: String,
    currentValue: LocalAppDesignValue?,
): LocalAppDesignValue {
    if (field.kind != LocalAppFieldKind.MultipleChoice) return LocalAppDesignValue.Choice(optionValue)
    val selected = (currentValue as? LocalAppDesignValue.Choices)?.values.orEmpty()
    return LocalAppDesignValue.Choices(
        if (optionValue in selected) selected - optionValue else selected + optionValue,
    )
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun LocalAppDesignerScreen(appId: String, state: LocalAppsUiState, onAction: (LocalAppsAction) -> Unit) {
    val app = state.apps.firstOrNull { it.id == appId }
    if (app == null) {
        Scaffold(topBar = { LocalAppsTopBar(stringResource(R.string.local_apps_designer_title), onBack = { onAction(LocalAppsAction.Back) }) }) { padding ->
            Box(Modifier.fillMaxSize().padding(padding), contentAlignment = Alignment.Center) { CircularProgressIndicator() }
        }
        return
    }
    if (!isDesignerEditable(app.workflow)) {
        DesignerUnavailableScreen(app = app, onAction = onAction)
        return
    }
    // The designer session itself (`state.designer`) is created by
    // `reduceDetails` the first time a `GetAppDetails` reply for this app
    // lands (Task 19 — `openDesigner` always requests one); a still-null
    // designer here just means that reply has not arrived yet.
    val designer = state.designer?.takeIf { it.appId == appId }
    if (designer == null) {
        Scaffold(topBar = { LocalAppsTopBar(app.name, onBack = { onAction(LocalAppsAction.Back) }) }) { padding ->
            Box(Modifier.fillMaxSize().padding(padding), contentAlignment = Alignment.Center) { CircularProgressIndicator() }
        }
        return
    }
    val steps = designerSteps(state, appId)
    val stepIndex = designer.stepIndex.coerceIn(0, (steps.size - 1).coerceAtLeast(0))
    val step = steps.getOrNull(stepIndex)
    val currentStepComplete = step?.fields?.all { field ->
        !field.required || (designer.values[field.id] ?: field.defaultValue)?.isPresent() == true
    } ?: false
    Scaffold(
        topBar = { LocalAppsTopBar(designer.appName, onBack = { onAction(LocalAppsAction.Back) }) },
        bottomBar = {
            Surface(tonalElevation = 3.dp) {
                Row(
                    horizontalArrangement = Arrangement.spacedBy(10.dp),
                    modifier = Modifier.fillMaxWidth().padding(16.dp),
                ) {
                    OutlinedButton(
                        enabled = stepIndex > 0,
                        onClick = { onAction(LocalAppsAction.ChangeStep(stepIndex - 1)) },
                        modifier = Modifier.weight(1f),
                    ) { Text(stringResource(R.string.local_apps_previous)) }
                    Button(
                        enabled = currentStepComplete,
                        onClick = {
                            if (stepIndex < steps.lastIndex) {
                                onAction(LocalAppsAction.ChangeStep(stepIndex + 1))
                            } else {
                                // The final step starts an LLM round trip
                                // (`collecting_spec -> planning`), NOT
                                // `ConfirmDesign`/`confirm_design`, which is the
                                // LATER "confirm the derived plan" gate
                                // (`awaiting_spec_confirmation -> generating`,
                                // Task 20's plan-confirm screen) that arms
                                // automatically once planning finishes. Stays
                                // on this screen either way: `isDesignerEditable`
                                // above flips to the busy 出计划中 state as soon
                                // as `AppWorkflowChanged` reports `planning`.
                                onAction(LocalAppsAction.BeginPlanning(appId))
                            }
                        },
                        modifier = Modifier.weight(1f),
                    ) {
                        Text(
                            if (stepIndex < steps.lastIndex) {
                                stringResource(R.string.local_apps_next)
                            } else {
                                stringResource(R.string.local_apps_generate_plan)
                            },
                        )
                    }
                }
            }
        },
    ) { padding ->
        Column(
            verticalArrangement = Arrangement.spacedBy(14.dp),
            modifier = Modifier
                .fillMaxSize()
                .padding(padding)
                .verticalScroll(rememberScrollState())
                .padding(16.dp),
        ) {
            Text(
                stringResource(R.string.local_apps_step_progress, stepIndex + 1, steps.size),
                style = MaterialTheme.typography.labelLarge,
            )
            LinearProgressIndicator(
                progress = { if (steps.isEmpty()) 0f else (stepIndex + 1).toFloat() / steps.size },
                modifier = Modifier.fillMaxWidth(),
            )
            step?.let {
                Text(it.title, style = MaterialTheme.typography.headlineSmall, modifier = Modifier.semantics { heading() })
                it.description?.let { description -> Text(description, color = MaterialTheme.colorScheme.onSurfaceVariant) }
                it.fields.forEach { field ->
                    LocalAppDynamicField(
                        field = field,
                        value = designer.values[field.id] ?: field.defaultValue ?: field.emptyValue(),
                        onValueChange = { value, debounce ->
                            onAction(LocalAppsAction.EditField(field.id, value, debounce))
                        },
                    )
                }
            }
            OutlinedButton(onClick = { onAction(LocalAppsAction.RequestSuggestion) }, modifier = Modifier.fillMaxWidth()) {
                Text(stringResource(R.string.local_apps_request_suggestion))
            }
            designer.suggestion?.let { suggestion ->
                SuggestionPanel(suggestion = suggestion, onAction = onAction)
            }
            designer.conflictRevision?.let {
                Text(
                    stringResource(R.string.local_apps_design_conflict_reloaded, it.toString()),
                    color = MaterialTheme.colorScheme.error,
                )
            }
            Spacer(Modifier.height(84.dp))
        }
    }
}

/**
 * The four intermediate/failure states plus a generic fallback that
 * [LocalAppDesignerScreen] renders instead of the editable form
 * ([isDesignerEditable] false) — mirrors iOS's
 * `LocalAppDesignerView.unavailableView(for:)`. `AwaitingSpecConfirmation`
 * and `GenerationFailed` land in the generic fallback: their real screens
 * (the plan-confirm sheet, and re-arming the confirm gate) are Task 20's job;
 * this screen only needs to not dead-end while waiting for them.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun DesignerUnavailableScreen(app: LocalAppItem, onAction: (LocalAppsAction) -> Unit) {
    var brief by remember(app.id) { mutableStateOf(app.brief) }
    Scaffold(topBar = { LocalAppsTopBar(app.name, onBack = { onAction(LocalAppsAction.Back) }) }) { padding ->
        Box(Modifier.fillMaxSize().padding(padding), contentAlignment = Alignment.Center) {
            when (app.workflow) {
                LocalAppWorkflow.AuthoringQuestionnaire ->
                    DesignerBusyState(stringResource(R.string.local_apps_authoring_questionnaire))
                LocalAppWorkflow.Planning ->
                    DesignerBusyState(stringResource(R.string.local_apps_planning))
                LocalAppWorkflow.QuestionnaireFailed ->
                    DesignerFailedState(
                        detail = stringResource(R.string.local_apps_questionnaire_failed_detail),
                        onRetry = { onAction(LocalAppsAction.RetryQuestionnaire(app.id)) },
                    ) {
                        OutlinedTextField(
                            value = brief,
                            onValueChange = { brief = it },
                            label = { Text(stringResource(R.string.local_apps_brief)) },
                            modifier = Modifier.fillMaxWidth(),
                        )
                        Button(
                            enabled = brief.isNotBlank(),
                            onClick = { onAction(LocalAppsAction.UpdateBrief(app.id, brief)) },
                            modifier = Modifier.fillMaxWidth(),
                        ) { Text(stringResource(R.string.local_apps_update_brief)) }
                    }
                LocalAppWorkflow.PlanFailed ->
                    DesignerFailedState(
                        detail = stringResource(R.string.local_apps_plan_failed_detail),
                        onRetry = { onAction(LocalAppsAction.RetryPlan(app.id)) },
                    )
                else -> DesignerBusyState(stringResource(R.string.local_apps_designer_waiting))
            }
        }
    }
}

@Composable
private fun DesignerBusyState(detail: String) {
    Column(horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.spacedBy(16.dp)) {
        CircularProgressIndicator()
        Text(detail, style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
    }
}

@Composable
private fun DesignerFailedState(
    detail: String,
    onRetry: () -> Unit,
    extraActions: @Composable ColumnScope.() -> Unit = {},
) {
    Column(
        horizontalAlignment = Alignment.CenterHorizontally,
        verticalArrangement = Arrangement.spacedBy(16.dp),
        modifier = Modifier.padding(24.dp),
    ) {
        Icon(Icons.Rounded.Refresh, contentDescription = null, modifier = Modifier.size(40.dp), tint = MaterialTheme.colorScheme.error)
        Text(detail, style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
        Button(onClick = onRetry, modifier = Modifier.fillMaxWidth()) { Text(stringResource(R.string.common_retry)) }
        extraActions()
    }
}

@Composable
private fun LocalAppDynamicField(
    field: LocalAppDesignField,
    value: LocalAppDesignValue,
    onValueChange: (LocalAppDesignValue, Boolean) -> Unit,
) {
    Card(colors = CardDefaults.cardColors(containerColor = MaterialTheme.colorScheme.surfaceContainerLow)) {
        Column(verticalArrangement = Arrangement.spacedBy(8.dp), modifier = Modifier.fillMaxWidth().padding(14.dp)) {
            Text(field.label + if (field.required) " *" else "", fontWeight = FontWeight.SemiBold)
            field.description?.let { Text(it, style = MaterialTheme.typography.bodySmall) }
            when (field.kind) {
                LocalAppFieldKind.ShortText,
                LocalAppFieldKind.LongText,
                LocalAppFieldKind.Color -> {
                    val text = (value as? LocalAppDesignValue.Text)?.value.orEmpty()
                    OutlinedTextField(
                        value = text,
                        onValueChange = { onValueChange(LocalAppDesignValue.Text(it), true) },
                        minLines = if (field.kind == LocalAppFieldKind.LongText) 3 else 1,
                        singleLine = field.kind != LocalAppFieldKind.LongText,
                        modifier = Modifier.fillMaxWidth(),
                    )
                }

                LocalAppFieldKind.SingleChoice -> {
                    val selected = (value as? LocalAppDesignValue.Choice)?.value
                    LazyRow(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                        items(field.options, key = { it.value }) { option ->
                            FilterChip(
                                selected = selected == option.value,
                                onClick = { onValueChange(LocalAppDesignValue.Choice(option.value), false) },
                                label = { Text(option.label) },
                            )
                        }
                    }
                }

                LocalAppFieldKind.MultipleChoice -> {
                    val selected = (value as? LocalAppDesignValue.Choices)?.values.orEmpty()
                    field.options.forEach { option ->
                        Row(verticalAlignment = Alignment.CenterVertically) {
                            Checkbox(
                                checked = option.value in selected,
                                onCheckedChange = { checked ->
                                    onValueChange(
                                        LocalAppDesignValue.Choices(
                                            if (checked) selected + option.value else selected - option.value,
                                        ),
                                        false,
                                    )
                                },
                            )
                            Text(option.label)
                        }
                    }
                }

                LocalAppFieldKind.Boolean -> {
                    val checked = (value as? LocalAppDesignValue.Toggle)?.value == true
                    Row(verticalAlignment = Alignment.CenterVertically) {
                        Text(
                            if (checked) {
                                stringResource(R.string.local_apps_field_enabled)
                            } else {
                                stringResource(R.string.settings_status_not_enabled)
                            },
                            modifier = Modifier.weight(1f),
                        )
                        Switch(checked = checked, onCheckedChange = { onValueChange(LocalAppDesignValue.Toggle(it), false) })
                    }
                }

                LocalAppFieldKind.Density -> {
                    val compact = (value as? LocalAppDesignValue.Density)?.compact == true
                    Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                        FilterChip(
                            selected = compact,
                            onClick = { onValueChange(LocalAppDesignValue.Density(true), false) },
                            label = { Text(stringResource(R.string.settings_density_compact)) },
                        )
                        FilterChip(
                            selected = !compact,
                            onClick = { onValueChange(LocalAppDesignValue.Density(false), false) },
                            label = { Text(stringResource(R.string.settings_density_comfortable)) },
                        )
                    }
                }

                LocalAppFieldKind.ScreenList,
                LocalAppFieldKind.FeatureList,
                LocalAppFieldKind.DomainList -> {
                    val values = (value as? LocalAppDesignValue.StringList)?.values.orEmpty()
                    StringListEditor(
                        values = values,
                        placeholder = if (field.kind == LocalAppFieldKind.DomainList) {
                            "example.com"
                        } else {
                            stringResource(R.string.local_apps_add_item_placeholder)
                        },
                        validator = { candidate ->
                            candidate.isNotBlank() &&
                                (field.kind != LocalAppFieldKind.DomainList || isValidDomain(candidate))
                        },
                        onChange = { onValueChange(LocalAppDesignValue.StringList(it), false) },
                    )
                }

                LocalAppFieldKind.DataFieldList -> {
                    val fields = (value as? LocalAppDesignValue.DataFields)?.values.orEmpty()
                    DataFieldListEditor(fields = fields, onChange = { onValueChange(LocalAppDesignValue.DataFields(it), false) })
                }
            }
            // Appended under EVERY field kind's own editor above, not a
            // replacement for it: `allowsCustom`/`allowsDefer` are legal on
            // any field (`AppDesignFieldDto`, local-apps#questionnaire), so a
            // shortText/color/etc. field can still offer 「由你决定」 even
            // though it has no options to chip. Task 19 — these two
            // affordances had no UI treatment before this (Task 18 only
            // wired the DTO/model fields through).
            if (field.allowsDefer || field.allowsCustom) {
                DesignerFieldChips(field = field, value = value, onValueChange = onValueChange)
            }
        }
    }
}

/**
 * Renders a field's 「由你决定」 chip (when [LocalAppDesignField.allowsDefer])
 * and `Other…` free-text box (when [LocalAppDesignField.allowsCustom]) —
 * mirrors iOS's `DesignerFieldChips`. Appended under [LocalAppDynamicField]'s
 * own editor for the field's kind; not a replacement for it.
 */
@Composable
private fun DesignerFieldChips(
    field: LocalAppDesignField,
    value: LocalAppDesignValue?,
    onValueChange: (LocalAppDesignValue, Boolean) -> Unit,
) {
    val deferredLabel = stringResource(R.string.local_apps_value_deferred)
    val chips = chipsFor(field)
    if (chips.isNotEmpty()) {
        LazyRow(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            items(chips, key = { it.toString() }) { chip ->
                val selected = when (chip) {
                    is DesignerChip.Option -> isChipOptionSelected(field, chip.value, value)
                    DesignerChip.Defer -> value is LocalAppDesignValue.Deferred
                }
                FilterChip(
                    selected = selected,
                    onClick = {
                        selectChip(field, chip, value) { onValueChange(it, false) }
                    },
                    label = {
                        Text(
                            when (chip) {
                                is DesignerChip.Option -> field.options.firstOrNull { it.value == chip.value }?.label ?: chip.value
                                DesignerChip.Defer -> deferredLabel
                            },
                        )
                    },
                )
            }
        }
    }
    if (showsCustomInput(field)) {
        var customText by remember(field.id) { mutableStateOf(customTextFor(field, value)) }
        OutlinedTextField(
            value = customText,
            onValueChange = { text ->
                customText = text
                onValueChange(commitCustomChipText(field, text, value), true)
            },
            placeholder = { Text(stringResource(R.string.local_apps_custom_other)) },
            singleLine = true,
            modifier = Modifier.fillMaxWidth(),
        )
    }
}

private fun isChipOptionSelected(field: LocalAppDesignField, optionValue: String, value: LocalAppDesignValue?): Boolean =
    when (value) {
        is LocalAppDesignValue.Choice -> value.value == optionValue
        is LocalAppDesignValue.Choices -> optionValue in value.values
        is LocalAppDesignValue.Text -> value.value == optionValue
        else -> false
    }

/** The free-text box's current content: whatever part of [value] is NOT one of [field]'s declared options. */
private fun customTextFor(field: LocalAppDesignField, value: LocalAppDesignValue?): String {
    val optionValues = field.options.mapTo(hashSetOf()) { it.value }
    return when (value) {
        is LocalAppDesignValue.Text -> value.value.takeUnless { it in optionValues }.orEmpty()
        is LocalAppDesignValue.Choices -> value.values.firstOrNull { it !in optionValues }.orEmpty()
        else -> ""
    }
}

private fun commitCustomChipText(field: LocalAppDesignField, text: String, value: LocalAppDesignValue?): LocalAppDesignValue {
    if (field.kind != LocalAppFieldKind.MultipleChoice) return LocalAppDesignValue.Text(text)
    val optionValues = field.options.mapTo(hashSetOf()) { it.value }
    val selectedOptions = (value as? LocalAppDesignValue.Choices)?.values.orEmpty().filter { it in optionValues }
    return LocalAppDesignValue.Choices(if (text.isNotEmpty()) selectedOptions + text else selectedOptions)
}

@Composable
private fun StringListEditor(
    values: List<String>,
    placeholder: String,
    validator: (String) -> Boolean,
    onChange: (List<String>) -> Unit,
) {
    var draft by remember(values) { mutableStateOf("") }
    values.forEachIndexed { index, item ->
        Row(verticalAlignment = Alignment.CenterVertically, modifier = Modifier.fillMaxWidth()) {
            Text(item, modifier = Modifier.weight(1f))
            IconButton(onClick = { onChange(values.filterIndexed { i, _ -> i != index }) }) {
                Icon(Icons.Rounded.Close, contentDescription = stringResource(R.string.local_apps_delete_item_a11y, item))
            }
        }
    }
    Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
        OutlinedTextField(value = draft, onValueChange = { draft = it }, placeholder = { Text(placeholder) }, singleLine = true, modifier = Modifier.weight(1f))
        IconButton(
            enabled = validator(draft.trim()),
            onClick = {
                val candidate = draft.trim()
                if (candidate.isNotEmpty() && candidate !in values) onChange(values + candidate)
                draft = ""
            },
        ) { Icon(Icons.Rounded.Add, contentDescription = stringResource(R.string.local_apps_add_a11y)) }
    }
}

/**
 * Mirrors `manifest::validate_domain` (lingxi-code/local-apps/src/manifest.rs),
 * which accepts ASCII lowercase letters, digits and `-` only. Kotlin's
 * Unicode-aware [Char.isLetterOrDigit] would enable the Add button for
 * `API.Example.com` or `北京.cn`, which the engine then rejects at ingest with
 * `invalid_request` — a value the designer can never persist.
 */
internal fun isValidDomain(value: String): Boolean =
    value.length in 1..253 &&
        "://" !in value &&
        value.split('.').all { label ->
            label.isNotBlank() && label.length <= 63 &&
                label.firstOrNull()?.isAsciiDomainChar() == true &&
                label.lastOrNull()?.isAsciiDomainChar() == true &&
                label.all { it.isAsciiDomainChar() || it == '-' }
        }

private fun Char.isAsciiDomainChar(): Boolean = this in 'a'..'z' || this in '0'..'9'

@Composable
private fun DataFieldListEditor(fields: List<LocalAppDataField>, onChange: (List<LocalAppDataField>) -> Unit) {
    fields.forEachIndexed { index, field ->
        var menuOpen by remember(field.id) { mutableStateOf(false) }
        Card(modifier = Modifier.fillMaxWidth()) {
            Column(Modifier.padding(10.dp)) {
                Row(verticalAlignment = Alignment.CenterVertically) {
                    OutlinedTextField(
                        value = field.label,
                        onValueChange = { label -> onChange(fields.updated(index, field.copy(label = label))) },
                        label = { Text(stringResource(R.string.local_apps_data_field_name)) },
                        singleLine = true,
                        modifier = Modifier.weight(1f),
                    )
                    IconButton(onClick = { onChange(fields.filterIndexed { i, _ -> i != index }) }) {
                        Icon(Icons.Rounded.Close, contentDescription = stringResource(R.string.local_apps_field_delete))
                    }
                }
                Box {
                    OutlinedButton(onClick = { menuOpen = true }) { Text(field.type.label()) }
                    DropdownMenu(expanded = menuOpen, onDismissRequest = { menuOpen = false }) {
                        LocalAppDataFieldType.entries.forEach { type ->
                            DropdownMenuItem(
                                text = { Text(type.label()) },
                                onClick = {
                                    menuOpen = false
                                    onChange(fields.updated(index, field.copy(type = type)))
                                },
                            )
                        }
                    }
                }
                Row(verticalAlignment = Alignment.CenterVertically) {
                    Checkbox(
                        checked = field.required,
                        onCheckedChange = { onChange(fields.updated(index, field.copy(required = it))) },
                    )
                    Text(stringResource(R.string.local_apps_field_required))
                }
            }
        }
    }
    val newFieldLabel = stringResource(R.string.local_apps_new_field_default_label)
    OutlinedButton(
        onClick = {
            val id = generateFieldId(fields)
            onChange(fields + LocalAppDataField(id, newFieldLabel, LocalAppDataFieldType.Text, false))
        },
        modifier = Modifier.fillMaxWidth(),
    ) {
        Icon(Icons.Rounded.Add, contentDescription = null)
        Spacer(Modifier.width(6.dp))
        Text(stringResource(R.string.local_apps_field_add))
    }
}

@Composable
private fun SuggestionPanel(suggestion: LocalAppSuggestion, onAction: (LocalAppsAction) -> Unit) {
    Card(colors = CardDefaults.cardColors(containerColor = MaterialTheme.colorScheme.tertiaryContainer)) {
        Column(verticalArrangement = Arrangement.spacedBy(8.dp), modifier = Modifier.padding(14.dp)) {
            Text(stringResource(R.string.local_apps_agent_suggestion), style = MaterialTheme.typography.titleMedium)
            Text(suggestion.summary)
            suggestion.changes.forEach { change ->
                Text("${change.label}：${change.before} → ${change.after}", style = MaterialTheme.typography.bodySmall)
            }
            Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                Button(onClick = { onAction(LocalAppsAction.ApplySuggestion) }) { Text(stringResource(R.string.local_apps_apply_suggestion)) }
                TextButton(onClick = { onAction(LocalAppsAction.DismissSuggestion) }) { Text(stringResource(R.string.local_apps_ignore)) }
            }
        }
    }
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun LocalAppPreviewScreen(
    appId: String,
    state: LocalAppsUiState,
    onAction: (LocalAppsAction) -> Unit,
    onExternalNavigation: (String) -> Unit,
) {
    val app = state.apps.firstOrNull { it.id == appId }
    val preview = state.previews[appId]
    val generation = state.generation[appId]
    var feedback by remember(appId) { mutableStateOf("") }
    Scaffold(
        topBar = {
            LocalAppsTopBar(
                app?.name ?: stringResource(R.string.local_apps_preview_title),
                onBack = { onAction(LocalAppsAction.Back) },
            )
        },
    ) { padding ->
        Column(
            verticalArrangement = Arrangement.spacedBy(12.dp),
            modifier = Modifier.fillMaxSize().padding(padding).padding(16.dp),
        ) {
            generation?.let {
                Text(it.state, fontWeight = FontWeight.SemiBold)
                it.percent?.let { percent -> LinearProgressIndicator(progress = { percent / 100f }, modifier = Modifier.fillMaxWidth()) }
                it.detail?.let { detail -> Text(detail, style = MaterialTheme.typography.bodySmall) }
            }
            // `openApp` routes a failed generation here, where nothing else is
            // actionable. The engine re-opens the designer from GenerationFailed
            // (state.rs open_designer), which is the recovery iOS already offers.
            if (app?.workflow == LocalAppWorkflow.GenerationFailed) {
                Button(
                    onClick = { onAction(LocalAppsAction.OpenDesigner(appId)) },
                    modifier = Modifier.fillMaxWidth(),
                ) { Text(stringResource(R.string.local_apps_continue_design)) }
            }
            // Only the WebView needs a url; approval and revision need just the
            // gate's interaction id, so they must stay reachable while the
            // runtime is still coming up.
            val previewUrl = state.previewUrl(appId)
            if (preview != null) {
                if (previewUrl != null) {
                    BoundLocalAppWebView(
                        appId = appId,
                        url = previewUrl,
                        state = state,
                        onAction = onAction,
                        onExternalNavigation = onExternalNavigation,
                        modifier = Modifier.fillMaxWidth().weight(1f),
                    )
                } else {
                    Box(Modifier.fillMaxWidth().weight(1f), contentAlignment = Alignment.Center) {
                        Text(stringResource(R.string.local_apps_preview_not_ready_detail))
                    }
                }
                Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    Button(onClick = { onAction(LocalAppsAction.ApprovePreview(appId)) }) { Text(stringResource(R.string.local_apps_approve_preview)) }
                    OutlinedButton(onClick = { onAction(LocalAppsAction.StartRuntime(appId)) }) { Text(stringResource(R.string.local_apps_ui_action_reload)) }
                }
                OutlinedTextField(
                    value = feedback,
                    onValueChange = { feedback = it },
                    label = { Text(stringResource(R.string.local_apps_feedback)) },
                    modifier = Modifier.fillMaxWidth(),
                )
                Button(
                    enabled = feedback.isNotBlank(),
                    onClick = { onAction(LocalAppsAction.SubmitRevision(appId, feedback)); feedback = "" },
                    modifier = Modifier.fillMaxWidth(),
                ) { Text(stringResource(R.string.local_apps_submit_to_agent)) }
            } else if (generation == null) {
                Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) {
                    Text(stringResource(R.string.local_apps_awaiting_generation_job))
                }
            }
        }
    }
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun LocalAppDetailsScreen(
    appId: String,
    state: LocalAppsUiState,
    onAction: (LocalAppsAction) -> Unit,
    onExternalNavigation: (String) -> Unit,
) {
    val app = state.apps.firstOrNull { it.id == appId }
    Scaffold(
        topBar = {
            LocalAppsTopBar(
                app?.name ?: stringResource(R.string.local_apps_detail_title),
                onBack = { onAction(LocalAppsAction.Back) },
            )
        },
    ) { padding ->
        Column(Modifier.fillMaxSize().padding(padding)) {
            LazyRow(
                horizontalArrangement = Arrangement.spacedBy(8.dp),
                modifier = Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 8.dp),
            ) {
                items(LocalAppDetailsTab.entries, key = { it.name }) { tab ->
                    FilterChip(
                        selected = state.selectedDetailsTab == tab,
                        onClick = { onAction(LocalAppsAction.SelectDetailsTab(tab)) },
                        label = { Text(tab.label()) },
                    )
                }
            }
            HorizontalDivider()
            when (state.selectedDetailsTab) {
                LocalAppDetailsTab.Preview -> {
                    val url = state.previewUrl(appId)
                    if (url != null) {
                        BoundLocalAppWebView(
                            appId = appId,
                            url = url,
                            state = state,
                            onAction = onAction,
                            onExternalNavigation = onExternalNavigation,
                            modifier = Modifier.fillMaxSize(),
                        )
                    } else {
                        DetailsPlaceholder(
                            stringResource(R.string.local_apps_not_running_title),
                            stringResource(R.string.local_apps_not_running_detail),
                        )
                    }
                }
                LocalAppDetailsTab.Data -> LocalAppDataDetails(state.details[appId])
                LocalAppDetailsTab.Code -> LocalAppCodeDetails(appId, state.details[appId])
                LocalAppDetailsTab.History -> LocalAppHistoryDetails(appId, state.details[appId], onAction)
                LocalAppDetailsTab.PermissionsLogs -> LocalAppPermissionsDetails(
                    appId = appId,
                    details = state.details[appId],
                    onAction = onAction,
                )
            }
        }
    }
}

@Composable
private fun LocalAppDataDetails(details: LocalAppDetails?) {
    if (details == null) {
        DetailsPlaceholder(
            stringResource(R.string.local_apps_loading_data_title),
            stringResource(R.string.local_apps_loading_data_detail),
        )
        return
    }
    val requiredSuffix = stringResource(R.string.local_apps_required_suffix)
    LazyColumn(
        verticalArrangement = Arrangement.spacedBy(12.dp),
        modifier = Modifier.fillMaxSize().padding(16.dp),
    ) {
        item {
            Card {
                Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
                    Text(stringResource(R.string.local_apps_data_section_title), style = MaterialTheme.typography.titleMedium)
                    Text(stringResource(R.string.local_apps_data_section_detail))
                }
            }
        }
        if (details.collections.isEmpty()) {
            item {
                Text(
                    stringResource(R.string.local_apps_no_collections),
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
        }
        items(details.collections, key = { it.id }) { collection ->
            Card {
                Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                    Text(collection.label, style = MaterialTheme.typography.titleMedium)
                    Text(collection.id, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                    collection.fields.forEach { field ->
                        HorizontalDivider()
                        Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.SpaceBetween) {
                            Text(field.label)
                            val typeLabel = field.type.label()
                            Text(
                                buildString {
                                    append(typeLabel)
                                    if (field.required) append(requiredSuffix)
                                },
                                style = MaterialTheme.typography.bodySmall,
                                color = MaterialTheme.colorScheme.onSurfaceVariant,
                            )
                        }
                    }
                }
            }
        }
    }
}

@Composable
private fun LocalAppCodeDetails(appId: String, details: LocalAppDetails?) {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    val browserResult = remember(appId, details?.workspaceRelativePath) {
        details?.workspaceRelativePath?.let { path ->
            runCatching { LocalAppCodeBrowser(context.filesDir, path, strings = localAppsStrings(context)) }
        }
    }
    val browser = browserResult?.getOrNull()
    var files by remember(browser) { mutableStateOf<List<LocalAppSourceFile>>(emptyList()) }
    var selectedPath by remember(browser) { mutableStateOf<String?>(null) }
    var editorText by remember(browser) { mutableStateOf("") }
    var loading by remember(browser) { mutableStateOf(browser != null) }
    var message by remember(browser) { mutableStateOf(browserResult?.exceptionOrNull()?.message) }

    suspend fun refresh() {
        val active = browser ?: return
        loading = true
        runCatching { withContext(Dispatchers.IO) { active.listFiles() } }
            .onSuccess { files = it; message = null }
            .onFailure { message = it.message }
        loading = false
    }

    LaunchedEffect(browser) { refresh() }
    if (details == null) {
        DetailsPlaceholder(
            stringResource(R.string.local_apps_loading_workspace_title),
            stringResource(R.string.local_apps_loading_workspace_detail),
        )
        return
    }
    if (browser == null) {
        DetailsPlaceholder(
            stringResource(R.string.local_apps_workspace_open_failed_title),
            message ?: stringResource(R.string.local_apps_workspace_path_invalid),
        )
        return
    }
    val savedMessage = stringResource(R.string.local_apps_source_saved_message)

    if (selectedPath == null) {
        LazyColumn(
            verticalArrangement = Arrangement.spacedBy(8.dp),
            modifier = Modifier.fillMaxSize().padding(16.dp),
        ) {
            item {
                Text(details.workspaceRelativePath, style = MaterialTheme.typography.bodySmall)
                Text(
                    stringResource(R.string.local_apps_source_browser_notice),
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
            if (loading) item { LinearProgressIndicator(Modifier.fillMaxWidth()) }
            if (!loading && files.isEmpty()) item { Text(stringResource(R.string.local_apps_source_no_editable_files)) }
            items(files, key = { it.relativePath }) { file ->
                Card(onClick = {
                    scope.launch {
                        runCatching { withContext(Dispatchers.IO) { browser.read(file.relativePath) } }
                            .onSuccess { text -> selectedPath = file.relativePath; editorText = text; message = null }
                            .onFailure { message = it.message }
                    }
                }) {
                    Row(Modifier.fillMaxWidth().padding(14.dp), horizontalArrangement = Arrangement.SpaceBetween) {
                        Text(file.relativePath, modifier = Modifier.weight(1f))
                        Text("${file.size} B", style = MaterialTheme.typography.bodySmall)
                    }
                }
            }
            message?.let { detail -> item { Text(detail, color = MaterialTheme.colorScheme.error) } }
        }
    } else {
        Column(Modifier.fillMaxSize().padding(16.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            Text(selectedPath.orEmpty(), style = MaterialTheme.typography.titleSmall)
            OutlinedTextField(
                value = editorText,
                onValueChange = { editorText = it; message = null },
                modifier = Modifier.fillMaxWidth().weight(1f),
                textStyle = MaterialTheme.typography.bodySmall,
            )
            message?.let { Text(it, color = MaterialTheme.colorScheme.error) }
            Row(horizontalArrangement = Arrangement.spacedBy(8.dp), modifier = Modifier.fillMaxWidth()) {
                OutlinedButton(onClick = { selectedPath = null; editorText = "" }, modifier = Modifier.weight(1f)) {
                    Text(stringResource(R.string.common_close))
                }
                Button(
                    onClick = {
                        val path = selectedPath ?: return@Button
                        scope.launch {
                            runCatching { withContext(Dispatchers.IO) { browser.save(path, editorText) } }
                                .onSuccess { message = savedMessage }
                                .onFailure { message = it.message }
                        }
                    },
                    modifier = Modifier.weight(1f),
                ) { Text(stringResource(R.string.voice_save_button)) }
            }
        }
    }
}

@Composable
private fun LocalAppHistoryDetails(
    appId: String,
    details: LocalAppDetails?,
    onAction: (LocalAppsAction) -> Unit,
) {
    var pendingRestore by remember(appId) { mutableStateOf<LocalAppCheckpoint?>(null) }
    LazyColumn(
        verticalArrangement = Arrangement.spacedBy(8.dp),
        modifier = Modifier.fillMaxSize().padding(16.dp),
    ) {
        item {
            Text(stringResource(R.string.local_apps_history_notice))
        }
        if (details == null) item { LinearProgressIndicator(Modifier.fillMaxWidth()) }
        if (details != null && details.checkpoints.isEmpty()) item { Text(stringResource(R.string.local_apps_no_checkpoints_detail)) }
        items(details?.checkpoints.orEmpty(), key = { it.id }) { checkpoint ->
            Card {
                Row(Modifier.fillMaxWidth().padding(14.dp), verticalAlignment = Alignment.CenterVertically) {
                    Column(Modifier.weight(1f)) {
                        Text(checkpoint.label, fontWeight = FontWeight.SemiBold)
                        Text(
                            "${checkpoint.kind} · ${DateFormat.getDateTimeInstance().format(Date(checkpoint.createdAtMs))}",
                            style = MaterialTheme.typography.bodySmall,
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                        )
                    }
                    TextButton(onClick = { pendingRestore = checkpoint }) { Text(stringResource(R.string.local_apps_checkpoint_restore_action)) }
                }
            }
        }
    }
    pendingRestore?.let { checkpoint ->
        AlertDialog(
            onDismissRequest = { pendingRestore = null },
            title = { Text(stringResource(R.string.local_apps_restore_checkpoint_confirm_title)) },
            text = {
                Text(
                    stringResource(R.string.local_apps_restore_checkpoint_confirm_detail, checkpoint.label),
                )
            },
            confirmButton = {
                Button(onClick = {
                    pendingRestore = null
                    onAction(LocalAppsAction.RestoreCheckpoint(appId, checkpoint.id))
                }) { Text(stringResource(R.string.onboarding_cta_continue)) }
            },
            dismissButton = { TextButton(onClick = { pendingRestore = null }) { Text(stringResource(R.string.common_cancel)) } },
        )
    }
}

@Composable
private fun LocalAppPermissionsDetails(
    appId: String,
    details: LocalAppDetails?,
    onAction: (LocalAppsAction) -> Unit,
) {
    var confirmReset by remember { mutableStateOf(false) }
    LazyColumn(
        verticalArrangement = Arrangement.spacedBy(12.dp),
        modifier = Modifier.fillMaxSize().padding(16.dp),
    ) {
        item {
            Text(stringResource(R.string.local_apps_capability_section_title), style = MaterialTheme.typography.titleMedium)
            Text(stringResource(R.string.local_apps_capability_section_detail))
            OutlinedButton(onClick = { confirmReset = true }) {
                Text(stringResource(R.string.local_apps_permissions_reset))
            }
        }
        item {
            Text(stringResource(R.string.local_apps_manifest_domains_title), style = MaterialTheme.typography.titleMedium)
            if (details?.allowedDomains.isNullOrEmpty()) {
                Text(stringResource(R.string.local_apps_no_domains_detail), color = MaterialTheme.colorScheme.onSurfaceVariant)
            } else {
                details?.allowedDomains.orEmpty().forEach { Text("• $it") }
            }
        }
        item {
            Text(stringResource(R.string.local_apps_runtime_log_title), style = MaterialTheme.typography.titleMedium)
            Text(
                stringResource(
                    R.string.local_apps_runtime_log_status,
                    details?.runtime?.state?.label() ?: stringResource(R.string.local_apps_loading_generic),
                ),
            )
            details?.runtime?.mode?.let { Text(stringResource(R.string.local_apps_runtime_log_mode, it.name)) }
            details?.runtime?.recovery?.let { Text(stringResource(R.string.local_apps_runtime_log_recovery, it)) }
            details?.runtime?.detail?.let { Text(it, color = MaterialTheme.colorScheme.error) }
            if (details?.runtime?.detail == null) {
                Text(stringResource(R.string.local_apps_runtime_log_none))
            }
        }
    }
    if (confirmReset) {
        AlertDialog(
            onDismissRequest = { confirmReset = false },
            title = { Text(stringResource(R.string.local_apps_permissions_reset_title)) },
            text = { Text(stringResource(R.string.local_apps_permissions_reset_confirm_detail)) },
            confirmButton = {
                Button(onClick = {
                    confirmReset = false
                    onAction(LocalAppsAction.ResetPermissions(appId))
                }) { Text(stringResource(R.string.local_apps_revoke_action)) }
            },
            dismissButton = {
                TextButton(onClick = { confirmReset = false }) { Text(stringResource(R.string.common_cancel)) }
            },
        )
    }
}

@Composable
private fun DetailsPlaceholder(title: String, detail: String) {
    Column(verticalArrangement = Arrangement.Center, horizontalAlignment = Alignment.CenterHorizontally, modifier = Modifier.fillMaxSize().padding(32.dp)) {
        Text(title, style = MaterialTheme.typography.titleLarge)
        Spacer(Modifier.height(8.dp))
        Text(detail, style = MaterialTheme.typography.bodyMedium)
    }
}

@Composable
private fun BoundLocalAppWebView(
    appId: String,
    url: String,
    state: LocalAppsUiState,
    onAction: (LocalAppsAction) -> Unit,
    onExternalNavigation: (String) -> Unit,
    modifier: Modifier = Modifier,
) {
    var controller by remember(appId) { mutableStateOf<LocalAppWebViewController?>(null) }
    val responses = state.bridgeResults.values.filter { it.appId == appId }
    val uiRequest = state.pendingUiAction?.takeIf { it.appId == appId }
    LaunchedEffect(controller, responses) {
        val activeController = controller ?: return@LaunchedEffect
        responses.forEach { result ->
            activeController.resolveBridgeRequest(result.requestId, result.ok, result.payloadJson)
            onAction(LocalAppsAction.AcknowledgeBridgeResult(result.requestId))
        }
    }
    LaunchedEffect(controller, uiRequest) {
        val activeController = controller ?: return@LaunchedEffect
        val request = uiRequest ?: return@LaunchedEffect
        activeController.execute(request.action) { result ->
            onAction(
                LocalAppsAction.UiActionHandled(
                    requestId = request.requestId,
                    resultJson = result.resultJson,
                    error = result.error,
                ),
            )
        }
    }
    LocalAppWebView(
        appId = appId,
        url = url,
        onExternalNavigation = onExternalNavigation,
        modifier = modifier,
        onBridgeRequest = { onAction(LocalAppsAction.BridgeRequest(it)) },
        onControllerReady = { controller = it },
    )
}

@Composable
private fun AuthorizationDialog(
    request: LocalAppAuthorizationRequest,
    onAction: (LocalAppsAction) -> Unit,
) {
    AlertDialog(
        onDismissRequest = { onAction(LocalAppsAction.ResolveAuthorization(LocalAppAuthorizationDecision.Deny)) },
        title = { Text(request.title) },
        text = {
            Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                Text(request.reason)
                Text(
                    stringResource(R.string.local_apps_authorization_app_id, request.appId),
                    style = MaterialTheme.typography.bodySmall,
                )
                Button(
                    onClick = { onAction(LocalAppsAction.ResolveAuthorization(LocalAppAuthorizationDecision.AllowOnce)) },
                    modifier = Modifier.fillMaxWidth(),
                ) { Text(stringResource(R.string.local_apps_allow_once)) }
                OutlinedButton(
                    onClick = { onAction(LocalAppsAction.ResolveAuthorization(LocalAppAuthorizationDecision.AllowSession)) },
                    modifier = Modifier.fillMaxWidth(),
                ) { Text(stringResource(R.string.local_apps_allow_session)) }
                TextButton(
                    onClick = { onAction(LocalAppsAction.ResolveAuthorization(LocalAppAuthorizationDecision.AllowAlways)) },
                    modifier = Modifier.fillMaxWidth(),
                ) { Text(stringResource(R.string.local_apps_allow_always)) }
            }
        },
        confirmButton = {},
        dismissButton = {
            TextButton(onClick = { onAction(LocalAppsAction.ResolveAuthorization(LocalAppAuthorizationDecision.Deny)) }) {
                Text(stringResource(R.string.local_apps_deny))
            }
        },
    )
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun LocalAppsTopBar(title: String, onBack: () -> Unit) {
    TopAppBar(
        title = { Text(title, maxLines = 1, overflow = TextOverflow.Ellipsis) },
        navigationIcon = {
            IconButton(onClick = onBack) {
                Icon(Icons.AutoMirrored.Rounded.ArrowBack, contentDescription = stringResource(R.string.local_apps_ui_action_back))
            }
        },
    )
}

@Composable
private fun LocalAppWorkflow.label(): String = when (this) {
    LocalAppWorkflow.AuthoringQuestionnaire -> stringResource(R.string.local_apps_workflow_authoring_questionnaire)
    LocalAppWorkflow.QuestionnaireFailed -> stringResource(R.string.local_apps_workflow_questionnaire_failed)
    LocalAppWorkflow.CollectingSpec -> stringResource(R.string.local_apps_workflow_collecting_spec)
    LocalAppWorkflow.Planning -> stringResource(R.string.local_apps_workflow_planning)
    LocalAppWorkflow.PlanFailed -> stringResource(R.string.local_apps_workflow_plan_failed)
    LocalAppWorkflow.AwaitingSpecConfirmation -> stringResource(R.string.local_apps_workflow_awaiting_spec_confirm)
    LocalAppWorkflow.Generating -> stringResource(R.string.chat_run_generating)
    LocalAppWorkflow.Validating -> stringResource(R.string.local_apps_workflow_validating_label)
    LocalAppWorkflow.AwaitingPreviewConfirmation -> stringResource(R.string.local_apps_workflow_awaiting_preview_label)
    LocalAppWorkflow.Revising -> stringResource(R.string.local_apps_workflow_revising_label)
    LocalAppWorkflow.Ready -> stringResource(R.string.settings_linux_state_ready)
    LocalAppWorkflow.GenerationFailed -> stringResource(R.string.local_apps_workflow_generation_failed)
    LocalAppWorkflow.ValidationFailed -> stringResource(R.string.local_apps_workflow_validation_failed)
}

@Composable
private fun LocalAppRuntimeState.label(): String = when (this) {
    LocalAppRuntimeState.Stopped -> stringResource(R.string.local_apps_runtime_stopped)
    LocalAppRuntimeState.Starting -> stringResource(R.string.settings_cu_state_starting)
    LocalAppRuntimeState.Running -> stringResource(R.string.local_apps_runtime_running)
    LocalAppRuntimeState.Stopping -> stringResource(R.string.settings_cu_state_stopping)
    LocalAppRuntimeState.Failed -> stringResource(R.string.chat_run_status_failed)
}

@Composable
private fun LocalAppDataFieldType.label(): String = when (this) {
    LocalAppDataFieldType.Text -> stringResource(R.string.local_apps_data_field_type_text)
    LocalAppDataFieldType.LongText -> stringResource(R.string.local_apps_data_field_type_long_text)
    LocalAppDataFieldType.Integer -> stringResource(R.string.local_apps_data_field_type_integer)
    LocalAppDataFieldType.Decimal -> stringResource(R.string.local_apps_data_field_type_decimal)
    LocalAppDataFieldType.Boolean -> stringResource(R.string.local_apps_data_field_type_boolean)
    LocalAppDataFieldType.DateTime -> stringResource(R.string.local_apps_data_field_type_datetime)
    LocalAppDataFieldType.Enum -> stringResource(R.string.local_apps_data_field_type_enum)
    LocalAppDataFieldType.ImageRef -> stringResource(R.string.local_apps_data_field_type_image_ref)
}

@Composable
private fun LocalAppDetailsTab.label(): String = when (this) {
    LocalAppDetailsTab.Preview -> stringResource(R.string.local_apps_section_preview)
    LocalAppDetailsTab.Data -> stringResource(R.string.local_apps_section_data)
    LocalAppDetailsTab.Code -> stringResource(R.string.local_apps_section_code)
    LocalAppDetailsTab.History -> stringResource(R.string.local_apps_section_history)
    LocalAppDetailsTab.PermissionsLogs -> stringResource(R.string.local_apps_section_permissions_logs)
}

private fun LocalAppDesignField.emptyValue(): LocalAppDesignValue = when (kind) {
    LocalAppFieldKind.ShortText, LocalAppFieldKind.LongText, LocalAppFieldKind.Color -> LocalAppDesignValue.Text("")
    LocalAppFieldKind.SingleChoice -> LocalAppDesignValue.Choice(options.firstOrNull()?.value.orEmpty())
    LocalAppFieldKind.MultipleChoice -> LocalAppDesignValue.Choices(emptyList())
    LocalAppFieldKind.Boolean -> LocalAppDesignValue.Toggle(false)
    LocalAppFieldKind.Density -> LocalAppDesignValue.Density(compact = false)
    LocalAppFieldKind.ScreenList, LocalAppFieldKind.FeatureList, LocalAppFieldKind.DomainList -> LocalAppDesignValue.StringList(emptyList())
    LocalAppFieldKind.DataFieldList -> LocalAppDesignValue.DataFields(emptyList())
}

private fun LocalAppDesignValue.isPresent(): Boolean = when (this) {
    is LocalAppDesignValue.Text -> value.isNotBlank()
    is LocalAppDesignValue.Choice -> value.isNotBlank()
    is LocalAppDesignValue.Choices -> values.isNotEmpty()
    is LocalAppDesignValue.Toggle -> true
    is LocalAppDesignValue.Density -> true
    is LocalAppDesignValue.StringList -> values.isNotEmpty()
    is LocalAppDesignValue.DataFields -> values.isNotEmpty() && values.all { it.id.isNotBlank() && it.label.isNotBlank() }
    // The user explicitly chose to let the LLM decide — that IS a complete
    // answer, not a missing one (local-apps#questionnaire, Task 1/13/19: the
    // core gate treats `Deferred` as satisfying a required field the same way).
    LocalAppDesignValue.Deferred -> true
}

private fun List<LocalAppDataField>.updated(index: Int, value: LocalAppDataField): List<LocalAppDataField> =
    mapIndexed { current, item -> if (current == index) value else item }

// The engine's manifest contract is ^[a-z][a-z0-9_]{0,63}$ (manifest.rs
// validate_identifier); a hyphen fails generation with no way back to an
// editable draft.
internal fun generateFieldId(fields: List<LocalAppDataField>): String {
    var index = fields.size + 1
    while (fields.any { it.id == "field_$index" }) index += 1
    return "field_$index"
}
