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
import androidx.compose.foundation.layout.sizeIn
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Slider
import androidx.compose.material3.SliderDefaults
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
import com.lingxi.code.model.Presets
import com.lingxi.code.model.VoiceConfig
import com.lingxi.code.theme.LingXiTheme

/**
 * The "simple" settings pages owned by A6 — account, notifications, input,
 * privacy, and language. Each is a thin composition of [SettingsSection] /
 * [SettingsRow] / [RadioList], ported from the iOS `SimplePages.swift`.
 * Toggles that the iOS code held as local `@State` stay local here too; the
 * language radio + notifications bind to the hoisted store so the main-list
 * summary stays in sync.
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
    var smartSugg by remember { mutableStateOf(true) }
    var fromHistory by remember { mutableStateOf(true) }
    Column(Modifier.fillMaxWidth()) {
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

// MARK: - Voice input and output --------------------------------------------
/**
 * Shared voice settings for chat and Direct-build Computer Use. Changes are
 * persisted by [SettingsStore] and read by the Android Computer Use audio host
 * at call time, so an Agent listen/speak call uses the same language, voice and
 * speed selected here.
 *
 * @param voice the live voice config (preset / speed / auto-play).
 * @param onChange writes a mutated config back into the store.
 */
@Composable
fun VoicePage(voice: VoiceConfig, onChange: (VoiceConfig) -> Unit) {
    val t = LingXiTheme.palette

    Column(Modifier.fillMaxWidth()) {
        SettingsSection(
            label = "听 · 语音识别",
            footer = "聊天麦克风和 Computer Use 的 listen 动作共用此设置。麦克风权限只可由用户在前台授予。",
        ) {
            RadioList(
                options = listOf(
                    RadioOption("system", "Android 系统语音识别", "使用设备当前的识别服务"),
                ),
                selected = voice.inputProvider,
                onSelect = { onChange(voice.copy(inputProvider = it)) },
            )
            Column(
                verticalArrangement = Arrangement.spacedBy(8.dp),
                modifier = Modifier.fillMaxWidth().padding(horizontal = 14.dp, vertical = 12.dp),
            ) {
                Row(
                    verticalAlignment = Alignment.CenterVertically,
                    modifier = Modifier.fillMaxWidth(),
                ) {
                    Text(
                        "识别语言",
                        color = t.text,
                        fontSize = 14.sp,
                        fontWeight = FontWeight.Medium,
                        modifier = Modifier.weight(1f),
                    )
                    Text(
                        voiceLanguageLabel(voice.inputLanguage),
                        color = t.text3,
                        fontSize = 13.sp,
                    )
                }
                Row(
                    horizontalArrangement = Arrangement.spacedBy(6.dp),
                    modifier = Modifier.fillMaxWidth(),
                ) {
                    listOf("auto", "zh-CN", "en-US", "ja-JP").forEach { language ->
                        Text(
                            text = voiceLanguageShortLabel(language),
                            color = if (voice.inputLanguage == language) t.accent else t.text3,
                            fontSize = 12.sp,
                            modifier = Modifier
                                .clip(CircleShape)
                                .background(
                                    if (voice.inputLanguage == language) {
                                        t.accent.tint(0.14f)
                                    } else {
                                        t.surfaceActive
                                    },
                                )
                                .clickable { onChange(voice.copy(inputLanguage = language)) }
                                .weight(1f)
                                .sizeIn(minHeight = 48.dp)
                                .padding(horizontal = 6.dp, vertical = 7.dp),
                        )
                    }
                }
            }
        }

        SettingsSection(
            label = "说 · 语音合成",
            footer = "AI 回复自动播放和 Computer Use 的 speak 动作共用输出声音与语速。",
        ) {
            RadioList(
                options = Presets.voice
                    .filter { it.id == "system" }
                    .map { RadioOption(it.id, it.name, it.sub) },
                selected = voice.preset,
                onSelect = { onChange(voice.copy(preset = it)) },
            )
            if (voice.preset == "system") {
                Box(Modifier.fillMaxWidth().padding(horizontal = 14.dp, vertical = 12.dp)) {
                    SettingsField(
                        value = voice.voiceId,
                        onValueChange = { onChange(voice.copy(voiceId = it)) },
                        placeholder = "系统 voice id（default）",
                    )
                }
            }
        }

        SettingsSection(label = "播放选项", footer = "自动播放：AI 回复完成后立即朗读。") {
            SettingsRow(
                label = "语速",
                value = String.format("%.1fx", voice.speed),
                chevron = false,
            ) {
                Slider(
                    value = voice.speed,
                    onValueChange = { v ->
                        // Snap to 0.1 steps in 0.5..2.0 (matches the iOS Slider step).
                        onChange(voice.copy(speed = snapVoiceSpeed(v)))
                    },
                    valueRange = 0.5f..2.0f,
                    steps = 14, // 0.5..2.0 by 0.1 → 16 stops → 14 interior steps
                    colors = SliderDefaults.colors(
                        thumbColor = t.accent,
                        activeTrackColor = t.accent,
                        inactiveTrackColor = t.surfaceActive,
                    ),
                    modifier = Modifier.width(120.dp),
                )
            }
            SettingsRow(label = "自动播放回复", chevron = false, isLast = true) {
                LXToggle(
                    checked = voice.autoPlay,
                    onCheckedChange = { onChange(voice.copy(autoPlay = it)) },
                )
            }
        }
    }
}

/**
 * Snap a raw slider value to the voice-rate grid: 0.1 increments clamped to the
 * 0.5x..2.0x range (the iOS `Slider(in: 0.5...2, step: 0.1)`). PURE so the
 * snapping is unit-testable on the plain JVM.
 */
fun snapVoiceSpeed(raw: Float): Float =
    (Math.round(raw * 10f) / 10f).coerceIn(0.5f, 2.0f)

private fun voiceLanguageLabel(language: String): String = when (language) {
    "zh-CN" -> "简体中文"
    "en-US" -> "English"
    "ja-JP" -> "日本語"
    else -> "自动"
}

private fun voiceLanguageShortLabel(language: String): String = when (language) {
    "zh-CN" -> "中文"
    "en-US" -> "EN"
    "ja-JP" -> "日本語"
    else -> "自动"
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
