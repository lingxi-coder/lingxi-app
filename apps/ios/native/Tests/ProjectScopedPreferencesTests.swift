import XCTest
@testable import LingxiCode

final class ProjectScopedPreferencesTests: XCTestCase {
    func testHandoffDraftSourceAndReferenceSurviveRestoreWithoutChangingOwners() {
        let suite = "ProjectScopedPreferencesTests.\(UUID().uuidString)"
        let defaults = UserDefaults(suiteName: suite)!
        defer { defaults.removePersistentDomain(forName: suite) }
        let preferences = ProjectScopedPreferences(defaults: defaults)
        let units: [UInt16] = [0x61, 0xD800, 0x62]
        let reference = SessionHandoffReference(sourceScopeKey: "project.p", sourceSessionID: "source",
            sourceMode: .code, targetMode: .chat, cutoff: "fixed-cutoff", requestID: "request")
        preferences.setDraft(String(decoding: units, as: UTF16.self), scope: .global, mode: .chat, ownerSessionID: "target")
        preferences.setDraftSourceCodeUnits(units, scope: .global, mode: .chat, ownerSessionID: "target")
        preferences.setHandoffReference(reference, sessionID: "target", scope: .global, mode: .chat)
        let reopened = ProjectScopedPreferences(defaults: defaults)
        XCTAssertEqual(reopened.draftSourceCodeUnits(scope: .global, mode: .chat, ownerSessionID: "target"), units)
        XCTAssertNil(reopened.draftSourceCodeUnits(scope: .project("p"), mode: .chat, ownerSessionID: "target"))
        XCTAssertNil(reopened.draftSourceCodeUnits(scope: .global, mode: .code, ownerSessionID: "target"))
        XCTAssertEqual(reopened.handoffReference(sessionID: "target", scope: .global, mode: .chat), reference)
        XCTAssertNil(reopened.handoffReference(sessionID: "target", scope: .global, mode: .code))
        reopened.setDraft("edited", scope: .global, mode: .chat, ownerSessionID: "target")
        XCTAssertNil(reopened.draftSourceCodeUnits(scope: .global, mode: .chat, ownerSessionID: "target"))
        reopened.setDraft("é", scope: .global, mode: .chat, ownerSessionID: "target")
        reopened.setDraftSourceCodeUnits(Array("é".utf16), scope: .global, mode: .chat, ownerSessionID: "target")
        reopened.setDraft("e\u{0301}", scope: .global, mode: .chat, ownerSessionID: "target")
        XCTAssertNil(reopened.draftSourceCodeUnits(scope: .global, mode: .chat, ownerSessionID: "target"),
                     "Canonical-equivalent edits must not restore stale source units")
    }

    func testLastScopeIsStoredIndependentlyForEachMode() {
        let suite = "ProjectScopedPreferencesTests.\(UUID().uuidString)"
        let defaults = UserDefaults(suiteName: suite)!
        defer { defaults.removePersistentDomain(forName: suite) }
        let preferences = ProjectScopedPreferences(defaults: defaults)
        preferences.setLastScope(.project("code-project"), mode: .code)
        preferences.setLastScope(.global, mode: .chat)
        XCTAssertEqual(preferences.lastScope(mode: .code), .project("code-project"))
        XCTAssertEqual(preferences.lastScope(mode: .chat), .global)
    }

    func testSessionScopeCannotFollowItsWorkspaceIntoAProject() {
        let suite = "ProjectScopedPreferencesTests.\(UUID().uuidString)"
        let defaults = UserDefaults(suiteName: suite)!
        defer { defaults.removePersistentDomain(forName: suite) }
        let preferences = ProjectScopedPreferences(defaults: defaults)
        XCTAssertTrue(preferences.claimSessionScope(sessionID: "sess:abc", scope: .global))
        XCTAssertFalse(preferences.claimSessionScope(sessionID: "abc", scope: .project("p")))
        XCTAssertEqual(preferences.sessionScope(sessionID: "abc"), .global)
    }

