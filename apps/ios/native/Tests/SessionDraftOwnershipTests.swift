import XCTest
@testable import LingxiCode

final class SessionDraftOwnershipTests: XCTestCase {
    func testFreshLaunchUsesLastExplicitModeAfterRestoringAnotherMode() {
        let suite = "SessionDraftOwnershipTests.\(UUID().uuidString)"
        let defaults = UserDefaults(suiteName: suite)!
        defer { defaults.removePersistentDomain(forName: suite) }
        let preferences = ProjectScopedPreferences(defaults: defaults)
        preferences.setLastMode(.chat)
        preferences.setCurrentMode(.code)
        XCTAssertEqual(preferences.launchMode(restoredSessionID: "saved"), .code)
        XCTAssertEqual(preferences.launchMode(restoredSessionID: ""), .chat)
        XCTAssertEqual(preferences.lastMode(), .chat)
    }

    func testRestoringSameModeKeepsEachSessionDraft() {
        let suite = "ProjectScopedPreferencesTests.\(UUID().uuidString)"
        let defaults = UserDefaults(suiteName: suite)!
        defer { defaults.removePersistentDomain(forName: suite) }
        let preferences = ProjectScopedPreferences(defaults: defaults)
        let scope = ConversationScope.project("shared")
        preferences.setDraft("session B draft", scope: scope, mode: .code, ownerSessionID: "B")

        let restored = preferences.draftForSessionTransition(
            "session A draft", scope: scope, from: .code, to: .code,
            fromOwner: "A", toOwner: "B", restoring: true
        )
        XCTAssertEqual(restored, "session B draft")
        XCTAssertEqual(preferences.draft(scope: scope, mode: .code, ownerSessionID: "A"), "session A draft")

        let reopened = preferences.draftForSessionTransition(
            "edited B draft", scope: scope, from: .code, to: .code,
            fromOwner: "B", toOwner: "A", restoring: true
        )
        XCTAssertEqual(reopened, "session A draft")
        XCTAssertEqual(preferences.draft(scope: scope, mode: .code, ownerSessionID: "B"), "edited B draft")
    }

    func testRestoringAnotherModeAndMissingOwnerNeverCopiesOutgoingDraft() {
        let suite = "ProjectScopedPreferencesTests.\(UUID().uuidString)"
        let defaults = UserDefaults(suiteName: suite)!
        defer { defaults.removePersistentDomain(forName: suite) }
        let preferences = ProjectScopedPreferences(defaults: defaults)
        let scope = ConversationScope.project("shared")
        preferences.setDraft("chat draft", scope: scope, mode: .chat, ownerSessionID: "chat")
        XCTAssertEqual(preferences.draftForSessionTransition(
            "code draft", scope: scope, from: .code, to: .chat,
            fromOwner: "code", toOwner: "chat", restoring: true
        ), "chat draft")
        XCTAssertEqual(preferences.draftForSessionTransition(
            "chat draft", scope: scope, from: .chat, to: .code,
            fromOwner: "chat", toOwner: "unseen", restoring: true
        ), "")
        XCTAssertEqual(preferences.draft(scope: scope, mode: .code, ownerSessionID: "code"), "code draft")
    }

    func testModeChangeAndNewSessionIDKeepTheOwnersDraft() {
        let suite = "ProjectScopedPreferencesTests.\(UUID().uuidString)"
        let defaults = UserDefaults(suiteName: suite)!
        defer { defaults.removePersistentDomain(forName: suite) }
        let preferences = ProjectScopedPreferences(defaults: defaults)
        let scope = ConversationScope.global
        XCTAssertEqual(preferences.draftForSessionTransition(
            "new draft", scope: scope, from: .code, to: .chat,
            fromOwner: "", toOwner: "", restoring: false
        ), "new draft")
        XCTAssertEqual(preferences.draft(scope: scope, mode: .code, ownerSessionID: ""), "")
        XCTAssertEqual(preferences.draftForSessionTransition(
            "new draft", scope: scope, from: .chat, to: .chat,
            fromOwner: "", toOwner: "new-session", restoring: false
        ), "new draft")
        XCTAssertEqual(preferences.draft(scope: scope, mode: .chat, ownerSessionID: ""), "")
        XCTAssertEqual(preferences.draft(scope: scope, mode: .code, ownerSessionID: "new-session"), "new draft")
        XCTAssertEqual(preferences.draftForSessionTransition(
            "edited", scope: scope, from: .chat, to: .code,
            fromOwner: "new-session", toOwner: "new-session", restoring: false
        ), "edited")
        XCTAssertEqual(preferences.draft(scope: scope, mode: .chat, ownerSessionID: "new-session"), "edited")
        XCTAssertEqual(preferences.draft(scope: .project("another"), mode: .chat, ownerSessionID: "new-session"), "")
    }
}
