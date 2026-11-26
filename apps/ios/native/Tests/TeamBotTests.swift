import Foundation
import XCTest
@testable import LingxiCode

final class TeamBotTests: XCTestCase {
    private let sharedBotId = "sharedbot_01"
    private func command(_ action: String = "submit") -> [String: Any] {
        ["schemaVersion": 1, "commandId": "command_0001", "teamId": "team_0001", "generation": 7, "action": action,
         "payload": ["conversationId": "chat_0001", "messageId": "message_0001", "changeId": "change_0001", "text": "Hello", "targets": [sharedBotId]]]
    }
    private func vectors() throws -> [String: Any] {
        var path = URL(fileURLWithPath: #filePath)
        while !FileManager.default.fileExists(atPath: path.appendingPathComponent("packages/teams/contracts/execution-vectors.json").path), path.path != "/" { path.deleteLastPathComponent() }
        let fixture = Bundle(for: Self.self).url(forResource: "execution-vectors", withExtension: "json") ?? path.appendingPathComponent("packages/teams/contracts/execution-vectors.json")
        return try XCTUnwrap(try JSONSerialization.jsonObject(with: Data(contentsOf: fixture)) as? [String: Any])
    }
    func testSharedCommandVectors() throws {
        let value = try vectors()
        for row in try XCTUnwrap(value["commands"] as? [[String: Any]]) {
            let input = try XCTUnwrap(row["input"] as? [String: Any])
            if row["valid"] as? Bool == true { XCTAssertNoThrow(try TeamExecutionContract.validate(input), "\(row["name"] ?? "")") }
            else { XCTAssertThrowsError(try TeamExecutionContract.validate(input), "\(row["name"] ?? "")") }
        }
    }
    /// Bot v3 B6: the states and codes the contract lists are the ones this app can show and read from the log (`taskStates`, `approvalOutcomes`).
    func testEveryStateOfTheContractIsOneThisAppKnows() throws {
        let value = try vectors()
        let states = try XCTUnwrap(value["taskStates"] as? [String])
        XCTAssertTrue(states.contains("awaiting_approval"))
        for state in states {
            XCTAssertNotEqual(TeamsBotText.runState(state).labelKey, "chat_status_unknown", state)
            XCTAssertNotNil(TeamTriageRules.outcomes[state], state)
        }
        XCTAssertEqual(Set(TeamTriageRules.outcomes.keys), Set(states), "the log reads every state the contract lists, and no other")
        XCTAssertEqual(TeamTriageRules.approvals, Set(try XCTUnwrap(value["approvalOutcomes"] as? [String])))
    }
    /// A reply that waits for its maintainer can still change and can be taken back by the one who asked; no worker is changing it.
    func testATaskWhoseReplyAwaitsApprovalIsNeitherFinishedNorBeingWorkedOn() throws {
        func task(_ state: String) throws -> TeamTask {
            try JSONDecoder().decode(TeamTask.self, from: Data(#"{"taskId":"task_0001","conversationId":"chat_0001","sharedBotId":"sharedbot_01","requester":{"uid":"member","generation":3},"state":"\#(state)","createdAt":1,"deadline":2,"replyId":null,"artifactId":null}"#.utf8))
        }
        let requester = TeamMemberRef(uid: "member", generation: 3), maintainer = TeamMemberRef(uid: "owner", generation: 1), other = TeamMemberRef(uid: "other", generation: 1)
        let held = try task("awaiting_approval")
        XCTAssertFalse(held.terminal); XCTAssertFalse(held.busy)
        XCTAssertTrue(TeamTaskActions.canCancel(held, viewer: requester, maintainer: maintainer))
        XCTAssertTrue(TeamTaskActions.canCancel(held, viewer: maintainer, maintainer: maintainer))
        XCTAssertFalse(TeamTaskActions.canCancel(held, viewer: other, maintainer: maintainer))
        for state in ["completed", "declined", "failed", "cancelled"] { XCTAssertTrue(try task(state).terminal, state) }
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
    func testTheListHistorySaysWhoChangedWhatAndKeepsOlderPagesWhenTheNewestIsReadAgain() throws {
        let page = { (json: String) in try JSONDecoder().decode(TeamSharedListHistory.self, from: Data(json.utf8)) }
        let newest = try page(#"{"sharedBotId":"sharedbot_01","dataRevision":4,"hasMore":true,"revisions":[{"dataRevision":4,"author":{"kind":"shared_bot","sharedBotId":"sharedbot_01","taskId":"task_0001","requester":{"uid":"bob","generation":1}},"changes":[{"op":"update","itemId":"item_0001","text":"Whole milk","done":true,"before":{"text":"Milk","done":false}}],"createdAt":3},{"dataRevision":3,"author":{"kind":"user","uid":"alice","generation":1},"changes":[{"op":"remove","itemId":"item_0003","before":{"text":"Wine","done":false}}],"createdAt":2}]}"#)
        let older = try page(#"{"sharedBotId":"sharedbot_01","dataRevision":4,"hasMore":false,"revisions":[{"dataRevision":2,"author":{"kind":"user","uid":"carol","generation":1},"changes":[{"op":"add","itemId":"item_0002","text":"Oat milk"},{"op":"update","itemId":"item_0001","done":false,"before":{"text":"Milk","done":false}}],"createdAt":1}]}"#)
        typealias Rules = TeamSharedListHistoryRules
        XCTAssertEqual(Rules.lines(newest.revisions[0].changes[0]), [.renamed("Milk", "Whole milk"), .ticked("Whole milk")])
        XCTAssertEqual(Rules.lines(newest.revisions[1].changes[0]), [.removed("Wine")])
        XCTAssertEqual(older.revisions[0].changes.flatMap(Rules.lines), [.added("Oat milk"), .unchanged("Milk")])
        let members = [TeamMember(uid: "alice", displayName: "Alice", role: "owner", generation: 1), TeamMember(uid: "bob", displayName: "Bob", role: "member", generation: 1)]
        XCTAssertEqual(Rules.author(newest.revisions[0].author, viewer: "alice", members: members, botName: "Helper"), .bot("Helper", for: .member("Bob")))
        XCTAssertEqual(Rules.author(newest.revisions[1].author, viewer: "alice", members: members, botName: "Helper"), .person(.you))
        XCTAssertEqual(Rules.author(older.revisions[0].author, viewer: "alice", members: members, botName: "Helper"), .person(.former), "carol is not a member")
        XCTAssertEqual(Rules.author(newest.revisions[1].author, viewer: "bob", members: [TeamMember(uid: "alice", displayName: "Alice", role: "owner", generation: 2)], botName: "Helper"),
                       .person(.former), "an earlier membership of the same person is not the current one")
        let both = Rules.merge(Rules.merge(nil, newest, newest: true), older, newest: false)
        XCTAssertEqual(both.revisions.map(\.dataRevision), [4, 3, 2]); XCTAssertFalse(both.hasMore)
        let again = Rules.merge(both, newest, newest: true)
        XCTAssertEqual(again.revisions.map(\.dataRevision), [4, 3, 2], "reading the newest page again keeps the older page")
        XCTAssertFalse(again.hasMore)
        var body: [String: Any] = ["schemaVersion": 1, "commandId": "command_0001", "teamId": "team_0001", "generation": 1, "action": "listHistory",
                                   "payload": ["sharedBotId": "sharedbot_01", "beforeRevision": 3]]
        XCTAssertNoThrow(try TeamExecutionContract.validate(body))
        body["payload"] = ["sharedBotId": "sharedbot_01", "beforeRevision": 0]
        XCTAssertThrowsError(try TeamExecutionContract.validate(body))
    }
    func testADocumentEditIsValidOnlyAsTheServiceStoresIt() throws {
        let artifact = try JSONDecoder().decode(TeamArtifact.self, from: Data(#"{"artifactId":"artifact_0001","revision":2,"digest":"d","document":{"title":"Plan","body":"One"},"currentRevision":3}"#.utf8))
        XCTAssertTrue(artifact.old)
        var draft = TeamDocumentDraft(artifact.document)
        XCTAssertFalse(draft.changed(from: artifact.document))
        // Spaces around the title are not a change; the body is compared exactly.
        draft.title = " Plan "
        XCTAssertFalse(draft.changed(from: artifact.document))
        draft.body = "One\n"
        XCTAssertTrue(draft.changed(from: artifact.document))
        XCTAssertTrue(draft.valid)
        var body = command("reviseArtifact")
        body["payload"] = ["artifactId": "artifact_0001", "expectedRevision": 3, "operationId": "operation_0001", "document": draft.payload]
        XCTAssertNoThrow(try TeamExecutionContract.validate(body))
        XCTAssertEqual(draft.payload["title"] as? String, "Plan")
        draft.title = String(repeating: "t", count: 121)
        XCTAssertFalse(draft.valid)
        draft.title = "Plan"; draft.body = ""
        XCTAssertFalse(draft.valid)
        draft.body = String(repeating: "a", count: TeamExecutionContract.documentBytes + 1)
        XCTAssertTrue(draft.tooLong)
        XCTAssertFalse(draft.valid)
    }
    func testARepeatedDocumentSaveKeepsItsOperationAndAnyOtherEditGetsANewOne() {
        var made = 0
        let id = { made += 1; return "operation_000\(made)" }
        let draft = TeamDocumentDraft(title: "Plan ", body: "Two")
        let first = TeamDocumentOperation.of(nil, revision: 3, draft: draft, newId: id)
        XCTAssertEqual(TeamDocumentOperation.of(first, revision: 3, draft: TeamDocumentDraft(title: "Plan", body: "Two"), newId: id), first)
        XCTAssertEqual(TeamDocumentOperation.of(first, revision: 3, draft: TeamDocumentDraft(title: "Plan", body: "Three"), newId: id).id, "operation_0002")
        // Saving the same text over a newer revision is another decision.
        XCTAssertEqual(TeamDocumentOperation.of(first, revision: 4, draft: draft, newId: id).id, "operation_0003")
    }
    func testTheHistoryNamesMembersAndBotsAndKeepsAGenericNameForThoseWhoLeft() throws {
        let history = try JSONDecoder().decode(TeamArtifactRevisions.self, from: Data(#"{"artifactId":"artifact_0001","currentRevision":2,"revisions":[{"revision":2,"parentRevision":1,"digest":"b","title":"Plan","author":{"kind":"user","uid":"member","generation":3},"createdAt":2000},{"revision":1,"parentRevision":null,"digest":"a","title":"Plan","author":{"kind":"shared_bot","sharedBotId":"sharedbot_01"},"createdAt":1000,"sourceTaskId":"task_0001"}]}"#.utf8))
        let members = [TeamMember(uid: "member", displayName: "Ada", role: "member", generation: 3)]
        let strings = TeamsListStrings.en
        XCTAssertEqual(history.revisions.map(\.revision), [2, 1])
        XCTAssertNil(history.revisions[1].parentRevision)
        XCTAssertEqual(TeamDocumentRules.authorName(history.revisions[0].author, members: members, bots: [try card()], strings: strings), "Ada")
        XCTAssertEqual(TeamDocumentRules.authorName(history.revisions[1].author, members: members, bots: [try card()], strings: strings), "Helper")
        XCTAssertEqual(TeamDocumentRules.authorName(history.revisions[0].author, members: [], bots: [], strings: strings), strings.member)
        XCTAssertEqual(TeamDocumentRules.authorName(history.revisions[1].author, members: [], bots: [], strings: strings), strings.bot)
        XCTAssertEqual(TeamsErrors.key("ARTIFACT_REVISION_LIMIT"), "teams_error_doc_limit")
        XCTAssertEqual(TeamsErrors.key("ARTIFACT_UNCHANGED"), "teams_error_doc_unchanged")
    }
    func testOnlyAnActionNobodyCouldVerifyAsksForADecision() throws {
        let listed = try JSONDecoder().decode(TeamTaskOperations.self, from: Data(#"{"taskId":"task_0001","operations":[{"operationId":"a","logicalStepId":"s1","category":"queryable","tool":{"name":"send_mail","version":"1"},"state":"dispatched","epoch":1,"dispatchedAt":1000},{"operationId":"b","logicalStepId":"s2","category":"unqueryable","tool":{"name":"post","version":"1"},"state":"unknown","epoch":1,"dispatchedAt":1000},{"operationId":"c","logicalStepId":"s3","category":"unqueryable","tool":{"name":"post","version":"1"},"state":"succeeded","epoch":1,"dispatchedAt":1000,"settledAt":2000,"resolvedBy":{"uid":"owner","generation":1},"resolvedAt":2000},{"operationId":"d","logicalStepId":"s4","category":"queryable","tool":{"name":"post","version":"1"},"state":"failed","epoch":1,"dispatchedAt":1000},{"operationId":"e","logicalStepId":"s5","category":"store_idempotent","tool":{"name":"read","version":"1"},"state":"dispatched","epoch":1,"dispatchedAt":1000}]}"#.utf8)).operations
        XCTAssertEqual(listed.map(\.unverified), [true, true, false, false, false])
        XCTAssertEqual(listed.map(TeamOperationRules.stateKey), ["teams_ops_state_unreported", "teams_ops_state_unknown", "teams_ops_state_happened", "teams_ops_state_not_happened", "teams_ops_state_repeatable"])
        XCTAssertEqual(listed[2].resolvedBy, TeamMemberRef(uid: "owner", generation: 1))
    }
    func testTheRequesterSeesTheActionsAndOnlyTheMaintainerConfirmsThem() throws {
        func task(_ state: String) throws -> TeamTask {
            try JSONDecoder().decode(TeamTask.self, from: Data(#"{"taskId":"task_0001","conversationId":"chat_0001","sharedBotId":"sharedbot_01","requester":{"uid":"member","generation":3},"state":"\#(state)","createdAt":1,"deadline":2,"replyId":null,"artifactId":null}"#.utf8))
        }
        let requester = TeamMemberRef(uid: "member", generation: 3), maintainer = TeamMemberRef(uid: "owner", generation: 1), other = TeamMemberRef(uid: "other", generation: 1)
        let unchecked = try task("needs_reconciliation")
        XCTAssertTrue(TeamOperationRules.canReview(unchecked, viewer: requester, maintainer: maintainer))
        XCTAssertTrue(TeamOperationRules.canReview(unchecked, viewer: maintainer, maintainer: maintainer))
        XCTAssertFalse(TeamOperationRules.canReview(unchecked, viewer: other, maintainer: maintainer))
        XCTAssertFalse(TeamOperationRules.canReview(unchecked, viewer: TeamMemberRef(uid: "member", generation: 4), maintainer: maintainer), "A member who rejoined is another member")
        XCTAssertFalse(TeamOperationRules.canReview(try task("failed"), viewer: maintainer, maintainer: maintainer))
        XCTAssertTrue(TeamOperationRules.canResolve(viewer: maintainer, maintainer: maintainer))
        XCTAssertFalse(TeamOperationRules.canResolve(viewer: requester, maintainer: maintainer))
        XCTAssertFalse(TeamOperationRules.canResolve(viewer: requester, maintainer: nil))
        XCTAssertEqual(TeamOperationRules.errorKey("MAINTAINER_REQUIRED"), "teams_error_ops_maintainer")
        XCTAssertEqual(TeamOperationRules.errorKey("TASK_RUNNING"), "teams_error_ops_running")
        XCTAssertEqual(TeamOperationRules.errorKey("OPERATION_CONFLICT"), "teams_error_ops_conflict")
        XCTAssertEqual(TeamOperationRules.errorKey("MEMBERSHIP_REQUIRED"), TeamsErrors.key("MEMBERSHIP_REQUIRED"))
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

    func testAutoReplyOffersPublishedBotsOfTheChatToItsPeopleAndAlwaysLetsOneBeSwitchedOff() throws {
        func card(_ id: String, state: String = "published", daily: Int? = 5) throws -> TeamBotCard {
            var row: [String: Any] = ["sharedBotId": id, "name": id, "description": "", "maintainer": ["uid": "owner", "generation": 1], "revision": 1, "state": state,
                                      "permitted": true, "ready": true, "onlineUntil": 0, "usePolicy": ["mode": "all"], "quotas": ["team": 1, "member": 1, "bot": 1]]
            if let daily { row["autoReply"] = ["daily": daily] }
            return try JSONDecoder().decode(TeamBotCard.self, from: JSONSerialization.data(withJSONObject: row))
        }
        let cards = [try card("sharedbot_a"), try card("sharedbot_b", daily: nil), try card("sharedbot_c", state: "paused"), try card("sharedbot_d")]
        XCTAssertEqual(cards[0].autoReply, TeamBotAutoReply(daily: 5))
        var chat = TeamMessageConversation(id: "chat_0001", kind: "group", title: "Trip", participants: ["user:alice", "shared_bot:sharedbot_a", "shared_bot:sharedbot_b", "shared_bot:sharedbot_c"],
                                           lastSeq: 0, lastChangeSeq: 0, lastReadSeq: 0, autoReply: TeamConversationAutoReply(revision: 3, targets: ["sharedbot_c"]))
        let offered = TeamAutoReply.bots(chat, cards: cards, uid: "alice")
        XCTAssertEqual(offered.bots.map(\.sharedBotId), ["sharedbot_a", "sharedbot_c"], "a Bot not in the Chat or not published for it is not offered; one already on is")
        XCTAssertEqual(offered.on, ["sharedbot_c"])
        XCTAssertEqual(TeamAutoReply.targets(offered.bots, on: offered.on, switching: "sharedbot_a", to: true), ["sharedbot_a", "sharedbot_c"])
        XCTAssertEqual(TeamAutoReply.targets(offered.bots, on: offered.on, switching: "sharedbot_c", to: false), [])
        XCTAssertTrue(TeamAutoReply.bots(chat, cards: cards, uid: "bob").bots.isEmpty, "only the Chat's people change it")
        // A Bot on in the Chat that this person cannot see (published to others only) stays on whatever they switch.
        chat = TeamMessageConversation(id: "chat_0001", kind: "group", title: "Trip", participants: ["user:alice", "shared_bot:sharedbot_a", "shared_bot:sharedbot_x"],
                                       lastSeq: 0, lastChangeSeq: 0, lastReadSeq: 0, autoReply: TeamConversationAutoReply(revision: 4, targets: ["sharedbot_x", "sharedbot_gone"]))
        let hidden = TeamAutoReply.bots(chat, cards: cards, uid: "alice")
        XCTAssertEqual(hidden.on, []); XCTAssertEqual(hidden.kept, ["sharedbot_x"], "a Bot no longer in the Chat is not kept")
        XCTAssertEqual(TeamAutoReply.targets(hidden.bots, on: hidden.on, kept: hidden.kept, switching: "sharedbot_a", to: true), ["sharedbot_a", "sharedbot_x"])
        chat = TeamMessageConversation(id: "chat_0002", kind: "dm", title: "", participants: ["user:alice", "shared_bot:sharedbot_a"], lastSeq: 0, lastChangeSeq: 0, lastReadSeq: 0)
        XCTAssertTrue(TeamAutoReply.bots(chat, cards: cards, uid: "alice").bots.isEmpty)
    }
}

/// P0 member projections and example admission are Foundation-only so they can run without a native app build.
final class TeamBotMemberEntryTests: XCTestCase {
    private func card(_ extra: [String: Any] = [:]) throws -> TeamBotCard {
        var json: [String: Any] = ["sharedBotId": "sharedbot_01", "name": "Helper", "description": "", "maintainer": ["uid": "owner", "generation": 3],
            "revision": 2, "state": "published", "permitted": true, "ready": true, "onlineUntil": 0,
            "usePolicy": ["mode": "all"], "quotas": ["team": 100, "member": 20, "bot": 50]]
        json.merge(extra) { _, new in new }
        return try JSONDecoder().decode(TeamBotCard.self, from: JSONSerialization.data(withJSONObject: json))
    }
    func testMissingProjectionNeverInventsCapabilityOrRemainingQuota() throws {
        let value = try card()
        XCTAssertEqual(value.memberAvailability, .unverified)
        XCTAssertNil(value.capabilities)
        XCTAssertNil(value.memberRemaining)
        XCTAssertNil(value.memberQuotaReset)
        XCTAssertEqual(value.memberExamples, [])
        XCTAssertEqual(TeamBotMemberText.availabilityKey(value.memberAvailability), "teams_bot_avail_unverified")
        XCTAssertEqual(TeamBotMemberText.remaining(value.memberRemaining), .init(label: "teams_bot_remaining_today", value: .key("chat_status_unknown")))
        XCTAssertEqual(TeamBotMemberText.capabilities(value.capabilities).map(\.value), [.key("chat_status_unknown"), .key("chat_status_unknown")],
                       "a Bot that has not reported what it can do is Unknown, not unsupported")
        XCTAssertNil(TeamBotMemberText.reset(value.memberQuotaReset))
    }
    func testServerMemberProjectionPreservesZeroAndActualCapabilities() throws {
        let value = try card(["examples": [" First ", "", "First", "Second", "Third", "Fourth"],
            "capabilities": ["durableWait": true, "writeOperations": false], "availability": "quota_exhausted", "dailyRemaining": 0, "quotaResetsAt": 86_400_000])
        XCTAssertEqual(value.memberExamples, ["First", "Second", "Third"])
        XCTAssertEqual(value.capabilities, TeamBotCapabilities(durableWait: true, writeOperations: false))
        XCTAssertEqual(value.memberAvailability, .quota_exhausted)
        XCTAssertEqual(value.memberRemaining, 0)
        XCTAssertEqual(TeamBotMemberText.availabilityKey(value.memberAvailability), "teams_bot_avail_quota")
        XCTAssertEqual(TeamBotMemberText.remaining(value.memberRemaining), .init(label: "teams_bot_remaining_today", value: .figure("0")), "zero is a figure, not Unknown")
        XCTAssertEqual(TeamBotMemberText.reset(value.memberQuotaReset), .init(label: "teams_bot_resets", value: .figure("1970-01-02 00:00 UTC")))
        XCTAssertEqual(TeamBotMemberText.capabilities(value.capabilities), [.init(label: "teams_bot_wait_resume", value: .key("teams_bot_supported")),
                                                                            .init(label: "teams_bot_write_ops", value: .key("teams_bot_not_supported"))])
    }
    func testUnknownStateAndInvalidProjectionStayUnverified() throws {
        let value = try card(["availability": "raw_host_failure_credentials", "dailyRemaining": -1, "quotaResetsAt": -1])
        XCTAssertEqual(value.memberAvailability, .unverified)
        XCTAssertNil(value.memberRemaining)
        XCTAssertNil(value.memberQuotaReset)
        XCTAssertNil(TeamBotMemberText.reset(.infinity))
        XCTAssertThrowsError(try card(["capabilities": ["durableWait": true]]))
        for state in ["ready", "paused", "offline", "maintenance", "quota_exhausted", "unverified"] {
            let card = try card(["availability": state])
            XCTAssertEqual(card.memberAvailability.rawValue, state)
        }
        XCTAssertEqual(TeamBotMemberText.availabilityKey(value.memberAvailability), "teams_bot_avail_unverified")
    }
    /// The member text is the catalog's: the keys Desktop and Web read (`packages/teams/BotMemberDetails.tsx`), each with its own words in all five languages.
    func testMemberTextIsTheSharedCatalogKeysInEveryLanguage() throws {
        let states: [(TeamBotAvailability, String)] = [(.ready, "teams_bot_avail_ready"), (.paused, "teams_bot_disabled"), (.offline, "teams_bot_avail_offline"),
            (.maintenance, "teams_bot_avail_maintenance"), (.quota_exhausted, "teams_bot_avail_quota"), (.unverified, "teams_bot_avail_unverified")]
        for (state, key) in states { XCTAssertEqual(TeamBotMemberText.availabilityKey(state), key, "\(state)") }
        XCTAssertEqual(Set(states.map(\.1)).count, states.count, "each state has its own words")
        let reset = try XCTUnwrap(TeamBotMemberText.reset(86_400_000))
        let items = TeamBotMemberText.capabilities(TeamBotCapabilities(durableWait: false, writeOperations: true)) + [TeamBotMemberText.remaining(3), reset]
        XCTAssertEqual(items.map(\.label), ["teams_bot_wait_resume", "teams_bot_write_ops", "teams_bot_remaining_today", "teams_bot_resets"])
        XCTAssertEqual(items.map(\.value), [.key("teams_bot_not_supported"), .key("teams_bot_supported"), .figure("3"), .figure("1970-01-02 00:00 UTC")])
        let catalogue = try Catalogue.load()
        let keys = states.map(\.1) + items.map(\.label) + ["teams_bot_supported", "teams_bot_not_supported", "chat_status_unknown", "teams_bot_examples_hint"]
        for key in keys {
            for language in ["en", "zh-Hans", "zh-Hant", "ja", "ko"] {
                XCTAssertFalse((catalogue.value(key, language) ?? "").isEmpty, "\(key) has no \(language) text")
            }
        }
    }
    func testExamplesOnlyFillAnUntouchedComposer() {
        let empty = TeamComposerDraft()
        XCTAssertEqual(empty.fillingExample("Example", hasFiles: false).text, "Example")
        XCTAssertEqual(empty.fillingExample("Example", hasFiles: true), empty)
        XCTAssertEqual(empty.fillingExample("  ", hasFiles: false), empty)
        for draft in [TeamComposerDraft(text: "Existing"), TeamComposerDraft(text: " "), TeamComposerDraft(replyToID: "reply"), TeamComposerDraft(targets: ["sharedbot_other"])] {
            XCTAssertEqual(draft.fillingExample("Example", hasFiles: false), draft)
        }
    }
    func testDraftOwnershipIncludesAccountTeamGenerationAndConversation() {
        let key = TeamComposerDraft.key(uid: "u", teamID: "t", generation: 1, conversationID: "c")
        XCTAssertNotEqual(key, TeamComposerDraft.key(uid: "other", teamID: "t", generation: 1, conversationID: "c"))
        XCTAssertNotEqual(key, TeamComposerDraft.key(uid: "u", teamID: "other", generation: 1, conversationID: "c"))
        XCTAssertNotEqual(key, TeamComposerDraft.key(uid: "u", teamID: "t", generation: 2, conversationID: "c"))
        XCTAssertNotEqual(key, TeamComposerDraft.key(uid: "u", teamID: "t", generation: 1, conversationID: "other"))
    }
    func testDirectoryKeepsServerVisiblePausedCardsWithoutGrantingUse() throws {
        let published = try card(), paused = try card(["sharedBotId": "sharedbot_paused", "state": "paused", "permitted": false, "availability": "paused"])
        let retired = try card(["sharedBotId": "sharedbot_retired", "state": "deleted"])
        XCTAssertEqual(TeamBotMemberRules.directory([published, paused, retired]).map(\.sharedBotId), ["sharedbot_01", "sharedbot_paused"])
        XCTAssertFalse(paused.selectable)
        XCTAssertEqual(TeamBotMemberText.availabilityKey(paused.memberAvailability), "teams_bot_disabled", "a Bot whose sharing stopped reads Sharing stopped (U1), never Paused")
    }
    func testOnlyNewBotSubmitsObserveKnownAvailability() throws {
        for state in ["paused", "offline", "maintenance", "quota_exhausted"] {
            let card = try card(["availability": state])
            let block = { (targets: Set<String>, operation: String, wait: Bool) in
                TeamBotMemberRules.blockedAvailability(targets: targets, cards: [card], operation: operation, answeringWait: wait)
            }
            XCTAssertEqual(block(["sharedbot_01"], "create", false)?.rawValue, state)
            XCTAssertNil(block(["sharedbot_01"], "create", true), "a wait answer starts no task")
            XCTAssertNil(block(["sharedbot_01"], "edit", false))
            XCTAssertNil(block(["sharedbot_01"], "recall", false))
            XCTAssertNil(block([], "create", false), "human-only messages retain local queueing")
            XCTAssertNil(block(["other_bot"], "create", false))
        }
        for fields: [String: Any] in [[:], ["availability": "unverified"], ["availability": "ready"]] {
            XCTAssertNil(TeamBotMemberRules.blockedAvailability(targets: ["sharedbot_01"], cards: [try card(fields)]))
        }
    }
    func testMaintenanceRefreshExpiresIndependentlyOfHostLeaseUsingFakeClock() throws {
        let maintenance = try card(["availability": "maintenance", "ready": false, "onlineUntil": 9000, "statusExpiresAt": 2000, "dailyRemaining": 4])
        let ready = try card(["sharedBotId": "sharedbot_ready", "availability": "ready", "onlineUntil": 8000, "statusExpiresAt": 6000])
        XCTAssertEqual(TeamBotMemberRules.nextStatusRefresh([ready, maintenance], now: 1000), 2000)
        XCTAssertEqual(maintenance.expiringStatus(at: 1999).memberAvailability, .maintenance)
        let expired = maintenance.expiringStatus(at: 2000)
        XCTAssertEqual(expired.memberAvailability, .unverified)
        XCTAssertNil(expired.memberRemaining, "a slow/failed refresh must not leave a fresh-looking remaining count")
        XCTAssertEqual(TeamBotMemberRules.blockedAvailability(targets: ["sharedbot_01"], cards: [expired]), .maintenance)
        XCTAssertEqual(TeamBotMemberRules.nextStatusRefresh([ready, expired], now: 2000), 6000)
        XCTAssertNil(TeamBotMemberRules.nextStatusRefresh([ready, expired], now: 6000))
        let legacy = try card(["onlineUntil": 5000]), legacyOffline = try card(["ready": false, "onlineUntil": 5000])
        XCTAssertEqual(legacy.memberStatusExpiry, 5000)
        XCTAssertNil(legacyOffline.memberStatusExpiry)
        XCTAssertEqual(TeamBotMemberRules.nextStatusRefresh([legacy, legacyOffline], now: 4000), 5000)
    }

    func testKnownRefusalRemainsBlockedDuringPendingAndFailedRefreshUntilFreshReady() throws {
        for state in ["paused", "offline", "maintenance", "quota_exhausted"] {
            let known = try card(["availability": state, "statusExpiresAt": 2000])
            let pending = known.expiringStatus(at: 2000)
            let failed = TeamBotMemberRules.failedValidation([pending])
            XCTAssertEqual(TeamBotMemberRules.blockedAvailability(targets: ["sharedbot_01"], cards: [pending])?.rawValue, state)
            XCTAssertEqual(TeamBotMemberRules.blockedAvailability(targets: ["sharedbot_01"], cards: failed)?.rawValue, state)
        }
        let maintenance = try card(["availability": "maintenance", "ready": false, "onlineUntil": 9000, "statusExpiresAt": 2000])
        let pending = maintenance.expiringStatus(at: 2000)
        XCTAssertEqual(pending.memberAvailability, .unverified)
        XCTAssertEqual(TeamBotMemberRules.blockedAvailability(targets: ["sharedbot_01"], cards: [pending]), .maintenance)
        let failed = TeamBotMemberRules.failedValidation([pending, try card(["sharedBotId": "legacy"])])
        XCTAssertEqual(failed.map(\.sharedBotId), ["sharedbot_01"], "legacy cards keep their original failed-read behavior")
        XCTAssertEqual(TeamBotMemberRules.blockedAvailability(targets: ["sharedbot_01"], cards: failed), .maintenance)
        let unknown = TeamBotMemberRules.validatedCards([try card(["availability": "unverified"])], previous: failed, now: 2500)
        XCTAssertEqual(TeamBotMemberRules.blockedAvailability(targets: ["sharedbot_01"], cards: unknown), .maintenance)
        let staleReady = TeamBotMemberRules.validatedCards([try card(["availability": "ready", "statusExpiresAt": 2400])], previous: failed, now: 2500)
        XCTAssertEqual(TeamBotMemberRules.blockedAvailability(targets: ["sharedbot_01"], cards: staleReady), .maintenance)
        let ready = TeamBotMemberRules.validatedCards([try card(["availability": "ready", "statusExpiresAt": 4000])], previous: failed, now: 2500)
        XCTAssertNil(TeamBotMemberRules.blockedAvailability(targets: ["sharedbot_01"], cards: ready))
        XCTAssertNil(TeamBotMemberRules.blockedAvailability(targets: [], cards: failed))
        XCTAssertNil(TeamBotMemberRules.blockedAvailability(targets: ["sharedbot_01"], cards: failed, answeringWait: true))
        XCTAssertNil(TeamBotMemberRules.blockedAvailability(targets: ["sharedbot_01"], cards: failed, operation: "edit"))
        XCTAssertNil(TeamBotMemberRules.blockedAvailability(targets: ["sharedbot_01"], cards: failed, operation: "recall"))
    }

}
