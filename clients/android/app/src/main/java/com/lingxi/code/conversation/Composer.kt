package com.lingxi.code.conversation

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.IconButton
import androidx.compose.material3.LocalTextStyle
import androidx.compose.material3.Text
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.ImageBitmap
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.semantics.clearAndSetSemantics
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.onClick
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.foundation.Image
import androidx.compose.foundation.layout.height
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.stringResource
import com.lingxi.code.R
import com.lingxi.code.components.LXIcon
import com.lingxi.code.components.LXIconName
import com.lingxi.code.components.ModelDetailsDialog
import com.lingxi.code.components.ModelDetailsInfoButton
import com.lingxi.code.components.UiTags
import com.lingxi.code.components.tint
import com.lingxi.code.bindings.ImageRefDto
import com.lingxi.code.model.CatalogModelDetails
import com.lingxi.code.model.EngineModelCatalog
import com.lingxi.code.model.ModelOption
import com.lingxi.code.model.ModelProviderStatus
import com.lingxi.code.settings.ModelRecentsStore
import com.lingxi.code.theme.LingXiTheme
import com.lingxi.code.voice.voiceHold

/**
 * The bottom composer pill — ported from the iOS `Composer`.
 *
 * A rounded surface holding a multi-line text field, an attach (+) button, a
 * [ModelChip] dropdown, and a trailing action that swaps between a mic (when
 * empty) and an accent send button (when there is text). Send is hoisted via
 * [onSend]; the selected [model] and [onModelChange] keep model selection in the
 * caller's state. [onMicLongPress] hooks the future voice-flow overlay.
 *
 * @param text current composer text (hoisted)
 * @param onTextChange edit callback
 * @param onMicHoldStart fired when the mic is held past the voice-flow threshold
 *   (opens the immersive [VoiceFlowOverlay]).
 * @param onMicHoldRelease fired when the held finger lifts (dismisses / sends).
 */
