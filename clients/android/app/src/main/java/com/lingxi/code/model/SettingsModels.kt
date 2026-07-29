package com.lingxi.code.model

import androidx.compose.ui.graphics.Color
import com.lingxi.code.theme.Palette
import java.util.UUID

/**
 * Settings domain models + mock data, ported 1:1 from the iOS
 * `SettingsModels.swift` (itself a verbatim port of the prototype's
 * `SettingsSheet` useState block). This is the canonical settings mock dataset
 * the Android settings surface renders.
 *
 * Provider/preset accent colors reuse the exact `Color(srgb: r, g, b)` values
 * from the iOS sources, mapped to Compose `Color(red = r, green = g, blue = b)`.
 * A6 consumes the counts on the main list; A7/A8 build the provider / skill /
 * MCP / Dream editors on top of this same data.
 */

/** Connection state of a provider / MCP server. */
enum class ConnStatus(val label: String) {
    Configured("已配置"),
    Connected("已连接"),
    Idle("未验证"),
    Testing("检测中…"),
    Error("连接失败");

    /** Dot color for a given palette (idle resolves to text4). */
    fun dot(t: Palette): Color = when (this) {
        Configured -> t.accent
        Connected -> t.statusConnected
        Idle -> t.text4
        Testing -> t.statusTesting
        Error -> t.statusError
    }
}

// MARK: - Presets ------------------------------------------------------------

data class ProviderPreset(
    val id: String,
    val name: String,
    val sub: String,
    val color: Color,
    val defaultUrl: String,
    val keyPrefix: String,
    val models: List<String>,
    val needsCx: Boolean = false,
)

data class GenericProvider(
    val id: String,
    val preset: String,
    val name: String,
    val url: String,
    val key: String,
    val model: String = "",
    val cx: String = "",
    val status: ConnStatus,
    val isDefault: Boolean = false,
    val enabled: Boolean,
    val credentialConfigured: Boolean = false,
)

data class Skill(
    val id: String,
    val name: String,
    val author: String,
    val desc: String,
    val triggers: List<String>,
    val enabled: Boolean,
    val builtin: Boolean,
)

data class MCPServer(
    val id: String,
    val name: String,
    val url: String,
    val tools: Int,
    val status: ConnStatus,
    val enabled: Boolean,
    val transport: String,
    val auth: String? = null,
)

data class VoiceConfig(
    val preset: String = "system",
    val voiceId: String = "zh-CN-XiaoxiaoNeural",
    val speed: Float = 1.0f,
    val autoPlay: Boolean = false,
)

data class DreamConfig(
    val enabled: Boolean = true,
    val window: String = "night",          // night | always | custom
    val onCharging: Boolean = true,
    val onWifi: Boolean = true,
    val activities: Map<String, Boolean> = mapOf(
        "reorganize" to true, "plan" to true, "recap" to true,
        "prefetch" to false, "polish" to false,
    ),
    val budget: String = "medium",          // low | medium | high
    val lastRun: String = "今早 03:24 · 整理 7 条记忆 / 草拟今日计划",
)

data class NotifConfig(
    val workflows: Boolean = true,
    val mentions: Boolean = true,
    val crons: Boolean = true,
    val marketing: Boolean = false,
) {
    val enabledCount: Int get() = listOf(workflows, mentions, crons, marketing).count { it }
}

// MARK: - Presets data -------------------------------------------------------

