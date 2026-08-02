import SwiftUI
import Observation

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

/// Global UI state persisted in `UserDefaults` and shared through the iOS 17
/// Observation environment. Keeping persistence here makes every mutation
/// synchronous and testable without coupling the model to a SwiftUI view.
@Observable
@MainActor
final class AppState {
    @ObservationIgnored private let defaults: UserDefaults

    var themeRaw: String { didSet { defaults.set(themeRaw, forKey: "theme") } }
    /// The selected accent oklch id (matches an `Accents.all` entry).
    var accentId: String { didSet { defaults.set(accentId, forKey: "accent") } }
    var density: String { didSet { defaults.set(density, forKey: "density") } }
    var fontSize: Double { didSet { defaults.set(fontSize, forKey: "fontSize") } }

    // MARK: First-run profile + onboarding (the prototype's `lx_settings` blob +
    // `lx_setup_done` flag). Persisted so the SetupWizard runs once and the
    // VoiceOrb / drawer / settings can read the chosen names.
    /// The assistant's wake-word name (prototype `assistantName`, default 灵犀).
    var assistantName: String { didSet { defaults.set(assistantName, forKey: "assistantName") } }
    /// How the assistant addresses the user (prototype `userName`).
    var userName: String { didSet { defaults.set(userName, forKey: "userName") } }
    /// Open FlowMode by default when entering voice (prototype `flowDefault`).
    var flowDefault: Bool { didSet { defaults.set(flowDefault, forKey: "flowDefault") } }
    /// Show the pop-up text input inside FlowMode (prototype `inputDialog`).
    var inputDialog: Bool { didSet { defaults.set(inputDialog, forKey: "inputDialog") } }
    /// Whether first-run setup is complete (prototype `lx_setup_done`).
    var setupDone: Bool { didSet { defaults.set(setupDone, forKey: "setupDone") } }

    /// The onboarding choice: prefer recognizer-enforced on-device STT when the
    /// current locale supports it, otherwise allow the system network fallback.
    var voiceRecognitionMode: String {
        didSet { defaults.set(voiceRecognitionMode, forKey: "voiceRecognitionMode") }
    }
    var voiceLanguage: String { didSet { defaults.set(voiceLanguage, forKey: "voiceLanguage") } }

    init(defaults: UserDefaults = .standard) {
        self.defaults = defaults
        themeRaw = defaults.string(forKey: "theme") ?? "dark"
        accentId = defaults.string(forKey: "accent") ?? "oklch(70% 0.18 268)"
        density = defaults.string(forKey: "density") ?? "comfortable"
        fontSize = defaults.object(forKey: "fontSize") == nil ? 15 : defaults.double(forKey: "fontSize")
        assistantName = defaults.string(forKey: "assistantName") ?? "灵犀"
        userName = defaults.string(forKey: "userName") ?? ""
        flowDefault = defaults.object(forKey: "flowDefault") == nil ? true : defaults.bool(forKey: "flowDefault")
        inputDialog = defaults.object(forKey: "inputDialog") == nil ? true : defaults.bool(forKey: "inputDialog")
        #if DEBUG
            // UI tests use real app navigation with a deterministic local
            // source. A separate override keeps the onboarding layout directly
            // testable without clearing the simulator's persisted preferences.
            let environment = ProcessInfo.processInfo.environment
            if environment["LINGXI_FORCE_ONBOARDING"] == "1" {
                setupDone = false
            } else {
                setupDone = environment["LINGXI_UI_TESTING"] == "1"
                    || defaults.bool(forKey: "setupDone")
            }
        #else
            setupDone = defaults.bool(forKey: "setupDone")
        #endif
        voiceRecognitionMode = defaults.string(forKey: "voiceRecognitionMode") ?? "on-device"
        voiceLanguage = defaults.string(forKey: "voiceLanguage")
            ?? VoiceCapabilityModel.automaticLanguageIdentifier
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
