package com.lingxi.code.settings

import androidx.compose.animation.core.RepeatMode
import androidx.compose.animation.core.animateFloat
import androidx.compose.animation.core.infiniteRepeatable
import androidx.compose.animation.core.rememberInfiniteTransition
import androidx.compose.animation.core.tween
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.draw.clip
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.lingxi.code.components.LXIcon
import com.lingxi.code.components.LXIconName
import com.lingxi.code.components.LXToggle
import com.lingxi.code.theme.LingXiTheme

/**
 * Dream mode, ported 1:1 from the iOS `DreamPage.swift`.
 *
 * A gradient-orb header (pulsing radial fill behind a moon glyph), the master
 * enable row (sub = last-run summary), a time-window radio (night / always /
 * custom), run-condition toggles (charging / Wi-Fi), the 5 stackable activities,
 * a compute-budget radio (low / medium / high), and a "last-night review"
 * timeline. All bound to the hoisted [SettingsStore]'s [com.lingxi.code.model.DreamConfig].
 */

private val DreamRose = Color(red = 0.809f, green = 0.4552f, blue = 0.8891f)  // oklch(70% 0.18 320)
private val DreamIndigo = Color(red = 0.1289f, green = 0.214f, blue = 0.6526f) // gradient inner-glow target

/** A Dream background activity (stackable). */
private data class DreamActivity(val key: String, val label: String, val sub: String)

private val DreamActivities = listOf(
    DreamActivity("reorganize", "整理记忆", "合并相似 / 去重 / 归档过期"),
    DreamActivity("plan", "草拟今日计划", "基于昨日未完成 + 日历"),
    DreamActivity("recap", "总结过去一周", "每周日凌晨生成回顾报告"),
    DreamActivity("prefetch", "预热常用上下文", "预生成你最常问的回答"),
    DreamActivity("polish", "润色草稿", "为待发邮件/文档生成 2 套候选"),
)

@Composable
fun DreamPage(
    state: SettingsUiState,
    store: SettingsStore,
) {
    val t = LingXiTheme.palette
    val dream = state.dream

    Column(Modifier.fillMaxWidth()) {
        // Gradient orb header -------------------------------------------------
        Column(
            horizontalAlignment = Alignment.CenterHorizontally,
            modifier = Modifier.fillMaxWidth().padding(top = 12.dp, bottom = 22.dp),
        ) {
            DreamOrb()
            Text(
                "Dream 模式",
                color = t.text,
                fontSize = 19.sp,
                fontWeight = FontWeight.Bold,
                modifier = Modifier.padding(top = 14.dp),
            )
            Text(
                "当设备闲置时，灵犀在后台运行\n整理、规划、润色 — 醒来即可看到结果",
                color = t.text3,
                fontSize = 12.5f.sp,
                lineHeight = 17.sp,
                textAlign = TextAlign.Center,
                modifier = Modifier.fillMaxWidth().padding(top = 6.dp, start = 16.dp, end = 16.dp),
            )
        }

        // Master enable -------------------------------------------------------
        SettingsSection {
            SettingsRow(
                label = "开启 Dream 模式",
                sub = dream.lastRun,
                chevron = false,
                isLast = true,
                trailing = {
                    LXToggle(
                        checked = dream.enabled,
                        onCheckedChange = { store.setDream(dream.copy(enabled = it)) },
                    )
                },
            )
        }

        // Time window ---------------------------------------------------------
        SectionWithRadio(
            label = "时间窗",
            options = listOf(
                RadioOption("night", "夜间 (00:00–06:00)", "默认，最不打扰"),
                RadioOption("always", "随时", "只要设备闲置"),
                RadioOption("custom", "自定义时段", "设置每日时间窗"),
            ),
            selected = dream.window,
            onSelect = { store.setDream(dream.copy(window = it)) },
        )

        // Run conditions ------------------------------------------------------
        SettingsSection(label = "运行条件", footer = "确保不会影响日常使用：仅在充电 + Wi-Fi 时跑昂贵任务。") {
            SettingsRow(
                label = "仅在充电时",
                chevron = false,
                trailing = {
                    LXToggle(
                        checked = dream.onCharging,
                        onCheckedChange = { store.setDream(dream.copy(onCharging = it)) },
                    )
                },
            )
            SettingsRow(
                label = "仅在 Wi-Fi 时",
                chevron = false,
                isLast = true,
                trailing = {
                    LXToggle(
                        checked = dream.onWifi,
                        onCheckedChange = { store.setDream(dream.copy(onWifi = it)) },
                    )
                },
            )
        }

        // Allowed activities (stackable) --------------------------------------
        SettingsSection(label = "允许的活动", footer = "可叠加，越多越费电与算力。") {
            DreamActivities.forEachIndexed { i, a ->
                val on = dream.activities[a.key] ?: false
                SettingsRow(
                    label = a.label,
                    sub = a.sub,
                    chevron = false,
                    isLast = i == DreamActivities.size - 1,
                    trailing = {
                        LXToggle(
                            checked = on,
                            onCheckedChange = { v ->
                                store.setDream(dream.copy(activities = dream.activities + (a.key to v)))
                            },
                        )
                    },
                )
            }
        }

        // Compute budget ------------------------------------------------------
        SectionWithRadio(
            label = "算力预算",
            footer = "高预算会优先调用更强模型（如 Claude Opus），并访问 MCP 工具。低预算只用本地 + 最便宜模型。",
            options = listOf(
                RadioOption("low", "低", "本地模型为主 · 几乎免费"),
                RadioOption("medium", "中", "中等模型 · 默认"),
                RadioOption("high", "高", "推理模型 + 工具 · 最深入"),
            ),
            selected = dream.budget,
            onSelect = { store.setDream(dream.copy(budget = it)) },
        )

        // Last-night review ---------------------------------------------------
        SettingsSection(label = "昨夜 Dream 回顾") {
            SettingsRow(label = "03:24 - 03:41 · 整理记忆", sub = "合并 12 条 → 7 条，归档 5 条过期", onTap = {})
            SettingsRow(label = "03:41 - 04:02 · 草拟今日计划", sub = "基于 8 个未完成事项 + 3 场会议", onTap = {})
            SettingsRow(label = "04:02 - 04:08 · 润色邮件", sub = "为 2 封草稿各生成 1 套候选", isLast = true, onTap = {})
        }
    }
}

