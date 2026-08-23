package com.lingxi.code.model

import androidx.compose.ui.graphics.Color
import com.lingxi.code.R
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

/**
 * Connection state of a provider / MCP server.
 *
 * [label] intentionally stays the literal zh-Hans copy: [SettingsMockTest]
 * (`connStatus_labels_matchPrototype`, pure JVM, no `Context`) pins these
 * exact strings. The real, localized text is resolved at the one render site
 * ([com.lingxi.code.settings.ProviderPages]'s `ConnStatus.localizedLabel()`)
 * via `stringResource`, reusing the existing `settings_provider_status_*`
 * catalog keys — this raw field is never shown to the user directly.
 */
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

data class LlmProviderCatalogEntry(
    val profileId: String,
    val displayName: String,
    val baseUrl: String,
    val protocol: String,
    val auth: String,
    val credentialEnv: String?,
    val modelIds: List<String>,
    val modelDetails: List<CatalogModelDetails>,
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
    val schemaVersion: Int = CURRENT_SCHEMA_VERSION,
    val recognitionMode: String = MODE_AUTOMATIC,
    val language: String = LANGUAGE_AUTO,
    val voiceSelection: String = DEFAULT_VOICE_SELECTION,
    val rate: Float = 1.0f,
    val autoPlayReplies: Boolean = false,
) {
    // Legacy accessors kept so concurrent runtime callers continue to compile
    // while the settings and voice runtime converge on the shared contract.
    val inputProvider: String get() = "system"
    val inputLanguage: String get() = language
    val preset: String get() = if (voiceSelection.startsWith(SHERPA_VOICE_PREFIX)) "sherpa" else "system"
    val voiceId: String
        get() = when {
            voiceSelection == DEFAULT_VOICE_SELECTION -> DEFAULT_VOICE_ID
            voiceSelection.startsWith(SYSTEM_VOICE_PREFIX) -> voiceSelection.removePrefix(SYSTEM_VOICE_PREFIX)
            else -> voiceSelection
        }
    val speed: Float get() = rate
    val autoPlay: Boolean get() = autoPlayReplies
    val voiceLang: String
        get() = when (language.substringBefore('-').lowercase()) {
            "zh" -> "zh"
            "en" -> "en"
            else -> ""
        }

    companion object {
        const val CURRENT_SCHEMA_VERSION = 2
        const val MODE_AUTOMATIC = "automatic"
        const val MODE_LOCAL_ONLY = "localOnly"
        const val LANGUAGE_AUTO = "auto"
        const val DEFAULT_VOICE_ID = "default"
        const val SYSTEM_VOICE_PREFIX = "system:"
        const val SHERPA_VOICE_PREFIX = "sherpa:"
        const val DEFAULT_VOICE_SELECTION = "${SYSTEM_VOICE_PREFIX}${DEFAULT_VOICE_ID}"

        fun systemVoiceSelection(id: String): String =
            "${SYSTEM_VOICE_PREFIX}${id.ifBlank { DEFAULT_VOICE_ID }}"

        fun sherpaVoiceSelection(modelId: String, voiceId: String): String =
            "${SHERPA_VOICE_PREFIX}$modelId:${voiceId}"
    }
}

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

/**
 * `name`/`sub` below intentionally stay the literal zh-Hans copy. `Presets`
 * has no `Context`, and is consumed both from real render sites (the
 * preset picker, [com.lingxi.code.settings.ProviderPages]) and from a
 * plain-data seed path ([ProviderSettingsRepository.newProvider] copies
 * [ProviderPreset.name] into a freshly-created [GenericProvider.name], the
 * same way `editing.name` is otherwise free user-editable text). The real,
 * localized text is resolved at the render sites via small `id`-keyed
 * `stringResource` lookups (`ProviderPreset.localizedName()`/`.localizedSub()`
 * in `ProviderPages.kt`, `SimplePages.kt`'s voice preset radio, and
 * `MainSettingsPage.kt`'s voice-preset summary), reusing the existing
 * `settings_provider_preset_*_name`/`_sub` catalog keys — this object is
 * confirmed LIVE for all four catalogs (`llm`/`search`/`fetch`/`voice` are
 * all reachable and rendered on Android, unlike the iOS port where
 * `Presets.search`/`Presets.fetch` are dead).
 */
