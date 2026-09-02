package com.lingxi.code.model

import androidx.compose.runtime.Immutable
import androidx.compose.ui.graphics.Color
import com.lingxi.code.bindings.ModelBillingModeDto
import com.lingxi.code.bindings.ModelDetailsDto
import com.lingxi.code.bindings.ImageRefDto
import java.util.UUID

/**
 * Domain models + mock data, ported 1:1 from the iOS `Models.swift` (itself a
 * verbatim port of the `lingxi-iphone.html` prototype). This is the single
 * canonical mock dataset the Android shell renders — drawer, conversation and
 * settings all read from [MockData].
 *
 * Colors reuse the exact oklch→sRGB values already computed in the brand
 * `DesignTokens`; the iOS `Color(srgb: r, g, b)` maps directly to Compose
 * `Color(red = r, green = g, blue = b)`. The original oklch string is kept in a
 * trailing comment for traceability.
 */

data class Workspace(
    val id: String,
    val name: String,
    val icon: String,
    val color: Color,
)

data class Chat(
    val id: String,
    val wsId: String,
    val title: String,
    val group: String,
    val preview: String,
    val activity: String,
)

data class ProjectSession(
    val id: String,
    val title: String,
    val activity: String,
    val preview: String,
    val pinned: Boolean = false,
    val msgs: Int,
    val mode: SessionMode = SessionMode.Code,
    val updatedAtEpochMillis: Long = 0L,
)

data class Project(
    val id: String,
    val wsId: String,
    val name: String,
    val icon: String,
    val color: Color,
    val desc: String,
    val sessions: List<ProjectSession>,
    val storageKind: String = "internal",
    val syncState: String = "",
    val updatedAtEpochMillis: Long = 0L,
)

data class Cron(
    val id: String,
    val wsId: String,
    val title: String,
    val cron: String,
    val next: String,
    val desc: String,
    val enabled: Boolean,
)

/**
 * Compact, provider-published model facts shown under a model-picker row.
 *
 * Empty fields are deliberately omitted instead of guessed. In particular,
 * Anthropic does not publish parameter counts, while OpenRouter aliases resolve
 * dynamically, so the UI states those limitations explicitly.
 */
