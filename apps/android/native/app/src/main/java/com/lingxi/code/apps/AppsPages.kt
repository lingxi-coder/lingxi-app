package com.lingxi.code.apps

import android.text.format.Formatter
import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.systemBars
import androidx.compose.foundation.layout.windowInsetsPadding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.lingxi.code.R
import com.lingxi.code.components.LXIcon
import com.lingxi.code.components.LXIconName
import com.lingxi.code.components.PageTopBar
import com.lingxi.code.components.UiTags
import com.lingxi.code.components.blockTouchesBehind
import com.lingxi.code.market.MarketApp
import com.lingxi.code.market.MarketException
import com.lingxi.code.market.MarketState
import com.lingxi.code.market.MarketVersion
import com.lingxi.code.market.UpdateStatus
import com.lingxi.code.settings.SettingsRow
import com.lingxi.code.settings.SettingsSection
import com.lingxi.code.theme.LingXiTheme
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch

// MARK: - shared pieces ---------------------------------------------------------

private val CoverColors = mapOf(
    "tools" to Color(0xFF6E96FF),
    "productivity" to Color(0xFF00C2C3),
    "games" to Color(0xFFCE74E3),
    "education" to Color(0xFFDE9300),
    "visualization" to Color(0xFFE5687A),
)
private val OtherCoverColor = Color(0xFF8A8F98)

/** A cover in the market's single-character style, tinted by category. */
@Composable
internal fun AppCover(name: String, category: String, size: Dp, modifier: Modifier = Modifier) {
    val color = CoverColors[category] ?: OtherCoverColor
    val shape = RoundedCornerShape(size * 0.26f)
    Box(
        contentAlignment = Alignment.Center,
        modifier = modifier
            .size(size)
            .clip(shape)
            .background(color.copy(alpha = 0.16f))
            .border(0.5.dp, color.copy(alpha = 0.35f), shape),
    ) {
        Text(appInitial(name), color = color, fontSize = (size.value * 0.42f).sp, fontWeight = FontWeight.Bold)
    }
}

@Composable
internal fun categoryLabel(category: String): String = stringResource(
    when (category) {
        "tools" -> R.string.apps_category_tools
        "productivity" -> R.string.apps_category_productivity
        "games" -> R.string.apps_category_games
        "education" -> R.string.apps_category_education
        "visualization" -> R.string.apps_category_visualization
        else -> R.string.apps_category_other
    },
)

/** The market's error code, or the exception's own message when it is not a market error. */
internal fun marketErrorCode(error: Throwable): String =
    (error as? MarketException)?.code ?: error.message ?: "UNKNOWN_ERROR"

/** Installs the listing's current version and keeps its name for the pages. */
internal fun installApp(localApps: LocalApps, app: MarketApp) {
    val version = app.currentVersion ?: return
    localApps.info.remember(app)
    localApps.market.store.install(app.id, version.id)
}

internal fun updateApp(localApps: LocalApps, appId: String, versionId: String, app: MarketApp?) {
    app?.let(localApps.info::remember)
    localApps.market.store.update(appId, versionId)
}

/** Uninstalls, then forgets what the pages knew about the app once it is gone. */
internal fun uninstallApp(localApps: LocalApps, appId: String) {
    val store = localApps.market.store
    val job = store.uninstall(appId)
    localApps.scope.launch {
        job.join()
        if (store.state.value.installed.none { it.appId == appId }) localApps.info.forget(appId)
    }
}

/** Deletes an app made in chat (key `chat:<id>`) with its data, then forgets it once it is gone. */
internal fun deleteChatApp(localApps: LocalApps, key: String) {
    val id = chatAppIdOf(key) ?: return
    val job = localApps.chat.delete(id)
    localApps.scope.launch {
        job.join()
        if (localApps.chat.state.value.none { it.id == id }) localApps.info.forget(key)
    }
}

