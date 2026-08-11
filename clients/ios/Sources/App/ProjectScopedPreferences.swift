import Foundation

/// Persists user-editable conversation state under an explicit conversation
/// scope. The global scope is a real scope of its own; it never shares keys
/// with a managed project or a local app, so switching workspaces cannot leak
/// drafts or sessions.
struct ProjectScopedPreferences {
    private let defaults: UserDefaults

    init(defaults: UserDefaults = .standard) {
        self.defaults = defaults
    }

    // MARK: scope-keyed accessors

    func draft(scope: ConversationScope) -> String {
        defaults.string(forKey: key("draft", scope: scope)) ?? ""
    }

    func setDraft(_ value: String, scope: ConversationScope) {
        defaults.set(value, forKey: key("draft", scope: scope))
    }

    func activeSessionID(scope: ConversationScope) -> String {
        storedActiveSessionID(scope: scope) ?? ""
    }

    /// Unlike `activeSessionID`, preserves the distinction between a missing
    /// legacy key and an explicitly saved empty id (the user chose New Chat).
    func storedActiveSessionID(scope: ConversationScope) -> String? {
        defaults.string(forKey: key("active-session", scope: scope))
    }

    func setActiveSessionID(_ value: String, scope: ConversationScope) {
        defaults.set(value, forKey: key("active-session", scope: scope))
    }

    // MARK: legacy projectID spellings (nil == global)

    func draft(projectID: String?) -> String {
        draft(scope: ConversationScope(projectID: projectID))
    }

    func setDraft(_ value: String, projectID: String?) {
        setDraft(value, scope: ConversationScope(projectID: projectID))
    }

    func activeSessionID(projectID: String?) -> String {
        activeSessionID(scope: ConversationScope(projectID: projectID))
    }

    func storedActiveSessionID(projectID: String?) -> String? {
        storedActiveSessionID(scope: ConversationScope(projectID: projectID))
    }

    func setActiveSessionID(_ value: String, projectID: String?) {
        setActiveSessionID(value, scope: ConversationScope(projectID: projectID))
    }

    /// `ConversationScope.preferenceScope` keeps the historical key strings
    /// for global/project scopes byte-identical (existing user state must
    /// survive) and only adds the `app.<id>` namespace for local apps.
    private func key(_ field: String, scope: ConversationScope) -> String {
        "conversation.\(scope.preferenceScope).\(field)"
    }
}
