import SwiftUI

struct DreamPage: View {
    @Environment(\.theme) private var t
    @Bindable var store: SettingsStore

    @State private var orbPulse = false
    private let rose = Color(srgb: 0.809, 0.4552, 0.8891) // oklch(70% 0.18 320)

    private struct Activity { let key: String; let label: String; let sub: String }
    private let acts: [Activity] = [
        .init(key: "reorganize", label: String(localized: "dream_activity_reorganize"),
              sub: String(localized: "dream_activity_reorganize_sub")),
        .init(key: "plan",       label: String(localized: "dream_activity_plan"),
              sub: String(localized: "dream_activity_plan_sub")),
        .init(key: "recap",      label: String(localized: "dream_activity_recap"),
              sub: String(localized: "dream_activity_recap_sub")),
        .init(key: "prefetch",   label: String(localized: "dream_activity_prefetch"),
              sub: String(localized: "dream_activity_prefetch_sub")),
        .init(key: "polish",     label: String(localized: "dream_activity_polish"),
              sub: String(localized: "dream_activity_polish_sub")),
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
                Text("settings_dream_mode").font(.system(size: 19, weight: .bold)).foregroundColor(t.text)
                Text("dream_description_blurb")
                    .font(.system(size: 12.5)).foregroundColor(t.text3)
                    .multilineTextAlignment(.center).lineSpacing(5).padding(.top, 6).padding(.horizontal, 16)
            }
            .padding(.top, 12).padding(.bottom, 22)
            .onAppear { withAnimation(.easeInOut(duration: 3).repeatForever()) { orbPulse = true } }

            SettingsSection {
                SettingsRow(label: String(localized: "dream_enable"), sub: store.dream.lastRun, chevron: false, isLast: true) {
                    LXToggle(isOn: $store.dream.enabled)
                }
            }

            SettingsSection(label: String(localized: "dream_section_time_window")) {
                RadioList(options: [
                    .init(value: "night",  label: String(localized: "dream_window_night"),
                          sub: String(localized: "dream_window_night_sub")),
                    .init(value: "always", label: String(localized: "dream_window_always"),
                          sub: String(localized: "dream_window_always_sub")),
                    .init(value: "custom", label: String(localized: "dream_window_custom"),
                          sub: String(localized: "dream_window_custom_sub")),
                ], value: $store.dream.window)
            }

            SettingsSection(label: String(localized: "dream_section_conditions"),
                            footer: String(localized: "dream_conditions_footer")) {
                SettingsRow(label: String(localized: "dream_only_charging"), chevron: false) { LXToggle(isOn: $store.dream.onCharging) }
                SettingsRow(label: String(localized: "dream_only_wifi"), chevron: false, isLast: true) { LXToggle(isOn: $store.dream.onWifi) }
            }

            SettingsSection(label: String(localized: "dream_section_activities"),
                            footer: String(localized: "dream_activities_footer")) {
                ForEach(Array(acts.enumerated()), id: \.element.key) { i, a in
                    SettingsRow(label: a.label, sub: a.sub, chevron: false, isLast: i == acts.count - 1) {
                        LXToggle(isOn: Binding(
                            get: { store.dream.activities[a.key] ?? false },
                            set: { store.dream.activities[a.key] = $0 }))
                    }
                }
            }

            SettingsSection(label: String(localized: "dream_section_compute_budget"),
                            footer: String(localized: "dream_compute_budget_footer")) {
                RadioList(options: [
                    .init(value: "low",    label: String(localized: "dream_budget_low"),
                          sub: String(localized: "dream_budget_low_sub")),
                    .init(value: "medium", label: String(localized: "dream_budget_medium"),
                          sub: String(localized: "dream_budget_medium_sub")),
                    .init(value: "high",   label: String(localized: "dream_budget_high"),
                          sub: String(localized: "dream_budget_high_sub")),
                ], value: $store.dream.budget)
            }

            SettingsSection(label: String(localized: "dream_section_last_night")) {
                SettingsRow(label: String(localized: "dream_recap_item_1"),
                            sub: String(localized: "dream_recap_sub_1"), onTap: {})
                SettingsRow(label: String(localized: "dream_recap_item_2"),
                            sub: String(localized: "dream_recap_sub_2"), onTap: {})
                SettingsRow(label: String(localized: "dream_recap_item_3"),
                            sub: String(localized: "dream_recap_sub_3"), isLast: true, onTap: {})
            }
        }
    }
}