data class ModelMetadata(
    val displayName: String? = null,
    val providerLabel: String? = null,
    val modelId: String? = null,
    val description: String? = null,
    val thinking: String? = null,
    val capabilities: String? = null,
    val contextWindow: String? = null,
    val maxOutput: String? = null,
    val pricing: String? = null,
    val status: String? = null,
) {
    val summaryItems: List<String>
        get() = listOfNotNull(thinking, capabilities, contextWindow, maxOutput, pricing, status)

    val searchableText: String
        get() = listOfNotNull(
            displayName,
            providerLabel,
            modelId,
            description,
            *summaryItems.toTypedArray(),
        ).joinToString(" ")

    companion object {
        fun fromDetails(detail: ModelDetailsDto): ModelMetadata {
            val capabilityBadges = buildList {
                if (detail.capabilities.reasoning) add("Thinking")
                if (detail.capabilities.vision) add("图片")
                if (detail.capabilities.documents) add("文档")
                if (detail.capabilities.tools) add("工具")
                if (detail.capabilities.structuredOutput) add("结构化输出")
            }.takeIf { it.isNotEmpty() }?.joinToString(" · ")
            val thinkingSummary = buildThinkingSummary(detail)
            val contextSummary = detail.contextWindowTokens?.let(::formatTokenCount)?.let { "$it 上下文" }
            val maxOutputSummary = detail.maxOutputTokens?.let(::formatTokenCount)?.let { "$it 输出" }
            val pricingSummary = buildPricingSummary(detail)
            val statusSummary = detail.status
                ?.takeIf(String::isNotBlank)
                ?.replaceFirstChar { it.uppercaseChar() }
            return ModelMetadata(
                displayName = detail.displayName.takeIf(String::isNotBlank),
                providerLabel = detail.providerLabel.takeIf(String::isNotBlank),
                modelId = detail.modelId.takeIf(String::isNotBlank),
                description = detail.description?.takeIf(String::isNotBlank),
                thinking = thinkingSummary,
                capabilities = capabilityBadges,
                contextWindow = contextSummary,
                maxOutput = maxOutputSummary,
                pricing = pricingSummary,
                status = statusSummary,
            )
        }

        private fun buildThinkingSummary(detail: ModelDetailsDto): String? {
            val options = detail.reasoning.options
            if (options.isEmpty()) return null
            val labels = options.mapNotNull { option ->
                when (val selection = option.selection) {
                    is com.lingxi.code.bindings.ReasoningSelectionDto.Automatic -> "自动"
                    is com.lingxi.code.bindings.ReasoningSelectionDto.Disabled -> "关闭"
                    is com.lingxi.code.bindings.ReasoningSelectionDto.Enabled -> "开启"
                    is com.lingxi.code.bindings.ReasoningSelectionDto.Level -> selection.id
                    is com.lingxi.code.bindings.ReasoningSelectionDto.TokenBudget ->
                        "${formatTokenCount(selection.tokens)} 预算"
                }
            }
            return if (labels.isEmpty()) null else "Thinking ${labels.joinToString("/")}"
        }

        private fun buildPricingSummary(detail: ModelDetailsDto): String? {
            val pricing = detail.pricing ?: return null
            return when (pricing.billingMode) {
                ModelBillingModeDto.SUBSCRIPTION -> "套餐/订阅内"
                ModelBillingModeDto.FREE -> "免费"
                ModelBillingModeDto.UNKNOWN -> "价格未提供"
                ModelBillingModeDto.PER_TOKEN -> {
                    val input = pricing.inputPerMillion?.let { "\$${formatPrice(it)} in" }
                    val output = pricing.outputPerMillion?.let { "\$${formatPrice(it)} out" }
                    listOfNotNull(input, output).takeIf { it.isNotEmpty() }?.joinToString(" · ")
                        ?: "价格未提供"
                }
            }
        }

        private fun formatTokenCount(value: ULong): String {
            val n = value.toDouble()
            return when {
                n >= 1_000_000.0 -> "${trimDecimal(n / 1_000_000.0)}M"
                n >= 1_000.0 -> "${trimDecimal(n / 1_000.0)}K"
                else -> value.toString()
            }
        }

        private fun formatPrice(value: Double): String = trimDecimal(value)

        private fun trimDecimal(value: Double): String {
            val rounded = kotlin.math.round(value * 100.0) / 100.0
            return if (rounded % 1.0 == 0.0) rounded.toInt().toString() else rounded.toString()
        }
    }
}

data class CatalogReasoningOption(
    val label: String,
    val persistable: Boolean,
)

data class CatalogModelPricingTier(
    val contextThresholdTokens: ULong,
    val inputPerMillion: Double?,
    val outputPerMillion: Double?,
    val cacheReadPerMillion: Double?,
    val cacheWritePerMillion: Double?,
    val reasoningPerMillion: Double?,
)

data class CatalogModelPricing(
    val billingMode: ModelBillingModeDto,
    val inputPerMillion: Double?,
    val outputPerMillion: Double?,
    val cacheReadPerMillion: Double?,
    val cacheWritePerMillion: Double?,
    val reasoningPerMillion: Double?,
    val tiers: List<CatalogModelPricingTier>,
    val source: String?,
)

