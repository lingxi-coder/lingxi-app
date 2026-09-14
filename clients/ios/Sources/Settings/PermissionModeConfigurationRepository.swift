import Foundation

/// Device-local preference for the bypass risk warning.
///
/// The engine persists the last successful explicit permission-mode selection
/// and restores it during startup and session changes. Keep that preference in
/// the shared engine's preference store instead of duplicating it in UserDefaults.
/// A controls snapshot (including temporary plan mode) is not a user selection.
@MainActor
final class PermissionModeConfigurationRepository {
    static let shared = PermissionModeConfigurationRepository()

    private static let bypassWarningSuppressedKey = "permission-mode.bypass-warning-suppressed"

    private let defaults: UserDefaults

    init(defaults: UserDefaults = .standard) {
        self.defaults = defaults
    }

    func bypassWarningSuppressed() -> Bool {
        defaults.bool(forKey: Self.bypassWarningSuppressedKey)
    }

    func setBypassWarningSuppressed(_ suppressed: Bool) {
        defaults.set(suppressed, forKey: Self.bypassWarningSuppressedKey)
    }

}
