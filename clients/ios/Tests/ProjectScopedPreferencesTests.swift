import XCTest
@testable import LingxiCode

final class ProjectScopedPreferencesTests: XCTestCase {
    func testDraftsAndSessionsAreIsolatedByProject() {
        let suite = "ProjectScopedPreferencesTests.\(UUID().uuidString)"
        let defaults = UserDefaults(suiteName: suite)!
        defer { defaults.removePersistentDomain(forName: suite) }
        let preferences = ProjectScopedPreferences(defaults: defaults)

        preferences.setDraft("global", projectID: nil)
        preferences.setDraft("alpha", projectID: "A")
        preferences.setDraft("beta", projectID: "B")
        preferences.setActiveSessionID("session-a", projectID: "A")
        preferences.setActiveSessionID("session-b", projectID: "B")

        XCTAssertEqual(preferences.draft(projectID: nil), "global")
        XCTAssertEqual(preferences.draft(projectID: "A"), "alpha")
        XCTAssertEqual(preferences.draft(projectID: "B"), "beta")
        XCTAssertEqual(preferences.activeSessionID(projectID: "A"), "session-a")
        XCTAssertEqual(preferences.activeSessionID(projectID: "B"), "session-b")
    }

    func testStoredSessionDistinguishesLegacyMissingKeyFromExplicitNewChat() {
        let suite = "ProjectScopedPreferencesTests.\(UUID().uuidString)"
        let defaults = UserDefaults(suiteName: suite)!
        defer { defaults.removePersistentDomain(forName: suite) }
        let preferences = ProjectScopedPreferences(defaults: defaults)

        XCTAssertNil(preferences.storedActiveSessionID(projectID: "legacy-project"))

        preferences.setActiveSessionID("", projectID: "legacy-project")

        XCTAssertEqual(preferences.storedActiveSessionID(projectID: "legacy-project"), "")
    }

    /// Back-compat pin: `.project(id)` / `.global` MUST produce the SAME key
    /// strings the pre-scope `projectID:` path wrote, so existing user state
    /// survives the ConversationScope refactor. `.localApp` only ADDS the
    /// `app.<id>` namespace.
    func testScopeKeysPreserveLegacyProjectKeysAndAddTheAppNamespace() {
        let suite = "ProjectScopedPreferencesTests.\(UUID().uuidString)"
        let defaults = UserDefaults(suiteName: suite)!
        defer { defaults.removePersistentDomain(forName: suite) }
        let preferences = ProjectScopedPreferences(defaults: defaults)

        // State written under the LEGACY spelling (what shipped builds wrote):
        defaults.set("legacy-draft", forKey: "conversation.proj-a.draft")
        defaults.set("legacy-session", forKey: "conversation.proj-a.active-session")
        defaults.set("global-draft", forKey: "conversation.global.draft")

        // …must be readable through the scope spelling unchanged.
        XCTAssertEqual(preferences.draft(scope: .project("Proj-A")), "legacy-draft")
        XCTAssertEqual(preferences.storedActiveSessionID(scope: .project("proj-a")), "legacy-session")
        XCTAssertEqual(preferences.draft(scope: .global), "global-draft")

        // The app namespace is additive and isolated from a same-named project.
        preferences.setDraft("app-draft", scope: .localApp("proj-a"))
        preferences.setActiveSessionID("app-session", scope: .localApp("Proj-A"))
        XCTAssertEqual(defaults.string(forKey: "conversation.app.proj-a.draft"), "app-draft")
        XCTAssertEqual(defaults.string(forKey: "conversation.app.proj-a.active-session"), "app-session")
        XCTAssertEqual(preferences.draft(scope: .localApp("proj-a")), "app-draft")
        XCTAssertEqual(preferences.draft(scope: .project("proj-a")), "legacy-draft", "the project key is untouched")
    }

    func testAppScopeIsolatesDraftsAndSessionsPerApp() {
        let suite = "ProjectScopedPreferencesTests.\(UUID().uuidString)"
        let defaults = UserDefaults(suiteName: suite)!
        defer { defaults.removePersistentDomain(forName: suite) }
        let preferences = ProjectScopedPreferences(defaults: defaults)

        preferences.setDraft("tracker-draft", scope: .localApp("tracker"))
        preferences.setDraft("notes-draft", scope: .localApp("notes"))
        preferences.setActiveSessionID("tracker-session", scope: .localApp("tracker"))

        XCTAssertEqual(preferences.draft(scope: .localApp("tracker")), "tracker-draft")
        XCTAssertEqual(preferences.draft(scope: .localApp("notes")), "notes-draft")
        XCTAssertEqual(preferences.activeSessionID(scope: .localApp("tracker")), "tracker-session")
        XCTAssertNil(preferences.storedActiveSessionID(scope: .localApp("notes")))
    }
}
