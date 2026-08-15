import Foundation

/// User-tier permission-mode persistence shared with the Rust engine.
/// Unknown settings keys are retained and writes are atomic.
@MainActor
final class PermissionModeConfigurationRepository {
    static let shared = PermissionModeConfigurationRepository(
        appSandboxRoot: URL(fileURLWithPath: ConversationSourceFactory.appSandboxRoot(), isDirectory: true)
    )

    private let settingsURL: URL
    private let allowed: Set<String> = ["default", "acceptEdits", "plan", "auto", "dontAsk", "bypassPermissions"]

    init(appSandboxRoot: URL) {
        settingsURL = appSandboxRoot
            .appendingPathComponent(".lingxi", isDirectory: true)
            .appendingPathComponent("settings.json", isDirectory: false)
    }

    func load() -> String {
        guard let root = try? readRoot(),
              let permissions = root["permissions"] as? [String: Any],
              let mode = permissions["defaultMode"] as? String,
              allowed.contains(mode) else { return "auto" }
        return mode
    }

    func save(_ mode: String) throws {
        guard allowed.contains(mode) else {
            throw NSError(domain: "PermissionModeConfiguration", code: 1, userInfo: [NSLocalizedDescriptionKey: "Unknown permission mode"])
        }
        var root = try readRoot() ?? [:]
        var permissions = root["permissions"] as? [String: Any] ?? [:]
        permissions["defaultMode"] = mode
        root["permissions"] = permissions
        let data = try JSONSerialization.data(withJSONObject: root, options: [.prettyPrinted])
        try FileManager.default.createDirectory(at: settingsURL.deletingLastPathComponent(), withIntermediateDirectories: true)
        try data.write(to: settingsURL, options: [.atomic])
    }

    private func readRoot() throws -> [String: Any]? {
        guard FileManager.default.fileExists(atPath: settingsURL.path) else { return nil }
        let data = try Data(contentsOf: settingsURL)
        let object = try JSONSerialization.jsonObject(with: data)
        guard let root = object as? [String: Any] else {
            throw NSError(domain: "PermissionModeConfiguration", code: 2, userInfo: [NSLocalizedDescriptionKey: "settings.json must contain an object"])
        }
        return root
    }
}
