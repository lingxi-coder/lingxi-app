package com.lingxi.code.apps

import android.content.Intent
import android.net.Uri
import androidx.activity.compose.BackHandler
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.*
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
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
    var launch by remember(appId) { mutableStateOf<AppLaunch?>(null) }
    var release by remember(appId) { mutableStateOf<AppRelease?>(null) }
    var error by remember(appId) { mutableStateOf<String?>(null) }
    var busy by remember(appId) { mutableStateOf(false) }
    var needsConsent by remember(appId) { mutableStateOf(false) }
    var opened by rememberSaveable(appId) { mutableStateOf(apps.consumeBrowserReview(appId)) }
    fun display(value: AppLaunch) {
        // ACTION_VIEW delegates the page to the user's external browser.
        context.startActivity(Intent(Intent.ACTION_VIEW, Uri.parse(value.url)).addCategory(Intent.CATEGORY_BROWSABLE))
        opened = true
    }
    suspend fun prepare(openBrowser: Boolean) {
        if (busy) return
        busy = true; error = null
        try {
            val app = apps.get(appId)
            val current = apps.release(appId, app.currentRevision); release = current
            if (current.executesCode && current.digest !in app.executionGrants) { needsConsent = true; return }
            val value = apps.openExternalBrowser(appId)
            launch = value; apps.markOpened(appId)
            if (openBrowser) display(value)
        } catch (failure: Exception) { error = failure.message }
        finally { busy = false }
    }
    LaunchedEffect(appId) { prepare(!opened) }
    BackHandler(onBack = onClose)
    Column(Modifier.fillMaxSize().background(LingXiTheme.palette.windowBg).blockTouchesBehind().windowInsetsPadding(WindowInsets.systemBars).padding(24.dp).testTag(UiTags.APP_RUNTIME_PAGE), verticalArrangement = Arrangement.spacedBy(16.dp), horizontalAlignment = Alignment.CenterHorizontally) {
        Text("本地应用", style = MaterialTheme.typography.headlineSmall)
        Text("应用页面在外部浏览器打开。模型凭据、Agent 授权和工具审批由灵犀管理。")
        if (busy) CircularProgressIndicator()
        error?.let { Text(it, color = MaterialTheme.colorScheme.error) }
        Button(enabled = !busy, onClick = { scope.launch { prepare(true) } }) { Text(if (launch == null) "启动并在浏览器打开" else "返回浏览器") }
        TextButton(enabled = !busy, onClick = { onShowDetails(appId) }) { Text("应用设置与 Agent 授权") }
        TextButton(enabled = !busy, onClick = { scope.launch { try { apps.stop(appId); onClose() } catch (failure: Exception) { error = failure.message } } }) { Text("停止应用与 Agent 任务") }
        TextButton(onClick = onClose) { Text("关闭管理页面") }
        Text("关闭此管理页面不会停止浏览器中的应用。可通过此处或运行通知停止应用。", style = MaterialTheme.typography.bodySmall)
    }
    if (release?.capabilities?.contains("agent") == true) LocalAppAgentInteractions(apps, appId)
    val selected = release
    if (needsConsent && selected != null) AlertDialog(onDismissRequest = onClose, title = { Text("授权此版本运行") }, text = { Text("此版本将运行应用代码。版本 ${selected.revision}，内容 ${selected.digest.take(16)}。") }, confirmButton = {
        TextButton(onClick = { scope.launch { try { apps.grant(appId, selected.digest); needsConsent = false; prepare(true) } catch (failure: Exception) { error = failure.message } } }) { Text("允许此版本") }
    }, dismissButton = { TextButton(onClick = onClose) { Text("取消") } })
}
