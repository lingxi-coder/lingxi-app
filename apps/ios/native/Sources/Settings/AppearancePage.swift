import SwiftUI

struct AppearancePage: View {
    @Environment(AppState.self) private var app
    @Environment(\.theme) private var t
    @Environment(\.dynamicTypeSize) private var typeSize

    var body: some View {
        @Bindable var app = app
        VStack(spacing: 0) {
            SettingsSection(label: String(localized: "settings_section_theme")) {
                RadioList(options: [
                    .init(value: "system", label: String(localized: "settings_appearance_system"),
                          sub: String(localized: "settings_appearance_system_sub")),
                    .init(value: "light", label: String(localized: "settings_appearance_light"),
                          sub: String(localized: "settings_appearance_light_sub")),
                    .init(value: "dark",  label: String(localized: "settings_appearance_dark"),
                          sub: String(localized: "settings_appearance_dark_sub")),
                ], value: Binding(get: { app.themeRaw }, set: { app.setTheme($0) }))
            }

            SettingsSection(label: String(localized: "settings_section_accent_color")) {
                accentGrid(columns: typeSize.isAccessibilitySize ? 3 : 6)
                .padding(16)
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
                        Text("A").font(.caption).foregroundColor(t.text4)
                        Slider(value: $app.fontSize, in: 13...19, step: 1).tint(t.accent)
                        Text("A").font(.body).foregroundColor(t.text4)
                    }
                    Text("settings_font_size_preview")
                        .font(.scaledSystem(app.fontSize, relativeTo: .body)).foregroundColor(t.text).lineSpacing(app.fontSize * 0.5)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .padding(.horizontal, 12).padding(.vertical, 10)
                        .background(t.windowBg).clipShape(RoundedRectangle(cornerRadius: 9))
                }
                .padding(.horizontal, 18).padding(.vertical, 16)
            }
        }
    }

    private func accentGrid(columns: Int) -> some View {
        LazyVGrid(columns: Array(repeating: GridItem(.flexible(minimum: 44)), count: columns), spacing: 16) {
            ForEach(Accents.all) { accent in
                Button {
                    app.accentId = accent.id
                } label: {
                    VStack(spacing: 8) {
                        Circle().fill(accent.color)
                            .frame(width: 34, height: 34)
                            .overlay {
                                if app.accentId == accent.id {
                                    Image(systemName: "checkmark")
                                        .font(.body.bold())
                                        .foregroundStyle(.white)
                                        .shadow(color: .black.opacity(0.5), radius: 1)
                                }
                            }
                        Text(accent.name).font(.caption).foregroundStyle(t.text2)
                            .fixedSize(horizontal: false, vertical: true)
                    }
                    .frame(minWidth: 44, minHeight: 60)
                    .contentShape(.rect)
                }
                .buttonStyle(.plain)
                .accessibilityLabel(accent.name)
                .accessibilityAddTraits(app.accentId == accent.id ? [.isSelected] : [])
            }
        }
    }

}
