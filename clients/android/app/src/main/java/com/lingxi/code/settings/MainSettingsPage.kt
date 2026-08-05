package com.lingxi.code.settings

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.navigation.NavHostController
import com.lingxi.code.components.LXIconName
import com.lingxi.code.components.LXToggle
import com.lingxi.code.computeruse.ComputerUseFeatureProvider
import com.lingxi.code.model.Presets
import com.lingxi.code.model.ProviderKind
import com.lingxi.code.theme.Accents
import com.lingxi.code.theme.AppLanguage
import com.lingxi.code.theme.LingXiTheme

/**
 * The main grouped settings list — the Android port of the iOS
 * `MainSettingsPage`: an account card followed by the 智能 / 能力扩展 / 应用 /
 * 隐私与安全 / 关于 sections. Each row navigates the nested
 * [navController] to its detail page; the toggle rows mutate the hoisted store
 * via lambdas (kept inline-simple for the privacy switches, which are local).
 */
@Composable
fun MainSettingsPage(
    state: SettingsUiState,
    isDark: Boolean,
    navController: NavHostController,
    onReplayOnboarding: () -> Unit = {},
) {
    val t = LingXiTheme.palette

    Column(Modifier.fillMaxWidth()) {
        AccountCard(onAccount = { navController.navigate(SettingsRoutes.ACCOUNT) })

        // 智能 ----------------------------------------------------------------
        SettingsSection(label = "智能") {
            val dl = state.llmProviders.firstOrNull { it.isDefault } ?: state.llmProviders.firstOrNull()
            val ds = state.searchProviders.firstOrNull { it.isDefault } ?: state.searchProviders.firstOrNull()
            val df = state.fetchProviders.firstOrNull { it.isDefault } ?: state.fetchProviders.firstOrNull()
            SettingsRow(
                icon = LXIconName.Sparkle, iconColor = Accents.color(forId = "oklch(70% 0.18 268)"),
                label = "LLM 提供商", sub = "默认: ${dl?.name ?: "未配置"}",
                value = "${state.llmProviders.count { it.enabled }} 个启用",
                onTap = { navController.navigate(SettingsRoutes.providerList(ProviderKind.Llm.name)) },
            )
            SettingsRow(
                icon = LXIconName.Search, iconColor = Color(red = 0f, green = 0.7151f, blue = 0.7672f),
                label = "联网搜索", sub = if (ds != null) "默认: ${ds.name}" else "未配置",
                value = "${state.searchProviders.count { it.enabled }} 个启用",
                onTap = { navController.navigate(SettingsRoutes.providerList(ProviderKind.Search.name)) },
            )
            SettingsRow(
                icon = LXIconName.Link, iconColor = Color(red = 0.8713f, green = 0.58f, blue = 0f),
                label = "网页抓取", sub = if (df != null) "默认: ${df.name}" else "未配置",
                value = "${state.fetchProviders.count { it.enabled }} 个启用",
                onTap = { navController.navigate(SettingsRoutes.providerList(ProviderKind.Fetch.name)) },
            )
            SettingsRow(
                icon = LXIconName.Mic, iconColor = Color(red = 0.8018f, green = 0.4038f, blue = 0.8909f),
                label = "语音与音频",
                sub = "系统听写 · ${Presets.voice.firstOrNull { it.id == state.voice.preset }?.name ?: "系统语音"}",
                value = voiceLanguageSummary(state.voice.inputLanguage),
                isLast = true,
                onTap = { navController.navigate(SettingsRoutes.VOICE) },
            )
        }

        // 能力扩展 ------------------------------------------------------------
        SettingsSection(
            label = "能力扩展",
            footer = "Skills 是可复用的 AI 行为包；MCP 是接入外部工具的标准协议；Dream 让灵犀在你休息时主动整理与规划。",
        ) {
            SettingsRow(
                icon = LXIconName.Skill, iconColor = Color(red = 0f, green = 0.7601f, blue = 0.7664f),
                label = "Skills", sub = "技能包 · 提示词 · 操作流",
                value = "${state.skills.count { it.enabled }} / ${state.skills.size} 启用",
                onTap = { navController.navigate(SettingsRoutes.SKILLS) },
            )
            SettingsRow(
                icon = LXIconName.Plug, iconColor = Color(red = 0f, green = 0.78f, blue = 0.55f),
                label = "MCP 服务器", sub = "Model Context Protocol",
                value = "${state.mcpServers.count { it.enabled }} 连接",
                onTap = { navController.navigate(SettingsRoutes.MCP_LIST) },
            )
            SettingsRow(
                icon = LXIconName.Workflow, iconColor = Color(red = 0.3503f, green = 0.6649f, blue = 0.9741f),
                label = "Linux 运行时",
                sub = state.linuxRuntime.summary,
                value = state.linuxRuntime.badge,
                onTap = { navController.navigate(SettingsRoutes.LINUX_RUNTIME) },
            )
            if (ComputerUseFeatureProvider.available) {
                SettingsRow(
                    icon = LXIconName.Sparkle,
                    iconColor = Color(red = 0.62f, green = 0.48f, blue = 0.96f),
                    label = "Computer Use",
                    sub = "Android 屏幕观察与受控操作",
                    value = "Direct",
                    onTap = { navController.navigate(SettingsRoutes.COMPUTER_USE) },
                )
            }
            SettingsRow(
                icon = LXIconName.Dream, iconColor = Color(red = 0.809f, green = 0.4552f, blue = 0.8891f),
                label = "Dream 模式", sub = "后台离线思考与整理",
                value = if (state.dream.enabled) "开启" else "关闭",
                onTap = { navController.navigate(SettingsRoutes.DREAM) },
            )
            SettingsRow(
                icon = LXIconName.Clock, iconColor = Color(red = 0.95f, green = 0.6f, blue = 0.2f),
                label = "定时任务", sub = "按 Cron 计划后台执行",
                isLast = true,
                onTap = { navController.navigate(SettingsRoutes.CRON) },
            )
        }

        // 应用 ----------------------------------------------------------------
        SettingsSection(label = "应用") {
            SettingsRow(
                icon = LXIconName.Sun, iconColor = Color(red = 0.896f, green = 0.6013f, blue = 0f),
                label = "外观", value = if (isDark) "深色" else "浅色",
                onTap = { navController.navigate(SettingsRoutes.APPEARANCE) },
            )
            SettingsRow(
                icon = LXIconName.Message, iconColor = Color(red = 0.3503f, green = 0.6649f, blue = 0.9741f),
                label = "语言", value = AppLanguage.label(state.language),
                onTap = { navController.navigate(SettingsRoutes.LANGUAGE) },
            )
            SettingsRow(
                icon = LXIconName.Cog, iconColor = t.text3,
                label = "通知", value = "${state.notifs.enabledCount} 项开启",
                onTap = { navController.navigate(SettingsRoutes.NOTIFICATIONS) },
            )
            SettingsRow(
                icon = LXIconName.Paperclip, iconColor = Color(red = 0.9351f, green = 0.5079f, blue = 0.4015f),
                label = "键盘与输入", sub = "语音输入 · 候选词", isLast = true,
                onTap = { navController.navigate(SettingsRoutes.INPUT) },
            )
        }

        // 隐私与安全 ----------------------------------------------------------
        SettingsSection(label = "隐私与安全") {
            PrivacyToggleRow(LXIconName.Pin, t.ok, "生物识别锁", state.bioLock) {}
            SettingsRow(
                icon = LXIconName.Brain, iconColor = t.text3,
                label = "数据与隐私", sub = "导出 · 删除 · 透明度报告",
                onTap = { navController.navigate(SettingsRoutes.PRIVACY) },
            )
            PrivacyToggleRow(LXIconName.Sparkle, t.text3, "使用诊断", state.telemetry) {}
            PrivacyToggleRow(LXIconName.Check, t.text3, "自动更新", state.autoUpdate, isLast = true) {}
        }

        // 关于 ----------------------------------------------------------------
        SettingsSection(label = "关于") {
            SettingsRow(icon = LXIconName.Sparkle, label = "灵犀", value = "2.4.1 (build 8721)", chevron = false)
            SettingsRow(icon = LXIconName.Play, label = "重新观看引导", sub = "再过一遍首次设置向导", onTap = onReplayOnboarding)
            SettingsRow(icon = LXIconName.Book, label = "帮助中心", onTap = {})
            SettingsRow(icon = LXIconName.Message, label = "反馈与建议", onTap = {})
            SettingsRow(
                icon = LXIconName.Link,
                label = "开源许可",
                isLast = true,
                onTap = { navController.navigate(SettingsRoutes.OPEN_SOURCE) },
            )
        }

        Text(
            text = "© 2026 灵犀 AI · 用户偏好仅在本地",
            color = t.text4,
            fontSize = 11.sp,
            lineHeight = 16.sp,
            textAlign = TextAlign.Center,
            modifier = Modifier.fillMaxWidth().padding(top = 8.dp, bottom = 4.dp),
        )
    }
}

