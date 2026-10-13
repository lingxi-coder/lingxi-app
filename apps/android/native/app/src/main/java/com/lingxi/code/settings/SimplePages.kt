package com.lingxi.code.settings

import com.lingxi.code.voice.offline.localizedDisplayName
import android.content.Context
import android.content.ContextWrapper
import android.content.Intent
import android.net.Uri
import android.provider.Settings
import androidx.activity.ComponentActivity
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.sizeIn
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.material3.Slider
import androidx.compose.material3.SliderDefaults
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.lingxi.code.R
import com.lingxi.code.components.LXToggle
import com.lingxi.code.components.tint
import com.lingxi.code.theme.AppLanguage
import com.lingxi.code.theme.AppLanguageStore
import com.lingxi.code.theme.LingXiTheme
import com.lingxi.code.voice.audio.AudioConfigurationV4
import com.lingxi.code.voice.audio.AudioProviderKind
import com.lingxi.code.voice.audio.AudioReadiness
import com.lingxi.code.voice.audio.AudioRecognitionPreference
import com.lingxi.code.voice.audio.AudioSource
import com.lingxi.code.voice.audio.AudioSpeechPreference
import com.lingxi.code.voice.audio.AudioVoiceSelection
import com.lingxi.code.voice.audio.AudioCloudBinding
import com.lingxi.code.voice.audio.ProviderAudioCapability
import com.lingxi.code.voice.audio.ProviderAudioProfileOption
import com.lingxi.code.voice.offline.GeneratedVoiceModelCatalog
import com.lingxi.code.voice.offline.ModelState
import com.lingxi.code.voice.offline.VoiceModelDownloader
import kotlinx.coroutines.launch
import java.util.Locale