data class CatalogModelDetails(
    val reference: String,
    val providerId: String,
    val providerLabel: String,
    val displayName: String,
    val modelId: String,
    val description: String?,
    val family: String?,
    val status: String?,
    val releaseDate: String?,
    val lastUpdated: String?,
    val knowledgeCutoff: String?,
    val inputModalities: List<String>,
    val outputModalities: List<String>,
    val contextWindowTokens: ULong?,
    val maxInputTokens: ULong?,
    val maxOutputTokens: ULong?,
    val openWeights: Boolean?,
    val attachments: Boolean?,
    val temperatureControl: Boolean?,
    val pricing: CatalogModelPricing?,
    val capabilities: List<String>,
    val reasoningOptions: List<CatalogReasoningOption>,
    val reasoningEditable: Boolean,
    val reasoningForced: Boolean,
    val reasoningDefault: String?,
    val metadata: ModelMetadata,
) {
    companion object {
        fun fromDto(dto: ModelDetailsDto): CatalogModelDetails =
            CatalogModelDetails(
                reference = dto.reference,
                providerId = dto.providerId,
                providerLabel = dto.providerLabel,
                displayName = dto.displayName,
                modelId = dto.modelId,
                description = dto.description,
                family = dto.family,
                status = dto.status,
                releaseDate = dto.releaseDate,
                lastUpdated = dto.lastUpdated,
                knowledgeCutoff = dto.knowledgeCutoff,
                inputModalities = dto.inputModalities,
                outputModalities = dto.outputModalities,
                contextWindowTokens = dto.contextWindowTokens,
                maxInputTokens = dto.maxInputTokens,
                maxOutputTokens = dto.maxOutputTokens,
                openWeights = dto.openWeights,
                attachments = dto.attachments,
                temperatureControl = dto.temperatureControl,
                pricing = dto.pricing?.let { pricing ->
                    CatalogModelPricing(
                        billingMode = pricing.billingMode,
                        inputPerMillion = pricing.inputPerMillion,
                        outputPerMillion = pricing.outputPerMillion,
                        cacheReadPerMillion = pricing.cacheReadPerMillion,
                        cacheWritePerMillion = pricing.cacheWritePerMillion,
                        reasoningPerMillion = pricing.reasoningPerMillion,
                        tiers = pricing.tiers.map { tier ->
                            CatalogModelPricingTier(
                                contextThresholdTokens = tier.contextThresholdTokens,
                                inputPerMillion = tier.inputPerMillion,
                                outputPerMillion = tier.outputPerMillion,
                                cacheReadPerMillion = tier.cacheReadPerMillion,
                                cacheWritePerMillion = tier.cacheWritePerMillion,
                                reasoningPerMillion = tier.reasoningPerMillion,
                            )
                        },
                        source = pricing.source,
                    )
                },
                capabilities = buildCapabilities(dto),
                reasoningOptions = dto.reasoning.options.mapNotNull { option ->
                    selectionLabel(option.selection)?.let { label ->
                        CatalogReasoningOption(label = label, persistable = option.persistable)
                    }
                },
                reasoningEditable = dto.reasoning.editable,
                reasoningForced = dto.reasoning.forcedReasoning,
                reasoningDefault = selectionLabel(dto.reasoning.providerDefault),
                metadata = ModelMetadata.fromDetails(dto),
            )

        private fun buildCapabilities(dto: ModelDetailsDto): List<String> = buildList {
            if (dto.capabilities.reasoning) add("Thinking")
            if (dto.capabilities.vision) add("图片")
            if (dto.capabilities.documents) add("文档")
            if (dto.capabilities.tools) add("工具")
            if (dto.capabilities.structuredOutput) add("结构化输出")
            if (dto.capabilities.streaming) add("流式")
        }

        private fun selectionLabel(
            selection: com.lingxi.code.bindings.ReasoningSelectionDto,
        ): String? = when (selection) {
            is com.lingxi.code.bindings.ReasoningSelectionDto.Automatic -> "自动"
            is com.lingxi.code.bindings.ReasoningSelectionDto.Disabled -> "关闭"
            is com.lingxi.code.bindings.ReasoningSelectionDto.Enabled -> "开启"
            is com.lingxi.code.bindings.ReasoningSelectionDto.Level -> selection.id
            is com.lingxi.code.bindings.ReasoningSelectionDto.TokenBudget ->
                "${formatTokenCount(selection.tokens)} 预算"
        }

        private fun formatTokenCount(value: ULong): String {
            val n = value.toDouble()
            return when {
                n >= 1_000_000.0 -> "${trimDecimal(n / 1_000_000.0)}M"
                n >= 1_000.0 -> "${trimDecimal(n / 1_000.0)}K"
                else -> value.toString()
            }
        }

        private fun trimDecimal(value: Double): String {
            val rounded = kotlin.math.round(value * 100.0) / 100.0
            return if (rounded % 1.0 == 0.0) rounded.toInt().toString() else rounded.toString()
        }
    }
}

/**
 * Settings-side state for one engine provider profile.
 *
 * [ConnStatus.Configured] means a credential exists but has not been proven by
 * a live request. Only Configured/Connected rows can be selected; all other
 * states route to that provider's settings editor.
 */