/** Confirms removing an app; an app made in chat is deleted rather than uninstalled. */
@Composable
internal fun UninstallDialog(name: String, onConfirm: () -> Unit, onDismiss: () -> Unit, chat: Boolean = false) {
    val t = LingXiTheme.palette
    AlertDialog(
        onDismissRequest = onDismiss,
        title = {
            Text(stringResource(if (chat) R.string.apps_delete_title_fmt else R.string.apps_uninstall_title_fmt, name))
        },
        text = { Text(stringResource(R.string.apps_uninstall_body)) },
        confirmButton = {
            TextButton(onClick = onConfirm) {
                Text(
                    stringResource(if (chat) R.string.common_delete else R.string.apps_action_uninstall),
                    color = t.danger,
                )
            }
        },
        dismissButton = {
            TextButton(onClick = onDismiss) { Text(stringResource(R.string.common_cancel)) }
        },
    )
}

/** The compact main button of a market row: a pill, or a ring while busy (tap the ring to cancel). */
@Composable
private fun AppActionPill(action: AppAction, onClick: () -> Unit) {
    val t = LingXiTheme.palette
    when (action) {
        is AppAction.Downloading -> {
            val cancel = stringResource(R.string.common_cancel)
            Box(
                contentAlignment = Alignment.Center,
                modifier = Modifier
                    .size(40.dp)
                    .clip(CircleShape)
                    .semantics { contentDescription = cancel }
                    .clickable(onClick = onClick),
            ) {
                val fraction = action.fraction
                if (fraction != null) {
                    CircularProgressIndicator(
                        progress = { fraction },
                        modifier = Modifier.size(26.dp),
                        color = t.accent,
                        strokeWidth = 2.5.dp,
                        trackColor = t.border,
                    )
                } else {
                    CircularProgressIndicator(modifier = Modifier.size(26.dp), color = t.accent, strokeWidth = 2.5.dp)
                }
                Box(Modifier.size(8.dp).clip(RoundedCornerShape(1.5.dp)).background(t.accent))
            }
        }
        AppAction.Installing, AppAction.Removing ->
            CircularProgressIndicator(modifier = Modifier.size(22.dp), color = t.text3, strokeWidth = 2.dp)
        AppAction.Unavailable -> Unit
        AppAction.Get, AppAction.Open, is AppAction.Update -> {
            val filled = action != AppAction.Open
            Text(
                text = actionLabel(action),
                color = if (filled) Color.White else t.accent,
                fontSize = 13.sp,
                fontWeight = FontWeight.SemiBold,
                modifier = Modifier
                    .clip(RoundedCornerShape(50))
                    .background(if (filled) t.accent else t.accent.copy(alpha = 0.14f))
                    .clickable(onClick = onClick)
                    .padding(horizontal = 16.dp, vertical = 7.dp),
            )
        }
    }
}

@Composable
private fun actionLabel(action: AppAction): String = when (action) {
    AppAction.Get -> stringResource(R.string.apps_action_get)
    AppAction.Open -> stringResource(R.string.common_open)
    is AppAction.Update -> stringResource(R.string.apps_action_update)
    is AppAction.Downloading -> stringResource(R.string.apps_status_downloading)
    AppAction.Installing -> stringResource(R.string.apps_status_installing)
    AppAction.Removing -> stringResource(R.string.apps_status_removing)
    AppAction.Unavailable -> stringResource(R.string.apps_no_version)
}

/** Runs what an app's main button stands for. */
private fun runAction(localApps: LocalApps, app: MarketApp, action: AppAction, onOpenApp: (String) -> Unit) {
    when (action) {
        AppAction.Get -> installApp(localApps, app)
        AppAction.Open -> onOpenApp(app.id)
        is AppAction.Update -> updateApp(localApps, app.id, action.versionId, app)
        is AppAction.Downloading -> localApps.market.store.cancel(app.id)
        AppAction.Installing, AppAction.Removing, AppAction.Unavailable -> Unit
    }
}

private fun MarketState.actionFor(appId: String, currentVersion: MarketVersion?): AppAction = appAction(
    currentVersion = currentVersion,
    installed = installed.firstOrNull { it.appId == appId },
    update = updates.firstOrNull { it.appId == appId },
    operation = operations[appId],
)