/** Persisted language and native voice controls. */
// MARK: - Language ----------------------------------------------------------
@Composable
fun LanguagePage(language: String, onSelect: (String) -> Unit) {
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
 * @param voice the live device-local v4 audio config.
 * @param onChange writes a mutated config back into the store.
 */
@Composable
fun VoicePage(
    voice: AudioConfigurationV4,
    capability: VoiceCapabilitySnapshot,
    revision: Long = 0,
    saving: Boolean = false,
    saveError: String? = null,
    onProviderSettings: () -> Unit = {},
    onChange: ((AudioConfigurationV4) -> AudioConfigurationV4) -> Unit,
) {
    val t = LingXiTheme.palette
    val context = LocalContext.current
    val modelStates by VoiceModelDownloader.states.collectAsState()
    val allClearLabel = stringResource(R.string.settings_voice_all_clear)
    val appSettingsIntent = remember(context) {
        Intent(
            Settings.ACTION_APPLICATION_DETAILS_SETTINGS,
            Uri.fromParts("package", context.packageName, null),
        )
    }

    Column(Modifier.fillMaxWidth()) {
        SettingsSection(
            label = stringResource(R.string.audio_settings_input),
            footer = stringResource(R.string.settings_voice_listen_footer),
        ) {
            VoiceChoiceRow(label = stringResource(R.string.voice_recognition_language),
                options = listOf(
                    RadioOption("auto", stringResource(R.string.onboarding_voice_language_system)),
                    RadioOption("zh-CN", stringResource(R.string.onboarding_voice_language_zh)),
                    RadioOption("en-US", stringResource(R.string.onboarding_voice_language_en)),
                    RadioOption("ja-JP", stringResource(R.string.onboarding_voice_language_ja)),
                ) + if (voice.language !in listOf("auto", "zh-CN", "en-US", "ja-JP")) listOf(RadioOption(voice.language, voice.language)) else emptyList(),
                selected = voice.language, onSelect = { language -> onChange { it.copy(language = language) } })
            Text(stringResource(R.string.audio_language_hint), color = t.text3, fontSize = 12.sp,
                modifier = Modifier.padding(horizontal = 14.dp, vertical = 6.dp))
            VoiceChoiceRow(label = stringResource(R.string.audio_source_title),
                options = listOf(
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
                ) + if (capability.providerRecognition?.supported == true || capability.providerRecognitionProfiles.isNotEmpty()) {
                    listOf(RadioOption(AudioSource.PROVIDER.value, stringResource(R.string.audio_settings_cloud_source)))
                } else emptyList(),
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
            if (voice.recognition.source == AudioSource.PROVIDER) {
                ProviderAudioControls(voice.recognition.cloud, capability.providerRecognition, capability.providerRecognitionProfiles,
                    onProviderSettings) { cloud -> onChange { it.copy(recognition = it.recognition.copy(cloud = cloud)) } }
            }
            if (voice.recognition.source == AudioSource.OFFLINE) {
                Text(stringResource(R.string.audio_offline_recognition_hint), color = t.text3, fontSize = 12.sp,
                    modifier = Modifier.padding(horizontal = 14.dp, vertical = 8.dp))
                val sttModels = GeneratedVoiceModelCatalog.all.filter { it.kind == com.lingxi.code.voice.offline.GeneratedModelKind.Stt }
                VoiceChoiceRow(label = stringResource(R.string.audio_offline_recognition_models),
                    options = listOf(
                        RadioOption("", "Language default model", "Select the first installed model for this language"),
                    ) + sttModels.map { model ->
                        RadioOption(
                            model.id,
                            model.localizedDisplayName(context.resources.configuration.locales[0].language),
                            model.languages.joinToString(", ") + " · " + stringResource(if (modelStates[model.id] == ModelState.Ready) R.string.audio_model_installed else R.string.audio_model_not_installed),
                        )
                    },
                    selected = voice.recognition.offlineModelId.orEmpty(),
                    onSelect = { modelId ->
                        onChange { current ->
                            current.copy(recognition = current.recognition.copy(offlineModelId = modelId.ifBlank { null }))
                        }
                    },
                )
            }
        }

        SettingsSection(
            label = stringResource(R.string.audio_settings_output),
            footer = stringResource(R.string.settings_voice_speak_footer),
        ) {
            val systemVoices = capability.systemVoiceOptions
            val offlineVoices = capability.offlineVoiceOptions
            VoiceChoiceRow(label = stringResource(R.string.audio_source_title),
                options = listOf(
                    RadioOption(AudioSource.AUTOMATIC.value, stringResource(R.string.settings_voice_mode_automatic), "Resolve system first, then an installed offline model"),
                    RadioOption(AudioSource.SYSTEM.value, stringResource(R.string.settings_voice_backend_system), stringResource(R.string.settings_voice_android_system_sub)),
                    RadioOption(AudioSource.OFFLINE.value, stringResource(R.string.settings_voice_mode_local_only), stringResource(R.string.settings_voice_mode_local_only_sub)),
                ) + if (capability.providerSpeech?.supported == true || capability.providerSpeechProfiles.isNotEmpty()) {
                    listOf(RadioOption(AudioSource.PROVIDER.value, stringResource(R.string.audio_settings_cloud_source)))
                } else emptyList(),
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
            if (voice.speech.source == AudioSource.PROVIDER) {
                ProviderAudioControls(voice.speech.cloud, capability.providerSpeech, capability.providerSpeechProfiles,
                    onProviderSettings) { cloud -> onChange { it.copy(speech = it.speech.copy(cloud = cloud, voice = null)) } }
            }
            if (voice.speech.source == AudioSource.OFFLINE) {
                Text(stringResource(R.string.audio_offline_speech_hint), color = t.text3, fontSize = 12.sp,
                    modifier = Modifier.padding(horizontal = 14.dp, vertical = 8.dp))
                val ttsModels = GeneratedVoiceModelCatalog.all.filter { it.kind == com.lingxi.code.voice.offline.GeneratedModelKind.Tts }
                VoiceChoiceRow(label = stringResource(R.string.audio_offline_speech_models),
                    options = listOf(
                        RadioOption("", "Language default model", "Select the first installed model for this language"),
                    ) + ttsModels.map { model ->
                        RadioOption(model.id, model.localizedDisplayName(context.resources.configuration.locales[0].language),
                            model.languages.joinToString(", ") + " · " + stringResource(if (modelStates[model.id] == ModelState.Ready) R.string.audio_model_installed else R.string.audio_model_not_installed))
                    },
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
            val voiceChoices = when (voice.speech.source) {
                AudioSource.SYSTEM -> systemVoices
                AudioSource.OFFLINE -> offlineVoices.filter {
                    voice.speech.offlineModelId == null || it.selection?.modelId == voice.speech.offlineModelId
                }
                AudioSource.PROVIDER -> capability.voiceOptions.filter { it.source == VoiceOptionSource.Provider }
                else -> emptyList()
            }
            VoiceChoiceRow(label = stringResource(R.string.voice_voice_name),
                options = listOf(
                    RadioOption("", stringResource(R.string.settings_voice_system_default), "Use the selected source's default voice"),
                ) + voiceChoices.map { option -> RadioOption(option.id, option.label, option.details) },
                selected = voice.speech.voice?.let { requested ->
                    voiceChoices.firstOrNull { it.selection == requested }?.id ?: "unavailable"
                } ?: "",
                onSelect = { key ->
                    val option = voiceChoices.firstOrNull { it.id == key }
                    if (option == null) {
                        onChange { current ->
                            current.copy(
                                speech = current.speech.copy(voice = null),
                            )
                        }
                    } else {
                        val selection = checkNotNull(option.selection)
                        onChange { current ->
                            current.copy(
                                speech = current.speech.copy(
                                    source = selection.source,
                                    offlineModelId = selection.modelId.takeIf { selection.source == AudioSource.OFFLINE },
                                    cloud = if (selection.source == AudioSource.PROVIDER) current.speech.cloud.copy(modelId = selection.modelId) else current.speech.cloud,
                                    voice = selection,
                                ),
                            )
                        }
                    }
                },
            )
        }

        SettingsSection(
            label = stringResource(R.string.settings_voice_playback_options),
            footer = stringResource(R.string.settings_voice_playback_footer),
        ) {
            if (voice.speech.source == AudioSource.PROVIDER) {
                SettingsRow(
                    label = stringResource(R.string.voice_speech_rate),
                    value = "1.0x",
                    sub = "Provider speech supports fixed 1.0x speed.",
                    chevron = false,
                )
                if (voice.rate != 1.0) {
                    SettingsRow(
                        label = "Reset speech speed to 1.0x",
                        sub = "Saved speed: ${String.format(Locale.US, "%.1fx", voice.rate)}. Reset to use provider speech.",
                        onTap = { onChange { current -> current.copy(rate = 1.0) } },
                    )
                }
            } else {
                SettingsRow(
                    label = stringResource(R.string.voice_speech_rate),
                    value = String.format(Locale.US, "%.1fx", voice.rate),
                    chevron = false,
                ) {
                    Slider(
                        value = voice.rate.toFloat(),
                        onValueChange = { v ->
                            onChange { current -> current.copy(rate = snapVoiceSpeed(v).toDouble()) }
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
            }
            SettingsRow(label = stringResource(R.string.voice_auto_play), chevron = false, isLast = true) {
                LXToggle(
                    checked = voice.autoPlayReplies,
                    onCheckedChange = { enabled -> onChange { current -> current.copy(autoPlayReplies = enabled) } },
                )
            }
        }

        VoiceSettingsDisclosure(title = stringResource(R.string.audio_settings_offline_models),
            initiallyOpen = voice.recognition.source == AudioSource.OFFLINE || voice.speech.source == AudioSource.OFFLINE) {
        SettingsSection(
            label = stringResource(R.string.settings_voice_model_section),
            footer = stringResource(R.string.audio_offline_models_hint),
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

        }
        VoiceSettingsDisclosure(title = stringResource(R.string.audio_settings_advanced),
            initiallyOpen = capability.microphonePermission != VoicePermissionStatus.Granted) {
        SettingsSection(label = "Voice conversation", footer = "Agent voice uses the current conversation, tools and permissions.") {
            RadioList(options = buildList {
                add(RadioOption("agent", "Agent conversation", "Recognize speech, run the current Agent, and speak its reply"))
                if (capability.realtimeConversationSupported) add(RadioOption("realtime", "Realtime conversation", "Native audio with the current Agent"))
            }, selected = voice.conversation.mode, onSelect = { mode -> onChange { it.copy(conversation = it.conversation.copy(mode = mode)) } })
            if (voice.conversation.mode == "realtime") {
                ProviderAudioControls(voice.conversation.cloud, capability.providerRealtime, capability.providerRealtimeProfiles,
                    onProviderSettings) { cloud -> onChange { it.copy(conversation = it.conversation.copy(cloud = cloud, voice = null)) } }
                val realtimeVoices = capability.providerRealtime?.voices.orEmpty().filter { it.selection.modelId == capability.providerRealtime?.modelId }
                if (realtimeVoices.isNotEmpty()) RadioList(options = listOf(RadioOption("", "Catalog default voice")) + realtimeVoices.map { RadioOption(it.selection.id, it.label) },
                    selected = voice.conversation.voice?.id.orEmpty(), onSelect = { id ->
                        onChange { it.copy(conversation = it.conversation.copy(voice = realtimeVoices.firstOrNull { it.selection.id == id }?.selection)) }
                    })
                RadioList(options = listOf(RadioOption("turn_based", "Take turns", "Release the microphone to commit speech; microphone pauses during playback")),
                    selected = voice.conversation.interaction, onSelect = { interaction -> onChange { it.copy(conversation = it.conversation.copy(interaction = interaction)) } })
            }
            SettingsRow(label = "Audio interaction", value = if (capability.fullDuplex) "Full duplex" else "Take turns",
                sub = if (capability.acousticEchoCancellation) "Acoustic echo cancellation available" else "Stop playback before microphone capture", chevron = false)
            if (voice.conversation.mode == "realtime" && !capability.realtimeConversationSupported) {
                SettingsRow(label = "Realtime unavailable", sub = capability.realtimeConversationReason,
                    onTap = { onChange { it.copy(conversation = it.conversation.copy(mode = "agent")) } }, isLast = true)
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
                sub = capability.blockingIssues.joinToString(" · ") { it.name }.ifBlank { allClearLabel },
                isLast = true,
                onTap = { context.startActivity(appSettingsIntent) },
            )
        }

        SettingsSection(label = stringResource(R.string.audio_settings_status)) {
            SettingsRow(
                label = stringResource(R.string.audio_settings_status),
                value = if (saving) stringResource(R.string.audio_settings_saving) else stringResource(R.string.audio_settings_saved),
                sub = when {
                    saving -> "Saving…"
                    saveError != null -> saveError
                    else -> "${stringResource(R.string.audio_settings_saved)} · v$revision"
                },
                chevron = false,
                isLast = true,
            )
        }
        }
    }
}

@Composable
private fun ProviderAudioControls(
    cloud: AudioCloudBinding, capability: ProviderAudioCapability?, profiles: List<ProviderAudioProfileOption>,
    onProviderSettings: () -> Unit, onChange: (AudioCloudBinding) -> Unit,
) {
    VoiceChoiceRow(label = stringResource(R.string.audio_settings_account), options = listOf(RadioOption("follow_session", stringResource(R.string.audio_settings_follow_session)),
        RadioOption("explicit_profile", stringResource(R.string.audio_settings_select_account))),
        selected = cloud.binding, onSelect = { onChange(cloud.copy(binding = it, modelId = null)) })
    if (cloud.binding == "explicit_profile") {
        VoiceChoiceRow(label = stringResource(R.string.audio_settings_account), options = profiles.map { RadioOption(it.profileId, it.label, null) },
            selected = cloud.profileId.orEmpty(), onSelect = { onChange(cloud.copy(profileId = it, modelId = null)) })
    }
    val models = capability?.modelIds.orEmpty()
    if (models.isNotEmpty()) VoiceChoiceRow(label = stringResource(R.string.audio_settings_model), options = listOf(RadioOption("", stringResource(R.string.audio_settings_default))) + models.filterNotNull().map { RadioOption(it, it) },
        selected = cloud.modelId.orEmpty(), onSelect = { onChange(cloud.copy(modelId = it.ifBlank { null })) })
    SettingsRow(label = stringResource(R.string.audio_settings_account),
        value = stringResource(if (capability?.readiness == "ready") R.string.audio_settings_connected else R.string.audio_guide_configuration_title), onTap = onProviderSettings, isLast = true)
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
private fun audioSourceLabel(source: AudioSource): String = when (source) {
    AudioSource.AUTOMATIC -> stringResource(R.string.settings_voice_mode_automatic)
    AudioSource.SYSTEM -> stringResource(R.string.settings_voice_backend_system)
    AudioSource.OFFLINE -> stringResource(R.string.settings_voice_backend_sherpa)
    else -> source.value
}

@Composable
private fun readinessLabel(readiness: AudioReadiness): String = when (readiness) {
    AudioReadiness.AVAILABLE -> stringResource(R.string.settings_status_on)
    AudioReadiness.PERMISSION_REQUIRED -> stringResource(R.string.voice_permission_denied_label)
    AudioReadiness.DENIED -> stringResource(R.string.voice_permission_denied_label)
    AudioReadiness.UNAVAILABLE -> stringResource(R.string.settings_voice_unavailable_short)
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
    com.lingxi.code.voice.offline.GeneratedVoiceModelCatalog.packFor(language)
        .forEach(VoiceModelDownloader::cancel)
}