    func testInvalidScopeRegistryIsNotOverwrittenByAClaim() {
        let suite = "ProjectScopedPreferencesTests.\(UUID().uuidString)"
        let defaults = UserDefaults(suiteName: suite)!
        defer { defaults.removePersistentDomain(forName: suite) }
        let key = "conversation.spaces.session-scopes"
        for invalid: [String: Any] in [["existing": 4], ["existing": "project."]] {
            defaults.set(invalid, forKey: key)
            let preferences = ProjectScopedPreferences(defaults: defaults)
            XCTAssertFalse(preferences.hasValidSessionScopeRegistry)
            XCTAssertFalse(preferences.claimSessionScope(sessionID: "new", scope: .global))
            XCTAssertEqual(defaults.dictionary(forKey: key) as NSDictionary?, invalid as NSDictionary)
        }
    }

    func testObservedScopeRegistryCannotDisappearAndBeRecreated() {
        let suite = "ProjectScopedPreferencesTests.\(UUID().uuidString)"
        let defaults = UserDefaults(suiteName: suite)!
        defer { defaults.removePersistentDomain(forName: suite) }
        let preferences = ProjectScopedPreferences(defaults: defaults)
        XCTAssertTrue(preferences.claimSessionScope(sessionID: "existing", scope: .global))
        defaults.removeObject(forKey: "conversation.spaces.session-scopes")
        XCTAssertFalse(preferences.hasValidSessionScopeRegistry)
        XCTAssertFalse(preferences.claimSessionScope(sessionID: "new", scope: .global))
        XCTAssertNil(defaults.object(forKey: "conversation.spaces.session-scopes"))
    }

    func testLastModeDefaultsToCodeAndPersistsTheSelection() {
        let suite = "ProjectScopedPreferencesTests.\(UUID().uuidString)"
        let defaults = UserDefaults(suiteName: suite)!
        defer { defaults.removePersistentDomain(forName: suite) }
        let preferences = ProjectScopedPreferences(defaults: defaults)
        defaults.set("chat", forKey: "conversation.last-mode")
        defaults.set("chat", forKey: "conversation.current-mode")
        XCTAssertEqual(preferences.lastMode(), .code)
        XCTAssertEqual(preferences.currentMode(), .code)
        preferences.setLastMode(.chat)
        XCTAssertEqual(ProjectScopedPreferences(defaults: defaults).lastMode(), .chat)
        preferences.setCurrentMode(.code)
        XCTAssertEqual(preferences.currentMode(), .code)
        XCTAssertEqual(preferences.lastMode(), .chat)
        XCTAssertEqual(preferences.currentMode(), .code)
    }
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

    func testRestoreUsesOnlyTheSelectedModeSession() {
        XCTAssertEqual(
            ConversationModeRestorePolicy.sessionID(
                mode: .chat,
                scoped: nil
            ),
            ""
        )
        XCTAssertEqual(
            ConversationModeRestorePolicy.sessionID(
                mode: .chat,
                scoped: "chat-session"
            ),
            "chat-session"
        )
    }

    func testCodeRestoreDoesNotAdoptAnUnscopedSession() {
        XCTAssertEqual(
            ConversationModeRestorePolicy.sessionID(
                mode: .code,
                scoped: nil
            ),
            ""
        )
    }

    func testAutomaticResumeKeepsSessionCapturedDirectoryAndMode() {
        let summary = ProjectSessionSummary(sessionId: "session-a", title: "A", messageCount: 1, relativeTime: "Now", updatedAt: Date(),
            workspaceDirectory: "/home/lingxi/a")
        XCTAssertEqual(ConversationModeRestorePolicy.sessionID(mode: .code, scoped: "session-a", sessions: [summary]), "session-a")
        XCTAssertEqual(ConversationModeRestorePolicy.sessionID(mode: .chat, scoped: "session-a", sessions: [summary]), "")
    }

