import Foundation

/// Persists user-editable conversation state under an explicit conversation
/// scope. The global scope is a real scope of its own; it never shares keys
/// with a managed project, so switching workspaces cannot leak
/// drafts or sessions.
struct ProjectScopedPreferences {
    private let defaults: UserDefaults
    private static let workspacePinnedAtKey = "conversation.workspace.pinned-at"
    private static let sessionPinnedAtKey = "conversation.session.pinned-at"
    private static let cronResultsSeenAtKey = "conversation.cron.results-seen-at"

    init(defaults: UserDefaults = .standard) {
        self.defaults = defaults
    }

    /// UI tests use simulated session identifiers and must never persist them
    /// in the user's ordinary conversation preferences.
    static func forRuntime(
        environment: [String: String] = ProcessInfo.processInfo.environment,
        appDefaults: UserDefaults = .standard,
        uiTestSuiteName: String = "\(Bundle.main.bundleIdentifier ?? "com.lingxi.code").ui-testing"
    ) -> ProjectScopedPreferences {
        #if DEBUG
        if environment["LINGXI_UI_TESTING"] == "1" {
            guard let defaults = UserDefaults(suiteName: uiTestSuiteName) else {
                preconditionFailure("Unable to create isolated UI test preferences")
            }
            return ProjectScopedPreferences(defaults: defaults)
        }
        #endif
        return ProjectScopedPreferences(defaults: appDefaults)
    }

    // MARK: scope-keyed accessors

    func draft(scope: ConversationScope) -> String {
        defaults.string(forKey: key("draft", scope: scope)) ?? ""
    }

    func draft(scope: ConversationScope, mode: SessionMode) -> String {
        defaults.string(forKey: key("draft", scope: scope, mode: mode)) ?? ""
    }

    func setDraft(_ value: String, scope: ConversationScope) {
        defaults.set(value, forKey: key("draft", scope: scope))
    }

    func setDraft(_ value: String, scope: ConversationScope, mode: SessionMode) {
        defaults.set(value, forKey: key("draft", scope: scope, mode: mode))
    }

    func activeSessionID(scope: ConversationScope) -> String {
        storedActiveSessionID(scope: scope) ?? ""
    }

    func activeSessionID(scope: ConversationScope, mode: SessionMode) -> String {
        storedActiveSessionID(scope: scope, mode: mode) ?? ""
    }

    /// Unlike `activeSessionID`, preserves the distinction between a missing
    /// legacy key and an explicitly saved empty id (the user chose New Chat).
    func storedActiveSessionID(scope: ConversationScope) -> String? {
        defaults.string(forKey: key("active-session", scope: scope))
    }

    func storedActiveSessionID(scope: ConversationScope, mode: SessionMode) -> String? {
        defaults.string(forKey: key("active-session", scope: scope, mode: mode))
    }

    func setActiveSessionID(_ value: String, scope: ConversationScope) {
        defaults.set(value, forKey: key("active-session", scope: scope))
    }

    func setActiveSessionID(_ value: String, scope: ConversationScope, mode: SessionMode) {
        defaults.set(value, forKey: key("active-session", scope: scope, mode: mode))
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

    // MARK: workspace presentation preferences

    func workspacePinnedAt() -> [String: Date] {
        guard let raw = defaults.dictionary(forKey: Self.workspacePinnedAtKey) as? [String: TimeInterval] else {
            return [:]
        }
        return raw.mapValues(Date.init(timeIntervalSince1970:))
    }

    func setWorkspacePinned(_ pinned: Bool, workspaceKey: String, now: Date = Date()) -> [String: Date] {
        var current = workspacePinnedAt()
        if pinned {
            current[workspaceKey] = now
        } else {
            current.removeValue(forKey: workspaceKey)
        }
        defaults.set(current.mapValues(\.timeIntervalSince1970), forKey: Self.workspacePinnedAtKey)
        return current
    }

    /// Session pins are a client-side presentation choice, like workspace pins:
    /// the engine catalog has no notion of them.
    func sessionPinnedAt() -> [String: Date] {
        guard let raw = defaults.dictionary(forKey: Self.sessionPinnedAtKey) as? [String: TimeInterval] else {
            return [:]
        }
        return raw.mapValues(Date.init(timeIntervalSince1970:))
    }

    func setSessionPinned(_ pinned: Bool, sessionID: String, now: Date = Date()) -> [String: Date] {
        var current = sessionPinnedAt()
        if pinned {
            current[sessionID] = now
        } else {
            current.removeValue(forKey: sessionID)
        }
        defaults.set(current.mapValues(\.timeIntervalSince1970), forKey: Self.sessionPinnedAtKey)
        return current
    }

    /// When the user last looked at the scheduled-task page; results that
    /// finished after it count as new on the sidebar entry.
    func cronResultsSeenAt() -> Date? {
        let value = defaults.double(forKey: Self.cronResultsSeenAtKey)
        return value > 0 ? Date(timeIntervalSince1970: value) : nil
    }

    func setCronResultsSeenAt(_ date: Date) {
        defaults.set(date.timeIntervalSince1970, forKey: Self.cronResultsSeenAtKey)
    }

    func workspaceCollapsedKeys(mode: SessionMode) -> Set<String> {
        let values = defaults.stringArray(forKey: key("collapsed-workspaces", mode: mode)) ?? []
        return Set(values)
    }

    func setWorkspaceCollapsed(
        _ collapsed: Bool,
        workspaceKey: String,
        mode: SessionMode
    ) -> Set<String> {
        var current = workspaceCollapsedKeys(mode: mode)
        if collapsed {
            current.insert(workspaceKey)
        } else {
            current.remove(workspaceKey)
        }
        defaults.set(Array(current).sorted(), forKey: key("collapsed-workspaces", mode: mode))
        return current
    }

    /// `ConversationScope.preferenceScope` keeps the historical key strings
    /// for global/project scopes byte-identical (existing user state must
    /// survive).
    private func key(_ field: String, scope: ConversationScope) -> String {
        "conversation.\(scope.preferenceScope).\(field)"
    }

    private func key(_ field: String, scope: ConversationScope, mode: SessionMode) -> String {
        "conversation.\(scope.preferenceScope).\(mode.rawValue).\(field)"
    }

    private func key(_ field: String, mode: SessionMode) -> String {
        "conversation.\(mode.rawValue).\(field)"
    }
}
