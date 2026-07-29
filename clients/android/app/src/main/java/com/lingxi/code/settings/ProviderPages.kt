package com.lingxi.code.settings

import androidx.compose.animation.core.RepeatMode
import androidx.compose.animation.core.animateFloat
import androidx.compose.animation.core.infiniteRepeatable
import androidx.compose.animation.core.rememberInfiniteTransition
import androidx.compose.animation.core.tween
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
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.scale
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.text.input.VisualTransformation
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.lingxi.code.components.LXIcon
import com.lingxi.code.components.LXIconName
import com.lingxi.code.components.mix
import com.lingxi.code.components.tint
import com.lingxi.code.model.ConnStatus
import com.lingxi.code.model.GenericProvider
import com.lingxi.code.model.ProviderKind
import com.lingxi.code.model.ProviderPreset
import com.lingxi.code.theme.LXFont
import com.lingxi.code.theme.LingXiTheme

/**
 * The three provider flows (LLM / web search / web fetch), ported 1:1 from the
 * iOS `ProviderPages.swift`. All three share the list → preset-picker → edit
 * pattern over the same [SettingsStore]; [ProviderKind] selects the dataset and
 * copy. LLM provider credentials persist through the engine's shared secure
 * store; non-secret row config persists locally in Android storage.
 *
 * Navigation note — the iOS `host.replaceTopTwo(with: [.list, .edit])` after
 * picking a preset (so the picker is popped and the edit page sits atop the
 * list) maps to a Navigation-Compose navigate-with-popUpTo: see [pickPreset].
 */

private fun ProviderKind.blurb(): String = when (this) {
    ProviderKind.Llm -> "灵犀本身不调用云端 LLM — 由你添加的提供商完成推理。密钥仅本机加密。"
    ProviderKind.Search -> "让 AI 接入实时网页搜索 — 选择一个搜索提供商以启用\"联网\"模式。"
    ProviderKind.Fetch -> "抓取网页正文用于阅读、摘要、引用 — 推荐 Jina Reader（免费）。"
}

/** Resolve the preset backing a provider (custom is the fallback for LLM). */
private fun ProviderKind.preset(of: GenericProvider): ProviderPreset =
    presets.firstOrNull { it.id == of.preset } ?: presets.last()

internal data class ProviderApplyUiState(
    val label: String,
    val enabled: Boolean,
    val saveCredential: Boolean,
)

internal fun providerApplyUiState(
    hasPendingConfiguration: Boolean,
    hasCredentialDraft: Boolean,
    busy: Boolean,
): ProviderApplyUiState = ProviderApplyUiState(
    label = when {
        busy -> "应用中…"
        hasCredentialDraft -> "保存并应用"
        hasPendingConfiguration -> "应用"
        else -> "已应用"
    },
    enabled = !busy && (hasCredentialDraft || hasPendingConfiguration),
    saveCredential = hasCredentialDraft,
)

// MARK: - Provider list ------------------------------------------------------

/**
 * The "已添加" provider list for a [kind]: a status dot + current model + default
 * badge per row and a dashed add button. Mobile does not yet wire smart-routing
 * or a configurable streaming default into the engine launch contract, so those
 * controls are rendered as explicitly unavailable instead of fake toggles.
 */
