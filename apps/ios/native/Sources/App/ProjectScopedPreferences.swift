import Foundation

private final class SessionScopeRegistryObservation: @unchecked Sendable {
    private let lock = NSLock()
    private var wasPresent: Bool

    init(wasPresent: Bool) { self.wasPresent = wasPresent }

    func observed(_ isPresent: Bool) -> Bool {
        lock.lock()
        defer { lock.unlock() }
        if isPresent { wasPresent = true }
        return wasPresent
    }
}

/// Persists user-editable conversation state under an explicit conversation
/// scope. The global scope is a real scope of its own; it never shares keys
/// with a managed project, so switching workspaces cannot leak
/// drafts or sessions.
struct ProjectScopedPreferences {
    private let defaults: UserDefaults
    private let scopeRegistryObservation: SessionScopeRegistryObservation
    private static let workspacePinnedAtKey = "conversation.workspace.pinned-at"
    private static let sessionPinnedAtKey = "conversation.session.pinned-at"
    private static let cronResultsSeenAtKey = "conversation.cron.results-seen-at"
    private static let lastModeKey = "conversation.spaces.last-mode"
    private static let currentModeKey = "conversation.spaces.current-mode"

    init(defaults: UserDefaults = .standard) {
        self.defaults = defaults
        self.scopeRegistryObservation = SessionScopeRegistryObservation(
            wasPresent: defaults.object(forKey: "conversation.spaces.session-scopes") != nil)
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

    func lastMode() -> SessionMode {
        defaults.string(forKey: Self.lastModeKey).flatMap(SessionMode.init(rawValue:)) ?? .code
    }

    func setLastMode(_ mode: SessionMode) {
        defaults.set(mode.rawValue, forKey: Self.lastModeKey)
    }

    func currentMode() -> SessionMode {
        defaults.string(forKey: Self.currentModeKey).flatMap(SessionMode.init(rawValue:)) ?? lastMode()
    }

    func setCurrentMode(_ mode: SessionMode) {
        defaults.set(mode.rawValue, forKey: Self.currentModeKey)
    }

    func lastScope(mode: SessionMode) -> ConversationScope? {
        defaults.string(forKey: "conversation.spaces.last-scope.\(mode.rawValue)").flatMap(ConversationScope.init(workspaceKey:))
    }

    func setLastScope(_ scope: ConversationScope, mode: SessionMode) {
        guard case .remote = scope else {
            defaults.set(scope.workspaceKey, forKey: "conversation.spaces.last-scope.\(mode.rawValue)")
            return
        }
    }

    func draft(scope: ConversationScope, mode: SessionMode) -> String {
        defaults.string(forKey: key("draft", scope: scope, mode: mode)) ?? ""
    }

    func setDraft(_ value: String, scope: ConversationScope, mode: SessionMode) {
        defaults.set(value, forKey: key("draft", scope: scope, mode: mode))
    }

    /// A session UUID owns one draft. The mode-scoped slot is used before a
    /// new session receives its ID.
    func draft(scope: ConversationScope, mode: SessionMode, ownerSessionID: String) -> String {
        let owner = canonicalSessionID(ownerSessionID)
        guard !owner.isEmpty else { return draft(scope: scope, mode: mode) }
        return (defaults.dictionary(forKey: key("session-drafts", scope: scope)) as? [String: String])?[owner] ?? ""
    }

    func setDraft(_ value: String, scope: ConversationScope, mode: SessionMode, ownerSessionID: String) {
        let owner = canonicalSessionID(ownerSessionID)
        guard !owner.isEmpty else {
            setDraft(value, scope: scope, mode: mode)
            return
        }
        let draftsKey = key("session-drafts", scope: scope)
        var drafts = defaults.dictionary(forKey: draftsKey) as? [String: String] ?? [:]
        drafts[owner] = value
        defaults.set(drafts, forKey: draftsKey)
    }

    /// Store the original UTF-16 alongside its display draft so a restored
    /// handoff or suggestion does not normalize otherwise unrepresentable units.
    func draftSourceCodeUnits(scope: ConversationScope, mode: SessionMode, ownerSessionID: String) -> [UInt16]? {
        let storageKey = key("draft-source-units", scope: scope, mode: mode)
        let owner = canonicalSessionID(ownerSessionID)
        guard let values = defaults.dictionary(forKey: storageKey) as? [String: Data],
              let data = values[owner],
              let units = try? JSONDecoder().decode([UInt16].self, from: data),
              Array(String(decoding: units, as: UTF16.self).utf16)
                == Array(draft(scope: scope, mode: mode, ownerSessionID: ownerSessionID).utf16)
        else { return nil }
        return units
    }

    func setDraftSourceCodeUnits(_ units: [UInt16]?, scope: ConversationScope,
                                 mode: SessionMode, ownerSessionID: String) {
        let storageKey = key("draft-source-units", scope: scope, mode: mode)
        let owner = canonicalSessionID(ownerSessionID)
        var values = defaults.dictionary(forKey: storageKey) as? [String: Data] ?? [:]
        if let units, let data = try? JSONEncoder().encode(units) {
            values[owner] = data
        } else {
            values.removeValue(forKey: owner)
        }
        defaults.set(values, forKey: storageKey)
    }

    /// Save the outgoing owner before restoring the prior selection.
    func draftForFailedSelection(
        _ value: String,
        scope: ConversationScope,
        mode: SessionMode,
        fromOwner: String,
        toOwner: String
    ) -> String {
        setDraft(value, scope: scope, mode: mode, ownerSessionID: fromOwner)
        return draft(scope: scope, mode: mode, ownerSessionID: toOwner)
    }

    func activeSessionID(scope: ConversationScope, mode: SessionMode) -> String {
        storedActiveSessionID(scope: scope, mode: mode) ?? ""
    }

    func storedActiveSessionID(scope: ConversationScope, mode: SessionMode) -> String? {
        defaults.string(forKey: key("active-session", scope: scope, mode: mode))
    }

    func setActiveSessionID(_ value: String, scope: ConversationScope, mode: SessionMode) {
        defaults.set(value, forKey: key("active-session", scope: scope, mode: mode))
    }

    /// Session scope is immutable even when its cwd later points into another Project.
    func sessionScope(sessionID: String) -> ConversationScope? {
        let id = canonicalSessionID(sessionID)
        guard !id.isEmpty,
              let value = sessionScopeRegistry()?[id]
        else { return nil }
        return ConversationScope(workspaceKey: value)
    }

    var sessionScopeClaims: [String: ConversationScope]? {
        guard let owners = sessionScopeRegistry() else { return nil }
        var claims: [String: ConversationScope] = [:]
        for (id, value) in owners {
            guard let scope = ConversationScope(workspaceKey: value) else { return nil }
            claims[id] = scope
        }
        return claims
    }

    var hasValidSessionScopeRegistry: Bool { sessionScopeClaims != nil }

    private func sessionScopeRegistry() -> [String: String]? {
        let key = "conversation.spaces.session-scopes"
        guard let stored = defaults.object(forKey: key) else {
            return scopeRegistryObservation.observed(false) ? nil : [:]
        }
        _ = scopeRegistryObservation.observed(true)
        guard let owners = stored as? [String: String], owners.allSatisfy({ id, scope in
            !id.isEmpty && id == canonicalSessionID(id) && ConversationScope(workspaceKey: scope) != nil
        }) else { return nil }
        return owners
    }

    @discardableResult
    func claimSessionScope(sessionID: String, scope: ConversationScope) -> Bool {
        let id = canonicalSessionID(sessionID)
        guard !id.isEmpty else { return false }
        if case .remote = scope { return false }
        guard var owners = sessionScopeRegistry() else { return false }
        if let old = owners[id] { return old == scope.workspaceKey }
        owners[id] = scope.workspaceKey
        defaults.set(owners, forKey: "conversation.spaces.session-scopes")
        _ = scopeRegistryObservation.observed(true)
        return true
    }

    func setHandoffReference(_ reference: SessionHandoffReference?, sessionID: String,
                             scope: ConversationScope, mode: SessionMode) {
        let storageKey = key("handoff-references", scope: scope, mode: mode)
        var values = defaults.dictionary(forKey: storageKey) as? [String: Data] ?? [:]
        let owner = canonicalSessionID(sessionID)
        if let reference, let data = try? JSONEncoder().encode(reference) {
            values[owner] = data
        } else {
            values.removeValue(forKey: owner)
        }
        defaults.set(values, forKey: storageKey)
    }

    func handoffReference(sessionID: String, scope: ConversationScope, mode: SessionMode) -> SessionHandoffReference? {
        guard let values = defaults.dictionary(forKey: key("handoff-references", scope: scope, mode: mode)) as? [String: Data],
              let data = values[canonicalSessionID(sessionID)] else { return nil }
        return try? JSONDecoder().decode(SessionHandoffReference.self, from: data)
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

    private func key(_ field: String, scope: ConversationScope) -> String {
        "conversation.spaces.\(scope.preferenceScope).\(field)"
    }

    private func key(_ field: String, scope: ConversationScope, mode: SessionMode) -> String {
        "conversation.spaces.\(scope.preferenceScope).\(mode.rawValue).\(field)"
    }

    private func key(_ field: String, mode: SessionMode) -> String {
        "conversation.spaces.\(mode.rawValue).\(field)"
    }
}

enum ConversationModeRestorePolicy {
    static func sessionID(
        mode: SessionMode,
        scoped: String?,
        sessions: [ProjectSessionSummary] = []
    ) -> String {
        let selected = scoped ?? ""
        guard !selected.isEmpty else { return "" }
        if let indexed = sessions.first(where: { canonicalSessionID($0.sessionId) == canonicalSessionID(selected) }), indexed.mode != mode { return "" }
        return selected
    }
}
