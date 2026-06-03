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
