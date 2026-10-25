package com.lingxi.code.conversation

import android.content.ClipData
import android.content.ClipboardManager
import android.content.Context
import android.content.Intent
import android.net.Uri
import androidx.activity.compose.BackHandler
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.clickable
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyRow
import androidx.compose.material3.pulltorefresh.PullToRefreshBox
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.grid.GridCells
import androidx.compose.foundation.lazy.grid.LazyVerticalGrid
import androidx.compose.foundation.lazy.grid.items
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.outlined.ArrowBack
import androidx.compose.material.icons.outlined.*
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.window.Dialog
import androidx.compose.ui.window.DialogProperties
import androidx.core.content.FileProvider
import coil3.compose.AsyncImage
import com.lingxi.code.R
import com.lingxi.code.project.*
import kotlinx.coroutines.*
import java.io.File
import java.text.DateFormat
import java.util.Date

private data class FileTransfer(val sources: List<String>, val move: Boolean)
private data class FileNameEdit(val entry: WorkspaceFileEntry?)
private data class FileImport(val uris: List<Uri>, val tree: Boolean)

@OptIn(ExperimentalMaterial3Api::class)
@Composable
internal fun WorkspaceFilesDialog(
    repository: WorkspaceDirectoryRepository, initialPath: String,
    protectedPaths: () -> List<String>, canMutate: () -> Boolean, onDismiss: () -> Unit,
    selectionTitle: String? = null, onSelect: (suspend (String) -> Unit)? = null,
    onAddDirectory: (suspend (String) -> Unit)? = null,
) {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    val latestCanMutate by rememberUpdatedState(canMutate)
    val latestProtected by rememberUpdatedState(protectedPaths)
    var path by remember { mutableStateOf(initialPath) }
    var query by remember { mutableStateOf("") }
    var recursive by remember { mutableStateOf(false) }
    var hidden by remember { mutableStateOf(false) }
    var grid by remember { mutableStateOf(false) }
    var sort by remember { mutableStateOf(FileSort.Name) }
    var revision by remember { mutableIntStateOf(0) }
    var rows by remember { mutableStateOf(emptyList<WorkspaceFileEntry>()) }
    var loading by remember { mutableStateOf(true) }
    var selecting by remember { mutableStateOf(false) }
    var selected by remember { mutableStateOf(emptySet<String>()) }
    var error by remember { mutableStateOf<String?>(null) }
    var job by remember { mutableStateOf<Job?>(null) }
    var progress by remember { mutableStateOf<FileProgress?>(null) }
    var preview by remember { mutableStateOf<FileSnapshot?>(null) }
    var previewEntry by remember { mutableStateOf<WorkspaceFileEntry?>(null) }
    var info by remember { mutableStateOf<WorkspaceFileEntry?>(null) }
    var edit by remember { mutableStateOf<FileNameEdit?>(null) }
    var name by remember { mutableStateOf("") }
    var deleting by remember { mutableStateOf(emptyList<String>()) }
    var transfer by remember { mutableStateOf<FileTransfer?>(null) }
    var destination by remember { mutableStateOf<String?>(null) }
    var conflict by remember { mutableStateOf(false) }
    var importing by remember { mutableStateOf<FileImport?>(null) }
    var exporting by remember { mutableStateOf(emptyList<String>()) }
    var exportUri by remember { mutableStateOf<Uri?>(null) }
    var menu by remember { mutableStateOf(false) }
    var goTo by remember { mutableStateOf(false) }
    var enteredPath by remember { mutableStateOf("") }
    val busy = job != null
    fun navigate(next: String) { path = next; query = ""; selected = emptySet(); rows = emptyList() }
    fun message(cause: Exception) = if (cause is WorkspaceDirectoryException) context.getString(cause.messageResource) else cause.message.orEmpty()
    fun failures(values: List<FileFailure>) { if (values.isNotEmpty()) error = values.joinToString("\n") { it.path + ": " + (it.messageResource?.let(context::getString) ?: it.message) } }
    val report: suspend (FileProgress) -> Unit = { value -> withContext(Dispatchers.Main) { progress = value } }
    fun action(mutation: Boolean = false, block: suspend () -> Unit) {
        if (job != null) return
        progress = FileProgress(0, 1, context.getString(R.string.files_progress))
        job = scope.launch(start = CoroutineStart.LAZY) {
            try {
                if (mutation && !latestCanMutate()) throw WorkspaceDirectoryException(R.string.workspace_browser_busy)
                block()
            } catch (cancelled: CancellationException) { throw cancelled }
            catch (cause: Exception) { error = message(cause) }
            finally { job = null; progress = null; revision++; selected = emptySet() }
        }.also { it.start() }
    }
    fun share(targets: List<WorkspaceFileEntry>) = action {
        val snapshots = mutableListOf<FileSnapshot>()
        var handedToSystem = false
        try {
            withContext(Dispatchers.IO) {
                val root = File(context.cacheDir, "workspace-file-share")
                // Retain snapshots already handed to other apps until their lease expires.
                root.listFiles()?.filter { System.currentTimeMillis() - it.lastModified() > 24 * 60 * 60 * 1000L }?.forEach { it.deleteRecursively() }
                targets.forEach { snapshots += repository.snapshot(it.path, root) }
            }
            val uris = ArrayList(snapshots.map { FileProvider.getUriForFile(context, context.packageName + ".fileprovider", it.file) })
            val intent = Intent(if (uris.size == 1) Intent.ACTION_SEND else Intent.ACTION_SEND_MULTIPLE).setType(if (uris.size == 1) fileMime(snapshots.first().file.name) else "*/*")
            if (uris.size == 1) intent.putExtra(Intent.EXTRA_STREAM, uris.first()) else intent.putParcelableArrayListExtra(Intent.EXTRA_STREAM, uris)
            intent.clipData = ClipData.newRawUri("Files", uris.first()).apply { uris.drop(1).forEach { addItem(ClipData.Item(it)) } }
            intent.addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
            context.startActivity(Intent.createChooser(intent, context.getString(R.string.files_share)))
            handedToSystem = true
        } finally {
            if (!handedToSystem) withContext(NonCancellable + Dispatchers.IO) { snapshots.forEach { it.close() } }
        }
    }
    val importFiles = rememberLauncherForActivityResult(ActivityResultContracts.OpenMultipleDocuments()) { uris ->
        if (uris.isNotEmpty()) { importing = FileImport(uris, false); conflict = true }
    }
    val importFolder = rememberLauncherForActivityResult(ActivityResultContracts.OpenDocumentTree()) { uri ->
        if (uri != null) { importing = FileImport(listOf(uri), true); conflict = true }
    }
    val export = rememberLauncherForActivityResult(ActivityResultContracts.OpenDocumentTree()) { uri ->
        if (uri != null) { exportUri = uri; conflict = true } else exporting = emptyList()
    }
    LaunchedEffect(path, query, recursive, hidden, sort, revision) {
        loading = true
        try {
            val result = withContext(Dispatchers.IO) { sortedFiles(repository.files(path, hidden, query, recursive), sort) }
            ensureActive(); rows = result
        } catch (cancelled: CancellationException) { throw cancelled }
        catch (cause: Exception) { error = message(cause) }
        finally { loading = false }
    }
    fun releaseSnapshot(snapshot: FileSnapshot) { scope.launch(NonCancellable + Dispatchers.IO) { snapshot.close() } }
    DisposableEffect(Unit) { onDispose { job?.cancel(); preview?.let(::releaseSnapshot) } }
    val display = rows
    fun open(entry: WorkspaceFileEntry) {
        if (selecting) selected = if (entry.path in selected) selected - entry.path else selected + entry.path
        else if (entry.directory) action { val next = withContext(Dispatchers.IO) { repository.canonical(entry.path) }; navigate(next) }
        else action {
            var pending: FileSnapshot? = null
            try {
                val snapshot = withContext(Dispatchers.IO) {
                    repository.snapshot(entry.path, File(context.cacheDir, "workspace-file-preview")).also { pending = it }
                }
                currentCoroutineContext().ensureActive()
                previewEntry = entry; preview = snapshot; pending = null
            } finally {
                withContext(NonCancellable + Dispatchers.IO) { pending?.close() }
            }
        }
    }
    Dialog(onDismissRequest = onDismiss, properties = DialogProperties(usePlatformDefaultWidth = false, dismissOnBackPress = false)) {
        BackHandler {
            if (selecting) { selecting = false; selected = emptySet() }
            else if (path != initialPath && path != repository.guestHome) navigate(path.substringBeforeLast('/')) else onDismiss()
        }
        Surface(Modifier.fillMaxSize()) {
            Scaffold(topBar = {
                TopAppBar(title = { Text(path.substringAfterLast('/'), maxLines = 1, overflow = TextOverflow.Ellipsis) }, navigationIcon = {
                    IconButton(onClick = onDismiss) { Icon(Icons.AutoMirrored.Outlined.ArrowBack, stringResource(R.string.files_close)) }
                }, actions = {
                    Box {
                        IconButton(onClick = { menu = true }, modifier = Modifier.testTag("files.actions")) { Icon(Icons.Outlined.MoreVert, stringResource(R.string.workspace_browser_folder_actions)) }
                        DropdownMenu(menu, { menu = false }) {
                            DropdownMenuItem(text = { Text(stringResource(if (grid) R.string.files_list else R.string.files_grid)) }, onClick = { grid = !grid; menu = false })
                            DropdownMenuItem(text = { Text(stringResource(R.string.files_select)) }, onClick = { selecting = !selecting; selected = emptySet(); menu = false })
                            FileSort.entries.forEach { option -> DropdownMenuItem(text = { Text(stringResource(when (option) {
                                FileSort.Name -> R.string.files_name; FileSort.Modified -> R.string.files_modified; FileSort.Size -> R.string.files_size; FileSort.Type -> R.string.files_type
                            }) + if (sort == option) " ✓" else "") }, onClick = { sort = option; menu = false }) }
                            DropdownMenuItem(text = { Text(stringResource(R.string.files_hidden)) }, trailingIcon = { Checkbox(hidden, null) }, onClick = { hidden = !hidden; menu = false })
                            DropdownMenuItem(text = { Text(stringResource(R.string.files_refresh)) }, onClick = { revision++; menu = false })
                            DropdownMenuItem(text = { Text(stringResource(R.string.workspace_browser_go_to_folder)) }, onClick = { enteredPath = repository.display(path); goTo = true; menu = false })
                            if (onAddDirectory != null) DropdownMenuItem(text = { Text(stringResource(R.string.workspace_add_directory)) }, enabled = !busy && canMutate(), onClick = { menu = false; action(true) { onAddDirectory(path) } })
                            HorizontalDivider()
                            DropdownMenuItem(text = { Text(stringResource(R.string.files_new_folder)) }, enabled = !busy && canMutate(), onClick = { edit = FileNameEdit(null); name = ""; menu = false })
                            DropdownMenuItem(text = { Text(stringResource(R.string.files_import_files)) }, enabled = !busy && canMutate(), onClick = { importFiles.launch(arrayOf("*/*")); menu = false })
                            DropdownMenuItem(text = { Text(stringResource(R.string.files_import_folder)) }, enabled = !busy && canMutate(), onClick = { importFolder.launch(null); menu = false })
                        }
                    }
                })
            }, bottomBar = {
                Column(Modifier.navigationBarsPadding()) {
                    if (selecting) Row(Modifier.fillMaxWidth().padding(horizontal = 12.dp), verticalAlignment = Alignment.CenterVertically) {
                        Text("${selected.size}", Modifier.padding(end = 8.dp))
                        var selectedMenu by remember { mutableStateOf(false) }
                        Box {
                            TextButton(onClick = { selectedMenu = true }, enabled = selected.isNotEmpty() && !busy) { Text(stringResource(R.string.files_selected)) }
                            DropdownMenu(selectedMenu, { selectedMenu = false }) {
                                DropdownMenuItem(text = { Text(stringResource(R.string.files_copy)) }, enabled = canMutate(), onClick = { transfer = FileTransfer(selected.toList(), false); selectedMenu = false })
                                DropdownMenuItem(text = { Text(stringResource(R.string.files_move)) }, enabled = canMutate(), onClick = { transfer = FileTransfer(selected.toList(), true); selectedMenu = false })
                                DropdownMenuItem(text = { Text(stringResource(R.string.files_delete)) }, enabled = canMutate(), onClick = { deleting = selected.toList(); selectedMenu = false })
                                DropdownMenuItem(text = { Text(stringResource(R.string.files_share)) },
                                    enabled = rows.filter { it.path in selected }.all { !it.directory },
                                    onClick = { share(rows.filter { it.path in selected }); selectedMenu = false })
                                if (selected.size == 1) {
                                    val entry = rows.firstOrNull { it.path in selected }
                                    if (entry != null) {
                                        DropdownMenuItem(text = { Text(stringResource(R.string.files_details)) }, onClick = { info = entry; selectedMenu = false })
                                        DropdownMenuItem(text = { Text(stringResource(R.string.files_rename)) }, enabled = canMutate(), onClick = { edit = FileNameEdit(entry); name = entry.name; selectedMenu = false })
                                    }
                                }
                                DropdownMenuItem(text = { Text(stringResource(R.string.files_export)) }, onClick = { exporting = selected.toList(); export.launch(null); selectedMenu = false })
                            }
                        }
                        Spacer(Modifier.weight(1f)); TextButton(onClick = { selecting = false; selected = emptySet() }) { Text(stringResource(R.string.files_done)) }
                    }
                    if (selectionTitle != null && onSelect != null) Button(onClick = { action { onSelect(path); onDismiss() } },
                        enabled = !busy && !loading && canMutate(), modifier = Modifier.fillMaxWidth().padding(16.dp).heightIn(min = 48.dp).testTag("workspace.choose")) { Text(selectionTitle) }
                }
            }) { padding ->
                Column(Modifier.padding(padding).fillMaxSize().testTag("files.browser")) {
                    LazyRow(Modifier.fillMaxWidth().heightIn(min = 48.dp), verticalAlignment = Alignment.CenterVertically) {
                        item { TextButton(onClick = { navigate(repository.guestHome) }) { Icon(Icons.Outlined.Home, "~") } }
                        val pieces = path.removePrefix(repository.guestHome).trimStart('/').split('/').filter { it.isNotEmpty() }
                        items(pieces.size) { index ->
                            TextButton(onClick = { navigate(repository.guestHome + "/" + pieces.take(index + 1).joinToString("/")) }) {
                                Text(pieces[index], maxLines = 1)
                            }
                        }
                        item { IconButton(onClick = { navigate(path.substringBeforeLast('/')) }, enabled = path != repository.guestHome) { Icon(Icons.Outlined.ArrowUpward, stringResource(R.string.files_parent)) } }
                    }
                    OutlinedTextField(query, { query = it }, placeholder = { Text(stringResource(R.string.files_search)) },
                        singleLine = true, modifier = Modifier.fillMaxWidth().padding(horizontal = 12.dp).testTag("files.search"), leadingIcon = { Icon(Icons.Outlined.Search, null) })
                    if (query.isNotEmpty()) Row(verticalAlignment = Alignment.CenterVertically) { Checkbox(recursive, { recursive = it }); Text(stringResource(R.string.files_recursive)) }
                    progress?.let { value -> Row(Modifier.padding(12.dp), verticalAlignment = Alignment.CenterVertically) {
                        Column(Modifier.weight(1f)) { Text(value.name, maxLines = 1); LinearProgressIndicator(progress = { value.completed.toFloat() / maxOf(1, value.total) }, modifier = Modifier.fillMaxWidth()); Text("${value.completed}/${value.total}") }
                        TextButton(onClick = { job?.cancel() }) { Text(stringResource(R.string.files_cancel)) }
                    } }
                    if (loading) LinearProgressIndicator(Modifier.fillMaxWidth())
                    PullToRefreshBox(isRefreshing = loading, onRefresh = { revision++ }, modifier = Modifier.weight(1f)) {
                    if (!loading && display.isEmpty()) Box(Modifier.fillMaxSize().fillMaxWidth(), contentAlignment = Alignment.Center) { Text(stringResource(if (query.isEmpty()) R.string.files_empty else R.string.files_no_results)) }
                    else if (grid) LazyVerticalGrid(GridCells.Adaptive(140.dp), Modifier.fillMaxSize().testTag("files.grid"), contentPadding = PaddingValues(12.dp)) {
                        items(display, key = { it.path }) { entry -> FileCell(entry, repository, entry.path in selected, { open(entry) }, { selecting = true; selected = selected + entry.path }) }
                    } else LazyColumn(Modifier.fillMaxSize().testTag("files.list")) {
                        items(display, key = { it.path }) { entry ->
                            var actions by remember { mutableStateOf(false) }
                            ListItem(headlineContent = { Text(entry.name, maxLines = 2, overflow = TextOverflow.Ellipsis) },
                                supportingContent = { Text(fileSubtitle(context, entry), style = MaterialTheme.typography.bodySmall) },
                                leadingContent = { FileIcon(entry, repository, Modifier.size(36.dp)) },
                                modifier = Modifier.combinedClickable(onClick = { open(entry) }, onLongClick = { selecting = true; selected = selected + entry.path }).testTag("files.entry.${entry.name}"),
                                trailingContent = { Row(verticalAlignment = Alignment.CenterVertically) {
                                    if (entry.path in selected) Icon(Icons.Outlined.CheckCircle, null)
                                    Box {
                                        IconButton(onClick = { actions = true }, Modifier.testTag("files.entry.actions.${entry.name}")) { Icon(Icons.Outlined.MoreVert, stringResource(R.string.workspace_browser_folder_actions)) }
                                        DropdownMenu(actions, { actions = false }) {
                                            DropdownMenuItem(text = { Text(stringResource(R.string.files_details)) }, onClick = { info = entry; actions = false })
                                            DropdownMenuItem(text = { Text(stringResource(R.string.files_copy_path)) }, onClick = { (context.getSystemService(Context.CLIPBOARD_SERVICE) as ClipboardManager).setPrimaryClip(ClipData.newPlainText(entry.name, entry.path)); actions = false })
                                            if (!entry.directory) DropdownMenuItem(text = { Text(stringResource(R.string.files_share)) }, enabled = !busy, onClick = { share(listOf(entry)); actions = false })
                                            DropdownMenuItem(text = { Text(stringResource(R.string.files_export)) }, enabled = !busy, onClick = { exporting = listOf(entry.path); export.launch(null); actions = false })
                                            DropdownMenuItem(text = { Text(stringResource(R.string.files_rename)) }, enabled = !busy && canMutate(), onClick = { edit = FileNameEdit(entry); name = entry.name; actions = false })
                                            DropdownMenuItem(text = { Text(stringResource(R.string.files_copy)) }, enabled = !busy && canMutate(), onClick = { transfer = FileTransfer(listOf(entry.path), false); actions = false })
                                            DropdownMenuItem(text = { Text(stringResource(R.string.files_move)) }, enabled = !busy && canMutate(), onClick = { transfer = FileTransfer(listOf(entry.path), true); actions = false })
                                            DropdownMenuItem(text = { Text(stringResource(R.string.files_delete)) }, enabled = !busy && canMutate(), onClick = { deleting = listOf(entry.path); actions = false })
                                        }
                                    }
                                } })
                        }
                    }
                    }

                }
            }
        }
        edit?.let { target -> AlertDialog(onDismissRequest = { edit = null }, title = { Text(stringResource(if (target.entry == null) R.string.files_new_folder else R.string.files_rename)) },
            text = { OutlinedTextField(name, { name = it }, singleLine = true, modifier = Modifier.testTag("files.name")) },
            confirmButton = { TextButton(enabled = name.isNotBlank() && !busy, onClick = {
                val value = name; val protected = latestProtected() + path
                action(true) { withContext(Dispatchers.IO) { if (target.entry == null) repository.create(path, value) else repository.rename(target.entry.path, value, protected) }; edit = null }
            }) { Text(stringResource(R.string.files_save)) } }, dismissButton = { TextButton(onClick = { edit = null }) { Text(stringResource(R.string.files_cancel)) } }) }
        if (deleting.isNotEmpty()) AlertDialog(onDismissRequest = { deleting = emptyList() }, title = { Text(stringResource(R.string.files_delete)) },
            text = { Text(context.getString(R.string.files_delete_warning) + "\n${deleting.size}\n" + deleting.joinToString("\n")) },
            confirmButton = { TextButton(enabled = !busy && canMutate(), onClick = {
                val sources = deleting; deleting = emptyList(); val protected = latestProtected() + path
                action(true) { failures(withContext(Dispatchers.IO) { repository.deleteFiles(sources, protected, report) }) }
            }) { Text(stringResource(R.string.files_delete), color = MaterialTheme.colorScheme.error) } },
            dismissButton = { TextButton(onClick = { deleting = emptyList() }) { Text(stringResource(R.string.files_cancel)) } })
        if (goTo) AlertDialog(onDismissRequest = { goTo = false }, title = { Text(stringResource(R.string.workspace_browser_go_to_folder)) },
            text = { OutlinedTextField(enteredPath, { enteredPath = it }, singleLine = true) }, confirmButton = { TextButton(onClick = {
                action { val next = withContext(Dispatchers.IO) { repository.canonical(enteredPath) }; navigate(next); goTo = false }
            }) { Text(stringResource(R.string.workspace_browser_open)) } }, dismissButton = { TextButton(onClick = { goTo = false }) { Text(stringResource(R.string.files_cancel)) } })
        if (conflict) AlertDialog(onDismissRequest = { conflict = false; destination = null; importing = null; exportUri = null; exporting = emptyList() }, title = { Text(stringResource(R.string.files_conflict)) },
            text = { Text(stringResource(R.string.files_choose_destination) + "\n" + (destination ?: path)) },
            confirmButton = { Column {
                for (policy in listOf(FileConflict.KeepBoth, FileConflict.Skip)) TextButton(onClick = {
                    val request = transfer; val target = destination; val incoming = importing; val outgoing = exporting; val uri = exportUri
                    conflict = false; transfer = null; destination = null; importing = null; exportUri = null; exporting = emptyList()
                    val protected = latestProtected() + path
                    action(mutation = uri == null) {
                        failures(withContext(Dispatchers.IO) {
                            when {
                                request != null && target != null -> repository.transferFiles(request.sources, target, request.move, policy, protected, report)
                                incoming != null -> repository.importDocuments(context, incoming.uris, path, incoming.tree, policy, report)
                                uri != null -> repository.exportDocuments(context, outgoing, uri, policy, report)
                                else -> emptyList()
                            }
                        })
                    }
                }) { Text(stringResource(if (policy == FileConflict.KeepBoth) R.string.files_keep_both else R.string.files_skip)) }
            } }, dismissButton = { TextButton(onClick = { conflict = false; transfer = null; importing = null; destination = null; exportUri = null; exporting = emptyList() }) { Text(stringResource(R.string.files_cancel)) } })
        info?.let { entry -> AlertDialog(onDismissRequest = { info = null }, title = { Text(entry.name) },
            text = { androidx.compose.foundation.text.selection.SelectionContainer { Text(entry.path + "\n" + fileSubtitle(context, entry) + "\n" + if (entry.symbolicLink) context.getString(R.string.files_link) else fileMime(entry.name)) } },
            confirmButton = { TextButton(onClick = { info = null }) { Text(stringResource(R.string.files_close)) } }) }
        error?.let { value -> AlertDialog(onDismissRequest = { error = null }, title = { Text(stringResource(R.string.files_failed)) }, text = { Text(value) }, confirmButton = { TextButton(onClick = { error = null }) { Text(stringResource(android.R.string.ok)) } }) }
        preview?.let { snapshot -> WorkspaceFilePreviewDialog(snapshot, previewEntry!!, onDismiss = { releaseSnapshot(snapshot); preview = null; previewEntry = null }) }
        transfer?.takeIf { destination == null }?.let { request ->
            WorkspaceFilesDialog(repository, path, protectedPaths, canMutate, onDismiss = { if (destination == null) transfer = null },
                selectionTitle = "${request.sources.size} · " + stringResource(R.string.files_paste), onSelect = { target -> destination = target; conflict = true })
        }
    }
}