@Composable
fun ProviderListPage(
    kind: ProviderKind,
    state: SettingsUiState,
    store: SettingsStore,
    onEdit: (id: String) -> Unit,
    onAdd: () -> Unit,
    onReconnectEngine: () -> Unit = {},
) {
    val t = LingXiTheme.palette
    val arr = state.providers(kind)

    Column(Modifier.fillMaxWidth()) {
        Blurb(kind.blurb())

        SettingsSection(label = "已添加 · ${arr.size}") {
            if (arr.isEmpty()) {
                Text(
                    "尚未添加任何提供商",
                    color = t.text4,
                    fontSize = 13.sp,
                    modifier = Modifier.fillMaxWidth().padding(vertical = 24.dp),
                    textAlign = androidx.compose.ui.text.style.TextAlign.Center,
                )
            }
            arr.forEachIndexed { i, p ->
                val preset = kind.preset(of = p)
                ProviderRow(
                    provider = p,
                    preset = preset,
                    isLast = i == arr.size - 1,
                    onTap = { onEdit(p.id) },
                )
            }
        }

        DashedAddButton(title = "添加${kind.title}", onClick = onAdd)

        if (kind == ProviderKind.Llm) {
            if (state.pendingLlmProviderChanges.isNotEmpty()) {
                Column(Modifier.padding(top = 22.dp)) {
                    SettingsSection(
                        label = "待应用",
                        footer = "URL、模型、启用状态或默认提供商已写入本机配置；重连后才会用于新请求。",
                    ) {
                        SettingsRow(
                            icon = LXIconName.Workflow,
                            iconColor = t.accent,
                            label = "提供商配置有更改",
                            sub = "${state.pendingLlmProviderChanges.size} 项待应用",
                            chevron = false,
                            isLast = true,
                            trailing = {
                                ProviderTextAction(
                                    label = "应用并重连",
                                    enabled = true,
                                    onClick = {
                                        onReconnectEngine()
                                        store.markLlmConfigurationApplied()
                                    },
                                )
                            },
                        )
                    }
                }
            }
            Column(Modifier.padding(top = 22.dp)) {
                SettingsSection(
                    label = "高级",
                    footer = "当前移动引擎尚未提供这两项可配置能力；设置页不会保存无效开关。",
                ) {
                    SettingsRow(
                        icon = LXIconName.Sparkle, iconColor = t.accent,
                        label = "智能路由", sub = "尚未接入移动引擎", chevron = false,
                        trailing = {
                            Text("不可用", color = t.text4, fontSize = 12.sp)
                        },
                    )
                    SettingsRow(
                        label = "默认流式策略",
                        sub = "当前由移动引擎固定管理",
                        icon = LXIconName.Workflow,
                        chevron = false,
                        isLast = true,
                        trailing = {
                            Text("不可配置", color = t.text4, fontSize = 12.sp)
                        },
                    )
                }
            }
        }
    }
}

/** One row of the provider list: icon tile, name + badges, status dot + model. */
@Composable
private fun ProviderRow(
    provider: GenericProvider,
    preset: ProviderPreset,
    isLast: Boolean,
    onTap: () -> Unit,
) {
    val t = LingXiTheme.palette
    Column(Modifier.fillMaxWidth()) {
        Row(
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(12.dp),
            modifier = Modifier
                .fillMaxWidth()
                .clickable(onClick = onTap)
                .padding(horizontal = 14.dp, vertical = 12.dp),
        ) {
            PresetTile(preset = preset, size = 28.dp, fontSize = 13.sp)
            Column(modifier = Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(3.dp)) {
                Row(
                    verticalAlignment = Alignment.CenterVertically,
                    horizontalArrangement = Arrangement.spacedBy(6.dp),
                ) {
                    Text(
                        provider.name, color = t.text, fontSize = 14.sp, fontWeight = FontWeight.Medium,
                        maxLines = 1, overflow = TextOverflow.Ellipsis,
                    )
                    if (provider.isDefault) Badge("默认", t.accent, strong = true)
                    if (!provider.enabled) Badge("停用", t.text4)
                }
                Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(5.dp)) {
                    Box(Modifier.size(6.dp).clip(CircleShape).background(provider.status.dot(t)))
                    Text(
                        "${provider.status.label} · ${provider.model.ifEmpty { "—" }}",
                        color = t.text4, fontSize = 11.5f.sp, fontFamily = LXFont.mono,
                        maxLines = 1, overflow = TextOverflow.Ellipsis,
                    )
                }
            }
            LXIcon(name = LXIconName.ChevronR, size = 13.dp, color = t.text4, stroke = 1.7f)
        }
        if (!isLast) Box(Modifier.fillMaxWidth().size(0.5.dp).background(t.border))
    }
}

// MARK: - Provider picker (preset list) --------------------------------------

/**
 * The preset picker: pick a preset (Anthropic / OpenAI / … / Custom) and the
 * store seeds a new provider prefilled from it. [onPicked] receives the new id
 * so the host can replace the picker with the edit page atop the list.
 */