@Composable
private fun SearchField(query: String, onQueryChange: (String) -> Unit, modifier: Modifier = Modifier) {
    val t = LingXiTheme.palette
    val shape = RoundedCornerShape(12.dp)
    Row(
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(8.dp),
        modifier = modifier
            .fillMaxWidth()
            .clip(shape)
            .background(t.surface)
            .border(0.5.dp, t.border, shape)
            .padding(horizontal = 14.dp, vertical = 11.dp),
    ) {
        LXIcon(name = LXIconName.Search, size = 16.dp, color = t.text4, stroke = 2f)
        Box(modifier = Modifier.weight(1f)) {
            if (query.isEmpty()) {
                Text(stringResource(R.string.apps_search_hint), color = t.text4, fontSize = 14.sp, maxLines = 1)
            }
            BasicTextField(
                value = query,
                onValueChange = onQueryChange,
                singleLine = true,
                textStyle = TextStyle(color = t.text, fontSize = 14.sp),
                cursorBrush = SolidColor(t.accent),
                modifier = Modifier
                    .fillMaxWidth()
                    .testTag(UiTags.APPS_SEARCH),
            )
        }
        if (query.isNotEmpty()) {
            Box(
                contentAlignment = Alignment.Center,
                modifier = Modifier
                    .size(20.dp)
                    .clip(CircleShape)
                    .clickable { onQueryChange("") },
            ) {
                LXIcon(
                    name = LXIconName.X,
                    size = 13.dp,
                    color = t.text4,
                    stroke = 2f,
                    contentDescription = stringResource(R.string.drawer_clear_search_a11y),
                )
            }
        }
    }
}

// MARK: - 应用 page -------------------------------------------------------------

enum class AppsTab { Mine, Market }

/**
 * 应用: the installed apps and the apps made in chat (我的), and the Local App
 * Market (市场). Opening an app runs it full screen; long-press a tile for its
 * details, update and uninstall, or to delete an app made in chat.
 */
@Composable
fun AppsPage(
    localApps: LocalApps,
    onBack: () -> Unit,
    onOpenApp: (String) -> Unit,
    onShowDetails: (String) -> Unit,
    modifier: Modifier = Modifier,
) {
    val t = LingXiTheme.palette
    val store = localApps.market.store
    val market by store.state.collectAsState()
    val info by localApps.info.state.collectAsState()
    val chatApps by localApps.chat.state.collectAsState()
    val apps = remember(market.installed, info, market.updates, market.operations, chatApps) {
        myApps(market.installed, info, market.updates, market.operations, chatApps)
    }
    var tab by rememberSaveable { mutableStateOf(AppsTab.Mine) }
    var query by rememberSaveable { mutableStateOf("") }
    var searched by remember { mutableStateOf(false) }
    var uninstalling by remember { mutableStateOf<MyApp?>(null) }

    // Apps made in chat since the list was last read.
    LaunchedEffect(Unit) { localApps.chat.refresh() }
    // Ask about updates once there is something installed to ask about.
    LaunchedEffect(market.installed.isNotEmpty()) {
        if (market.installed.isNotEmpty()) store.checkUpdates()
    }
    // The market loads when it is first shown and follows the search as the
    // user types; a result already on screen for the same text is kept.
    LaunchedEffect(tab, query) {
        if (tab != AppsTab.Market) return@LaunchedEffect
        val current = store.state.value
        if (searched && current.query == query.trim() && (current.apps.isNotEmpty() || current.loading)) {
            return@LaunchedEffect
        }
        if (searched) delay(300)
        searched = true
        store.query(query)
    }

    Column(
        modifier = modifier
            .fillMaxSize()
            .background(t.windowBg)
            .blockTouchesBehind()
            .windowInsetsPadding(WindowInsets.systemBars)
            .testTag(UiTags.APPS_PAGE),
    ) {
        PageTopBar(title = stringResource(R.string.apps_title), onBack = onBack)
        TabSwitch(selected = tab, onSelect = { tab = it })
        Box(Modifier.weight(1f)) {
            when (tab) {
                AppsTab.Mine -> MineTab(
                    apps = apps,
                    runtimeError = market.runtimeError,
                    onBrowseMarket = { tab = AppsTab.Market },
                    onOpenApp = onOpenApp,
                    onShowDetails = onShowDetails,
                    onUpdate = { app ->
                        val versionId = market.updates.firstOrNull {
                            it.appId == app.appId && it.status == UpdateStatus.UPDATE_AVAILABLE
                        }?.currentVersionId
                        if (versionId != null) updateApp(localApps, app.appId, versionId, app = null)
                    },
                    onUninstall = { uninstalling = it },
                    onCheckUpdates = { store.checkUpdates() },
                )
                AppsTab.Market -> MarketTab(
                    market = market,
                    query = query,
                    searched = searched,
                    onQueryChange = { query = it },
                    onShowDetails = onShowDetails,
                    onAction = { app, action -> runAction(localApps, app, action, onOpenApp) },
                    onLoadMore = { store.loadNext() },
                    // A failed first page starts over; a failed later page resumes.
                    onRetry = { if (market.apps.isEmpty()) store.query(query) else store.loadNext() },
                )
            }
        }
    }

    uninstalling?.let { app ->
        val chat = app.source == AppSource.CHAT
        UninstallDialog(
            name = app.name,
            chat = chat,
            onConfirm = {
                uninstalling = null
                if (chat) deleteChatApp(localApps, app.appId) else uninstallApp(localApps, app.appId)
            },
            onDismiss = { uninstalling = null },
        )
    }
}

