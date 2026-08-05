package com.lingxi.code.settings

import android.content.Context
import android.content.ContextWrapper
import androidx.activity.ComponentActivity
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
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.lingxi.code.R
import com.lingxi.code.components.LXIconName
import com.lingxi.code.components.LXToggle
import com.lingxi.code.components.tint
import com.lingxi.code.model.NotifConfig
import com.lingxi.code.model.Presets
import com.lingxi.code.model.VoiceConfig
import com.lingxi.code.theme.AppLanguage
import com.lingxi.code.theme.AppLanguageStore
import com.lingxi.code.theme.LingXiTheme
import kotlinx.coroutines.launch

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
                stringResource(R.string.settings_account_pro_renewal),
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

        SettingsSection(label = stringResource(R.string.settings_section_monthly_usage)) {
            SettingsRow(label = stringResource(R.string.settings_conversation_count), value = "247 / 1000", chevron = false)
            SettingsRow(label = stringResource(R.string.settings_inference_duration), value = stringResource(R.string.settings_inference_duration_value), chevron = false)
            SettingsRow(label = stringResource(R.string.settings_storage), value = "1.2 / 10 GB", chevron = false, isLast = true)
        }
        SettingsSection {
            SettingsRow(icon = LXIconName.Brain, label = stringResource(R.string.settings_manage_subscription), onTap = {})
            SettingsRow(icon = LXIconName.Link, label = stringResource(R.string.settings_sync_devices), sub = stringResource(R.string.settings_sync_devices_sub), onTap = {})
            SettingsRow(icon = LXIconName.X, label = stringResource(R.string.settings_logout), danger = true, isLast = true, onTap = {})
        }
    }
}