@Composable
fun ProviderPickerPage(
    kind: ProviderKind,
    store: SettingsStore,
    onPicked: (newId: String) -> Unit,
) {
    val t = LingXiTheme.palette
    Column(Modifier.fillMaxWidth()) {
        Blurb("选择一个预设 — 灵犀会自动预填 API 地址、Key 前缀和可用模型。")
        Column(
            modifier = Modifier
                .fillMaxWidth()
                .clip(RoundedCornerShape(12.dp))
                .background(t.surface)
                .border(0.5.dp, t.border, RoundedCornerShape(12.dp)),
        ) {
            kind.presets.forEachIndexed { i, p ->
                Column {
                    Row(
                        verticalAlignment = Alignment.CenterVertically,
                        horizontalArrangement = Arrangement.spacedBy(12.dp),
                        modifier = Modifier
                            .fillMaxWidth()
                            .clickable { onPicked(store.addProvider(kind, p.id)) }
                            .padding(horizontal = 14.dp, vertical = 12.dp),
                    ) {
                        PresetTile(preset = p, size = 32.dp, fontSize = 14.sp)
                        Column(modifier = Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
                            Text(p.name, color = t.text, fontSize = 14.sp, fontWeight = FontWeight.SemiBold)
                            Text(
                                p.sub + if (p.models.isEmpty()) "" else " · ${p.models.size} 模型",
                                color = t.text4, fontSize = 11.5f.sp,
                            )
                        }
                        LXIcon(name = LXIconName.Plus, size = 15.dp, color = t.text4, stroke = 2f)
                    }
                    if (i < kind.presets.size - 1) Box(Modifier.fillMaxWidth().size(0.5.dp).background(t.border))
                }
            }
        }
    }
}

// MARK: - Provider edit ------------------------------------------------------

/**
 * The provider editor: a live credential-status banner,
 * display-name / API-base / API-key (show-hide) fields, an optional cx field
 * (Google search), an LLM default-model radio (or a free model-id field for
 * custom), and an enable / set-default / remove section.
 *
 * State is hoisted: every field edits through [store]; [providerId] is looked up
 * each recomposition so the rendered values track the store. If the provider was
 * removed (or the id is unknown) the page pops via [onPop].
 */