@Composable
private fun TabSwitch(selected: AppsTab, onSelect: (AppsTab) -> Unit) {
    val t = LingXiTheme.palette
    Row(
        horizontalArrangement = Arrangement.spacedBy(6.dp),
        modifier = Modifier.padding(horizontal = 16.dp, vertical = 10.dp),
    ) {
        AppsTab.entries.forEach { option ->
            val active = option == selected
            val shape = RoundedCornerShape(50)
            Text(
                text = stringResource(
                    when (option) {
                        AppsTab.Mine -> R.string.apps_tab_mine
                        AppsTab.Market -> R.string.apps_tab_market
                    },
                ),
                color = if (active) t.text else t.text3,
                fontSize = 13.sp,
                fontWeight = if (active) FontWeight.SemiBold else FontWeight.Medium,
                modifier = Modifier
                    .clip(shape)
                    .background(if (active) t.text.copy(alpha = 0.10f) else t.surface)
                    .border(0.5.dp, if (active) t.text.copy(alpha = 0.3f) else t.border, shape)
                    .testTag(UiTags.appsTab(option.name))
                    .clickable { onSelect(option) }
                    .padding(horizontal = 16.dp, vertical = 7.dp),
            )
        }
    }
}

@Composable
private fun MineTab(
    apps: List<MyApp>,
    runtimeError: String?,
    onBrowseMarket: () -> Unit,
    onOpenApp: (String) -> Unit,
    onShowDetails: (String) -> Unit,
    onUpdate: (MyApp) -> Unit,
    onUninstall: (MyApp) -> Unit,
    onCheckUpdates: () -> Unit,
) {
    val t = LingXiTheme.palette
    Column(
        modifier = Modifier
            .fillMaxSize()
            .verticalScroll(rememberScrollState())
            .padding(horizontal = 16.dp)
            .padding(bottom = 16.dp),
    ) {
        if (runtimeError != null) {
            Text(
                text = stringResource(R.string.apps_webview_unsupported, runtimeError),
                color = t.statusError,
                fontSize = 12.5f.sp,
                modifier = Modifier
                    .fillMaxWidth()
                    .padding(bottom = 12.dp)
                    .clip(RoundedCornerShape(10.dp))
                    .background(t.statusError.copy(alpha = 0.10f))
                    .padding(12.dp),
            )
        }
        if (apps.isEmpty()) {
            Text(
                text = stringResource(R.string.apps_mine_empty),
                color = t.text3,
                fontSize = 14.sp,
                modifier = Modifier.padding(top = 24.dp, bottom = 12.dp),
            )
            Text(
                text = stringResource(R.string.apps_browse_market),
                color = Color.White,
                fontSize = 14.sp,
                fontWeight = FontWeight.SemiBold,
                modifier = Modifier
                    .clip(RoundedCornerShape(12.dp))
                    .background(t.accent)
                    .clickable(onClick = onBrowseMarket)
                    .padding(horizontal = 18.dp, vertical = 11.dp),
            )
            return@Column
        }
        Row(verticalAlignment = Alignment.CenterVertically, modifier = Modifier.padding(top = 4.dp, bottom = 6.dp)) {
            Text(
                text = "${stringResource(R.string.drawer_filter_all)} · ${apps.size}",
                color = t.text3,
                fontSize = 12.5f.sp,
                fontWeight = FontWeight.Medium,
                modifier = Modifier.weight(1f),
            )
            Text(
                text = stringResource(R.string.apps_check_updates),
                color = t.accent,
                fontSize = 12.5f.sp,
                fontWeight = FontWeight.Medium,
                modifier = Modifier
                    .clip(RoundedCornerShape(8.dp))
                    .clickable(onClick = onCheckUpdates)
                    .padding(horizontal = 8.dp, vertical = 6.dp),
            )
        }
        apps.chunked(GridColumns).forEach { row ->
            Row(modifier = Modifier.fillMaxWidth()) {
                row.forEach { app ->
                    AppTile(
                        app = app,
                        onOpen = { onOpenApp(app.appId) },
                        onShowDetails = { onShowDetails(app.appId) },
                        onUpdate = { onUpdate(app) },
                        onUninstall = { onUninstall(app) },
                        modifier = Modifier.weight(1f),
                    )
                }
                repeat(GridColumns - row.size) { Spacer(Modifier.weight(1f)) }
            }
        }
    }
}

