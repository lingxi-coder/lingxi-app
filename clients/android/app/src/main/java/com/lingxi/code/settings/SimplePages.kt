package com.lingxi.code.settings

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
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
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.lingxi.code.components.LXIconName
import com.lingxi.code.components.LXToggle
import com.lingxi.code.components.tint
import com.lingxi.code.model.NotifConfig
import com.lingxi.code.theme.LingXiTheme

/**
 * The "simple" settings pages owned by A6 — account, knowledge, memory,
 * workflows, notifications, input, privacy, language. Each is a thin
 * composition of [SettingsSection] / [SettingsRow] / [RadioList], ported 1:1
 * from the iOS `SimplePages.swift`. Toggles that the iOS code held as local
 * `@State` stay local here too; the language radio + notifications bind to the
 * hoisted store so the main-list summary stays in sync.
 */

// MARK: - Account -----------------------------------------------------------
@Composable
fun AccountPage() {
    val t = LingXiTheme.palette
    Column(Modifier.fillMaxWidth()) {
        Column(
            modifier = Modifier.fillMaxWidth().padding(top = 8.dp, bottom = 18.dp),
            horizontalAlignment = Alignment.CenterHorizontally,
        ) {
            Box(
                contentAlignment = Alignment.Center,
                modifier = Modifier
                    .size(76.dp)
                    .clip(CircleShape)
                    .background(Brush.linearGradient(listOf(t.accent, t.accent2))),
            ) {
                Text("Y", color = Color.White, fontSize = 30.sp, fontWeight = FontWeight.SemiBold)
            }
            Text("Yuxin Yang", color = t.text, fontSize = 18.sp, fontWeight = FontWeight.Bold, modifier = Modifier.padding(top = 12.dp))
            Text("yuxin@axielix.com", color = t.text4, fontSize = 13.sp, modifier = Modifier.padding(top = 4.dp))
            Text(
                "Pro · 续费日 2026-09-30",
                color = t.accent,
                fontSize = 11.5f.sp,
                fontWeight = FontWeight.SemiBold,
                modifier = Modifier
                    .padding(top = 10.dp)
                    .clip(CircleShape)
                    .background(t.accent.tint(0.18f))
                    .padding(horizontal = 12.dp, vertical = 4.dp),
            )
        }

        SettingsSection(label = "本月用量") {
            SettingsRow(label = "对话次数", value = "247 / 1000", chevron = false)
            SettingsRow(label = "推理时长", value = "5.5 / 8 小时", chevron = false)
            SettingsRow(label = "存储", value = "1.2 / 10 GB", chevron = false, isLast = true)
        }
        SettingsSection {
            SettingsRow(icon = LXIconName.Brain, label = "管理订阅", onTap = {})
            SettingsRow(icon = LXIconName.Link, label = "同步设备", sub = "3 台设备已连接", onTap = {})
            SettingsRow(icon = LXIconName.X, label = "退出登录", danger = true, isLast = true, onTap = {})
        }
    }
}

// MARK: - Knowledge ---------------------------------------------------------
@Composable
fun KnowledgePage() {
    var autoRecall by remember { mutableStateOf(true) }
    Column(Modifier.fillMaxWidth()) {
        Blurb("知识库内容会被嵌入并附加到 AI 上下文。所有索引在本机完成。")
        SettingsSection(label = "存储") {
            SettingsRow(label = "使用空间", value = "142 MB", chevron = false)
            SettingsRow(label = "文件数", value = "24 个", chevron = false)
            SettingsRow(label = "索引模型", value = "bge-m3-local", isLast = true, onTap = {})
        }
        SettingsSection(label = "行为") {
            SettingsRow(label = "自动检索", sub = "每次提问自动召回相关片段", chevron = false) {
                LXToggle(checked = autoRecall, onCheckedChange = { autoRecall = it })
            }
            SettingsRow(label = "召回数量上限", value = "8 段", isLast = true, onTap = {})
        }
    }
}

// MARK: - Memory ------------------------------------------------------------
@Composable
fun MemoryPage() {
    Column(Modifier.fillMaxWidth()) {
        Blurb("灵犀根据对话自动提取关于你的偏好、习惯、关系。你可以随时编辑或删除。")
        SettingsSection(label = "近期记忆") {
            SettingsRow(label = "偏好深色 + 中文", sub = "2026-05-12 形成", onTap = {})
            SettingsRow(label = "工作日 8:30 倾向收到晨报", sub = "2026-05-10 形成", onTap = {})
            SettingsRow(label = "正在做 AxieLix 灵犀项目", sub = "2026-05-08 形成", isLast = true, onTap = {})
        }
        SettingsSection {
            SettingsRow(icon = LXIconName.X, label = "清除全部记忆", chevron = false, danger = true, isLast = true, onTap = {})
        }
    }
}

// MARK: - Workflows ---------------------------------------------------------
@Composable
fun WorkflowsPage() {
    var w1 by remember { mutableStateOf(true) }
    var w2 by remember { mutableStateOf(true) }
    var w3 by remember { mutableStateOf(true) }
    var w4 by remember { mutableStateOf(false) }
    SettingsSection(
        label = "自动化",
        footer = "工作流由 cron 表达式或事件触发。在主界面侧栏 → 定时 可创建。",
    ) {
        SettingsRow(label = "每日晨报", sub = "工作日 08:30", chevron = false) { LXToggle(checked = w1, onCheckedChange = { w1 = it }) }
        SettingsRow(label = "周报自动生成", sub = "每周五 17:00", chevron = false) { LXToggle(checked = w2, onCheckedChange = { w2 = it }) }
        SettingsRow(label = "客户反馈周聚合", sub = "每周一 09:00", chevron = false) { LXToggle(checked = w3, onCheckedChange = { w3 = it }) }
        SettingsRow(label = "凌晨日志巡检", sub = "已暂停", chevron = false, isLast = true) { LXToggle(checked = w4, onCheckedChange = { w4 = it }) }
    }
}

