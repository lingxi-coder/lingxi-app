package com.lingxi.code.settings

import android.content.Intent
import android.net.Uri
import android.provider.Settings
import android.text.format.Formatter
import androidx.compose.foundation.clickable
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.layout.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.draw.shadow
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.stateDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.outlined.Add
import androidx.compose.material.icons.outlined.Remove
import androidx.compose.material3.*
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.material3.Slider
import androidx.compose.material3.SliderDefaults
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.lingxi.code.R
import com.lingxi.code.components.LXToggle
import com.lingxi.code.theme.LingXiTheme
import com.lingxi.code.voice.audio.AudioCloudBinding
import com.lingxi.code.voice.audio.AudioConfigurationV4
import com.lingxi.code.voice.audio.AudioReadiness
import com.lingxi.code.voice.audio.AudioSource
import com.lingxi.code.voice.audio.ProviderAudioCapability
import com.lingxi.code.voice.audio.ProviderAudioProfileOption
import com.lingxi.code.voice.offline.GeneratedModelKind
import com.lingxi.code.voice.offline.GeneratedOfflineModelEntry
import com.lingxi.code.voice.offline.GeneratedVoiceModelCatalog
import com.lingxi.code.voice.offline.ModelState
import com.lingxi.code.voice.offline.VoiceModelDownloader
import com.lingxi.code.voice.offline.localizedDisplayName
import java.util.Locale

/**
 * Voice settings, in the same order as the desktop page: input, output (with the
 * voice, speed and auto-play that used to be a separate "playback" block),
 * offline models managed one by one, and a collapsed "advanced & permissions"
 * block. Shared by chat and Direct-build Computer Use: changes are persisted by
 * [SettingsStore] and read by the audio host at call time.
 *
 * @param voice the live device-local v4 audio config.
 * @param onChange writes a mutated config back into the store.
 */
