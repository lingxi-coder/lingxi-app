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
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateMapOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalResources
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.core.content.ContextCompat
import com.lingxi.code.R
import com.lingxi.code.components.LXToggle
import com.lingxi.code.computeruse.ComputerUseCaptureMode
import com.lingxi.code.computeruse.ComputerUseConfiguration
import com.lingxi.code.computeruse.ComputerUseFeatureProvider
import com.lingxi.code.computeruse.ComputerUseGrant
import com.lingxi.code.computeruse.ComputerUseSessionState
import com.lingxi.code.computeruse.ComputerUseTier
import com.lingxi.code.theme.LingXiTheme
import com.lingxi.code.voice.audio.AudioConfigurationV3
import java.util.Locale

@Composable
fun ComputerUseSettingsPage(
    voice: AudioConfigurationV3,
    capability: VoiceCapabilitySnapshot,
    onOpenAudioSettings: () -> Unit,
) {
    val context = LocalContext.current
    val resources = LocalResources.current
    val feature = ComputerUseFeatureProvider
    val state by feature.state.collectAsState()
    val configuration by feature.configuration.collectAsState()
    val apps = remember { feature.listLaunchableApps(context) }
    val selected = remember {
        mutableStateMapOf<String, ComputerUseTier>().apply {
            val launchablePackages = apps.mapTo(mutableSetOf()) { it.packageName }
            val initial = if (state.grants.isNotEmpty()) {
                state.grants.associate { it.packageName to it.tier }
            } else {
                configuration.appSelections
            }
            putAll(initial.filterKeys { it in launchablePackages })
        }
    }
    var search by remember { mutableStateOf("") }
    var pendingStart by remember { mutableStateOf<List<ComputerUseGrant>?>(null) }
    var startFeedback by remember { mutableStateOf<String?>(null) }
    var microphoneGranted by remember {
        mutableStateOf(
            ContextCompat.checkSelfPermission(context, Manifest.permission.RECORD_AUDIO) ==
                PackageManager.PERMISSION_GRANTED,
        )
    }
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
            ).onFailure { error ->
                startFeedback = error.message ?: resources.getString(R.string.settings_cu_start_failed)
            }
        } else {
            startFeedback = resources.getString(R.string.settings_cu_capture_cancelled)
        }
    }
    val notificationLauncher = rememberLauncherForActivityResult(
        ActivityResultContracts.RequestPermission(),
    ) { granted ->
        val grants = pendingStart
        if (!granted || grants == null) {
            pendingStart = null
            if (!granted) {
                startFeedback = resources.getString(R.string.settings_cu_notification_required)
            }
        } else {
            val projection = feature.mediaProjectionRequest(context)
            if (projection != null) {
                projectionLauncher.launch(projection)
            } else {
                pendingStart = null
                feature.start(
                    context = context,
                    grants = grants,
                    includeSystemUi = grants.any { it.systemUi },
                ).onFailure { error ->
                    startFeedback = error.message ?: resources.getString(R.string.settings_cu_start_failed)
                }
            }
        }
    }
    val microphoneLauncher = rememberLauncherForActivityResult(
        ActivityResultContracts.RequestPermission(),
    ) { granted ->
        microphoneGranted = granted
        val current = feature.configuration.value
        if (current.listenEnabled != granted) {
            updateComputerUseConfiguration(
                context = context,
                feature = feature,
                configuration = current.copy(listenEnabled = granted),
            )
        }
    }
    val t = LingXiTheme.palette
    val active = state.sessionState != ComputerUseSessionState.Inactive
    val filteredApps = apps.filter {
        search.isBlank() ||
            it.label.contains(search, ignoreCase = true) ||
            it.packageName.contains(search, ignoreCase = true)
    }
    LaunchedEffect(state.sessionState, state.grants, configuration.appSelections) {
        val launchablePackages = apps.mapTo(mutableSetOf()) { it.packageName }
        val source = if (state.grants.isNotEmpty()) {
            state.grants.associate { it.packageName to it.tier }
        } else {
            configuration.appSelections
        }
        val restored = source.filterKeys { it in launchablePackages }
        if (selected.toMap() != restored) {
            selected.clear()
            selected.putAll(restored)
        }
    }

    Column(verticalArrangement = Arrangement.spacedBy(16.dp)) {
        Column(
            verticalArrangement = Arrangement.spacedBy(8.dp),
            modifier = Modifier
                .fillMaxWidth()
                .background(t.surface, RoundedCornerShape(14.dp))
                .padding(16.dp),
        ) {
            Text(stringResource(R.string.settings_cu_direct_title), color = t.text, fontWeight = FontWeight.SemiBold)
            Text(
                stringResource(R.string.settings_cu_direct_desc),
                color = t.text3,
                fontSize = 13.sp,
            )
            Text(
                if (state.serviceEnabled) stringResource(R.string.settings_cu_a11y_connected) else stringResource(R.string.settings_cu_a11y_not_enabled),
                color = if (state.serviceEnabled) t.ok else t.statusTesting,
                fontSize = 13.sp,
                fontWeight = FontWeight.Medium,
            )
            Text(
                stringResource(R.string.settings_cu_session_status_fmt, state.sessionState.label(), state.captureMode.label()),
                color = t.text2,
                fontSize = 13.sp,
            )
            state.activePackage?.let {
                Text(stringResource(R.string.settings_cu_current_app_fmt, it), color = t.text3, fontSize = 12.sp)
            }
            state.lastError?.let {
                Text(it, color = t.danger, fontSize = 12.sp)
            }
            startFeedback?.let {
                Text(it, color = t.danger, fontSize = 12.sp)
            }
            TextButton(
                onClick = { feature.openAccessibilitySettings(context) },
                modifier = Modifier.sizeIn(minHeight = 48.dp),
            ) {
                Text(if (state.serviceEnabled) stringResource(R.string.settings_cu_check_service) else stringResource(R.string.settings_cu_enable_a11y))
            }
        }

        SettingsSection(
            label = stringResource(R.string.settings_cu_section_audio),
            footer = stringResource(R.string.settings_cu_audio_footer),
        ) {
            SettingsRow(
                label = stringResource(R.string.settings_cu_allow_listen),
                sub = if (microphoneGranted) {
                    stringResource(R.string.settings_cu_mic_granted)
                } else {
                    stringResource(R.string.settings_cu_mic_needed)
                },
                chevron = false,
            ) {
                LXToggle(
                    checked = configuration.listenEnabled,
                    onCheckedChange = { enabled ->
                        if (enabled && !microphoneGranted) {
                            microphoneLauncher.launch(Manifest.permission.RECORD_AUDIO)
                        } else {
                            updateComputerUseConfiguration(
                                context = context,
                                feature = feature,
                                configuration = configuration.copy(listenEnabled = enabled),
                            )
                        }
                    },
                )
            }
            Column(
                verticalArrangement = Arrangement.spacedBy(8.dp),
                modifier = Modifier.fillMaxWidth().padding(horizontal = 14.dp, vertical = 12.dp),
            ) {
                Row(
                    verticalAlignment = Alignment.CenterVertically,
                    modifier = Modifier.fillMaxWidth(),
                ) {
                    Text(
                        stringResource(R.string.settings_cu_max_listen),
                        color = t.text,
                        fontSize = 14.sp,
                        fontWeight = FontWeight.Medium,
                        modifier = Modifier.weight(1f),
                    )
                    Text(
                        stringResource(R.string.settings_cu_seconds_fmt, configuration.maxListenSeconds),
                        color = t.text3,
                        fontSize = 13.sp,
                    )
                }
                Row(
                    horizontalArrangement = Arrangement.spacedBy(6.dp),
                    modifier = Modifier.fillMaxWidth(),
                ) {
                    listOf(5, 15, 30, 60).forEach { seconds ->
                        FilterChip(
                            selected = configuration.maxListenSeconds == seconds,
                            onClick = {
                                updateComputerUseConfiguration(
                                    context,
                                    feature,
                                    configuration.copy(maxListenSeconds = seconds),
                                )
                            },
                            label = { Text("${seconds}s") },
                            modifier = Modifier.weight(1f).sizeIn(minHeight = 48.dp),
                        )
                    }
                }
            }
            SettingsRow(
                label = stringResource(R.string.settings_cu_allow_speak),
                sub = stringResource(R.string.settings_cu_speak_sub),
                chevron = false,
            ) {
                LXToggle(
                    checked = configuration.speakEnabled,
                    onCheckedChange = { enabled ->
                        updateComputerUseConfiguration(
                            context,
                            feature,
                            configuration.copy(speakEnabled = enabled),
                        )
                    },
                )
            }
            SettingsRow(
                label = stringResource(R.string.settings_cu_audio_settings),
                sub = "${voiceLanguageLabel(capability.effectiveLanguage.ifBlank { voice.language })} · ${String.format(Locale.US, "%.1fx", voice.rate)} · ${capability.effectiveVoice?.label ?: stringResource(R.string.settings_voice_system_default)}",
                value = stringResource(R.string.common_open),
                isLast = true,
                onTap = onOpenAudioSettings,
            )
        }

        SettingsSection(
            label = stringResource(R.string.settings_linux_execution_boundary),
            footer = stringResource(R.string.settings_cu_boundary_footer),
        ) {
            SettingsRow(
                label = stringResource(R.string.settings_cu_agent_tools),
                value = "android_use",
                chevron = false,
            )
            SettingsRow(
                label = stringResource(R.string.settings_cu_background_exec),
                sub = stringResource(R.string.settings_cu_background_exec_sub),
                value = stringResource(R.string.permission_allow),
                chevron = false,
            )
            SettingsRow(
                label = stringResource(R.string.settings_cu_session_limit),
                sub = stringResource(R.string.settings_cu_session_limit_sub),
                value = stringResource(R.string.settings_cu_session_limit_value),
                chevron = false,
            )
            SettingsRow(
                label = stringResource(R.string.settings_cu_high_risk),
                sub = stringResource(R.string.settings_cu_high_risk_sub),
                value = stringResource(R.string.settings_cu_enforced),
                chevron = false,
                isLast = true,
            )
        }

        if (!active) {
            OutlinedTextField(
                value = search,
                onValueChange = { search = it },
                label = { Text(stringResource(R.string.settings_cu_search_apps)) },
                singleLine = true,
                modifier = Modifier.fillMaxWidth(),
            )
            Text(
                stringResource(R.string.settings_cu_selection_note),
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
                            persistComputerUseAppSelections(context, feature, selected)
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
                            if (selected.containsKey(app.packageName)) stringResource(R.string.settings_cu_selected) else stringResource(R.string.settings_cu_not_selected),
                            color = if (selected.containsKey(app.packageName)) t.accent else t.text4,
                            fontSize = 12.sp,
                        )
                    }
                    if (selected.containsKey(app.packageName)) {
                        Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                            ComputerUseTier.entries.forEach { tier ->
                                FilterChip(
                                    selected = selected[app.packageName] == tier,
                                    onClick = {
                                        selected[app.packageName] = tier
                                        persistComputerUseAppSelections(context, feature, selected)
                                    },
                                    label = { Text(tier.label()) },
                                    modifier = Modifier.sizeIn(minHeight = 48.dp),
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
                Text(stringResource(R.string.settings_cu_stop_now))
            }
        } else {
            Button(
                enabled = selected.isNotEmpty() && state.serviceEnabled,
                onClick = {
                    startFeedback = null
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
                    if (
                        Build.VERSION.SDK_INT >= 33 &&
                        ContextCompat.checkSelfPermission(
                            context,
                            Manifest.permission.POST_NOTIFICATIONS,
                        ) != PackageManager.PERMISSION_GRANTED
                    ) {
                        pendingStart = grants
                        notificationLauncher.launch(Manifest.permission.POST_NOTIFICATIONS)
                        return@Button
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
                        ).onFailure { error ->
                            startFeedback = error.message ?: resources.getString(R.string.settings_cu_start_failed)
                        }
                    }
                },
                modifier = Modifier.fillMaxWidth().sizeIn(minHeight = 52.dp),
            ) {
                Text(stringResource(R.string.settings_cu_start_session))
            }
        }

        Text(
            stringResource(R.string.settings_cu_forbidden_note),
            color = t.text4,
            fontSize = 12.sp,
            modifier = Modifier.padding(bottom = 8.dp),
        )
        TextButton(
            onClick = { feature.clearAudit(context) },
            modifier = Modifier.sizeIn(minHeight = 48.dp),
        ) {
            Text(stringResource(R.string.settings_cu_clear_audit))
        }
    }
}

