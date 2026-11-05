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