data class ModelProviderStatus(
    val profileId: String,
    val settingsId: String,
    val name: String,
    val status: ConnStatus,
    val enabled: Boolean,
    val credentialConfigured: Boolean,
) {
    val canSelect: Boolean
        get() = enabled && (status == ConnStatus.Configured || status == ConnStatus.Connected)

    val displayLabel: String
        get() = when {
            !enabled -> "未启用"
            status == ConnStatus.Idle && !credentialConfigured -> "未配置"
            else -> status.label
        }
}

data class ModelOption(
    val id: String,
    val name: String,
    val desc: String,
    val tag: String,
    val color: Color,
    /** Provider profile id from the qualified engine ref (`provider/model`). */
    val providerId: String = "",
    /** Stable, human-facing label derived from [providerId]. */
    val providerName: String = "",
    /** Published capability/size facts for the compact picker detail line. */
    val metadata: ModelMetadata = ModelMetadata(),
    /** Full provider/model detail payload for the info dialog. */
    val details: CatalogModelDetails? = null,
) {
    /** Name with the "Lingxi-" prefix stripped (composer chip label). */
    val shortName: String get() = name.replace("Lingxi-", "")
}

/** A provider section in the model picker, preserving the engine's input order. */
data class ModelProviderGroup(
    val id: String,
    val name: String,
    val models: List<ModelOption>,
)

enum class Role { User, Ai }

/**
 * A single conversation turn. [id] is a fresh UUID so list diffing is stable
 * even when two messages share text (matches the iOS `Message` `let id = UUID()`).
 */
@Immutable
data class Message(
    val role: Role,
    val text: String,
    val tag: String? = null,
    val id: String = UUID.randomUUID().toString(),
    /** Images attached to this user turn, kept in the same order as SendPrompt. */
    val images: List<ImageRefDto> = emptyList(),
    /**
     * The message's ORDERED content, when the engine supplied structure.
     *
     * Empty for every locally-composed message (a user turn, a streamed
     * assistant turn) — those render [text] as before. A restored or completed
     * engine message fills this so its tool calls render as the engine's derived
     * header + `⎿` result block instead of being flattened into [text], which is
     * what discarded every diff, body, and file path before.
     *
     * Only the assistant bubble renders these; a user bubble stays text-only.
     */
    val blocks: List<com.lingxi.code.conversation.MessageContent> = emptyList(),
)

/** A unified "session" reference used by the conversation title bar. */
data class SessionRef(
    val id: String,
    val title: String,
)

// MARK: - Mock data ---------------------------------------------------------

/** Verbatim mock data from `lingxi-iphone.html` / the iOS `MockData`. */
object MockData {

    val workspaces: List<Workspace> = listOf(
        Workspace("personal", "个人", "◐", Color(red = 0.4340f, green = 0.5865f, blue = 1.0000f)), // oklch(70% 0.18 268)
        Workspace("work", "工作", "◑", Color(red = 0.0000f, green = 0.7601f, blue = 0.7664f)),      // oklch(70% 0.16 195)
        Workspace("research", "研究", "◒", Color(red = 0.8090f, green = 0.4552f, blue = 0.8891f)),  // oklch(72% 0.18 320)
        Workspace("creative", "创作", "◓", Color(red = 0.8696f, green = 0.5765f, blue = 0.0000f)),  // oklch(72% 0.16 75)
    )

    val chats: List<Chat> = listOf(
        Chat("c1", "work", "重装 LingXi", "今天", "nvm 残留清理完成", "2 小时前"),
        Chat("c2", "work", "客户邮件回复模板", "昨天", "已生成 4 套话术", "昨天"),
        Chat("c3", "work", "上海差旅规划", "本周", "机酒路线", "周二"),
    )