@Composable
fun VoicePage(
    voice: AudioConfigurationV4,
    capability: VoiceCapabilitySnapshot,
    @Suppress("UNUSED_PARAMETER") revision: Long = 0,
    @Suppress("UNUSED_PARAMETER") saving: Boolean = false,
    saveError: String? = null,
    onProviderSettings: () -> Unit = {},
    onChange: ((AudioConfigurationV4) -> AudioConfigurationV4) -> Unit,
) {
    val t = LingXiTheme.palette
    val context = LocalContext.current
    val modelStates by VoiceModelDownloader.states.collectAsState()
    val modelLanguage = context.resources.configuration.locales[0].language
    val allClearLabel = stringResource(R.string.settings_voice_all_clear)
    val catalogDefaultLabel = stringResource(R.string.audio_catalog_default)
    val appSettingsIntent = remember(context) {
        Intent(
            Settings.ACTION_APPLICATION_DETAILS_SETTINGS,
            Uri.fromParts("package", context.packageName, null),
        )
    }
    Column(Modifier.fillMaxWidth()) {
        // MARK: Input
        SettingsSection(
            label = stringResource(R.string.audio_settings_input),
            footer = stringResource(R.string.audio_language_hint),
        ) {
            VoiceChoiceRow(
                label = stringResource(R.string.voice_recognition_language),
                options = listOf(
                    RadioOption("auto", stringResource(R.string.onboarding_voice_language_system)),
                    RadioOption("zh-CN", stringResource(R.string.onboarding_voice_language_zh)),
                    RadioOption("en-US", stringResource(R.string.onboarding_voice_language_en)),
                    RadioOption("ja-JP", stringResource(R.string.onboarding_voice_language_ja)),
                ) + if (voice.language !in listOf("auto", "zh-CN", "en-US", "ja-JP")) {
                    listOf(RadioOption(voice.language, voice.language))
                } else {
                    emptyList()
                },
                selected = voice.language,
                onSelect = { language -> onChange { it.copy(language = language) } },
            )
            VoiceChoiceRow(
                label = stringResource(R.string.audio_source_title),
                options = voiceSourceOptions(
                    capability.providerRecognition?.supported == true || capability.providerRecognitionProfiles.isNotEmpty(),
                ),
                selected = voice.recognition.source.value,
                onSelect = { source ->
                    onChange { current ->
                        current.copy(
                            recognition = current.recognition.copy(
                                source = AudioSource(source),
                                offlineModelId = if (source == AudioSource.OFFLINE.value) current.recognition.offlineModelId else null,
                            ),
                        )
                    }
                },
            )
            SettingsRow(
                label = stringResource(R.string.audio_availability),
                value = readinessLabel(capability.recognitionReadiness),
                valueColor = if (capability.recognitionReadiness == AudioReadiness.AVAILABLE) t.ok else t.text3,
                sub = capability.recognitionReason,
                chevron = false,
            )
            if (voice.recognition.source == AudioSource.OFFLINE) {
                VoiceChoiceRow(
                    label = stringResource(R.string.audio_offline_recognition_models),
                    options = offlineModelOptions(GeneratedModelKind.Stt, modelStates, modelLanguage),
                    selected = voice.recognition.offlineModelId.orEmpty(),
                    onSelect = { modelId ->
                        onChange { current ->
                            current.copy(recognition = current.recognition.copy(offlineModelId = modelId.ifBlank { null }))
                        }
                    },
                )
            }
            if (voice.recognition.source == AudioSource.PROVIDER) {
                ProviderAudioControls(
                    voice.recognition.cloud, capability.providerRecognition, capability.providerRecognitionProfiles,
                    onProviderSettings,
                ) { cloud -> onChange { it.copy(recognition = it.recognition.copy(cloud = cloud)) } }
            }
        }

        // MARK: Output
        SettingsSection(
            label = stringResource(R.string.audio_settings_output),
            footer = stringResource(R.string.settings_voice_playback_footer),
        ) {
            VoiceChoiceRow(
                label = stringResource(R.string.audio_source_title),
                options = voiceSourceOptions(
                    capability.providerSpeech?.supported == true || capability.providerSpeechProfiles.isNotEmpty(),
                ),
                selected = voice.speech.source.value,
                onSelect = { source ->
                    onChange { current ->
                        current.copy(
                            speech = current.speech.copy(
                                source = AudioSource(source),
                                offlineModelId = if (source == AudioSource.OFFLINE.value) current.speech.offlineModelId else null,
                                voice = current.speech.voice?.takeIf { it.source.value == source },
                            ),
                        )
                    }
                },
            )
            SettingsRow(
                label = stringResource(R.string.audio_availability),
                value = readinessLabel(capability.speechReadiness),
                valueColor = if (capability.speechReadiness == AudioReadiness.AVAILABLE) t.ok else t.text3,
                sub = capability.speechReason,
                chevron = false,
            )
            if (voice.speech.source == AudioSource.OFFLINE) {
                VoiceChoiceRow(
                    label = stringResource(R.string.audio_offline_speech_models),
                    options = offlineModelOptions(GeneratedModelKind.Tts, modelStates, modelLanguage),
                    selected = voice.speech.offlineModelId.orEmpty(),
                    onSelect = { modelId ->
                        val selectedModel = modelId.ifBlank { null }
                        onChange { current ->
                            current.copy(
                                speech = current.speech.copy(
                                    offlineModelId = selectedModel,
                                    voice = current.speech.voice?.takeIf { it.modelId == selectedModel },
                                ),
                            )
                        }
                    },
                )
            }
            if (voice.speech.source == AudioSource.PROVIDER) {
                ProviderAudioControls(
                    voice.speech.cloud, capability.providerSpeech, capability.providerSpeechProfiles,
                    onProviderSettings,
                ) { cloud -> onChange { it.copy(speech = it.speech.copy(cloud = cloud, voice = null)) } }
            }
            val voiceChoices = when (voice.speech.source) {
                AudioSource.SYSTEM -> capability.systemVoiceOptions
                AudioSource.OFFLINE -> capability.offlineVoiceOptions.filter {
                    voice.speech.offlineModelId == null || it.selection?.modelId == voice.speech.offlineModelId
                }
                AudioSource.PROVIDER -> capability.voiceOptions.filter { it.source == VoiceOptionSource.Provider }
                else -> emptyList()
            }
            VoiceChoiceRow(
                label = stringResource(R.string.voice_voice_name),
                options = listOf(RadioOption("", stringResource(R.string.settings_voice_system_default))) +
                    voiceChoices.map { option -> RadioOption(option.id, option.label, option.details) },
                selected = voice.speech.voice?.let { requested ->
                    voiceChoices.firstOrNull { it.selection == requested }?.id ?: "unavailable"
                } ?: "",
                onSelect = { key ->
                    val option = voiceChoices.firstOrNull { it.id == key }
                    if (option == null) {
                        onChange { current -> current.copy(speech = current.speech.copy(voice = null)) }
                    } else {
                        val selection = checkNotNull(option.selection)
                        onChange { current ->
                            current.copy(
                                speech = current.speech.copy(
                                    source = selection.source,
                                    offlineModelId = selection.modelId.takeIf { selection.source == AudioSource.OFFLINE },
                                    cloud = if (selection.source == AudioSource.PROVIDER) {
                                        current.speech.cloud.copy(modelId = selection.modelId)
                                    } else {
                                        current.speech.cloud
                                    },
                                    voice = selection,
                                ),
                            )
                        }
                    }
                },
            )
            if (voice.speech.source == AudioSource.PROVIDER) {
                SettingsRow(
                    label = stringResource(R.string.voice_speech_rate),
                    value = "1.0x",
                    sub = stringResource(R.string.audio_cloud_rate_fixed),
                    chevron = false,
                )
                if (voice.rate != 1.0) {
                    SettingsRow(
                        label = stringResource(R.string.audio_cloud_rate_reset),
                        sub = String.format(Locale.US, "%.1fx", voice.rate),
                        onTap = { onChange { current -> current.copy(rate = 1.0) } },
                    )
                }
            } else {
                VoiceRateControl(voice.rate.toFloat()) { rate ->
                    onChange { current -> current.copy(rate = rate.toDouble()) }
                }
            }
            SettingsRow(label = stringResource(R.string.voice_auto_play), chevron = false, isLast = true) {
                LXToggle(
                    checked = voice.autoPlayReplies,
                    onCheckedChange = { enabled -> onChange { current -> current.copy(autoPlayReplies = enabled) } },
                )
            }
        }

        if (saveError != null) {
            Text(
                saveError,
                color = t.danger,
                fontSize = androidx.compose.material3.MaterialTheme.typography.bodySmall.fontSize,
                modifier = Modifier.padding(horizontal = 4.dp, vertical = 6.dp),
            )
        }

        // MARK: Offline models, one by one
        VoiceSettingsDisclosure(
            title = stringResource(R.string.audio_settings_offline_models),
            initiallyOpen = voice.recognition.source == AudioSource.OFFLINE ||
                voice.speech.source == AudioSource.OFFLINE ||
                modelStates.values.any { it.isBusy || it is ModelState.Failed },
        ) {
            val models = GeneratedVoiceModelCatalog.all
            SettingsSection(footer = stringResource(R.string.audio_offline_models_hint)) {
                models.forEachIndexed { index, model ->
                    OfflineModelRow(
                        model = model,
                        state = modelStates[model.id] ?: ModelState.NotInstalled,
                        displayName = model.localizedDisplayName(modelLanguage),
                        sizeLabel = Formatter.formatShortFileSize(context, model.approxSizeBytes),
                        isLast = index == models.lastIndex,
                    )
                }
            }
        }

        // MARK: Advanced & permissions
        VoiceSettingsDisclosure(
            title = stringResource(R.string.audio_settings_advanced),
            initiallyOpen = capability.microphonePermission != VoicePermissionStatus.Granted,
        ) {
            SettingsSection(label = stringResource(R.string.audio_conversation_section)) {
                RadioList(
                    options = buildList {
                        add(RadioOption("agent", stringResource(R.string.audio_agent_conversation)))
                        if (capability.realtimeConversationSupported || voice.conversation.mode == "realtime") {
                            add(RadioOption("realtime", stringResource(R.string.audio_realtime_conversation)))
                        }
                    },
                    selected = voice.conversation.mode,
                    onSelect = { mode -> onChange { it.copy(conversation = it.conversation.copy(mode = mode)) } },
                )
                if (voice.conversation.mode == "realtime") {
                    ProviderAudioControls(
                        voice.conversation.cloud, capability.providerRealtime, capability.providerRealtimeProfiles,
                        onProviderSettings,
                    ) { cloud -> onChange { it.copy(conversation = it.conversation.copy(cloud = cloud, voice = null)) } }
                    val realtimeVoices = capability.providerRealtime?.voices.orEmpty()
                        .filter { it.selection.modelId == capability.providerRealtime?.modelId }
                    if (realtimeVoices.isNotEmpty()) {
                        VoiceChoiceRow(
                            label = stringResource(R.string.voice_voice_name),
                            options = listOf(RadioOption("", catalogDefaultLabel)) +
                                realtimeVoices.map { RadioOption(it.selection.id, it.label) },
                            selected = voice.conversation.voice?.id.orEmpty(),
                            onSelect = { id ->
                                onChange {
                                    it.copy(
                                        conversation = it.conversation.copy(
                                            voice = realtimeVoices.firstOrNull { v -> v.selection.id == id }?.selection,
                                        ),
                                    )
                                }
                            },
                        )
                    }
                    VoiceChoiceRow(
                        label = stringResource(R.string.audio_realtime_interaction),
                        options = listOf(RadioOption("turn_based", stringResource(R.string.audio_realtime_turn_based))),
                        selected = voice.conversation.interaction,
                        onSelect = { interaction ->
                            onChange { it.copy(conversation = it.conversation.copy(interaction = interaction)) }
                        },
                    )
                    if (!capability.realtimeConversationSupported) {
                        SettingsRow(
                            label = stringResource(R.string.audio_realtime_unavailable),
                            sub = capability.realtimeConversationReason,
                            chevron = false,
                            isLast = true,
                        )
                    }
                }
            }
            SettingsSection(
                label = stringResource(R.string.settings_voice_permissions_section),
                footer = stringResource(R.string.settings_voice_permissions_footer),
            ) {
                SettingsRow(
                    label = stringResource(R.string.voice_microphone),
                    value = permissionStatusLabel(capability.microphonePermission),
                    valueColor = if (capability.microphonePermission == VoicePermissionStatus.Granted) t.ok else t.text3,
                    sub = if (capability.microphonePermission == VoicePermissionStatus.Granted) {
                        stringResource(R.string.voice_mic_permission_ok)
                    } else {
                        stringResource(R.string.voice_mic_permission_enable_hint)
                    },
                    chevron = false,
                )
                SettingsRow(
                    label = stringResource(R.string.settings_voice_system_recognizer),
                    value = if (capability.platformRecognizerAvailable) {
                        stringResource(R.string.settings_status_on)
                    } else {
                        stringResource(R.string.settings_voice_unavailable_short)
                    },
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
                    sub = capability.blockingIssues.joinToString(" · ") { it.name }.ifBlank { allClearLabel },
                    isLast = true,
                    onTap = { context.startActivity(appSettingsIntent) },
                )
            }
        }
    }
}

