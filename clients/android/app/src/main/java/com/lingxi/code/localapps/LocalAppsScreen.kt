package com.lingxi.code.localapps

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.RowScope
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
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
import androidx.compose.material.icons.rounded.OpenInFull
import androidx.compose.material.icons.rounded.Pause
import androidx.compose.material.icons.rounded.PlayArrow
import androidx.compose.material.icons.rounded.Refresh
import androidx.compose.material.icons.rounded.Stop
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.AssistChip
import androidx.compose.material3.Button
import androidx.compose.material3.Card
import androidx.compose.material3.CardDefaults
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
import androidx.compose.ui.draw.alpha
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
    /**
     * Leave the local-apps surface and switch the conversation into the app's
     * scope: `session != null` resumes that catalog row, `null` starts a fresh
     * session in the app's workspace. Wired by the root shell.
     */
    onOpenAppSession: (appId: String, session: LocalAppSessionRow?) -> Unit = { _, _ -> },
) {
    val state by viewModel.uiState.collectAsStateWithLifecycle()
    LocalAppsScreen(
        state = state,
        onAction = viewModel::onAction,
        onOpenDrawer = onOpenDrawer,
        onExternalNavigation = onExternalNavigation,
        onOpenAppSession = onOpenAppSession,
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
    onOpenAppSession: (appId: String, session: LocalAppSessionRow?) -> Unit = { _, _ -> },
) {
    Box(modifier.fillMaxSize()) {
        when (val destination = state.destination) {
            LocalAppsDestination.Library -> LocalAppsLibraryScreen(
                state = state,
                onAction = onAction,
                onOpenDrawer = onOpenDrawer,
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
                onOpenAppSession = onOpenAppSession,
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
        state.pendingDependencyChangeConfirmation?.let { request ->
            DependencyChangeConfirmationDialog(request = request, onAction = onAction)
        }
    }
}

@Composable
fun LocalAppApprovalSheetDialog(
    sheet: LocalAppApprovalSheet,
    onAction: (LocalAppsAction) -> Unit,
) {
    val context = LocalContext.current
    val canApprove = sheet.state == LocalAppApprovalReceiptState.Pending
    AlertDialog(
        onDismissRequest = { onAction(LocalAppsAction.ResolveApprovalSheet(false)) },
        title = {
            Text(
                when (sheet) {
                    is LocalAppCreateApprovalSheet -> stringResource(R.string.local_apps_create_confirm_title)
                    is LocalAppMcpProposalApprovalSheet -> stringResource(R.string.local_apps_mcp_proposal_title)
                    is LocalAppProfileApprovalSheet -> stringResource(R.string.local_apps_profile_proposal_title)
                },
            )
        },
        text = {
            Column(
                verticalArrangement = Arrangement.spacedBy(8.dp),
                modifier = Modifier
                    .widthIn(max = 640.dp)
                    .heightIn(max = 480.dp)
                    .verticalScroll(rememberScrollState()),
            ) {
                when (sheet) {
                    is LocalAppCreateApprovalSheet -> {
                        LocalAppApprovalFact(
                            label = stringResource(R.string.local_apps_create_confirm_app_name),
                            value = sheet.appName,
                        )
                        if (sheet.brief.isNotBlank()) {
                            LocalAppApprovalFact(
                                label = stringResource(R.string.local_apps_create_confirm_app_brief),
                                value = sheet.brief,
                            )
                        }
                        LocalAppApprovalFact(
                            label = stringResource(R.string.local_apps_create_confirm_selected_template),
                            value = sheet.templateName,
                        )
                        LocalAppApprovalFact(
                            label = stringResource(R.string.local_apps_create_confirm_runtime_profile),
                            value = buildString {
                                append(sheet.runtimeProfile.family.label(context))
                                append('\n')
                                append("r${sheet.runtimeProfile.revision} · ${sheet.runtimeProfile.surface.label(context)}")
                                if (sheet.dependencies.isNotEmpty()) {
                                    append('\n')
                                    append(
                                        sheet.dependencies.joinToString("\n") { dependency ->
                                            listOfNotNull(
                                                dependency.packageName,
                                                dependency.version?.let { "@$it" },
                                                dependency.downloadStatus?.localizedDependencyStatus(context),
                                            ).joinToString(" ")
                                        },
                                    )
                                }
                            },
                        )
                        LocalAppApprovalFact(
                            label = stringResource(R.string.local_apps_create_confirm_agent_reason),
                            value = sheet.reason,
                        )
                        if (sheet.rejectedCandidates.isNotEmpty()) {
                            LocalAppApprovalFact(
                                label = stringResource(R.string.local_apps_create_confirm_rejected_candidates),
                                value = sheet.rejectedCandidates.joinToString("\n"),
                            )
                        }
                        if (sheet.initialTools.isNotEmpty()) {
                            LocalAppApprovalFact(
                                label = stringResource(R.string.local_apps_create_confirm_initial_tools),
                                value = sheet.initialTools.joinToString("\n") { tool ->
                                    listOfNotNull(tool.name, tool.summary.takeUnless { it == tool.name }).joinToString(" · ")
                                },
                            )
                        }
                        if (sheet.permissionCeilings.isNotEmpty()) {
                            LocalAppApprovalFact(
                                label = stringResource(R.string.local_apps_create_confirm_permission_ceiling),
                                value = sheet.permissionCeilings.joinToString("\n"),
                            )
                        }
                        if (sheet.gates.isNotEmpty()) {
                            LocalAppApprovalFact(
                                label = stringResource(R.string.local_apps_create_confirm_required_gates),
                                value = sheet.gates.joinToString("\n") { gate ->
                                    buildString {
                                        append(gate.name)
                                        append(" · ")
                                        append(gate.status.label(context))
                                        gate.detail?.takeIf { it.isNotBlank() }?.let {
                                            append(" · ")
                                            append(it)
                                        }
                                    }
                                },
                            )
                        }
                    }
                    is LocalAppMcpProposalApprovalSheet -> {
                        if (sheet.summary.isNotBlank()) {
                            Text(sheet.summary, style = MaterialTheme.typography.bodyMedium)
                        }
                        val added = sheet.toolDiffs.filter { it.before == null && it.after != null }
                        val removed = sheet.toolDiffs.filter { it.before != null && it.after == null }
                        val changed = sheet.toolDiffs.filter { it.before != null && it.after != null && it.changedFields.isNotEmpty() }
                        if (added.isNotEmpty()) {
                            LocalAppApprovalFact(
                                label = stringResource(R.string.local_apps_mcp_proposal_added),
                                value = added.joinToString("\n") { diff ->
                                    listOfNotNull(
                                        diff.after?.name ?: diff.name,
                                        diff.after?.title,
                                        diff.after?.description,
                                    ).joinToString(" · ")
                                },
                            )
                        }
                        if (removed.isNotEmpty()) {
                            LocalAppApprovalFact(
                                label = stringResource(R.string.local_apps_mcp_proposal_removed),
                                value = removed.joinToString("\n") { it.before?.name ?: it.name },
                            )
                        }
                        changed.forEach { tool ->
                            LocalAppMcpToolDiffCard(tool, context)
                        }
                        if (sheet.requiredChanges.isNotEmpty()) {
                            LocalAppApprovalFact(
                                label = stringResource(R.string.local_apps_mcp_proposal_required_flow_changes),
                                value = sheet.requiredChanges.joinToString("\n"),
                            )
                        }
                        if (sheet.excludedCapabilities.isNotEmpty()) {
                            LocalAppApprovalFact(
                                label = stringResource(R.string.local_apps_mcp_proposal_excluded_capabilities),
                                value = sheet.excludedCapabilities.joinToString("\n"),
                            )
                        }
                        if (sheet.pendingGates.isNotEmpty()) {
                            LocalAppApprovalFact(
                                label = stringResource(R.string.local_apps_create_confirm_required_gates),
                                value = sheet.pendingGates.joinToString("\n") { gate ->
                                    buildString {
                                        append(gate.name)
                                        append(" · ")
                                        append(gate.status.label(context))
                                        gate.detail?.takeIf { it.isNotBlank() }?.let {
                                            append(" · ")
                                            append(it)
                                        }
                                    }
                                },
                            )
                        }
                        if (sheet.state == LocalAppApprovalReceiptState.Superseded) {
                            Text(
                                text = stringResource(R.string.local_apps_mcp_proposal_receipt_superseded),
                                style = MaterialTheme.typography.bodySmall,
                                color = MaterialTheme.colorScheme.onSurfaceVariant,
                            )
                        }
                    }
                    is LocalAppProfileApprovalSheet -> {
                        LocalAppApprovalFact(
                            label = stringResource(R.string.local_apps_create_confirm_agent_reason),
                            value = sheet.reason,
                        )
                        LocalAppApprovalFact(
                            label = stringResource(R.string.local_apps_profile_proposal_navigation_title),
                            value = sheet.instructions,
                        )
                    }
                }
                sheet.expiresAtMs?.let { expiresAtMs ->
                    Text(
                        text = when (sheet) {
                            is LocalAppCreateApprovalSheet -> stringResource(
                                R.string.local_apps_create_confirm_receipt_expires_fmt,
                                DateFormat.getDateTimeInstance().format(Date(expiresAtMs)),
                            )
                            is LocalAppMcpProposalApprovalSheet -> stringResource(
                                R.string.local_apps_mcp_proposal_receipt_expires_fmt,
                                DateFormat.getDateTimeInstance().format(Date(expiresAtMs)),
                            )
                            is LocalAppProfileApprovalSheet -> DateFormat.getDateTimeInstance().format(Date(expiresAtMs))
                        },
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                }
            }
        },
        confirmButton = {
            Button(
                onClick = { onAction(LocalAppsAction.ResolveApprovalSheet(true)) },
                enabled = canApprove,
            ) {
                Text(
                    when (sheet) {
                        is LocalAppCreateApprovalSheet -> stringResource(R.string.local_apps_create_confirm_approve)
                        is LocalAppMcpProposalApprovalSheet -> stringResource(R.string.local_apps_mcp_proposal_approve)
                        is LocalAppProfileApprovalSheet -> stringResource(R.string.local_apps_profile_proposal_apply)
                    },
                )
            }
        },
        dismissButton = {
            TextButton(onClick = { onAction(LocalAppsAction.ResolveApprovalSheet(false)) }) {
                Text(
                    when (sheet) {
                        is LocalAppCreateApprovalSheet -> stringResource(R.string.local_apps_create_confirm_reject)
                        is LocalAppMcpProposalApprovalSheet -> stringResource(R.string.local_apps_mcp_proposal_reject)
                        is LocalAppProfileApprovalSheet -> stringResource(R.string.common_cancel)
                    },
                )
            }
        },
    )
}

@Composable
private fun LocalAppApprovalFact(label: String, value: String) {
    Column(verticalArrangement = Arrangement.spacedBy(2.dp)) {
        Text(label, style = MaterialTheme.typography.labelMedium, fontWeight = FontWeight.SemiBold)
        Text(value, style = MaterialTheme.typography.bodyMedium)
    }
}

@Composable
private fun LocalAppMcpToolDiffCard(
    diff: LocalAppApprovalToolDiff,
    context: android.content.Context,
) {
    Column(
        verticalArrangement = Arrangement.spacedBy(6.dp),
        modifier = Modifier.semantics { heading() },
    ) {
        Text(
            text = "${stringResource(R.string.local_apps_mcp_proposal_changed)} · ${diff.name}",
            style = MaterialTheme.typography.labelLarge,
            fontWeight = FontWeight.SemiBold,
        )
        LocalAppMcpToolSurfaceFact(
            title = stringResource(R.string.local_apps_mcp_proposal_before),
            surface = diff.before,
            changedFields = diff.changedFields,
            context = context,
        )
        LocalAppMcpToolSurfaceFact(
            title = stringResource(R.string.local_apps_mcp_proposal_after),
            surface = diff.after,
            changedFields = diff.changedFields,
            context = context,
        )
    }
}

@Composable
private fun LocalAppMcpToolSurfaceFact(
    title: String,
    surface: LocalAppApprovalToolSurface?,
    changedFields: List<LocalAppApprovalToolField>,
    context: android.content.Context,
) {
    if (surface == null) return
    LocalAppApprovalFact(
        label = title,
        value = changedFields.joinToString("\n\n") { field ->
            buildString {
                append(field.label(context))
                append('\n')
                append(surface.valueFor(field))
            }
        },
    )
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun LocalAppsLibraryScreen(
    state: LocalAppsUiState,
    onAction: (LocalAppsAction) -> Unit,
    onOpenDrawer: () -> Unit,
) {
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
                    // No form. The button creates an empty shell app and the
                    // conversation that follows is where its shape, its name and
                    // its requirements get settled.
                    IconButton(onClick = { onAction(LocalAppsAction.Create) }) {
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

                state.filteredApps.isEmpty() -> EmptyApps(
                    onCreate = { onAction(LocalAppsAction.Create) },
                )

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
}

@Composable
private fun RuntimeModeBanner(mode: LocalAppRuntimeMode) {
    val direct = mode == LocalAppRuntimeMode.ViteStatic
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

/**
 * One library row.
 *
 * A DRAFT (`scaffolded == false`) renders the localized 「新应用」/「创建中」 pair
 * instead of its stored identity — [localAppCardText] is the single predicate,
 * shared with every other render point, and it is what keeps the engine's
 * non-localized `"untitled"` placeholder and the empty brief off this screen.
 * Tapping stays a plain [LocalAppsAction.OpenApp]; the view model is what
 * splits a draft (resume its pinned conversation) from a formed app (open its
 * Details), so the predicate lives in exactly one place. Deleting still works.
 */
@Composable
private fun LocalAppCard(app: LocalAppItem, onAction: (LocalAppsAction) -> Unit) {
    var menuExpanded by remember(app.id) { mutableStateOf(false) }
    var confirmDelete by remember(app.id) { mutableStateOf(false) }
    val draftTitle = stringResource(R.string.local_apps_draft_card_title)
    val cardText = localAppCardText(
        app = app,
        draftTitle = draftTitle,
        draftSubtitle = stringResource(R.string.local_apps_draft_card_subtitle),
    )
    Card(
        onClick = { onAction(LocalAppsAction.OpenApp(app.id)) },
        colors = CardDefaults.cardColors(containerColor = MaterialTheme.colorScheme.surfaceContainer),
        modifier = Modifier.fillMaxWidth(),
    ) {
        Column(Modifier.padding(16.dp)) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                Column(Modifier.weight(1f)) {
                    Text(cardText.title, style = MaterialTheme.typography.titleMedium, maxLines = 1, overflow = TextOverflow.Ellipsis)
                    Text(
                        cardText.subtitle,
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                    )
                }
                FlowRowBadges(
                    badges = localAppStatusBadges(
                        workflow = app.workflow,
                        runtimeError = app.runtime.detail,
                        runtimeProfileStatus = app.runtimeProfileStatus,
                        mcpVerification = app.mcpVerification,
                        uiVerification = app.uiVerification,
                    ),
                )
                Box {
                    IconButton(onClick = { menuExpanded = true }) {
                        Icon(Icons.Rounded.MoreVert, contentDescription = stringResource(R.string.local_apps_card_menu_a11y))
                    }
                    DropdownMenu(expanded = menuExpanded, onDismissRequest = { menuExpanded = false }) {
                        // The permanent home for the home-screen Widget request.
                        // It used to exist ONLY as a checkbox inside the create
                        // dialog, so deleting that dialog without moving it here
                        // would silently retire the feature. Hidden for a draft:
                        // a shell is excluded from the widget snapshot, so its
                        // widget would be an empty, un-openable tile.
                        if (app.scaffolded) {
                            DropdownMenuItem(
                                text = { Text(stringResource(R.string.local_apps_widget_add)) },
                                onClick = {
                                    menuExpanded = false
                                    onAction(LocalAppsAction.RequestWidget(app.id))
                                },
                            )
                        }
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
            app.runtimeProfileStatus?.let { status ->
                LocalAppRuntimeProfileStatusBadge(status)
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
                } else if (app.workflow.isPublished) {
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
            title = {
                Text(
                    stringResource(
                        R.string.local_apps_delete_confirm,
                        localAppDisplayName(app, draftTitle, fallback = app.id),
                    ),
                )
            },
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
 * A standalone full-screen WebView surface for a running app. The runtime's
 * loopback url is the only preview source (the old generation/preview-confirm
 * gate left the protocol with v3).
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun LocalAppPreviewScreen(
    appId: String,
    state: LocalAppsUiState,
    onAction: (LocalAppsAction) -> Unit,
    onExternalNavigation: (String) -> Unit,
) {
    val app = state.apps.firstOrNull { it.id == appId }
    val previewUrl = state.previewUrl(appId)
    // The host bar is hidden for the RUNNING state only, and restored for every other
    // one — the exact conditional iOS spells as
    //   .toolbar(previewURL == nil ? .visible : .hidden, for: .navigationBar)
    // in LocalAppDetailView.swift's LocalAppPreviewView.
    //
    // Hidden while running because the app draws its OWN header inside the WebView, so
    // a host bar stacked above it read as two title bars; the start/pause and exit
    // controls live in the floating RunPill instead.
    //
    // VISIBLE when there is nothing to render, because the placeholder carries no pill
    // — the pill is composed inside the `previewUrl != null` branch below. Removing the
    // bar unconditionally left that state with no title AND no back affordance: a bare
    // centred Column with a Start button. On a gesture-navigation device with an app
    // that is slow to start, that is a screen with no VISIBLE way off it. System back
    // does still leave the destination in both states (RootScreen.kt's BackHandler maps
    // a back press on a non-Library destination to LocalAppsAction.Back, which is what
    // this navigation icon dispatches too), but "still works" is not "discoverable".
    //
    // `local_apps_section_preview` 「运行」 is the same key iOS puts on
    // `.navigationTitle`, so the two clients name this screen identically.
    Scaffold(
        topBar = {
            if (previewUrl == null) {
                LocalAppsTopBar(
                    stringResource(R.string.local_apps_section_preview),
                    onBack = { onAction(LocalAppsAction.Back) },
                )
            }
        },
    ) { padding ->
        Box(modifier = Modifier.fillMaxSize().padding(padding)) {
            if (previewUrl != null) {
                BoundLocalAppWebView(
                    appId = appId,
                    url = previewUrl,
                    state = state,
                    onAction = onAction,
                    onExternalNavigation = onExternalNavigation,
                    modifier = Modifier.fillMaxSize(),
                )
                // "running" is read off the SAME chain the render predicate resolves.
                // previewUrl() (LocalAppsContract.kt) is
                //   apps.firstOrNull { it.id == appId }?.runtime?.url ?: details[appId]?.runtime?.url
                // — it FALLS BACK to `details`. Reading the state off the `apps` row alone
                // let the two disagree whenever `apps` has no row for an app that is
                // already running: a widget deep link straight into this destination
                // before ListApps lands, or the momentary empty window while reduceApps()
                // rebuilds the list. The WebView then rendered a live app while the pill
                // showed ▶, and tapping it dispatched StartRuntime on a running runtime.
                // `takeIf { it.url != null }` is what makes this elvis land on the same
                // side as previewUrl's: the `apps` runtime wins here exactly when it is
                // the one that supplied the url being rendered.
                val runtime = app?.runtime?.takeIf { it.url != null } ?: state.details[appId]?.runtime
                // Exhaustive `when`, no `else`: a state added to LocalAppRuntimeState
                // must break this compile rather than silently pick a button.
                //
                // STOPPING counts as running, and that is the whole point of this
                // block. A url OUTLIVES the running state on three independent layers:
                //   · the wire — `lower_runtime_details` (local_apps_bridge.rs:402)
                //     builds `loopback_url: runtime.port.map(|port| …)`, a pure
                //     function of the PORT that never consults the state;
                //   · the engine — `stop_runtime` (local_apps_host.rs) writes
                //     `update_runtime_record(app_id, Stopping, runtime.port, …)`, i.e.
                //     it keeps the port through the whole shutdown window;
                //   · this client — the AppRuntimeChanged reducer's
                //     `?: app.runtime.copy(state = …)` preserves the previous url
                //     whenever the event arrives without a details payload.
                // So `previewUrl` stays non-null across a shutdown, this pill stays
                // composed over the page, and calling STOPPING "not running" would draw
                // ▶ / local_apps_run_start and dispatch StartRuntime into a runtime the
                // engine is tearing down — which nothing downstream catches:
                // `startRuntimeIfNeeded` short-circuits only on Running/Starting, so
                // Stopping falls straight through to `ClientCommand.StartApp`.
                // Offering "stop" instead costs nothing: `stop_runtime` has already
                // removed the entry from its in-memory table by the time the record
                // says Stopping, so a second StopApp hits the `None =>` arm and returns
                // `{"state":"stopped"}` without touching the runtime.
                val running = when (runtime?.state) {
                    LocalAppRuntimeState.Running,
                    LocalAppRuntimeState.Starting,
                    LocalAppRuntimeState.Stopping -> true
                    // A stale url outliving a dead runtime: the page on screen cannot
                    // be paused, and ▶ genuinely does bring it back, so start is the
                    // honest offer here.
                    LocalAppRuntimeState.Stopped,
                    LocalAppRuntimeState.Failed -> false
                    // Unreachable — `previewUrl != null` means one of the two sources
                    // above resolved a runtime — but named rather than folded into an
                    // `else`, so this stays a compile-time exhaustive match.
                    null -> false
                }
                RunPill(
                    running = running,
                    onToggle = {
                        if (running) {
                            onAction(LocalAppsAction.StopRuntime(appId))
                        } else {
                            onAction(LocalAppsAction.StartRuntime(appId))
                        }
                    },
                    onExit = { onAction(LocalAppsAction.Back) },
                    // Bottom-START, not bottom-end, and it must stay that way: an Ionic
                    // page parks its OWN furniture in the bottom-end corner — IonFab
                    // defaults to vertical="bottom" horizontal="end", and the right-most
                    // IonTabBar tab lands there too — so a host control pinned bottom-end
                    // sits on top of the app's own button. iOS's twin is .bottomLeading
                    // (LocalAppDetailView.swift) for the same reason; the two clients must
                    // match. Alignment.BottomStart is layout-direction aware, so this is
                    // the leading corner in RTL as well.
                    //
                    // A plain padding, deliberately: RootScreen.kt wraps this whole route in
                    // windowInsetsPadding(WindowInsets.systemBars), which CONSUMES the insets,
                    // so a navigationBarsPadding() here would measure zero on device.
                    modifier = Modifier.align(Alignment.BottomStart).padding(16.dp),
                )
            } else {
                // Not running: no app content to fill the screen, so no pill — the centred
                // placeholder keeps its own start button, and back still exits.
                Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) {
                    Column(horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.spacedBy(12.dp)) {
                        Text(stringResource(R.string.local_apps_not_running_detail))
                        Button(onClick = { onAction(LocalAppsAction.StartRuntime(appId)) }) {
                            Text(stringResource(R.string.common_start))
                        }
                    }
                }
            }
        }
    }
}

/**
 * Every button in [RunPill], the collapsed toggle included.
 *
 * 48dp is Android's documented minimum touch target (iOS's twin uses 44pt, its
 * own platform minimum, at `LocalAppRunControl.tapTarget`). Stated explicitly
 * rather than left to Material3's `minimumInteractiveComponentSize()` because
 * this is a load-bearing size, not a default worth inheriting silently: with
 * the host bar hidden this pill is the ONLY host affordance on the screen, and
 * for an app generated from the canvas scaffold it is the only affordance at
 * all — that template renders a `<canvas>` and overlays, with no header of its
 * own. A missed tap here has no fallback.
 */
private val RunPillTapTarget = 48.dp

/**
 * The run surface's only host control: a floating pill over the running app.
 * Collapsed it is one dimmed button so it stays out of the app's way; tapping it
 * expands to pause/start and exit, and tapping it AGAIN collapses it back — the
 * toggle is composed in both states, exactly like iOS's `expanded.toggle()`.
 * "Pause" stops the runtime — the app content disappears and the placeholder
 * returns; there is no freeze-the-frame pause.
 *
 * Child order is strictly [toggle, pause-or-start, exit], identical to iOS's
 * `LocalAppRunControl`.
 */
@Composable
private fun RunPill(
    running: Boolean,
    onToggle: () -> Unit,
    onExit: () -> Unit,
    modifier: Modifier = Modifier,
) {
    var expanded by remember { mutableStateOf(false) }
    Surface(
        color = MaterialTheme.colorScheme.secondaryContainer,
        shape = RoundedCornerShape(24.dp),
        modifier = modifier.alpha(if (expanded) 1f else 0.6f),
    ) {
        Row(
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(4.dp),
            modifier = Modifier.padding(horizontal = 4.dp),
        ) {
            // FIRST, and outside the `if` — both halves matter.
            //
            // OUTSIDE, and it flips rather than sets: this button used to live in an
            // `else` branch and only ever assign `expanded = true`, so the single
            // writer of the state stopped being composed the instant it ran. Nothing
            // left in the tree could set it back, and the expanded pill stayed pinned
            // at full opacity over the app's own corner for the rest of the visit.
            //
            // FIRST rather than last: the pill is anchored bottom-START, so the
            // leading-most child sits on fixed pixels and every sibling after it grows
            // away from the corner. With the toggle last, expanding slid it away by one
            // button and dropped the newly composed FIRST child — pause/stop — onto the
            // exact rect the finger had just tapped, so a second tap at the same point
            // stopped the runtime instead of collapsing the control. Leading-most, its
            // hit rect is identical in both states (a fixed [RunPillTapTarget] square
            // whose leading edge is the Row's 4dp padding regardless of how many
            // siblings follow), so tap-tap always means expand-then-collapse. iOS says
            // the same thing at the same place: "FIRST, not last."
            IconButton(
                onClick = { expanded = !expanded },
                modifier = Modifier.size(RunPillTapTarget),
            ) {
                Icon(Icons.Rounded.MoreVert, contentDescription = stringResource(R.string.local_apps_more))
            }
            if (expanded) {
                IconButton(onClick = onToggle, modifier = Modifier.size(RunPillTapTarget)) {
                    Icon(
                        // Icons.Rounded.Pause, not Icons.Rounded.Stop. The label under
                        // this glyph is local_apps_run_pause 「暂停应用」, and a filled
                        // square is the universal STOP mark — the glyph and its own
                        // accessibility label were saying different things, and the
                        // square also collided with the library card's genuine stop
                        // button (LocalAppCard), which does use Icons.Rounded.Stop.
                        // iOS draws "pause.fill" here for the same key.
                        if (running) Icons.Rounded.Pause else Icons.Rounded.PlayArrow,
                        contentDescription = stringResource(
                            if (running) R.string.local_apps_run_pause else R.string.local_apps_run_start,
                        ),
                    )
                }
                IconButton(onClick = onExit, modifier = Modifier.size(RunPillTapTarget)) {
                    Icon(Icons.Rounded.Close, contentDescription = stringResource(R.string.local_apps_run_exit))
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
    onOpenAppSession: (appId: String, session: LocalAppSessionRow?) -> Unit,
) {
    val app = state.apps.firstOrNull { it.id == appId }
    Scaffold(
        topBar = {
            LocalAppsTopBar(
                localAppDisplayName(
                    app,
                    draftTitle = stringResource(R.string.local_apps_draft_card_title),
                    fallback = stringResource(R.string.local_apps_detail_title),
                ),
                onBack = { onAction(LocalAppsAction.Back) },
                // The details page is the ONLY in-app entrance to the full-bleed
                // run surface — without it `openFromWidget` is the sole writer of
                // `LocalAppsDestination.Preview` and the whole run experience is
                // reachable only from a home-screen widget. Mirrors iOS's
                // 「打开应用」 button on the app's overview.
                //
                // Hidden for an app that is not Ready: the surface would show
                // nothing but the not-running placeholder, and the Preview TAB
                // below already covers inspecting a half-built app.
                actions = {
                    if (app?.workflow?.isPublished == true) {
                        IconButton(onClick = { onAction(LocalAppsAction.OpenRunSurface(appId)) }) {
                            Icon(
                                Icons.Rounded.OpenInFull,
                                contentDescription = stringResource(R.string.local_apps_open_preview),
                            )
                        }
                    }
                },
            )
        },
    ) { padding ->
        Column(Modifier.fillMaxSize().padding(padding)) {
            app?.let {
                FlowRowBadges(
                    badges = localAppStatusBadges(
                        workflow = it.workflow,
                        runtimeError = it.runtime.detail ?: state.details[appId]?.runtime?.detail,
                        runtimeProfileStatus = state.details[appId]?.runtimeProfileStatus ?: it.runtimeProfileStatus,
                        mcpVerification = state.details[appId]?.mcpVerification ?: it.mcpVerification,
                        uiVerification = state.details[appId]?.uiVerification ?: it.uiVerification,
                    ),
                    modifier = Modifier.padding(horizontal = 16.dp, vertical = 8.dp),
                )
                Column(
                    verticalArrangement = Arrangement.spacedBy(4.dp),
                    modifier = Modifier.padding(horizontal = 16.dp),
                ) {
                    (state.details[appId]?.mcpVerification ?: it.mcpVerification)?.let { summary ->
                        VerificationSummaryRow(
                            label = stringResource(R.string.local_apps_verification_mcp),
                            summary = summary,
                        )
                    }
                    (state.details[appId]?.uiVerification ?: it.uiVerification)?.let { summary ->
                        VerificationSummaryRow(
                            label = stringResource(R.string.local_apps_verification_ui),
                            summary = summary,
                        )
                    }
                }
            }
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
                LocalAppDetailsTab.Sessions -> LocalAppSessionsDetails(
                    appId = appId,
                    page = state.appSessions[appId],
                    onAction = onAction,
                    onOpenAppSession = onOpenAppSession,
                )
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
                LocalAppDetailsTab.Mcp -> LocalAppMcpDetails(
                    appId = appId,
                    state = state,
                    onAction = onAction,
                )
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
private fun LocalAppMcpDetails(
    appId: String,
    state: LocalAppsUiState,
    onAction: (LocalAppsAction) -> Unit,
) {
    val managed = state.managedMcp(appId)
    val draft = state.mcpDraft(appId)
    val pending = state.mcpPendingByApp[appId]
    val error = state.mcpErrorByApp[appId]
    val toolSectionEnabled = managed.tools.isNotEmpty() && managed.status != LocalAppManagedMcpStatus.Authoring
    Column(
        verticalArrangement = Arrangement.spacedBy(12.dp),
        modifier = Modifier
            .fillMaxSize()
            .verticalScroll(rememberScrollState())
            .padding(16.dp),
    ) {
        Card(modifier = Modifier.fillMaxWidth()) {
            Column(
                verticalArrangement = Arrangement.spacedBy(10.dp),
                modifier = Modifier.padding(16.dp),
            ) {
                Text(
                    stringResource(R.string.local_apps_mcp_title),
                    style = MaterialTheme.typography.titleMedium,
                )
                Text(
                    stringResource(R.string.local_apps_mcp_status_fmt, managed.status.label()),
                    style = MaterialTheme.typography.bodyMedium,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
                if (pending != null) {
                    LinearProgressIndicator(modifier = Modifier.fillMaxWidth())
                    Text(
                        pending,
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                }
                if (error != null) {
                    Text(
                        error,
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.error,
                    )
                }
                ToggleRow(
                    title = stringResource(R.string.local_apps_mcp_enable_service),
                    subtitle = stringResource(R.string.local_apps_mcp_enable_service_detail),
                    checked = managed.enabled,
                    enabled = managed.tools.isNotEmpty() || managed.status != LocalAppManagedMcpStatus.NeedsSetup,
                    onCheckedChange = { onAction(LocalAppsAction.SetMcpEnabled(appId, it)) },
                )
                ToggleRow(
                    title = stringResource(R.string.local_apps_mcp_pin_current_conversation),
                    subtitle = stringResource(R.string.local_apps_mcp_pin_current_conversation_detail),
                    checked = managed.pinnedToCurrentConversation,
                    enabled = managed.tools.isNotEmpty(),
                    onCheckedChange = { onAction(LocalAppsAction.SetMcpPinnedToConversation(appId, it)) },
                )
                managed.widget?.takeIf { it.available }?.let { widget ->
                    Text(
                        widget.detail ?: widget.label ?: stringResource(R.string.local_apps_mcp_widget_ready),
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                }
                managed.mcpVerification?.let { VerificationSummaryRow(label = stringResource(R.string.local_apps_verification_mcp), summary = it) }
                managed.uiVerification?.let { VerificationSummaryRow(label = stringResource(R.string.local_apps_verification_ui), summary = it) }
            }
        }

        Card(modifier = Modifier.fillMaxWidth()) {
            Column(
                verticalArrangement = Arrangement.spacedBy(12.dp),
                modifier = Modifier.padding(16.dp),
            ) {
                Text(
                    stringResource(R.string.local_apps_mcp_customize_title),
                    style = MaterialTheme.typography.titleMedium,
                )
                OutlinedTextField(
                    value = draft.userGoal,
                    onValueChange = { onAction(LocalAppsAction.UpdateMcpGoal(appId, it)) },
                    label = { Text(stringResource(R.string.local_apps_mcp_goal_label)) },
                    placeholder = { Text(stringResource(R.string.local_apps_mcp_goal_placeholder)) },
                    minLines = 3,
                    modifier = Modifier.fillMaxWidth(),
                )
                Row(horizontalArrangement = Arrangement.spacedBy(8.dp), modifier = Modifier.fillMaxWidth()) {
                    Button(
                        onClick = { onAction(LocalAppsAction.StartMcpAuthoring(appId)) },
                        modifier = Modifier.weight(1f),
                    ) {
                        Text(
                            stringResource(
                                if (managed.status == LocalAppManagedMcpStatus.NeedsSetup) {
                                    R.string.local_apps_mcp_start_authoring
                                } else {
                                    R.string.local_apps_mcp_update_authoring
                                },
                            ),
                        )
                    }
                }
                if (managed.status == LocalAppManagedMcpStatus.NeedsSetup && managed.tools.isEmpty()) {
                    Text(
                        stringResource(R.string.local_apps_mcp_needs_setup_detail),
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                }
            }
        }

        if (managed.tools.isEmpty()) {
            Card(modifier = Modifier.fillMaxWidth()) {
                Column(
                    verticalArrangement = Arrangement.spacedBy(6.dp),
                    modifier = Modifier.padding(16.dp),
                ) {
                    Text(
                        stringResource(R.string.local_apps_mcp_tools_empty_title),
                        style = MaterialTheme.typography.titleSmall,
                    )
                    Text(
                        stringResource(R.string.local_apps_mcp_tools_empty_detail),
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                }
            }
        } else {
            Text(
                stringResource(R.string.local_apps_mcp_tools_title),
                style = MaterialTheme.typography.titleMedium,
            )
            managed.tools.forEach { tool ->
                Card(modifier = Modifier.fillMaxWidth()) {
                    Column(
                        verticalArrangement = Arrangement.spacedBy(10.dp),
                        modifier = Modifier.padding(16.dp),
                    ) {
                        ToggleRow(
                            title = tool.title ?: tool.name,
                            subtitle = tool.description ?: tool.permissionCeiling,
                            checked = tool.enabled,
                            enabled = toolSectionEnabled,
                            onCheckedChange = {
                                onAction(LocalAppsAction.SetMcpToolEnabled(appId, tool.name, it))
                            },
                        )
                        Text(
                            stringResource(R.string.local_apps_mcp_tool_permission_fmt, tool.permissionCeiling),
                            style = MaterialTheme.typography.bodySmall,
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                        )
                    }
                }
            }
        }
    }
}

@Composable
private fun ToggleRow(
    title: String,
    subtitle: String,
    checked: Boolean,
    enabled: Boolean,
    onCheckedChange: (Boolean) -> Unit,
) {
    Row(
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(12.dp),
        modifier = Modifier.fillMaxWidth(),
    ) {
        Column(modifier = Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(4.dp)) {
            Text(title, style = MaterialTheme.typography.bodyLarge)
            Text(
                subtitle,
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
        }
        Switch(checked = checked, onCheckedChange = onCheckedChange, enabled = enabled)
    }
}

/**
 * The Sessions tab — the app's workspace-scoped conversation catalog. The
 * pinned init session renders first with an 「初始化」 badge; tapping any row
 * (or 「新会话」) hands off to the root shell, which switches the conversation
 * into the app's scope. 「加载更多」 appears while the engine reports another
 * page (`next_offset != null`).
 */
@Composable
private fun LocalAppSessionsDetails(
    appId: String,
    page: LocalAppSessionPage?,
    onAction: (LocalAppsAction) -> Unit,
    onOpenAppSession: (appId: String, session: LocalAppSessionRow?) -> Unit,
) {
    LazyColumn(
        verticalArrangement = Arrangement.spacedBy(8.dp),
        modifier = Modifier.fillMaxSize().padding(16.dp),
    ) {
        item(key = "new-session") {
            Button(
                onClick = { onOpenAppSession(appId, null) },
                modifier = Modifier.fillMaxWidth(),
            ) {
                Icon(Icons.Rounded.Add, contentDescription = null, modifier = Modifier.size(18.dp))
                Spacer(Modifier.width(6.dp))
                Text(stringResource(R.string.drawer_project_new_session_short))
            }
        }
        when {
            page == null || !page.loaded -> item(key = "loading") {
                Row(
                    verticalAlignment = Alignment.CenterVertically,
                    horizontalArrangement = Arrangement.spacedBy(10.dp),
                    modifier = Modifier.fillMaxWidth().padding(vertical = 12.dp),
                ) {
                    CircularProgressIndicator(modifier = Modifier.size(18.dp))
                    Text(stringResource(R.string.local_apps_sessions_loading), style = MaterialTheme.typography.bodyMedium)
                }
            }
            page.rows.isEmpty() -> item(key = "empty") {
                Text(
                    stringResource(R.string.local_apps_sessions_empty),
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    modifier = Modifier.padding(vertical = 12.dp),
                )
            }
            else -> items(page.rows, key = { it.uuid }) { row ->
                LocalAppSessionCard(row = row, onClick = { onOpenAppSession(appId, row) })
            }
        }
        if (page?.nextOffset != null) {
            item(key = "load-more") {
                OutlinedButton(
                    onClick = { onAction(LocalAppsAction.LoadAppSessions(appId, page.nextOffset)) },
                    modifier = Modifier.fillMaxWidth(),
                ) { Text(stringResource(R.string.local_apps_session_load_more)) }
            }
        }
    }
}

@Composable
private fun LocalAppSessionCard(row: LocalAppSessionRow, onClick: () -> Unit) {
    Card(onClick = onClick, modifier = Modifier.fillMaxWidth()) {
        Column(Modifier.padding(14.dp), verticalArrangement = Arrangement.spacedBy(4.dp)) {
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                Text(
                    row.title,
                    style = MaterialTheme.typography.titleSmall,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                    modifier = Modifier.weight(1f, fill = false),
                )
                if (row.isInit) {
                    AssistChip(
                        onClick = {},
                        label = { Text(stringResource(R.string.local_apps_session_init_badge)) },
                    )
                }
            }
            Text(
                stringResource(R.string.local_apps_session_meta, row.relativeTime, row.messageCount),
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
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
        details?.runtimeProfileStatus?.let { status ->
            item {
                Column(verticalArrangement = Arrangement.spacedBy(4.dp)) {
                    Text(
                        stringResource(R.string.local_apps_runtime_profile_health_title),
                        style = MaterialTheme.typography.titleMedium,
                    )
                    LocalAppRuntimeProfileStatusBadge(status)
                }
            }
        }
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
            activeController.resolveBridgeRequest(
                requestId = result.requestId,
                ok = result.ok,
                resultJson = result.payloadJson,
                error = result.error,
                errorCode = result.errorCode,
            )
            onAction(LocalAppsAction.AcknowledgeBridgeResult(result.appId, result.requestId))
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
                if (request.allowsPersistentGrant) {
                    OutlinedButton(
                        onClick = { onAction(LocalAppsAction.ResolveAuthorization(LocalAppAuthorizationDecision.AllowSession)) },
                        modifier = Modifier.fillMaxWidth(),
                    ) { Text(stringResource(R.string.local_apps_allow_session)) }
                    TextButton(
                        onClick = { onAction(LocalAppsAction.ResolveAuthorization(LocalAppAuthorizationDecision.AllowAlways)) },
                        modifier = Modifier.fillMaxWidth(),
                    ) { Text(stringResource(R.string.local_apps_allow_always)) }
                }
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

@Composable
private fun DependencyChangeConfirmationDialog(
    request: LocalAppDependencyChangeConfirmationRequest,
    onAction: (LocalAppsAction) -> Unit,
) {
    AlertDialog(
        onDismissRequest = { onAction(LocalAppsAction.ResolveDependencyChangeConfirmation(false)) },
        title = { Text(stringResource(R.string.local_apps_dependency_change_title)) },
        text = {
            LazyColumn(
                modifier = Modifier
                    .fillMaxWidth()
                    .heightIn(max = 520.dp),
                verticalArrangement = Arrangement.spacedBy(12.dp),
            ) {
                item {
                    Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                        Text(request.reason.localizedDependencyConfirmationReason())
                        Text(
                            stringResource(R.string.local_apps_authorization_app_id, request.appId),
                            style = MaterialTheme.typography.bodySmall,
                        )
                    }
                }
                items(request.changes) { change ->
                    Card(
                        colors = CardDefaults.cardColors(containerColor = MaterialTheme.colorScheme.surfaceContainer),
                    ) {
                        Column(
                            modifier = Modifier.padding(12.dp),
                            verticalArrangement = Arrangement.spacedBy(6.dp),
                        ) {
                            Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                                Text(
                                    change.kind.label(),
                                    style = MaterialTheme.typography.labelMedium,
                                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                                )
                                Text(
                                    change.packageName + (change.version?.let { "@$it" } ?: ""),
                                    style = MaterialTheme.typography.titleSmall,
                                )
                            }
                            Text(
                                stringResource(
                                    R.string.local_apps_dependency_change_cache_download,
                                    change.cacheStatus.localizedDependencyStatus(),
                                    change.downloadStatus.localizedDependencyStatus(),
                                ),
                                style = MaterialTheme.typography.bodySmall,
                            )
                        }
                    }
                }
                item {
                    Column(
                        modifier = Modifier
                            .fillMaxWidth()
                            .padding(top = 4.dp),
                        verticalArrangement = Arrangement.spacedBy(6.dp),
                    ) {
                        DependencyPolicyRow(
                            label = stringResource(R.string.local_apps_dependency_change_license_risk),
                            value = request.licenseRisk.localizedDependencyRisk(),
                        )
                        DependencyPolicyRow(
                            label = stringResource(R.string.local_apps_dependency_change_sbom_risk),
                            value = request.sbomRisk.localizedDependencyRisk(),
                        )
                        DependencyPolicyRow(
                            label = stringResource(R.string.local_apps_dependency_change_scripts),
                            value = if (request.lifecycleScriptsBlocked) {
                                stringResource(R.string.local_apps_dependency_change_blocked)
                            } else {
                                stringResource(R.string.local_apps_dependency_change_allowed)
                            },
                        )
                        DependencyPolicyRow(
                            label = stringResource(R.string.local_apps_dependency_change_native_addons),
                            value = if (request.nativeAddonsBlocked) {
                                stringResource(R.string.local_apps_dependency_change_blocked)
                            } else {
                                stringResource(R.string.local_apps_dependency_change_allowed)
                            },
                        )
                        Text(
                            request.rollbackPolicy.localizedDependencyRollbackPolicy(),
                            style = MaterialTheme.typography.bodySmall,
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                            modifier = Modifier.padding(top = 4.dp),
                        )
                    }
                }
            }
        },
        confirmButton = {
            Button(onClick = { onAction(LocalAppsAction.ResolveDependencyChangeConfirmation(true)) }) {
                Text(stringResource(R.string.local_apps_dependency_change_approve))
            }
        },
        dismissButton = {
            TextButton(onClick = { onAction(LocalAppsAction.ResolveDependencyChangeConfirmation(false)) }) {
                Text(stringResource(R.string.common_cancel))
            }
        },
    )
}

@Composable
private fun DependencyPolicyRow(label: String, value: String) {
    Row(
        modifier = Modifier.fillMaxWidth(),
        horizontalArrangement = Arrangement.SpaceBetween,
    ) {
        Text(label, style = MaterialTheme.typography.bodySmall)
        Text(
            value,
            style = MaterialTheme.typography.bodySmall,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
        )
    }
}

@Composable
private fun LocalAppDependencyChangeKind.label(): String = when (this) {
    LocalAppDependencyChangeKind.Add -> stringResource(R.string.local_apps_dependency_change_add)
    LocalAppDependencyChangeKind.Update -> stringResource(R.string.local_apps_dependency_change_update)
    LocalAppDependencyChangeKind.Remove -> stringResource(R.string.local_apps_dependency_change_remove)
}

@Composable
private fun String.localizedDependencyRisk(): String = when (this) {
    "unknown_until_resolution" -> stringResource(R.string.local_apps_dependency_change_unknown_until_resolution)
    else -> this
}

@Composable
private fun String.localizedDependencyConfirmationReason(): String = when (this) {
    "pre_resolution_no_network" -> stringResource(R.string.local_apps_dependency_change_pre_resolution_no_network)
    else -> this
}

@Composable
private fun String.localizedDependencyRollbackPolicy(): String = when (this) {
    "rollback_on_validation_failure" -> stringResource(R.string.local_apps_dependency_change_rollback_on_failure)
    else -> this
}

@Composable
private fun String.localizedDependencyStatus(): String = when (this) {
    "may_be_required" -> stringResource(R.string.local_apps_dependency_change_download_may_be_required)
    "not_required" -> stringResource(R.string.local_apps_dependency_change_download_not_required)
    "not_needed" -> stringResource(R.string.local_apps_dependency_change_cache_not_needed)
    else -> this
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun LocalAppsTopBar(
    title: String,
    onBack: () -> Unit,
    actions: @Composable RowScope.() -> Unit = {},
) {
    TopAppBar(
        title = { Text(title, maxLines = 1, overflow = TextOverflow.Ellipsis) },
        navigationIcon = {
            IconButton(onClick = onBack) {
                Icon(Icons.AutoMirrored.Rounded.ArrowBack, contentDescription = stringResource(R.string.local_apps_ui_action_back))
            }
        },
        actions = actions,
    )
}

@Composable
private fun LocalAppWorkflow.label(): String = when (this) {
    LocalAppWorkflow.Draft -> stringResource(R.string.local_apps_workflow_draft)
    LocalAppWorkflow.PublishedUnverified -> stringResource(R.string.local_apps_verification_status_unverified)
    LocalAppWorkflow.PublishedVerified -> stringResource(R.string.local_apps_verification_status_passed)
}

@Composable
private fun LocalAppManagedMcpStatus.label(): String = when (this) {
    LocalAppManagedMcpStatus.Disabled -> stringResource(R.string.local_apps_mcp_status_disabled)
    LocalAppManagedMcpStatus.NeedsSetup -> stringResource(R.string.local_apps_mcp_status_needs_setup)
    LocalAppManagedMcpStatus.Authoring -> stringResource(R.string.local_apps_mcp_status_authoring)
    LocalAppManagedMcpStatus.Enabled -> stringResource(R.string.local_apps_mcp_status_enabled)
    LocalAppManagedMcpStatus.NeedsRevalidation -> stringResource(R.string.local_apps_mcp_status_needs_revalidation)
    LocalAppManagedMcpStatus.Error -> stringResource(R.string.local_apps_mcp_status_error)
}

@Composable
private fun FlowRowBadges(
    badges: List<LocalAppStatusBadgeKind>,
    modifier: Modifier = Modifier,
) {
    Row(
        horizontalArrangement = Arrangement.spacedBy(8.dp),
        modifier = modifier.fillMaxWidth(),
    ) {
        badges.forEach { badge ->
            AssistChip(
                onClick = {},
                enabled = false,
                label = {
                    Text(
                        when (badge) {
                            LocalAppStatusBadgeKind.Draft -> stringResource(R.string.local_apps_workflow_draft)
                            LocalAppStatusBadgeKind.PublishedUnverified -> stringResource(R.string.local_apps_verification_status_unverified)
                            LocalAppStatusBadgeKind.PublishedVerified -> stringResource(R.string.local_apps_verification_status_passed)
                            LocalAppStatusBadgeKind.VerificationPending -> stringResource(R.string.local_apps_verification_status_pending)
                            LocalAppStatusBadgeKind.VerificationFailed -> stringResource(R.string.local_apps_verification_status_failed)
                            LocalAppStatusBadgeKind.Error -> stringResource(R.string.local_apps_error_title)
                        },
                    )
                },
            )
        }
    }
}

@Composable
private fun VerificationSummaryRow(
    label: String,
    summary: LocalAppVerificationSummary,
) {
    Text(
        text = "$label · ${summary.status.label()} · ${summary.summary}",
        style = MaterialTheme.typography.bodySmall,
        color = MaterialTheme.colorScheme.onSurfaceVariant,
        modifier = Modifier.fillMaxWidth(),
    )
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
private fun LocalAppRuntimeProfileFamily.label(): String = when (this) {
    LocalAppRuntimeProfileFamily.ReactDom -> stringResource(R.string.local_apps_runtime_profile_family_react_dom)
    LocalAppRuntimeProfileFamily.Canvas2d -> stringResource(R.string.local_apps_runtime_profile_family_canvas_2d)
    LocalAppRuntimeProfileFamily.Three3d -> stringResource(R.string.local_apps_runtime_profile_family_three_3d)
    LocalAppRuntimeProfileFamily.Phaser2d -> stringResource(R.string.local_apps_runtime_profile_family_phaser_2d)
    LocalAppRuntimeProfileFamily.Babylon3d -> stringResource(R.string.local_apps_runtime_profile_family_babylon_3d)
}

@Composable
private fun LocalAppRuntimeProfileSurface.label(): String = when (this) {
    LocalAppRuntimeProfileSurface.Dom -> stringResource(R.string.local_apps_runtime_profile_surface_dom)
    LocalAppRuntimeProfileSurface.Canvas -> stringResource(R.string.local_apps_runtime_profile_surface_canvas)
}

private fun LocalAppRuntimeProfileFamily.label(context: android.content.Context): String = when (this) {
    LocalAppRuntimeProfileFamily.ReactDom -> context.getString(R.string.local_apps_runtime_profile_family_react_dom)
    LocalAppRuntimeProfileFamily.Canvas2d -> context.getString(R.string.local_apps_runtime_profile_family_canvas_2d)
    LocalAppRuntimeProfileFamily.Three3d -> context.getString(R.string.local_apps_runtime_profile_family_three_3d)
    LocalAppRuntimeProfileFamily.Phaser2d -> context.getString(R.string.local_apps_runtime_profile_family_phaser_2d)
    LocalAppRuntimeProfileFamily.Babylon3d -> context.getString(R.string.local_apps_runtime_profile_family_babylon_3d)
}

private fun LocalAppRuntimeProfileSurface.label(context: android.content.Context): String = when (this) {
    LocalAppRuntimeProfileSurface.Dom -> context.getString(R.string.local_apps_runtime_profile_surface_dom)
    LocalAppRuntimeProfileSurface.Canvas -> context.getString(R.string.local_apps_runtime_profile_surface_canvas)
}

@Composable
private fun LocalAppVerificationStatus.label(): String = when (this) {
    LocalAppVerificationStatus.Pending -> stringResource(R.string.local_apps_verification_status_pending)
    LocalAppVerificationStatus.Passed -> stringResource(R.string.local_apps_verification_status_passed)
    LocalAppVerificationStatus.Failed -> stringResource(R.string.local_apps_verification_status_failed)
    LocalAppVerificationStatus.Unverified -> stringResource(R.string.local_apps_verification_status_unverified)
    LocalAppVerificationStatus.Unavailable -> stringResource(R.string.local_apps_verification_status_unavailable)
}

private fun LocalAppVerificationStatus.label(context: android.content.Context): String = when (this) {
    LocalAppVerificationStatus.Pending -> context.getString(R.string.local_apps_verification_status_pending)
    LocalAppVerificationStatus.Passed -> context.getString(R.string.local_apps_verification_status_passed)
    LocalAppVerificationStatus.Failed -> context.getString(R.string.local_apps_verification_status_failed)
    LocalAppVerificationStatus.Unverified -> context.getString(R.string.local_apps_verification_status_unverified)
    LocalAppVerificationStatus.Unavailable -> context.getString(R.string.local_apps_verification_status_unavailable)
}

@Composable
private fun LocalAppApprovalToolField.label(): String = when (this) {
    LocalAppApprovalToolField.Name -> stringResource(R.string.local_apps_field_name)
    LocalAppApprovalToolField.Title -> stringResource(R.string.settings_display_name)
    LocalAppApprovalToolField.Description -> stringResource(R.string.local_apps_brief)
    LocalAppApprovalToolField.InputSchema -> stringResource(R.string.local_apps_mcp_proposal_field_input_schema)
    LocalAppApprovalToolField.OutputSchema -> stringResource(R.string.local_apps_mcp_proposal_field_output_schema)
    LocalAppApprovalToolField.Annotations -> stringResource(R.string.local_apps_mcp_proposal_field_annotations)
    LocalAppApprovalToolField.Execution -> stringResource(R.string.local_apps_mcp_proposal_field_execution)
    LocalAppApprovalToolField.VisibleMeta -> stringResource(R.string.local_apps_mcp_proposal_field_visible_meta)
    LocalAppApprovalToolField.SemanticFlow -> stringResource(R.string.local_apps_mcp_proposal_field_semantic_flow)
    LocalAppApprovalToolField.PermissionCeiling -> stringResource(R.string.local_apps_mcp_proposal_field_permission_ceiling)
}

private fun LocalAppApprovalToolField.label(context: android.content.Context): String = when (this) {
    LocalAppApprovalToolField.Name -> context.getString(R.string.local_apps_field_name)
    LocalAppApprovalToolField.Title -> context.getString(R.string.settings_display_name)
    LocalAppApprovalToolField.Description -> context.getString(R.string.local_apps_brief)
    LocalAppApprovalToolField.InputSchema -> context.getString(R.string.local_apps_mcp_proposal_field_input_schema)
    LocalAppApprovalToolField.OutputSchema -> context.getString(R.string.local_apps_mcp_proposal_field_output_schema)
    LocalAppApprovalToolField.Annotations -> context.getString(R.string.local_apps_mcp_proposal_field_annotations)
    LocalAppApprovalToolField.Execution -> context.getString(R.string.local_apps_mcp_proposal_field_execution)
    LocalAppApprovalToolField.VisibleMeta -> context.getString(R.string.local_apps_mcp_proposal_field_visible_meta)
    LocalAppApprovalToolField.SemanticFlow -> context.getString(R.string.local_apps_mcp_proposal_field_semantic_flow)
    LocalAppApprovalToolField.PermissionCeiling -> context.getString(R.string.local_apps_mcp_proposal_field_permission_ceiling)
}

private fun LocalAppApprovalToolSurface.valueFor(field: LocalAppApprovalToolField): String = when (field) {
    LocalAppApprovalToolField.Name -> name
    LocalAppApprovalToolField.Title -> title.orEmpty()
    LocalAppApprovalToolField.Description -> description.orEmpty()
    LocalAppApprovalToolField.InputSchema -> inputSchemaJson
    LocalAppApprovalToolField.OutputSchema -> outputSchemaJson.orEmpty()
    LocalAppApprovalToolField.Annotations -> annotationsJson.orEmpty()
    LocalAppApprovalToolField.Execution -> executionJson.orEmpty()
    LocalAppApprovalToolField.VisibleMeta -> visibleMetaJson.orEmpty()
    LocalAppApprovalToolField.SemanticFlow -> semanticFlowJson
    LocalAppApprovalToolField.PermissionCeiling -> permissionCeiling
}

@Composable
private fun String.localizedRuntimeProfileStatus(): String = when (this) {
    "bundled" -> stringResource(R.string.local_apps_runtime_profile_status_bundled)
    "cached" -> stringResource(R.string.local_apps_runtime_profile_status_cached)
    "download_required" -> stringResource(R.string.local_apps_runtime_profile_status_download_required)
    "unavailable" -> stringResource(R.string.local_apps_runtime_profile_status_unavailable)
    "gated" -> stringResource(R.string.local_apps_runtime_profile_status_gated)
    else -> this
}

private fun String.localizedDependencyStatus(context: android.content.Context): String = when (this) {
    "may_be_required" -> context.getString(R.string.local_apps_dependency_change_download_may_be_required)
    "not_required" -> context.getString(R.string.local_apps_dependency_change_download_not_required)
    "not_needed" -> context.getString(R.string.local_apps_dependency_change_cache_not_needed)
    else -> this
}

@Composable
private fun LocalAppRuntimeProfileStatusBadge(status: LocalAppRuntimeProfileStatus) {
    val color = when (status) {
        LocalAppRuntimeProfileStatus.Verified -> MaterialTheme.colorScheme.primary
        LocalAppRuntimeProfileStatus.DependenciesDirty,
        LocalAppRuntimeProfileStatus.MigrationAvailable,
        LocalAppRuntimeProfileStatus.RebuildRequired -> MaterialTheme.colorScheme.tertiary
        LocalAppRuntimeProfileStatus.CoreDependencyDrift,
        LocalAppRuntimeProfileStatus.RuntimeBundleMissing,
        LocalAppRuntimeProfileStatus.RuntimeContractCorrupt -> MaterialTheme.colorScheme.error
    }
    Text(
        text = status.localizedLabel(),
        style = MaterialTheme.typography.labelMedium,
        color = color,
    )
}

@Composable
private fun LocalAppRuntimeProfileStatus.localizedLabel(): String = when (this) {
    LocalAppRuntimeProfileStatus.Verified -> stringResource(R.string.local_apps_runtime_profile_health_verified)
    LocalAppRuntimeProfileStatus.DependenciesDirty ->
        stringResource(R.string.local_apps_runtime_profile_health_dependencies_dirty)
    LocalAppRuntimeProfileStatus.CoreDependencyDrift ->
        stringResource(R.string.local_apps_runtime_profile_health_core_dependency_drift)
    LocalAppRuntimeProfileStatus.RebuildRequired ->
        stringResource(R.string.local_apps_runtime_profile_health_rebuild_required)
    LocalAppRuntimeProfileStatus.MigrationAvailable ->
        stringResource(R.string.local_apps_runtime_profile_health_migration_available)
    LocalAppRuntimeProfileStatus.RuntimeBundleMissing ->
        stringResource(R.string.local_apps_runtime_profile_health_runtime_bundle_missing)
    LocalAppRuntimeProfileStatus.RuntimeContractCorrupt ->
        stringResource(R.string.local_apps_runtime_profile_health_runtime_contract_corrupt)
}

@Composable
private fun LocalAppDetailsTab.label(): String = when (this) {
    LocalAppDetailsTab.Sessions -> stringResource(R.string.local_apps_section_sessions)
    LocalAppDetailsTab.Preview -> stringResource(R.string.local_apps_section_preview)
    LocalAppDetailsTab.Mcp -> stringResource(R.string.local_apps_section_mcp)
    LocalAppDetailsTab.Data -> stringResource(R.string.local_apps_section_data)
    LocalAppDetailsTab.Code -> stringResource(R.string.local_apps_section_code)
    LocalAppDetailsTab.History -> stringResource(R.string.local_apps_section_history)
    LocalAppDetailsTab.PermissionsLogs -> stringResource(R.string.local_apps_section_permissions_logs)
}
