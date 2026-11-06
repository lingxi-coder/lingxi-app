import XCTest
@testable import LingxiCode

final class TeamsTests: XCTestCase {
    func testInvitationPreservesCaseAndRejectsAmbiguousLinks() {
        let code = "AbCdEfGhIjKlMnOpQrStUvWxYz_12345"
        for raw in [TeamInvitation.link(code), "lingxi://team/join?code=\(code)"] {
            XCTAssertEqual(TeamInvitation.code(raw), code)
            XCTAssertEqual(CanonicalDeepLink.parse(raw)?.route, "team/join")
            XCTAssertEqual(CanonicalDeepLink.parse(raw)?.params["code"], code)
            XCTAssertFalse(CanonicalDeepLink.parse(raw)!.needsContextRebind(workspace: "project.other", mode: "code"))
        }
        for raw in ["lingxi://team/join", "lingxi://team/join?code=short", "lingxi://team/join?code=\(code)&code=\(code)",
                    "lingxi://team/join?code=\(code)&workspace=secret", "lingxi://team/join?code=\(code)#payload=x",
                    "https://user:password@lingxios.net/open/team/join?code=\(code)"] {
            XCTAssertNil(TeamInvitation.code(raw), raw)
            XCTAssertNil(CanonicalDeepLink.parse(raw), raw)
        }
    }

    // MARK: Errors

    func testFailureCodesMapToTheKeysTheUIShows() throws {
        let table: [(codes: [String], key: String)] = [
            (["INVALID_DATA", "INVALID_MESSAGE_SNAPSHOT"], "teams_error_invalid_data"),
            (["MEMBERSHIP_REQUIRED", "STALE_MEMBERSHIP", "INVALID_SESSION", "SIGN_IN_REQUIRED"], "teams_stale_error"),
            (["BOT_USE_REQUIRED", "INVALID_BOT_TARGET", "BOT_PARTICIPANT_REQUIRED", "BOT_SELECTION_STALE"], "teams_error_bot_access_changed"),
            (["REQUEST_QUOTA_EXCEEDED"], "teams_error_request_quota"),
            (["MESSAGE_LISTENER_FAILED"], "teams_error_sync_failed"),
            (["MESSAGE_RECALLED", "MESSAGE_ID_CONFLICT", "MESSAGE_AUTHOR_REQUIRED", "PARTICIPANT_REQUIRED", "CONVERSATION_REJECTED", "INVALID_INPUT",
              "BOT_REVISION_CONFLICT", "MAINTAINER_REQUIRED", "MESSAGE_IN_FLIGHT", "REPLY_REQUIRED"], "teams_message_failed"),
            (["INVITE_UNAVAILABLE"], "teams_invite_error"),
            (["STORAGE_FAILED", "MESSAGE_STORAGE_FULL"], "teams_messages_storage_error"),
            (["SERVICE_UNAVAILABLE", "UNKNOWN", "SOMETHING_NEW", ""], "teams_error"),
        ]
        let catalogue = try Catalogue.load()
        for (codes, key) in table {
            for code in codes { XCTAssertEqual(TeamsErrors.key(code), key, code) }
            XCTAssertNotNil(catalogue.value(key, "en"), "\(key) has no English text")
            XCTAssertNotNil(catalogue.value(key, "zh-Hans"), "\(key) has no Chinese text")
        }
    }

    func testOnlyMembershipFailuresMeanTheTeamIsGone() {
        for code in ["MEMBERSHIP_REQUIRED", "STALE_MEMBERSHIP", "INVALID_SESSION", "SIGN_IN_REQUIRED", "CONVERSATION_REQUIRED"] {
            XCTAssertTrue(TeamsErrors.isMembershipLoss(code), code)
        }
        for code in ["SERVICE_UNAVAILABLE", "BOT_USE_REQUIRED", "INVALID_DATA", "MESSAGE_LISTENER_FAILED", ""] {
            XCTAssertFalse(TeamsErrors.isMembershipLoss(code), code)
        }
    }

    // MARK: Words

