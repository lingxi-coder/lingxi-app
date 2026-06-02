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
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.lingxi.code.components.LXIcon
import com.lingxi.code.components.LXIconName
import com.lingxi.code.components.LXToggle
import com.lingxi.code.components.mix
import com.lingxi.code.components.tint
import com.lingxi.code.theme.LingXiTheme

/**
 * Skills surface, ported 1:1 from the iOS `SkillsPages.swift`. A list grouped by
 * author (官方 / 我 / 社区 …) — each row shows the skill's description + trigger
 * summary and an enable toggle, tapping pushes a detail page (triggers, required
 * permissions, custom prompt, enable / auto-suggest, delete). A dashed "browse"
 * button and a "create" button sit in the footer.
 *
 * Mock-only: the enable toggle mutates the hoisted [SettingsStore]; browse /
 * create / edit-prompt are decorative seams.
 */

// Sky (official) vs amber (community/me) icon tint — oklch values from iOS.
private val SkillSky = Color(red = 0f, green = 0.7601f, blue = 0.7664f)   // oklch(72% 0.16 195)
private val SkillAmber = Color(red = 0.896f, green = 0.6013f, blue = 0f)  // oklch(75% 0.17 75)

// The order authors appear in (matches the iOS `authorOrder`).
private val AuthorOrder = listOf("官方", "我", "社区 · @arxiv-fan", "社区 · @lin")

@Composable
fun SkillsPage(
    state: SettingsUiState,
    store: SettingsStore,
    onDetail: (id: String) -> Unit,
) {
    val t = LingXiTheme.palette

    Column(Modifier.fillMaxWidth()) {
        Blurb(
            "Skills 是可复用的 AI 行为包，封装了系统提示词、工具调用与触发条件。" +
                "启用后会出现在对应触发器或斜杠菜单。",
        )

        AuthorOrder.forEach { author ->
            val arr = state.skills.filter { it.author == author }
            if (arr.isNotEmpty()) {
                SettingsSection(label = author) {
                    arr.forEachIndexed { i, s ->
                        SettingsRow(
                            icon = LXIconName.Skill,
                            iconColor = if (s.builtin) SkillSky else SkillAmber,
                            label = s.name,
                            sub = s.desc + " · " + s.triggers.joinToString(" / "),
                            chevron = false,
                            isLast = i == arr.size - 1,
                            onTap = { onDetail(s.id) },
                            trailing = {
                                LXToggle(
                                    checked = s.enabled,
                                    onCheckedChange = { store.setSkillEnabled(s.id, it) },
                                )
                            },
                        )
                    }
                }
            }
        }

        DashedAddButton(title = "浏览 Skills 商店", modifier = Modifier.padding(bottom = 8.dp))

        Row(
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(8.dp, Alignment.CenterHorizontally),
            modifier = Modifier
                .fillMaxWidth()
                .clip(RoundedCornerShape(12.dp))
                .background(t.surface)
                .border(0.5.dp, t.border, RoundedCornerShape(12.dp))
                .clickable {}
                .padding(13.dp),
        ) {
            LXIcon(name = LXIconName.Edit, size = 14.dp, color = t.text2, stroke = 1.8f)
            Text("创建自定义 Skill", color = t.text2, fontSize = 13.5f.sp, fontWeight = FontWeight.Medium)
        }
    }
}

/**
 * Skill detail — a brand icon tile + name + author header, the description card,
 * the triggers list (each checked), required permissions, then a control section
 * (enable toggle, auto-suggest, edit-prompt, delete / "officially maintained").
 * If the skill id is unknown the page pops via [onPop].
 */
@Composable
fun SkillDetailPage(
    skillId: String,
    state: SettingsUiState,
    store: SettingsStore,
    onPop: () -> Unit,
) {
    val t = LingXiTheme.palette
    val s = state.skills.firstOrNull { it.id == skillId }
    if (s == null) {
        LaunchedEffect(skillId) { onPop() }
        return
    }

    Column(Modifier.fillMaxWidth()) {
        // Header --------------------------------------------------------------
        Column(
            horizontalAlignment = Alignment.CenterHorizontally,
            modifier = Modifier.fillMaxWidth().padding(top = 8.dp, bottom = 18.dp),
        ) {
            Box(
                contentAlignment = Alignment.Center,
                modifier = Modifier
                    .size(64.dp)
                    .clip(RoundedCornerShape(16.dp))
                    .background(SkillSky.mix(t.surface, 0.20f))
                    .border(0.5.dp, SkillSky.tint(0.35f), RoundedCornerShape(16.dp)),
            ) {
                LXIcon(name = LXIconName.Skill, size = 28.dp, color = SkillSky, stroke = 1.7f)
            }
            Text(
                s.name,
                color = t.text,
                fontSize = 18.sp,
                fontWeight = FontWeight.Bold,
                modifier = Modifier.padding(top = 12.dp),
            )
            Text(
                "${s.author} · v1.2.0",
                color = t.text4,
                fontSize = 12.sp,
                modifier = Modifier.padding(top = 4.dp),
            )
        }

        // Description card ----------------------------------------------------
        Text(
            s.desc,
            color = t.text2,
            fontSize = 13.sp,
            lineHeight = 18.sp,
            modifier = Modifier
                .fillMaxWidth()
                .padding(bottom = 14.dp)
                .clip(RoundedCornerShape(12.dp))
                .background(t.surface)
                .border(0.5.dp, t.border, RoundedCornerShape(12.dp))
                .padding(16.dp),
        )

        // Triggers ------------------------------------------------------------
        SettingsSection(label = "触发") {
            s.triggers.forEachIndexed { i, tr ->
                SettingsRow(
                    label = tr,
                    chevron = false,
                    isLast = i == s.triggers.size - 1,
                    trailing = { LXIcon(name = LXIconName.Check, size = 15.dp, color = t.ok, stroke = 2.4f) },
                )
            }
        }

        // Required permissions ------------------------------------------------
        SettingsSection(label = "所需权限") {
            SettingsRow(label = "读取知识库", chevron = false)
            SettingsRow(label = "调用 LLM", chevron = false)
            SettingsRow(label = "访问 MCP · GitHub", chevron = false, isLast = true)
        }

        // Controls ------------------------------------------------------------
        SettingsSection {
            SettingsRow(
                label = "启用此 Skill",
                chevron = false,
                trailing = {
                    LXToggle(
                        checked = s.enabled,
                        onCheckedChange = { store.setSkillEnabled(s.id, it) },
                    )
                },
            )
            SettingsRow(
                label = "自动建议",
                sub = "检测到匹配场景时主动提示",
                chevron = false,
                trailing = { LocalToggle(seed = true) },
            )
            SettingsRow(label = "编辑提示词", onTap = {})
            SettingsRow(
                icon = LXIconName.X,
                iconColor = if (s.builtin) t.text3 else t.danger,
                label = if (s.builtin) "此 Skill 由官方维护" else "删除此 Skill",
                chevron = false,
                danger = !s.builtin,
                isLast = true,
            )
        }
    }
}