private const val GridColumns = 4

@OptIn(ExperimentalFoundationApi::class)
@Composable
private fun AppTile(
    app: MyApp,
    onOpen: () -> Unit,
    onShowDetails: () -> Unit,
    onUpdate: () -> Unit,
    onUninstall: () -> Unit,
    modifier: Modifier = Modifier,
) {
    val t = LingXiTheme.palette
    var menuOpen by remember { mutableStateOf(false) }
    val busy = app.activeOperation != null
    Box(modifier) {
        Column(
            horizontalAlignment = Alignment.CenterHorizontally,
            modifier = Modifier
                .fillMaxWidth()
                .clip(RoundedCornerShape(12.dp))
                .testTag(UiTags.appTile(app.appId))
                .combinedClickable(
                    onClick = { if (!busy) onOpen() },
                    onLongClick = { menuOpen = true },
                )
                .padding(vertical = 8.dp),
        ) {
            Box {
                AppCover(app.name, app.category, 56.dp)
                if (busy) {
                    Box(
                        contentAlignment = Alignment.Center,
                        modifier = Modifier
                            .size(56.dp)
                            .clip(RoundedCornerShape(14.dp))
                            .background(t.windowBg.copy(alpha = 0.55f)),
                    ) {
                        CircularProgressIndicator(modifier = Modifier.size(24.dp), color = t.accent, strokeWidth = 2.5.dp)
                    }
                } else if (app.updateAvailable) {
                    Box(
                        Modifier
                            .align(Alignment.TopEnd)
                            .offset(x = 3.dp, y = (-3).dp)
                            .size(11.dp)
                            .clip(CircleShape)
                            .background(t.windowBg)
                            .padding(2.dp)
                            .clip(CircleShape)
                            .background(t.danger),
                    )
                }
            }
            Text(
                text = app.name,
                color = t.text2,
                fontSize = 12.sp,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
                modifier = Modifier.padding(top = 6.dp, start = 2.dp, end = 2.dp),
            )
        }
        DropdownMenu(expanded = menuOpen, onDismissRequest = { menuOpen = false }) {
            // An app made in chat has no market page; it can only be deleted.
            if (app.source == AppSource.MARKET) {
                DropdownMenuItem(
                    text = { Text(stringResource(R.string.apps_details)) },
                    onClick = {
                        menuOpen = false
                        onShowDetails()
                    },
                )
            }
            if (app.updateAvailable && !busy) {
                DropdownMenuItem(
                    text = { Text(stringResource(R.string.apps_action_update)) },
                    onClick = {
                        menuOpen = false
                        onUpdate()
                    },
                )
            }
            if (!busy) {
                DropdownMenuItem(
                    text = {
                        Text(
                            stringResource(
                                if (app.source == AppSource.CHAT) R.string.common_delete else R.string.apps_action_uninstall,
                            ),
                            color = t.danger,
                        )
                    },
                    onClick = {
                        menuOpen = false
                        onUninstall()
                    },
                )
            }
        }
    }
}