// MARK: - Notifications -----------------------------------------------------
@Composable
fun NotificationsPage(notifs: NotifConfig, onChange: (NotifConfig) -> Unit) {
    SettingsSection(
        label = stringResource(R.string.settings_section_notification_type),
        footer = stringResource(R.string.settings_notifications_footer),
    ) {
        SettingsRow(label = stringResource(R.string.settings_notif_workflow_complete), sub = stringResource(R.string.settings_notif_workflow_complete_sub), chevron = false) {
            LXToggle(checked = notifs.workflows, onCheckedChange = { onChange(notifs.copy(workflows = it)) })
        }
        SettingsRow(label = stringResource(R.string.settings_notif_mention), sub = stringResource(R.string.settings_notif_mention_sub), chevron = false) {
            LXToggle(checked = notifs.mentions, onCheckedChange = { onChange(notifs.copy(mentions = it)) })
        }
        SettingsRow(label = stringResource(R.string.settings_notif_cron_report), sub = stringResource(R.string.settings_notif_cron_report_sub), chevron = false) {
            LXToggle(checked = notifs.crons, onCheckedChange = { onChange(notifs.copy(crons = it)) })
        }
        SettingsRow(label = stringResource(R.string.settings_notif_product_update), sub = stringResource(R.string.settings_notif_product_update_sub), chevron = false, isLast = true) {
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
        SettingsSection(label = stringResource(R.string.settings_section_suggestions)) {
            SettingsRow(label = stringResource(R.string.settings_smart_suggestions), chevron = false) { LXToggle(checked = smartSugg, onCheckedChange = { smartSugg = it }) }
            SettingsRow(label = stringResource(R.string.settings_suggestions_history), chevron = false, isLast = true) { LXToggle(checked = fromHistory, onCheckedChange = { fromHistory = it }) }
        }
    }
}

// MARK: - Privacy -----------------------------------------------------------
@Composable
fun PrivacyPage() {
    var contribute by remember { mutableStateOf(false) }
    var crash by remember { mutableStateOf(true) }
    Column(Modifier.fillMaxWidth()) {
        SettingsSection(label = stringResource(R.string.settings_section_data)) {
            SettingsRow(icon = LXIconName.Brain, label = stringResource(R.string.settings_export_data), onTap = {})
            SettingsRow(icon = LXIconName.X, label = stringResource(R.string.settings_delete_account), danger = true, isLast = true, onTap = {})
        }
        SettingsSection(
            label = stringResource(R.string.settings_section_visibility),
            footer = stringResource(R.string.settings_privacy_footer),
        ) {
            SettingsRow(label = stringResource(R.string.settings_contribute_training), chevron = false) { LXToggle(checked = contribute, onCheckedChange = { contribute = it }) }
            SettingsRow(label = stringResource(R.string.settings_crash_report), chevron = false, isLast = true) { LXToggle(checked = crash, onCheckedChange = { crash = it }) }
        }
    }
}

// MARK: - Language ----------------------------------------------------------
@Composable
fun LanguagePage(language: String, onSelect: (String) -> Unit) {
    var follow by remember { mutableStateOf(true) }
    val context = LocalContext.current
    val store = remember { AppLanguageStore(context.applicationContext) }
    val current by store.language.collectAsState(
        initial = AppLanguageStore.currentLanguage(context.applicationContext),
    )
    val scope = rememberCoroutineScope()
    val activity = context.findComponentActivity()
    Column(Modifier.fillMaxWidth()) {
        SettingsSection(
            label = stringResource(R.string.settings_language_title),
            footer = stringResource(R.string.settings_language_footer),
        ) {
            RadioList(
                options = AppLanguage.SUPPORTED.map { (code, label) -> RadioOption(code, label) },
                selected = current,
                onSelect = { code ->
                    scope.launch {
                        store.setLanguage(code)
                        activity?.recreate()
                    }
                },
            )
        }
        SettingsSection(label = stringResource(R.string.settings_section_region)) {
            SettingsRow(label = stringResource(R.string.settings_date_format), value = "2026/5/14", isLast = true, onTap = {})
        }
        SettingsSection(label = stringResource(R.string.settings_section_ai_reply_language)) {
            SettingsRow(label = stringResource(R.string.settings_follow_interface), sub = stringResource(R.string.settings_follow_interface_sub), chevron = false, isLast = true) {
                LXToggle(checked = follow, onCheckedChange = { follow = it })
            }
        }
    }
}

/** Unwraps [ContextWrapper] chains to the hosting [ComponentActivity] (or null). */
private tailrec fun Context.findComponentActivity(): ComponentActivity? = when (this) {
    is ComponentActivity -> this
    is ContextWrapper -> baseContext.findComponentActivity()
    else -> null
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
            label = stringResource(R.string.settings_voice_listen_section),
            footer = stringResource(R.string.settings_voice_listen_footer),
        ) {
            RadioList(
                options = listOf(
                    RadioOption("system", stringResource(R.string.settings_voice_android_system), stringResource(R.string.settings_voice_android_system_sub)),
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
                        stringResource(R.string.voice_recognition_language),
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
            label = stringResource(R.string.settings_voice_speak_section),
            footer = stringResource(R.string.settings_voice_speak_footer),
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
                        placeholder = stringResource(R.string.settings_voice_id_placeholder),
                    )
                }
            }
        }

        SettingsSection(label = stringResource(R.string.settings_voice_playback_options), footer = stringResource(R.string.settings_voice_playback_footer)) {
            SettingsRow(
                label = stringResource(R.string.voice_speech_rate),
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
            SettingsRow(label = stringResource(R.string.voice_auto_play), chevron = false, isLast = true) {
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

@Composable
private fun voiceLanguageLabel(language: String): String = when (language) {
    "zh-CN" -> stringResource(R.string.common_lang_zh_hans)
    "en-US" -> stringResource(R.string.onboarding_voice_language_en)
    "ja-JP" -> stringResource(R.string.onboarding_voice_language_ja)
    else -> stringResource(R.string.settings_auto)
}

@Composable
private fun voiceLanguageShortLabel(language: String): String = when (language) {
    "zh-CN" -> stringResource(R.string.onboarding_voice_language_zh)
    "en-US" -> "EN"
    "ja-JP" -> stringResource(R.string.onboarding_voice_language_ja)
    else -> stringResource(R.string.settings_auto)
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
