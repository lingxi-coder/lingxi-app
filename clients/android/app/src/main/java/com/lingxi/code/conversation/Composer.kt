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
import androidx.compose.ui.platform.testTag
import com.lingxi.code.components.LXIcon
import com.lingxi.code.components.LXIconName
import com.lingxi.code.components.UiTags
import com.lingxi.code.components.tint
import com.lingxi.code.model.EngineModelCatalog
import com.lingxi.code.model.ModelOption
import com.lingxi.code.model.ModelProviderStatus
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
                        text = "向灵犀提问…",
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
                    LXIcon(name = LXIconName.Plus, size = 18.dp, color = t.text3, stroke = 1.8f, contentDescription = "更多")
                }
                // Camera affordance: tap drives a real on-device capture through
                // the same CameraController the engine bridges onto
                // `traits::CameraControl`; the result surfaces as the attachment
                // chip above (mirrors the mic → transcript surfacing).
                IconHit(
                    onClick = { if (enabled) onCameraClick() },
                    modifier = Modifier.testTag(UiTags.COMPOSER_CAMERA),
                ) {
                    LXIcon(name = LXIconName.Paperclip, size = 18.dp, color = t.text3, stroke = 1.8f, contentDescription = "拍照")
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
                                .clearAndSetSemantics { contentDescription = "停止生成" },
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
                        LXIcon(name = LXIconName.ArrowUp, size = 16.dp, color = Color.White, contentDescription = "发送")
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
                                    contentDescription = "普通录音"
                                    onClick(label = "开始普通录音") {
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
                                    contentDescription = if (flowModeActive) "关闭心流模式" else "开启心流模式"
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
            contentDescription = "已拍摄的照片",
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
            LXIcon(name = LXIconName.X, size = 14.dp, color = t.text3, stroke = 2f, contentDescription = "移除照片")
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
    var open by remember { mutableStateOf(false) }
    var query by remember { mutableStateOf("") }
    val chipShape = RoundedCornerShape(8.dp)
    val filteredModels = remember(models, query) { EngineModelCatalog.filter(models, query) }
    val groups = remember(filteredModels) { EngineModelCatalog.groups(filteredModels) }

    LaunchedEffect(open) {
        if (!open) query = ""
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
                text = if (modelSetupRequired) "设置模型" else model.shortName,
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
            groups.forEachIndexed { groupIndex, group ->
                val providerStatus = EngineModelCatalog.providerStatus(group.id, providerStatuses)
                val providerLabel = providerStatus?.displayLabel ?: "未配置"
                val providerColor = providerStatus?.status?.dot(t) ?: t.text4
                Row(
                    verticalAlignment = Alignment.CenterVertically,
                    modifier = Modifier.padding(
                        start = 15.dp,
                        end = 15.dp,
                        top = if (groupIndex == 0) 8.dp else 12.dp,
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
                    val active = m.id == model.id
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
                                open = false
                                if (canSelect) {
                                    onModelChange(m)
                                } else {
                                    onOpenProviderSettings(providerStatus?.settingsId)
                                }
                            }
                            .padding(horizontal = 10.dp, vertical = 8.dp),
                    ) {
                        Dot(color = m.color, size = 8.dp)
                        Column(Modifier.weight(1f)) {
                            Text(
                                text = m.name,
                                color = if (canSelect) t.text else t.text2,
                                fontSize = 13.sp,
                                fontWeight = FontWeight.Medium,
                            )
                            Text(
                                text = m.desc,
                                color = t.text3,
                                fontSize = 10.5f.sp,
                                maxLines = 1,
                                overflow = TextOverflow.Ellipsis,
                            )
                            if (m.metadata.summaryItems.isNotEmpty()) {
                                Text(
                                    text = m.metadata.summaryItems.joinToString(" · "),
                                    color = t.text3,
                                    fontSize = 9.5f.sp,
                                    lineHeight = 13.sp,
                                    maxLines = 2,
                                    overflow = TextOverflow.Ellipsis,
                                )
                            }
                        }
                        when {
                            !canSelect -> Row(
                                verticalAlignment = Alignment.CenterVertically,
                                horizontalArrangement = Arrangement.spacedBy(2.dp),
                            ) {
                                Text("配置", color = t.accent, fontSize = 10.sp)
                                LXIcon(
                                    name = LXIconName.ChevronR,
                                    size = 10.dp,
                                    color = t.accent,
                                    stroke = 2f,
                                    contentDescription = "配置 ${group.name}",
                                )
                            }
                            active -> LXIcon(
                                name = LXIconName.Check,
                                size = 13.dp,
                                color = t.accent,
                                stroke = 2f,
                                contentDescription = "当前模型",
                            )
                        }
                    }
                }
            }
            if (groups.isEmpty()) {
                Text(
                    text = "没有匹配的模型",
                    color = t.text3,
                    fontSize = 12.sp,
                    modifier = Modifier.padding(horizontal = 16.dp, vertical = 18.dp),
                )
            }
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
            contentDescription = "搜索模型",
        )
        Box(Modifier.weight(1f)) {
            if (query.isEmpty()) {
                Text("搜索模型、Provider 或规格", color = t.text4, fontSize = 12.sp)
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