@Composable
fun Composer(
    text: String,
    onTextChange: (String) -> Unit,
    model: ModelOption,
    onModelChange: (ModelOption) -> Unit,
    onSend: () -> Unit,
    modifier: Modifier = Modifier,
    /** The real engine catalog the model chip's dropdown shows. */
    availableModels: List<ModelOption> = emptyList(),
    onMicClick: () -> Unit = {},
    onMicHoldStart: () -> Unit = {},
    onMicHoldRelease: () -> Unit = {},
    onFlowModeClick: () -> Unit = {},
    flowModeActive: Boolean = false,
    onCameraClick: () -> Unit = {},
    attachment: ComposerAttachment? = null,
    onRemoveAttachment: () -> Unit = {},
    /** True while a turn streams — the trailing action becomes a Stop button. */
    isStreaming: Boolean = false,
    /** False until the visible engine session has been confirmed. */
    enabled: Boolean = true,
    /** True when no enabled Provider/model choice has been persisted yet. */
    modelSetupRequired: Boolean = false,
    /** Opens the LLM Provider settings page from the chip or send action. */
    onOpenModelSettings: () -> Unit = {},
    /** Current settings/credential state keyed by the engine provider profile. */
    modelProviderStatuses: List<ModelProviderStatus> = emptyList(),
    /** Opens a specific provider editor, or the provider list when id is null. */
    onOpenProviderSettings: (String?) -> Unit = { onOpenModelSettings() },
    /** Fired by the Stop button to cancel the in-flight turn. */
    onStop: () -> Unit = {},
    /** True when a recovered WaitingForUser turn can be discarded. */
    showDiscardRecovery: Boolean = false,
    /** Discards the recovered turn after the host confirms cancellation. */
    onDiscardRecovery: () -> Unit = {},
) {
    val t = LingXiTheme.palette
    val hasText = text.trim().isNotEmpty()
    val shape = RoundedCornerShape(22.dp)

    Column(
        modifier = modifier
            .fillMaxWidth()
            .padding(horizontal = 14.dp)
            .padding(top = 8.dp, bottom = 4.dp),
    ) {
        Column(
            modifier = Modifier
                .clip(shape)
                .background(t.composerBg)
                .border(0.5.dp, t.borderStrong, shape)
                .padding(horizontal = 12.dp)
                .padding(top = 10.dp, bottom = 8.dp),
            verticalArrangement = Arrangement.spacedBy(4.dp),
        ) {
            // Captured-photo attachment chip (the device-vision analog of how a
            // voice transcript lands in the draft): a thumbnail + remove button,
            // shown only once a camera capture has surfaced an image.
            if (attachment != null) {
                AttachmentThumb(attachment = attachment, onRemove = onRemoveAttachment)
            }

            // Text field (1..5 lines), brand-styled with a placeholder.
            Box(modifier = Modifier.padding(horizontal = 4.dp, vertical = 2.dp)) {
                if (text.isEmpty()) {
                    Text(
                        text = stringResource(R.string.composer_placeholder),
                        color = t.text4,
                        fontSize = 15.5f.sp,
                    )
                }
                BasicTextField(
                    value = text,
                    onValueChange = onTextChange,
                    enabled = enabled,
                    textStyle = LocalTextStyle.current.merge(
                        TextStyle(color = t.text, fontSize = 15.5f.sp),
                    ),
                    cursorBrush = SolidColor(t.accent),
                    maxLines = 5,
                    keyboardActions = androidx.compose.foundation.text.KeyboardActions(
                        // Don't start a second turn from the IME Send key while one
                        // is already streaming (the VM also guards this).
                        onSend = {
                            if (enabled && !isStreaming) {
                                if (modelSetupRequired) onOpenModelSettings() else onSend()
                            }
                        },
                    ),
                    keyboardOptions = androidx.compose.foundation.text.KeyboardOptions(
                        imeAction = ImeAction.Send,
                    ),
                    modifier = Modifier.fillMaxWidth(),
                )
            }

            // Action row: attach · model chip · spacer · mic|send.
            Row(verticalAlignment = Alignment.CenterVertically) {
                IconHit(onClick = {}) {
                    LXIcon(
                        name = LXIconName.Plus,
                        size = 18.dp,
                        color = t.text3,
                        stroke = 1.8f,
                        contentDescription = stringResource(R.string.composer_more),
                    )
                }
                // Camera affordance: tap drives a real on-device capture through
                // the same CameraController the engine bridges onto
                // `traits::CameraControl`; the result surfaces as the attachment
                // chip above (mirrors the mic → transcript surfacing).
                IconHit(
                    onClick = { if (enabled) onCameraClick() },
                    modifier = Modifier.testTag(UiTags.COMPOSER_CAMERA),
                ) {
                    LXIcon(
                        name = LXIconName.Paperclip,
                        size = 18.dp,
                        color = t.text3,
                        stroke = 1.8f,
                        contentDescription = stringResource(R.string.composer_camera),
                    )
                }
                ModelChip(
                    model = model,
                    models = availableModels,
                    modelSetupRequired = modelSetupRequired,
                    onOpenModelSettings = onOpenModelSettings,
                    providerStatuses = modelProviderStatuses,
                    onOpenProviderSettings = onOpenProviderSettings,
                    onModelChange = onModelChange,
                )
                Spacer(Modifier.weight(1f))
                val stopGeneratingDescription = stringResource(R.string.composer_stop_generating)
                val sendDescription = stringResource(R.string.composer_send)
                val recordDescription = stringResource(R.string.composer_record)
                val startRecordingLabel = stringResource(R.string.composer_start_recording)
                val flowModeOnDescription = stringResource(R.string.voice_close_flow_mode_a11y)
                val flowModeOffDescription = stringResource(R.string.composer_flow_mode)
                when {
                    // Streaming: the send action becomes a Stop button that
                    // cancels the in-flight turn (a filled square — the universal
                    // stop glyph — inside the accent slot).
                    isStreaming -> Box(
                        modifier = Modifier
                            .size(34.dp)
                            .clip(RoundedCornerShape(10.dp))
                            .background(t.accent)
                            .clickable(onClick = onStop)
                            .testTag(UiTags.COMPOSER_STOP),
                        contentAlignment = Alignment.Center,
                    ) {
                        Box(
                            modifier = Modifier
                                .size(12.dp)
                                .clip(RoundedCornerShape(3.dp))
                                .background(Color.White)
                                .clearAndSetSemantics { contentDescription = stopGeneratingDescription },
                        )
                    }
                    // A recovered WaitingForUser checkpoint has no local
                    // executor, but still owns the durable session identity.
                    // Keep the composer action visible so the user can submit
                    // the correlated discard and wait for terminal confirmation.
                    showDiscardRecovery -> Box(
                        modifier = Modifier
                            .size(34.dp)
                            .clip(RoundedCornerShape(10.dp))
                            .background(t.accent)
                            .clickable(onClick = onDiscardRecovery)
                            .testTag(UiTags.COMPOSER_DISCARD),
                        contentAlignment = Alignment.Center,
                    ) {
                        Box(
                            modifier = Modifier
                                .size(12.dp)
                                .clip(RoundedCornerShape(3.dp))
                                .background(Color.White)
                                .clearAndSetSemantics {
                                    contentDescription = stopGeneratingDescription
                                },
                        )
                    }
                    // Idle with text: the accent Send button.
                    hasText -> Box(
                        modifier = Modifier
                            .size(34.dp)
                            .clip(RoundedCornerShape(10.dp))
                            .background(t.accent)
                            .clickable(enabled = enabled) {
                                if (modelSetupRequired) onOpenModelSettings() else onSend()
                            }
                            .testTag(UiTags.COMPOSER_SEND),
                        contentAlignment = Alignment.Center,
                    ) {
                        LXIcon(name = LXIconName.ArrowUp, size = 16.dp, color = Color.White, contentDescription = sendDescription)
                    }
                    // Idle, empty: separate ordinary recording and Flow Mode.
                    else -> Row(
                        horizontalArrangement = Arrangement.spacedBy(6.dp),
                        verticalAlignment = Alignment.CenterVertically,
                    ) {
                        Box(
                            modifier = Modifier
                                .size(34.dp)
                                .voiceHold(
                                    onTap = { if (enabled) onMicClick() },
                                    onStart = { if (enabled) onMicHoldStart() },
                                    onRelease = { if (enabled) onMicHoldRelease() },
                                )
                                .semantics {
                                    contentDescription = recordDescription
                                    onClick(label = startRecordingLabel) {
                                        if (enabled) onMicClick()
                                        enabled
                                    }
                                },
                            contentAlignment = Alignment.Center,
                        ) {
                            LXIcon(name = LXIconName.Mic, size = 18.dp, color = t.text2, stroke = 1.8f)
                        }
                        Box(
                            modifier = Modifier
                                .size(40.dp)
                                .clip(CircleShape)
                                .background(if (flowModeActive) t.accent else t.text)
                                .clickable(enabled = enabled, onClick = onFlowModeClick)
                                .semantics {
                                    contentDescription = if (flowModeActive) {
                                        flowModeOnDescription
                                    } else {
                                        flowModeOffDescription
                                    }
                                },
                            contentAlignment = Alignment.Center,
                        ) {
                            LXIcon(
                                name = LXIconName.AudioWave,
                                size = 18.dp,
                                color = if (flowModeActive) Color.White else t.windowBg,
                                stroke = 1.65f,
                            )
                        }
                    }
                }
            }
        }
    }
}

