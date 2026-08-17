import Foundation

/// Device-local preference for the bypass risk warning.
///
/// The selected permission mode belongs to the active session transcript and
/// is restored by the engine when that session is resumed. It is intentionally
/// not stored in the app-wide settings file.
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
