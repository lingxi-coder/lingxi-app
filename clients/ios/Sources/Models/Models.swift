import SwiftUI

// MARK: - Domain models + mock data (verbatim from lingxi-iphone.html)

struct Workspace: Identifiable, Equatable {
    let id: String
    let name: String
    let icon: String
    let color: Color
}

struct Chat: Identifiable, Equatable {
    let id: String
    let wsId: String
    let title: String
    let group: String
    let preview: String
    let activity: String
}

struct ProjectSession: Identifiable, Equatable {
    let id: String
    let title: String
    let activity: String
    let preview: String
    var pinned: Bool = false
    let msgs: Int
}

struct Project: Identifiable, Equatable {
    let id: String
    let wsId: String
    let name: String
    let icon: String
    let color: Color
    let desc: String
    let sessions: [ProjectSession]
}

struct Cron: Identifiable, Equatable {
    let id: String
    let wsId: String
    let title: String
    let cron: String
    let next: String
    let desc: String
    let enabled: Bool
}

struct ModelOption: Identifiable, Equatable {
    let id: String
    let name: String
    let desc: String
    let tag: String
    let color: Color
    /// Name with the "Lingxi-" prefix stripped (composer chip label).
    var shortName: String { name.replacingOccurrences(of: "Lingxi-", with: "") }
}

enum Role { case user, ai }

struct Message: Identifiable, Equatable {
    let id = UUID()
    let role: Role
    var tag: String? = nil
    let text: String
}

/// A unified "session" reference used by ChatView's title bar.
struct SessionRef: Identifiable, Equatable {
    let id: String
    let title: String
}

// MARK: - Mock data ---------------------------------------------------------

enum MockData {
    static let workspaces: [Workspace] = [
        .init(id: "personal", name: "个人", icon: "◐", color: Color(srgb: 0.4340, 0.5865, 1.0000)), // oklch(70% 0.18 268)
        .init(id: "work",     name: "工作", icon: "◑", color: Color(srgb: 0.0000, 0.7601, 0.7664)), // oklch(70% 0.16 195)
        .init(id: "research", name: "研究", icon: "◒", color: Color(srgb: 0.8090, 0.4552, 0.8891)), // oklch(72% 0.18 320)
        .init(id: "creative", name: "创作", icon: "◓", color: Color(srgb: 0.8696, 0.5765, 0.0000)), // oklch(72% 0.16 75)
    ]

    static let chats: [Chat] = [
        .init(id: "c1", wsId: "work", title: "重装 Claude Code", group: "今天", preview: "nvm 残留清理完成", activity: "2 小时前"),
        .init(id: "c2", wsId: "work", title: "客户邮件回复模板", group: "昨天", preview: "已生成 4 套话术", activity: "昨天"),
        .init(id: "c3", wsId: "work", title: "上海差旅规划", group: "本周", preview: "机酒路线", activity: "周二"),
    ]

    static let projects: [Project] = [
        .init(id: "p1", wsId: "work", name: "灵犀 OS 设计", icon: "◑",
              color: Color(srgb: 0.4340, 0.5865, 1.0000), // oklch(70% 0.18 268)
              desc: "多端 UI · 14 文件 · 8 记忆",
              sessions: [
                .init(id: "s1",   title: "设计灵犀 iPhone 版", activity: "刚刚",   preview: "类 Claude 移动端布局", pinned: true, msgs: 8),
                .init(id: "p1s2", title: "iPad 横屏推演",      activity: "昨天",   preview: "Pencil 标注入口", msgs: 14),
                .init(id: "p1s3", title: "深色色板校准",        activity: "5月3日", preview: "oklch 节点对齐", msgs: 22),
              ]),
        .init(id: "p2", wsId: "work", name: "Q2 OKR & 周报", icon: "◐",
              color: Color(srgb: 0.0000, 0.7601, 0.7664), // oklch(72% 0.16 195)
              desc: "目标对齐 · 6 文件 · 3 记忆",
              sessions: [
                .init(id: "p2s1", title: "整理 Q2 OKR 草案", activity: "5 小时前", preview: "已对齐三方", msgs: 24),
                .init(id: "p2s2", title: "周报自动化模板",   activity: "昨天",     preview: "从多源聚合", msgs: 6),
              ]),
        .init(id: "p3", wsId: "work", name: "Code & 工程", icon: "◇",
              color: Color(srgb: 0.2085, 0.7571, 0.4656), // oklch(72% 0.16 155)
              desc: "Bug 排查 · 23 文件 · 12 记忆",
              sessions: [
                .init(id: "p3s1", title: "WebSocket 重连排查", activity: "昨天", preview: "指数退避方案", msgs: 31),
                .init(id: "p3s2", title: "PRD v2 评审反馈",   activity: "周一", preview: "12 评论 4 待办", msgs: 18),
              ]),
    ]

