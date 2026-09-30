import SwiftUI

struct AppearancePage: View {
    @Environment(AppState.self) private var app
    @Environment(\.theme) private var t

    var body: some View {
        @Bindable var app = app
        VStack(spacing: 0) {
            SettingsSection(label: String(localized: "settings_section_theme")) {
                RadioList(options: [
                    .init(value: "light", label: String(localized: "settings_appearance_light"),
                          sub: String(localized: "settings_appearance_light_sub")),
                    .init(value: "dark",  label: String(localized: "settings_appearance_dark"),
                          sub: String(localized: "settings_appearance_dark_sub")),
                ], value: Binding(get: { app.themeRaw }, set: { app.setTheme($0) }))
            }

            SettingsSection(label: String(localized: "settings_section_accent_color")) {
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

            SettingsSection(label: String(localized: "settings_section_density")) {
                RadioList(options: [
                    .init(value: "compact",     label: String(localized: "settings_density_compact"),
                          sub: String(localized: "settings_density_compact_sub")),
                    .init(value: "comfortable", label: String(localized: "settings_density_comfortable"),
                          sub: String(localized: "settings_density_comfortable_sub")),
                    .init(value: "spacious",    label: String(localized: "settings_density_spacious"),
                          sub: String(localized: "settings_density_spacious_sub")),
                ], value: $app.density)
            }

            SettingsSection(label: String(localized: "settings_section_font_size"),
                            footer: String(localized: "settings_font_size_footer \(Int(app.fontSize))")) {
                VStack(spacing: 8) {
                    HStack(spacing: 8) {
                        Text("A").font(.system(size: 11)).foregroundColor(t.text4)
                        Slider(value: $app.fontSize, in: 13...19, step: 1).tint(t.accent)
                        Text("A").font(.system(size: 17)).foregroundColor(t.text4)
                    }
                    Text("settings_font_size_preview")
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
