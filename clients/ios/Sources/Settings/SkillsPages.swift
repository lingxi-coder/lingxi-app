import SwiftUI

// MARK: - Skills (grouped by author)
struct SkillsPage: View {
    @Environment(\.theme) private var t
    @Bindable var store: SettingsStore
    let host: SettingsHost

    private let authorOrder = ["官方", "我", "社区 · @arxiv-fan", "社区 · @lin"]

    var body: some View {
        VStack(spacing: 0) {
            Text("Skills 是可复用的 AI 行为包，封装了系统提示词、工具调用与触发条件。启用后会出现在对应触发器或斜杠菜单。")
                .font(.system(size: 11.5)).foregroundColor(t.text3).lineSpacing(4)
                .frame(maxWidth: .infinity, alignment: .leading).padding(.bottom, 14)

            ForEach(authorOrder, id: \.self) { author in
                let arr = store.skills.filter { $0.author == author }
                if !arr.isEmpty {
                    SettingsSection(label: author) {
                        ForEach(Array(arr.enumerated()), id: \.element.id) { i, s in
                            SettingsRow(icon: .skill,
                                        iconColor: s.builtin ? Color(srgb: 0,0.7601,0.7664) : Color(srgb: 0.896,0.6013,0),
                                        label: s.name,
                                        sub: s.desc + " · " + s.triggers.joined(separator: " / "),
                                        chevron: false, isLast: i == arr.count - 1,
                                        onTap: { host.push(.skillDetail(s.id)) }) {
                                LXToggle(isOn: toggle(s.id))
                            }
                        }
                    }
                }
            }

            DashedAddButton(title: "浏览 Skills 商店").padding(.bottom, 8)
            Button {} label: {
                HStack(spacing: 8) {
                    LXIcon(name: .edit, size: 14, color: t.text2, stroke: 1.8)
                    Text("创建自定义 Skill").font(.system(size: 13.5, weight: .medium)).foregroundColor(t.text2)
                }
                .frame(maxWidth: .infinity).padding(13)
                .background(t.surface).clipShape(RoundedRectangle(cornerRadius: 12))
                .overlay(RoundedRectangle(cornerRadius: 12).stroke(t.border, lineWidth: 0.5))
            }
        }
    }

    private func toggle(_ id: String) -> Binding<Bool> {
        Binding(get: { store.skills.first(where: { $0.id == id })?.enabled ?? false },
                set: { v in if let i = store.skills.firstIndex(where: { $0.id == id }) { store.skills[i].enabled = v } })
    }
}

// MARK: - Skill detail
struct SkillDetailPage: View {
    @Environment(\.theme) private var t
    @Bindable var store: SettingsStore
    let host: SettingsHost
    let skillId: String

    private let sky = Color(srgb: 0, 0.7601, 0.7664) // oklch(72% 0.16 195)

    var body: some View {
        guard let s = store.skills.first(where: { $0.id == skillId }) else {
            DispatchQueue.main.async { host.pop() }
            return AnyView(EmptyView())
        }
        return AnyView(VStack(spacing: 0) {
            VStack(spacing: 0) {
                LXIcon(name: .skill, size: 28, color: sky, stroke: 1.7)
                    .frame(width: 64, height: 64)
                    .background(sky.mix(with: t.surface, amount: 0.20))
                    .clipShape(RoundedRectangle(cornerRadius: 16))
                    .overlay(RoundedRectangle(cornerRadius: 16).stroke(sky.tint(0.35), lineWidth: 0.5))
                    .padding(.bottom, 12)
                Text(s.name).font(.system(size: 18, weight: .bold)).foregroundColor(t.text)
                Text("\(s.author) · v1.2.0").font(.system(size: 12)).foregroundColor(t.text4).padding(.top, 4)
            }
            .padding(.top, 8).padding(.bottom, 18)

            Text(s.desc).font(.system(size: 13)).foregroundColor(t.text2).lineSpacing(5)
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(16).background(t.surface).clipShape(RoundedRectangle(cornerRadius: 12))
                .overlay(RoundedRectangle(cornerRadius: 12).stroke(t.border, lineWidth: 0.5))
                .padding(.bottom, 14)

            SettingsSection(label: "触发") {
                ForEach(Array(s.triggers.enumerated()), id: \.offset) { i, tr in
                    SettingsRow(label: tr, chevron: false, isLast: i == s.triggers.count - 1) {
                        LXIcon(name: .check, size: 15, color: t.ok, stroke: 2.4)
                    }
                }
            }
            SettingsSection(label: "所需权限") {
                SettingsRow(label: "读取知识库", chevron: false)
                SettingsRow(label: "调用 LLM", chevron: false)
                SettingsRow(label: "访问 MCP · GitHub", chevron: false, isLast: true)
            }
            SettingsSection {
                SettingsRow(label: "启用此 Skill", chevron: false) {
                    LXToggle(isOn: Binding(
                        get: { store.skills.first(where: { $0.id == skillId })?.enabled ?? false },
                        set: { v in if let i = store.skills.firstIndex(where: { $0.id == skillId }) { store.skills[i].enabled = v } }))
                }
                SettingsRow(label: "自动建议", sub: "检测到匹配场景时主动提示", chevron: false) {
                    LXToggle(isOn: .constant(true))
                }
                SettingsRow(label: "编辑提示词", onTap: {})
                SettingsRow(icon: .x, label: s.builtin ? "此 Skill 由官方维护" : "删除此 Skill",
                            chevron: false, danger: !s.builtin, isLast: true)
            }
        })
    }
}
