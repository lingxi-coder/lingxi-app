import FirebaseAuth
import FirebaseCore
import FirebaseFirestore
import XCTest
@testable import LingxiCode

final class TeamBotTests: XCTestCase {
    private let key = "bot:owner:abcdefghijklmnopqrstuv:abcdefghijklmnopqrstuv"
    private let questionJSON = """
    {"id":"bot_question_0001","seq":3,"author":{"kind":"bot","uid":"owner","botKey":"bot:owner:abcdefghijklmnopqrstuv:abcdefghijklmnopqrstuv"},"kind":"question","text":"Please clarify","replyTo":"message_0001","createdAt":null,"editedAt":null,"recalled":false,"changeSeq":3,"changeId":"bot_change_0001","triggerId":"trigger_0001","origin":{"uid":"alice","generation":7},"question":{"id":"question_0001","questions":["Which file?","Which format?"]}}
    """
    private func message() throws -> TeamMessage { try JSONDecoder().decode(TeamMessage.self, from: Data(questionJSON.utf8)) }
    private func state(_ status: String = "running", answered: Bool = false, deadline: Double = 2000) -> TeamBotState {
        TeamBotState(state: status, ownerUid: "owner", botKey: key, origin: TeamBotOrigin(uid: "alice", generation: 7), leaseUntil: deadline, questionId: "question_0001", answered: answered)
    }
    func testDisabledOwnerCardRetainsRevisionWithoutBecomingSelectable() throws {
        let raw = """
        {"key":"bot:owner:abcdefghijklmnopqrstuv:abcdefghijklmnopqrstuv","ownerUid":"owner","name":"Writer","description":"Writes reports","revision":12,"use":"all","enabled":false,"permitted":false,"online":false}
        """
        let card = try JSONDecoder().decode(TeamBotCard.self, from: Data(raw.utf8))
        XCTAssertEqual(card.revision, 12)
        XCTAssertFalse(card.enabled)
        XCTAssertFalse(card.selectable)
        let inconsistent = raw.replacingOccurrences(of: "\"permitted\":false", with: "\"permitted\":true")
        XCTAssertFalse(try JSONDecoder().decode(TeamBotCard.self, from: Data(inconsistent.utf8)).selectable, "A disabled binding cannot become a participant even with a stale permission hint")
    }
    func testMissingAndNonparticipatingCardsBlockDraftTargetsAndImplicitBotDM() throws {
        let active = TeamBotCard(key: key, ownerUid: "owner", name: "Writer", description: "", revision: 1, use: "all", enabled: true, permitted: true, online: false)
        let group = TeamMessageConversation(id: "thread_0001", kind: "group", title: "Group", participants: ["user:alice", key], lastSeq: 0, lastChangeSeq: 0, lastReadSeq: 0)
        XCTAssertEqual(TeamBotSelection.unavailable([key], cards: []), [key], "Revoked other-owner cards are omitted from the list")
        XCTAssertFalse(TeamBotSelection.canSend(in: group, selected: [key], cards: []))
        XCTAssertTrue(TeamBotSelection.canSend(in: group, selected: [], cards: []), "A human-only draft can still be queued")
        XCTAssertTrue(TeamBotSelection.canSend(in: group, selected: [key], cards: [active]), "Presence is a hint; an offline enabled Bot remains selectable")
        let removedParty = TeamMessageConversation(id: "thread_0001", kind: "group", title: "Group", participants: ["user:alice"], lastSeq: 0, lastChangeSeq: 0, lastReadSeq: 0)
        XCTAssertFalse(TeamBotSelection.canSend(in: removedParty, selected: [key], cards: [active]))
        let dm = TeamMessageConversation(id: "thread_0002", kind: "bot_dm", title: "Bot", participants: ["user:alice", key], lastSeq: 0, lastChangeSeq: 0, lastReadSeq: 0)
        XCTAssertFalse(TeamBotSelection.canSend(in: dm, selected: [], cards: []), "Implicit DM targets cannot bypass missing-card validation")
        XCTAssertTrue(TeamBotSelection.canSend(in: dm, selected: [], cards: [active]))
    }
    func testRevocationBeforeAnswerAcceptanceUnblocksSyncAndDiscard() throws {
        var intent = TeamBotIntent(action: "botAnswer", conversationId: "thread_0001", triggerId: "trigger_0001", questionId: "question_0001", answerId: "answer_00001", answers: ["Which file?": "report.txt"], botKey: nil, text: nil, attempted: true, leaseUntil: 1234)
        intent.recordFailure("SERVICE_UNAVAILABLE")
        XCTAssertFalse(intent.canDiscard(now: 2000), "An ambiguous first attempt retains its immutable body")
        XCTAssertNil(intent.error)
        intent.recordFailure("BOT_USE_REQUIRED")
        XCTAssertTrue(TeamBotIntent.isPermanentFailure("BOT_USE_REQUIRED"), "The refresh loop must continue into sync after definitive revocation")
        XCTAssertEqual(intent.error, "BOT_USE_REQUIRED")
        XCTAssertTrue(intent.canDiscard(now: 2000))
        var projection = TeamMessageProjection(version: 1, uid: "alice", teamId: "team_0001", generation: 7)
        projection.botOutbox = [intent]
        let message = try message()
        let conversation = TeamMessageConversation(id: "thread_0001", kind: "bot_dm", title: "Bot", participants: ["user:alice", key], lastSeq: 3, lastChangeSeq: 1, lastReadSeq: 0)
        projection.conversations = [conversation]
        projection.threads[conversation.id] = TeamMessageProjection.Thread(messages: [message])
        XCTAssertEqual(projection.threads[conversation.id]?.messages.first?.id, message.id)
        XCTAssertEqual(projection.botOutbox?.first?.error, "BOT_USE_REQUIRED")
    }
    func testLostAnswerAckThenNewQuestionKeepsExactRetryAndCurrentQuestion() throws {
        let old = TeamBotIntent(action: "botAnswer", conversationId: "thread_0001", triggerId: "trigger_0001", questionId: "question_0001", answerId: "answer_00001", answers: ["Which file?": "report.txt", "Which format?": "text"], botKey: nil, text: nil, attempted: true)
        let newMessage = try JSONDecoder().decode(TeamMessage.self, from: Data(questionJSON.replacingOccurrences(of: "question_0001", with: "question_0002").utf8))
        let current = TeamBotState(state: "running", ownerUid: "owner", botKey: key, origin: TeamBotOrigin(uid: "alice", generation: 7), leaseUntil: 2000, questionId: "question_0002", answered: false)
        var projection = TeamMessageProjection(version: 1, uid: "alice", teamId: "team_0001", generation: 7)
        projection.botOutbox = [old]
        projection.threads["thread_0001"] = TeamMessageProjection.Thread(messages: [newMessage])
        let restored = try JSONDecoder().decode(TeamMessageProjection.self, from: JSONEncoder().encode(projection))
        XCTAssertTrue(restored.threads.isEmpty, "History must be restored by the Firestore SDK")
        let retry = try XCTUnwrap(restored.botOutbox?.first)
        XCTAssertEqual(retry.body["questionId"] as? String, "question_0001")
        XCTAssertEqual(retry.body["answerId"] as? String, "answer_00001")
        XCTAssertEqual(retry.answers, old.answers)
        XCTAssertFalse(restored.hasPendingAnswer(for: newMessage), "An older question's pending receipt must not hide the current question")
        // A successful exact-receipt ACK after the worker advances removes only
        // the old intent; it never replaces the current botState or question.
        projection.acknowledgeBotIntent(retry.id)
        XCTAssertTrue((projection.botOutbox ?? []).isEmpty)
        XCTAssertEqual(projection.threads["thread_0001"]?.messages.first?.question?.id, "question_0002")
        XCTAssertTrue(current.canAnswer(newMessage, uid: "alice", generation: 7, now: 1000))
    }
    func testServerQuestionRoutesOnlyToOriginalHumanGeneration() throws {
        let message = try message(), state = state()
        XCTAssertTrue(state.canAnswer(message, uid: "alice", generation: 7, now: 1000))
        XCTAssertFalse(state.canAnswer(message, uid: "owner", generation: 7, now: 1000))
        XCTAssertFalse(state.canAnswer(message, uid: "bob", generation: 7, now: 1000))
        XCTAssertFalse(state.canAnswer(message, uid: "alice", generation: 8, now: 1000))
        var forged = message
        forged.origin = TeamBotOrigin(uid: "bob", generation: 7)
        XCTAssertFalse(state.canAnswer(forged, uid: "alice", generation: 7, now: 1000))
    }
    func testAnsweredExpiredAndRevokedQuestionsCannotBeAnswered() throws {
        let message = try message()
        for state in [state(answered: true), state(deadline: 1000), state("revoked"), state("completed"), state("recovery")] {
            XCTAssertFalse(state.canAnswer(message, uid: "alice", generation: 7, now: 1000))
        }
        XCTAssertTrue(state(deadline: 1000).canRecover(uid: "owner", now: 1000))
        XCTAssertTrue(state("claimed", deadline: 1000).canRecover(uid: "owner", now: 1000))
        XCTAssertFalse(state("claimed").canRecover(uid: "owner", now: 1000))
        XCTAssertFalse(state("recovery").canRecover(uid: "alice", now: 1000))
        XCTAssertFalse(state("revoked").canRecover(uid: "owner", now: 1000))
    }
    func testExplicitBotTargetIdentityMatchesTrustedBackendFixture() throws {
        let humanJSON = """
        {"id":"message_0001","seq":1,"author":{"kind":"user","uid":"alice"},"kind":"text","text":"@Bot please help","changeSeq":1,"changeId":"change_0001"}
        """
        var human = try JSONDecoder().decode(TeamMessage.self, from: Data(humanJSON.utf8))
        let group = TeamMessageConversation(id: "thread_0001", kind: "group", title: "Group", participants: ["user:alice", key], lastSeq: 1, lastChangeSeq: 1, lastReadSeq: 0)
        XCTAssertEqual(human.botTriggers(in: group), [], "Text mentions alone cannot request execution")
        human.mentions = [key]
        XCTAssertEqual(human.botTriggers(in: group), ["61efbb64d844fcca6e98e71d6188e9256ea5b9e89aa7b8a15d395a0d40f9e6ab"])
        let dm = TeamMessageConversation(id: "thread_0002", kind: "bot_dm", title: "Bot", participants: ["user:alice", key], lastSeq: 1, lastChangeSeq: 1, lastReadSeq: 0)
        human.mentions = nil
        XCTAssertEqual(human.botTriggers(in: dm), ["61efbb64d844fcca6e98e71d6188e9256ea5b9e89aa7b8a15d395a0d40f9e6ab"])
    }
    func testRestartPreservesExactAnswerAndTargetsInAtomicProjection() throws {
        let answers = ["Which file?": "report.txt", "Which format?": "plain text"]
        let intent = TeamBotIntent(action: "botAnswer", conversationId: "thread_0001", triggerId: "trigger_0001", questionId: "question_0001", answerId: "answer_00001", answers: answers, botKey: nil, text: nil, attempted: true, leaseUntil: 1234)
        var projection = TeamMessageProjection(version: 1, uid: "alice", teamId: "team_0001", generation: 7)
        projection.botOutbox = [intent]
        projection.threads["thread_0001"] = TeamMessageProjection.Thread(messages: [try message()])
        projection.creation = TeamMessageCreation(teamId: "team_0001", generation: 7, conversationId: "thread_0001", kind: "bot_dm", title: "Bot", participantUids: ["alice"], botKeys: [key])
        projection.outbox = [TeamMessageIntent(teamId: "team_0001", generation: 7, conversationId: "thread_0001", messageId: "message_0001", changeId: "change_0001", operation: "create", text: "Do this", replyTo: nil, targets: [key])]
        let restored = try JSONDecoder().decode(TeamMessageProjection.self, from: JSONEncoder().encode(projection))
        XCTAssertTrue(restored.threads.isEmpty, "History must be restored by the Firestore SDK")
        let retry = try XCTUnwrap(restored.botOutbox?.first)
        XCTAssertEqual(retry.questionId, "question_0001")
        XCTAssertEqual(retry.answerId, intent.answerId)
        XCTAssertEqual(retry.answers, answers)
        XCTAssertEqual(retry.attempted, true)
        XCTAssertEqual(restored.creation?.botKeys, [key])
        XCTAssertEqual(restored.creation?.participantUids, ["alice"])
        XCTAssertEqual(restored.outbox.first?.targets, [key])
        XCTAssertEqual(try JSONSerialization.data(withJSONObject: retry.body, options: .sortedKeys), try JSONSerialization.data(withJSONObject: intent.body, options: .sortedKeys))
        XCTAssertNil(retry.body["attempted"])
        XCTAssertNil(retry.body["leaseUntil"])
        XCTAssertNil(retry.body["botKey"], "Answers do not use the owner/executor command shape")
    }
    func testAtomicFileReopenRetainsAttemptedAnswerBody() async throws {
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent("teams-bot-tests-" + UUID().uuidString)
        let intent = TeamBotIntent(action: "botAnswer", conversationId: "thread_0001", triggerId: "trigger_0001", questionId: "question_0001", answerId: "answer_00001", answers: ["Which file?": "report.txt"], botKey: nil, text: nil, attempted: true, leaseUntil: 1234)
        let original = TeamCommandFiles(directory: directory)
        _ = try await original.update(uid: "alice", teamId: "team_0001", generation: 7) { $0.botOutbox = [intent] }
        let reopened = TeamCommandFiles(directory: directory)
        let restored = try await reopened.update(uid: "alice", teamId: "team_0001", generation: 7) { _ in }
        XCTAssertEqual(restored.botOutbox?.first?.answerId, intent.answerId)
        XCTAssertEqual(restored.botOutbox?.first?.answers, intent.answers)
        XCTAssertEqual(restored.botOutbox?.first?.attempted, true)
        let newGeneration = try await reopened.update(uid: "alice", teamId: "team_0001", generation: 8) { _ in }
        XCTAssertTrue((newGeneration.botOutbox ?? []).isEmpty, "A rejoin must not replay the previous generation's answer")
    }
    func testAnswersRequireEveryPromptAndRespectBackendUTF16Limit() throws {
        let question = try XCTUnwrap(message().question)
        XCTAssertFalse(TeamBotIntent.validAnswers(["Which file?": "a"], for: question))
        XCTAssertFalse(TeamBotIntent.validAnswers(["Which file?": " ", "Which format?": "text"], for: question))
        XCTAssertFalse(TeamBotIntent.validAnswers(["Which file?": String(repeating: "😀", count: 1001), "Which format?": "text"], for: question))
        XCTAssertTrue(TeamBotIntent.validAnswers(["Which file?": "a", "Which format?": "text"], for: question))
    }
}