private fun fileSubtitle(context: Context, entry: WorkspaceFileEntry): String =
    (if (entry.directory) "" else android.text.format.Formatter.formatFileSize(context, entry.bytes) + " · ") + DateFormat.getDateInstance(DateFormat.SHORT).format(Date(entry.modified))

@Composable private fun FileIcon(entry: WorkspaceFileEntry, repository: WorkspaceDirectoryRepository, modifier: Modifier) {
    if (entry.kind == "image" && !entry.symbolicLink) {
        var file by remember(entry.path, entry.modified) { mutableStateOf<File?>(null) }
        LaunchedEffect(entry.path, entry.modified) { file = withContext(Dispatchers.IO) { runCatching { repository.resolve(entry.path, false) }.getOrNull() } }
        AsyncImage(file, null, modifier = modifier)
    } else Icon(when { entry.symbolicLink -> Icons.Outlined.Link; entry.directory -> Icons.Outlined.Folder; entry.kind == "pdf" -> Icons.Outlined.PictureAsPdf; entry.kind == "media" -> Icons.Outlined.PlayCircle; entry.kind == "text" -> Icons.Outlined.Code; else -> Icons.Outlined.InsertDriveFile }, null, modifier,
        tint = if (entry.directory) MaterialTheme.colorScheme.primary else MaterialTheme.colorScheme.onSurfaceVariant)
}
@Composable private fun FileCell(entry: WorkspaceFileEntry, repository: WorkspaceDirectoryRepository, selected: Boolean, onClick: () -> Unit, onLongClick: () -> Unit) {
    Card(Modifier.padding(6.dp).combinedClickable(onClick = onClick, onLongClick = onLongClick).testTag("files.entry.${entry.name}")) {
        Column(Modifier.padding(12.dp).fillMaxWidth(), horizontalAlignment = Alignment.CenterHorizontally) {
            FileIcon(entry, repository, Modifier.size(54.dp)); Text(entry.name, maxLines = 2, overflow = TextOverflow.Ellipsis)
            Text(fileSubtitle(LocalContext.current, entry), style = MaterialTheme.typography.bodySmall, maxLines = 1, overflow = TextOverflow.Ellipsis)
            if (selected) Icon(Icons.Outlined.CheckCircle, null)
        }
    }
}