    val projects: List<Project> = listOf(
        Project(
            id = "p1", wsId = "work", name = "灵犀 OS 设计", icon = "◑",
            color = Color(red = 0.4340f, green = 0.5865f, blue = 1.0000f), // oklch(70% 0.18 268)
            desc = "多端 UI · 14 文件 · 8 记忆",
            sessions = listOf(
                ProjectSession("s1", "设计灵犀 iPhone 版", "刚刚", "类 Claude 移动端布局", pinned = true, msgs = 8),
                ProjectSession("p1s2", "iPad 横屏推演", "昨天", "Pencil 标注入口", msgs = 14),
                ProjectSession("p1s3", "深色色板校准", "5月3日", "oklch 节点对齐", msgs = 22),
            ),
        ),
        Project(
            id = "p2", wsId = "work", name = "Q2 OKR & 周报", icon = "◐",
            color = Color(red = 0.0000f, green = 0.7601f, blue = 0.7664f), // oklch(72% 0.16 195)
            desc = "目标对齐 · 6 文件 · 3 记忆",
            sessions = listOf(
                ProjectSession("p2s1", "整理 Q2 OKR 草案", "5 小时前", "已对齐三方", msgs = 24),
                ProjectSession("p2s2", "周报自动化模板", "昨天", "从多源聚合", msgs = 6),
            ),
        ),
        Project(
            id = "p3", wsId = "work", name = "Code & 工程", icon = "◇",
            color = Color(red = 0.2085f, green = 0.7571f, blue = 0.4656f), // oklch(72% 0.16 155)
            desc = "Bug 排查 · 23 文件 · 12 记忆",
            sessions = listOf(
                ProjectSession("p3s1", "WebSocket 重连排查", "昨天", "指数退避方案", msgs = 31),
                ProjectSession("p3s2", "PRD v2 评审反馈", "周一", "12 评论 4 待办", msgs = 18),
            ),
        ),
    )

    val crons: List<Cron> = listOf(
        Cron("cr1", "work", "每日晨报", "工作日 08:30", "明早 08:30", "聚合 Linear/GitHub/邮件 → 早会摘要", enabled = true),
        Cron("cr2", "work", "周报自动生成", "每周五 17:00", "周五 17:00", "git 提交 + 日历 → 周报草稿", enabled = true),
        Cron("cr3", "work", "客户反馈周聚合", "每周一 09:00", "下周一 09:00", "7 天工单聚类 + 情感分析", enabled = true),
        Cron("cr4", "work", "凌晨日志巡检", "每日 03:00", "— 已暂停", "错误日志分类 + 告警", enabled = false),
    )

    val models: List<ModelOption> = listOf(
        ModelOption("lx-72b", "Lingxi-72B", "主力", "默认", Color(red = 0.4340f, green = 0.5865f, blue = 1.0000f)),
        ModelOption("lx-72b-r", "Lingxi-72B-R", "推理", "慢", Color(red = 0.8090f, green = 0.4552f, blue = 0.8891f)),
        ModelOption("lx-32b", "Lingxi-32B", "高速", "快", Color(red = 0.0000f, green = 0.7601f, blue = 0.7664f)),
        ModelOption("lx-code", "Lingxi-Code", "代码", "编程", Color(red = 0.2085f, green = 0.7571f, blue = 0.4656f)),
    )

    val messagesDefault: List<Message> = listOf(
        Message(
            role = Role.User,
            text = "帮我做一版手机上的灵犀 AI 助手，参考 Claude iOS 应用的极简风格，但要保留多 workspace 和工作流的能力。",
        ),
        Message(
            role = Role.Ai,
            tag = "思考了 48 秒",
            text = "已完成。\n\n**iPhone 版本设计要点**：\n\n1. **主界面 = 对话**。打开即进入最近会话，没有冗余首页。\n2. **左滑/汉堡 → 抽屉**，包含 workspace pill、session 列表和设置。\n3. **顶部 chip 显示工作流进度**，一行可滑动，与 Mac/iPad 一致。\n4. **底部胶囊 composer**，按住录音、点附件出 sheet。\n\n点击左上角菜单试试抽屉。",
        ),
        Message(
            role = Role.User,
            text = "能不能加个语音\"心流\"模式？随时按住屏幕说话，松开发送。",
        ),
        Message(
            role = Role.Ai,
            tag = "思考了 12 秒",
            text = "已加。**按住屏幕任意位置 0.6 秒**会进入沉浸录音态：背景虚化，中央波形脉动，松开立即发送给当前模型。键盘/composer 临时隐藏。\n\n再次按住录音时，AI 的上一条回复会变为半透明，提示\"上下文已记入\"。",
        ),
    )