object Presets {
    /**
     * A preset's `models` are the radio choices AND the fallback written into
     * `GenericProvider.model` when a provider is added, which
     * `buildEngineLaunchConfig` turns into the engine's `default_model`. Every
     * id here must therefore be one the engine still curates
     * (`traits::is_curated_model`) — a stale id is registered only because it
     * is the configured default, so `curated_model_refs` pins it and the picker
     * renders it as a stray row above the real shortlist, the same wart
     * `MobileEngineConfig::default()` was moved off `claude-sonnet-4-20250514`
     * to avoid. Anthropic's list mirrors `is_curated_model`'s "anthropic" arm,
     * `provider_default_model` first, and matches the iOS `Presets.llm` twin.
     */
    val llm: List<ProviderPreset> = listOf(
        ProviderPreset("anthropic", "Anthropic", "Claude API", Color(red = 0.9351f, green = 0.5079f, blue = 0.4015f), "https://api.anthropic.com", "sk-ant-", listOf("claude-opus-5", "claude-fable-5", "claude-sonnet-5", "claude-haiku-4-5")),
        ProviderPreset("openai", "OpenAI", "ChatGPT API", Color(red = 0.1326f, green = 0.7261f, blue = 0.5350f), "https://api.openai.com/v1", "sk-proj-", listOf("gpt-5.6-sol", "gpt-5.6-terra", "gpt-5.6-luna")),
        ProviderPreset("google", "Google", "Gemini API", Color(red = 0.3503f, green = 0.6649f, blue = 0.9741f), "https://generativelanguage.googleapis.com/v1", "AIza", listOf("gemini-3.7-flash", "gemini-3.6-flash", "gemini-3.5-flash", "gemini-3.1-pro-preview")),
        ProviderPreset("deepseek", "DeepSeek", "DeepSeek API", Color(red = 0.6451f, green = 0.5662f, blue = 1.0000f), "https://api.deepseek.com", "sk-", listOf("deepseek-v4-flash", "deepseek-v4-flash-vision-exp", "deepseek-v4-pro")),
        ProviderPreset("kimi", "Kimi", "Moonshot AI", Color(red = 0.4340f, green = 0.5865f, blue = 1.0000f), "https://api.moonshot.cn/v1", "sk-", listOf("kimi-k3")),
        ProviderPreset("kimi-code", "Kimi Code", "编程会员套餐", Color(red = 0.2784f, green = 0.6980f, blue = 0.9490f), "https://api.kimi.com/coding/v1", "sk-", listOf("k3")),
        ProviderPreset("qwen", "通义千问", "DashScope", Color(red = 0.8826f, green = 0.6256f, blue = 0.2074f), "https://dashscope.aliyuncs.com/v1", "sk-", listOf("qwen-max", "qwen-plus", "qwen-turbo")),
        ProviderPreset("openrouter", "OpenRouter", "多模型聚合", Color(red = 0.0000f, green = 0.7441f, blue = 0.7802f), "https://openrouter.ai/api/v1", "sk-or-", listOf("openrouter/auto", "~anthropic/claude-sonnet-latest", "~openai/gpt-latest", "~openai/gpt-mini-latest", "~google/gemini-flash-latest")),
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
        ProviderPreset("system", "系统语音", "设备本地 · 免费", Color(red = 0.5728f, green = 0.6177f, blue = 0.7466f), "", "", emptyList()),
    )
}

/**
 * The three kinds of provider list (LLM / web search / web fetch).
 *
 * [titleRes] is a string resource id (not a stored `String`) so this enum
 * needs no `Context` to construct; callers resolve it with `stringResource`
 * at the render site.
 */
enum class ProviderKind(val titleRes: Int, val idPrefix: String) {
    Llm(R.string.settings_llm_providers, "l"),
    Search(R.string.settings_web_search, "s"),
    Fetch(R.string.settings_web_fetch, "f");

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

    /**
     * [resolve] maps (string resource id, zh-Hans fallback) to the localized
     * text; the default returns [fallback] verbatim so [SettingsMockTest]
     * (pure JVM, no `Context`) and any preview keep seeing the original
     * Chinese copy unmodified. The production caller ([SettingsStore]'s
     * `_state` init) passes its real Context-backed resolver instead.
     */
    fun skills(resolve: (id: Int, fallback: String) -> String = { _, fallback -> fallback }): List<Skill> = listOf(
        Skill(
            "sk1",
            resolve(R.string.settings_skill_seed_weekly_report_name, "周报生成"),
            "官方",
            resolve(R.string.settings_skill_seed_weekly_report_desc, "聚合 Linear / GitHub / 日历自动出周报"),
            listOf(
                resolve(R.string.settings_skill_seed_weekly_report_trigger_1, "每周五 17:00"),
                resolve(R.string.settings_skill_seed_weekly_report_trigger_2, "@周报"),
            ),
            enabled = true,
            builtin = true,
        ),
        Skill(
            "sk2",
            resolve(R.string.settings_skill_seed_code_review_name, "代码评审"),
            "官方",
            resolve(R.string.settings_skill_seed_code_review_desc, "对粘贴的 diff 给出严格 review"),
            listOf("/review", resolve(R.string.settings_skill_seed_code_review_trigger_2, "拖入 .diff")),
            enabled = true,
            builtin = true,
        ),
        Skill(
            "sk3",
            resolve(R.string.settings_skill_seed_meeting_notes_name, "会议纪要"),
            "官方",
            resolve(R.string.settings_skill_seed_meeting_notes_desc, "从语音/文本提取要点 + action item"),
            listOf(resolve(R.string.settings_skill_seed_meeting_notes_trigger_1, "会议结束后")),
            enabled = true,
            builtin = true,
        ),
        Skill(
            "sk4",
            resolve(R.string.settings_skill_seed_paper_reading_name, "论文精读"),
            "社区 · @arxiv-fan",
            resolve(R.string.settings_skill_seed_paper_reading_desc, "arXiv 链接 → 结构化摘要 + 批注"),
            listOf(resolve(R.string.settings_skill_seed_paper_reading_trigger_1, "粘贴 arxiv URL")),
            enabled = false,
            builtin = false,
        ),
        Skill(
            "sk5",
            "CSS Doctor",
            "社区 · @lin",
            resolve(R.string.settings_skill_seed_css_doctor_desc, "诊断布局问题并给出修复"),
            listOf("/css"),
            enabled = false,
            builtin = false,
        ),
        Skill(
            "sk6",
            resolve(R.string.settings_skill_seed_polish_name, "英文润色"),
            "我",
            resolve(R.string.settings_skill_seed_polish_desc, "中→英写作润色，保留原意"),
            listOf("/polish"),
            enabled = true,
            builtin = false,
        ),
    )

