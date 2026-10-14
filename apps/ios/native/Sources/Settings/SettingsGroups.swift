import Foundation

/// The six groups of the phone settings home: account card, six groups and a
/// search field instead of the desktop's sixteen-row list. A group page lists
/// the existing settings pages unchanged.
enum SettingsGroup: String, CaseIterable, Hashable, Sendable, Identifiable {
    case models
    case modes
    case phone
    case general
    case data
    case about

    var id: String { rawValue }

    // `stringLiteral:` matters: an interpolated literal would look up the
    // key "settings_group_%@" and show that raw key on screen.
    var title: String { String(localized: String.LocalizationValue(stringLiteral: "settings_group_\(rawValue)")) }
    var summary: String { String(localized: String.LocalizationValue(stringLiteral: "settings_group_\(rawValue)_summary")) }

    var icon: LXIconName {
        switch self {
        case .models: .sparkle
        case .modes: .workflow
        case .phone: .terminal
        case .general: .cog
        case .data: .folder
        case .about: .book
        }
    }
}