@Composable
private fun MarketTab(
    market: MarketState,
    query: String,
    searched: Boolean,
    onQueryChange: (String) -> Unit,
    onShowDetails: (String) -> Unit,
    onAction: (MarketApp, AppAction) -> Unit,
    onLoadMore: () -> Unit,
    onRetry: () -> Unit,
) {
    val t = LingXiTheme.palette
    Column(Modifier.fillMaxSize()) {
        SearchField(
            query = query,
            onQueryChange = onQueryChange,
            modifier = Modifier.padding(horizontal = 16.dp).padding(bottom = 8.dp),
        )
        LazyColumn(
            modifier = Modifier
                .weight(1f)
                .fillMaxWidth()
                .testTag(UiTags.APPS_MARKET_LIST),
        ) {
            items(market.apps, key = MarketApp::id) { app ->
                MarketAppRow(
                    app = app,
                    action = market.actionFor(app.id, app.currentVersion),
                    onClick = { onShowDetails(app.id) },
                    onAction = { onAction(app, it) },
                )
            }
            item(key = "footer") {
                Box(
                    contentAlignment = Alignment.Center,
                    modifier = Modifier
                        .fillMaxWidth()
                        .padding(horizontal = 16.dp, vertical = 18.dp),
                ) {
                    when {
                        market.loading ->
                            CircularProgressIndicator(modifier = Modifier.size(22.dp), color = t.text3, strokeWidth = 2.dp)
                        market.error != null -> Column(horizontalAlignment = Alignment.CenterHorizontally) {
                            Text(
                                text = stringResource(R.string.apps_market_error_fmt, market.error),
                                color = t.text3,
                                fontSize = 13.sp,
                            )
                            Text(
                                text = stringResource(R.string.common_retry),
                                color = t.accent,
                                fontSize = 13.sp,
                                fontWeight = FontWeight.SemiBold,
                                modifier = Modifier
                                    .padding(top = 8.dp)
                                    .clip(RoundedCornerShape(8.dp))
                                    .clickable(onClick = onRetry)
                                    .padding(horizontal = 12.dp, vertical = 6.dp),
                            )
                        }
                        market.cursor != null -> {
                            // The end of the list came into view: fetch the next page.
                            LaunchedEffect(market.apps.size) { onLoadMore() }
                            CircularProgressIndicator(modifier = Modifier.size(22.dp), color = t.text3, strokeWidth = 2.dp)
                        }
                        searched && market.apps.isEmpty() ->
                            Text(stringResource(R.string.apps_market_empty), color = t.text4, fontSize = 13.sp)
                    }
                }
            }
        }
    }
}

