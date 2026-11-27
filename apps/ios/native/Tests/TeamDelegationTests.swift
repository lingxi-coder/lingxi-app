import Foundation
import XCTest
@testable import LingxiCode

/// Bots asking Bots (Bot v3 B8b), as a member reads it: the chain of a delegated task, the Bot a task waits for, who may cancel, the line of a row of the auto-reply log and the card of a Bot. The decoding and
/// the rules, Foundation-only like the log tests; the screens are `TeamBotViews.swift` and `TeamTriageLogViews.swift`. `packages/teams/delegation.ts` and Android's `TeamDelegationsTest` say the same.
final class TeamDelegationTests: XCTestCase {
    private let alice = TeamMemberRef(uid: "alice", generation: 1), bob = TeamMemberRef(uid: "bob", generation: 1)
    private let carol = TeamMemberRef(uid: "carol", generation: 1), dan = TeamMemberRef(uid: "dan", generation: 1)

    private func decode<T: Decodable>(_ type: T.Type, _ value: Any) throws -> T { try JSONDecoder().decode(T.self, from: JSONSerialization.data(withJSONObject: value)) }
    private func ref(_ member: TeamMemberRef) -> [String: Any] { ["uid": member.uid, "generation": member.generation] }
    private func card(_ id: String, _ name: String, maintainer: TeamMemberRef, state: String = "published", extra: [String: Any] = [:]) throws -> TeamBotCard {
        var value: [String: Any] = ["sharedBotId": id, "name": name, "description": "", "maintainer": ref(maintainer), "revision": 1, "state": state, "permitted": true, "ready": true, "onlineUntil": 0,
                                    "usePolicy": ["mode": "all"], "quotas": ["team": 1, "member": 1, "bot": 1]]
        value.merge(extra) { _, new in new }
        return try decode(TeamBotCard.self, value)
    }
    private func link(_ id: String, _ name: String) -> [String: Any] { ["sharedBotId": id, "name": name] }
    /// What the service writes of a task another Bot asked for (B8a): Alice asked Planner, Planner asked Travel.
    private func delegation(parent: String = "sharedbot_planner", chain: [[String: Any]]? = nil) -> [String: Any] {
        ["delegationId": "delegation_1", "rootTaskId": "task_root", "parentTaskId": "task_root", "parentBotId": parent, "depth": 1, "expectReply": true,
         "chain": chain ?? [link("sharedbot_planner", "Planner"), link("sharedbot_travel", "Travel")]]
    }
    private func task(_ id: String = "task_child", bot: String = "sharedbot_travel", state: String = "running", delegation: [String: Any]? = nil, wait: [String: Any]? = nil, extra: [String: Any] = [:]) throws -> TeamTask {
        var value: [String: Any] = ["taskId": id, "conversationId": "chat_0001", "sharedBotId": bot, "requester": ref(alice), "state": state, "createdAt": 1.0, "deadline": 2.0]
        if let delegation { value["delegation"] = delegation }
        if let wait { value["wait"] = wait }
        value.merge(extra) { _, new in new }
        return try decode(TeamTask.self, value)
    }
    /// The task of Planner that waits for the reply of Travel's task; the person who asked is the recipient of the wait, and has nothing to answer.
    private func plannerWaiting(_ kind: String = "delegation") throws -> TeamTask {
        var wait: [String: Any] = ["waitId": "wait_1", "kind": kind, "recipient": ref(alice), "expiresAt": 9_000_000.0, "messageId": "message_1"]
        if kind == "delegation" { wait["delegation"] = ["sharedBotId": "sharedbot_travel", "taskId": "task_child"] }
        return try task("task_root", bot: "sharedbot_planner", state: "waiting", wait: wait)
    }
    private func teamCards() throws -> [TeamBotCard] {
        [try card("sharedbot_planner", "Planner", maintainer: bob), try card("sharedbot_travel", "Travel", maintainer: carol)]
    }

    // MARK: The chain

    func testATaskAnotherBotAskedForCarriesWhoAskedAndTheChainFromTheBotThePersonAsked() throws {
        let asked = try task(delegation: delegation())
        XCTAssertEqual(asked.delegation, TeamTaskDelegation(parentBotId: "sharedbot_planner", chain: [TeamDelegationLink(sharedBotId: "sharedbot_planner", name: "Planner"), TeamDelegationLink(sharedBotId: "sharedbot_travel", name: "Travel")]))
        XCTAssertNil(try task().delegation, "a task nobody delegated has none")
        XCTAssertEqual(try task(extra: ["somethingNew": ["a": 1]]).taskId, "task_child", "what this version does not read of a task does not fail it")
    }