@Composable
private fun voiceSourceOptions(providerAvailable: Boolean): List<RadioOption> =
    listOf(
        RadioOption(
            AudioSource.AUTOMATIC.value,
            stringResource(R.string.settings_voice_mode_automatic),
            stringResource(R.string.settings_voice_mode_automatic_sub),
        ),
        RadioOption(
            AudioSource.SYSTEM.value,
            stringResource(R.string.settings_voice_backend_system),
            stringResource(R.string.settings_voice_android_system_sub),
        ),
        RadioOption(
            AudioSource.OFFLINE.value,
            stringResource(R.string.settings_voice_mode_local_only),
            stringResource(R.string.settings_voice_mode_local_only_sub),
        ),
    ) + if (providerAvailable) {
        listOf(RadioOption(AudioSource.PROVIDER.value, stringResource(R.string.audio_settings_cloud_source)))
    } else {
        emptyList()
    }

@Composable
private fun offlineModelOptions(
    kind: GeneratedModelKind,
    modelStates: Map<String, ModelState>,
    language: String,
): List<RadioOption> {
    val installed = stringResource(R.string.audio_model_installed)
    val notInstalled = stringResource(R.string.audio_model_not_installed)
    return listOf(RadioOption("", stringResource(R.string.audio_language_default_model))) +
        GeneratedVoiceModelCatalog.all.filter { it.kind == kind }.map { model ->
            RadioOption(
                model.id,
                model.localizedDisplayName(language),
                model.languages.joinToString(", ") + " · " +
                    if (modelStates[model.id] == ModelState.Ready) installed else notInstalled,
            )
        }
}

