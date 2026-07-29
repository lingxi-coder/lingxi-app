package com.lingxi.code.settings

import android.Manifest
import android.app.Activity
import android.content.pm.PackageManager
import android.os.Build
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.sizeIn
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Button
import androidx.compose.material3.FilterChip
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateMapOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.core.content.ContextCompat
import com.lingxi.code.computeruse.ComputerUseCaptureMode
import com.lingxi.code.computeruse.ComputerUseFeatureProvider
import com.lingxi.code.computeruse.ComputerUseGrant
import com.lingxi.code.computeruse.ComputerUseSessionState
import com.lingxi.code.computeruse.ComputerUseTier
import com.lingxi.code.theme.LingXiTheme

@Composable
fun ComputerUseSettingsPage() {
    val context = LocalContext.current
    val feature = ComputerUseFeatureProvider
    val state by feature.state.collectAsState()
    val apps = remember { feature.listLaunchableApps(context) }
    val selected = remember {
        mutableStateMapOf<String, ComputerUseTier>().apply {
            state.grants.forEach { put(it.packageName, it.tier) }
        }
    }
    var search by remember { mutableStateOf("") }
    var pendingStart by remember { mutableStateOf<List<ComputerUseGrant>?>(null) }
    val projectionLauncher = rememberLauncherForActivityResult(
        ActivityResultContracts.StartActivityForResult(),
    ) { result ->
        val grants = pendingStart
        pendingStart = null
        if (grants != null && result.resultCode == Activity.RESULT_OK && result.data != null) {
            feature.start(
                context = context,
                grants = grants,
                includeSystemUi = grants.any { it.systemUi },
                projectionResultCode = result.resultCode,
                projectionData = result.data,
            )
        }
    }
    val notificationLauncher = rememberLauncherForActivityResult(
        ActivityResultContracts.RequestPermission(),
    ) { }
    val t = LingXiTheme.palette
    val active = state.sessionState != ComputerUseSessionState.Inactive
    val filteredApps = apps.filter {
        search.isBlank() ||
            it.label.contains(search, ignoreCase = true) ||
            it.packageName.contains(search, ignoreCase = true)
    }

    Column(verticalArrangement = Arrangement.spacedBy(16.dp)) {
        Column(
            verticalArrangement = Arrangement.spacedBy(8.dp),
            modifier = Modifier
                .fillMaxWidth()
                .background(t.surface, RoundedCornerShape(14.dp))
                .padding(16.dp),
        ) {
            Text("Direct 版 Computer Use", color = t.text, fontWeight = FontWeight.SemiBold)
            Text(
                "仅在用户主动启动的会话中控制所选应用；活动会话期间，前台对话和后台定时任务都可使用本次授权。锁屏、服务断开、30 分钟无操作或 2 小时上限都会立即停止。",
                color = t.text3,
                fontSize = 13.sp,
            )
            Text(
                if (state.serviceEnabled) "无障碍服务：已连接" else "无障碍服务：未启用",
                color = if (state.serviceEnabled) t.ok else t.statusTesting,
                fontSize = 13.sp,
                fontWeight = FontWeight.Medium,
            )
            Text(
                "会话：${state.sessionState.label()} · 截图：${state.captureMode.label()}",
                color = t.text2,
                fontSize = 13.sp,
            )
            state.activePackage?.let {
                Text("当前应用：$it", color = t.text3, fontSize = 12.sp)
            }
            state.lastError?.let {
                Text(it, color = t.danger, fontSize = 12.sp)
            }
            TextButton(
                onClick = { feature.openAccessibilitySettings(context) },
                modifier = Modifier.sizeIn(minHeight = 48.dp),
            ) {
                Text(if (state.serviceEnabled) "检查系统服务设置" else "启用无障碍服务")
            }
        }

        if (!active) {
            OutlinedTextField(
                value = search,
                onValueChange = { search = it },
                label = { Text("搜索可启动应用") },
                singleLine = true,
                modifier = Modifier.fillMaxWidth(),
            )
            Text(
                "为每个应用选择本次会话权限",
                color = t.text2,
                fontWeight = FontWeight.SemiBold,
            )
            filteredApps.forEach { app ->
                Column(
                    verticalArrangement = Arrangement.spacedBy(8.dp),
                    modifier = Modifier
                        .fillMaxWidth()
                        .background(t.surface, RoundedCornerShape(12.dp))
                        .clickable {
                            if (selected.containsKey(app.packageName)) {
                                selected.remove(app.packageName)
                            } else {
                                selected[app.packageName] = ComputerUseTier.Read
                            }
                        }
                        .padding(14.dp),
                ) {
                    Row(
                        verticalAlignment = Alignment.CenterVertically,
                        modifier = Modifier.fillMaxWidth(),
                    ) {
                        Column(Modifier.weight(1f)) {
                            Text(app.label, color = t.text, fontWeight = FontWeight.Medium)
                            Text(app.packageName, color = t.text4, fontSize = 11.sp)
                        }
                        Text(
                            if (selected.containsKey(app.packageName)) "已选择" else "未选择",
                            color = if (selected.containsKey(app.packageName)) t.accent else t.text4,
                            fontSize = 12.sp,
                        )
                    }
                    if (selected.containsKey(app.packageName)) {
                        Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                            ComputerUseTier.entries.forEach { tier ->
                                FilterChip(
                                    selected = selected[app.packageName] == tier,
                                    onClick = { selected[app.packageName] = tier },
                                    label = { Text(tier.label()) },
                                )
                            }
                        }
                    }
                }
            }
        }

        if (active) {
            Button(
                onClick = { feature.stop(context) },
                modifier = Modifier.fillMaxWidth().sizeIn(minHeight = 52.dp),
            ) {
                Text("立即停止 Computer Use")
            }
        } else {
            Button(
                enabled = selected.isNotEmpty() && state.serviceEnabled,
                onClick = {
                    if (
                        Build.VERSION.SDK_INT >= 33 &&
                        ContextCompat.checkSelfPermission(
                            context,
                            Manifest.permission.POST_NOTIFICATIONS,
                        ) != PackageManager.PERMISSION_GRANTED
                    ) {
                        notificationLauncher.launch(Manifest.permission.POST_NOTIFICATIONS)
                        return@Button
                    }
                    val grants = apps.mapNotNull { app ->
                        selected[app.packageName]?.let { tier ->
                            ComputerUseGrant(
                                packageName = app.packageName,
                                label = app.label,
                                tier = tier,
                                systemUi = app.systemUi,
                            )
                        }
                    }
                    val projection = feature.mediaProjectionRequest(context)
                    if (projection != null) {
                        pendingStart = grants
                        projectionLauncher.launch(projection)
                    } else {
                        feature.start(
                            context,
                            grants,
                            includeSystemUi = grants.any { it.systemUi },
                        )
                    }
                },
                modifier = Modifier.fillMaxWidth().sizeIn(minHeight = 52.dp),
            ) {
                Text("启动控制会话")
            }
        }

        Text(
            "禁止控制密码、OTP、生物识别、支付/转账确认、系统授权、设备管理、VPN、安装/卸载和 FLAG_SECURE 窗口。高风险提交、发送、拨号和删除动作每次都需要确认。",
            color = t.text4,
            fontSize = 12.sp,
            modifier = Modifier.padding(bottom = 8.dp),
        )
        TextButton(
            onClick = { feature.clearAudit(context) },
            modifier = Modifier.sizeIn(minHeight = 48.dp),
        ) {
            Text("清除本地 Computer Use 审计记录")
        }
    }
}

private fun ComputerUseTier.label(): String = when (this) {
    ComputerUseTier.Read -> "只读"
    ComputerUseTier.Click -> "点击"
    ComputerUseTier.Full -> "完整"
}

private fun ComputerUseSessionState.label(): String = when (this) {
    ComputerUseSessionState.Inactive -> "未启动"
    ComputerUseSessionState.Starting -> "启动中"
    ComputerUseSessionState.Active -> "活动"
    ComputerUseSessionState.AwaitingApproval -> "等待确认"
    ComputerUseSessionState.Stopping -> "停止中"
}

private fun ComputerUseCaptureMode.label(): String = when (this) {
    ComputerUseCaptureMode.None -> "不可用"
    ComputerUseCaptureMode.Accessibility -> "Accessibility"
    ComputerUseCaptureMode.MediaProjection -> "MediaProjection"
}