    func testTheChainReadsFromTheBotAskedToTheBotThatRunsTheTaskAndNamesNoBotByItsId() throws {
        let cards = try teamCards()
        XCTAssertEqual(TeamDelegations.chain(try task(delegation: delegation()), cards: cards, unnamed: "Bot"), "Planner → Travel")
        XCTAssertNil(TeamDelegations.chain(try task(), cards: cards, unnamed: "Bot"), "a task nobody delegated")
        XCTAssertNil(TeamDelegations.chain(try task(delegation: delegation(chain: [link("sharedbot_travel", "Travel")])), cards: cards, unnamed: "Bot"), "a chain of one Bot is no chain")
        let three = delegation(chain: [link("sharedbot_planner", "Planner"), link("sharedbot_travel", "Travel"), link("sharedbot_chef", "Chef")])
        XCTAssertEqual(TeamDelegations.chain(try task(delegation: three), cards: cards, unnamed: "Bot"), "Planner → Travel → Chef", "at most two hops, so at most three Bots")
        // A Bot is named by the card the Team lists now; the name it had when it was asked is for a Bot that has no card.
        let renamed = [try card("sharedbot_planner", "Planner", maintainer: bob), try card("sharedbot_travel", "Travel 2", maintainer: carol)]
        XCTAssertEqual(TeamDelegations.chain(try task(delegation: delegation()), cards: renamed, unnamed: "Bot"), "Planner → Travel 2")
        XCTAssertEqual(TeamDelegations.chain(try task(delegation: delegation()), cards: [], unnamed: "Bot"), "Planner → Travel", "no card of either: the names of the chain")
        // A chain made from a handoff names a Bot by its id: that is no name.
        let byId = delegation(chain: [link("sharedbot_planner", "sharedbot_planner"), link("sharedbot_travel", "Travel")])
        XCTAssertEqual(TeamDelegations.chain(try task(delegation: byId), cards: [], unnamed: "Bot"), "Bot → Travel")
    }

    // MARK: Waiting for a Bot

    func testATaskThatWaitsForAnotherBotNamesItAndIsNotAQuestionToThePersonWhoAsked() throws {
        let waiting = try plannerWaiting()
        XCTAssertEqual(waiting.wait?.kind, "delegation")
        XCTAssertEqual(waiting.wait?.delegation, TeamTaskWaitDelegation(sharedBotId: "sharedbot_travel", taskId: "task_child"))
        XCTAssertEqual(waiting.wait?.recipient, alice, "the service names the person who asked as the recipient")
        XCTAssertNil(TeamTaskActions.answerableWait(waiting, viewer: alice, pendingWaitIds: []), "there is nothing for them to answer")
        XCTAssertNil(try plannerWaiting("human_input").wait?.delegation, "a question to a person waits for no Bot")
        XCTAssertEqual(TeamTaskActions.answerableWait(try plannerWaiting("human_input"), viewer: alice, pendingWaitIds: []), "wait_1", "a question to a person is answered as before")
        XCTAssertFalse(waiting.terminal); XCTAssertFalse(waiting.busy)
    }