/** A full-width continuous-looking track with native discrete adjustment semantics. */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
internal fun VoiceRateControl(rate: Float, onRateChange: (Float) -> Unit) {
    val colors = MaterialTheme.colorScheme
    val label = stringResource(R.string.voice_speech_rate)
    val valueLabel = String.format(Locale.US, "%.1f×", rate)
    Column(Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 10.dp)) {
        Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
            Text(label, style = MaterialTheme.typography.bodyLarge, modifier = Modifier.weight(1f))
            Text(valueLabel, style = MaterialTheme.typography.labelLarge, color = colors.onSurfaceVariant)
        }
        Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
            IconButton(onClick = { onRateChange(snapVoiceSpeed(rate - 0.1f)) },
                enabled = rate > 0.5f, modifier = Modifier.testTag("voice.rate.decrease")) {
                Icon(Icons.Outlined.Remove, contentDescription = stringResource(R.string.voice_rate_decrease))
            }
            Slider(
                value = rate.coerceIn(0.5f, 2f), onValueChange = { onRateChange(snapVoiceSpeed(it)) },
                valueRange = 0.5f..2f, steps = 14,
                modifier = Modifier.weight(1f).testTag("voice.rate")
                    .semantics { contentDescription = label; stateDescription = valueLabel },
                thumb = {
                    Box(Modifier.size(24.dp).shadow(2.dp, CircleShape)
                        .background(colors.surface, CircleShape).border(2.dp, colors.primary, CircleShape))
                },
                track = { slider ->
                    SliderDefaults.Track(sliderState = slider, modifier = Modifier.height(4.dp),
                        thumbTrackGapSize = 0.dp, drawStopIndicator = null, drawTick = { _, _ -> })
                },
            )
            IconButton(onClick = { onRateChange(snapVoiceSpeed(rate + 0.1f)) },
                enabled = rate < 2f, modifier = Modifier.testTag("voice.rate.increase")) {
                Icon(Icons.Outlined.Add, contentDescription = stringResource(R.string.voice_rate_increase))
            }
        }
    }
    HorizontalDivider(color = LingXiTheme.palette.border, modifier = Modifier.padding(horizontal = 16.dp))
}