    func testTaskLabelsCoverOnlyExecutionTwoStatesAndExistInBothLanguages() throws {
        let states: [String: String] = [
            "queued": "cron_status_queued", "running": "teams_run_processing", "completed": "chat_status_completed", "failed": "chat_status_failed",
            "expired": "teams_task_expired", "interrupted": "teams_task_interrupted", "authorization_required": "teams_task_authorization_required", "revoked": "teams_run_revoked",
        ]
        for (state, key) in states { XCTAssertEqual(TeamsBotText.stateKey(state), key, state) }
        for retired in ["pending", "claimed", "recovery", "recovered", "missed", "cancelled"] { XCTAssertEqual(TeamsBotText.stateKey(retired), "chat_status_unknown") }
        XCTAssertEqual(Set(TeamsBotText.allKeys), Set(states.values).union(["chat_status_unknown"]))
        let catalogue = try Catalogue.load()
        for key in TeamsBotText.allKeys {
            XCTAssertFalse((catalogue.value(key, "en") ?? "").isEmpty, "\(key) has no English text")
            XCTAssertFalse((catalogue.value(key, "zh-Hans") ?? "").isEmpty, "\(key) has no Chinese text")
        }
    }

    /// The fixed word sets the vector tests use are the catalogue's words, so a vector row that passes also
    /// reads right on screen.
    func testListVocabularyIsTheCatalogue() throws {
        let catalogue = try Catalogue.load()
        let words: [(key: String, en: String, zh: String)] = [
            ("teams_conversation_channel", TeamsListStrings.en.channel, TeamsListStrings.zh.channel),
            ("teams_label_direct", TeamsListStrings.en.directMessage, TeamsListStrings.zh.directMessage),
            ("teams_conversation_group", TeamsListStrings.en.group, TeamsListStrings.zh.group),
            ("teams_member", TeamsListStrings.en.member, TeamsListStrings.zh.member),
            ("teams_preview_you", TeamsListStrings.en.you, TeamsListStrings.zh.you),
            ("teams_preview_none", TeamsListStrings.en.noMessages, TeamsListStrings.zh.noMessages),
            ("teams_message_recalled", TeamsListStrings.en.recalled, TeamsListStrings.zh.recalled),
            ("teams_preview_non_text", TeamsListStrings.en.nonText, TeamsListStrings.zh.nonText),
        ]
        for word in words {
            XCTAssertEqual(catalogue.value(word.key, "en"), word.en, word.key)
            XCTAssertEqual(catalogue.value(word.key, "zh-Hans"), word.zh, word.key)
        }
        XCTAssertEqual(TeamsListStrings.en.bot, "Bot")
        XCTAssertEqual(TeamsListStrings.zh.bot, "Bot", "The Bot is called Bot in both languages")
        XCTAssertTrue(TeamsListStrings.isChinese(languageTag: "zh-Hans"))
        XCTAssertTrue(TeamsListStrings.isChinese(languageTag: "zh-Hant-TW"))
        XCTAssertFalse(TeamsListStrings.isChinese(languageTag: "en"))
    }

    // MARK: Memory

    func testTheSelectedTeamAndCollapsedSectionsAreKeptPerAccountAndTeam() {
        let suite = "teams-preferences-" + UUID().uuidString
        let defaults = UserDefaults(suiteName: suite)!
        defer { defaults.removePersistentDomain(forName: suite) }
        let preferences = TeamsPreferences(defaults: defaults)
        XCTAssertNil(preferences.selectedTeam(uid: "alice"))
        preferences.setSelectedTeam("team-1", uid: "alice")
        XCTAssertEqual(preferences.selectedTeam(uid: "alice"), "team-1")
        XCTAssertNil(preferences.selectedTeam(uid: "bob"), "Another account starts with nothing chosen")
        XCTAssertEqual(defaults.string(forKey: "teams.selected.alice"), "team-1", "The key is shared with Android's")
        preferences.setSelectedTeam("", uid: "alice")
        XCTAssertNil(preferences.selectedTeam(uid: "alice"), "An empty choice is no choice")

        XCTAssertTrue(preferences.collapsed(uid: "alice", teamID: "team-1").isEmpty)
        preferences.setCollapsed([.direct, .channels], uid: "alice", teamID: "team-1")
        XCTAssertEqual(preferences.collapsed(uid: "alice", teamID: "team-1"), [.channels, .direct])
        XCTAssertEqual(defaults.stringArray(forKey: "teams.collapsed.alice.team-1"), ["channels", "direct"])
        XCTAssertTrue(preferences.collapsed(uid: "alice", teamID: "team-2").isEmpty)
        XCTAssertTrue(preferences.collapsed(uid: "bob", teamID: "team-1").isEmpty)
        defaults.set(["bots", "nonsense"], forKey: "teams.collapsed.alice.team-3")
        XCTAssertEqual(preferences.collapsed(uid: "alice", teamID: "team-3"), [.bots], "Values that are not sections are ignored")
        preferences.setCollapsed([], uid: "alice", teamID: "team-1")
        XCTAssertNil(defaults.object(forKey: "teams.collapsed.alice.team-1"), "Expanding everything forgets the entry")
        XCTAssertEqual(TeamsSection.allCases.map(\.rawValue), ["channels", "direct", "bots"])
    }