// MARK: - Notifications -----------------------------------------------------
@Composable
fun NotificationsPage(notifs: NotifConfig, onChange: (NotifConfig) -> Unit) {
    SettingsSection(
        label = "通知类型",
        footer = "所有通知通过系统通知中心，灵犀不会单独打扰你。",
    ) {
        SettingsRow(label = "工作流完成", sub = "AI 跑完多步任务时", chevron = false) {
            LXToggle(checked = notifs.workflows, onCheckedChange = { onChange(notifs.copy(workflows = it)) })
        }
        SettingsRow(label = "我被 @", sub = "会话内有人提到你", chevron = false) {
            LXToggle(checked = notifs.mentions, onCheckedChange = { onChange(notifs.copy(mentions = it)) })
        }
        SettingsRow(label = "定时任务报告", sub = "cron 触发执行后", chevron = false) {
            LXToggle(checked = notifs.crons, onCheckedChange = { onChange(notifs.copy(crons = it)) })
        }
        SettingsRow(label = "产品更新", sub = "新功能与重要变更", chevron = false, isLast = true) {
            LXToggle(checked = notifs.marketing, onCheckedChange = { onChange(notifs.copy(marketing = it)) })
        }
    }
}

// MARK: - Input -------------------------------------------------------------
@Composable
fun InputPage() {
    var autoSend by remember { mutableStateOf(true) }
    var smartSugg by remember { mutableStateOf(true) }
    var fromHistory by remember { mutableStateOf(true) }
    Column(Modifier.fillMaxWidth()) {
        SettingsSection(label = "语音输入") {
            SettingsRow(label = "按住说话识别语言", value = "自动", onTap = {})
            SettingsRow(label = "松开后自动发送", chevron = false, isLast = true) { LXToggle(checked = autoSend, onCheckedChange = { autoSend = it }) }
        }
        SettingsSection(label = "候选词") {
            SettingsRow(label = "启用智能候选", chevron = false) { LXToggle(checked = smartSugg, onCheckedChange = { smartSugg = it }) }
            SettingsRow(label = "基于历史会话", chevron = false, isLast = true) { LXToggle(checked = fromHistory, onCheckedChange = { fromHistory = it }) }
        }
    }
}

// MARK: - Privacy -----------------------------------------------------------
@Composable
fun PrivacyPage() {
    var contribute by remember { mutableStateOf(false) }
    var crash by remember { mutableStateOf(true) }
    Column(Modifier.fillMaxWidth()) {
        SettingsSection(label = "数据") {
            SettingsRow(icon = LXIconName.Brain, label = "导出我的所有数据", onTap = {})
            SettingsRow(icon = LXIconName.X, label = "删除账号与数据", danger = true, isLast = true, onTap = {})
        }
        SettingsSection(
            label = "可见性",
            footer = "灵犀对你的承诺：密钥永远不离开本机；对话默认不被用于训练。",
        ) {
            SettingsRow(label = "使用数据贡献训练", chevron = false) { LXToggle(checked = contribute, onCheckedChange = { contribute = it }) }
            SettingsRow(label = "崩溃报告", chevron = false, isLast = true) { LXToggle(checked = crash, onCheckedChange = { crash = it }) }
        }
    }
}

// MARK: - Language ----------------------------------------------------------
@Composable
fun LanguagePage(language: String, onSelect: (String) -> Unit) {
    var follow by remember { mutableStateOf(true) }
    Column(Modifier.fillMaxWidth()) {
        SettingsSection(
            label = "语言",
            footer = "切换语言后将重新加载界面。AI 对话语言独立配置。",
        ) {
            RadioList(
                options = listOf(
                    RadioOption("zh-CN", "简体中文"),
                    RadioOption("zh-TW", "繁體中文"),
                    RadioOption("en-US", "English (US)"),
                    RadioOption("ja-JP", "日本語"),
                ),
                selected = language,
                onSelect = onSelect,
            )
        }
        SettingsSection(label = "区域") {
            SettingsRow(label = "日期格式", value = "2026/5/14", isLast = true, onTap = {})
        }
        SettingsSection(label = "AI 回复语言") {
            SettingsRow(label = "跟随界面", sub = "灵犀根据你的输入语言自动判断", chevron = false, isLast = true) {
                LXToggle(checked = follow, onCheckedChange = { follow = it })
            }
        }
    }
}

// MARK: - Placeholder (A7/A8 seam) ------------------------------------------
/**
 * A clean placeholder for the 智能 / 能力扩展 surfaces that A7/A8 will build
 * (providers, voice, skills, MCP, Dream). Renders a centered card so the
 * navigation, top bar and back behavior are fully exercisable today without
 * faking those editors.
 */
@Composable
fun PlaceholderPage(title: String, note: String) {
    val t = LingXiTheme.palette
    Column(
        modifier = Modifier.fillMaxWidth(),
        horizontalAlignment = Alignment.CenterHorizontally,
        verticalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        Box(
            modifier = Modifier
                .fillMaxWidth()
                .clip(RoundedCornerShape(12.dp))
                .background(t.surface)
                .border(0.5.dp, t.border, RoundedCornerShape(12.dp))
                .padding(20.dp),
        ) {
            Column(verticalArrangement = Arrangement.spacedBy(6.dp)) {
                Text(title, color = t.text, fontSize = 15.sp, fontWeight = FontWeight.SemiBold)
                Text(note, color = t.text3, fontSize = 12.5f.sp, lineHeight = 18.sp)
            }
        }
    }
}
