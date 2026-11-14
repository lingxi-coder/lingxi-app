import Foundation
import XCTest
@testable import LingxiCode

/// Bot services (Bots offered outside Teams): the rules a phone applies before it asks the service anything.
final class HostedServiceTests: XCTestCase {
    private let serviceId = "s_travel_0001", sharedBotId = "sharedbot_01", uid = "caller"

    func testALinkOrAnIdNamesAServiceAndNothingElseDoes() {
        XCTAssertEqual(HostedService.serviceId(from: " https://lingxios.net/services/\(serviceId) "), serviceId)
        XCTAssertEqual(HostedService.serviceId(from: "https://lingxios.net/services/\(serviceId)/"), serviceId)
        XCTAssertEqual(HostedService.serviceId(from: serviceId), serviceId)
        for value in ["", "team00001", "https://lingxios.net/teams/join?code=abc", "https://lingxios.net/services/\(serviceId)/extra",
                      "https://lingxios.net/services/team00001", "/services/\(serviceId)", "services/\(serviceId)", "s_ab", "S_travel_0001"] {
            XCTAssertNil(HostedService.serviceId(from: value), value)
        }
    }

    func testEveryAvailabilityRefusalAndEndedRequestHasWordsInEveryLanguage() throws {
        var keys = ["available", "disabled", "not_ready", "host_offline", "service_quota_exhausted", "user_quota_exhausted", "withdrawn"].map(HostedService.availabilityKey)
        XCTAssertEqual(Set(keys).count, 7)
        let codes = ["NOT_FOUND", "SERVICE_ENTRY_REQUIRED", "SERVICE_ACCESS_REQUIRED", "STALE_MEMBERSHIP", "SERVICE_WITHDRAWN", "SERVICE_DISABLED", "SERVICE_USER_BUSY", "SERVICE_BUSY",
                     "REQUEST_QUOTA_EXCEEDED", "HOST_UNAVAILABLE", "SERVICE_SESSION_LIMIT", "REPORT_LIMIT", "SERVICE_UNAVAILABLE", "INVALID_SERVICE_LINK", "OWN_SERVICE"]
        XCTAssertEqual(Set(codes.map(HostedService.errorKey)).count, codes.count)
        XCTAssertTrue(codes.map(HostedService.errorKey).allSatisfy { $0.hasPrefix("services_") })
        // A code a service has no word of its own for is said as Teams says it.
        XCTAssertEqual(HostedService.errorKey("INVALID_INPUT"), TeamsErrors.key("INVALID_INPUT"))
        XCTAssertEqual(HostedService.errorKey("SOMETHING_NEW"), "teams_error")
        let ended = ["failed", "expired", "interrupted", "revoked", "cancelled", "authorization_required", "needs_reconciliation"]
        XCTAssertEqual(Set(ended.compactMap(HostedService.endedKey)).count, ended.count)
        for state in ["queued", "running", "waiting", "completed"] { XCTAssertNil(HostedService.endedKey(state)) }
        keys += codes.map(HostedService.errorKey) + ended.compactMap(HostedService.endedKey)
        keys += ["services_task_queued", "services_task_running", "services_task_approval", "services_task_question"] + HostedService.reportReasons.map { "services_report_\($0)" }
        var path = URL(fileURLWithPath: #filePath)
        while !FileManager.default.fileExists(atPath: path.appendingPathComponent("Resources/Localizable.xcstrings").path), path.path != "/" { path.deleteLastPathComponent() }
        let catalogue = try XCTUnwrap(try JSONSerialization.jsonObject(with: Data(contentsOf: path.appendingPathComponent("Resources/Localizable.xcstrings"))) as? [String: Any])
        let strings = try XCTUnwrap(catalogue["strings"] as? [String: Any])
        for key in keys {
            let entry = try XCTUnwrap(strings[key] as? [String: Any], key)
            XCTAssertEqual(Set(try XCTUnwrap(entry["localizations"] as? [String: Any], key).keys), ["en", "ja", "ko", "zh-Hans", "zh-Hant"], key)
        }
        XCTAssertTrue(["NOT_FOUND", "SERVICE_ENTRY_REQUIRED", "SERVICE_ACCESS_REQUIRED", "STALE_MEMBERSHIP", "SERVICE_WITHDRAWN"].allSatisfy(HostedService.isAccessLoss))
        XCTAssertFalse(["SERVICE_DISABLED", "SERVICE_BUSY", "SERVICE_USER_BUSY", "REQUEST_QUOTA_EXCEEDED", "HOST_UNAVAILABLE", "SERVICE_UNAVAILABLE"].contains(where: HostedService.isAccessLoss))
    }

    func testAServiceAndThePublishersOwnAreToldApart() throws {
        let service = #"{"serviceId":"s_travel_0001","name":"Travel helper","description":"Answers travel questions","publisherName":"Pat","sharedBotId":"sharedbot_01","availability":"host_offline","entered":false,"cost":"publisher","visibility":"requester_and_publisher","retention":"Kept for 30 days.","limits":{"userConcurrent":1,"userDaily":20,"userDailyLeft":18}}"#
        guard case let .service(view) = try JSONDecoder().decode(HostedServiceLookup.self, from: Data(service.utf8)) else { return XCTFail("a service") }
        XCTAssertEqual([view.name, view.publisherName, view.retention, view.availability], ["Travel helper", "Pat", "Kept for 30 days.", "host_offline"])
        XCTAssertEqual([view.limits.userConcurrent, view.limits.userDaily, view.limits.userDailyLeft], [1, 20, 18])
        XCTAssertFalse(view.entered); XCTAssertNil(view.generation); XCTAssertFalse(view.available); XCTAssertFalse(view.withdrawn)
        // A service whose Bot is not published yet has no daily number.
        let bare = service.replacingOccurrences(of: #","userDaily":20,"userDailyLeft":18"#, with: "")
        guard case let .service(early) = try JSONDecoder().decode(HostedServiceLookup.self, from: Data(bare.utf8)) else { return XCTFail("a service") }
        XCTAssertNil(early.limits.userDaily)
        let mine = #"{"serviceId":"s_travel_0001","state":"active","online":true,"publisher":"caller","publisherName":"Me","sharedBotId":"sharedbot_01","name":"Travel helper","description":"","audience":{"mode":"public"},"limits":{"userConcurrent":1,"serviceConcurrent":4},"retention":"x","cost":"publisher","visibility":"requester_and_publisher","revision":1,"createdAt":1,"updatedAt":1}"#
        XCTAssertEqual(try JSONDecoder().decode(HostedServiceLookup.self, from: Data(mine.utf8)), .mine)
        let entry = try JSONDecoder().decode(HostedServiceEntry.self, from: Data(#"{"generation":3,"service":\#(service.replacingOccurrences(of: #""entered":false"#, with: #""entered":true,"generation":3"#))}"#.utf8))
        XCTAssertEqual([entry.generation, entry.service.generation], [3, 3]); XCTAssertTrue(entry.service.entered)
    }

    private func message(_ id: String, seq: Int, change: Int, text: String, bot: Bool = false, author: String? = nil) -> String {
        let who = bot ? #"{"kind":"shared_bot","sharedBotId":"\#(author ?? sharedBotId)"}"# : #"{"kind":"user","uid":"\#(author ?? uid)"}"#
        return #"{"id":"\#(id)","seq":\#(seq),"author":\#(who),"kind":"text","text":"\#(text)","replyTo":null,"createdAt":"2026-11-13T09:00:00.000Z","editedAt":null,"changeSeq":\#(change),"changeId":"change_\#(change)"}"#
    }
    private func page(_ changes: [(Int, String)], _ messages: [String], more: Bool = false) throws -> HostedServiceSync {
        let log = changes.map { #"{"seq":\#($0.0),"messageId":"\#($0.1)"}"# }.joined(separator: ",")
        return try JSONDecoder().decode(HostedServiceSync.self, from: Data(#"{"conversation":{"id":"chat_0001"},"changes":[\#(log)],"messages":[\#(messages.joined(separator: ","))],"hasMore":\#(more)}"#.utf8))
    }

    func testASessionIsReadByItsChangeLogAndABrokenPageIsRefusedWhole() throws {
        var log = HostedServiceLog()
        XCTAssertTrue(try log.apply(try page([(1, "m1"), (2, "m2")], [message("m1", seq: 1, change: 1, text: "Where to?"), message("m2", seq: 2, change: 2, text: "Lisbon", bot: true)], more: true), uid: uid, sharedBotId: sharedBotId))
        XCTAssertEqual(log.after, 2); XCTAssertEqual(log.messages.map(\.text), ["Where to?", "Lisbon"])
        // A later change to an earlier message replaces it; the order stays the messages' own.
        XCTAssertFalse(try log.apply(try page([(3, "m1")], [message("m1", seq: 1, change: 3, text: "Where to next?")]), uid: uid, sharedBotId: sharedBotId))
        XCTAssertEqual(log.after, 3); XCTAssertEqual(log.messages.map(\.text), ["Where to next?", "Lisbon"])
        // Nothing new: the cursor stays and nothing more is asked for, whatever the page says.
        XCTAssertFalse(try log.apply(try page([], [], more: true), uid: uid, sharedBotId: sharedBotId))
        let before = log
        let broken: [(HostedServiceSync, String)] = [
            (try page([(5, "m3")], [message("m3", seq: 3, change: 5, text: "skipped")]), "CHANGE_GAP"),
            (try page([(4, "m3")], []), "CHANGE_GAP"),
            (try page([(4, "m3"), (5, "m4")], [message("m3", seq: 3, change: 4, text: "ok"), message("m4", seq: 4, change: 5, text: "someone else", author: "stranger")]), "INVALID_DATA"),
            (try page([(4, "m3")], [message("m3", seq: 3, change: 4, text: "another Bot", bot: true, author: "sharedbot_99")]), "INVALID_DATA"),
        ]
        for (page, code) in broken {
            XCTAssertThrowsError(try log.apply(page, uid: uid, sharedBotId: sharedBotId)) { XCTAssertEqual(($0 as? TeamMessageFailure)?.code, code) }
            XCTAssertEqual(log, before, "a refused page changes nothing")
        }
    }

    private func task(_ state: String, wait: String? = nil, session: String = "chat_0001", recipient: String? = nil) throws -> TeamTask {
        let waiting = wait.map { #","wait":{"waitId":"wait_0001","kind":"\#($0)","recipient":{"uid":"\#(recipient ?? uid)","generation":2},"expiresAt":9e15,"messageId":"message_9"}"# } ?? ""
        return try JSONDecoder().decode(TeamTask.self, from: Data(#"{"taskId":"task_0001","conversationId":"\#(session)","sharedBotId":"sharedbot_01","requester":{"uid":"caller","generation":2},"state":"\#(state)","createdAt":1,"deadline":9e15,"replyId":null,"artifactId":null\#(waiting)}"#.utf8))
    }

    func testCommandsNameTheServiceNeverATeamAndAreCheckedLikeTheirTeamCounterparts() throws {
        let sent = try HostedServiceCommands.send(serviceId: serviceId, generation: 2, sharedBotId: sharedBotId, sessionId: "chat_0001", text: "Where to?", tasks: [], uid: uid,
                                                  messageId: "message_0001", changeId: "change_0001", commandId: "command_0001")
        XCTAssertEqual(Set(sent.keys), ["schemaVersion", "commandId", "serviceId", "generation", "action", "payload"])
        XCTAssertEqual(sent["serviceId"] as? String, serviceId); XCTAssertEqual(sent["action"] as? String, "submit"); XCTAssertEqual(sent["generation"] as? Int, 2)
        let payload = try XCTUnwrap(sent["payload"] as? [String: Any])
        XCTAssertEqual(payload["targets"] as? [String], [sharedBotId]); XCTAssertEqual(payload["text"] as? String, "Where to?"); XCTAssertNil(payload["waitId"])
        // The Bot's question to the viewer, in this session, is answered; anything else is a new request.
        let answered = try HostedServiceCommands.send(serviceId: serviceId, generation: 2, sharedBotId: sharedBotId, sessionId: "chat_0001", text: "Two", tasks: [try task("waiting", wait: "human_input")], uid: uid)
        XCTAssertEqual(answered["action"] as? String, "answerWait")
        XCTAssertEqual((answered["payload"] as? [String: Any])?["waitId"] as? String, "wait_0001"); XCTAssertNil((answered["payload"] as? [String: Any])?["targets"])
        for other in [try task("waiting", wait: "tool_approval"), try task("waiting", wait: "human_input", session: "chat_0002"), try task("waiting", wait: "human_input", recipient: "someone"), try task("running")] {
            XCTAssertEqual(try HostedServiceCommands.send(serviceId: serviceId, generation: 2, sharedBotId: sharedBotId, sessionId: "chat_0001", text: "Two", tasks: [other], uid: uid)["action"] as? String, "submit")
        }
        XCTAssertEqual(HostedServiceCommands.approval(in: [try task("running"), try task("waiting", wait: "tool_approval")], uid: uid, generation: 2)?.wait?.waitId, "wait_0001")
        XCTAssertNil(HostedServiceCommands.approval(in: [try task("waiting", wait: "human_input"), try task("waiting", wait: "tool_approval", recipient: "someone")], uid: uid, generation: 2))
        let session = try HostedServiceCommands.newSession(serviceId: serviceId, generation: 2, sharedBotId: sharedBotId, sessionId: "chat_0001", title: HostedService.sessionTitle(String(repeating: "a", count: 90)), uid: uid)
        let made = try XCTUnwrap(session["payload"] as? [String: Any])
        XCTAssertEqual(made["kind"] as? String, "bot_dm"); XCTAssertEqual((made["title"] as? String)?.count, 60)
        XCTAssertEqual(made["participantUids"] as? [String], [uid]); XCTAssertEqual(made["sharedBotIds"] as? [String], [sharedBotId])
        XCTAssertNil(session["teamId"])
        // What the service would refuse is not sent: an action that is not a caller's, a Team's id, an empty request.
        XCTAssertThrowsError(try HostedServiceCommands.task(serviceId: serviceId, generation: 2, action: "publishSharedBot", payload: [:]))
        XCTAssertThrowsError(try HostedServiceCommands.task(serviceId: serviceId, generation: 2, action: "readArtifact", payload: ["artifactId": "artifact_0001"]))
        XCTAssertThrowsError(try HostedServiceCommands.task(serviceId: "team_0001", generation: 2, action: "cancelTask", payload: ["taskId": "task_0001"]))
        XCTAssertThrowsError(try HostedServiceCommands.task(serviceId: serviceId, generation: 0, action: "cancelTask", payload: ["taskId": "task_0001"]))
        XCTAssertThrowsError(try HostedServiceCommands.send(serviceId: serviceId, generation: 2, sharedBotId: sharedBotId, sessionId: "chat_0001", text: "", tasks: [], uid: uid))
        XCTAssertEqual(try HostedServiceCommands.sync(serviceId: serviceId, generation: 2, sessionId: "chat_0001", after: 4) as NSDictionary,
                       ["action": "sync", "serviceId": serviceId, "generation": 2, "conversationId": "chat_0001", "after": 4])
        XCTAssertEqual(try HostedServiceCommands.sessions(serviceId: serviceId, generation: 2) as NSDictionary, ["action": "list", "serviceId": serviceId, "generation": 2])
        XCTAssertThrowsError(try HostedServiceCommands.sessions(serviceId: "team_0001", generation: 2))
    }

    func testServiceCommandsCarryTheAcknowledgementAndAReportOnlyWhatWasChosen() throws {
        XCTAssertEqual(try HostedServiceCommands.service("listServices") as NSDictionary, ["action": "listServices"])
        XCTAssertEqual(try HostedServiceCommands.service("enterService", serviceId: serviceId, more: ["acknowledged": true]) as NSDictionary, ["action": "enterService", "serviceId": serviceId, "acknowledged": true])
        XCTAssertEqual(try HostedServiceCommands.report(serviceId: serviceId, reportId: "report_0001", reason: "privacy", text: "  it asked for my address \n", sessionId: nil) as NSDictionary,
                       ["action": "reportService", "serviceId": serviceId, "reportId": "report_0001", "reason": "privacy", "text": "it asked for my address"])
        XCTAssertEqual(try HostedServiceCommands.report(serviceId: serviceId, reportId: "report_0001", reason: "other", text: "", sessionId: "chat_0001")["sessionId"] as? String, "chat_0001")
        XCTAssertThrowsError(try HostedServiceCommands.report(serviceId: serviceId, reportId: "report_0001", reason: "boring", text: "", sessionId: nil))
        XCTAssertThrowsError(try HostedServiceCommands.report(serviceId: serviceId, reportId: "report_0001", reason: "spam", text: String(repeating: "a", count: 1001), sessionId: nil))
        // A phone offers nothing: the publisher's commands are not ones it can make.
        for action in ["createService", "updateService", "setServiceOnline", "withdrawService"] { XCTAssertThrowsError(try HostedServiceCommands.service(action, serviceId: serviceId)) }
        XCTAssertThrowsError(try HostedServiceCommands.service("getService")); XCTAssertThrowsError(try HostedServiceCommands.service("listServices", serviceId: serviceId))
        XCTAssertThrowsError(try HostedServiceCommands.service("getService", serviceId: "team_0001"))
    }

    func testTheLineUnderTheMessagesSaysWhatARequestIsDoing() throws {
        XCTAssertEqual(HostedService.taskKey(try task("queued")), "services_task_queued")
        XCTAssertEqual(HostedService.taskKey(try task("running")), "services_task_running")
        XCTAssertEqual(HostedService.taskKey(try task("waiting", wait: "human_input")), "services_task_question")
        XCTAssertEqual(HostedService.taskKey(try task("waiting", wait: "tool_approval")), "services_task_approval")
        XCTAssertEqual(HostedService.taskKey(try task("interrupted")), "services_task_interrupted")
        XCTAssertNil(HostedService.taskKey(try task("completed")))
    }
}
