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

            LocalAppsDestination.Templates -> LocalAppTemplatePicker(
                state = state,
                onAction = onAction,
            )

            is LocalAppsDestination.Designer -> LocalAppDesignerScreen(
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
                title = { Text("应用操作失败") },
                text = { Text(message) },
                confirmButton = {
                    TextButton(onClick = { onAction(LocalAppsAction.DismissError) }) {
                        Text("知道了")
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
    Scaffold(
        topBar = {
            TopAppBar(
                title = { Text(stringResource(R.string.local_apps_title)) },
                navigationIcon = {
                    IconButton(onClick = onOpenDrawer) {
                        Icon(Icons.Rounded.Menu, contentDescription = "打开侧栏")
                    }
                },
                actions = {
                    IconButton(onClick = { onAction(LocalAppsAction.Refresh) }) {
                        Icon(Icons.Rounded.Refresh, contentDescription = stringResource(R.string.local_apps_refresh))
                    }
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
                modifier = Modifier.fillMaxWidth(),
            )
            LazyRow(
                horizontalArrangement = Arrangement.spacedBy(8.dp),
                modifier = Modifier.padding(vertical = 12.dp),
            ) {
                item {
                    FilterChip(
                        selected = state.templateFilter == null,
                        onClick = { onAction(LocalAppsAction.FilterTemplate(null)) },
                        label = { Text("全部") },
                    )
                }
                items(state.templates, key = { it.kind }) { template ->
                    FilterChip(
                        selected = state.templateFilter == template.kind,
                        onClick = { onAction(LocalAppsAction.FilterTemplate(template.kind)) },
                        label = { Text(template.name) },
                    )
                }
            }

            when {
                state.loading -> Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) {
                    CircularProgressIndicator()
                }

                state.filteredApps.isEmpty() -> EmptyApps(onCreate = { onAction(LocalAppsAction.Create) })

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
                Text(if (direct) "Direct · Next 本地服务" else "Play · 静态应用", fontWeight = FontWeight.SemiBold)
                Text(
                    if (direct) "使用生产构建，运行实例受设备内存配额控制。" else "使用已校验的静态导出产物。",
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
                    Text(app.templateName, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                }
                AssistChip(onClick = {}, label = { Text(app.workflow.label()) })
                Box {
                    IconButton(onClick = { menuExpanded = true }) {
                        Icon(Icons.Rounded.MoreVert, contentDescription = "应用操作")
                    }
                    DropdownMenu(expanded = menuExpanded, onDismissRequest = { menuExpanded = false }) {
                        DropdownMenuItem(
                            text = { Text("删除应用", color = MaterialTheme.colorScheme.error) },
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
                        Text("停止")
                    }
                } else if (app.workflow == LocalAppWorkflow.Ready) {
                    Button(onClick = { onAction(LocalAppsAction.StartRuntime(app.id)) }) {
                        Icon(Icons.Rounded.PlayArrow, contentDescription = null, modifier = Modifier.size(18.dp))
                        Spacer(Modifier.width(6.dp))
                        Text("启动")
                    }
                }
            }
        }
    }
    if (confirmDelete) {
        AlertDialog(
            onDismissRequest = { confirmDelete = false },
            title = { Text("删除 ${app.name}？") },
            text = { Text("代码、构建产物、日志和本地 SQLite 数据都会永久删除。") },
            confirmButton = {
                Button(onClick = {
                    confirmDelete = false
                    onAction(LocalAppsAction.DeleteApp(app.id))
                }) { Text("删除应用") }
            },
            dismissButton = { TextButton(onClick = { confirmDelete = false }) { Text("取消") } },
        )
    }
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun LocalAppTemplatePicker(state: LocalAppsUiState, onAction: (LocalAppsAction) -> Unit) {
    Scaffold(
        topBar = { LocalAppsTopBar(stringResource(R.string.local_apps_templates_title), onBack = { onAction(LocalAppsAction.Back) }) },
    ) { padding ->
        Column(
            modifier = Modifier
                .fillMaxSize()
                .padding(padding)
                .padding(horizontal = 16.dp),
        ) {
            OutlinedTextField(
                value = state.createName,
                onValueChange = { onAction(LocalAppsAction.ChangeCreateName(it)) },
                label = { Text("应用名称") },
                supportingText = { Text("稍后可在设计器中继续完善用途和目标用户。") },
                singleLine = true,
                modifier = Modifier.fillMaxWidth(),
            )
            Spacer(Modifier.height(12.dp))
            if (state.templatesLoading) {
                Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) { CircularProgressIndicator() }
            } else if (state.templates.isEmpty()) {
                Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) {
                    Text("模板尚未从本地引擎加载")
                }
            } else {
                LazyColumn(verticalArrangement = Arrangement.spacedBy(10.dp)) {
                    items(state.templates, key = { it.kind }) { template ->
                        Card(
                            onClick = { onAction(LocalAppsAction.SelectTemplate(template.kind)) },
                            colors = CardDefaults.cardColors(
                                containerColor = if (state.selectedTemplateKind == template.kind) {
                                    MaterialTheme.colorScheme.primaryContainer
                                } else {
                                    MaterialTheme.colorScheme.surfaceContainer
                                },
                            ),
                            modifier = Modifier.fillMaxWidth(),
                        ) {
                            Column(Modifier.padding(16.dp)) {
                                Text(template.name, style = MaterialTheme.typography.titleMedium)
                                Text(template.description, style = MaterialTheme.typography.bodyMedium)
                                Text("${template.steps.size} 步 · 模板 v${template.version}", style = MaterialTheme.typography.labelSmall)
                            }
                        }
                    }
                    item {
                        Button(
                            enabled = state.createName.isNotBlank() && state.selectedTemplateKind != null,
                            onClick = { onAction(LocalAppsAction.CreateSelectedTemplate) },
                            modifier = Modifier.fillMaxWidth(),
                        ) { Text("创建并开始设计") }
                    }
                    item { Spacer(Modifier.height(20.dp)) }
                }
            }
        }
    }
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun LocalAppDesignerScreen(state: LocalAppsUiState, onAction: (LocalAppsAction) -> Unit) {
    val designer = state.designer
    if (designer == null) {
        Scaffold(topBar = { LocalAppsTopBar(stringResource(R.string.local_apps_designer_title), onBack = { onAction(LocalAppsAction.Back) }) }) { padding ->
            Box(Modifier.fillMaxSize().padding(padding), contentAlignment = Alignment.Center) { CircularProgressIndicator() }
        }
        return
    }
    val steps = designer.template.steps.sortedBy { it.order }
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
                    ) { Text("上一步") }
                    Button(
                        enabled = currentStepComplete,
                        onClick = {
                            if (stepIndex < steps.lastIndex) onAction(LocalAppsAction.ChangeStep(stepIndex + 1))
                            else onAction(LocalAppsAction.ConfirmDesign)
                        },
                        modifier = Modifier.weight(1f),
                    ) { Text(if (stepIndex < steps.lastIndex) "下一步" else stringResource(R.string.local_apps_confirm_generate)) }
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
            Text("步骤 ${stepIndex + 1} / ${steps.size}", style = MaterialTheme.typography.labelLarge)
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
                Text("让 Agent 提出设计建议")
            }
            designer.suggestion?.let { suggestion ->
                SuggestionPanel(suggestion = suggestion, onAction = onAction)
            }
            designer.conflictRevision?.let {
                Text("设计已在其他位置更新到版本 $it，已重新加载，请检查后继续。", color = MaterialTheme.colorScheme.error)
            }
            Spacer(Modifier.height(84.dp))
        }
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
                        Text(if (checked) "已启用" else "未启用", modifier = Modifier.weight(1f))
                        Switch(checked = checked, onCheckedChange = { onValueChange(LocalAppDesignValue.Toggle(it), false) })
                    }
                }

                LocalAppFieldKind.Density -> {
                    val compact = (value as? LocalAppDesignValue.Density)?.compact == true
                    Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                        FilterChip(selected = compact, onClick = { onValueChange(LocalAppDesignValue.Density(true), false) }, label = { Text("紧凑") })
                        FilterChip(selected = !compact, onClick = { onValueChange(LocalAppDesignValue.Density(false), false) }, label = { Text("舒适") })
                    }
                }

                LocalAppFieldKind.ScreenList,
                LocalAppFieldKind.FeatureList,
                LocalAppFieldKind.DomainList -> {
                    val values = (value as? LocalAppDesignValue.StringList)?.values.orEmpty()
                    StringListEditor(
                        values = values,
                        placeholder = if (field.kind == LocalAppFieldKind.DomainList) "example.com" else "添加一项",
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
        }
    }
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
                Icon(Icons.Rounded.Close, contentDescription = "删除 $item")
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
        ) { Icon(Icons.Rounded.Add, contentDescription = "添加") }
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
                        label = { Text("字段名称") },
                        singleLine = true,
                        modifier = Modifier.weight(1f),
                    )
                    IconButton(onClick = { onChange(fields.filterIndexed { i, _ -> i != index }) }) {
                        Icon(Icons.Rounded.Close, contentDescription = "删除字段")
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
                    Text("必填")
                }
            }
        }
    }
    OutlinedButton(
        onClick = {
            val id = generateFieldId(fields)
            onChange(fields + LocalAppDataField(id, "新字段", LocalAppDataFieldType.Text, false))
        },
        modifier = Modifier.fillMaxWidth(),
    ) {
        Icon(Icons.Rounded.Add, contentDescription = null)
        Spacer(Modifier.width(6.dp))
        Text("添加数据字段")
    }
}