@Composable
fun ProviderEditPage(
    kind: ProviderKind,
    providerId: String,
    state: SettingsUiState,
    store: SettingsStore,
    onReconnectEngine: () -> Unit = {},
    onPop: () -> Unit,
) {
    val t = LingXiTheme.palette
    val editing = state.providers(kind).firstOrNull { it.id == providerId }
    if (editing == null) {
        LaunchedEffect(providerId) { onPop() }
        return
    }
    val preset = kind.preset(of = editing)
    var showKey by remember { mutableStateOf(false) }
    var keyDraft by remember(providerId) { mutableStateOf("") }
    var removalMessage by remember(providerId) { mutableStateOf<String?>(null) }
    var removing by remember(providerId) { mutableStateOf(false) }
    var applying by remember(providerId) { mutableStateOf(false) }
    var credentialBusy by remember(providerId) { mutableStateOf(false) }
    var credentialMessage by remember(providerId, editing.credentialConfigured) {
        mutableStateOf(
            when {
                kind != ProviderKind.Llm -> "当前页面仅对 LLM provider 接入真实 credential 存储。"
                !store.state.value.providers(kind).any { it.id == providerId } -> ""
                !store.state.value.providers(kind).first { it.id == providerId }.credentialConfigured ->
                    "尚未保存凭据"
                else -> "凭据已在本机安全区配置；尚未执行网络连通性验证"
            },
        )
    }
    val builtInSupported = kind != ProviderKind.Llm || ProviderSettingsRepository.engineCredentialIdFor(editing) != null
    val applyState = providerApplyUiState(
        hasPendingConfiguration = providerId in state.pendingLlmProviderChanges,
        hasCredentialDraft = keyDraft.isNotBlank(),
        busy = applying || credentialBusy,
    )

    Column(Modifier.fillMaxWidth()) {
        StatusBanner(status = editing.status) {
            credentialMessage = "正在读取本机安全存储…"
            store.refreshProviderStatuses { error ->
                val refreshed = store.state.value.providers(kind).firstOrNull { it.id == providerId }
                credentialMessage = when {
                    error != null -> "刷新失败：$error"
                    refreshed?.credentialConfigured == true ->
                        "本地状态已刷新：凭据已在本机安全区配置；此操作不会联网验证 Key"
                    else -> "本地状态已刷新：未找到已保存凭据"
                }
            }
        }

        FieldLabel("显示名称")
        SettingsField(
            value = editing.name,
            onValueChange = { v -> store.updateProvider(kind, providerId) { it.copy(name = v) } },
            mono = false,
            modifier = Modifier.padding(bottom = 14.dp),
        )

        FieldLabel("API 地址")
        SettingsField(
            value = editing.url,
            onValueChange = { v -> store.updateProvider(kind, providerId) { it.copy(url = v) } },
            placeholder = preset.defaultUrl,
        )
        FieldHint("默认 `${preset.defaultUrl.ifEmpty { "—" }}` · 可填代理 / 镜像")

        FieldLabel("API Key")
        KeyField(
            value = keyDraft,
            placeholder = if (editing.credentialConfigured) {
                preset.keyPrefix + "••••••••••••（已安全保存）"
            } else {
                preset.keyPrefix + "..."
            },
            show = showKey,
            onToggleShow = { showKey = !showKey },
            onValueChange = { v -> keyDraft = v },
        )
        FieldHint(
            when {
                kind != ProviderKind.Llm -> "搜索/抓取 provider 仍是本地配置页；当前未接入真实 engine credential 协议。"
                !builtInSupported -> "该预设当前不在 Android 内建 provider catalog 中；需后续 bridge/profile 扩展。"
                editing.credentialConfigured -> "安全存储不会回传明文；输入新 Key 可覆盖当前凭据。"
                else -> "凭据保存到引擎共享安全存储；Anthropic 还会镜像到当前启动兼容 key 槽。"
            },
        )

        if (preset.needsCx) {
            FieldLabel("Custom Search Engine ID (cx)")
            SettingsField(
                value = editing.cx,
                onValueChange = { v -> store.updateProvider(kind, providerId) { it.copy(cx = v) } },
                placeholder = "0123456789abcdef:ghi",
            )
            FieldHint("在 Google Programmable Search 创建并粘贴搜索引擎 ID")
        }

        if (kind == ProviderKind.Llm) {
            if (preset.models.isNotEmpty()) {
                ModelPicker(
                    models = preset.models,
                    selected = editing.model,
                    accent = preset.color,
                    onSelect = { m -> store.updateProvider(kind, providerId) { it.copy(model = m) } },
                )
            } else {
                FieldLabel("模型 ID")
                SettingsField(
                    value = editing.model,
                    onValueChange = { v -> store.updateProvider(kind, providerId) { it.copy(model = v) } },
                    placeholder = "llama-3.3-70b",
                    modifier = Modifier.padding(bottom = 14.dp),
                )
            }
        }

        if (kind == ProviderKind.Llm) {
            SettingsSection(
                label = "应用",
                footer = if (providerId in state.pendingLlmProviderChanges) {
                    "配置已保存但尚未用于当前引擎。点击应用后会重建引擎，新请求将使用最新配置。"
                } else {
                    "当前引擎已使用这份提供商配置。"
                },
            ) {
                SettingsRow(
                    label = "应用配置并重连",
                    sub = "一次性应用 URL、模型与启用状态",
                    chevron = false,
                    isLast = true,
                    trailing = {
                        ProviderTextAction(
                            label = applyState.label,
                            enabled = applyState.enabled,
                            onClick = {
                                applying = true
                                if (applyState.saveCredential) {
                                    credentialMessage = "正在保存凭据并应用配置…"
                                    store.saveProviderCredential(kind, providerId, keyDraft) { error ->
                                        if (error != null) {
                                            credentialMessage = "保存失败：$error"
                                            applying = false
                                        } else {
                                            keyDraft = ""
                                            credentialMessage = "凭据已安全保存，配置已应用"
                                            onReconnectEngine()
                                            store.markLlmConfigurationApplied()
                                            applying = false
                                        }
                                    }
                                } else {
                                    onReconnectEngine()
                                    store.markLlmConfigurationApplied()
                                    applying = false
                                }
                            },
                        )
                    },
                )
            }

            SettingsSection(
                label = "凭据",
                footer = credentialMessage,
            ) {
                SettingsRow(
                    label = "保存凭据",
                    sub = if (builtInSupported) "写入引擎共享安全存储" else "当前预设未接入 Android 内建 provider catalog",
                    chevron = false,
                    trailing = {
                        Text(
                            "保存",
                            color = if (builtInSupported) t.accent else t.text4,
                            fontSize = 12.sp,
                            fontWeight = FontWeight.Medium,
                            modifier = Modifier
                                .clip(RoundedCornerShape(6.dp))
                                .clickable(
                                    enabled = builtInSupported && keyDraft.isNotBlank() && !credentialBusy && !applying,
                                ) {
                                    credentialBusy = true
                                    store.saveProviderCredential(kind, providerId, keyDraft) { error ->
                                        credentialMessage =
                                            error?.let { "保存失败：$it" } ?: "凭据已保存到引擎共享安全存储"
                                        if (error == null) {
                                            keyDraft = ""
                                            onReconnectEngine()
                                            store.markLlmConfigurationApplied()
                                        }
                                        credentialBusy = false
                                    }
                                }
                                .padding(horizontal = 7.dp, vertical = 5.dp),
                        )
                    },
                )
                SettingsRow(
                    label = "清除已保存凭据",
                    sub = if (editing.credentialConfigured) "删除共享安全存储中的 provider key" else "当前没有已保存凭据",
                    chevron = false,
                    isLast = true,
                    trailing = {
                        Text(
                            "清除",
                            color = if (editing.credentialConfigured && builtInSupported) t.danger else t.text4,
                            fontSize = 12.sp,
                            fontWeight = FontWeight.Medium,
                            modifier = Modifier
                                .clip(RoundedCornerShape(6.dp))
                                .clickable(
                                    enabled = editing.credentialConfigured && builtInSupported && !credentialBusy && !applying,
                                ) {
                                    credentialBusy = true
                                    store.clearProviderCredential(kind, providerId) { error ->
                                        credentialMessage =
                                            error?.let { "清除失败：$it" } ?: "共享安全存储中的凭据已清除"
                                        if (error == null) {
                                            onReconnectEngine()
                                            store.markLlmConfigurationApplied()
                                        }
                                        credentialBusy = false
                                    }
                                }
                                .padding(horizontal = 7.dp, vertical = 5.dp),
                        )
                    },
                )
            }
        }

        SettingsSection {
            SettingsRow(
                label = "启用", chevron = false,
                trailing = {
                    com.lingxi.code.components.LXToggle(
                        checked = editing.enabled,
                        onCheckedChange = { v -> store.updateProvider(kind, providerId) { it.copy(enabled = v) } },
                    )
                },
            )
            SettingsRow(
                label = "设为默认", chevron = false, isLast = true,
                onTap = {
                    store.setDefaultProvider(kind, providerId)
                    if (kind == ProviderKind.Llm) {
                        onReconnectEngine()
                        store.markLlmConfigurationApplied()
                    }
                },
                trailing = {
                    if (editing.isDefault) {
                        LXIcon(name = LXIconName.Check, size = 16.dp, color = t.accent, stroke = 2.2f)
                    } else {
                        Text("未启用", color = t.text4, fontSize = 12.sp)
                    }
                },
            )
        }

        Row(
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.Center,
            modifier = Modifier
                .fillMaxWidth()
                .padding(top = 8.dp)
                .clip(RoundedCornerShape(11.dp))
                .border(0.5.dp, t.border, RoundedCornerShape(11.dp))
                .clickable(enabled = !removing) {
                    removing = true
                    removalMessage = null
                    store.removeProvider(kind, providerId) { error ->
                        removing = false
                        removalMessage = error
                        if (error == null) {
                            if (kind == ProviderKind.Llm) {
                                onReconnectEngine()
                                store.markLlmConfigurationApplied()
                            }
                            onPop()
                        }
                    }
                }
                .padding(12.dp),
        ) {
            Text(
                if (removing) "正在删除凭据…" else "移除此提供商",
                color = if (removing) t.text4 else t.danger,
                fontSize = 13.5f.sp,
                fontWeight = FontWeight.Medium,
            )
        }
        if (removalMessage != null) {
            Text(
                removalMessage.orEmpty(),
                color = t.danger,
                fontSize = 12.sp,
                modifier = Modifier.padding(top = 8.dp, start = 4.dp, end = 4.dp),
            )
        }
    }
}

