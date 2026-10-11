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
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.lingxi.code.R
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

// The order authors appear in (matches the iOS `authorOrder`). These are raw
// data constants; UI rendering maps them through [authorLabel]. They stay
// Chinese literals on purpose: they're match-keys compared by `==` against
// [com.lingxi.code.model.Skill.author] (itself a match-key for the same
// reason — see SettingsModels.kt/SettingsMock.skills), not display text —
// localizing one side would break the comparison. The i18n extraction pass
// confirmed this is the ONLY consumer of these 4 literals; nothing here is
// an unextracted gap.
private val AuthorOrder = listOf("官方", "我", "社区 · @arxiv-fan", "社区 · @lin")

private const val CommunityAuthorPrefix = "社区 · "

@Composable
private fun authorLabel(author: String): String = when (author) {
    "官方" -> stringResource(R.string.skills_author_official)
    "我" -> stringResource(R.string.skills_author_mine)
    else -> stringResource(
        R.string.skills_author_community_fmt,
        author.removePrefix(CommunityAuthorPrefix),
    )
}

@Composable
fun SkillsPage(
    state: SettingsUiState,
    store: SettingsStore,
    onDetail: (id: String) -> Unit,
) {
    val t = LingXiTheme.palette

    Column(Modifier.fillMaxWidth()) {
        Blurb(stringResource(R.string.skills_description_blurb))

        AuthorOrder.forEach { author ->
            val arr = state.skills.filter { it.author == author }
            if (arr.isNotEmpty()) {
                SettingsSection(label = authorLabel(author)) {
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

        DashedAddButton(title = stringResource(R.string.skills_browse_store), modifier = Modifier.padding(bottom = 8.dp))

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
            Text(stringResource(R.string.skills_create_custom), color = t.text2, fontSize = 13.5f.sp, fontWeight = FontWeight.Medium)
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
                "${authorLabel(s.author)} · v1.2.0",
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
        SettingsSection(label = stringResource(R.string.skills_section_triggers)) {
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
        SettingsSection(label = stringResource(R.string.skills_section_required_permissions)) {
            SettingsRow(label = stringResource(R.string.skills_perm_call_llm), chevron = false)
            SettingsRow(label = stringResource(R.string.skills_perm_access_mcp_github), chevron = false, isLast = true)
        }

        // Controls ------------------------------------------------------------
        SettingsSection {
            SettingsRow(
                label = stringResource(R.string.skills_enable_this),
                chevron = false,
                trailing = {
                    LXToggle(
                        checked = s.enabled,
                        onCheckedChange = { store.setSkillEnabled(s.id, it) },
                    )
                },
            )
            SettingsRow(
                label = stringResource(R.string.skills_auto_suggest),
                sub = stringResource(R.string.skills_auto_suggest_sub),
                chevron = false,
                trailing = { LocalToggle(seed = true) },
            )
            SettingsRow(label = stringResource(R.string.skills_edit_prompt), onTap = {})
            SettingsRow(
                icon = LXIconName.X,
                iconColor = if (s.builtin) t.text3 else t.danger,
                label = if (s.builtin) stringResource(R.string.skills_official_maintained) else stringResource(R.string.skills_delete),
                chevron = false,
                danger = !s.builtin,
                isLast = true,
            )
        }
    }
}
