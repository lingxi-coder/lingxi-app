import XCTest
@testable import LingxiCode

final class SessionDraftOwnershipTests: XCTestCase {
    func testFreshLaunchKeepsTheSelectedSpaceBeforeSessionCreation() {
        let suite = "SessionDraftOwnershipTests.\(UUID().uuidString)"
        let defaults = UserDefaults(suiteName: suite)!
        defer { defaults.removePersistentDomain(forName: suite) }
        let preferences = ProjectScopedPreferences(defaults: defaults)
        preferences.setLastMode(.chat)
        preferences.setCurrentMode(.code)
        XCTAssertEqual(preferences.currentMode(), .code)
        XCTAssertEqual(preferences.lastMode(), .chat)
    }

    func testRestoringSameModeKeepsEachSessionDraft() {
        let suite = "ProjectScopedPreferencesTests.\(UUID().uuidString)"
        let defaults = UserDefaults(suiteName: suite)!
        defer { defaults.removePersistentDomain(forName: suite) }
        let preferences = ProjectScopedPreferences(defaults: defaults)
        let scope = ConversationScope.project("shared")
        preferences.setDraft("session B draft", scope: scope, mode: .code, ownerSessionID: "B")

        let restored = preferences.draftForFailedSelection(
            "session A draft", scope: scope, mode: .code,
            fromOwner: "A", toOwner: "B"
        )
        XCTAssertEqual(restored, "session B draft")
        XCTAssertEqual(preferences.draft(scope: scope, mode: .code, ownerSessionID: "A"), "session A draft")

        let reopened = preferences.draftForFailedSelection(
            "edited B draft", scope: scope, mode: .code,
            fromOwner: "B", toOwner: "A"
        )
        XCTAssertEqual(reopened, "session A draft")
        XCTAssertEqual(preferences.draft(scope: scope, mode: .code, ownerSessionID: "B"), "edited B draft")
    }

    func testRestoringMissingOwnerNeverCopiesOutgoingDraft() {
        let suite = "ProjectScopedPreferencesTests.\(UUID().uuidString)"
        let defaults = UserDefaults(suiteName: suite)!
        defer { defaults.removePersistentDomain(forName: suite) }
        let preferences = ProjectScopedPreferences(defaults: defaults)
        let scope = ConversationScope.project("shared")
        XCTAssertEqual(preferences.draftForFailedSelection(
            "code draft", scope: scope, mode: .code,
            fromOwner: "code", toOwner: "unseen"
        ), "")
        XCTAssertEqual(preferences.draft(scope: scope, mode: .code, ownerSessionID: "code"), "code draft")
    }

    func testNewSessionDraftsRemainSeparateByModeAndScope() {
        let suite = "ProjectScopedPreferencesTests.\(UUID().uuidString)"
        let defaults = UserDefaults(suiteName: suite)!
        defer { defaults.removePersistentDomain(forName: suite) }
        let preferences = ProjectScopedPreferences(defaults: defaults)
        let scope = ConversationScope.global
        preferences.setDraft("chat draft", scope: scope, mode: .chat, ownerSessionID: "")
        preferences.setDraft("code draft", scope: scope, mode: .code, ownerSessionID: "")
        XCTAssertEqual(preferences.draft(scope: scope, mode: .chat, ownerSessionID: ""), "chat draft")
        XCTAssertEqual(preferences.draft(scope: scope, mode: .code, ownerSessionID: ""), "code draft")
        XCTAssertEqual(preferences.draft(scope: .project("another"), mode: .chat, ownerSessionID: ""), "")
    }
}
