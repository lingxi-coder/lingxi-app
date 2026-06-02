import SwiftUI

struct AppearancePage: View {
    @EnvironmentObject private var app: AppState
    @Environment(\.theme) private var t

    var body: some View {
        VStack(spacing: 0) {
            SettingsSection(label: "主题") {
                RadioList(options: [
                    .init(value: "light", label: "浅色", sub: "暖白纸感 + 半透卡片"),
                    .init(value: "dark",  label: "深色", sub: "午夜紫 + 低饱和"),
                ], value: Binding(get: { app.themeRaw }, set: { app.setTheme($0) }))
            }

            SettingsSection(label: "强调色") {
                LazyVGrid(columns: Array(repeating: GridItem(.flexible(), spacing: 10), count: 6), spacing: 10) {
                    ForEach(Accents.all) { a in
                        VStack(spacing: 5) {
                            Circle().fill(a.color).frame(width: 34, height: 34)
                                .overlay(Circle().stroke(t.windowBg, lineWidth: app.accentId == a.id ? 2.5 : 0))
                                .overlay(Circle().stroke(a.color, lineWidth: app.accentId == a.id ? 2 : 0).scaleEffect(1.18))
                            Text(a.name).font(.system(size: 11)).foregroundColor(t.text3).lineLimit(1)
                        }
                        .onTapGesture { app.accentId = a.id }
                    }
                }
                .padding(14)
            }

            SettingsSection(label: "密度") {
                RadioList(options: [
                    .init(value: "compact",     label: "紧凑", sub: "一屏显示更多内容"),
                    .init(value: "comfortable", label: "舒适", sub: "默认，平衡"),
                    .init(value: "spacious",    label: "宽松", sub: "更大间距，更易读"),
                ], value: $app.density)
            }

            SettingsSection(label: "字号", footer: "当前 \(Int(app.fontSize))pt · 影响对话与列表正文") {
                VStack(spacing: 8) {
                    HStack(spacing: 8) {
                        Text("A").font(.system(size: 11)).foregroundColor(t.text4)
                        Slider(value: $app.fontSize, in: 13...19, step: 1).tint(t.accent)
                        Text("A").font(.system(size: 17)).foregroundColor(t.text4)
                    }
                    Text("\"灵犀，帮我整理今天的会议要点，重点标出有 action item 的部分。\"")
                        .font(.system(size: app.fontSize)).foregroundColor(t.text).lineSpacing(app.fontSize * 0.5)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .padding(.horizontal, 12).padding(.vertical, 10)
                        .background(t.windowBg).clipShape(RoundedRectangle(cornerRadius: 9))
                }
                .padding(.horizontal, 18).padding(.vertical, 16)
            }
        }
    }
}