/** Progress has its own layout so model metadata can never truncate it. */
@Composable
internal fun OfflineModelProgress(state: ModelState, stateText: String) {
    val context = LocalContext.current
    val colors = MaterialTheme.colorScheme
    Column(Modifier.fillMaxWidth().padding(top = 6.dp).testTag("voice.model-progress"),
        verticalArrangement = Arrangement.spacedBy(6.dp)) {
        Text(stateText, style = MaterialTheme.typography.bodySmall,
            color = if (state is ModelState.Failed) colors.error else colors.onSurfaceVariant)
        when (state) {
            is ModelState.Downloading -> {
                val fraction = if (state.total > 0) (state.bytes.toDouble() / state.total).coerceIn(0.0, 1.0).toFloat() else null
                if (fraction != null) LinearProgressIndicator(progress = { fraction }, modifier = Modifier.fillMaxWidth())
                else LinearProgressIndicator(modifier = Modifier.fillMaxWidth())
                Text(Formatter.formatShortFileSize(context, state.bytes.coerceAtLeast(0)) +
                    if (state.total > 0) " / " + Formatter.formatShortFileSize(context, state.total) else "",
                    style = MaterialTheme.typography.labelSmall, color = colors.onSurfaceVariant)
            }
            is ModelState.Queued, ModelState.Verifying, ModelState.Extracting ->
                LinearProgressIndicator(modifier = Modifier.fillMaxWidth())
            else -> Unit
        }
    }
}

/** One offline model: name, kind · size, state, and the one action that fits the state. */
@Composable
internal fun OfflineModelRow(
    model: GeneratedOfflineModelEntry,
    state: ModelState,
    displayName: String,
    sizeLabel: String,
    isLast: Boolean,
) {
    val t = LingXiTheme.palette
    val kind = stringResource(
        if (model.kind == GeneratedModelKind.Stt) R.string.settings_section_voice_input else R.string.audio_settings_output,
    )
    val stateText = when (state) {
        ModelState.NotInstalled -> stringResource(R.string.voice_model_state_not_installed)
        is ModelState.Queued -> stringResource(R.string.voice_model_state_queued)
        is ModelState.Downloading ->
            stringResource(R.string.voice_model_state_downloading) +
                if (state.total > 0) " ${(state.bytes.toDouble() / state.total * 100).coerceIn(0.0, 100.0).toInt()}%" else ""
        ModelState.Verifying -> stringResource(R.string.voice_model_state_verifying)
        ModelState.Extracting -> stringResource(R.string.voice_model_state_installing)
        ModelState.Ready -> stringResource(R.string.voice_model_state_ready)
        is ModelState.Failed -> stringResource(R.string.voice_model_state_failed) + ": " + state.message
    }
    val actionLabel = when {
        state == ModelState.Ready -> stringResource(R.string.common_delete)
        state.isBusy -> stringResource(R.string.common_cancel)
        state is ModelState.Failed -> stringResource(R.string.common_retry)
        else -> stringResource(R.string.voice_offline_download)
    }
    val actionColor: Color = if (state == ModelState.Ready) t.danger else t.accent
    Column(Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 10.dp)) {
        Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
            Text(displayName, style = MaterialTheme.typography.bodyLarge, color = t.text,
                modifier = Modifier.weight(1f))
            TextButton(onClick = {
                when {
                    state == ModelState.Ready -> VoiceModelDownloader.remove(model)
                    state.isBusy -> VoiceModelDownloader.cancel(model)
                    else -> VoiceModelDownloader.start(model)
                }
            }) { Text(actionLabel, color = actionColor) }
        }
        Text("$kind · $sizeLabel", style = MaterialTheme.typography.bodySmall, color = t.text2)
        OfflineModelProgress(state, stateText)
    }
    if (!isLast) HorizontalDivider(color = t.border, modifier = Modifier.padding(horizontal = 16.dp))
}