    /**
     * The mobile bundled catalog. This mirrors the engine's compiled registry
     * (`skill-api`'s `BUILTIN_MOBILE`) instead of inventing Android-only
     * placeholder rows, so the settings surface exposes the same five Local
     * Apps skills as slash discovery.
     *
     * [resolve] carries the same (string resource id, zh-Hans fallback)
     * contract as [skills] and MUST be passed by any production caller — the
     * descriptions are user-facing prose, and a resolver-less call renders
     * Simplified Chinese on an English / Japanese / Korean / zh-TW device.
     *
     * `name` is deliberately NOT localized: it is the skill's canonical
     * identifier, the same string the `/create-local-app` trigger and the
     * engine's registry use. Translating it would break that correspondence.
     */
    fun bundledSkills(resolve: (id: Int, fallback: String) -> String = { _, fallback -> fallback }): List<Skill> = listOf(
        Skill(
            "create-local-app",
            "create-local-app",
            "官方",
            resolve(
                R.string.settings_skill_bundled_create_local_app_desc,
                "编排设计、依赖、生成、构建和验证本地应用",
            ),
            listOf("/create-local-app"),
            enabled = true,
            builtin = true,
        ),
        Skill(
            "frontend-design",
            "frontend-design",
            "官方",
            resolve(
                R.string.settings_skill_bundled_frontend_design_desc,
                "为目标平台和 form factor 设计原生前端方向",
            ),
            listOf("/frontend-design"),
            enabled = true,
            builtin = true,
        ),
        Skill(
            "frontend-qa",
            "frontend-qa",
            "官方",
            resolve(
                R.string.settings_skill_bundled_frontend_qa_desc,
                "用 Browser 与原生 WebView 验证界面和交互",
            ),
            listOf("/frontend-qa"),
            enabled = true,
            builtin = true,
        ),
        Skill(
            "accessibility",
            "accessibility",
            "官方",
            resolve(
                R.string.settings_skill_bundled_accessibility_desc,
                "检查语义、键盘、触控、对比度和动态字体",
            ),
            listOf("/accessibility"),
            enabled = true,
            builtin = true,
        ),
        Skill(
            "react-best-practices",
            "react-best-practices",
            "官方",
            resolve(
                R.string.settings_skill_bundled_react_best_practices_desc,
                "检查 React 状态、效果、性能和完整状态",
            ),
            listOf("/react-best-practices"),
            enabled = true,
            builtin = true,
        ),
    )

    fun mcpServers(resolve: (id: Int, fallback: String) -> String = { _, fallback -> fallback }): List<MCPServer> = listOf(
        MCPServer("mcp1", "Filesystem", "stdio://npx -y @modelcontextprotocol/server-filesystem", 8, ConnStatus.Connected, enabled = true, transport = "stdio"),
        MCPServer("mcp2", "GitHub", "https://mcp.github.com", 14, ConnStatus.Connected, enabled = true, transport = "sse", auth = "oauth"),
        MCPServer("mcp3", "Linear", "https://mcp.linear.app", 6, ConnStatus.Connected, enabled = true, transport = "sse", auth = "oauth"),
        MCPServer("mcp4", "Notion", "https://mcp.notion.com", 12, ConnStatus.Idle, enabled = false, transport = "sse", auth = "oauth"),
        MCPServer(
            "mcp5",
            resolve(R.string.settings_mcp_seed_postgres_local_name, "Postgres (本地)"),
            "stdio://uvx mcp-server-postgres",
            4,
            ConnStatus.Error,
            enabled = true,
            transport = "stdio",
        ),
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