/**
 * A photo the user captured via the composer's camera affordance, ready to send.
 *
 * [thumb] is the decoded preview the composer renders; [width]/[height] carry the
 * source dimensions surfaced by the device-vision capture (the `CapturedImageFfi`
 * carrier the engine bridges onto `traits::CapturedImage`).
 */
data class ComposerAttachment(
    val thumb: ImageBitmap,
    val width: Int,
    val height: Int,
    val mediaType: String = "image/jpeg",
    val base64: String = "",
)

/** Convert the reviewed attachment into the exact shared SendPrompt DTO. */
fun ComposerAttachment.toImageRef(): ImageRefDto = ImageRefDto(
    mediaType = mediaType,
    base64 = base64,
)

/** A captured-photo thumbnail chip with a remove (×) affordance. */
@Composable
private fun AttachmentThumb(attachment: ComposerAttachment, onRemove: () -> Unit) {
    val t = LingXiTheme.palette
    Row(
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(8.dp),
        modifier = Modifier.padding(horizontal = 4.dp, vertical = 2.dp),
    ) {
        Image(
            bitmap = attachment.thumb,
            contentDescription = stringResource(R.string.composer_captured_photo),
            contentScale = ContentScale.Crop,
            modifier = Modifier
                .size(48.dp)
                .clip(RoundedCornerShape(10.dp)),
        )
        Text(
            text = "${attachment.width}×${attachment.height}",
            color = t.text3,
            fontSize = 12.sp,
        )
        Spacer(Modifier.weight(1f))
        Box(
            modifier = Modifier
                .size(28.dp)
                .clip(RoundedCornerShape(8.dp))
                .clickable(onClick = onRemove),
            contentAlignment = Alignment.Center,
        ) {
            LXIcon(
                name = LXIconName.X,
                size = 14.dp,
                color = t.text3,
                stroke = 2f,
                contentDescription = stringResource(R.string.composer_remove_photo),
            )
        }
    }
}

