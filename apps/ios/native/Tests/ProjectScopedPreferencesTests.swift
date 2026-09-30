import XCTest
@testable import LingxiCode

final class ProjectScopedPreferencesTests: XCTestCase {
    func testChatRestoreNeverFallsBackToLegacyCodeSession() {
        XCTAssertEqual(
            ConversationModeRestorePolicy.sessionID(
                mode: .chat,
                scoped: nil,
                legacyScoped: "legacy-code",
                projectLastActive: "project-code"
            ),
            ""
        )
        XCTAssertEqual(
            ConversationModeRestorePolicy.sessionID(
                mode: .chat,
                scoped: "chat-session",
                legacyScoped: "legacy-code",
                projectLastActive: "project-code"
            ),
            "chat-session"
        )
    }

    func testCodeRestoreKeepsLegacyFallback() {
        XCTAssertEqual(
            ConversationModeRestorePolicy.sessionID(
                mode: .code,
                scoped: nil,
                legacyScoped: "legacy-code",
                projectLastActive: "project-code"
            ),
            "legacy-code"
        )
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

    func testWorkspaceGroupBuilderSortsGlobalPinnedAndRecentGroupsDeterministically() {
        let now = Date(timeIntervalSince1970: 1_000_000)
        let groups = WorkspaceGroupBuilder.seededConversationGroups(
            section: .code,
            query: "",
            groups: [
                WorkspaceGroupSeed(
                    scope: .project("older"),
                    kind: .project,
                    title: "Older",
                    subtitle: nil,
                    updatedAt: now.addingTimeInterval(-500),
                    sessions: [
                        WorkspaceSessionRow(
                            id: "older-code",
                            title: "Older code",
                            mode: .code,
                            modifiedAt: now.addingTimeInterval(-400),
                            relativeTime: "earlier",
                            messageCount: 1,
                            isInit: false
                        )
                    ]
                ),
                WorkspaceGroupSeed(
                    scope: .global,
                    kind: .global,
                    title: "Global",
                    subtitle: nil,
                    updatedAt: nil,
                    sessions: []
                ),
                WorkspaceGroupSeed(
                    scope: .project("pinned"),
                    kind: .project,
                    title: "Pinned",
                    subtitle: nil,
                    updatedAt: now.addingTimeInterval(-700),
                    sessions: [
                        WorkspaceSessionRow(
                            id: "pinned-code",
                            title: "Pinned code",
                            mode: .code,
                            modifiedAt: now.addingTimeInterval(-800),
                            relativeTime: "old",
                            messageCount: 1,
                            isInit: false
                        )
                    ]
                ),
                WorkspaceGroupSeed(
                    scope: .localApp("app"),
                    kind: .localApp,
                    title: "App",
                    subtitle: nil,
                    updatedAt: now.addingTimeInterval(-100),
                    sessions: [
                        WorkspaceSessionRow(
                            id: "app-code",
                            title: "App code",
                            mode: .code,
                            modifiedAt: now.addingTimeInterval(-50),
                            relativeTime: "now",
                            messageCount: 2,
                            isInit: false
                        )
                    ]
                )
            ],
            pinnedAt: ["project.pinned": now.addingTimeInterval(-10)]
        )

        XCTAssertEqual(groups.map(\.key), ["global", "project.pinned", "app.app", "project.older"])
    }

    func testWorkspaceGroupBuilderFiltersByModeAndSearch() {
        let now = Date(timeIntervalSince1970: 1_000_000)
        let groups = WorkspaceGroupBuilder.seededConversationGroups(
            section: .chat,
            query: "spec",
            groups: [
                WorkspaceGroupSeed(
                    scope: .project("alpha"),
                    kind: .project,
                    title: "Alpha",
                    subtitle: nil,
                    updatedAt: now,
                    sessions: [
                        WorkspaceSessionRow(
                            id: "code-1",
                            title: "Build spec",
                            mode: .code,
                            modifiedAt: now,
                            relativeTime: "now",
                            messageCount: 2,
                            isInit: false
                        ),
                        WorkspaceSessionRow(
                            id: "chat-1",
                            title: "Review spec",
                            mode: .chat,
                            modifiedAt: now.addingTimeInterval(-5),
                            relativeTime: "now",
                            messageCount: 3,
                            isInit: false
                        ),
                        WorkspaceSessionRow(
                            id: "chat-2",
                            title: "Unrelated notes",
                            mode: .chat,
                            modifiedAt: now.addingTimeInterval(-6),
                            relativeTime: "now",
                            messageCount: 1,
                            isInit: false
                        )
                    ]
                ),
                WorkspaceGroupSeed(
                    scope: .project("beta"),
                    kind: .project,
                    title: "Beta",
                    subtitle: nil,
                    updatedAt: now.addingTimeInterval(-20),
                    sessions: [
                        WorkspaceSessionRow(
                            id: "beta-chat",
                            title: "Notes",
                            mode: .chat,
                            modifiedAt: now.addingTimeInterval(-10),
                            relativeTime: "now",
                            messageCount: 1,
                            isInit: false
                        )
                    ]
                )
            ]
        )

        XCTAssertEqual(groups.map(\.key), ["project.alpha"])
        XCTAssertEqual(groups.first?.sessions.map(\.id), ["chat-1"])

        let workspaceMatch = WorkspaceGroupBuilder.seededConversationGroups(
            section: .chat,
            query: "Alpha",
            groups: [
                WorkspaceGroupSeed(
                    scope: .project("alpha"),
                    kind: .project,
                    title: "Alpha",
                    subtitle: nil,
                    updatedAt: now,
                    sessions: [
                        WorkspaceSessionRow(
                            id: "chat-1",
                            title: "Review spec",
                            mode: .chat,
                            modifiedAt: now,
                            relativeTime: "now",
                            messageCount: 3,
                            isInit: false
                        ),
                        WorkspaceSessionRow(
                            id: "chat-2",
                            title: "Unrelated notes",
                            mode: .chat,
                            modifiedAt: now.addingTimeInterval(-1),
                            relativeTime: "now",
                            messageCount: 1,
                            isInit: false
                        )
                    ]
                )
            ]
        )
        XCTAssertEqual(workspaceMatch.first?.sessions.map(\.id), ["chat-1", "chat-2"])
    }
}