private val ModelState.isBusy: Boolean
    get() = this is ModelState.Queued || this is ModelState.Downloading ||
        this == ModelState.Verifying || this == ModelState.Extracting

@Composable
private fun ProviderAudioControls(
    cloud: AudioCloudBinding,
    capability: ProviderAudioCapability?,
    profiles: List<ProviderAudioProfileOption>,
    onProviderSettings: () -> Unit,
    onChange: (AudioCloudBinding) -> Unit,
) {
    VoiceChoiceRow(
        label = stringResource(R.string.audio_provider_binding),
        options = listOf(
            RadioOption("follow_session", stringResource(R.string.audio_follow_session)),
            RadioOption("explicit_profile", stringResource(R.string.audio_explicit_profile)),
        ),
        selected = cloud.binding,
        onSelect = { onChange(cloud.copy(binding = it, modelId = null)) },
    )
    if (cloud.binding == "explicit_profile") {
        VoiceChoiceRow(
            label = stringResource(R.string.audio_provider_profile),
            options = listOf(RadioOption("", stringResource(R.string.audio_choose_profile))) +
                profiles.map { RadioOption(it.profileId, it.label, null) },
            selected = cloud.profileId.orEmpty(),
            onSelect = { onChange(cloud.copy(profileId = it.ifBlank { null }, modelId = null)) },
        )
    }
    val models = capability?.modelIds.orEmpty().filterNotNull()
    if (models.isNotEmpty()) {
        VoiceChoiceRow(
            label = stringResource(R.string.audio_model),
            options = listOf(RadioOption("", stringResource(R.string.audio_catalog_default))) +
                models.map { RadioOption(it, it) },
            selected = cloud.modelId.orEmpty(),
            onSelect = { onChange(cloud.copy(modelId = it.ifBlank { null })) },
        )
    }
    SettingsRow(
        label = stringResource(R.string.audio_settings_account),
        value = stringResource(
            if (capability?.readiness == "ready") R.string.audio_settings_connected else R.string.audio_guide_configuration_title,
        ),
        onTap = onProviderSettings,
    )
}

/**
 * Snap a raw slider value to the voice-rate grid: 0.1 increments clamped to the
 * 0.5x..2.0x range (the iOS `Slider(in: 0.5...2, step: 0.1)`). PURE so the
 * snapping is unit-testable on the plain JVM.
 */
fun snapVoiceSpeed(raw: Float): Float =
    (Math.round(raw * 10f) / 10f).coerceIn(0.5f, 2.0f)

@Composable
private fun readinessLabel(readiness: AudioReadiness): String = when (readiness) {
    AudioReadiness.AVAILABLE -> stringResource(R.string.settings_status_available)
    AudioReadiness.PERMISSION_REQUIRED -> stringResource(R.string.voice_permission_denied_label)
    AudioReadiness.DENIED -> stringResource(R.string.voice_permission_denied_label)
    AudioReadiness.UNAVAILABLE -> stringResource(R.string.settings_voice_unavailable_short)
}

@Composable
private fun permissionStatusLabel(status: VoicePermissionStatus): String = when (status) {
    VoicePermissionStatus.Granted -> stringResource(R.string.voice_permission_authorized_label)
    VoicePermissionStatus.Denied -> stringResource(R.string.voice_permission_denied_label)
    VoicePermissionStatus.Unknown -> stringResource(R.string.voice_permission_unknown_label)
}