/** 34×34 tappable icon slot (the iOS `Button { … }.frame(34,34)` idiom). */
@Composable
private fun IconHit(
    onClick: () -> Unit,
    modifier: Modifier = Modifier,
    content: @Composable () -> Unit,
) {
    Box(
        modifier = modifier.size(34.dp).clickable(onClick = onClick),
        contentAlignment = Alignment.Center,
    ) { content() }
}

/**
 * Model-selector chip + anchored dropdown menu. The engine has already curated
 * the latest common models; this menu groups exactly those rows by provider and
 * keeps the active provider-qualified id highlighted.
 */
@Composable
private fun ModelChip(
    model: ModelOption,
    models: List<ModelOption>,
    modelSetupRequired: Boolean,
    onOpenModelSettings: () -> Unit,
    providerStatuses: List<ModelProviderStatus>,
    onOpenProviderSettings: (String?) -> Unit,
    onModelChange: (ModelOption) -> Unit,
) {
    val t = LingXiTheme.palette
    val context = LocalContext.current
    var open by remember { mutableStateOf(false) }
    var query by remember { mutableStateOf("") }
    var selectedDetails by remember { mutableStateOf<CatalogModelDetails?>(null) }
    val chipShape = RoundedCornerShape(8.dp)
    val filteredModels = remember(models, query) { EngineModelCatalog.filter(models, query) }
    val groups = remember(filteredModels) { EngineModelCatalog.groups(filteredModels) }
    val recentsStore = remember(context) { ModelRecentsStore(context) }
    // Re-read on every open rather than once: a pick made in this composition
    // must be at the top the next time the menu is shown.
    var recentRefs by remember { mutableStateOf(recentsStore.references()) }
    val recents = remember(filteredModels, recentRefs) {
        EngineModelCatalog.recents(filteredModels, recentRefs)
    }

    LaunchedEffect(open) {
        if (open) {
            recentRefs = recentsStore.references()
        } else {
            query = ""
        }
    }

    Box {
        Row(
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(5.dp),
            modifier = Modifier
                .clip(chipShape)
                .background(if (open) t.surfaceHover else Color.Transparent)
                .clickable {
                    if (modelSetupRequired) {
                        onOpenModelSettings()
                    } else if (models.isNotEmpty()) {
                        open = true
                    }
                }
                .then(
                    if (modelSetupRequired) Modifier.testTag(UiTags.MODEL_SETUP_CHIP)
                    else Modifier.testTag(UiTags.MODEL_PICKER_CHIP),
                )
                .padding(horizontal = 9.dp, vertical = 5.dp),
        ) {
            if (modelSetupRequired) {
                LXIcon(name = LXIconName.Cog, size = 13.dp, color = t.accent, stroke = 1.9f)
            } else {
                Dot(color = model.color, size = 6.dp)
            }
            Text(
                text = if (modelSetupRequired) stringResource(R.string.composer_setup_model) else model.shortName,
                color = if (modelSetupRequired) t.accent else t.text2,
                fontSize = 12.sp,
                fontWeight = FontWeight.Medium,
            )
            LXIcon(
                name = if (modelSetupRequired) LXIconName.ChevronR else LXIconName.Chevron,
                size = 11.dp,
                color = if (modelSetupRequired) t.accent else t.text4,
                stroke = 2f,
            )
        }

        DropdownMenu(
            expanded = open,
            onDismissRequest = { open = false },
            containerColor = t.surface,
            modifier = Modifier
                .width(330.dp)
                .heightIn(max = 600.dp),
        ) {
            ModelSearchField(
                query = query,
                onQueryChange = { query = it },
            )
            if (recents.isNotEmpty()) {
                Text(
                    text = stringResource(R.string.composer_recent_models),
                    color = t.text3,
                    fontSize = 11.sp,
                    fontWeight = FontWeight.SemiBold,
                    modifier = Modifier
                        .padding(start = 15.dp, end = 15.dp, top = 8.dp, bottom = 4.dp)
                        .fillMaxWidth(),
                )
                recents.forEach { m ->
                    ModelPickerRow(
                        model = m,
                        active = m.id == model.id,
                        providerStatuses = providerStatuses,
                        onShowDetails = { selectedDetails = it },
                        onSelected = {
                            open = false
                            recentsStore.record(m.id)
                            recentRefs = recentsStore.references()
                            onModelChange(m)
                        },
                        onOpenProviderSettings = { settingsId ->
                            open = false
                            onOpenProviderSettings(settingsId)
                        },
                    )
                }
            }
            groups.forEachIndexed { groupIndex, group ->
                val providerStatus = EngineModelCatalog.providerStatus(group.id, providerStatuses)
                val providerLabel = providerStatus?.displayLabel ?: stringResource(R.string.settings_provider_unconfigured)
                val providerColor = providerStatus?.status?.dot(t) ?: t.text4
                Row(
                    verticalAlignment = Alignment.CenterVertically,
                    modifier = Modifier.padding(
                        start = 15.dp,
                        end = 15.dp,
                        // The tighter first-group inset only applies when this
                        // header really is the first thing under the search
                        // field — the recents group can now precede it.
                        top = if (groupIndex == 0 && recents.isEmpty()) 8.dp else 12.dp,
                        bottom = 4.dp,
                    ).fillMaxWidth(),
                ) {
                    Text(
                        text = group.name,
                        color = t.text3,
                        fontSize = 11.sp,
                        fontWeight = FontWeight.SemiBold,
                        modifier = Modifier.weight(1f),
                    )
                    Dot(color = providerColor, size = 6.dp)
                    Spacer(Modifier.width(5.dp))
                    Text(
                        text = providerLabel,
                        color = providerColor,
                        fontSize = 10.sp,
                        fontWeight = FontWeight.Medium,
                    )
                }
                group.models.forEach { m ->
                    ModelPickerRow(
                        model = m,
                        active = m.id == model.id,
                        providerStatuses = providerStatuses,
                        onShowDetails = { selectedDetails = it },
                        onSelected = {
                            open = false
                            recentsStore.record(m.id)
                            recentRefs = recentsStore.references()
                            onModelChange(m)
                        },
                        onOpenProviderSettings = { settingsId ->
                            open = false
                            onOpenProviderSettings(settingsId)
                        },
                    )
                }
            }
            if (groups.isEmpty() && recents.isEmpty()) {
                Text(
                    text = stringResource(R.string.composer_no_matching_models),
                    color = t.text3,
                    fontSize = 12.sp,
                    modifier = Modifier.padding(horizontal = 16.dp, vertical = 18.dp),
                )
            }
        }
        selectedDetails?.let { details ->
            ModelDetailsDialog(details = details, onDismiss = { selectedDetails = null })
        }
    }
}