    // MARK: Roles

    func testRolesDecideWhatTheTeamDetailsOffer() {
        func manage(_ viewer: String, _ member: String, me: String = "me", other: String = "them") -> Bool {
            TeamRoles.canManage(viewerRole: viewer, viewerUID: me, memberUID: other, memberRole: member)
        }
        XCTAssertTrue(manage("owner", "admin"))
        XCTAssertTrue(manage("owner", "member"))
        XCTAssertFalse(manage("owner", "owner"), "Nobody manages an owner")
        XCTAssertFalse(manage("owner", "member", other: "me"), "Nobody manages themselves here")
        XCTAssertTrue(manage("admin", "member"))
        XCTAssertFalse(manage("admin", "admin"))
        XCTAssertFalse(manage("admin", "owner"))
        XCTAssertFalse(manage("member", "member"))
        XCTAssertFalse(manage("member", "admin"))

        XCTAssertTrue(TeamRoles.canInvite(role: "owner"))
        XCTAssertTrue(TeamRoles.canInvite(role: "admin"))
        XCTAssertFalse(TeamRoles.canInvite(role: "member"), "Plain members have no Invites tab")
        XCTAssertTrue(TeamRoles.canRevoke(viewerRole: "owner", inviteRole: "admin"))
        XCTAssertTrue(TeamRoles.canRevoke(viewerRole: "admin", inviteRole: "member"))
        XCTAssertFalse(TeamRoles.canRevoke(viewerRole: "admin", inviteRole: "admin"), "Only an owner revokes an invitation for admins")
    }

    func testTeamListAndDetailDecodeStrictly() throws {
        let raw = """
        {"team":{"id":"team_0001","name":"Core","memberCount":2,"role":"owner","generation":3},
         "members":[{"uid":"alice","displayName":"Alice","role":"owner","generation":1},{"uid":"bob","displayName":"Bob","role":"member","generation":2}],
         "invites":[{"id":"invite_1","role":"member","expiresAt":"2026-11-05T10:00:00.000Z","uses":0,"maxUses":5,"revoked":false}]}
        """
        let detail = try JSONDecoder().decode(TeamDetail.self, from: Data(raw.utf8))
        XCTAssertEqual(detail.team.generation, 3)
        XCTAssertEqual(detail.members.map(\.uid), ["alice", "bob"])
        XCTAssertEqual(detail.invites.first?.maxUses, 5)
        XCTAssertEqual(TeamsStreamLogic.parseInstant(detail.invites.first?.expiresAt), 1_793_872_800_000, "The invitation expiry is an instant")
        let noGeneration = raw.replacingOccurrences(of: ",\"generation\":3", with: "")
        XCTAssertThrowsError(try JSONDecoder().decode(TeamDetail.self, from: Data(noGeneration.utf8)), "A team without its generation is invalid data")
    }
}

/// `Resources/Localizable.xcstrings`, found by walking up from the test file (the repository and the macOS
/// runner's scratch package both have it).
struct Catalogue {
    let strings: [String: Any]

    static func load(file: String = #filePath) throws -> Catalogue {
        var directory = URL(fileURLWithPath: file).deletingLastPathComponent()
        for _ in 0..<10 {
            let candidate = directory.appendingPathComponent("Resources/Localizable.xcstrings")
            if FileManager.default.fileExists(atPath: candidate.path) {
                let object = try JSONSerialization.jsonObject(with: Data(contentsOf: candidate)) as? [String: Any]
                return Catalogue(strings: try XCTUnwrap(object?["strings"] as? [String: Any]))
            }
            let parent = directory.deletingLastPathComponent()
            if parent.path == directory.path { break }
            directory = parent
        }
        throw XCTSkip("Resources/Localizable.xcstrings is not above \(file)")
    }

    /// The text of a key in a language, `nil` when it has none.
    func value(_ key: String, _ language: String) -> String? {
        let entry = strings[key] as? [String: Any]
        let localizations = entry?["localizations"] as? [String: Any]
        let unit = (localizations?[language] as? [String: Any])?["stringUnit"] as? [String: Any]
        return unit?["value"] as? String
    }
}
