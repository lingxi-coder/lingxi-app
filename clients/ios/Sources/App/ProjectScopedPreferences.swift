import Foundation

/// Persists user-editable conversation state under an explicit project scope.
/// The global scope is a real scope of its own; it never shares keys with a
/// managed project, so switching workspaces cannot leak drafts or sessions.
struct ProjectScopedPreferences {
    private let defaults: UserDefaults

    init(defaults: UserDefaults = .standard) {
        self.defaults = defaults
    }

    func draft(projectID: String?) -> String {
        defaults.string(forKey: key("draft", projectID: projectID)) ?? ""
    }

    func setDraft(_ value: String, projectID: String?) {
        defaults.set(value, forKey: key("draft", projectID: projectID))
    }

    func activeSessionID(projectID: String?) -> String {
        storedActiveSessionID(projectID: projectID) ?? ""
    }

    /// Unlike `activeSessionID`, preserves the distinction between a missing
    /// legacy key and an explicitly saved empty id (the user chose New Chat).
    func storedActiveSessionID(projectID: String?) -> String? {
        defaults.string(forKey: key("active-session", projectID: projectID))
    }

    func setActiveSessionID(_ value: String, projectID: String?) {
        defaults.set(value, forKey: key("active-session", projectID: projectID))
    }

    private func key(_ field: String, projectID: String?) -> String {
        let scope = projectID?.lowercased() ?? "global"
        return "conversation.\(scope).\(field)"
    }
}
