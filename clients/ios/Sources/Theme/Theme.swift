import SwiftUI

// MARK: - Theme environment
//
// Mirrors the prototype's `Theme` React context + `useT()` hook.
// `AppState` owns the user-facing toggles (theme, accent) and exposes a
// resolved `Palette`. We inject the palette through the SwiftUI environment so
// any view can read `@Environment(\.theme) var t` — the Swift analog of useT().

private struct ThemeKey: EnvironmentKey {
    static let defaultValue: Palette = DesignTokens.dark
}

extension EnvironmentValues {
    var theme: Palette {
        get { self[ThemeKey.self] }
        set { self[ThemeKey.self] = newValue }
    }
}

/// Global UI state — theme mode + accent override, persisted via @AppStorage.
final class AppState: ObservableObject {
    @AppStorage("theme") var themeRaw: String = "dark" {
        willSet { objectWillChange.send() }
    }
    /// The selected accent oklch id (matches an `Accents.all` entry).
    @AppStorage("accent") var accentId: String = "oklch(70% 0.18 268)" {
        willSet { objectWillChange.send() }
    }
    @AppStorage("density") var density: String = "comfortable" {
        willSet { objectWillChange.send() }
    }
    @AppStorage("fontSize") var fontSize: Double = 15 {
        willSet { objectWillChange.send() }
    }

    // MARK: First-run profile + onboarding (the prototype's `lx_settings` blob +
    // `lx_setup_done` flag). Persisted so the SetupWizard runs once and the
    // VoiceOrb / drawer / settings can read the chosen names.
    /// The assistant's wake-word name (prototype `assistantName`, default 灵犀).
    @AppStorage("assistantName") var assistantName: String = "灵犀" {
        willSet { objectWillChange.send() }
    }
    /// How the assistant addresses the user (prototype `userName`).
    @AppStorage("userName") var userName: String = "" {
        willSet { objectWillChange.send() }
    }
    /// Whether a voiceprint was enrolled in onboarding (prototype `voiceprint`).
    @AppStorage("voiceprint") var voiceprint: Bool = false {
        willSet { objectWillChange.send() }
    }
    /// The default model id picked in onboarding (prototype `modelId`, mock id).
    @AppStorage("defaultModelId") var defaultModelId: String = "lx-72b" {
        willSet { objectWillChange.send() }
    }
    /// Open FlowMode by default when entering voice (prototype `flowDefault`).
    @AppStorage("flowDefault") var flowDefault: Bool = true {
        willSet { objectWillChange.send() }
    }
    /// Show the pop-up text input inside FlowMode (prototype `inputDialog`).
    @AppStorage("inputDialog") var inputDialog: Bool = true {
        willSet { objectWillChange.send() }
    }
    /// Whether first-run setup is complete (prototype `lx_setup_done`).
    @AppStorage("setupDone") var setupDone: Bool = false {
        willSet { objectWillChange.send() }
    }

    var isDark: Bool { themeRaw == "dark" }

    func setTheme(_ value: String) { themeRaw = value }
    func toggleTheme() { themeRaw = isDark ? "light" : "dark" }

    /// Resolved palette with the user's accent override applied.
    var palette: Palette {
        var p = DesignTokens.palette(dark: isDark)
        p.accent = Accents.color(for: accentId)
        return p
    }

    var colorScheme: ColorScheme { isDark ? .dark : .light }
}