/**
 * One selectable model row. Shared by the "recently used" group and the
 * per-provider groups so the two cannot drift apart; the provider connection
 * state is resolved from the model's OWN provider, which the recents group needs
 * because its rows span several providers.
 */
@Composable
private fun ModelPickerRow(
    model: ModelOption,
    active: Boolean,
    providerStatuses: List<ModelProviderStatus>,
    onShowDetails: (CatalogModelDetails) -> Unit,
    onSelected: () -> Unit,
    onOpenProviderSettings: (String?) -> Unit,
) {
    val t = LingXiTheme.palette
    val providerStatus = EngineModelCatalog.providerStatus(model.providerId, providerStatuses)
    val canSelect = providerStatus?.canSelect == true
    Row(
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(10.dp),
        modifier = Modifier
            .fillMaxWidth()
            .padding(horizontal = 5.dp, vertical = 2.dp)
            .clip(RoundedCornerShape(8.dp))
            .background(if (active) t.accent.tint(0.15f) else Color.Transparent)
            .clickable {
                if (canSelect) onSelected() else onOpenProviderSettings(providerStatus?.settingsId)
            }
            .padding(horizontal = 10.dp, vertical = 8.dp),
    ) {
        Dot(color = model.color, size = 8.dp)
        Column(Modifier.weight(1f)) {
            Text(
                text = model.name,
                color = if (canSelect) t.text else t.text2,
                fontSize = 13.sp,
                fontWeight = FontWeight.Medium,
            )
            Text(
                text = model.desc,
                color = t.text3,
                fontSize = 10.5f.sp,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
            if (model.metadata.summaryItems.isNotEmpty()) {
                Text(
                    text = model.metadata.summaryItems.joinToString(" · "),
                    color = t.text3,
                    fontSize = 9.5f.sp,
                    lineHeight = 13.sp,
                    maxLines = 2,
                    overflow = TextOverflow.Ellipsis,
                )
            }
        }
        model.details?.let { details ->
            ModelDetailsInfoButton(onClick = { onShowDetails(details) }, modifier = Modifier.size(32.dp))
        }
        when {
            !canSelect -> Row(
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(2.dp),
            ) {
                Text(stringResource(R.string.composer_configure_button), color = t.accent, fontSize = 10.sp)
                LXIcon(
                    name = LXIconName.ChevronR,
                    size = 10.dp,
                    color = t.accent,
                    stroke = 2f,
                    contentDescription = stringResource(
                        R.string.composer_configure_provider,
                        model.providerName,
                    ),
                )
            }
            active -> LXIcon(
                name = LXIconName.Check,
                size = 13.dp,
                color = t.accent,
                stroke = 2f,
                contentDescription = stringResource(R.string.composer_current_model),
            )
        }
    }
}

@Composable
private fun ModelSearchField(
    query: String,
    onQueryChange: (String) -> Unit,
) {
    val t = LingXiTheme.palette
    Row(
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(8.dp),
        modifier = Modifier
            .fillMaxWidth()
            .padding(horizontal = 10.dp, vertical = 8.dp)
            .clip(RoundedCornerShape(9.dp))
            .background(t.surfaceHover)
            .padding(horizontal = 10.dp, vertical = 8.dp),
    ) {
        LXIcon(
            name = LXIconName.Search,
            size = 14.dp,
            color = t.text3,
            stroke = 1.8f,
            contentDescription = stringResource(R.string.composer_search_models_description),
        )
        Box(Modifier.weight(1f)) {
            if (query.isEmpty()) {
                Text(stringResource(R.string.composer_search_models_placeholder), color = t.text4, fontSize = 12.sp)
            }
            BasicTextField(
                value = query,
                onValueChange = onQueryChange,
                singleLine = true,
                textStyle = LocalTextStyle.current.merge(
                    TextStyle(color = t.text, fontSize = 12.sp),
                ),
                cursorBrush = SolidColor(t.accent),
                modifier = Modifier
                    .fillMaxWidth()
                    .testTag(UiTags.MODEL_PICKER_SEARCH),
            )
        }
    }
}

@Composable
private fun Dot(color: Color, size: androidx.compose.ui.unit.Dp) {
    Box(
        modifier = Modifier
            .size(size)
            .clip(androidx.compose.foundation.shape.CircleShape)
            .background(color),
    )
}
