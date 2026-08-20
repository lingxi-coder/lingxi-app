package com.lingxi.code.settings

import android.content.Context
import android.content.ContextWrapper
import android.content.Intent
import android.net.Uri
import android.provider.Settings
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
import com.lingxi.code.model.VoiceConfig
import com.lingxi.code.theme.AppLanguage
import com.lingxi.code.theme.AppLanguageStore
import com.lingxi.code.theme.LingXiTheme
import com.lingxi.code.voice.offline.ModelState
import com.lingxi.code.voice.offline.VoiceModelDownloader
import kotlinx.coroutines.launch
import java.util.Locale

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
    val current by store.language.collectAsState()
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
 * @param voice the live voice config (recognition / language / voice / rate).
 * @param onChange writes a mutated config back into the store.
 */
@Composable
fun VoicePage(
    voice: VoiceConfig,
    capability: VoiceCapabilitySnapshot,
    onChange: (VoiceConfig) -> Unit,
) {
    val t = LingXiTheme.palette
    val context = LocalContext.current
    val issueLabels = mapOf(
        VoiceBlockingIssue.MicrophonePermissionRequired to stringResource(R.string.voice_issue_need_mic_permission),
        VoiceBlockingIssue.OfflineLanguageUnsupported to stringResource(R.string.settings_voice_issue_offline_language_unsupported),
        VoiceBlockingIssue.OfflineRecognitionModelRequired to stringResource(R.string.settings_voice_issue_offline_pack_required),
        VoiceBlockingIssue.AutomaticRecognizerUnavailable to stringResource(R.string.voice_recognizer_unavailable_for_language),
        VoiceBlockingIssue.RequestedVoiceUnavailable to stringResource(R.string.voice_issue_voice_not_available),
        VoiceBlockingIssue.PlaybackVoiceUnavailable to stringResource(R.string.voice_no_playback_voice_available),
    )
    val allClearLabel = stringResource(R.string.settings_voice_all_clear)
    val appSettingsIntent = remember(context) {
        Intent(
            Settings.ACTION_APPLICATION_DETAILS_SETTINGS,
            Uri.fromParts("package", context.packageName, null),
        )
    }

    Column(Modifier.fillMaxWidth()) {
        SettingsSection(
            label = stringResource(R.string.settings_voice_listen_section),
            footer = stringResource(R.string.settings_voice_listen_footer),
        ) {
            RadioList(
                options = listOf(
                    RadioOption(
                        VoiceConfig.MODE_AUTOMATIC,
                        stringResource(R.string.settings_voice_mode_automatic),
                        stringResource(R.string.settings_voice_mode_automatic_sub),
                    ),
                    RadioOption(
                        VoiceConfig.MODE_LOCAL_ONLY,
                        stringResource(R.string.settings_voice_mode_local_only),
                        stringResource(R.string.settings_voice_mode_local_only_sub),
                    ),
                ),
                selected = voice.recognitionMode,
                onSelect = { onChange(voice.copy(recognitionMode = it)) },
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
                        voiceLanguageLabel(voice.language),
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
                            color = if (voice.language == language) t.accent else t.text3,
                            fontSize = 12.sp,
                            modifier = Modifier
                                .clip(CircleShape)
                                .background(
                                    if (voice.language == language) {
                                        t.accent.tint(0.14f)
                                    } else {
                                        t.surfaceActive
                                    },
                                )
                                .clickable { onChange(voice.copy(language = language)) }
                                .weight(1f)
                                .sizeIn(minHeight = 48.dp)
                                .padding(horizontal = 6.dp, vertical = 7.dp),
                        )
                    }
                }
            }
            SettingsRow(
                label = stringResource(R.string.settings_voice_requested_backend),
                value = recognitionBackendLabel(voice.recognitionMode),
                chevron = false,
            )
            SettingsRow(
                label = stringResource(R.string.settings_voice_effective_backend),
                value = recognitionBackendLabel(capability.effectiveRecognitionBackend),
                sub = capability.fallbackReason,
                chevron = false,
                isLast = true,
            )
        }

        SettingsSection(
            label = stringResource(R.string.settings_voice_speak_section),
            footer = stringResource(R.string.settings_voice_speak_footer),
        ) {
            val systemVoices = capability.systemVoiceOptions
            val offlineVoices = capability.offlineVoiceOptions
            if (systemVoices.isNotEmpty()) {
                RadioList(
                    options = listOf(
                        RadioOption(
                            VoiceConfig.DEFAULT_VOICE_SELECTION,
                            stringResource(R.string.settings_voice_system_default),
                            stringResource(R.string.settings_provider_preset_system_voice_sub),
                        ),
                    ) + systemVoices.map {
                        RadioOption(it.id, it.label, it.details)
                    },
                    selected = voice.voiceSelection.takeIf { it.startsWith(VoiceConfig.SYSTEM_VOICE_PREFIX) }
                        ?: VoiceConfig.DEFAULT_VOICE_SELECTION,
                    onSelect = { onChange(voice.copy(voiceSelection = it)) },
                )
            }
            if (offlineVoices.isNotEmpty()) {
                Column(Modifier.fillMaxWidth().padding(top = 10.dp)) {
                    Text(
                        text = stringResource(R.string.settings_voice_offline_group),
                        color = t.text3,
                        fontSize = 12.sp,
                        fontWeight = FontWeight.Medium,
                        modifier = Modifier.padding(horizontal = 2.dp, vertical = 6.dp),
                    )
                    RadioList(
                        options = offlineVoices.map { RadioOption(it.id, it.label, it.details) },
                        selected = voice.voiceSelection.takeIf { it.startsWith(VoiceConfig.SHERPA_VOICE_PREFIX) }.orEmpty(),
                        onSelect = { onChange(voice.copy(voiceSelection = it)) },
                    )
                }
            }
            SettingsRow(
                label = stringResource(R.string.settings_voice_requested_voice),
                value = capability.requestedVoice?.label ?: stringResource(R.string.settings_voice_system_default),
                chevron = false,
            )
            SettingsRow(
                label = stringResource(R.string.settings_voice_effective_voice),
                value = capability.effectiveVoice?.label ?: stringResource(R.string.settings_voice_unavailable_short),
                sub = capability.effectiveVoice?.details,
                chevron = false,
                isLast = true,
            )
        }

        SettingsSection(
            label = stringResource(R.string.settings_voice_model_section),
            footer = stringResource(R.string.settings_voice_model_footer),
        ) {
            capability.modelPackStates.forEachIndexed { index, pack ->
                SettingsRow(
                    label = pack.title,
                    sub = pack.subtitle,
                    value = voicePackStatusLabel(pack.state, pack.recognitionReady, pack.speechReady),
                    isLast = index == capability.modelPackStates.lastIndex,
                    onTap = {
                        val activeState = pack.state
                        if (activeState is ModelState.Queued || activeState is ModelState.Downloading || activeState is ModelState.Verifying || activeState is ModelState.Extracting) {
                            cancelVoicePack(pack.language)
                        } else {
                            VoiceModelDownloader.startPack(pack.language)
                        }
                    },
                )
            }
        }

        SettingsSection(
            label = stringResource(R.string.settings_voice_playback_options),
            footer = stringResource(R.string.settings_voice_playback_footer),
        ) {
            SettingsRow(
                label = stringResource(R.string.voice_speech_rate),
                value = String.format(Locale.US, "%.1fx", voice.rate),
                chevron = false,
            ) {
                Slider(
                    value = voice.rate,
                    onValueChange = { v ->
                        onChange(voice.copy(rate = snapVoiceSpeed(v)))
                    },
                    valueRange = 0.5f..2.0f,
                    steps = 14,
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
                    checked = voice.autoPlayReplies,
                    onCheckedChange = { onChange(voice.copy(autoPlayReplies = it)) },
                )
            }
        }

        SettingsSection(
            label = stringResource(R.string.settings_voice_permissions_section),
            footer = stringResource(R.string.settings_voice_permissions_footer),
        ) {
            SettingsRow(
                label = stringResource(R.string.voice_microphone),
                value = permissionStatusLabel(capability.microphonePermission),
                sub = if (capability.microphonePermission == VoicePermissionStatus.Granted) {
                    stringResource(R.string.voice_mic_permission_ok)
                } else {
                    stringResource(R.string.voice_mic_permission_enable_hint)
                },
                chevron = false,
            )
            SettingsRow(
                label = stringResource(R.string.settings_voice_system_recognizer),
                value = availabilityLabel(capability.platformRecognizerAvailable),
                sub = if (capability.platformRecognizerAvailable) {
                    stringResource(R.string.settings_voice_android_system_sub)
                } else {
                    stringResource(R.string.voice_recognizer_unavailable_retry)
                },
                chevron = false,
            )
            SettingsRow(
                label = stringResource(R.string.settings_voice_blocking_issues),
                value = capability.blockingIssues.size.toString(),
                sub = capability.blockingIssues.mapNotNull(issueLabels::get).joinToString(" · ")
                    .ifBlank { allClearLabel },
                isLast = true,
                onTap = { context.startActivity(appSettingsIntent) },
            )
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

@Composable
private fun recognitionBackendLabel(backend: String): String = when (backend) {
    VoiceConfig.MODE_LOCAL_ONLY, "sherpa" -> stringResource(R.string.settings_voice_backend_sherpa)
    VoiceConfig.MODE_AUTOMATIC -> stringResource(R.string.settings_voice_mode_automatic)
    "system" -> stringResource(R.string.settings_voice_backend_system)
    else -> stringResource(R.string.settings_voice_unavailable_short)
}

@Composable
private fun voicePackStatusLabel(state: ModelState, recognitionReady: Boolean, speechReady: Boolean): String =
    when (state) {
        is ModelState.Ready -> {
            if (recognitionReady && speechReady) {
                stringResource(R.string.settings_voice_pack_ready)
            } else {
                stringResource(R.string.settings_voice_pack_partial)
            }
        }
        is ModelState.Downloading -> stringResource(R.string.settings_voice_pack_downloading)
        is ModelState.Queued -> stringResource(R.string.settings_voice_pack_queued)
        is ModelState.Verifying -> stringResource(R.string.settings_voice_pack_verifying)
        is ModelState.Extracting -> stringResource(R.string.settings_voice_pack_extracting)
        is ModelState.Failed -> state.message
        ModelState.NotInstalled -> stringResource(R.string.settings_voice_pack_not_installed)
    }

@Composable
private fun permissionStatusLabel(status: VoicePermissionStatus): String = when (status) {
    VoicePermissionStatus.Granted -> stringResource(R.string.voice_permission_authorized_label)
    VoicePermissionStatus.Denied -> stringResource(R.string.voice_permission_denied_label)
    VoicePermissionStatus.Unknown -> stringResource(R.string.voice_permission_unknown_label)
}

@Composable
private fun availabilityLabel(available: Boolean): String =
    if (available) stringResource(R.string.settings_status_on) else stringResource(R.string.settings_voice_unavailable_short)

private fun cancelVoicePack(language: String) {
    com.lingxi.code.voice.offline.OfflineModelCatalog.packFor(language)
        .forEach(VoiceModelDownloader::cancel)
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
