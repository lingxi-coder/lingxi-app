import Foundation

/// The groups of the phone settings home: account card, the groups and a
/// search field instead of the desktop's sixteen-row list. A group page lists
/// the existing settings pages unchanged. Models and voice are separate
/// entries; voice has a single page, so its home row opens that page directly.
enum SettingsGroup: String, CaseIterable, Hashable, Sendable, Identifiable {
    case models
    case voice
    case modes
    case phone
    case general
    case data
    case about

    var id: String { rawValue }

    // `stringLiteral:` matters: an interpolated literal would look up the
    // key "settings_group_%@" and show that raw key on screen.
    var title: String { String(localized: String.LocalizationValue(stringLiteral: "settings_group_\(stringKey)")) }
    var summary: String { String(localized: String.LocalizationValue(stringLiteral: "settings_group_\(stringKey)_summary")) }

    /// Android still lists voice under "models", so the shared
    /// `settings_group_models` strings keep that meaning; iOS has its own.
    private var stringKey: String { self == .models ? "models_hub" : rawValue }

    /// The page a home row opens when the group is a single page.
    var directPage: SettingsPage? { self == .voice ? .voice : nil }

    var icon: LXIconName {
        switch self {
        case .models: .sparkle
        case .voice: .mic
        case .modes: .workflow
        case .phone: .terminal
        case .general: .cog
        case .data: .folder
        case .about: .book
        }
    }
}