    /** Flattened session lookup (chats + every project session). */
    val allSessions: List<SessionRef>
        get() {
            val refs = chats.map { SessionRef(it.id, it.title) }.toMutableList()
            for (p in projects) {
                refs += p.sessions.map { SessionRef(it.id, it.title) }
            }
            return refs
        }

    fun session(id: String): SessionRef =
        allSessions.firstOrNull { it.id == id } ?: allSessions[0]
}

// MARK: - Engine model catalog ----------------------------------------------

/**
 * The single source of truth for what the model picker shows + which row is
 * active. SHIP-BLOCKER #2: the [available] ids and [active] id are REAL,
 * provider-qualified model references reported by the engine (`ModelList` /
 * `ModelChanged`) — never the branded `lx-*` mock ids.
 * [EngineModelCatalog.options] turns this into the picker's [ModelOption] rows,
 * attaching friendly provider/model labels without ever losing the qualified
 * id (the id is what `SetModel` sends).
 *
 * The empty state ([available] empty) means the engine has not reported its
 * catalog yet. Production UI renders [EngineModelCatalog.pending] instead of a
 * fabricated model list.
 */
data class EngineModelState(
    val available: List<String> = emptyList(),
    val active: String = "",
    val details: Map<String, CatalogModelDetails> = emptyMap(),
) {
    val hasCatalog: Boolean get() = available.isNotEmpty()
}

/**
 * Provider grouping + friendly-name derivation for REAL qualified model refs.
 * The engine already curates the latest common models; this object only formats
 * the refs it receives and never expands them from a provider catalog. PURE —
 * no engine / Android dependency — so it is unit-testable on the plain JVM.
 */
object EngineModelCatalog {

    val pending: ModelOption = ModelOption(
        id = "",
        name = "加载模型…",
        desc = "等待移动端引擎返回真实模型目录",
        tag = "",
        color = Color(red = 0.5728f, green = 0.6177f, blue = 0.7466f),
    )

    // A small, deterministic accent palette (reusing the brand colors the mock
    // catalog already uses) so each picker row gets a stable dot color keyed by
    // its position — purely cosmetic, never affects the wire id.
    private val accents: List<Color> = listOf(
        Color(red = 0.4340f, green = 0.5865f, blue = 1.0000f), // indigo
        Color(red = 0.8090f, green = 0.4552f, blue = 0.8891f), // violet
        Color(red = 0.0000f, green = 0.7601f, blue = 0.7664f), // teal
        Color(red = 0.2085f, green = 0.7571f, blue = 0.4656f), // green
        Color(red = 0.8696f, green = 0.5765f, blue = 0.0000f), // amber
    )

    private val providerNames = mapOf(
        "anthropic" to "Anthropic",
        "builtin" to "Anthropic (Built-in)",
        "openai" to "OpenAI",
        "openai-chatgpt" to "OpenAI (ChatGPT)",
        "gemini" to "Google Gemini",
        "deepseek" to "DeepSeek",
        "kimi" to "Kimi",
        "kimi-code" to "Kimi Code",
        "openrouter" to "OpenRouter",
        "github-copilot" to "GitHub Copilot",
        "zai" to "Z.AI",
        "glm-coding" to "GLM Coding Plan",
    )

    /**
     * Stable display label for a provider profile id. Known providers keep
     * their official casing; custom profile ids get a deterministic title-case
     * fallback so the same input always renders the same section header.
     */
    fun providerDisplayName(id: String): String {
        if (id.isBlank()) return "其他"
        providerNames[id.lowercase()]?.let { return it }
        return id
            .split('-', '_')
            .filter(String::isNotBlank)
            .joinToString(" ") { segment ->
                segment.replaceFirstChar { char -> char.uppercaseChar() }
            }
            .ifBlank { id }
    }