@Composable
private fun SuggestionPanel(suggestion: LocalAppSuggestion, onAction: (LocalAppsAction) -> Unit) {
    Card(colors = CardDefaults.cardColors(containerColor = MaterialTheme.colorScheme.tertiaryContainer)) {
        Column(verticalArrangement = Arrangement.spacedBy(8.dp), modifier = Modifier.padding(14.dp)) {
            Text("Agent 建议", style = MaterialTheme.typography.titleMedium)
            Text(suggestion.summary)
            suggestion.changes.forEach { change ->
                Text("${change.label}：${change.before} → ${change.after}", style = MaterialTheme.typography.bodySmall)
            }
            Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                Button(onClick = { onAction(LocalAppsAction.ApplySuggestion) }) { Text("应用建议") }
                TextButton(onClick = { onAction(LocalAppsAction.DismissSuggestion) }) { Text("忽略") }
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
    Scaffold(topBar = { LocalAppsTopBar(app?.name ?: "应用预览", onBack = { onAction(LocalAppsAction.Back) }) }) { padding ->
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
                ) { Text("继续设计") }
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
                        Text("预览服务尚未就绪，仍可批准或提交修改。")
                    }
                }
                Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    Button(onClick = { onAction(LocalAppsAction.ApprovePreview(appId)) }) { Text("批准预览") }
                    OutlinedButton(onClick = { onAction(LocalAppsAction.StartRuntime(appId)) }) { Text("重新加载") }
                }
                OutlinedTextField(value = feedback, onValueChange = { feedback = it }, label = { Text("反馈修改") }, modifier = Modifier.fillMaxWidth())
                Button(
                    enabled = feedback.isNotBlank(),
                    onClick = { onAction(LocalAppsAction.SubmitRevision(appId, feedback)); feedback = "" },
                    modifier = Modifier.fillMaxWidth(),
                ) { Text("提交给 Agent") }
            } else if (generation == null) {
                Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) { Text("正在等待生成任务…") }
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
    Scaffold(topBar = { LocalAppsTopBar(app?.name ?: "应用详情", onBack = { onAction(LocalAppsAction.Back) }) }) { padding ->
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
                        DetailsPlaceholder("应用尚未运行", "启动后可在此预览。")
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
        DetailsPlaceholder("正在加载数据结构", "应用详情返回后会显示受控 SQLite 集合。")
        return
    }
    LazyColumn(
        verticalArrangement = Arrangement.spacedBy(12.dp),
        modifier = Modifier.fillMaxSize().padding(16.dp),
    ) {
        item {
            Card {
                Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
                    Text("受控本地数据", style = MaterialTheme.typography.titleMedium)
                    Text("SQLite 由 Rust AppService 独占管理。页面与 Agent 只能使用 collection API；原始 SQL 不会暴露。")
                }
            }
        }
        if (details.collections.isEmpty()) {
            item { Text("此应用未声明持久化集合。", color = MaterialTheme.colorScheme.onSurfaceVariant) }
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
                            Text(
                                buildString {
                                    append(field.type.label())
                                    if (field.required) append(" · 必填")
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
            runCatching { LocalAppCodeBrowser(context.filesDir, path) }
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
        DetailsPlaceholder("正在加载工作区", "应用详情返回后会显示可编辑源码。")
        return
    }
    if (browser == null) {
        DetailsPlaceholder("无法打开工作区", message ?: "工作区路径无效。")
        return
    }

    if (selectedPath == null) {
        LazyColumn(
            verticalArrangement = Arrangement.spacedBy(8.dp),
            modifier = Modifier.fillMaxSize().padding(16.dp),
        ) {
            item {
                Text(details.workspaceRelativePath, style = MaterialTheme.typography.bodySmall)
                Text(
                    "仅显示 1 MiB 以内的文本源码；符号链接、构建产物和私有目录已排除。依赖文件可编辑，但不受支持的版本会在构建校验时被拒绝。",
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
            if (loading) item { LinearProgressIndicator(Modifier.fillMaxWidth()) }
            if (!loading && files.isEmpty()) item { Text("暂无可编辑源码。") }
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
                    Text("关闭")
                }
                Button(
                    onClick = {
                        val path = selectedPath ?: return@Button
                        scope.launch {
                            runCatching { withContext(Dispatchers.IO) { browser.save(path, editorText) } }
                                .onSuccess { message = "已保存；下次构建会重新校验源码。" }
                                .onFailure { message = it.message }
                        }
                    },
                    modifier = Modifier.weight(1f),
                ) { Text("保存") }
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
            Text("恢复只回退 Git 管理的源码，SQLite 数据不会回滚。恢复前会自动创建 pre_restore 检查点。")
        }
        if (details == null) item { LinearProgressIndicator(Modifier.fillMaxWidth()) }
        if (details != null && details.checkpoints.isEmpty()) item { Text("尚无检查点。") }
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
                    TextButton(onClick = { pendingRestore = checkpoint }) { Text("恢复") }
                }
            }
        }
    }
    pendingRestore?.let { checkpoint ->
        AlertDialog(
            onDismissRequest = { pendingRestore = null },
            title = { Text("恢复源码检查点？") },
            text = { Text("将恢复到“${checkpoint.label}”。当前源码会先保存为 pre_restore，应用数据保持不变。") },
            confirmButton = {
                Button(onClick = {
                    pendingRestore = null
                    onAction(LocalAppsAction.RestoreCheckpoint(appId, checkpoint.id))
                }) { Text("继续") }
            },
            dismissButton = { TextButton(onClick = { pendingRestore = null }) { Text("取消") } },
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
            Text("能力授权", style = MaterialTheme.typography.titleMedium)
            Text("首次修改数据、控制 UI 或访问声明域名时，系统会提供允许一次、本次会话或始终允许。未授权请求默认拒绝。")
            OutlinedButton(onClick = { confirmReset = true }) {
                Text("撤销全部授权")
            }
        }
        item {
            Text("清单声明的 HTTPS 域名", style = MaterialTheme.typography.titleMedium)
            if (details?.allowedDomains.isNullOrEmpty()) {
                Text("无（默认 CSP 禁止外网）", color = MaterialTheme.colorScheme.onSurfaceVariant)
            } else {
                details?.allowedDomains.orEmpty().forEach { Text("• $it") }
            }
        }
        item {
            Text("运行日志", style = MaterialTheme.typography.titleMedium)
            Text("状态：${details?.runtime?.state?.label() ?: "加载中"}")
            details?.runtime?.mode?.let { Text("模式：${it.name}") }
            details?.runtime?.recovery?.let { Text("恢复：$it") }
            details?.runtime?.detail?.let { Text(it, color = MaterialTheme.colorScheme.error) }
            if (details?.runtime?.detail == null) {
                Text("当前没有运行错误。完整构建日志保存在应用 logs 目录，并可由受控 MCP read_logs 查询。")
            }
        }
    }
    if (confirmReset) {
        AlertDialog(
            onDismissRequest = { confirmReset = false },
            title = { Text("撤销该应用的全部授权？") },
            text = { Text("会话授权和持久授权都会清除。下次修改数据、控制界面或访问域名时会重新询问。") },
            confirmButton = {
                Button(onClick = {
                    confirmReset = false
                    onAction(LocalAppsAction.ResetPermissions(appId))
                }) { Text("撤销") }
            },
            dismissButton = {
                TextButton(onClick = { confirmReset = false }) { Text("取消") }
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
                Text("应用：${request.appId}", style = MaterialTheme.typography.bodySmall)
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
                Icon(Icons.AutoMirrored.Rounded.ArrowBack, contentDescription = "返回")
            }
        },
    )
}

