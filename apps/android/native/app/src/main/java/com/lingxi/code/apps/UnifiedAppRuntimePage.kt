package com.lingxi.code.apps

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.*
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import com.lingxi.code.R
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.unit.dp
import com.lingxi.code.components.UiTags
import com.lingxi.code.components.blockTouchesBehind
import com.lingxi.code.theme.LingXiTheme
import kotlinx.coroutines.launch

/** Trusted run controls only. Application content is displayed by the external browser. */
@Composable
fun UnifiedAppRuntimePage(apps: UnifiedLocalApps, appId: String, onClose: () -> Unit, onShowDetails: (String) -> Unit) {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    val browserReview by apps.browserReviewRequest.collectAsState()
    val appReview = browserReview?.takeIf { it.appId == appId }
    var launch by remember(appId) { mutableStateOf<AppLaunch?>(null) }
    var release by remember(appId) { mutableStateOf<AppRelease?>(null) }
    var error by remember(appId) { mutableStateOf<String?>(null) }
    var busy by remember(appId) { mutableStateOf(false) }
    var needsConsent by remember(appId) { mutableStateOf(false) }
    var needsBrowserConsent by remember(appId) { mutableStateOf(false) }
    var pendingChoose by remember(appId) { mutableStateOf(false) }
    var pendingOpen by remember(appId) { mutableStateOf(false) }
    var opened by rememberSaveable(appId) { mutableStateOf(appReview != null) }
    fun display(value: AppLaunch, chooseHandler: Boolean = false) {
        LocalAppBrowser.open(context, value.url, chooseHandler)
        opened = true
    }
    suspend fun prepare(openBrowser: Boolean, chooseHandler: Boolean = false) {
        if (busy) return
        busy = true; error = null; pendingChoose = chooseHandler; pendingOpen = openBrowser
        try {
            val app = apps.get(appId)
            val current = apps.release(appId, app.currentRevision); release = current
            if (current.executesCode && current.digest !in app.executionGrants) { needsConsent = true; return }
            if (!apps.keepRunning(appId, current.digest)) { needsBrowserConsent = true; return }
            if (openBrowser) check(LocalAppBrowser.isAvailable(context, "http://127.0.0.1/")) {
                "No installed browser can open this application page. Install or enable a browser, then try again."
            }
            val value = apps.openExternalBrowser(appId)
            launch = value; apps.markOpened(appId)
            if (openBrowser) display(value, chooseHandler)
        } catch (failure: Exception) { error = failure.message }
        finally { busy = false }
    }
    LaunchedEffect(appReview) {
        if (appReview != null) {
            opened = true
            // A wake link can arrive on the already-visible native route after
            // Android killed the non-sticky service. Re-establish the user
            // lease only for that explicit handoff; ordinary page tasks do not
            // bounce back into the native app.
            if (!LocalAppProcessService.owns(appId)) prepare(openBrowser = false)
        }
    }
    LaunchedEffect(appId) {
        val reviewPending = apps.browserReviewRequest.value?.appId == appId
        if (reviewPending) opened = true
        prepare(!reviewPending)
    }
    BackHandler(onBack = onClose)
    Column(Modifier.fillMaxSize().background(LingXiTheme.palette.windowBg).blockTouchesBehind().windowInsetsPadding(WindowInsets.systemBars).padding(24.dp).testTag(UiTags.APP_RUNTIME_PAGE), verticalArrangement = Arrangement.spacedBy(16.dp), horizontalAlignment = Alignment.CenterHorizontally) {
        Text("本地应用", style = MaterialTheme.typography.headlineSmall)
        Text("应用页面在外部浏览器打开。模型凭据、Agent 授权和工具审批由灵犀管理。")
        if (busy) CircularProgressIndicator()
        error?.let { Text(it, color = MaterialTheme.colorScheme.error) }
        Button(enabled = !busy, onClick = { scope.launch { prepare(true) } }) { Text(if (launch == null) "启动并在浏览器打开" else "返回浏览器") }
        TextButton(enabled = !busy, onClick = { scope.launch { prepare(true, chooseHandler = true) } }) { Text(stringResource(R.string.remote_apps_choose)) }
        if (appReview != null) Text(stringResource(R.string.local_app_agent_browser_return), style = MaterialTheme.typography.bodySmall)
        Text(stringResource(R.string.local_app_host_recommendation), style = MaterialTheme.typography.bodySmall)
        TextButton(enabled = !busy, onClick = { onShowDetails(appId) }) { Text("应用设置与 Agent 授权") }
        TextButton(enabled = !busy, onClick = { scope.launch { try { apps.stop(appId); onClose() } catch (failure: Exception) { error = failure.message } } }) { Text("停止应用与 Agent 任务") }
        TextButton(onClick = onClose) { Text("关闭管理页面") }
        Text("关闭此管理页面不会停止浏览器中的应用。可通过此处或运行通知停止应用。", style = MaterialTheme.typography.bodySmall)
    }
    if (release?.capabilities?.contains("agent") == true) LocalAppAgentInteractions(apps, appId, appReview?.runId)
    val selected = release
    if (needsBrowserConsent && selected != null) AlertDialog(
        onDismissRequest = { needsBrowserConsent = false; onClose() },
        title = { Text("允许浏览器使用期间运行") },
        text = { Text("灵犀需要在后台保留此应用的本地服务，并显示持续运行通知。可从通知或管理页面停止应用与 Agent 任务。授权仅适用于版本 ${selected.revision}（${selected.digest.take(16)}）；更新内容后需要重新允许。") },
        confirmButton = { TextButton(onClick = { scope.launch {
            try { apps.setKeepRunning(appId, selected.digest, true); needsBrowserConsent = false; prepare(pendingOpen, pendingChoose) }
            catch (failure: Exception) { error = failure.message }
        } }) { Text(if (pendingOpen) "允许并打开浏览器" else "允许") } },
        dismissButton = { TextButton(onClick = { needsBrowserConsent = false; onClose() }) { Text("取消") } },
    )
    if (needsConsent && selected != null) AlertDialog(onDismissRequest = onClose, title = { Text("授权此版本运行") }, text = { Text("此版本将运行应用代码。版本 ${selected.revision}，内容 ${selected.digest.take(16)}。") }, confirmButton = {
        TextButton(onClick = { scope.launch { try { apps.grant(appId, selected.digest); needsConsent = false; prepare(pendingOpen, pendingChoose) } catch (failure: Exception) { error = failure.message } } }) { Text("允许此版本") }
    }, dismissButton = { TextButton(onClick = onClose) { Text("取消") } })
}
