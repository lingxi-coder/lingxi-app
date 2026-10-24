package com.lingxi.code.apps

import androidx.compose.foundation.background
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.stringResource
import com.lingxi.code.R
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.dp
import com.lingxi.code.components.PageTopBar
import com.lingxi.code.components.UiTags
import com.lingxi.code.components.blockTouchesBehind
import com.lingxi.code.theme.LingXiTheme
import kotlinx.coroutines.launch
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext

@Composable
fun UnifiedAppsPage(apps: UnifiedLocalApps, onBack: () -> Unit, onOpen: (String) -> Unit, onDetails: (String) -> Unit,
    onAuthoring: (String?, String, String) -> Unit) {
    val state by apps.state.collectAsState()
    val scope = rememberCoroutineScope()
    var tab by rememberSaveable { mutableIntStateOf(0) }
    var query by rememberSaveable { mutableStateOf("") }
    var name by rememberSaveable { mutableStateOf("") }
    var sourceDirectory by rememberSaveable { mutableStateOf("") }
    var creating by rememberSaveable { mutableStateOf(false) }
    var busy by remember { mutableStateOf(false) }
    var error by remember { mutableStateOf<String?>(null) }
    var catalog by remember { mutableStateOf<List<CatalogApp>>(emptyList()) }
    var archives by remember { mutableStateOf<List<LegacyAppEntry>>(emptyList()) }
    var showingArchives by rememberSaveable { mutableStateOf(false) }
    fun action(block: suspend () -> Unit) { scope.launch { busy = true; error = null; try { block() } catch (e: Exception) { error = e.message } finally { busy = false } } }
    val importSource = rememberLauncherForActivityResult(ActivityResultContracts.OpenDocument()) { uri ->
        if (uri != null) action { val imported = apps.importArchive(uri); onDetails(imported.id) }
    }
    LaunchedEffect(Unit) { apps.refresh() }
    LaunchedEffect(tab, query) {
        if (tab == 1) {
            busy = true
            try { catalog = apps.market.list(query); error = null } catch (e: Exception) { error = e.message } finally { busy = false }
        }
    }
    LaunchedEffect(showingArchives) { if (showingArchives) try { archives = apps.legacyEntries() } catch (e: Exception) { error = e.message } }
    Column(Modifier.fillMaxSize().background(LingXiTheme.palette.windowBg).blockTouchesBehind().windowInsetsPadding(WindowInsets.systemBars).testTag(UiTags.APPS_PAGE)) {
        PageTopBar(title = stringResource(R.string.local_app_v2_title), onBack = onBack)
        PrimaryTabRow(selectedTabIndex = tab) {
            listOf(stringResource(R.string.local_app_v2_mine), stringResource(R.string.local_app_v2_market)).forEachIndexed { index, title -> Tab(selected = index == tab, onClick = { tab = index }, text = { Text(title) }) }
        }
        Row(Modifier.fillMaxWidth().padding(horizontal = 16.dp), horizontalArrangement = Arrangement.SpaceBetween) {
            TextButton(onClick = { creating = true }) { Text(stringResource(R.string.local_app_v2_create)) }
            if (tab == 0) TextButton(enabled = !busy, onClick = { importSource.launch(arrayOf("application/zip", "application/x-zip-compressed")) }) { Text(stringResource(R.string.local_app_v2_import_source)) }
            TextButton(onClick = { action { apps.refresh() } }) { Text(stringResource(R.string.local_app_v2_refresh)) }
        }
        if (busy || state.loading) LinearProgressIndicator(Modifier.fillMaxWidth())
        (error ?: state.error)?.let { Text(it, color = MaterialTheme.colorScheme.error, modifier = Modifier.padding(16.dp)) }
        OutlinedTextField(query, { query = it }, label = { Text(stringResource(R.string.local_app_v2_search)) }, singleLine = true, modifier = Modifier.fillMaxWidth().padding(horizontal = 16.dp))
        LazyColumn(Modifier.weight(1f), contentPadding = PaddingValues(16.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
            when (tab) {
                0 -> {
                    item { TextButton(onClick = { showingArchives = true }) { Text(stringResource(R.string.local_app_v2_legacy)) } }
                    if (state.apps.isEmpty() && !state.loading) item { Text(stringResource(R.string.local_app_v2_empty)) }
                    items(state.apps.filter { query.isBlank() || it.name.contains(query, true) }, key = { it.id }) { app ->
                        ElevatedCard(Modifier.fillMaxWidth().testTag(UiTags.appTile(app.id))) {
                            Column(Modifier.padding(16.dp)) {
                                Text(app.name, style = MaterialTheme.typography.titleMedium)
                                Text(app.summary, style = MaterialTheme.typography.bodyMedium)
                                Text(stringResource(R.string.local_app_v2_revision, app.currentRevision) + if (app.latestRevision > app.currentRevision) stringResource(R.string.local_app_v2_update_pending, app.latestRevision) else "", style = MaterialTheme.typography.labelMedium)
                                Row {
                                    TextButton(onClick = { onOpen(app.id) }) { Text(stringResource(R.string.local_app_v2_open)) }
                                    TextButton(onClick = { onDetails(app.id) }) { Text(stringResource(R.string.local_app_v2_manage)) }
                                    TextButton(onClick = { onAuthoring(app.id, app.name, "") }) { Text(stringResource(R.string.local_app_v2_develop)) }
                                }
                            }
                        }
                    }
                }
                1 -> items(catalog, key = { it.id }) { row ->
                    ElevatedCard(Modifier.fillMaxWidth()) { Column(Modifier.padding(16.dp)) {
                        Text(row.name, style = MaterialTheme.typography.titleMedium); Text(row.summary)
                        TextButton(enabled = !busy, onClick = { action { val installed = apps.market.install(row.id); onDetails(installed.id) } }) { Text(stringResource(R.string.local_app_v2_install)) }
                    } }
                }
            }
        }
    }
    if (showingArchives) AlertDialog(onDismissRequest = { showingArchives = false }, title = { Text(stringResource(R.string.local_app_v2_legacy)) }, text = {
        LazyColumn {
            item { Text(stringResource(R.string.local_app_v2_legacy_note)) }
            items(archives, key = { it.key }) { row ->
                ListItem(headlineContent = { Text(row.name) }, supportingContent = { Text(row.kind) }, trailingContent = {
                    TextButton(enabled = !busy, onClick = { action { val installed = apps.importLegacy(row); showingArchives = false; onDetails(installed.id) } }) { Text(stringResource(R.string.local_app_v2_import)) }
                })
            }
        }
    }, confirmButton = { TextButton(onClick = { showingArchives = false }) { Text(stringResource(R.string.local_app_v2_cancel)) } })
    if (creating) AlertDialog(onDismissRequest = { if (!busy) creating = false }, title = { Text(stringResource(R.string.local_app_v2_create)) }, text = {
        Column { Text(stringResource(R.string.local_app_v2_create_note)); OutlinedTextField(name, { name = it }, label = { Text(stringResource(R.string.local_app_v2_name)) }, singleLine = true)
            OutlinedTextField(sourceDirectory, { sourceDirectory = it }, label = { Text(stringResource(R.string.local_app_v2_source_directory)) }, placeholder = { Text("/home/lingxi/…") }, singleLine = true)
            Text(stringResource(R.string.local_app_v2_source_directory_note), style = MaterialTheme.typography.bodySmall)
        }
    }, confirmButton = { TextButton(enabled = name.isNotBlank() && !busy, onClick = { action {
        val directory = sourceDirectory.trim().takeIf { it.isNotEmpty() }?.also { guest ->
            require(withContext(Dispatchers.IO) { apps.hostFile(guest).isDirectory }) { "请选择已存在的 Linux 用户目录。" }
        } ?: apps.createAuthoring(name.trim())
        creating = false; onAuthoring(null, name.trim(), directory)
    } }) { Text(stringResource(R.string.local_app_v2_create_confirm)) } }, dismissButton = { TextButton(onClick = { creating = false }) { Text(stringResource(R.string.local_app_v2_cancel)) } })
}

@Composable
fun UnifiedAppDetailsPage(apps: UnifiedLocalApps, appId: String, onBack: () -> Unit, onOpen: (String) -> Unit,
    onAuthoring: (String?, String, String) -> Unit) {
    val scope = rememberCoroutineScope()
    var record by remember(appId) { mutableStateOf<AppRecord?>(null) }
    var releases by remember(appId) { mutableStateOf<List<AppRelease>>(emptyList()) }
    var runs by remember(appId) { mutableStateOf<List<AppRun>>(emptyList()) }
    var logs by remember(appId) { mutableStateOf("") }
    var busy by remember { mutableStateOf(false) }
    var error by remember { mutableStateOf<String?>(null) }
    var deleting by remember { mutableStateOf(false) }
    var pendingRelease by remember { mutableStateOf<AppRelease?>(null) }
    var publication by remember { mutableStateOf<AppMarketPublication?>(null) }
    var publicationStatus by remember { mutableStateOf<String?>(null) }
    var agentRelease by remember(appId) { mutableStateOf<AppRelease?>(null) }
    var serving by remember(appId) { mutableStateOf<AppRun?>(null) }
    var dataDirectory by remember(appId) { mutableStateOf("") }
    var sourceExport by remember(appId) { mutableStateOf<AppSourceExport?>(null) }
    val context = LocalContext.current
    var permissionRevision by remember { mutableIntStateOf(0) }
    suspend fun reload() { record = apps.get(appId); releases = apps.releases(appId); runs = apps.runs(appId); serving = (apps.request("status", appId) as org.json.JSONObject).optJSONObject("run")?.appRun()
        dataDirectory = withContext(Dispatchers.IO) { apps.guestPath((apps.request("layout", appId) as org.json.JSONObject).getString("dataPath")) }
    }
    fun action(block: suspend () -> Unit) { scope.launch { busy = true; error = null; try { block(); reload() } catch (e: Exception) { error = e.message } finally { busy = false } } }
    val exportSource = rememberLauncherForActivityResult(ActivityResultContracts.CreateDocument("application/zip")) { uri ->
        val prepared = sourceExport; sourceExport = null
        if (prepared == null) { if (uri != null) error = context.getString(R.string.local_app_v2_export_interrupted) }
        else action { try {
            if (uri != null) withContext(Dispatchers.IO) {
                context.contentResolver.openOutputStream(uri)?.use { output -> prepared.archive.inputStream().use { input -> input.copyTo(output) } }
                    ?: error("Unable to write source archive")
            }
        } finally { withContext(Dispatchers.IO) { prepared.close() } } }
    }
    LaunchedEffect(appId) { try { reload() } catch (e: Exception) { error = e.message } }
    Column(Modifier.fillMaxSize().background(LingXiTheme.palette.windowBg).blockTouchesBehind().windowInsetsPadding(WindowInsets.systemBars)) {
        PageTopBar(title = record?.name ?: stringResource(R.string.local_app_v2_title), onBack = onBack)
        if (busy) LinearProgressIndicator(Modifier.fillMaxWidth())
        LazyColumn(contentPadding = PaddingValues(16.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
            error?.let { item { Text(it, color = MaterialTheme.colorScheme.error) } }
            val app = record
            if (app != null) {
                item { Text(app.summary); Text(serving?.phase ?: runs.lastOrNull()?.phase ?: "stopped", style = MaterialTheme.typography.labelMedium) }
                item { Row { TextButton(onClick = { onOpen(appId) }) { Text(stringResource(R.string.local_app_v2_open)) }; TextButton(onClick = { onAuthoring(appId, app.name, "") }) { Text(stringResource(R.string.local_app_v2_develop)) }; TextButton(enabled = !busy, onClick = { action { apps.stop(appId) } }) { Text(stringResource(R.string.local_app_v2_stop)) } } }
                item { Row { TextButton(enabled = !busy, onClick = { action { apps.start(appId, "restart") } }) { Text(stringResource(R.string.local_app_v2_restart)) }; TextButton(enabled = !busy, onClick = { action { apps.repair(appId) } }) { Text(stringResource(R.string.local_app_v2_repair)) }; TextButton(enabled = !busy, onClick = { action { apps.revoke(appId); permissionRevision++ } }) { Text(stringResource(R.string.local_app_v2_revoke)) } } }
                item { Text(stringResource(R.string.local_app_v2_repair_note), style = MaterialTheme.typography.bodySmall) }
                item {
                    Text(stringResource(R.string.local_app_v2_data), style = MaterialTheme.typography.titleMedium)
                    Text(dataDirectory, style = MaterialTheme.typography.bodySmall)
                    Text(stringResource(R.string.local_app_v2_data_note), style = MaterialTheme.typography.bodySmall)
                    TextButton(enabled = !busy && sourceExport == null, onClick = { action {
                        val prepared = apps.sourceArchive(appId, app.currentRevision); sourceExport = prepared
                        try { exportSource.launch("${app.name.replace(Regex("[^\\p{L}\\p{N} _-]"), "_").take(60)}-v${app.currentRevision}.zip") }
                        catch (failure: Exception) { sourceExport = null; withContext(Dispatchers.IO) { prepared.close() }; throw failure }
                    } }) { Text(stringResource(R.string.local_app_v2_export_source)) }
                }
                item {
                    TextButton(enabled = !busy, onClick = { action { publication = apps.market.publisher.prepare(appId) } }) { Text(stringResource(R.string.local_app_v2_publish_market)) }
                    publicationStatus?.let { Text(it, style = MaterialTheme.typography.bodySmall) }
                    TextButton(onClick = {
                        val root = java.net.URI(com.lingxi.code.BuildConfig.LOCAL_APP_MARKET_API_ROOT)
                        val address = java.net.URI(root.scheme, root.authority, "/mine", null, null).toString()
                        context.startActivity(android.content.Intent(android.content.Intent.ACTION_VIEW, android.net.Uri.parse(address)))
                    }) { Text(stringResource(R.string.local_app_v2_manage_publications)) }
                }
                app.marketOrigin?.takeIf { it.remixedFrom == null }?.let { market -> item { TextButton(enabled = !busy, onClick = { action {
                    val updated = apps.market.install(market.appId, app)
                    if (updated.latestRevision > updated.currentRevision) {
                        val candidate = apps.release(appId, updated.latestRevision)
                        if (candidate.executesCode && candidate.digest !in updated.executionGrants) pendingRelease = candidate
                    }
                } }) { Text(stringResource(R.string.local_app_v2_update)) } } }
                item { Text(stringResource(R.string.local_app_v2_versions), style = MaterialTheme.typography.titleMedium) }
                items(releases, key = { it.revision }) { release ->
                    ListItem(headlineContent = { Text(stringResource(R.string.local_app_v2_revision, release.revision)) }, supportingContent = { Text(release.digest.take(16)) }, trailingContent = {
                        if (release.revision != app.currentRevision) TextButton(enabled = !busy, onClick = { if (release.executesCode && release.digest !in app.executionGrants) pendingRelease = release else action { apps.activate(appId, release.revision) } }) { Text(stringResource(R.string.local_app_v2_activate)) }
                    })
                }
                val release = releases.firstOrNull { it.revision == app.currentRevision }
                if (release != null) {
                    item { Text("浏览器使用期间通过运行通知保留本地服务。停止应用会停止服务与 Agent 任务。", style = MaterialTheme.typography.bodySmall) }
                    item { TextButton(onClick = { action {
                        val restored = apps.restoreSource(appId, app.marketOrigin?.remixedFrom == null && app.marketOrigin != null)
                        onAuthoring(restored.id, restored.name, restored.authoring?.sourceDirectory.orEmpty())
                    } }) { Text(stringResource(if (app.marketOrigin != null && app.marketOrigin.remixedFrom == null) R.string.local_app_v2_remix else R.string.local_app_v2_restore)) } }
                    if ("agent" in release.capabilities) item { TextButton(onClick = { agentRelease = release }) { Text("Agent 模型与工具授权") } }
                    item { Text(stringResource(R.string.local_app_v2_permissions), style = MaterialTheme.typography.titleMedium) }
                    item { Text("相机、麦克风、照片和文件由外部浏览器处理。请在浏览器的网站权限中管理授权和清除站点数据；灵犀不会代替浏览器授予或撤销这些权限。", style = MaterialTheme.typography.bodySmall) }
                }
                item { Row { Text(stringResource(R.string.local_app_v2_logs), style = MaterialTheme.typography.titleMedium); TextButton(onClick = { action { val run = apps.runs(appId).lastOrNull(); logs = run?.let { apps.logs(appId, it.runId) }.orEmpty() } }) { Text(stringResource(R.string.local_app_v2_read_logs)) } } }
                item { Text(logs.ifEmpty { stringResource(R.string.local_app_v2_logs_empty) }, style = MaterialTheme.typography.bodySmall) }
                item { TextButton(enabled = !busy, onClick = { deleting = true }) { Text(stringResource(R.string.local_app_v2_remove), color = MaterialTheme.colorScheme.error) } }
            }
        }
    }
    pendingRelease?.let { target -> AlertDialog(onDismissRequest = { pendingRelease = null }, title = { Text(stringResource(R.string.local_app_v2_allow_version_title)) }, text = {
        Text(stringResource(R.string.local_app_v2_execution_note, target.revision, target.digest.take(16)))
    }, confirmButton = { TextButton(onClick = { pendingRelease = null; action { apps.grant(appId, target.digest); apps.activate(appId, target.revision) } }) { Text(stringResource(R.string.local_app_v2_allow_release)) } }, dismissButton = { TextButton(onClick = { pendingRelease = null }) { Text(stringResource(R.string.local_app_v2_cancel)) } }) }
    publication?.let { prepared -> AlertDialog(onDismissRequest = { if (!busy) publication = null }, title = { Text(stringResource(R.string.local_app_v2_publish_review)) }, text = {
        LazyColumn {
            item { Text(stringResource(R.string.local_app_v2_publish_note, prepared.revision)); Text(prepared.name, style = MaterialTheme.typography.titleMedium); Text(prepared.summary) }
            items(prepared.files, key = { it }) { Text(it, style = MaterialTheme.typography.bodySmall) }
        }
    }, confirmButton = { TextButton(enabled = !busy, onClick = { action {
        val status = apps.market.publisher.publish(prepared)
        publicationStatus = context.getString(R.string.local_app_v2_publish_status, status)
        publication = null
    } }) { Text(stringResource(R.string.local_app_v2_publish_confirm)) } }, dismissButton = { TextButton(enabled = !busy, onClick = { publication = null }) { Text(stringResource(R.string.local_app_v2_cancel)) } }) }
    agentRelease?.let { release -> LocalAppAgentSettingsDialog(apps, release) { agentRelease = null } }
    if (deleting) AlertDialog(onDismissRequest = { deleting = false }, title = { Text(stringResource(R.string.local_app_v2_remove_title)) }, text = { Text(stringResource(R.string.local_app_v2_remove_note)) }, confirmButton = { TextButton(onClick = { deleting = false; scope.launch { try { apps.remove(appId); onBack() } catch (e: Exception) { error = e.message } } }) { Text(stringResource(R.string.local_app_v2_remove_confirm)) } }, dismissButton = { TextButton(onClick = { deleting = false }) { Text(stringResource(R.string.local_app_v2_cancel)) } })
}
