import Foundation
import XCTest
@testable import LingxiCode

final class TeamBotTests: XCTestCase {
    private let sharedBotId = "sharedbot_01"
    private func command(_ action: String = "submit") -> [String: Any] {
        ["schemaVersion": 1, "commandId": "command_0001", "teamId": "team_0001", "generation": 7, "action": action,
         "payload": ["conversationId": "chat_0001", "messageId": "message_0001", "changeId": "change_0001", "text": "Hello", "targets": [sharedBotId]]]
    }
    func testSharedCommandVectors() throws {
        var path = URL(fileURLWithPath: #filePath)
        while !FileManager.default.fileExists(atPath: path.appendingPathComponent("packages/teams/contracts/execution-vectors.json").path), path.path != "/" { path.deleteLastPathComponent() }
        let fixture = Bundle(for: Self.self).url(forResource: "execution-vectors", withExtension: "json") ?? path.appendingPathComponent("packages/teams/contracts/execution-vectors.json")
        let value = try XCTUnwrap(try JSONSerialization.jsonObject(with: Data(contentsOf: fixture)) as? [String: Any])
        for row in try XCTUnwrap(value["commands"] as? [[String: Any]]) {
            let input = try XCTUnwrap(row["input"] as? [String: Any])
            if row["valid"] as? Bool == true { XCTAssertNoThrow(try TeamExecutionContract.validate(input), "\(row["name"] ?? "")") }
            else { XCTAssertThrowsError(try TeamExecutionContract.validate(input), "\(row["name"] ?? "")") }
        }
    }
    private func card(_ extra: String = "", permitted: Bool = true, state: String = "published") throws -> TeamBotCard {
        let json = #"{"sharedBotId":"sharedbot_01","name":"Helper","description":"","maintainer":{"uid":"owner","generation":3},"revision":2,"state":"\#(state)","permitted":\#(permitted),"ready":true,"onlineUntil":0,"usePolicy":{"mode":"all"},"quotas":{"team":1,"member":1,"bot":1}\#(extra)}"#
        return try JSONDecoder().decode(TeamBotCard.self, from: Data(json.utf8))
    }
    func testTheListIsOfferedOnlyToThoseWhoMayReadIt() throws {
        let member = TeamMemberRef(uid: "member", generation: 3), owner = TeamMemberRef(uid: "owner", generation: 3)
        let listed = #","sharedList":{"title":"Groceries","approval":"requester"}"#
        XCTAssertNil(try card().sharedList)
        XCTAssertEqual(try card(listed).sharedList, TeamBotListInfo(title: "Groceries", approval: "requester"))
        XCTAssertFalse(TeamSharedListRules.canOpen(try card(), viewer: member))
        XCTAssertTrue(TeamSharedListRules.canOpen(try card(listed), viewer: member))
        // Not allowed to use the Bot: only its maintainer still reads the list.
        XCTAssertFalse(TeamSharedListRules.canOpen(try card(listed, permitted: false), viewer: member))
        XCTAssertTrue(TeamSharedListRules.canOpen(try card(listed, permitted: false), viewer: owner))
        XCTAssertFalse(TeamSharedListRules.canOpen(try card(listed, state: "paused"), viewer: owner))
    }
    func testARepeatedListSaveKeepsItsOperationAndAnyOtherChangeGetsANewOne() {
        var made = 0
        let id = { made += 1; return "operation_000\(made)" }
        let first = TeamSharedListOperation.of(nil, revision: 4, changes: [.add(text: "Bread")], newId: id)
        XCTAssertEqual(first.id, "operation_0001")
        // The answer was lost and the person saves again: the same request.
        XCTAssertEqual(TeamSharedListOperation.of(first, revision: 4, changes: [.add(text: "Bread")], newId: id), first)
        // Other text, or the same text on a newer list, is another change.
        XCTAssertEqual(TeamSharedListOperation.of(first, revision: 4, changes: [.add(text: "Milk")], newId: id).id, "operation_0002")
        XCTAssertEqual(TeamSharedListOperation.of(first, revision: 5, changes: [.add(text: "Bread")], newId: id).id, "operation_0003")
    }
    func testEveryListChangeIsACommandTheServiceAccepts() throws {
        let changes: [TeamSharedListChange] = [.add(text: "Bread"), .setText(itemId: "item_0001", text: "Rye bread"), .setDone(itemId: "item_0001", done: true), .remove(itemId: "item_0002")]
        var body = command("writeList")
        body["payload"] = ["sharedBotId": sharedBotId, "operationId": "operation_0001", "expectedDataRevision": 0, "changes": changes.map(\.payload)]
        XCTAssertNoThrow(try TeamExecutionContract.validate(body))
        XCTAssertTrue(TeamSharedListRules.usable(" Bread "))
        XCTAssertFalse(TeamSharedListRules.usable("  \n"))
        // 500 bytes fit; one more does not, and multi-byte text is counted in bytes.
        XCTAssertTrue(TeamSharedListRules.usable(String(repeating: "a", count: 500)))
        XCTAssertFalse(TeamSharedListRules.usable(String(repeating: "a", count: 501)))
        XCTAssertFalse(TeamSharedListRules.fits(String(repeating: "\u{6E05}", count: 167)))
        body["payload"] = ["sharedBotId": sharedBotId, "operationId": "operation_0001", "expectedDataRevision": 0, "changes": [TeamSharedListChange.add(text: String(repeating: "a", count: 501)).payload]]
        XCTAssertThrowsError(try TeamExecutionContract.validate(body))
    }
    func testAListThatMovedIsReadAgainAndSaysSo() throws {
        XCTAssertTrue(TeamSharedListRules.moved("LIST_REVISION_CONFLICT"))
        XCTAssertTrue(TeamSharedListRules.moved("LIST_ITEM_REQUIRED"))
        XCTAssertFalse(TeamSharedListRules.moved("LIST_LIMIT"))
        XCTAssertEqual(TeamsErrors.key("LIST_REVISION_CONFLICT"), "teams_list_moved")
        XCTAssertEqual(TeamsErrors.key("LIST_LIMIT"), "teams_error_list_full")
        XCTAssertEqual(TeamsErrors.key("LIST_REQUIRED"), "teams_error_list_unavailable")
        let list = try JSONDecoder().decode(TeamSharedList.self, from: Data(#"{"sharedBotId":"sharedbot_01","title":"Groceries","description":"","dataRevision":2,"items":[{"itemId":"item_0001","text":"Bread","done":true}],"updatedAt":5}"#.utf8))
        XCTAssertEqual(list.items, [TeamSharedListItem(itemId: "item_0001", text: "Bread", done: true)])
        XCTAssertEqual(list.dataRevision, 2)
    }
    func testRetiredBotAuthorCannotDecode() throws {
        let base = #"{"id":"message_0001","seq":1,"author":{"kind":"bot","uid":"owner","botKey":"bot:o:h:x"},"kind":"text","text":"old","changeSeq":1,"changeId":"change_0001"}"#
        XCTAssertThrowsError(try JSONDecoder().decode(TeamMessage.self, from: Data(base.utf8)))
        let current = base.replacingOccurrences(of: #"{"kind":"bot","uid":"owner","botKey":"bot:o:h:x"}"#, with: #"{"kind":"shared_bot","sharedBotId":"sharedbot_01","displayName":"Helper"}"#)
        let message = try JSONDecoder().decode(TeamMessage.self, from: Data(current.utf8))
        XCTAssertEqual(message.author.sharedBotId, sharedBotId)
        XCTAssertEqual(message.author.uid, "")
    }
    func testDurableSubmitAndPauseRemainIsolatedByAccountTeamGeneration() async throws {
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: directory) }
        let storage = TeamCommandFiles(directory: directory)
        let submit = TeamMessageIntent(teamId: "team_0001", generation: 7, conversationId: "chat_0001", messageId: "message_0001", changeId: "change_0001", operation: "create", text: "Hello", replyTo: nil, targets: [sharedBotId])
        let pause = TeamExecutionIntent(commandId: "command_0001", action: "pauseSharedBot", sharedBotId: sharedBotId, expectedRevision: 1)
        _ = try await storage.update(uid: "member", teamId: "team_0001", generation: 7) { $0.outbox = [submit]; $0.commands = [pause] }
        let restored = try await storage.load(uid: "member", teamId: "team_0001", generation: 7)
        XCTAssertEqual(restored.outbox, [submit]); XCTAssertEqual(restored.commands, [pause])
        XCTAssertNoThrow(try TeamExecutionContract.validate(submit.body))
        XCTAssertEqual(submit.body["commandId"] as? String, "change_0001")
        for (uid, team, generation) in [("other", "team_0001", 7), ("member", "team_0002", 7), ("member", "team_0001", 8)] {
            let isolated = try await storage.load(uid: uid, teamId: team, generation: generation)
            XCTAssertTrue(isolated.outbox.isEmpty); XCTAssertTrue(isolated.commands?.isEmpty ?? true)
        }
    }
    func testRetiredBotCommandCleanupPreservesHumanCommandsAndUnrelatedData() async throws {
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: directory) }
        let storage = TeamCommandFiles(directory: directory)
        let human = TeamMessageIntent(teamId: "team_0001", generation: 7, conversationId: "chat_0001", messageId: "message_0001", changeId: "change_0001", operation: "create", text: "Human draft", replyTo: nil)
        _ = try await storage.update(uid: "member", teamId: "team_0001", generation: 7) { $0.outbox = [human] }
        let file = try await storage.url(uid: "member", teamId: "team_0001", generation: 7)
        var raw = try XCTUnwrap(try JSONSerialization.jsonObject(with: Data(contentsOf: file)) as? [String: Any])
        raw["botOutbox"] = [["action": "botAnswer", "answerId": "retired_01"]]
        raw["businessSentinel"] = "human history is preserved"
        try JSONSerialization.data(withJSONObject: raw).write(to: file)
        let restored = try await storage.load(uid: "member", teamId: "team_0001", generation: 7)
        XCTAssertEqual(restored.outbox, [human])
        _ = try await storage.update(uid: "member", teamId: "team_0001", generation: 7) { $0.creation = nil; $0.outbox = [] }
        let cleaned = try XCTUnwrap(try JSONSerialization.jsonObject(with: Data(contentsOf: file)) as? [String: Any])
        XCTAssertNil(cleaned["botOutbox"]); XCTAssertEqual(cleaned["businessSentinel"] as? String, "human history is preserved")
    }
    func testRawSelectionIdsMatchOnlyExplicitSharedBotPrincipals() throws {
        let raw = #"{"sharedBotId":"sharedbot_01","name":"Helper","description":"","maintainer":{"uid":"owner","generation":1},"revision":1,"state":"published","permitted":true,"ready":false,"onlineUntil":0,"usePolicy":{"mode":"all"},"quotas":{"team":100,"member":10,"bot":50}}"#
        let card = try JSONDecoder().decode(TeamBotCard.self, from: Data(raw.utf8))
        XCTAssertTrue(card.selectable, "An offline owner Desktop can queue a request without member model credentials")
        XCTAssertTrue(TeamBotSelection.unavailable([sharedBotId], cards: [card], participants: ["shared_bot:\(sharedBotId)"]).isEmpty)
        XCTAssertEqual(TeamBotSelection.unavailable([sharedBotId], cards: [card], participants: [sharedBotId]), [sharedBotId])
    }
}
