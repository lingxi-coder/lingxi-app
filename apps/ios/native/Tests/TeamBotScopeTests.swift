import Foundation
import XCTest
@testable import LingxiCode

/// What a shared Bot may use of its maintainer's computer in a Chat (Bot v3 B4c): the effective-scope cases that Web, Android and iOS share
/// (`packages/teams/contracts/bot-scope-vectors.json`), the commands the service takes (`execution-vectors.json`, run by `TeamBotTests`), and what is
/// native to this reader: a card and a Chat as the service writes them, what a person's ticks say, who can limit which Bots, and what is sent.
final class TeamBotScopeTests: XCTestCase {
    private let calendar = #"{"id":"calendar","name":"Calendar"}"#, notes = #"{"id":"notes","name":"Notes"}"#
    private let orders = #"{"source":"orders","dataset":"orders-2026","write":true,"reportReview":false}"#, leads = #"{"source":"orders","dataset":"leads","write":false,"reportReview":false}"#

    private func published(_ json: String) throws -> TeamBotScope { try JSONDecoder().decode(TeamBotScope.self, from: Data(json.utf8)) }
    private var both: String { #"{"localApps":[\#(calendar),\#(notes)],"datasets":[\#(orders),\#(leads)]}"# }

    private func card(_ id: String = "sharedbot_01", scope: String? = nil) throws -> TeamBotCard {
        let extra = scope.map { #","scope":\#($0)"# } ?? ""
        let json = #"{"sharedBotId":"\#(id)","name":"Helper","description":"","maintainer":{"uid":"owner","generation":3},"revision":2,"state":"published","permitted":true,"ready":true,"onlineUntil":0,"usePolicy":{"mode":"all"},"quotas":{"team":1,"member":1,"bot":1}\#(extra)}"#
        return try JSONDecoder().decode(TeamBotCard.self, from: Data(json.utf8))
    }

    private func chat(kind: String = "group", participants: [String] = ["user:alice", "user:bob", "shared_bot:sharedbot_01"], botScopes: Any? = nil) throws -> TeamMessageConversation {
        var data: [String: Any] = ["kind": kind, "title": "Trip", "participants": participants, "lastSeq": 3, "lastChangeSeq": 3, "lastMessage": NSNull()]
        if let botScopes { data["botScopes"] = botScopes }
        return try TeamsPointerParser.parseConversation(id: "chat_0001", data: data, lastReadSeq: 0, millis: { _ in nil })
    }

    private func vectors(file: String = #filePath) throws -> [[String: Any]] {
        var path = URL(fileURLWithPath: file)
        while !FileManager.default.fileExists(atPath: path.appendingPathComponent("packages/teams/contracts/bot-scope-vectors.json").path), path.path != "/" { path.deleteLastPathComponent() }
        let fixture = Bundle(for: Self.self).url(forResource: "bot-scope-vectors", withExtension: "json") ?? path.appendingPathComponent("packages/teams/contracts/bot-scope-vectors.json")
        let value = try XCTUnwrap(try JSONSerialization.jsonObject(with: Data(contentsOf: fixture)) as? [String: Any])
        return try XCTUnwrap(value["cases"] as? [[String: Any]])
    }

    // MARK: The cases the platforms share

    func testTheEffectiveScopeVectors() throws {
        let rows = try vectors()
        XCTAssertGreaterThanOrEqual(rows.count, 8)
        for row in rows {
            let name = row["name"] as? String ?? ""
            let have = try (row["published"] as? [String: Any]).map { try published(String(decoding: JSONSerialization.data(withJSONObject: $0), as: UTF8.self)) }
            let entry = (row["narrowing"] as? [String: Any]).flatMap { TeamBotScopeRules.decode(["bot": $0])?["bot"] }
            let expected = try XCTUnwrap(row["effective"] as? [String: Any], name)
            XCTAssertEqual(TeamBotScopeRules.effective(have, entry?.narrowing),
                           try published(String(decoding: JSONSerialization.data(withJSONObject: expected), as: UTF8.self)), name)
        }
    }

    // MARK: What the service writes

    func testACardCarriesWhatItsMaintainerPublished() throws {
        XCTAssertNil(try card().scope, "a card has a scope only when it is not empty")
        let shared = try card(scope: both).scope
        XCTAssertEqual(shared?.localApps.map(\.name), ["Calendar", "Notes"])
        XCTAssertEqual(shared?.datasets, [TeamBotScopeDataset(source: "orders", dataset: "orders-2026", write: true, reportReview: false),
                                          TeamBotScopeDataset(source: "orders", dataset: "leads", write: false, reportReview: false)])
        // A list that is left out is empty; a scope with neither list is the empty scope.
        XCTAssertEqual(try card(scope: #"{"localApps":[\#(notes)]}"#).scope?.datasets, [])
        XCTAssertEqual(try card(scope: #"{"datasets":[\#(leads)]}"#).scope?.localApps, [])
        XCTAssertEqual(try card(scope: "{}").scope, TeamBotScope.empty)
        // A list that is there and unreadable breaks the card, as any other field of it does.
        XCTAssertThrowsError(try card(scope: #"{"localApps":"calendar"}"#))
    }

    func testAChatCarriesWhatItNarrowed() throws {
        XCTAssertNil(try chat().botScopes, "a Chat that never narrowed has none")
        XCTAssertNil(try chat(botScopes: [String: Any]()).botScopes)
        XCTAssertNil(try chat(botScopes: "narrowed").botScopes, "what is not a map is no narrowing, and not a reason to hide the Chat")
        let stored: [String: Any] = [
            "sharedbot_01": ["revision": 2, "localApps": ["notes"], "datasets": [["source": "orders", "dataset": "leads"]]] as [String: Any],
            "sharedbot_02": ["revision": 5],
            "sharedbot_03": ["revision": 1, "localApps": [String](), "datasets": [[String: Any]]()] as [String: Any],
        ]
        let held = try XCTUnwrap(chat(botScopes: stored).botScopes)
        XCTAssertEqual(held["sharedbot_01"], TeamBotScopeEntry(revision: 2, localApps: ["notes"], datasets: [TeamBotScopeDatasetRef(source: "orders", dataset: "leads")]))
        XCTAssertEqual(held["sharedbot_02"], TeamBotScopeEntry(revision: 5), "a limit that was lifted keeps only its revision")
        XCTAssertEqual(held["sharedbot_03"], TeamBotScopeEntry(revision: 1, localApps: [], datasets: []), "an empty list is not a list that is left out")
        XCTAssertTrue(TeamBotScopeRules.limits(held["sharedbot_01"]))
        XCTAssertTrue(TeamBotScopeRules.limits(held["sharedbot_03"]))
        XCTAssertFalse(TeamBotScopeRules.limits(held["sharedbot_02"]))
        XCTAssertFalse(TeamBotScopeRules.limits(nil))
    }

    func testAnEntryThatIsNotWellFormedIsLeftOut() throws {
        let stored: [String: Any] = [
            "revision_zero": ["revision": 0, "localApps": ["notes"]] as [String: Any],
            "no_revision": ["localApps": ["notes"]],
            "fraction": ["revision": 1.5],
            "flag": ["revision": true],
            "apps_not_a_list": ["revision": 1, "localApps": "notes"] as [String: Any],
            "app_not_text": ["revision": 1, "localApps": ["notes", 4] as [Any]] as [String: Any],
            "dataset_without_id": ["revision": 1, "datasets": [["source": "orders"]]] as [String: Any],
            "not_an_entry": "yes",
            "kept": ["revision": 1, "stray": true, "localApps": ["notes"]] as [String: Any],
        ]
        let read = try XCTUnwrap(chat(botScopes: stored).botScopes)
        XCTAssertEqual(Array(read.keys), ["kept"])
        XCTAssertEqual(read["kept"], TeamBotScopeEntry(revision: 1, localApps: ["notes"]), "a key the entry has no use for is dropped")
        XCTAssertNil(try chat(botScopes: ["revision_zero": ["revision": 0]] as [String: Any]).botScopes, "a Chat whose every entry is left out has none")
    }

    // MARK: What the Bot may use here

    func testWhatABotMayUseInAChatIsWhatItPublishedCutDownToWhatTheChatAllows() throws {
        let sharing = try card(scope: both)
        let free = try chat()
        XCTAssertEqual(TeamBotScopeRules.effective(of: sharing, in: nil), try published(both))
        XCTAssertEqual(TeamBotScopeRules.effective(of: sharing, in: free), try published(both))
        let narrowed = try chat(botScopes: ["sharedbot_01": ["revision": 1, "localApps": ["notes"], "datasets": [["source": "orders", "dataset": "leads"]]] as [String: Any]] as [String: Any])
        let effective = TeamBotScopeRules.effective(of: sharing, in: narrowed)
        XCTAssertEqual(effective, try published(#"{"localApps":[\#(notes)],"datasets":[\#(leads)]}"#))
        XCTAssertTrue(TeamBotScopeRules.narrowed(sharing.scope, to: effective))
        XCTAssertFalse(TeamBotScopeRules.narrowed(sharing.scope, to: TeamBotScopeRules.effective(of: sharing, in: free)))
        // Another Bot's entry is not this Bot's.
        let other = try chat(botScopes: ["sharedbot_02": ["revision": 1, "localApps": [String]()] as [String: Any]] as [String: Any])
        XCTAssertEqual(TeamBotScopeRules.effective(of: sharing, in: other), try published(both))
        // A limit that was lifted limits nothing; one that allows none says that the Bot uses nothing here.
        XCTAssertEqual(TeamBotScopeRules.effective(of: sharing, in: try chat(botScopes: ["sharedbot_01": ["revision": 3]] as [String: Any])), try published(both))
        let nothing: [String: Any] = ["sharedbot_01": ["revision": 2, "localApps": [String](), "datasets": [[String: Any]]()] as [String: Any]]
        let none = TeamBotScopeRules.effective(of: sharing, in: try chat(botScopes: nothing))
        XCTAssertTrue(none.isEmpty)
        XCTAssertTrue(TeamBotScopeRules.narrowed(sharing.scope, to: none))
        // A Bot published without a scope has none, whatever the Chat narrowed; and then there is nothing to be limited.
        let plain = try card("sharedbot_02")
        let plainEffective = TeamBotScopeRules.effective(of: plain, in: other)
        XCTAssertTrue(plainEffective.isEmpty)
        XCTAssertFalse(TeamBotScopeRules.narrowed(plain.scope, to: plainEffective))
    }

    // MARK: What is ticked

    func testWhatIsTickedBecomesTheNarrowing() throws {
        let have = try published(both)
        let all = TeamBotScopeRules.choice(of: have)
        XCTAssertEqual(all, TeamBotScopeChoice(localApps: ["calendar", "notes"], datasets: ["orders-2026", "leads"]))
        XCTAssertNil(TeamBotScopeRules.narrowing(of: all, in: have), "everything allowed lifts the limit")
        XCTAssertEqual(TeamBotScopeRules.narrowing(of: TeamBotScopeChoice(localApps: ["calendar"], datasets: all.datasets), in: have),
                       TeamBotScopeNarrowing(localApps: ["calendar"], datasets: nil), "a kind with nothing left out is not narrowed, so what is published of it later is allowed too")
        XCTAssertEqual(TeamBotScopeRules.narrowing(of: TeamBotScopeChoice(localApps: all.localApps, datasets: ["leads"]), in: have),
                       TeamBotScopeNarrowing(localApps: nil, datasets: [TeamBotScopeDatasetRef(source: "orders", dataset: "leads")]))
        XCTAssertEqual(TeamBotScopeRules.narrowing(of: TeamBotScopeChoice(), in: have), TeamBotScopeNarrowing(localApps: [], datasets: []),
                       "nothing allowed is said as empty lists, not as no narrowing")
        XCTAssertEqual(TeamBotScopeRules.narrowing(of: TeamBotScopeChoice(localApps: ["notes", "mail"], datasets: ["orders-2026", "removed", "leads"]), in: have),
                       TeamBotScopeNarrowing(localApps: ["notes"], datasets: nil), "what is not published is never named")
        let appsOnly = try published(#"{"localApps":[\#(calendar)]}"#)
        XCTAssertEqual(TeamBotScopeRules.narrowing(of: TeamBotScopeChoice(), in: appsOnly), TeamBotScopeNarrowing(localApps: [], datasets: nil), "a kind with nothing published is left out")
        XCTAssertNil(TeamBotScopeRules.narrowing(of: TeamBotScopeChoice(), in: .empty))
        // Whatever is ticked is what the service then lets the Bot use here.
        for apps in [[], ["calendar"], ["notes"], ["calendar", "notes"]] as [[String]] {
            for data in [[], ["orders-2026"], ["leads"], ["orders-2026", "leads"]] as [[String]] {
                let narrowing = TeamBotScopeRules.narrowing(of: TeamBotScopeChoice(localApps: apps, datasets: data), in: have)
                let effective = TeamBotScopeRules.effective(have, narrowing)
                XCTAssertEqual(TeamBotScopeRules.choice(of: effective),
                               TeamBotScopeChoice(localApps: have.localApps.map(\.id).filter(apps.contains), datasets: have.datasets.map(\.dataset).filter(data.contains)), "\(apps) \(data)")
            }
        }
    }

    func testTicksAreToggledWithoutDuplicatesAndComparedWithoutOrder() {
        XCTAssertEqual(TeamBotScopeRules.toggled(["a", "b"], "c", on: true), ["a", "b", "c"])
        XCTAssertEqual(TeamBotScopeRules.toggled(["a", "b"], "a", on: true), ["b", "a"], "ticking a ticked box leaves it once")
        XCTAssertEqual(TeamBotScopeRules.toggled(["a", "b"], "a", on: false), ["b"])
        XCTAssertEqual(TeamBotScopeRules.toggled(["a"], "z", on: false), ["a"])
        XCTAssertTrue(TeamBotScopeRules.same(TeamBotScopeChoice(localApps: ["a", "b"], datasets: ["x"]), TeamBotScopeChoice(localApps: ["b", "a"], datasets: ["x"])))
        XCTAssertFalse(TeamBotScopeRules.same(TeamBotScopeChoice(localApps: ["a"], datasets: []), TeamBotScopeChoice(localApps: ["a"], datasets: ["x"])))
        XCTAssertFalse(TeamBotScopeRules.same(TeamBotScopeChoice(localApps: ["a", "a"], datasets: []), TeamBotScopeChoice(localApps: ["a"], datasets: [])))
    }

    func testTheTicksStartAgainWhenWhatTheyWereTickedAgainstChanges() throws {
        let have = try published(both)
        let first = TeamBotScopeBasis(published: have, entry: nil)
        XCTAssertEqual(first.revision, 0)
        XCTAssertEqual(first.allowed, TeamBotScopeRules.choice(of: have))
        XCTAssertEqual(first, TeamBotScopeBasis(published: have, entry: nil), "nothing moved")
        let saved = TeamBotScopeBasis(published: have, entry: TeamBotScopeEntry(revision: 1, localApps: ["notes"]))
        XCTAssertNotEqual(first, saved, "a save, or someone else's")
        XCTAssertEqual(saved.allowed, TeamBotScopeChoice(localApps: ["notes"], datasets: ["orders-2026", "leads"]))
        XCTAssertNotEqual(saved, TeamBotScopeBasis(published: have, entry: TeamBotScopeEntry(revision: 2, localApps: ["notes"])), "the revision alone moves it")
        XCTAssertNotEqual(saved, TeamBotScopeBasis(published: try published(#"{"localApps":[\#(calendar),\#(notes)],"datasets":[\#(leads)]}"#), entry: TeamBotScopeEntry(revision: 1, localApps: ["notes"])),
                          "the maintainer published something else")
        XCTAssertEqual(TeamBotScopeBasis(published: nil, entry: nil).allowed, TeamBotScopeChoice())
    }

    // MARK: Who can limit what

    func testOnlyAPersonInABotChatCanLimitTheBotsThatShareSomething() throws {
        let sharing = try card(scope: both), plain = try card("sharedbot_02"), emptied = try card("sharedbot_03", scope: "{}")
        let cards = [sharing, plain, emptied]
        let all = ["user:alice", "user:bob", "shared_bot:sharedbot_01", "shared_bot:sharedbot_02", "shared_bot:sharedbot_03"]
        func limitable(_ conversation: TeamMessageConversation, uid: String = "alice") -> [String] {
            TeamBotScopeRules.limitable(conversation, cards: cards, uid: uid).map(\.sharedBotId)
        }
        XCTAssertEqual(limitable(try chat(participants: all)), ["sharedbot_01"], "a Bot that shares nothing has nothing to limit")
        XCTAssertEqual(limitable(try chat(kind: "channel", participants: all)), ["sharedbot_01"])
        XCTAssertEqual(limitable(try chat(kind: "bot_dm", participants: ["user:alice", "shared_bot:sharedbot_01"])), ["sharedbot_01"])
        XCTAssertEqual(limitable(try chat(participants: all), uid: "carol"), [], "only a person who takes part in the Chat limits it")
        XCTAssertEqual(limitable(try chat(participants: ["user:alice", "user:bob", "shared_bot:sharedbot_02"])), [], "a Bot that is not in the Chat is not limited by it")
        XCTAssertEqual(limitable(try chat(kind: "dm", participants: ["user:alice", "user:bob"])), [], "a chat with no Bots")
        XCTAssertTrue(TeamBotScopeRules.inBotChat(try chat(), uid: "bob"))
        XCTAssertFalse(TeamBotScopeRules.inBotChat(try chat(kind: "dm", participants: ["user:alice", "shared_bot:sharedbot_01"]), uid: "alice"))
    }

    // MARK: What is sent

    private func command(_ payload: [String: Any]) -> [String: Any] {
        ["schemaVersion": 1, "commandId": "command_0001", "teamId": "team_0001", "generation": 7, "action": "setConversationBotScope", "payload": payload]
    }
    private func send(_ narrowing: TeamBotScopeNarrowing?, expectedRevision: Int = 0) -> [String: Any] {
        command(["conversationId": "chat_0001", "sharedBotId": "sharedbot_01", "expectedRevision": expectedRevision, "scope": narrowing?.payload ?? NSNull()])
    }

    func testWhatTheEditorSendsIsWhatTheServiceAccepts() throws {
        XCTAssertNoThrow(try TeamExecutionContract.validate(send(nil)), "the lifted narrowing is a null scope")
        XCTAssertNoThrow(try TeamExecutionContract.validate(send(TeamBotScopeNarrowing(localApps: [], datasets: []), expectedRevision: 3)))
        XCTAssertNoThrow(try TeamExecutionContract.validate(send(TeamBotScopeNarrowing(localApps: ["calendar", "mail.app:1"], datasets: nil), expectedRevision: 1)))
        XCTAssertNoThrow(try TeamExecutionContract.validate(send(TeamBotScopeNarrowing(localApps: nil, datasets: [TeamBotScopeDatasetRef(source: "orders", dataset: "leads")]))))
        XCTAssertEqual(TeamBotScopeNarrowing(localApps: nil, datasets: nil).payload.count, 0)
        let payload = TeamBotScopeNarrowing(localApps: ["calendar"], datasets: [TeamBotScopeDatasetRef(source: "orders", dataset: "leads")]).payload
        XCTAssertEqual(try JSONSerialization.data(withJSONObject: payload, options: .sortedKeys),
                       Data(#"{"datasets":[{"dataset":"leads","source":"orders"}],"localApps":["calendar"]}"#.utf8))
    }

    func testTheCommandIsRefusedLikeTheServiceRefusesIt() {
        func refused(_ payload: [String: Any], _ why: String, file: StaticString = #filePath, line: UInt = #line) {
            XCTAssertThrowsError(try TeamExecutionContract.validate(command(payload)), why, file: file, line: line)
        }
        let base: [String: Any] = ["conversationId": "chat_0001", "sharedBotId": "sharedbot_01", "expectedRevision": 0]
        refused(base, "a scope that is left out is not a lifted narrowing")
        refused(base.merging(["scope": NSNull(), "extra": 1]) { $1 }, "a key it does not know")
        refused(base.merging(["scope": NSNull(), "expectedRevision": -1]) { $1 }, "a revision below zero")
        refused(base.merging(["scope": NSNull(), "expectedRevision": 1.5]) { $1 }, "a revision that is not whole")
        refused(base.merging(["scope": NSNull(), "sharedBotId": ""]) { $1 }, "no Bot")
        refused(base.merging(["scope": ["localApps": ["a", "a"]]]) { $1 }, "a Local App twice")
        refused(base.merging(["scope": ["localApps": ["has space"]]]) { $1 }, "a Local App id with a character the service does not take")
        refused(base.merging(["scope": ["localApps": (0...64).map { "app\($0)" }]]) { $1 }, "more than 64 Local Apps")
        refused(base.merging(["scope": ["datasets": [["source": "orders", "dataset": "a"], ["source": "other", "dataset": "a"]]]]) { $1 }, "a dataset twice, whatever its source")
        refused(base.merging(["scope": ["datasets": [["source": "orders", "dataset": "a", "write": true]]]]) { $1 }, "flags the Chat cannot set")
        refused(base.merging(["scope": ["datasets": [["source": "orders"]]]]) { $1 }, "a dataset without its id")
        refused(base.merging(["scope": ["datasets": ["a"]]]) { $1 }, "a dataset that is not a row")
        refused(base.merging(["scope": ["secrets": ["a"]]]) { $1 }, "a kind it does not know")
        refused(base.merging(["scope": "all"]) { $1 }, "a scope that is not a narrowing")
    }

    /// The rows of `execution-vectors.json` about this command: the verdicts of the service that every client's validator must give too (`TeamBotTests` runs the
    /// whole file; this keeps the scope rows in this suite, whatever else that file holds).
    func testTheScopeCommandVectorsAreTheServicesVerdicts() throws {
        var path = URL(fileURLWithPath: #filePath)
        while !FileManager.default.fileExists(atPath: path.appendingPathComponent("packages/teams/contracts/execution-vectors.json").path), path.path != "/" { path.deleteLastPathComponent() }
        let fixture = Bundle(for: Self.self).url(forResource: "execution-vectors", withExtension: "json") ?? path.appendingPathComponent("packages/teams/contracts/execution-vectors.json")
        let value = try XCTUnwrap(try JSONSerialization.jsonObject(with: Data(contentsOf: fixture)) as? [String: Any])
        let rows = try XCTUnwrap(value["commands"] as? [[String: Any]]).filter { ($0["input"] as? [String: Any])?["action"] as? String == "setConversationBotScope" }
        XCTAssertGreaterThanOrEqual(rows.count, 8)
        XCTAssertTrue(rows.contains { $0["valid"] as? Bool == true } && rows.contains { $0["valid"] as? Bool == false })
        for row in rows {
            let input = try XCTUnwrap(row["input"] as? [String: Any])
            if row["valid"] as? Bool == true { XCTAssertNoThrow(try TeamExecutionContract.validate(input), "\(row["name"] ?? "")") }
            else { XCTAssertThrowsError(try TeamExecutionContract.validate(input), "\(row["name"] ?? "")") }
        }
    }

    func testTheFailuresOfAScopeChangeAreToldAsTheyAre() throws {
        XCTAssertEqual(TeamsErrors.key("BOT_SCOPE_REVISION_CONFLICT"), "teams_error_scope_conflict")
        XCTAssertEqual(TeamsErrors.key("SCOPE_NOT_PUBLISHED"), "teams_error_scope_not_published")
        let catalogue = try Catalogue.load()
        for key in ["teams_error_scope_conflict", "teams_error_scope_not_published", "teams_bot_scope_title", "teams_bot_scope_note", "teams_bot_scope_limit_title %@",
                    "teams_bot_scope_apps", "teams_bot_scope_datasets", "teams_bot_scope_edit", "teams_bot_scope_uses", "teams_bot_scope_none", "teams_bot_scope_limited",
                    "teams_bot_scope_reset", "teams_bot_scope_saved"] {
            for language in ["en", "zh-Hans", "zh-Hant", "ja", "ko"] {
                XCTAssertFalse(catalogue.value(key, language)?.isEmpty ?? true, "\(key) has no \(language) text")
            }
        }
        XCTAssertNotEqual(catalogue.value("teams_error_scope_conflict", "zh-Hant"), catalogue.value("teams_error_scope_conflict", "zh-Hans"), "zh-Hant is its own text")
        XCTAssertNotEqual(catalogue.value("teams_error_scope_not_published", "zh-Hant"), catalogue.value("teams_error_scope_not_published", "zh-Hans"))
    }
}