@Composable
private fun MarketAppRow(app: MarketApp, action: AppAction, onClick: () -> Unit, onAction: (AppAction) -> Unit) {
    val t = LingXiTheme.palette
    Row(
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(12.dp),
        modifier = Modifier
            .fillMaxWidth()
            .testTag(UiTags.marketRow(app.id))
            .clickable(onClick = onClick)
            .padding(horizontal = 16.dp, vertical = 10.dp),
    ) {
        AppCover(app.name, app.category, 48.dp)
        Column(Modifier.weight(1f)) {
            Text(
                app.name,
                color = t.text,
                fontSize = 15.sp,
                fontWeight = FontWeight.SemiBold,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
            if (app.summary.isNotBlank()) {
                Text(app.summary, color = t.text3, fontSize = 12.5f.sp, maxLines = 2, overflow = TextOverflow.Ellipsis)
            }
            Text(categoryLabel(app.category), color = t.text4, fontSize = 11.sp, modifier = Modifier.padding(top = 2.dp))
        }
        AppActionPill(action = action, onClick = { onAction(action) })
    }
}

// MARK: - one app's page --------------------------------------------------------

/**
 * One app: what the market says about it, the button that gets, opens, updates
 * or cancels it, its versions, and uninstall. An installed app still shows its
 * remembered name and can be opened or removed while the market is unreachable.
 */
@Composable
fun MarketAppPage(
    localApps: LocalApps,
    appId: String,
    onBack: () -> Unit,
    onOpenApp: (String) -> Unit,
    modifier: Modifier = Modifier,
) {
    val t = LingXiTheme.palette
    val context = LocalContext.current
    val store = localApps.market.store
    val market by store.state.collectAsState()
    val info by localApps.info.state.collectAsState()
    var app by remember(appId) { mutableStateOf(market.apps.firstOrNull { it.id == appId }) }
    var versions by remember(appId) { mutableStateOf<List<MarketVersion>>(emptyList()) }
    var loadError by remember(appId) { mutableStateOf<String?>(null) }
    var attempt by remember(appId) { mutableStateOf(0) }
    var confirmUninstall by remember { mutableStateOf(false) }
    val installed = market.installed.firstOrNull { it.appId == appId }
    val known = info[appId]

    LaunchedEffect(appId, attempt) {
        loadError = null
        try {
            val loaded = store.detail(appId)
            app = loaded
            if (store.state.value.installed.any { it.appId == appId }) localApps.info.remember(loaded)
            versions = store.versions(appId).items
        } catch (e: CancellationException) {
            throw e
        } catch (e: Throwable) {
            loadError = marketErrorCode(e)
        }
    }

    val name = app?.name ?: known?.name ?: appId
    val category = app?.category ?: known?.category.orEmpty()
    val summary = app?.summary ?: known?.summary.orEmpty()
    val operation = market.operations[appId]
    val action = market.actionFor(appId, app?.currentVersion)

    Column(
        modifier = modifier
            .fillMaxSize()
            .background(t.windowBg)
            .blockTouchesBehind()
            .windowInsetsPadding(WindowInsets.systemBars)
            .testTag(UiTags.MARKET_APP_PAGE),
    ) {
        PageTopBar(title = stringResource(R.string.apps_details), onBack = onBack)
        Column(
            modifier = Modifier
                .weight(1f)
                .verticalScroll(rememberScrollState())
                .padding(16.dp),
        ) {
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(14.dp)) {
                AppCover(name, category, 72.dp)
                Column(Modifier.weight(1f)) {
                    Text(name, color = t.text, fontSize = 19.sp, fontWeight = FontWeight.Bold, maxLines = 2, overflow = TextOverflow.Ellipsis)
                    Text(
                        text = listOfNotNull(
                            categoryLabel(category),
                            installed?.let { stringResource(R.string.apps_installed) },
                        ).joinToString(" · "),
                        color = t.text4,
                        fontSize = 12.5f.sp,
                        modifier = Modifier.padding(top = 3.dp),
                    )
                }
            }
            if (summary.isNotBlank()) {
                Text(summary, color = t.text2, fontSize = 14.sp, modifier = Modifier.padding(top = 14.dp))
            }

            Spacer(Modifier.height(18.dp))
            MainActionButton(
                action = action,
                onClick = {
                    when (action) {
                        AppAction.Get -> app?.let { installApp(localApps, it) }
                        AppAction.Open -> onOpenApp(appId)
                        is AppAction.Update -> updateApp(localApps, appId, action.versionId, app)
                        is AppAction.Downloading -> store.cancel(appId)
                        AppAction.Installing, AppAction.Removing, AppAction.Unavailable -> Unit
                    }
                },
            )
            failedOperationCode(operation)?.let { code ->
                Text(
                    text = stringResource(R.string.apps_operation_failed_fmt, code),
                    color = t.statusError,
                    fontSize = 12.5f.sp,
                    modifier = Modifier.padding(top = 8.dp),
                )
            }
            if (app == null && loadError == null) {
                Box(Modifier.fillMaxWidth().padding(top = 20.dp), contentAlignment = Alignment.Center) {
                    CircularProgressIndicator(modifier = Modifier.size(22.dp), color = t.text3, strokeWidth = 2.dp)
                }
            }
            loadError?.let { code ->
                Row(verticalAlignment = Alignment.CenterVertically, modifier = Modifier.padding(top = 14.dp)) {
                    Text(
                        text = stringResource(R.string.apps_market_error_fmt, code),
                        color = t.text3,
                        fontSize = 13.sp,
                        modifier = Modifier.weight(1f),
                    )
                    Text(
                        text = stringResource(R.string.common_retry),
                        color = t.accent,
                        fontSize = 13.sp,
                        fontWeight = FontWeight.SemiBold,
                        modifier = Modifier
                            .clip(RoundedCornerShape(8.dp))
                            .clickable { attempt++ }
                            .padding(horizontal = 10.dp, vertical = 6.dp),
                    )
                }
            }

            app?.currentVersion?.let { version ->
                Spacer(Modifier.height(18.dp))
                SettingsSection(label = stringResource(R.string.apps_version_notes)) {
                    SettingsRow(
                        label = stringResource(R.string.apps_version_fmt, version.sequence),
                        sub = version.notes.takeIf(String::isNotBlank),
                        value = listOfNotNull(
                            version.sizeBytes?.let { Formatter.formatShortFileSize(context, it) },
                            version.publishedAt?.take(10),
                        ).joinToString(" · "),
                        chevron = false,
                        isLast = true,
                    )
                }
            }
            val older = versions.filter { it.id != app?.currentVersion?.id }
            if (older.isNotEmpty()) {
                Spacer(Modifier.height(16.dp))
                SettingsSection(label = stringResource(R.string.apps_versions)) {
                    older.forEachIndexed { index, version ->
                        SettingsRow(
                            label = stringResource(R.string.apps_version_fmt, version.sequence),
                            sub = version.notes.takeIf(String::isNotBlank),
                            value = version.publishedAt?.take(10),
                            chevron = false,
                            isLast = index == older.lastIndex,
                        )
                    }
                }
            }
            if (installed != null && action != AppAction.Removing) {
                Spacer(Modifier.height(16.dp))
                SettingsSection {
                    SettingsRow(
                        label = stringResource(R.string.apps_action_uninstall),
                        danger = true,
                        chevron = false,
                        isLast = true,
                        onTap = { confirmUninstall = true },
                        modifier = Modifier.testTag(UiTags.MARKET_APP_UNINSTALL),
                    )
                }
            }
        }
    }

    if (confirmUninstall) {
        UninstallDialog(
            name = name,
            onConfirm = {
                confirmUninstall = false
                uninstallApp(localApps, appId)
            },
            onDismiss = { confirmUninstall = false },
        )
    }
}