object Presets {
    val llm: List<ProviderPreset> = listOf(
        ProviderPreset("anthropic", "Anthropic", "Claude API", Color(red = 0.9351f, green = 0.5079f, blue = 0.4015f), "https://api.anthropic.com", "sk-ant-", listOf("claude-sonnet-4-5", "claude-opus-4", "claude-haiku-4-5")),
        ProviderPreset("openai", "OpenAI", "ChatGPT API", Color(red = 0.1326f, green = 0.7261f, blue = 0.5350f), "https://api.openai.com/v1", "sk-proj-", listOf("gpt-4o", "gpt-4o-mini", "o1-preview")),
        ProviderPreset("google", "Google", "Gemini API", Color(red = 0.3503f, green = 0.6649f, blue = 0.9741f), "https://generativelanguage.googleapis.com/v1", "AIza", listOf("gemini-2.5-pro", "gemini-2.5-flash")),
        ProviderPreset("deepseek", "DeepSeek", "DeepSeek API", Color(red = 0.6451f, green = 0.5662f, blue = 1.0000f), "https://api.deepseek.com", "sk-", listOf("deepseek-v4-flash", "deepseek-v4-pro")),
        ProviderPreset("qwen", "通义千问", "DashScope", Color(red = 0.8826f, green = 0.6256f, blue = 0.2074f), "https://dashscope.aliyuncs.com/v1", "sk-", listOf("qwen-max", "qwen-plus", "qwen-turbo")),
        ProviderPreset("openrouter", "OpenRouter", "多模型聚合", Color(red = 0.0000f, green = 0.7441f, blue = 0.7802f), "https://openrouter.ai/api/v1", "sk-or-", listOf("anthropic/claude-sonnet-4.5", "openai/gpt-4o", "google/gemini-2.5-pro")),
        ProviderPreset("custom", "自定义", "OpenAI 兼容端点", Color(red = 0.5728f, green = 0.6177f, blue = 0.7466f), "https://", "", emptyList()),
    )

    val search: List<ProviderPreset> = listOf(
        ProviderPreset("google", "Google", "官方 Custom Search", Color(red = 0.3503f, green = 0.6649f, blue = 0.9741f), "https://www.googleapis.com/customsearch/v1", "", emptyList(), needsCx = true),
        ProviderPreset("brave", "Brave", "独立索引 · 隐私优先", Color(red = 0.8716f, green = 0.2418f, blue = 0.1752f), "https://api.search.brave.com/res/v1", "", emptyList()),
        ProviderPreset("tavily", "Tavily", "AI 优化搜索", Color(red = 0.0000f, green = 0.7151f, blue = 0.7672f), "https://api.tavily.com", "", emptyList()),
        ProviderPreset("serper", "Serper", "Google 代理", Color(red = 0.0000f, green = 0.7391f, blue = 0.5219f), "https://google.serper.dev", "", emptyList()),
        ProviderPreset("bing", "Bing", "Microsoft", Color(red = 0.0000f, green = 0.7200f, blue = 0.8810f), "https://api.bing.microsoft.com/v7.0", "", emptyList()),
    )

    val fetch: List<ProviderPreset> = listOf(
        ProviderPreset("jina", "Jina Reader", "免费 · 推荐", Color(red = 0.8713f, green = 0.5800f, blue = 0.0000f), "https://r.jina.ai", "", emptyList()),
        ProviderPreset("firecrawl", "Firecrawl", "渲染 JS · 结构化", Color(red = 1.0000f, green = 0.4030f, blue = 0.1579f), "https://api.firecrawl.dev/v1", "", emptyList()),
        ProviderPreset("browserless", "Browserless", "Headless Chrome", Color(red = 0.6203f, green = 0.5486f, blue = 0.9581f), "https://chrome.browserless.io", "", emptyList()),
        ProviderPreset("scrapingbee", "ScrapingBee", "反爬代理", Color(red = 0.8960f, green = 0.6013f, blue = 0.0000f), "https://app.scrapingbee.com/api/v1", "", emptyList()),
    )

    val voice: List<ProviderPreset> = listOf(
        ProviderPreset("elevenlabs", "ElevenLabs", "高质量 · 多语种", Color(red = 0.8018f, green = 0.4038f, blue = 0.8909f), "https://api.elevenlabs.io/v1", "", emptyList()),
        ProviderPreset("openai-tts", "OpenAI TTS", "低延迟", Color(red = 0.1326f, green = 0.7261f, blue = 0.5350f), "https://api.openai.com/v1", "", emptyList()),
        ProviderPreset("system", "系统语音", "设备本地 · 免费", Color(red = 0.5728f, green = 0.6177f, blue = 0.7466f), "", "", emptyList()),
    )
}

/** The three kinds of provider list (LLM / web search / web fetch). */
enum class ProviderKind(val title: String, val idPrefix: String) {
    Llm("LLM 提供商", "l"),
    Search("联网搜索", "s"),
    Fetch("网页抓取", "f");

    val presets: List<ProviderPreset>
        get() = when (this) {
            Llm -> Presets.llm
            Search -> Presets.search
            Fetch -> Presets.fetch
        }
}

