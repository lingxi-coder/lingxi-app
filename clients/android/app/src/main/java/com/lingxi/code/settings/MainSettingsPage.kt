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
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.navigation.NavHostController
import com.lingxi.code.R
import com.lingxi.code.components.LXIconName
import com.lingxi.code.components.LXToggle
import com.lingxi.code.computeruse.ComputerUseFeatureProvider
import com.lingxi.code.model.Presets
import com.lingxi.code.model.ProviderKind
import com.lingxi.code.theme.Accents
import com.lingxi.code.theme.AppLanguage
import com.lingxi.code.theme.AppLanguageStore
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
    val context = LocalContext.current
    val languageStore = remember { AppLanguageStore(context.applicationContext) }
    val currentLanguage by languageStore.language.collectAsState()

    Column(Modifier.fillMaxWidth()) {
        AccountCard(onAccount = { navController.navigate(SettingsRoutes.ACCOUNT) })

        // 智能 ----------------------------------------------------------------
        SettingsSection(label = stringResource(R.string.settings_section_intelligence)) {
            val dl = state.llmProviders.firstOrNull { it.isDefault } ?: state.llmProviders.firstOrNull()
            val ds = state.searchProviders.firstOrNull { it.isDefault } ?: state.searchProviders.firstOrNull()
            val df = state.fetchProviders.firstOrNull { it.isDefault } ?: state.fetchProviders.firstOrNull()
            SettingsRow(
                icon = LXIconName.Sparkle, iconColor = Accents.color(forId = "oklch(70% 0.18 268)"),
                label = stringResource(R.string.settings_llm_providers),
                sub = stringResource(R.string.settings_provider_default_fmt, dl?.name ?: stringResource(R.string.settings_provider_unconfigured)),
                value = stringResource(R.string.settings_providers_enabled_fmt, state.llmProviders.count { it.enabled }),
                onTap = { navController.navigate(SettingsRoutes.providerList(ProviderKind.Llm.name)) },
            )
            SettingsRow(
                icon = LXIconName.Search, iconColor = Color(red = 0f, green = 0.7151f, blue = 0.7672f),
                label = stringResource(R.string.settings_web_search),
                sub = if (ds != null) stringResource(R.string.settings_provider_default_fmt, ds.name) else stringResource(R.string.settings_provider_unconfigured),
                value = stringResource(R.string.settings_providers_enabled_fmt, state.searchProviders.count { it.enabled }),
                onTap = { navController.navigate(SettingsRoutes.providerList(ProviderKind.Search.name)) },
            )
            SettingsRow(
                icon = LXIconName.Link, iconColor = Color(red = 0.8713f, green = 0.58f, blue = 0f),
                label = stringResource(R.string.settings_web_fetch),
                sub = if (df != null) stringResource(R.string.settings_provider_default_fmt, df.name) else stringResource(R.string.settings_provider_unconfigured),
                value = stringResource(R.string.settings_providers_enabled_fmt, state.fetchProviders.count { it.enabled }),
                onTap = { navController.navigate(SettingsRoutes.providerList(ProviderKind.Fetch.name)) },
            )
            SettingsRow(
                icon = LXIconName.Mic, iconColor = Color(red = 0.8018f, green = 0.4038f, blue = 0.8909f),
                label = stringResource(R.string.settings_voice_audio),
                sub = stringResource(
                    R.string.settings_voice_dictation_fmt,
                    Presets.voice.firstOrNull { it.id == state.voice.preset }
                        ?.takeIf { it.id != "system" }
                        ?.name
                        ?: stringResource(R.string.settings_voice_system),
                ),
                value = voiceLanguageSummary(state.voice.inputLanguage),
                isLast = true,
                onTap = { navController.navigate(SettingsRoutes.VOICE) },
            )
        }

        // 能力扩展 ------------------------------------------------------------
        SettingsSection(
            label = stringResource(R.string.settings_section_capabilities),
            footer = stringResource(R.string.settings_section_capabilities_footer),
        ) {
            SettingsRow(
                icon = LXIconName.Skill, iconColor = Color(red = 0f, green = 0.7601f, blue = 0.7664f),
                label = stringResource(R.string.settings_title_skills), sub = stringResource(R.string.settings_skills_sub),
                value = stringResource(R.string.settings_skills_enabled_fraction_fmt, state.skills.count { it.enabled }, state.skills.size),
                onTap = { navController.navigate(SettingsRoutes.SKILLS) },
            )
            SettingsRow(
                icon = LXIconName.Plug, iconColor = Color(red = 0f, green = 0.78f, blue = 0.55f),
                label = stringResource(R.string.settings_mcp_servers), sub = "Model Context Protocol",
                value = stringResource(R.string.settings_mcp_connections_fmt, state.mcpServers.count { it.enabled }),
                onTap = { navController.navigate(SettingsRoutes.MCP_LIST) },
            )
            SettingsRow(
                icon = LXIconName.Workflow, iconColor = Color(red = 0.3503f, green = 0.6649f, blue = 0.9741f),
                label = stringResource(R.string.settings_linux_runtime),
                sub = stringResource(state.linuxRuntime.summaryRes),
                value = stringResource(state.linuxRuntime.badgeRes),
                onTap = { navController.navigate(SettingsRoutes.LINUX_RUNTIME) },
            )
            if (ComputerUseFeatureProvider.available) {
                SettingsRow(
                    icon = LXIconName.Sparkle,
                    iconColor = Color(red = 0.62f, green = 0.48f, blue = 0.96f),
                    label = "Computer Use",
                    sub = stringResource(R.string.settings_computer_use_sub),
                    value = "Direct",
                    onTap = { navController.navigate(SettingsRoutes.COMPUTER_USE) },
                )
            }
            SettingsRow(
                icon = LXIconName.Dream, iconColor = Color(red = 0.809f, green = 0.4552f, blue = 0.8891f),
                label = stringResource(R.string.settings_dream_mode), sub = stringResource(R.string.settings_dream_sub),
                value = if (state.dream.enabled) stringResource(R.string.settings_status_on) else stringResource(R.string.settings_status_off),
                onTap = { navController.navigate(SettingsRoutes.DREAM) },
            )
            SettingsRow(
                icon = LXIconName.Clock, iconColor = Color(red = 0.95f, green = 0.6f, blue = 0.2f),
                label = stringResource(R.string.settings_title_cron), sub = stringResource(R.string.settings_cron_sub),
                isLast = true,
                onTap = { navController.navigate(SettingsRoutes.CRON) },
            )
        }

        // 应用 ----------------------------------------------------------------
        SettingsSection(label = stringResource(R.string.settings_section_app)) {
            SettingsRow(
                icon = LXIconName.Sun, iconColor = Color(red = 0.896f, green = 0.6013f, blue = 0f),
                label = stringResource(R.string.settings_appearance), value = if (isDark) stringResource(R.string.settings_appearance_dark) else stringResource(R.string.settings_appearance_light),
                onTap = { navController.navigate(SettingsRoutes.APPEARANCE) },
            )
            SettingsRow(
                icon = LXIconName.Message, iconColor = Color(red = 0.3503f, green = 0.6649f, blue = 0.9741f),
                label = stringResource(R.string.settings_language_title), value = AppLanguage.label(currentLanguage),
                onTap = { navController.navigate(SettingsRoutes.LANGUAGE) },
            )
            SettingsRow(
                icon = LXIconName.Cog, iconColor = t.text3,
                label = stringResource(R.string.settings_notifications), value = stringResource(R.string.settings_notifs_enabled_fmt, state.notifs.enabledCount),
                onTap = { navController.navigate(SettingsRoutes.NOTIFICATIONS) },
            )
            SettingsRow(
                icon = LXIconName.Paperclip, iconColor = Color(red = 0.9351f, green = 0.5079f, blue = 0.4015f),
                label = stringResource(R.string.settings_keyboard_input), sub = stringResource(R.string.settings_keyboard_input_sub), isLast = true,
                onTap = { navController.navigate(SettingsRoutes.INPUT) },
            )
        }

        // 隐私与安全 ----------------------------------------------------------
        SettingsSection(label = stringResource(R.string.settings_section_privacy_security)) {
            PrivacyToggleRow(LXIconName.Pin, t.ok, stringResource(R.string.settings_bio_lock), state.bioLock) {}
            SettingsRow(
                icon = LXIconName.Brain, iconColor = t.text3,
                label = stringResource(R.string.settings_data_privacy), sub = stringResource(R.string.settings_data_privacy_sub),
                onTap = { navController.navigate(SettingsRoutes.PRIVACY) },
            )
            PrivacyToggleRow(LXIconName.Sparkle, t.text3, stringResource(R.string.settings_usage_diagnostics), state.telemetry) {}
            PrivacyToggleRow(LXIconName.Check, t.text3, stringResource(R.string.settings_auto_update), state.autoUpdate, isLast = true) {}
        }

        // 关于 ----------------------------------------------------------------
        SettingsSection(label = stringResource(R.string.settings_section_about)) {
            SettingsRow(icon = LXIconName.Sparkle, label = stringResource(R.string.app_name), value = "2.4.1 (build 8721)", chevron = false)
            SettingsRow(icon = LXIconName.Play, label = stringResource(R.string.settings_rewatch_onboarding), sub = stringResource(R.string.settings_rewatch_onboarding_sub), onTap = onReplayOnboarding)
            SettingsRow(icon = LXIconName.Book, label = stringResource(R.string.settings_help_center), onTap = {})
            SettingsRow(icon = LXIconName.Message, label = stringResource(R.string.settings_feedback), onTap = {})
            SettingsRow(
                icon = LXIconName.Link,
                label = stringResource(R.string.settings_open_source),
                isLast = true,
                onTap = { navController.navigate(SettingsRoutes.OPEN_SOURCE) },
            )
        }

        Text(
            text = stringResource(R.string.settings_copyright),
            color = t.text4,
            fontSize = 11.sp,
            lineHeight = 16.sp,
            textAlign = TextAlign.Center,
            modifier = Modifier.fillMaxWidth().padding(top = 8.dp, bottom = 4.dp),
        )
    }
}

@Composable
private fun voiceLanguageSummary(language: String): String = when (language) {
    "zh-CN" -> stringResource(R.string.onboarding_voice_language_zh)
    "en-US" -> stringResource(R.string.onboarding_voice_language_en)
    "ja-JP" -> stringResource(R.string.onboarding_voice_language_ja)
    else -> stringResource(R.string.settings_auto)
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
            stringResource(R.string.settings_account),
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