    static let crons: [Cron] = [
        .init(id: "cr1", wsId: "work", title: "每日晨报",     cron: "工作日 08:30", next: "明早 08:30",  desc: "聚合 Linear/GitHub/邮件 → 早会摘要", enabled: true),
        .init(id: "cr2", wsId: "work", title: "周报自动生成", cron: "每周五 17:00", next: "周五 17:00",  desc: "git 提交 + 日历 → 周报草稿", enabled: true),
        .init(id: "cr3", wsId: "work", title: "客户反馈周聚合", cron: "每周一 09:00", next: "下周一 09:00", desc: "7 天工单聚类 + 情感分析", enabled: true),
        .init(id: "cr4", wsId: "work", title: "凌晨日志巡检", cron: "每日 03:00",   next: "— 已暂停",     desc: "错误日志分类 + 告警", enabled: false),
    ]

    static let models: [ModelOption] = [
        .init(id: "lx-72b",   name: "Lingxi-72B",   desc: "主力", tag: "默认", color: Color(srgb: 0.4340, 0.5865, 1.0000)),
        .init(id: "lx-72b-r", name: "Lingxi-72B-R", desc: "推理", tag: "慢",   color: Color(srgb: 0.8090, 0.4552, 0.8891)),
        .init(id: "lx-32b",   name: "Lingxi-32B",   desc: "高速", tag: "快",   color: Color(srgb: 0.0000, 0.7601, 0.7664)),
        .init(id: "lx-code",  name: "Lingxi-Code",  desc: "代码", tag: "编程", color: Color(srgb: 0.2085, 0.7571, 0.4656)),
    ]

    static let messagesDefault: [Message] = [
        .init(role: .user, text: "帮我做一版手机上的灵犀 AI 助手，参考 Claude iOS 应用的极简风格，但要保留多 workspace 和工作流的能力。"),
        .init(role: .ai, tag: "思考了 48 秒", text: "已完成。\n\n**iPhone 版本设计要点**：\n\n1. **主界面 = 对话**。打开即进入最近会话，没有冗余首页。\n2. **左滑/汉堡 → 抽屉**，包含 workspace pill、session 列表、知识库、设置。\n3. **顶部 chip 显示工作流进度**，一行可滑动，与 Mac/iPad 一致。\n4. **底部胶囊 composer**，按住录音、点附件出 sheet。\n\n点击左上角菜单试试抽屉。"),
        .init(role: .user, text: "能不能加个语音\"心流\"模式？随时按住屏幕说话，松开发送。"),
        .init(role: .ai, tag: "思考了 12 秒", text: "已加。**按住屏幕任意位置 0.6 秒**会进入沉浸录音态：背景虚化，中央波形脉动，松开立即发送给当前模型。键盘/composer 临时隐藏。\n\n再次按住录音时，AI 的上一条回复会变为半透明，提示\"上下文已记入\"。"),
    ]

    /// Flattened session lookup (chats + every project session).
    static var allSessions: [SessionRef] {
        var refs = chats.map { SessionRef(id: $0.id, title: $0.title) }
        for p in projects {
            refs.append(contentsOf: p.sessions.map { SessionRef(id: $0.id, title: $0.title) })
        }
        return refs
    }

    static func session(_ id: String) -> SessionRef {
        allSessions.first(where: { $0.id == id }) ?? allSessions[0]
    }
}
