import Foundation

/// The collapsible sections of the conversation list. The raw values are what is stored per team and what
/// Android stores too (`teams/TeamsWorkspace.kt`).
enum TeamsSection: String, CaseIterable, Sendable {
    case channels, direct, bots
}

/// What the Teams tab remembers on the device, per account (spec §6.1, §6.2): the selected team
/// (`teams.selected.<uid>`) and the collapsed list sections of each team (`teams.collapsed.<uid>.<teamId>`).
/// Foundation only, so it runs in the macOS XCTest harness.
struct TeamsPreferences {
    private let defaults: UserDefaults

    init(defaults: UserDefaults = .standard) {
        self.defaults = defaults
    }

    static func selectedKey(uid: String) -> String { "teams.selected.\(uid)" }
    static func collapsedKey(uid: String, teamID: String) -> String { "teams.collapsed.\(uid).\(teamID)" }

    func selectedTeam(uid: String) -> String? {
        let value = defaults.string(forKey: Self.selectedKey(uid: uid))
        return value?.isEmpty == false ? value : nil
    }

    func setSelectedTeam(_ teamID: String, uid: String) {
        defaults.set(teamID, forKey: Self.selectedKey(uid: uid))
    }

    /// Sections the viewer collapsed in this team. Values that are not sections are ignored.
    func collapsed(uid: String, teamID: String) -> Set<TeamsSection> {
        Set((defaults.stringArray(forKey: Self.collapsedKey(uid: uid, teamID: teamID)) ?? []).compactMap(TeamsSection.init(rawValue:)))
    }

    func setCollapsed(_ sections: Set<TeamsSection>, uid: String, teamID: String) {
        let key = Self.collapsedKey(uid: uid, teamID: teamID)
        if sections.isEmpty {
            defaults.removeObject(forKey: key)
        } else {
            defaults.set(sections.map(\.rawValue).sorted(), forKey: key)
        }
    }
}

/// SceneStorage may checkpoint an earlier navigation value when a process is terminated.
/// Keep an atomic snapshot per UIKit scene, including an empty value when the user goes back.
struct TeamsSceneNavigationStore {
    var directory: URL?

    func read(sceneID: String) throws -> String? {
        let file = try fileURL(sceneID: sceneID)
        guard FileManager.default.fileExists(atPath: file.path) else { return nil }
        return try JSONDecoder().decode(String.self, from: Data(contentsOf: file))
    }

    func write(_ value: String, sceneID: String) throws {
        let file = try fileURL(sceneID: sceneID)
        try FileManager.default.createDirectory(at: file.deletingLastPathComponent(), withIntermediateDirectories: true,
                                                attributes: [.posixPermissions: 0o700])
        try JSONEncoder().encode(value).write(to: file, options: [.atomic, .completeFileProtectionUntilFirstUserAuthentication])
    }

    private func fileURL(sceneID: String) throws -> URL {
        guard !sceneID.isEmpty, sceneID.utf8.count <= 160 else {
            throw CocoaError(.fileWriteInvalidFileName)
        }
        let root = try directory ?? FileManager.default.url(for: .applicationSupportDirectory, in: .userDomainMask,
                                                           appropriateFor: nil, create: true).appendingPathComponent("TeamsNavigation", isDirectory: true)
        let name = Data(sceneID.utf8).base64EncodedString()
            .replacingOccurrences(of: "/", with: "_").replacingOccurrences(of: "+", with: "-").replacingOccurrences(of: "=", with: "")
        return root.appendingPathComponent(name).appendingPathExtension("json")
    }
}