@MainActor
final class TeamFirestoreCacheTests: XCTestCase {
    private let base = URL(string: "http://127.0.0.1:41791")!
    private func request(_ path: String) async throws -> [String: Any] {
        let url = base.appendingPathComponent(path).appending(queryItems: [URLQueryItem(name: "client", value: "ios")])
        let (data, response) = try await URLSession.shared.data(from: url)
        let status = (response as? HTTPURLResponse)?.statusCode
        print("TEAMS_CACHE control \(path) \(status ?? 0)")
        XCTAssertEqual(status, 200, path)
        return try XCTUnwrap(JSONSerialization.jsonObject(with: data) as? [String: Any])
    }
    private func until(_ condition: @MainActor () -> Bool) async throws {
        let deadline = Date().addingTimeInterval(20)
        while !condition() {
            if Date() >= deadline { XCTFail("SDK cache condition timed out"); throw TeamMessageFailure(code: "TEST_TIMEOUT") }
            try await Task.sleep(for: .milliseconds(20))
        }
    }
    private func delete(_ app: FirebaseApp) async {
        await withCheckedContinuation { continuation in app.delete { _ in continuation.resume() } }
    }
    func testSDKCacheReopensOfflineAndStreamsOldChangesReadMarksAndRevocation() async throws {
        guard ProcessInfo.processInfo.environment["TEAM_CACHE_EMULATOR"] == "1" else { throw XCTSkip("Explicit local emulator acceptance only") }
        let phase = ProcessInfo.processInfo.environment["TEAM_CACHE_PHASE"]
        XCTAssertTrue(phase == "warm" || phase == "offline", "Explicit warm/offline phase required")
        let fixture = try await request("fixture")
        let uid = try XCTUnwrap(fixture["uid"] as? String), teamId = try XCTUnwrap(fixture["teamId"] as? String), id = try XCTUnwrap(fixture["conversationId"] as? String)
        let customToken = try XCTUnwrap(fixture["customToken"] as? String)
        let options = FirebaseOptions(googleAppID: "1:123456789:ios:0123456789abcdef", gcmSenderID: "123456789")
        options.projectID = "demo-lingxi-teams-t0"; options.apiKey = "AIzaSyAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"
        let name = "teams-cache-acceptance"
        FirebaseApp.configure(name: name, options: options)
        let app = try XCTUnwrap(FirebaseApp.app(name: name)), auth = Auth.auth(app: app), db = Firestore.firestore(app: app)
        let team = TeamSummary(id: teamId, name: "SDK cache fixture", memberCount: 1, role: "owner", generation: 1)
        let model = TeamMessagingModel(apiRoot: base.appendingPathComponent("v1/teams/messages"), auth: auth, db: db)
        do {
            auth.useEmulator(withHost: "127.0.0.1", port: 9098); db.useEmulator(withHost: "127.0.0.1", port: 8085)
            // The SDK rejects disabling TLS while its host is still the production default.
            let settings = db.settings; settings.isSSLEnabled = false; db.settings = settings
            if phase == "warm" { _ = try await auth.signIn(withCustomToken: customToken) }
            else { XCTAssertEqual(auth.currentUser?.uid, uid); try await db.disableNetwork() }
            print("TEAMS_CACHE authenticated phase \(phase ?? "missing")")
            model.start(uid: uid, team: team); model.select(id)
            print("TEAMS_CACHE model selected")
            try await until { model.cache?.threads[id]?.messages.count == 125 }
            if phase == "warm" {
                try await until { !model.offline && model.cache?.threads[id]?.messages.first?.text == "Body 1" && model.cache?.conversations.first?.lastReadSeq == 0 }
                print("TEAMS_CACHE warm-server-snapshot 125")
            } else {
                XCTAssertTrue(model.offline)
                print("TEAMS_CACHE disk-cache-offline-process-restart 125")
                _ = try await request("edit")
                XCTAssertEqual(model.cache?.threads[id]?.messages.first?.text, "Body 1")
                try await db.enableNetwork(); try await until { model.cache?.threads[id]?.messages.first?.text == "Edited oldest" }
                XCTAssertEqual(model.cache?.threads[id]?.messages[1].text, ""); XCTAssertEqual(model.cache?.threads[id]?.messages[1].recalled, true)
                _ = try await request("read"); try await until { model.cache?.conversations.first?.lastReadSeq == 120 }
                _ = try await request("revoke"); try await until { model.revoked }; XCTAssertNil(model.cache)
                print("TEAMS_CACHE old-edit-recall-read-revocation PASS")
            }
            model.stop(); try await db.terminate(); await delete(app)
        } catch { print("TEAMS_CACHE failure count \(model.cache?.threads[id]?.messages.count ?? -1) offline \(model.offline) error \(model.error)"); model.stop(); try? await db.terminate(); await delete(app); throw error }
    }
}
