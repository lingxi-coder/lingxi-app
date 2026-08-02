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
}