// MARK: - Pieces -------------------------------------------------------------

/** A preset's square brand tile: first character on a faint tint of its color. */
@Composable
private fun PresetTile(preset: ProviderPreset, size: androidx.compose.ui.unit.Dp, fontSize: androidx.compose.ui.unit.TextUnit) {
    val t = LingXiTheme.palette
    Box(
        contentAlignment = Alignment.Center,
        modifier = Modifier
            .size(size)
            .clip(RoundedCornerShape(if (size > 28.dp) 8.dp else 7.dp))
            .background(preset.color.mix(t.surface, 0.18f))
            .border(0.5.dp, preset.color.tint(0.28f), RoundedCornerShape(if (size > 28.dp) 8.dp else 7.dp)),
    ) {
        Text(
            preset.name.take(1),
            color = preset.color,
            fontSize = fontSize,
            fontWeight = FontWeight.Bold,
        )
    }
}

/** A small status / default / disabled badge. */
@Composable
private fun Badge(text: String, color: Color, strong: Boolean = false) {
    Text(
        text = text,
        color = color,
        fontSize = 10.sp,
        fontWeight = if (strong) FontWeight.SemiBold else FontWeight.Medium,
        modifier = Modifier
            .clip(RoundedCornerShape(4.dp))
            .background(color.tint(0.18f))
            .padding(horizontal = 6.dp, vertical = 1.dp),
    )
}