private fun updateComputerUseConfiguration(
    context: android.content.Context,
    feature: com.lingxi.code.computeruse.ComputerUseFeature,
    configuration: ComputerUseConfiguration,
) {
    feature.updateConfiguration(context, configuration)
}

private fun persistComputerUseAppSelections(
    context: android.content.Context,
    feature: com.lingxi.code.computeruse.ComputerUseFeature,
    selections: Map<String, ComputerUseTier>,
) {
    feature.updateConfiguration(
        context,
        feature.configuration.value.copy(appSelections = selections.toMap()),
    )
}

@Composable
private fun voiceLanguageLabel(language: String): String = when (language) {
    "zh-CN" -> stringResource(R.string.onboarding_voice_language_zh)
    "en-US" -> stringResource(R.string.onboarding_voice_language_en)
    "ja-JP" -> stringResource(R.string.onboarding_voice_language_ja)
    else -> stringResource(R.string.settings_cu_lang_auto_detect)
}

@Composable
private fun ComputerUseTier.label(): String = when (this) {
    ComputerUseTier.Read -> stringResource(R.string.settings_linux_read_only)
    ComputerUseTier.Click -> stringResource(R.string.computer_use_tier_click)
    ComputerUseTier.Full -> stringResource(R.string.settings_cu_tier_full)
}

@Composable
private fun ComputerUseSessionState.label(): String = when (this) {
    ComputerUseSessionState.Inactive -> stringResource(R.string.settings_cu_state_inactive)
    ComputerUseSessionState.Starting -> stringResource(R.string.settings_cu_state_starting)
    ComputerUseSessionState.Active -> stringResource(R.string.settings_cu_state_active)
    ComputerUseSessionState.AwaitingApproval -> stringResource(R.string.settings_cu_state_awaiting)
    ComputerUseSessionState.Stopping -> stringResource(R.string.settings_cu_state_stopping)
}

@Composable
private fun ComputerUseCaptureMode.label(): String = when (this) {
    ComputerUseCaptureMode.None -> stringResource(R.string.settings_linux_task_unavailable)
    ComputerUseCaptureMode.Accessibility -> "Accessibility"
    ComputerUseCaptureMode.MediaProjection -> "MediaProjection"
}
