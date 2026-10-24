import Foundation
import Observation

/// Native device grants are independent of manifest declarations, Linux
/// execution consent and OS-wide permissions. A new release starts ungranted.
@MainActor
@Observable
final class LocalAppDevicePermissions {
    #if LINGXI_FULL
    static let supported: Set<String> = ["clipboard", "files", "photos", "share", "camera", "microphone", "notifications"]
    #else
    // This channel has no approval to expose native APIs to downloaded mini apps.
    static let supported: Set<String> = []
    #endif
    private var grants: [String: [String]]
    @ObservationIgnored private let defaults: UserDefaults
    private let storageKey = "local-app-device-grants-v2"
    init(defaults: UserDefaults = .standard) {
        self.defaults = defaults
        grants = defaults.dictionary(forKey: storageKey) as? [String: [String]] ?? [:]
    }
    func allows(_ capability: String, appID: String, digest: String) -> Bool {
        grants[appID + ":" + digest]?.contains(capability) == true
    }
    func set(_ capability: String, allowed: Bool, appID: String, digest: String) {
        guard Self.supported.contains(capability) else { return }
        let key = appID + ":" + digest
        var values = Set(grants[key] ?? [])
        if allowed { values.insert(capability) } else { values.remove(capability) }
        grants[key] = values.sorted(); defaults.set(grants, forKey: storageKey)
    }
    func revoke(appID: String) {
        grants = grants.filter { !$0.key.hasPrefix(appID + ":") }
        defaults.set(grants, forKey: storageKey)
    }
}
