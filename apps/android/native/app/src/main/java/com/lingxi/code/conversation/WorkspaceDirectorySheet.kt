package com.lingxi.code.conversation

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.*
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.outlined.MoreVert
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import com.lingxi.code.R
import com.lingxi.code.components.LXIcon
import com.lingxi.code.components.LXIconName
import com.lingxi.code.model.ConversationScope
import com.lingxi.code.model.Project
import com.lingxi.code.project.*
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import java.io.File


/** Workspace selection and native browsing of the same persistent home mounted by Linux. */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
internal fun WorkspaceDirectorySheet(
    projects: List<Project>, currentScope: ConversationScope,
    currentDirectory: String?, directories: List<String>, runtimeStatus: String,
    protectedPaths: () -> List<String>, canMutate: () -> Boolean,
    onDismiss: () -> Unit, onPick: (ConversationScope) -> Unit,
    onPickDirectory: suspend (String) -> Unit,
    repositoryOverride: WorkspaceDirectoryRepository? = null,
    onAddDirectory: (suspend (String) -> Unit)? = null,
) {
    val context = LocalContext.current
    val repository = remember(repositoryOverride) { repositoryOverride ?: MobileUserStorage.directories(context).let { paths ->
        WorkspaceDirectoryRepository(File(paths.home), paths.guestHome,
            paths.defaultGuestCwd, paths.projectsGuestPath, File(paths.configHome))
    } }
    val scope = rememberCoroutineScope()
    var folder by remember { mutableStateOf<String?>(null) }
    var adding by remember { mutableStateOf(false) }
    var revision by remember { mutableIntStateOf(0) }
    var locations by remember { mutableStateOf(emptyList<WorkspaceLocation>()) }
    var busy by remember { mutableStateOf(false) }
    var error by remember { mutableStateOf<String?>(null) }
    var edit by remember { mutableStateOf<WorkspaceLocation?>(null) }
    var pendingDelete by remember { mutableStateOf<String?>(null) }
    val currentCanMutate by rememberUpdatedState(canMutate)
    val currentProtected by rememberUpdatedState(protectedPaths)
    val currentPick by rememberUpdatedState(onPickDirectory)
    fun errorText(cause: Throwable): String = if (cause is WorkspaceDirectoryException)
        context.getString(cause.messageResource) else cause.message ?: context.getString(R.string.workspace_browser_operation_failed)
    fun runAction(mutation: Boolean = false, action: suspend () -> Unit) {
        if (busy) return
        busy = true
        scope.launch {
            try {
                if (mutation && !currentCanMutate()) throw WorkspaceDirectoryException(R.string.workspace_browser_busy)
                action()
                revision++
            } catch (cause: kotlinx.coroutines.CancellationException) { throw cause }
            catch (cause: Exception) { error = errorText(cause) }
            finally { busy = false }
        }
    }
    LaunchedEffect(folder, revision) {
        try {
            locations = withContext(Dispatchers.IO) { repository.locations() }
        } catch (cause: kotlinx.coroutines.CancellationException) { throw cause }
        catch (cause: Exception) { error = errorText(cause) }
    }
    fun select(path: String) = runAction {
        if (!currentCanMutate()) throw WorkspaceDirectoryException(R.string.workspace_browser_busy)
        val canonical = withContext(Dispatchers.IO) { repository.canonical(path) }
        if (adding) {
            withContext(Dispatchers.IO) { repository.save(canonical, canonical.substringAfterLast('/')) }
            folder = null; adding = false
        } else {
            currentPick(canonical)
            onDismiss()
        }
    }
    if (folder != null) {
        WorkspaceFilesDialog(repository, folder!!, protectedPaths, canMutate,
            onDismiss = { folder = null; adding = false },
            selectionTitle = context.getString(if (adding) R.string.workspace_browser_add_location else R.string.workspace_browser_use_folder),
            onSelect = { chosen ->
                if (!currentCanMutate()) throw WorkspaceDirectoryException(R.string.workspace_browser_busy)
                if (adding) withContext(Dispatchers.IO) { repository.save(chosen, chosen.substringAfterLast('/')) }
                else { currentPick(chosen); onDismiss() }
                revision++
            }, onAddDirectory = onAddDirectory)
        return
    }
    ModalBottomSheet(onDismissRequest = onDismiss,
        sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true)) {
        Column(Modifier.fillMaxWidth().fillMaxHeight(0.94f).navigationBarsPadding().testTag("workspace.picker")) {
            Row(Modifier.fillMaxWidth().padding(horizontal = 12.dp), verticalAlignment = androidx.compose.ui.Alignment.CenterVertically) {
                TextButton(onClick = onDismiss) {
                    Text(stringResource(R.string.common_close))
                }
                Text(stringResource(R.string.workspace_browser_title),
                    Modifier.weight(1f), style = MaterialTheme.typography.titleLarge, maxLines = 1, overflow = TextOverflow.Ellipsis)

            }
            if (busy) LinearProgressIndicator(Modifier.fillMaxWidth())
            LazyColumn(Modifier.weight(1f).testTag("workspace.contents"), contentPadding = PaddingValues(bottom = 16.dp)) {
                    item {
                        ListItem(headlineContent = { Text(repository.guestHome.substringAfterLast('/')) },
                            supportingContent = { Text(repository.guestHome + "\nLinux · " + runtimeStatus) }, leadingContent = { LXIcon(LXIconName.Folder) })
                        WorkspaceHeading(R.string.workspace_browser_current_folder)
                        WorkspaceFolderRow(repository.display(currentDirectory ?: repository.workspace),
                            selected = true, onClick = { folder = currentDirectory ?: repository.workspace })
                        WorkspaceHeading(R.string.workspace_browser_locations)
                        WorkspaceFolderRow(stringResource(R.string.workspace_browser_workspace), repository.display(repository.workspace),
                            onClick = { select(repository.workspace) })
                        WorkspaceFolderRow(stringResource(R.string.workspace_browser_home), "~", onClick = { folder = repository.guestHome })
                        WorkspaceFolderRow(stringResource(R.string.workspace_browser_browse_folders), onClick = {
                            adding = false; folder = currentDirectory ?: repository.workspace
                        }, tag = "workspace.browse")
                        WorkspaceHeading(R.string.workspace_browser_saved_locations)
                    }
                    items(locations, key = { it.id }) { location ->
                        var menu by remember { mutableStateOf(false) }
                        ListItem(headlineContent = { Text(location.name) }, supportingContent = { Text(repository.display(location.path)) },
                            modifier = Modifier.clickable(enabled = !busy) { select(location.path) }.testTag("workspace.location.${location.id}"),
                            leadingContent = { LXIcon(LXIconName.Folder) }, trailingContent = {
                                Box {
                                    IconButton(onClick = { menu = true }, modifier = Modifier.testTag("workspace.location.actions.${location.id}")) { Icon(Icons.Outlined.MoreVert, stringResource(R.string.workspace_browser_folder_actions)) }
                                    DropdownMenu(menu, { menu = false }) {
                                        DropdownMenuItem(text = { Text(stringResource(R.string.workspace_browser_edit_location)) }, onClick = {
                                            menu = false; edit = location
                                        })
                                        DropdownMenuItem(text = { Text(stringResource(R.string.workspace_browser_remove_location)) }, enabled = !busy,
                                            onClick = { menu = false; runAction { withContext(Dispatchers.IO) { repository.unlink(location.id) } } })
                                        DropdownMenuItem(text = { Text(stringResource(R.string.workspace_browser_delete_folder)) }, enabled = !busy && canMutate(),
                                            onClick = { menu = false; pendingDelete = location.path })
                                    }
                                }
                            })
                    }
                    item {
                        WorkspaceFolderRow(stringResource(R.string.workspace_browser_add_location), tag = "workspace.add-location", onClick = {
                            adding = true; folder = currentDirectory ?: repository.workspace
                        })
                        if (directories.isNotEmpty()) WorkspaceHeading(R.string.workspace_browser_session_folders)
                    }
                    items(directories.distinct()) { path -> WorkspaceFolderRow(repository.display(path), onClick = { select(path) }) }
                    item {
                        WorkspaceHeading(R.string.workspace_browser_projects)
                        WorkspaceFolderRow(stringResource(R.string.common_global), selected = currentScope == ConversationScope.Global,
                            onClick = { onPick(ConversationScope.Global) })
                    }
                    items(projects, key = { it.id }) { project ->
                        WorkspaceFolderRow(project.name, selected = currentScope == ConversationScope.Project(project.id),
                            onClick = { onPick(ConversationScope.Project(project.id)) })
                    }

            }

        }
    }
    edit?.let { target ->
        var name by remember(target) { mutableStateOf(target.name) }
        var path by remember(target) { mutableStateOf(target.path) }
        AlertDialog(onDismissRequest = { if (!busy) edit = null },
            title = { Text(stringResource(R.string.workspace_browser_edit_location)) },
            text = { Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                OutlinedTextField(name, { name = it }, label = { Text(stringResource(R.string.workspace_browser_location_name)) },
                    modifier = Modifier.testTag("workspace.edit.name"), singleLine = true)
                OutlinedTextField(path, { path = it }, label = { Text(stringResource(R.string.workspace_browser_path)) },
                    modifier = Modifier.testTag("workspace.edit.path"), singleLine = true)
                Text(stringResource(R.string.workspace_browser_edit_location_hint))
            } },
            confirmButton = { TextButton(enabled = !busy && name.isNotBlank() && path.isNotBlank(), onClick = {
                runAction(mutation = true) {
                    withContext(Dispatchers.IO) { repository.save(path, name, target.id) }
                    edit = null
                }
            }, modifier = Modifier.testTag("workspace.edit.save")) { Text(stringResource(R.string.common_save)) } },
            dismissButton = { TextButton(onClick = { edit = null }) { Text(stringResource(R.string.common_cancel)) } })
    }
    pendingDelete?.let { path -> AlertDialog(onDismissRequest = { pendingDelete = null },
        title = { Text(stringResource(R.string.workspace_browser_delete_folder)) },
        text = { Text(stringResource(R.string.workspace_browser_delete_warning) + "\n\n" + path) },
        confirmButton = { TextButton(enabled = !busy && canMutate(), onClick = {
            runAction(mutation = true) {
                val protected = currentProtected() + listOfNotNull(currentDirectory)
                withContext(Dispatchers.IO) { repository.delete(path, protected) }
                pendingDelete = null
            }
        }) { Text(stringResource(R.string.workspace_browser_delete_folder), color = MaterialTheme.colorScheme.error) } },
        dismissButton = { TextButton(onClick = { pendingDelete = null }) { Text(stringResource(R.string.common_cancel)) } }) }
    error?.let { message -> AlertDialog(onDismissRequest = { error = null },
        title = { Text(stringResource(R.string.workspace_browser_operation_failed)) }, text = { Text(message) },
        confirmButton = { TextButton(onClick = { error = null }) { Text(stringResource(android.R.string.ok)) } }) }
}

@Composable private fun WorkspaceHeading(resource: Int) {
    Text(stringResource(resource), Modifier.padding(horizontal = 16.dp, vertical = 12.dp),
        style = MaterialTheme.typography.titleSmall, color = MaterialTheme.colorScheme.primary)
}

@Composable private fun WorkspaceFolderRow(label: String, detail: String? = null, selected: Boolean = false,
    tag: String = "workspace.row", onClick: () -> Unit) {
    ListItem(headlineContent = { Text(label) }, supportingContent = detail?.let { { Text(it) } },
        leadingContent = { LXIcon(LXIconName.Folder) }, trailingContent = { if (selected) Text("✓") },
        modifier = Modifier.clickable(onClick = onClick).testTag(tag))
}
