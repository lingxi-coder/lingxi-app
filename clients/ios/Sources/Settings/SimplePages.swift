import SwiftUI

// MARK: - Account
struct AccountPage: View {
    @Environment(\.theme) private var t
    var body: some View {
        VStack(spacing: 0) {
            VStack(spacing: 0) {
                Circle().fill(LinearGradient(colors: [t.accent, t.accent2], startPoint: .topLeading, endPoint: .bottomTrailing))
                    .frame(width: 76, height: 76)
                    .overlay(Text("Y").font(.system(size: 30, weight: .semibold)).foregroundColor(.white))
                    .padding(.bottom, 12)
                Text("Yuxin Yang").font(.system(size: 18, weight: .bold)).foregroundColor(t.text)
                Text("yuxin@axielix.com").font(.system(size: 13)).foregroundColor(t.text4).padding(.top, 4)
                Text("Pro · 续费日 2026-09-30")
                    .font(.system(size: 11.5, weight: .semibold)).foregroundColor(t.accent)
                    .padding(.horizontal, 12).padding(.vertical, 4)
                    .background(t.accent.tint(0.18)).clipShape(Capsule()).padding(.top, 10)
            }
            .padding(.top, 8).padding(.bottom, 18)

            SettingsSection(label: "本月用量") {
                SettingsRow(label: "对话次数", value: "247 / 1000", chevron: false)
                SettingsRow(label: "推理时长", value: "5.5 / 8 小时", chevron: false)
                SettingsRow(label: "存储", value: "1.2 / 10 GB", chevron: false, isLast: true)
            }
            SettingsSection {
                SettingsRow(icon: .brain, label: "管理订阅", onTap: {})
                SettingsRow(icon: .link, label: "同步设备", sub: "3 台设备已连接", onTap: {})
                SettingsRow(icon: .x, label: "退出登录", danger: true, isLast: true, onTap: {})
            }
        }
    }
}

// MARK: - Knowledge
struct KnowledgePage: View {
    @Environment(\.theme) private var t
    @State private var autoRecall = true
    var body: some View {
        VStack(spacing: 0) {
            blurb("知识库内容会被嵌入并附加到 AI 上下文。所有索引在本机完成。")
            SettingsSection(label: "存储") {
                SettingsRow(label: "使用空间", value: "142 MB", chevron: false)
                SettingsRow(label: "文件数", value: "24 个", chevron: false)
                SettingsRow(label: "索引模型", value: "bge-m3-local", isLast: true, onTap: {})
            }
            SettingsSection(label: "行为") {
                SettingsRow(label: "自动检索", sub: "每次提问自动召回相关片段", chevron: false) { LXToggle(isOn: $autoRecall) }
                SettingsRow(label: "召回数量上限", value: "8 段", isLast: true, onTap: {})
            }
        }
    }
}

// MARK: - Memory
struct MemoryPage: View {
    @Environment(\.theme) private var t
    var body: some View {
        VStack(spacing: 0) {
            blurb("灵犀根据对话自动提取关于你的偏好、习惯、关系。你可以随时编辑或删除。")
            SettingsSection(label: "近期记忆") {
                SettingsRow(label: "偏好深色 + 中文", sub: "2026-05-12 形成", onTap: {})
                SettingsRow(label: "工作日 8:30 倾向收到晨报", sub: "2026-05-10 形成", onTap: {})
                SettingsRow(label: "正在做 AxieLix 灵犀项目", sub: "2026-05-08 形成", isLast: true, onTap: {})
            }
            SettingsSection {
                SettingsRow(icon: .x, label: "清除全部记忆", chevron: false, danger: true, isLast: true, onTap: {})
            }
        }
    }
}

// MARK: - Workflows
struct WorkflowsPage: View {
    @State private var w1 = true
    @State private var w2 = true
    @State private var w3 = true
    @State private var w4 = false
    var body: some View {
        SettingsSection(label: "自动化", footer: "工作流由 cron 表达式或事件触发。在主界面侧栏 → 定时 可创建。") {
            SettingsRow(label: "每日晨报", sub: "工作日 08:30", chevron: false) { LXToggle(isOn: $w1) }
            SettingsRow(label: "周报自动生成", sub: "每周五 17:00", chevron: false) { LXToggle(isOn: $w2) }
            SettingsRow(label: "客户反馈周聚合", sub: "每周一 09:00", chevron: false) { LXToggle(isOn: $w3) }
            SettingsRow(label: "凌晨日志巡检", sub: "已暂停", chevron: false, isLast: true) { LXToggle(isOn: $w4) }
        }
    }
}