private fun LocalAppWorkflow.label(): String = when (this) {
    LocalAppWorkflow.CollectingSpec -> "设计中"
    LocalAppWorkflow.AwaitingSpecConfirmation -> "待确认设计"
    LocalAppWorkflow.Generating -> "生成中"
    LocalAppWorkflow.Validating -> "校验中"
    LocalAppWorkflow.AwaitingPreviewConfirmation -> "待批准预览"
    LocalAppWorkflow.Revising -> "修改中"
    LocalAppWorkflow.Ready -> "就绪"
    LocalAppWorkflow.GenerationFailed -> "生成失败"
    LocalAppWorkflow.ValidationFailed -> "校验失败"
}

private fun LocalAppRuntimeState.label(): String = when (this) {
    LocalAppRuntimeState.Stopped -> "已停止"
    LocalAppRuntimeState.Starting -> "启动中"
    LocalAppRuntimeState.Running -> "运行中"
    LocalAppRuntimeState.Stopping -> "停止中"
    LocalAppRuntimeState.Failed -> "运行失败"
}

private fun LocalAppDataFieldType.label(): String = when (this) {
    LocalAppDataFieldType.Text -> "文本"
    LocalAppDataFieldType.LongText -> "长文本"
    LocalAppDataFieldType.Integer -> "整数"
    LocalAppDataFieldType.Decimal -> "小数"
    LocalAppDataFieldType.Boolean -> "布尔"
    LocalAppDataFieldType.DateTime -> "日期时间"
    LocalAppDataFieldType.Enum -> "枚举"
    LocalAppDataFieldType.ImageRef -> "图片引用"
}

private fun LocalAppDetailsTab.label(): String = when (this) {
    LocalAppDetailsTab.Preview -> "预览"
    LocalAppDetailsTab.Data -> "数据"
    LocalAppDetailsTab.Code -> "代码"
    LocalAppDetailsTab.History -> "历史"
    LocalAppDetailsTab.PermissionsLogs -> "权限/日志"
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