/** The pulsing gradient orb: a radial rose→indigo fill behind a moon glyph. */
@Composable
private fun DreamOrb() {
    val t = LingXiTheme.palette
    val transition = rememberInfiniteTransition(label = "dreamOrb")
    val pulse by transition.animateFloat(
        initialValue = 0.45f,
        targetValue = 0.8f,
        animationSpec = infiniteRepeatable(tween(3000), RepeatMode.Reverse),
        label = "orbPulse",
    )
    Box(contentAlignment = Alignment.Center, modifier = Modifier.size(76.dp)) {
        Box(
            modifier = Modifier
                .size(76.dp)
                .alpha(pulse)
                .clip(CircleShape)
                .background(
                    Brush.radialGradient(
                        colors = listOf(DreamRose, DreamIndigo),
                        center = Offset(0.3f * 76f, 0.3f * 76f),
                        radius = 70f,
                    ),
                ),
        )
        Box(
            contentAlignment = Alignment.Center,
            modifier = Modifier.size(64.dp).clip(CircleShape).background(t.windowBg),
        ) {
            LXIcon(name = LXIconName.Dream, size = 32.dp, color = DreamRose, stroke = 1.6f)
        }
    }
}

/** A [SettingsSection]-styled label/footer wrapping a [RadioList]. */
@Composable
private fun SectionWithRadio(
    label: String,
    options: List<RadioOption>,
    selected: String,
    onSelect: (String) -> Unit,
    footer: String? = null,
) {
    val t = LingXiTheme.palette
    Column(Modifier.fillMaxWidth().padding(bottom = 22.dp)) {
        Text(
            text = label.uppercase(),
            color = t.text4,
            fontSize = 11.sp,
            fontWeight = FontWeight.SemiBold,
            letterSpacing = 0.6.sp,
            modifier = Modifier.padding(horizontal = 4.dp).padding(bottom = 8.dp),
        )
        RadioList(options = options, selected = selected, onSelect = onSelect)
        if (footer != null) {
            Text(
                text = footer,
                color = t.text4,
                fontSize = 11.sp,
                lineHeight = 16.sp,
                modifier = Modifier.padding(horizontal = 4.dp).padding(top = 8.dp),
            )
        }
    }
}