/**
 * Credential status is read from the engine's shared encrypted store. Refresh
 * never manufactures a successful network result.
 */
@Composable
private fun StatusBanner(status: ConnStatus, onTest: () -> Unit) {
    val t = LingXiTheme.palette
    val dot = status.dot(t)
    val refreshing = status == ConnStatus.Testing

    Row(
        verticalAlignment = Alignment.CenterVertically,
        modifier = Modifier
            .fillMaxWidth()
            .padding(bottom = 18.dp)
            .clip(RoundedCornerShape(10.dp))
            .background(dot.mix(t.surface, 0.08f))
            .border(0.5.dp, dot.tint(0.24f), RoundedCornerShape(10.dp))
            .padding(horizontal = 12.dp, vertical = 10.dp),
    ) {
        PulsingDot(color = dot, pulsing = status == ConnStatus.Testing)
        Text(
            status.label,
            color = t.text2, fontSize = 12.5f.sp, fontWeight = FontWeight.Medium,
            modifier = Modifier.padding(start = 8.dp).weight(1f),
        )
        Text(
            if (refreshing) "刷新中…" else "刷新本地状态",
            color = if (refreshing) t.text4 else t.text2,
            fontSize = 12.sp,
            fontWeight = FontWeight.Medium,
            modifier = Modifier
                .clip(RoundedCornerShape(7.dp))
                .background(t.windowBg)
                .border(0.5.dp, t.border, RoundedCornerShape(7.dp))
                .clickable(enabled = !refreshing, onClick = onTest)
                .padding(horizontal = 10.dp, vertical = 6.dp),
        )
    }
}

@Composable
private fun ProviderTextAction(
    label: String,
    enabled: Boolean,
    onClick: () -> Unit,
) {
    val t = LingXiTheme.palette
    Text(
        label,
        color = if (enabled) t.accent else t.text4,
        fontSize = 12.sp,
        fontWeight = FontWeight.Medium,
        modifier = Modifier
            .clip(RoundedCornerShape(6.dp))
            .clickable(enabled = enabled, onClick = onClick)
            .padding(horizontal = 7.dp, vertical = 5.dp),
    )
}

/** An 8dp dot with an optional pulsing ring (used while a connection is testing). */
@Composable
private fun PulsingDot(color: Color, pulsing: Boolean) {
    val transition = rememberInfiniteTransition(label = "statusDot")
    val ringScale by transition.animateFloat(
        initialValue = 1.0f, targetValue = 1.9f,
        animationSpec = infiniteRepeatable(tween(900), RepeatMode.Restart),
        label = "ringScale",
    )
    val ringAlpha by transition.animateFloat(
        initialValue = 0.55f, targetValue = 0f,
        animationSpec = infiniteRepeatable(tween(900), RepeatMode.Restart),
        label = "ringAlpha",
    )
    Box(contentAlignment = Alignment.Center, modifier = Modifier.size(8.dp)) {
        if (pulsing) {
            Box(
                Modifier
                    .size(8.dp)
                    .scale(ringScale)
                    .alpha(ringAlpha)
                    .clip(CircleShape)
                    .background(color),
            )
        }
        Box(Modifier.size(8.dp).clip(CircleShape).background(color))
    }
}