    func testTheBotATaskWaitsForIsNamedByItsCardThenByTheTaskItAskedAndThenNotAtAll() throws {
        let waiting = try plannerWaiting()
        let asked = try task("task_child", delegation: delegation(chain: [link("sharedbot_planner", "Planner"), link("sharedbot_travel", "Trip desk")]))
        XCTAssertEqual(TeamDelegations.waitingFor(waiting, cards: try teamCards()), "Travel", "by its card")
        XCTAssertEqual(TeamDelegations.waitingFor(waiting, cards: [try card("sharedbot_planner", "Planner", maintainer: bob)], known: [asked]), "Trip desk", "no card of it: the name it had when it was asked")
        XCTAssertNil(TeamDelegations.waitingFor(waiting, cards: [try card("sharedbot_planner", "Planner", maintainer: bob)]), "no name to give")
        XCTAssertNil(TeamDelegations.waitingFor(waiting, cards: [], known: [try task("task_other", delegation: delegation())]), "another task is not the one it asked")
        let byId = try task("task_child", delegation: delegation(chain: [link("sharedbot_planner", "Planner"), link("sharedbot_travel", "sharedbot_travel")]))
        XCTAssertNil(TeamDelegations.waitingFor(waiting, cards: [], known: [byId]), "an id is not a name")
        XCTAssertNil(TeamDelegations.waitingFor(try plannerWaiting("human_input"), cards: try teamCards()), "a task that waits for a person names no Bot")
        XCTAssertNil(TeamDelegations.waitingFor(try task("task_root", state: "running", wait: ["waitId": "w", "kind": "delegation", "recipient": ref(alice), "expiresAt": 1.0, "messageId": "m",
                                                                                                 "delegation": ["sharedBotId": "sharedbot_travel", "taskId": "task_child"]]), cards: try teamCards()), "a wait that is over is not shown")
    }

    // MARK: Who may cancel

    func testWhoMayCancelTheRequesterTheMaintainerOfTheBotThatRunsItAndTheMaintainerOfTheBotThatAsked() throws {
        let cards = try teamCards()
        let asked = try task(delegation: delegation())
        func may(_ viewer: TeamMemberRef, _ task: TeamTask) -> Bool {
            TeamTaskActions.canCancel(task, viewer: viewer, maintainer: cards.first { $0.sharedBotId == task.sharedBotId }?.maintainer, delegator: TeamDelegations.delegator(task, cards: cards))
        }
        XCTAssertEqual(TeamDelegations.delegator(asked, cards: cards), bob, "the maintainer of Planner, which asked")
        XCTAssertTrue(may(alice, asked), "the person who asked"); XCTAssertTrue(may(carol, asked), "the maintainer of the Bot that runs it"); XCTAssertTrue(may(bob, asked), "the maintainer of the Bot that asked for it")
        XCTAssertFalse(may(dan, asked), "anybody else"); XCTAssertFalse(may(TeamMemberRef(uid: "bob", generation: 0), asked), "the membership they held before is not the one they hold now")
        // Planner's maintainer may stop what Planner asked for, not what somebody else asked Travel for.
        let plain = try task()
        XCTAssertNil(TeamDelegations.delegator(plain, cards: cards)); XCTAssertFalse(may(bob, plain)); XCTAssertTrue(may(carol, plain))
        XCTAssertNil(TeamDelegations.delegator(asked, cards: [cards[1]]), "a Bot that is not in the list is nobody's to maintain"); XCTAssertFalse(TeamTaskActions.canCancel(asked, viewer: bob, maintainer: carol, delegator: nil))
        XCTAssertNil(TeamDelegations.delegator(try task(delegation: delegation(parent: "sharedbot_other")), cards: cards), "the Bot that asked is the one in the delegation, not any Bot of the chain")
        for state in ["completed", "failed", "cancelled", "expired"] { XCTAssertFalse(may(bob, try task(state: state, delegation: delegation())), state) }
        for state in ["queued", "running", "waiting", "awaiting_approval"] { XCTAssertTrue(may(bob, try task(state: state, delegation: delegation())), state) }
        XCTAssertFalse(may(bob, try task(delegation: delegation(), extra: ["cancelRequestedAt": 5.0])), "it is already being stopped")
    }

    // MARK: The auto-reply log

