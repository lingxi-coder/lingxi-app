import SwiftUI

struct DreamPage: View {
    @Environment(\.theme) private var t
    @ObservedObject var store: SettingsStore

    @State private var orbPulse = false
    private let rose = Color(srgb: 0.809, 0.4552, 0.8891) // oklch(70% 0.18 320)

    private struct Activity { let key: String; let label: String; let sub: String }
    private let acts: [Activity] = [
        .init(key: "reorganize", label: "整理记忆",     sub: "合并相似 / 去重 / 归档过期"),
        .init(key: "plan",       label: "草拟今日计划", sub: "基于昨日未完成 + 日历"),
        .init(key: "recap",      label: "总结过去一周", sub: "每周日凌晨生成回顾报告"),
        .init(key: "prefetch",   label: "预热常用上下文", sub: "预生成你最常问的回答"),
        .init(key: "polish",     label: "润色草稿",     sub: "为待发邮件/文档生成 2 套候选"),
    ]

    var body: some View {
        VStack(spacing: 0) {
            // Gradient orb header
            VStack(spacing: 0) {
                ZStack {
                    Circle()
                        .fill(RadialGradient(colors: [rose, Color(srgb: 0.1289,0.214,0.6526)],
                                             center: .init(x: 0.3, y: 0.3), startRadius: 2, endRadius: 70))
                        .frame(width: 76, height: 76)
                        .opacity(orbPulse ? 0.8 : 0.45)
                    Circle().fill(t.windowBg).frame(width: 64, height: 64)
                        .overlay(LXIcon(name: .dream, size: 32, color: rose, stroke: 1.6))
                }
                .padding(.bottom, 14)
                Text("Dream 模式").font(.system(size: 19, weight: .bold)).foregroundColor(t.text)
                Text("当设备闲置时，灵犀在后台运行\n整理、规划、润色 — 醒来即可看到结果")
                    .font(.system(size: 12.5)).foregroundColor(t.text3)
                    .multilineTextAlignment(.center).lineSpacing(5).padding(.top, 6).padding(.horizontal, 16)
            }
            .padding(.top, 12).padding(.bottom, 22)
            .onAppear { withAnimation(.easeInOut(duration: 3).repeatForever()) { orbPulse = true } }

            SettingsSection {
                SettingsRow(label: "开启 Dream 模式", sub: store.dream.lastRun, chevron: false, isLast: true) {
                    LXToggle(isOn: $store.dream.enabled)
                }
            }

            SettingsSection(label: "时间窗") {
                RadioList(options: [
                    .init(value: "night",  label: "夜间 (00:00–06:00)", sub: "默认，最不打扰"),
                    .init(value: "always", label: "随时",                sub: "只要设备闲置"),
                    .init(value: "custom", label: "自定义时段",          sub: "设置每日时间窗"),
                ], value: $store.dream.window)
            }

            SettingsSection(label: "运行条件", footer: "确保不会影响日常使用：仅在充电 + Wi-Fi 时跑昂贵任务。") {
                SettingsRow(label: "仅在充电时", chevron: false) { LXToggle(isOn: $store.dream.onCharging) }
                SettingsRow(label: "仅在 Wi-Fi 时", chevron: false, isLast: true) { LXToggle(isOn: $store.dream.onWifi) }
            }

            SettingsSection(label: "允许的活动", footer: "可叠加，越多越费电与算力。") {
                ForEach(Array(acts.enumerated()), id: \.element.key) { i, a in
                    SettingsRow(label: a.label, sub: a.sub, chevron: false, isLast: i == acts.count - 1) {
                        LXToggle(isOn: Binding(
                            get: { store.dream.activities[a.key] ?? false },
                            set: { store.dream.activities[a.key] = $0 }))
                    }
                }
            }

            SettingsSection(label: "算力预算",
                            footer: "高预算会优先调用更强模型（如 Claude Opus），并访问 MCP 工具。低预算只用本地 + 最便宜模型。") {
                RadioList(options: [
                    .init(value: "low",    label: "低", sub: "本地模型为主 · 几乎免费"),
                    .init(value: "medium", label: "中", sub: "中等模型 · 默认"),
                    .init(value: "high",   label: "高", sub: "推理模型 + 工具 · 最深入"),
                ], value: $store.dream.budget)
            }

            SettingsSection(label: "昨夜 Dream 回顾") {
                SettingsRow(label: "03:24 - 03:41 · 整理记忆", sub: "合并 12 条 → 7 条，归档 5 条过期", onTap: {})
                SettingsRow(label: "03:41 - 04:02 · 草拟今日计划", sub: "基于 8 个未完成事项 + 3 场会议", onTap: {})
                SettingsRow(label: "04:02 - 04:08 · 润色邮件", sub: "为 2 封草稿各生成 1 套候选", isLast: true, onTap: {})
            }
        }
    }
}
