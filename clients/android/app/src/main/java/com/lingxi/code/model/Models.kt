package com.lingxi.code.model

import androidx.compose.ui.graphics.Color
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
)

data class Project(
    val id: String,
    val wsId: String,
    val name: String,
    val icon: String,
    val color: Color,
    val desc: String,
    val sessions: List<ProjectSession>,
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

data class ModelOption(
    val id: String,
    val name: String,
    val desc: String,
    val tag: String,
    val color: Color,
) {
    /** Name with the "Lingxi-" prefix stripped (composer chip label). */
    val shortName: String get() = name.replace("Lingxi-", "")
}

enum class Role { User, Ai }

/**
 * A single conversation turn. [id] is a fresh UUID so list diffing is stable
 * even when two messages share text (matches the iOS `Message` `let id = UUID()`).
 */
data class Message(
    val role: Role,
    val text: String,
    val tag: String? = null,
    val id: String = UUID.randomUUID().toString(),
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
        Chat("c1", "work", "重装 Claude Code", "今天", "nvm 残留清理完成", "2 小时前"),
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
            text = "已完成。\n\n**iPhone 版本设计要点**：\n\n1. **主界面 = 对话**。打开即进入最近会话，没有冗余首页。\n2. **左滑/汉堡 → 抽屉**，包含 workspace pill、session 列表、知识库、设置。\n3. **顶部 chip 显示工作流进度**，一行可滑动，与 Mac/iPad 一致。\n4. **底部胶囊 composer**，按住录音、点附件出 sheet。\n\n点击左上角菜单试试抽屉。",
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
 * active. SHIP-BLOCKER #2: the [available] ids and [active] id are REAL Anthropic
 * wire ids reported by the engine (`ModelList` / `ModelChanged`) — never the
 * branded `lx-*` mock ids. [EngineModelCatalog.from] turns this into the
 * picker's [ModelOption] rows, attaching a friendly display name without ever
 * losing the wire id (the id is what `SetModel` sends).
 *
 * The empty state ([available] empty) means "the engine hasn't reported its
 * catalog yet (or we're in mock mode)" — the UI keeps showing [MockData.models]
 * until a real `ModelList` arrives.
 */
data class EngineModelState(
    val available: List<String> = emptyList(),
    val active: String = "",
) {
    val hasCatalog: Boolean get() = available.isNotEmpty()
}

/**
 * Friendly-name + accent derivation for a REAL Anthropic wire id, so the picker
 * reads "Claude Opus 4" instead of `claude-opus-4-20250514` while still carrying
 * the wire id through [ModelOption.id]. PURE — no engine / Android dependency —
 * so it is unit-testable on the plain JVM.
 */
object EngineModelCatalog {

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

    /**
     * Turn a wire id into a human label. Handles the Anthropic id shape
     * (`claude-<family>-<ver>-<date>`): drops a trailing 8-digit date stamp,
     * title-cases the remaining segments, and capitalizes `claude`. Unknown
     * shapes fall back to the raw id (never blank), so a model the engine adds
     * tomorrow still renders sanely without a code change.
     */
    fun displayName(id: String): String {
        if (id.isBlank()) return id
        val parts = id.split('-').filter { it.isNotEmpty() }
        // Drop a trailing all-digit date/version stamp (e.g. "20250514").
        val trimmed = parts.dropLastWhile { it.length >= 6 && it.all(Char::isDigit) }
        val segments = trimmed.ifEmpty { parts }
        return segments.joinToString(" ") { seg ->
            when {
                seg.equals("claude", ignoreCase = true) -> "Claude"
                seg.all { it.isDigit() || it == '.' } -> seg // keep version numbers as-is
                else -> seg.replaceFirstChar { c -> c.uppercaseChar() }
            }
        }
    }

    /**
     * Build the picker rows from the engine's REAL [ids]. Each [ModelOption.id]
     * is the verbatim wire id (what `SetModel` sends); the name is a friendly
     * label and the dot color is keyed by position. Returns an empty list for an
     * empty catalog (the caller falls back to [MockData.models]).
     */
    fun options(ids: List<String>): List<ModelOption> =
        ids.mapIndexed { i, id ->
            ModelOption(
                id = id,
                name = displayName(id),
                desc = id, // the wire id, surfaced as the secondary line for transparency
                tag = "",
                color = accents[i % accents.size],
            )
        }
}