/**
 * The API-key field: a masked [PasswordVisualTransformation] text field with an
 * inline 显示 / 隐藏 toggle and a copy affordance (copy is decorative — keys are
 * mock). Built here rather than overloading [SettingsField] to keep that helper
 * single-purpose.
 */
@Composable
private fun KeyField(
    value: String,
    placeholder: String,
    show: Boolean,
    onToggleShow: () -> Unit,
    onValueChange: (String) -> Unit,
) {
    val t = LingXiTheme.palette
    Box(
        modifier = Modifier
            .fillMaxWidth()
            .clip(RoundedCornerShape(10.dp))
            .background(t.surface)
            .border(0.5.dp, t.border, RoundedCornerShape(10.dp))
            .padding(start = 12.dp, end = 6.dp, top = 11.dp, bottom = 11.dp),
        contentAlignment = Alignment.CenterStart,
    ) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            Box(Modifier.weight(1f), contentAlignment = Alignment.CenterStart) {
                val style = TextStyle(color = t.text, fontSize = 13.5f.sp, fontFamily = LXFont.mono)
                if (value.isEmpty() && placeholder.isNotEmpty()) {
                    Text(placeholder, style = style.copy(color = t.text4), maxLines = 1, overflow = TextOverflow.Ellipsis)
                }
                BasicTextField(
                    value = value,
                    onValueChange = onValueChange,
                    textStyle = style,
                    singleLine = true,
                    cursorBrush = SolidColor(t.accent),
                    visualTransformation = if (show) VisualTransformation.None else PasswordVisualTransformation('•'),
                    modifier = Modifier.fillMaxWidth(),
                )
            }
            Text(
                if (show) "隐藏" else "显示",
                color = t.text3, fontSize = 11.sp, fontWeight = FontWeight.Medium,
                modifier = Modifier
                    .clip(RoundedCornerShape(6.dp))
                    .clickable(onClick = onToggleShow)
                    .padding(horizontal = 7.dp, vertical = 5.dp),
            )
            Box(
                modifier = Modifier
                    .clip(RoundedCornerShape(6.dp))
                    .clickable {}
                    .padding(horizontal = 5.dp, vertical = 4.dp),
            ) {
                LXIcon(name = LXIconName.Copy, size = 13.dp, color = t.text3, stroke = 1.8f)
            }
        }
    }
}

/** The LLM default-model single-select radio list. */
@Composable
private fun ModelPicker(
    models: List<String>,
    selected: String,
    accent: Color,
    onSelect: (String) -> Unit,
) {
    val t = LingXiTheme.palette
    Column(Modifier.fillMaxWidth()) {
        FieldLabel("默认模型")
        Column(
            modifier = Modifier
                .fillMaxWidth()
                .padding(bottom = 14.dp)
                .clip(RoundedCornerShape(10.dp))
                .background(t.surface)
                .border(0.5.dp, t.border, RoundedCornerShape(10.dp)),
        ) {
            models.forEachIndexed { i, m ->
                val sel = selected == m
                Column {
                    Row(
                        verticalAlignment = Alignment.CenterVertically,
                        horizontalArrangement = Arrangement.spacedBy(10.dp),
                        modifier = Modifier
                            .fillMaxWidth()
                            .clickable { onSelect(m) }
                            .padding(horizontal = 12.dp, vertical = 11.dp),
                    ) {
                        Box(
                            contentAlignment = Alignment.Center,
                            modifier = Modifier
                                .size(16.dp)
                                .clip(CircleShape)
                                .border(1.5.dp, if (sel) accent else t.border, CircleShape),
                        ) {
                            if (sel) Box(Modifier.size(8.dp).clip(CircleShape).background(accent))
                        }
                        Text(m, color = t.text, fontSize = 13.sp, fontFamily = LXFont.mono, modifier = Modifier.weight(1f))
                    }
                    if (i < models.size - 1) Box(Modifier.fillMaxWidth().size(0.5.dp).background(t.border))
                }
            }
        }
    }
}