    func testDraftsAndSessionsAreIsolatedByProject() {
        let suite = "ProjectScopedPreferencesTests.\(UUID().uuidString)"
        let defaults = UserDefaults(suiteName: suite)!
        defer { defaults.removePersistentDomain(forName: suite) }
        let preferences = ProjectScopedPreferences(defaults: defaults)

        preferences.setDraft("global", scope: .global, mode: .code)
        preferences.setDraft("alpha", scope: .project("A"), mode: .code)
        preferences.setDraft("beta", scope: .project("B"), mode: .code)
        preferences.setActiveSessionID("session-a", scope: .project("A"), mode: .code)
        preferences.setActiveSessionID("session-b", scope: .project("B"), mode: .code)

        XCTAssertEqual(preferences.draft(scope: .global, mode: .code), "global")
        XCTAssertEqual(preferences.draft(scope: .project("A"), mode: .code), "alpha")
        XCTAssertEqual(preferences.draft(scope: .project("B"), mode: .code), "beta")
        XCTAssertEqual(preferences.activeSessionID(scope: .project("A"), mode: .code), "session-a")
        XCTAssertEqual(preferences.activeSessionID(scope: .project("B"), mode: .code), "session-b")
    }

    func testStoredSessionDistinguishesMissingSelectionFromExplicitNewChat() {
        let suite = "ProjectScopedPreferencesTests.\(UUID().uuidString)"
        let defaults = UserDefaults(suiteName: suite)!
        defer { defaults.removePersistentDomain(forName: suite) }
        let preferences = ProjectScopedPreferences(defaults: defaults)

        XCTAssertNil(preferences.storedActiveSessionID(scope: .project("p"), mode: .code))

        preferences.setActiveSessionID("", scope: .project("p"), mode: .code)

        XCTAssertEqual(preferences.storedActiveSessionID(scope: .project("p"), mode: .code), "")
    }

    func testOldScopeKeysAreNotReadAndCurrentSpacesStayIsolated() {
        let suite = "ProjectScopedPreferencesTests.\(UUID().uuidString)"
        let defaults = UserDefaults(suiteName: suite)!
        defer { defaults.removePersistentDomain(forName: suite) }
        let preferences = ProjectScopedPreferences(defaults: defaults)

        defaults.set("legacy-draft", forKey: "conversation.proj-a.draft")
        defaults.set("legacy-session", forKey: "conversation.proj-a.active-session")
        defaults.set("legacy-code-draft", forKey: "conversation.proj-a.code.draft")
        defaults.set("legacy-code-session", forKey: "conversation.proj-a.code.active-session")
        defaults.set(["same-session": "legacy-session-draft"], forKey: "conversation.proj-a.session-drafts")
        XCTAssertEqual(preferences.draft(scope: .project("Proj-A"), mode: .code), "")
        XCTAssertNil(preferences.storedActiveSessionID(scope: .project("proj-a"), mode: .code))
        XCTAssertEqual(preferences.draft(scope: .project("proj-a"), mode: .code, ownerSessionID: "same-session"), "")

        preferences.setDraft("current", scope: .project("Proj-A"), mode: .code)
        preferences.setDraft("global", scope: .global, mode: .code)
        preferences.setDraft("scheduled", scope: .scheduled, mode: .code)
        XCTAssertEqual(preferences.draft(scope: .project("proj-a"), mode: .code), "current")
        XCTAssertEqual(preferences.draft(scope: .global, mode: .code), "global")
        XCTAssertEqual(preferences.draft(scope: .scheduled, mode: .code), "scheduled")
        XCTAssertEqual(defaults.string(forKey: "conversation.proj-a.draft"), "legacy-draft")
    }

    func testSessionDraftUsesCanonicalOwnerIdentity() {
        let suite = "ProjectScopedPreferencesTests.\(UUID().uuidString)"
        let defaults = UserDefaults(suiteName: suite)!
        defer { defaults.removePersistentDomain(forName: suite) }
        let preferences = ProjectScopedPreferences(defaults: defaults)
        let id = UUID().uuidString
        preferences.setDraft("owned", scope: .global, mode: .code, ownerSessionID: "sess:\(id)")
        XCTAssertEqual(preferences.draft(scope: .global, mode: .code, ownerSessionID: id.lowercased()), "owned")
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
        XCTAssertEqual(defaults.string(forKey: "conversation.spaces.project.proj-a.chat.draft"), "chat-draft")
        XCTAssertEqual(defaults.string(forKey: "conversation.spaces.project.proj-a.code.active-session"), "code-session")
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