    func testARowOfTheLogWhoseTaskAnotherBotAskedForSaysWhichBotAndOneThisVersionCannotReadIsStillARow() throws {
        func row(_ extra: [String: Any]) throws -> TeamTriageItem {
            var value: [String: Any] = ["taskId": "task-0001", "conversationId": "chat_0001", "messageId": "message-1", "createdAt": 1_700_000_000_000.0, "state": "completed",
                                        "triage": ["action": "run", "source": "rule", "reason": "addressed", "at": 1_700_000_001_000.0]]
            value.merge(extra) { _, new in new }
            return try decode(TeamTriageItem.self, value)
        }
        let read = try row(["delegation": delegation()])
        XCTAssertEqual(read.delegation?.parentBotId, "sharedbot_planner")
        XCTAssertEqual(TeamDelegations.delegatedBy(read.delegation, unnamed: "Bot"), "Planner")
        XCTAssertNil(try row([:]).delegation, "a message a person sent"); XCTAssertNil(TeamDelegations.delegatedBy(nil, unnamed: "Bot"))
        XCTAssertEqual(TeamDelegations.delegatedBy(try row(["delegation": delegation(chain: [link("sharedbot_planner", "sharedbot_planner"), link("sharedbot_travel", "Travel")])]).delegation, unnamed: "Bot"), "Bot", "a Bot named by its id is the Bot")
        XCTAssertEqual(TeamDelegations.delegatedBy(try row(["delegation": delegation(parent: "sharedbot_gone")]).delegation, unnamed: "Bot"), "Bot", "a parent that is not in the chain has no name to give")
        let unreadable: [Any] = [["parentBotId": "sharedbot_planner", "chain": [Any]()] as [String: Any], ["parentBotId": "", "chain": [link("sharedbot_planner", "Planner")]] as [String: Any],
                                 ["parentBotId": "sharedbot_planner", "chain": [["sharedBotId": "a"]]] as [String: Any], ["parentBotId": 7, "chain": "Planner → Travel"] as [String: Any], "planner", 7, NSNull(), [Any]()]
        for bad in unreadable {
            let item = try row(["delegation": bad])
            XCTAssertNil(item.delegation, "\(bad)"); XCTAssertEqual(item.taskId, "task-0001", "the row is still a row")
        }
        let page = try decode(TeamTriagePage.self, ["items": [["taskId": "task-0002", "conversationId": "chat_0001", "messageId": "message-2", "createdAt": 1_700_000_000_000.0, "state": "completed", "delegation": delegation(),
                                                               "triage": ["action": "run", "source": "rule", "at": 1_700_000_001_000.0]]]])
        XCTAssertEqual(page.items.first?.delegation?.chain.count, 2, "a page carries it")
    }

    // MARK: The Bot's card

    func testAPublishedBotsCardSaysWhomItTakesRequestsFromAndWhomItMayAskByName() throws {
        let cards = try teamCards()
        func bot(_ extra: [String: Any], state: String = "published") throws -> TeamBotCard { try card("sharedbot_desk", "Desk", maintainer: dan, state: state, extra: extra) }
        XCTAssertNil(try bot([:]).acceptDelegation, "a Bot published before delegation says nothing of it")
        XCTAssertEqual(TeamDelegations.accepts(try bot([:]), cards: cards), .nobody)
        XCTAssertEqual(TeamDelegations.accepts(try bot(["acceptDelegation": "none"]), cards: cards), .nobody)
        XCTAssertEqual(TeamDelegations.accepts(try bot(["acceptDelegation": "team_bots"]), cards: cards), .teamBots)
        XCTAssertEqual(TeamDelegations.accepts(try bot(["acceptDelegation": "listed", "acceptFrom": ["sharedbot_planner", "sharedbot_travel"]]), cards: cards), .listed(["Planner", "Travel"]))
        XCTAssertEqual(TeamDelegations.accepts(try bot(["acceptDelegation": "listed", "acceptFrom": ["sharedbot_gone", "sharedbot_planner"]]), cards: cards), .listed(["Planner"]), "a Bot that is gone from the Team is not listed")
        XCTAssertEqual(TeamDelegations.accepts(try bot(["acceptDelegation": "listed", "acceptFrom": ["sharedbot_gone"]]), cards: cards), .nobody, "a list of Bots that are all gone takes nobody")
        XCTAssertEqual(TeamDelegations.accepts(try bot(["acceptDelegation": "listed"]), cards: cards), .nobody, "a list that is not there is no one")
        XCTAssertEqual(TeamDelegations.accepts(try bot(["acceptDelegation": "from_the_future"]), cards: cards), .nobody, "a code this version does not know takes nobody")
        XCTAssertEqual(TeamDelegations.delegatesTo(try bot(["delegateTo": ["sharedbot_gone", "sharedbot_planner", "sharedbot_travel"]]), cards: cards), ["Planner", "Travel"])
        // The items, as the card says them: "label: value", the names as they are, and nothing about the limit its maintainer set.
        XCTAssertEqual(TeamBotMemberText.delegation(try bot(["acceptDelegation": "team_bots"]), cards: cards),
                       [TeamBotMemberText.Item(label: "teams_bot_accepts_delegation", value: .key("teams_bot_accepts_team"))])
        XCTAssertEqual(TeamBotMemberText.delegation(try bot([:]), cards: cards), [TeamBotMemberText.Item(label: "teams_bot_accepts_delegation", value: .key("teams_bot_accepts_none"))])
        XCTAssertEqual(TeamBotMemberText.delegation(try bot(["acceptDelegation": "listed", "acceptFrom": ["sharedbot_planner"], "delegateTo": ["sharedbot_travel"]]), cards: cards),
                       [TeamBotMemberText.Item(label: "teams_bot_accepts_delegation", value: .figure("Planner")), TeamBotMemberText.Item(label: "teams_bot_delegates_to", value: .figure("Travel"))])
        let limited = TeamBotMemberText.delegation(try bot(["acceptDelegation": "team_bots", "delegation": ["daily": 9], "delegateTo": ["sharedbot_travel"]]), cards: cards)
        XCTAssertEqual(limited.count, 2); XCTAssertFalse(limited.contains { item in if case .figure(let figure) = item.value { return figure.contains("9") } else { return false } }, "the limit of its maintainer is not for a member")
        XCTAssertEqual(TeamBotMemberText.delegation(try bot(["acceptDelegation": "team_bots"], state: "paused"), cards: cards), [], "a Bot that is not published is not asked")
    }

