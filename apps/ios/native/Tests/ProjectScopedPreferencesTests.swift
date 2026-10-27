import XCTest
@testable import LingxiCode

final class ProjectScopedPreferencesTests: XCTestCase {
    #if DEBUG
    func testUITestSessionsAndDraftsCannotOverwriteUserPreferences() {
        let appSuite = "ProjectScopedPreferencesTests.app.\(UUID().uuidString)"
        let testSuite = "ProjectScopedPreferencesTests.ui.\(UUID().uuidString)"
        let appDefaults = UserDefaults(suiteName: appSuite)!
        let testDefaults = UserDefaults(suiteName: testSuite)!
        defer {
            appDefaults.removePersistentDomain(forName: appSuite)
            testDefaults.removePersistentDomain(forName: testSuite)
        }

        let normal = ProjectScopedPreferences.forRuntime(
            environment: [:], appDefaults: appDefaults, uiTestSuiteName: testSuite
        )
        normal.setActiveSessionID("real-session", scope: .global, mode: .code)
        normal.setDraft("real draft", scope: .global, mode: .code)

        let uiTest = ProjectScopedPreferences.forRuntime(
            environment: ["LINGXI_UI_TESTING": "1"], appDefaults: appDefaults, uiTestSuiteName: testSuite
        )
        uiTest.setActiveSessionID("ui-files-session", scope: .global, mode: .code)
        uiTest.setDraft("test draft", scope: .global, mode: .code)

        let reopened = ProjectScopedPreferences.forRuntime(
            environment: [:], appDefaults: appDefaults, uiTestSuiteName: testSuite
        )
        XCTAssertEqual(reopened.activeSessionID(scope: .global, mode: .code), "real-session")
        XCTAssertEqual(reopened.draft(scope: .global, mode: .code), "real draft")
        let reopenedUITest = ProjectScopedPreferences.forRuntime(
            environment: ["LINGXI_UI_TESTING": "1"], appDefaults: appDefaults, uiTestSuiteName: testSuite
        )
        XCTAssertEqual(reopenedUITest.activeSessionID(scope: .global, mode: .code), "ui-files-session")
        XCTAssertEqual(reopenedUITest.draft(scope: .global, mode: .code), "test draft")
    }
    #endif

    func testChatRestoreNeverFallsBackToLegacyCodeSession() {
        XCTAssertEqual(
            ConversationModeRestorePolicy.sessionID(
                mode: .chat,
                scoped: nil,

                projectLastActive: "project-code"
            ),
            ""
        )
        XCTAssertEqual(
            ConversationModeRestorePolicy.sessionID(
                mode: .chat,
                scoped: "chat-session",

                projectLastActive: "project-code"
            ),
            "chat-session"
        )
    }

    func testCodeRestoreUsesCurrentProjectSession() {
        XCTAssertEqual(
            ConversationModeRestorePolicy.sessionID(
                mode: .code,
                scoped: nil,

                projectLastActive: "project-code"
            ),
            "project-code"
        )
    }

    func testAutomaticResumeDoesNotReplaceAnEmptyDraftInAnotherDirectory() {
        let summary = ProjectSessionSummary(sessionId: "session-a", title: "A", messageCount: 1, relativeTime: "Now", updatedAt: Date(),
            workspaceDirectory: "/home/lingxi/a")
        XCTAssertEqual(ConversationModeRestorePolicy.sessionID(mode: .code, scoped: "session-a", projectLastActive: "session-a",
            activeDirectory: "/home/lingxi/b", sessions: [summary]), "")
        XCTAssertEqual(ConversationModeRestorePolicy.sessionID(mode: .code, scoped: "session-a", projectLastActive: "session-a",
            activeDirectory: "/home/lingxi/a", sessions: [summary]), "session-a")
    }

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
    /// survives the ConversationScope refactor.
    func testScopeKeysPreserveLegacyProjectKeys() {
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

        preferences.setDraft("scheduled-draft", scope: .scheduled)
        XCTAssertEqual(defaults.string(forKey: "conversation.scheduled.draft"), "scheduled-draft")
        XCTAssertEqual(preferences.draft(scope: .project("proj-a")), "legacy-draft", "the project key is untouched")
    }

    func testModeScopedKeysKeepChatAndCodeStateSeparate() {
        let suite = "ProjectScopedPreferencesTests.\(UUID().uuidString)"
        let defaults = UserDefaults(suiteName: suite)!
        defer { defaults.removePersistentDomain(forName: suite) }
        let preferences = ProjectScopedPreferences(defaults: defaults)

        preferences.setDraft("chat-draft", scope: .project("proj-a"), mode: .chat)
        preferences.setDraft("code-draft", scope: .project("proj-a"), mode: .code)
        preferences.setActiveSessionID("chat-session", scope: .project("proj-a"), mode: .chat)
        preferences.setActiveSessionID("code-session", scope: .project("proj-a"), mode: .code)

        XCTAssertEqual(preferences.draft(scope: .project("proj-a"), mode: .chat), "chat-draft")
        XCTAssertEqual(preferences.draft(scope: .project("proj-a"), mode: .code), "code-draft")
        XCTAssertEqual(preferences.activeSessionID(scope: .project("proj-a"), mode: .chat), "chat-session")
        XCTAssertEqual(preferences.activeSessionID(scope: .project("proj-a"), mode: .code), "code-session")
        XCTAssertEqual(defaults.string(forKey: "conversation.proj-a.chat.draft"), "chat-draft")
        XCTAssertEqual(defaults.string(forKey: "conversation.proj-a.code.active-session"), "code-session")
    }

    func testSessionPinsPersistAndUnpinRemovesThem() {
        let suite = "ProjectScopedPreferencesTests.\(UUID().uuidString)"
        let defaults = UserDefaults(suiteName: suite)!
        defer { defaults.removePersistentDomain(forName: suite) }
        let preferences = ProjectScopedPreferences(defaults: defaults)
        let stamp = Date(timeIntervalSince1970: 1_000_000)

        XCTAssertTrue(preferences.sessionPinnedAt().isEmpty)

        let pinned = preferences.setSessionPinned(true, sessionID: "s1", now: stamp)
        XCTAssertEqual(pinned["s1"], stamp)
        XCTAssertEqual(preferences.sessionPinnedAt()["s1"], stamp)
        // Session pins are separate from workspace pins.
        XCTAssertTrue(preferences.workspacePinnedAt().isEmpty)

        let unpinned = preferences.setSessionPinned(false, sessionID: "s1")
        XCTAssertTrue(unpinned.isEmpty)
        XCTAssertTrue(preferences.sessionPinnedAt().isEmpty)
    }

    func testCronResultsSeenAtIsAbsentUntilSet() {
        let suite = "ProjectScopedPreferencesTests.\(UUID().uuidString)"
        let defaults = UserDefaults(suiteName: suite)!
        defer { defaults.removePersistentDomain(forName: suite) }
        let preferences = ProjectScopedPreferences(defaults: defaults)
        let stamp = Date(timeIntervalSince1970: 2_000_000)

        XCTAssertNil(preferences.cronResultsSeenAt())

        preferences.setCronResultsSeenAt(stamp)

        XCTAssertEqual(preferences.cronResultsSeenAt(), stamp)
    }
}