private fun voiceLanguageSummary(language: String): String = when (language) {
    "zh-CN" -> "中文"
    "en-US" -> "English"
    "ja-JP" -> "日本語"
    else -> "自动"
}

/**
 * The privacy section's toggle rows. The switches are local-only demo state
 * (matching the iOS `@State`); the store binding for these arrives with the
 * full privacy editor — here they reflect the seeded values and flip locally.
 */
@Composable
private fun PrivacyToggleRow(
    icon: LXIconName,
    iconColor: Color,
    label: String,
    seed: Boolean,
    isLast: Boolean = false,
    onChange: (Boolean) -> Unit,
) {
    var on by remember { mutableStateOf(seed) }
    SettingsRow(
        icon = icon, iconColor = iconColor, label = label, chevron = false, isLast = isLast,
        trailing = {
            LXToggle(checked = on, onCheckedChange = { on = it; onChange(it) })
        },
    )
}

@Composable
private fun AccountCard(onAccount: () -> Unit) {
    val t = LingXiTheme.palette
    Row(
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(12.dp),
        modifier = Modifier
            .fillMaxWidth()
            .padding(bottom = 22.dp)
            .clip(RoundedCornerShape(14.dp))
            .background(t.surface)
            .border(0.5.dp, t.border, RoundedCornerShape(14.dp))
            .padding(14.dp),
    ) {
        Box(
            contentAlignment = Alignment.Center,
            modifier = Modifier
                .size(46.dp)
                .clip(CircleShape)
                .background(Brush.linearGradient(listOf(t.accent, t.accent2))),
        ) {
            Text("Y", color = Color.White, fontSize = 17.sp, fontWeight = FontWeight.SemiBold)
        }
        Column(modifier = Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
            Text("Yuxin Yang", color = t.text, fontSize = 15.5f.sp, fontWeight = FontWeight.SemiBold)
            Text("yuxin@axielix.com · Pro", color = t.text4, fontSize = 12.sp)
        }
        Text(
            "账户",
            color = t.text2,
            fontSize = 12.sp,
            fontWeight = FontWeight.Medium,
            modifier = Modifier
                .clip(RoundedCornerShape(8.dp))
                .background(t.windowBg)
                .border(0.5.dp, t.border, RoundedCornerShape(8.dp))
                .clickable(onClick = onAccount)
                .padding(horizontal = 11.dp, vertical = 6.dp),
        )
    }
}
