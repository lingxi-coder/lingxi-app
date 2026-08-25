package com.lingxi.code.localapps

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
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
import androidx.compose.foundation.shape.RoundedCornerShape
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
        state.pendingProfileProposal?.let { proposal ->
            ProfileProposalDialog(proposal = proposal, onAction = onAction)
        }
    }
}

@Composable
private fun ProfileProposalDialog(
    proposal: LocalAppProfileProposal,
    onAction: (LocalAppsAction) -> Unit,
) {
    AlertDialog(
        onDismissRequest = { onAction(LocalAppsAction.ResolveProfileProposal(false)) },
        title = { Text("应用 Agent 请求更新指令") },
        text = {
            Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                Text(proposal.reason)
                Text(proposal.instructions)
                Text("版本 ${proposal.currentRevision} → ${proposal.currentRevision + 1uL}")
            }
        },
        confirmButton = {
            Button(onClick = { onAction(LocalAppsAction.ResolveProfileProposal(true)) }) {
                Text("应用")
            }
        },
        dismissButton = {
            TextButton(onClick = { onAction(LocalAppsAction.ResolveProfileProposal(false)) }) {
                Text(stringResource(R.string.local_apps_deny))
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
 * Tapping still opens the app (its pinned conversation is exactly where the
 * user needs to be) and deleting still works.
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
                AssistChip(onClick = {}, label = { Text(app.workflow.label()) })
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
    Scaffold(
        topBar = {
            LocalAppsTopBar(
                localAppDisplayName(
                    app,
                    draftTitle = stringResource(R.string.local_apps_draft_card_title),
                    fallback = stringResource(R.string.local_apps_preview_title),
                ),
                onBack = { onAction(LocalAppsAction.Back) },
            )
        },
    ) { padding ->
        Column(
            verticalArrangement = Arrangement.spacedBy(12.dp),
            modifier = Modifier.fillMaxSize().padding(padding).padding(16.dp),
        ) {
            val previewUrl = state.previewUrl(appId)
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
    LocalAppWorkflow.Draft -> stringResource(R.string.local_apps_workflow_draft)
    LocalAppWorkflow.Ready -> stringResource(R.string.settings_linux_state_ready)
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
    LocalAppDetailsTab.Sessions -> stringResource(R.string.local_apps_section_sessions)
    LocalAppDetailsTab.Preview -> stringResource(R.string.local_apps_section_preview)
    LocalAppDetailsTab.Data -> stringResource(R.string.local_apps_section_data)
    LocalAppDetailsTab.Code -> stringResource(R.string.local_apps_section_code)
    LocalAppDetailsTab.History -> stringResource(R.string.local_apps_section_history)
    LocalAppDetailsTab.PermissionsLogs -> stringResource(R.string.local_apps_section_permissions_logs)
}