// MARK: - Notifications
struct NotificationsPage: View {
    @ObservedObject var store: SettingsStore
    var body: some View {
        SettingsSection(label: "通知类型", footer: "所有通知通过系统通知中心，灵犀不会单独打扰你。") {
            SettingsRow(label: "工作流完成", sub: "AI 跑完多步任务时", chevron: false) { LXToggle(isOn: $store.notifs.workflows) }
            SettingsRow(label: "我被 @", sub: "会话内有人提到你", chevron: false) { LXToggle(isOn: $store.notifs.mentions) }
            SettingsRow(label: "定时任务报告", sub: "cron 触发执行后", chevron: false) { LXToggle(isOn: $store.notifs.crons) }
            SettingsRow(label: "产品更新", sub: "新功能与重要变更", chevron: false, isLast: true) { LXToggle(isOn: $store.notifs.marketing) }
        }
    }
}

// MARK: - Input
struct InputPage: View {
    @State private var autoSend = true
    @State private var smartSugg = true
    @State private var fromHistory = true
    var body: some View {
        VStack(spacing: 0) {
            SettingsSection(label: "语音输入") {
                SettingsRow(label: "按住说话识别语言", value: "自动", onTap: {})
                SettingsRow(label: "松开后自动发送", chevron: false, isLast: true) { LXToggle(isOn: $autoSend) }
            }
            SettingsSection(label: "候选词") {
                SettingsRow(label: "启用智能候选", chevron: false) { LXToggle(isOn: $smartSugg) }
                SettingsRow(label: "基于历史会话", chevron: false, isLast: true) { LXToggle(isOn: $fromHistory) }
            }
        }
    }
}

// MARK: - Privacy
struct PrivacyPage: View {
    @State private var contribute = false
    @State private var crash = true
    var body: some View {
        VStack(spacing: 0) {
            SettingsSection(label: "数据") {
                SettingsRow(icon: .brain, label: "导出我的所有数据", onTap: {})
                SettingsRow(icon: .x, label: "删除账号与数据", danger: true, isLast: true, onTap: {})
            }
            SettingsSection(label: "可见性", footer: "灵犀对你的承诺：密钥永远不离开本机；对话默认不被用于训练。") {
                SettingsRow(label: "使用数据贡献训练", chevron: false) { LXToggle(isOn: $contribute) }
                SettingsRow(label: "崩溃报告", chevron: false, isLast: true) { LXToggle(isOn: $crash) }
            }
        }
    }
}

// MARK: - Language
struct LanguagePage: View {
    @ObservedObject var store: SettingsStore
    @State private var follow = true
    var body: some View {
        VStack(spacing: 0) {
            SettingsSection(label: "语言", footer: "切换语言后将重新加载界面。AI 对话语言独立配置。") {
                RadioList(options: [
                    .init(value: "zh-CN", label: "简体中文"),
                    .init(value: "zh-TW", label: "繁體中文"),
                    .init(value: "en-US", label: "English (US)"),
                    .init(value: "ja-JP", label: "日本語"),
                ], value: $store.language)
            }
            SettingsSection(label: "区域") {
                SettingsRow(label: "日期格式", value: "2026/5/14", isLast: true, onTap: {})
            }
            SettingsSection(label: "AI 回复语言") {
                SettingsRow(label: "跟随界面", sub: "灵犀根据你的输入语言自动判断", chevron: false, isLast: true) { LXToggle(isOn: $follow) }
            }
        }
    }
}

// MARK: - Voice TTS
struct VoicePage: View {
    @Environment(\.theme) private var t
    @ObservedObject var store: SettingsStore
    @State private var apiKey = ""
    var body: some View {
        let preset = Presets.voice.first(where: { $0.id == store.voice.preset })
        VStack(spacing: 0) {
            SettingsSection(label: "语音合成 TTS") {
                RadioList(options: Presets.voice.map { .init(value: $0.id, label: $0.name, sub: $0.sub) },
                          value: $store.voice.preset)
            }
            if let preset, preset.id != "system" {
                SettingsSection(label: "API 配置", footer: "密钥仅本地存储。") {
                    VStack { SettingsField(text: $apiKey, placeholder: "API Key (\(preset.name))") }
                        .padding(.horizontal, 14).padding(.vertical, 12)
                }
            }
            SettingsSection(label: "选项", footer: "自动播放：AI 回复完成后立即朗读。") {
                SettingsRow(label: "语速", value: String(format: "%.1fx", store.voice.speed), chevron: false) {
                    Slider(value: $store.voice.speed, in: 0.5...2, step: 0.1).frame(width: 110).tint(t.accent)
                }
                SettingsRow(label: "自动播放回复", chevron: false, isLast: true) { LXToggle(isOn: $store.voice.autoPlay) }
            }
        }
    }
}

// MARK: - shared helper
@ViewBuilder
func blurb(_ text: String) -> some View {
    BlurbText(text: text)
}
private struct BlurbText: View {
    @Environment(\.theme) private var t
    let text: String
    var body: some View {
        Text(text).font(.system(size: 11.5)).foregroundColor(t.text3).lineSpacing(4)
            .frame(maxWidth: .infinity, alignment: .leading).padding(.bottom, 14)
    }
}