/** The detail page's full-width main button; a download shows its progress and cancels on tap. */
@Composable
private fun MainActionButton(action: AppAction, onClick: () -> Unit) {
    val t = LingXiTheme.palette
    val shape = RoundedCornerShape(12.dp)
    when (action) {
        is AppAction.Downloading -> Column(
            modifier = Modifier
                .fillMaxWidth()
                .clip(shape)
                .background(t.surface)
                .border(0.5.dp, t.border, shape)
                .clickable(onClick = onClick)
                .padding(horizontal = 14.dp, vertical = 12.dp),
        ) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                Text(stringResource(R.string.apps_status_downloading), color = t.text2, fontSize = 14.sp, modifier = Modifier.weight(1f))
                Text(stringResource(R.string.common_cancel), color = t.accent, fontSize = 14.sp, fontWeight = FontWeight.SemiBold)
            }
            val fraction = action.fraction
            if (fraction != null) {
                LinearProgressIndicator(
                    progress = { fraction },
                    modifier = Modifier.fillMaxWidth().padding(top = 10.dp),
                    color = t.accent,
                    trackColor = t.border,
                )
            } else {
                LinearProgressIndicator(modifier = Modifier.fillMaxWidth().padding(top = 10.dp), color = t.accent, trackColor = t.border)
            }
        }
        else -> {
            val enabled = action == AppAction.Get || action == AppAction.Open || action is AppAction.Update
            Row(
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(8.dp, Alignment.CenterHorizontally),
                modifier = Modifier
                    .fillMaxWidth()
                    .clip(shape)
                    .background(if (enabled) t.accent else t.surface)
                    .testTag(UiTags.MARKET_APP_ACTION)
                    .clickable(enabled = enabled, onClick = onClick)
                    .padding(vertical = 13.dp),
            ) {
                if (action == AppAction.Installing || action == AppAction.Removing) {
                    CircularProgressIndicator(modifier = Modifier.size(16.dp), color = t.text3, strokeWidth = 2.dp)
                }
                Text(
                    text = actionLabel(action),
                    color = if (enabled) Color.White else t.text3,
                    fontSize = 14.sp,
                    fontWeight = FontWeight.SemiBold,
                )
            }
        }
    }
}