// MARK: - Canonical settings mock data (the iOS SettingsStore defaults) -------

/**
 * The verbatim default settings dataset. Held separately from the mutable
 * [com.lingxi.code.settings.SettingsStore] so previews/tests can read the
 * canonical seed without a ViewModel.
 */
object SettingsMock {
    val llmProviders: List<GenericProvider> = listOf(
        GenericProvider("p_ant", "anthropic", "Anthropic", "https://api.anthropic.com", "sk-ant-api03-••••••••7Hq2", "claude-sonnet-4-5", status = ConnStatus.Connected, isDefault = true, enabled = true),
        GenericProvider("p_oai", "openai", "OpenAI", "https://api.openai.com/v1", "sk-proj-••••••••4nQ8", "gpt-4o", status = ConnStatus.Idle, enabled = true),
        GenericProvider("p_dsk", "deepseek", "DeepSeek", "https://api.deepseek.com", "", "deepseek-v4-flash", status = ConnStatus.Idle, enabled = false),
    )

    val searchProviders: List<GenericProvider> = listOf(
        GenericProvider("s_brv", "brave", "Brave", "https://api.search.brave.com/res/v1", "BSA••••••a9Z", status = ConnStatus.Connected, isDefault = true, enabled = true),
        GenericProvider("s_jin", "tavily", "Tavily", "https://api.tavily.com", "", status = ConnStatus.Idle, enabled = false),
    )

    val fetchProviders: List<GenericProvider> = listOf(
        GenericProvider("f_jin", "jina", "Jina Reader", "https://r.jina.ai", "", status = ConnStatus.Connected, isDefault = true, enabled = true),
    )

    val skills: List<Skill> = listOf(
        Skill("sk1", "周报生成", "官方", "聚合 Linear / GitHub / 日历自动出周报", listOf("每周五 17:00", "@周报"), enabled = true, builtin = true),
        Skill("sk2", "代码评审", "官方", "对粘贴的 diff 给出严格 review", listOf("/review", "拖入 .diff"), enabled = true, builtin = true),
        Skill("sk3", "会议纪要", "官方", "从语音/文本提取要点 + action item", listOf("会议结束后"), enabled = true, builtin = true),
        Skill("sk4", "论文精读", "社区 · @arxiv-fan", "arXiv 链接 → 结构化摘要 + 批注", listOf("粘贴 arxiv URL"), enabled = false, builtin = false),
        Skill("sk5", "CSS Doctor", "社区 · @lin", "诊断布局问题并给出修复", listOf("/css"), enabled = false, builtin = false),
        Skill("sk6", "英文润色", "我", "中→英写作润色，保留原意", listOf("/polish"), enabled = true, builtin = false),
    )

    val mcpServers: List<MCPServer> = listOf(
        MCPServer("mcp1", "Filesystem", "stdio://npx -y @modelcontextprotocol/server-filesystem", 8, ConnStatus.Connected, enabled = true, transport = "stdio"),
        MCPServer("mcp2", "GitHub", "https://mcp.github.com", 14, ConnStatus.Connected, enabled = true, transport = "sse", auth = "oauth"),
        MCPServer("mcp3", "Linear", "https://mcp.linear.app", 6, ConnStatus.Connected, enabled = true, transport = "sse", auth = "oauth"),
        MCPServer("mcp4", "Notion", "https://mcp.notion.com", 12, ConnStatus.Idle, enabled = false, transport = "sse", auth = "oauth"),
        MCPServer("mcp5", "Postgres (本地)", "stdio://uvx mcp-server-postgres", 4, ConnStatus.Error, enabled = true, transport = "stdio"),
    )

    /** Build a fresh provider from a preset (used by the A7 add-provider flow). */
    fun newProvider(kind: ProviderKind, presetId: String): GenericProvider {
        val preset = kind.presets.first { it.id == presetId }
        val id = kind.idPrefix + "_" + UUID.randomUUID().toString().take(5).lowercase()
        return GenericProvider(
            id = id, preset = presetId, name = preset.name, url = preset.defaultUrl,
            key = "", model = preset.models.firstOrNull() ?: "",
            status = ConnStatus.Idle, enabled = true,
        )
    }
}