    // MARK: Words, and what only the service sends

    func testTheDelegationWordsAreInEveryLanguageAndTheTraditionalChineseIsNotTheSimplified() throws {
        let catalogue = try Catalogue.load()
        let languages = ["en", "zh-Hans", "zh-Hant", "ja", "ko"]
        let keys = ["teams_task_waiting_bot %@", "teams_task_delegation_chain %@", "teams_triage_delegated_by %@", "teams_bot_accepts_delegation", "teams_bot_accepts_none", "teams_bot_accepts_team", "teams_bot_delegates_to"]
        for key in keys {
            let values = languages.map { catalogue.value(key, $0) }
            for (language, value) in zip(languages, values) { XCTAssertFalse((value ?? "").isEmpty, "\(key) has no text in \(language)") }
            for (language, value) in zip(languages, values) where language != "zh-Hans" && !(key == "teams_bot_accepts_none" && language == "zh-Hant") { XCTAssertNotEqual(value, values[1], "\(key) in \(language)") }
            XCTAssertNil((values.compactMap { $0 }.joined(separator: " ")).range(of: "assistant|助手|助理|アシスタント|어시스턴트", options: [.regularExpression, .caseInsensitive]), "a Bot is a Bot: \(key)")
        }
        XCTAssertEqual(catalogue.value("teams_task_waiting_bot %@", "en"), "Waiting for %@"); XCTAssertEqual(catalogue.value("teams_task_delegation_chain %@", "en"), "Delegated: %@")
        XCTAssertEqual(catalogue.value("teams_task_delegation_chain %@", "zh-Hant"), "委派：%@"); XCTAssertEqual(catalogue.value("teams_task_delegation_chain %@", "zh-Hans"), "委托：%@")
        XCTAssertTrue(TeamTriageRules.allKeys.contains("teams_triage_delegated_by %@"), "the log says it with this")
    }

    func testWhatOnlyTheServiceSendsIsRefusedFromAMember() throws {
        var path = URL(fileURLWithPath: #filePath)
        while !FileManager.default.fileExists(atPath: path.appendingPathComponent("packages/teams/contracts/execution-vectors.json").path), path.path != "/" { path.deleteLastPathComponent() }
        let fixture = Bundle(for: Self.self).url(forResource: "execution-vectors", withExtension: "json") ?? path.appendingPathComponent("packages/teams/contracts/execution-vectors.json")
        let value = try XCTUnwrap(try JSONSerialization.jsonObject(with: Data(contentsOf: fixture)) as? [String: Any])
        let rows = try XCTUnwrap(value["commands"] as? [[String: Any]])
        for name in ["reject-delegateTask-member", "reject-detachDelegation-member"] {
            let row = try XCTUnwrap(rows.first { $0["name"] as? String == name }, "\(name) is in the contract")
            XCTAssertEqual(row["valid"] as? Bool, false, name)
            XCTAssertThrowsError(try TeamExecutionContract.validate(try XCTUnwrap(row["input"] as? [String: Any])), "\(name): a member cannot send it")
        }
    }
}