    /**
     * Turn a qualified ref or request model id into a human label. The provider
     * prefix and a trailing date stamp are omitted; well-known model brands keep
     * their casing. Unknown shapes are still rendered deterministically.
     */
    fun displayName(id: String): String {
        if (id.isBlank()) return id
        val requestModel = id.substringAfter('/').substringAfterLast('/')
        val parts = requestModel.split('-', '_').filter { it.isNotEmpty() }
        // Drop a trailing all-digit date/version stamp (e.g. "20250514").
        val trimmed = parts.dropLastWhile { it.length >= 6 && it.all(Char::isDigit) }
        val segments = trimmed.ifEmpty { parts }
        return segments.joinToString(" ") { seg ->
            when (seg.lowercase()) {
                "claude" -> "Claude"
                "gpt" -> "GPT"
                "gemini" -> "Gemini"
                "deepseek" -> "DeepSeek"
                "qwen" -> "Qwen"
                "glm" -> "GLM"
                "llama" -> "Llama"
                "mistral" -> "Mistral"
                "codestral" -> "Codestral"
                "grok" -> "Grok"
                "kimi" -> "Kimi"
                else -> when {
                    seg.all { it.isDigit() || it == '.' } -> seg
                    else -> seg.replaceFirstChar { c -> c.uppercaseChar() }
                }
            }
        }
    }

    /**
     * Build picker rows only from the engine's curated [ids]. Each
     * [ModelOption.id] remains the verbatim qualified ref sent by `SetModel`.
     */
    fun options(
        ids: List<String>,
        detailsByReference: Map<String, CatalogModelDetails> = emptyMap(),
    ): List<ModelOption> =
        ids.mapIndexed { i, id ->
            val separator = id.indexOf('/')
            val providerId = id.takeIf { separator > 0 }?.substring(0, separator).orEmpty()
            val requestModel = id.takeIf { separator > 0 && separator < id.lastIndex }
                ?.substring(separator + 1)
                ?: id
            val details = detailsByReference[id]
            val metadata = details?.metadata ?: ModelMetadata()
            ModelOption(
                id = id,
                name = metadata.displayName ?: displayName(requestModel),
                desc = metadata.description ?: metadata.modelId ?: requestModel,
                tag = "",
                color = accents[i % accents.size],
                providerId = providerId,
                providerName = metadata.providerLabel ?: providerDisplayName(providerId),
                metadata = metadata,
                details = details,
            )
        }

    /** Search across visible labels, wire ids, provider names and metadata. */
    fun filter(models: List<ModelOption>, query: String): List<ModelOption> {
        val needle = query.trim()
        if (needle.isEmpty()) return models
        return models.filter { model ->
            sequenceOf(
                model.name,
                model.desc,
                model.id,
                model.providerName,
                model.metadata.searchableText,
            ).any { value -> value.contains(needle, ignoreCase = true) }
        }
    }

    /**
     * Resolve a provider profile deterministically. An enabled profile wins over
     * a stale disabled duplicate, then the more useful connection state wins.
     */
    fun providerStatus(
        providerId: String,
        statuses: List<ModelProviderStatus>,
    ): ModelProviderStatus? =
        statuses
            .asSequence()
            .filter { it.profileId.equals(providerId, ignoreCase = true) }
            .sortedWith(
                compareByDescending<ModelProviderStatus> { it.enabled }
                    .thenByDescending { it.status == ConnStatus.Connected }
                    .thenByDescending { it.status == ConnStatus.Configured },
            )
            .firstOrNull()

    /**
     * The recently-picked models the engine still offers, in RECENCY order (not
     * catalog order).
     *
     * A remembered reference whose provider was removed is silently dropped
     * rather than rendered as a row that cannot be selected. Callers pass the
     * already-filtered list, so a recent model that the current search query
     * excludes disappears from the recents group too.
     */
    fun recents(models: List<ModelOption>, recentRefs: List<String>): List<ModelOption> {
        if (recentRefs.isEmpty()) return emptyList()
        val byId = models.associateBy { it.id }
        return recentRefs.mapNotNull { byId[it] }
    }

    /**
     * Group already-curated picker rows by provider without adding, removing or
     * reordering any model. Provider sections follow first appearance order;
     * models inside each section follow the engine list verbatim.
     */
    fun groups(models: List<ModelOption>): List<ModelProviderGroup> {
        val grouped = linkedMapOf<String, MutableList<ModelOption>>()
        models.forEach { model ->
            grouped.getOrPut(model.providerId) { mutableListOf() }.add(model)
        }
        return grouped.map { (providerId, entries) ->
            ModelProviderGroup(
                id = providerId,
                name = entries.firstOrNull()?.providerName.orEmpty()
                    .ifBlank { providerDisplayName(providerId) },
                models = entries,
            )
        }
    }
}
