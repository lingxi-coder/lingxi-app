import Foundation

// The replies of a Bot that wait for its maintainer to let them go (Bot v3 B6; Desktop's and Web's rules are `packages/teams/held-replies.ts`). A Bot can be published so
// that the reply it makes is held for its maintainer, who reads it before anyone else and lets it go as it is, changes its text, or rejects it. The service drops a held
// reply after ten minutes and, when nobody asked the Bot, when a newer message comes. It keeps a held reply where only the maintainer can read it, so the three commands
// here are the one way a client ever sees one: every other member sees no more than that the task awaits approval. This file has no UI and no network: the decoding, the
// rules and the list a screen shows, which the logic tests run.

/// The document that goes with a held reply, as the Bot made it.
struct TeamHeldDocument: Decodable, Equatable, Sendable {
    let title: String
    let body: String
}

/// A reply of one Bot that waits for its maintainer, exactly as the Bot made it. A reply this version cannot read is not one: it is never half shown, and the ones around it are.
struct TeamHeldReply: Decodable, Equatable, Identifiable, Sendable {
    let taskId: String
    let conversationId: String
    let messageId: String
    let requester: TeamMemberRef
    /// Nobody asked the Bot: it was given the message because auto-reply is on. Only such a reply is dropped by a newer message.
    let unasked: Bool
    /// The service's clock, in epoch milliseconds: when it was held, and when it is dropped if nobody decides.
    let heldAt: Double
    let expiresAt: Double
    let text: String
    var document: TeamHeldDocument? = nil
    var conversationTitle: String? = nil
    /// The Bot that made it. The service's answer says whose list it is and not whose each reply is, so the screen that asked for a Bot's replies sets this.
    var sharedBotId = ""
    var id: String { taskId }

    init(taskId: String, conversationId: String, messageId: String, requester: TeamMemberRef, unasked: Bool, heldAt: Double, expiresAt: Double, text: String,
         document: TeamHeldDocument? = nil, conversationTitle: String? = nil, sharedBotId: String = "") {
        self.taskId = taskId; self.conversationId = conversationId; self.messageId = messageId; self.requester = requester; self.unasked = unasked
        self.heldAt = heldAt; self.expiresAt = expiresAt; self.text = text; self.document = document; self.conversationTitle = conversationTitle; self.sharedBotId = sharedBotId
    }
    private enum CodingKeys: String, CodingKey { case taskId, conversationId, messageId, requester, unasked, heldAt, expiresAt, text, document, conversationTitle }
    init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        let taskId = try values.decode(String.self, forKey: .taskId), conversationId = try values.decode(String.self, forKey: .conversationId)
        let messageId = try values.decode(String.self, forKey: .messageId), requester = try values.decode(TeamMemberRef.self, forKey: .requester)
        let unasked = try values.decode(Bool.self, forKey: .unasked), heldAt = try values.decode(Double.self, forKey: .heldAt), expiresAt = try values.decode(Double.self, forKey: .expiresAt)
        let text = try values.decode(String.self, forKey: .text)
        guard !taskId.isEmpty, !conversationId.isEmpty, !messageId.isEmpty, !requester.uid.isEmpty, requester.generation >= 0,
              TeamTriageRules.moment(heldAt), TeamTriageRules.moment(expiresAt), !text.isEmpty else {
            throw DecodingError.dataCorrupted(.init(codingPath: decoder.codingPath, debugDescription: "A reply this version cannot read."))
        }
        // A document that cannot be read is no reply to approve: the maintainer would be letting go of something they did not see.
        var document: TeamHeldDocument?
        if values.contains(.document) {
            guard let read = try? values.decode(TeamHeldDocument.self, forKey: .document), !read.title.isEmpty else {
                throw DecodingError.dataCorrupted(.init(codingPath: decoder.codingPath, debugDescription: "A document this version cannot read."))
            }
            document = read
        }
        let title = (try? values.decodeIfPresent(String.self, forKey: .conversationTitle)).flatMap { $0.isEmpty ? nil : $0 }
        self.init(taskId: taskId, conversationId: conversationId, messageId: messageId, requester: requester, unasked: unasked, heldAt: heldAt, expiresAt: expiresAt, text: text,
                  document: document, conversationTitle: title)
    }
}

/// The answer to `listHeldReplies`: newest first. A reply that this version cannot read is left out, and the others are still shown; an answer that is not a list at all is
/// no empty list, it does not decode.
struct TeamHeldAnswer: Decodable, Equatable, Sendable {
    let replies: [TeamHeldReply]

    init(replies: [TeamHeldReply]) { self.replies = replies }
    private enum CodingKeys: String, CodingKey { case replies }
    /// Takes the place of a reply that was not read, so the list goes on to the next one.
    private struct Skipped: Decodable {}
    init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        var rows = try values.nestedUnkeyedContainer(forKey: .replies)
        var replies: [TeamHeldReply] = []
        while !rows.isAtEnd {
            if let reply = try? rows.decode(TeamHeldReply.self) { replies.append(reply) } else { _ = try rows.decode(Skipped.self) }
        }
        self.init(replies: replies)
    }
}

/// What a decision on a held reply came to. Approving posts it (`completed`, with the `replyId` of the message and the `artifactId` of the document when it has one);
/// rejecting drops it (`declined`, `approval_rejected`). A reply that may not be posted any more (the message it answers was edited, its requester left) is dropped too:
/// `declined`, `revoked`.
struct TeamHeldDecision: Decodable, Equatable, Sendable {
    let taskId: String
    let state: String
    /// `approval_rejected` or `revoked`; anything else is left out.
    var declineReason: String? = nil
    var replyId: String? = nil
    var artifactId: String? = nil

    init(taskId: String, state: String, declineReason: String? = nil, replyId: String? = nil, artifactId: String? = nil) {
        self.taskId = taskId; self.state = state; self.declineReason = declineReason; self.replyId = replyId; self.artifactId = artifactId
    }
    private enum CodingKeys: String, CodingKey { case taskId, state, declineReason, replyId, artifactId }
    /// An answer that says nothing of the task is not a decision.
    init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        let taskId = try values.decode(String.self, forKey: .taskId), state = try values.decode(String.self, forKey: .state)
        guard !taskId.isEmpty, !state.isEmpty else { throw DecodingError.dataCorrupted(.init(codingPath: decoder.codingPath, debugDescription: "A decision this version cannot read.")) }
        let reason = (try? values.decodeIfPresent(String.self, forKey: .declineReason)).flatMap { ["approval_rejected", "revoked"].contains($0) ? $0 : nil }
        let word = { (key: CodingKeys) in (try? values.decodeIfPresent(String.self, forKey: key)).flatMap { $0.isEmpty ? nil : $0 } }
        self.init(taskId: taskId, state: state, declineReason: reason, replyId: word(.replyId), artifactId: word(.artifactId))
    }
}

/// What sending the maintainer's own text comes to: the text to send (none when it says what the Bot said), or why it cannot be sent.
enum TeamHeldEdit: Equatable, Sendable { case send(String?), empty, tooLong }

enum TeamHeldRules {
    /// How often the time left is brought up to date, in seconds.
    static let tick = 30.0
    /// How long after a reply's end it is read again, in seconds: the service has dropped it by then, whatever its clock says.
    static let expirySlack = 0.5
    /// A reply whose time ran out while it was shown.
    static let expired = "REPLY_EXPIRED"
    /// What a client says of an approval the service took that posted nothing, because the reply could no longer be posted (`declined`, `revoked`). The service never sends it.
    static let notPosted = "REPLY_NOT_POSTED"
    /// How the service says a reply is no longer there to decide: it was decided, ran out of time or was dropped for a newer message, it is gone, or it is not this person's
    /// to decide. A decision refused with one of these is not tried again.
    static let ended: Set<String> = ["REPLY_APPROVED", "REPLY_REJECTED", "REPLY_NOT_HELD", "REPLY_EXPIRED", "REPLY_SUPERSEDED", "TASK_REQUIRED", "MAINTAINER_REQUIRED"]

    // MARK: What may be sent

    /// The text of a held reply as the maintainer left it in the editor. The service takes it trimmed, at least a character and at most `TeamExecutionContract.heldReplyBytes` of
    /// UTF-8; text that is the Bot's own once trimmed is not sent at all, so that the reply is posted as the Bot made it and is not recorded as changed.
    static func edit(draft: String, edited: String) -> TeamHeldEdit {
        let text = edited.trimmingCharacters(in: .whitespacesAndNewlines)
        if text.isEmpty { return .empty }
        if text.utf8.count > TeamExecutionContract.heldReplyBytes { return .tooLong }
        return .send(text == draft.trimmingCharacters(in: .whitespacesAndNewlines) ? nil : text)
    }
    /// The payloads of the three commands, as `TeamExecutionContract.validate` takes them.
    static func listPayload(sharedBotId: String, conversationId: String? = nil) -> [String: Any] {
        var payload: [String: Any] = ["sharedBotId": sharedBotId]
        if let conversationId { payload["conversationId"] = conversationId }
        return payload
    }
    static func approvePayload(taskId: String, text: String? = nil) -> [String: Any] {
        var payload: [String: Any] = ["taskId": taskId]
        if let text { payload["text"] = text }
        return payload
    }
    static func rejectPayload(taskId: String) -> [String: Any] { ["taskId": taskId] }

    // MARK: Whose replies, and where they are shown

    /// The Bots of a Chat whose held replies this person decides on: the ones they maintain that are in it, answering when asked or on their own. A maintainer need not take part
    /// in a Chat for the service to show them its replies, so this does not depend on that.
    static func bots(_ conversation: TeamMessageConversation, cards: [TeamBotCard], viewer: TeamMemberRef) -> [TeamBotCard] {
        cards.filter { conversation.participants.contains("shared_bot:" + $0.sharedBotId) && TeamTriageRules.maintains($0, viewer: viewer) }
    }
    /// Where the held replies of a Chat are shown. A reply of a task the Chat lists sits with that task. A reply nobody asked for has no task in the list (the service keeps such
    /// a task out of every member's), so it is a row of its own, oldest first as the stream is; an asked reply whose task is not listed yet waits for it.
    static func place(_ rows: [TeamHeldRow], listed: Set<String>) -> (byTask: [String: TeamHeldRow], orphans: [TeamHeldRow]) {
        var byTask: [String: TeamHeldRow] = [:], orphans: [TeamHeldRow] = []
        for row in rows {
            if listed.contains(row.reply.taskId) { byTask[row.reply.taskId] = row } else if row.reply.unasked { orphans.insert(row, at: 0) }
        }
        return (byTask, orphans)
    }
    /// The replies that still wait: not decided, not ended, not out of time. What the Bot card counts.
    static func waiting(_ list: TeamHeldList) -> Int {
        list.rows.filter { $0.ended == nil && $0.reply.expiresAt > list.now }.count
    }
    /// A Chat and the replies of the list that were made in it.
    struct Chat: Equatable, Identifiable, Sendable {
        let id: String
        /// `nil` for a Chat the service gave no title for.
        let title: String?
        let rows: [TeamHeldRow]
    }
    /// The list by Chat, in the order of the list: the Chat of the newest reply first, and each Chat's replies newest first.
    static func byChat(_ rows: [TeamHeldRow]) -> [Chat] {
        var result: [Chat] = []
        for row in rows {
            if let index = result.firstIndex(where: { $0.id == row.reply.conversationId }) {
                result[index] = Chat(id: result[index].id, title: result[index].title, rows: result[index].rows + [row])
            } else {
                result.append(Chat(id: row.reply.conversationId, title: row.reply.conversationTitle, rows: [row]))
            }
        }
        return result
    }

    // MARK: Words (the shared `teams_held_*` and `teams_error_reply_*` keys)

    /// What a decision that did not go through says: the codes every message command can fail with, and the three of its own (the Bot is not theirs to decide for, the reply is
    /// gone, and `notPosted`). The same codes mean something else for other commands.
    static func errorKey(_ code: String) -> String {
        switch code {
        case "REPLY_APPROVED", "REPLY_REJECTED", "REPLY_NOT_HELD": "teams_error_reply_decided"
        case "REPLY_EXPIRED": "teams_error_reply_expired"
        case "REPLY_SUPERSEDED": "teams_error_reply_superseded"
        case "TASK_REQUIRED": "teams_error_reply_gone"
        case "MAINTAINER_REQUIRED": "teams_error_held_maintainer"
        case notPosted: "teams_held_revoked"
        default: TeamsErrors.key(code)
        }
    }
    /// Every key the held replies say anything with: each has text in every language.
    static let allKeys: [String] = [
        "teams_held_title", "teams_held_expires %@", "teams_held_document %@", "teams_held_reject", "teams_held_reject_confirm", "teams_held_reject_detail", "teams_held_edit_label",
        "teams_held_empty_text", "teams_held_too_long", "teams_held_unasked", "teams_held_waiting", "teams_held_intro", "teams_held_none", "teams_held_revoked",
        "teams_error_reply_decided", "teams_error_reply_expired", "teams_error_reply_superseded", "teams_error_reply_gone", "teams_error_held_maintainer",
        "teams_approval_approve", "teams_message_edit", "teams_message_send", "common_cancel", "common_close", "common_retry", "teams_list_refresh", "teams_loading",
        "teams_triage_chat_untitled", "friends_open_chat",
    ]

    // MARK: Reading

    static func code(_ error: Error) -> String { (error as? TeamMessageFailure)?.code ?? "SERVICE_UNAVAILABLE" }
    private static func newestFirst(_ a: TeamHeldReply, _ b: TeamHeldReply) -> Bool { a.heldAt != b.heldAt ? a.heldAt > b.heldAt : a.taskId < b.taskId }
    /// The replies of these Bots that wait for this person, newest first, each tagged with its Bot. `one` reads the replies of one Bot. A Bot the service says is not theirs after
    /// all adds nothing; if that is every Bot the answer is `MAINTAINER_REQUIRED`, and any other failure is the failure of the read (of the first Bot that failed).
    static func read(_ bots: [String], one: @escaping @Sendable (String) async throws -> [TeamHeldReply]) async throws -> [TeamHeldReply] {
        let answers = await withTaskGroup(of: (Int, Result<[TeamHeldReply], Error>).self) { group in
            for (index, id) in bots.enumerated() {
                group.addTask {
                    do { return (index, .success(try await one(id).map { var reply = $0; reply.sharedBotId = id; return reply })) } catch { return (index, .failure(error)) }
                }
            }
            var all: [(Int, Result<[TeamHeldReply], Error>)] = []
            for await answer in group { all.append(answer) }
            return all.sorted { $0.0 < $1.0 }.map(\.1)
        }
        var read: [TeamHeldReply] = [], answered = false
        for answer in answers {
            switch answer {
            case .success(let replies): read += replies; answered = true
            case .failure(let error): if code(error) != "MAINTAINER_REQUIRED" { throw error }
            }
        }
        guard answered else { throw TeamMessageFailure(code: "MAINTAINER_REQUIRED") }
        return read.sorted(by: newestFirst)
    }
}

/// One held reply as a screen shows it, and what is going on with it.
struct TeamHeldRow: Equatable, Identifiable, Sendable {
    enum Busy: Equatable, Sendable { case approve, reject }
    var reply: TeamHeldReply
    /// A decision on it is on its way.
    var busy: Busy? = nil
    /// The last decision failed in a way that can be tried again: how (a code).
    var error: String? = nil
    /// It is not there to be decided any more, and why: a code of the service, `TeamHeldRules.expired` for time that ran out while it was shown, `TeamHeldRules.notPosted` for an
    /// approval that posted nothing. It stays on screen, saying so, until it is closed.
    var ended: String? = nil
    var id: String { reply.taskId }
    /// Why the card shows no decision, if it does not: it ended, or its time is up even though the service has not been asked again.
    func over(at now: Double) -> String? { ended ?? (reply.expiresAt <= now ? TeamHeldRules.expired : nil) }
}

/// The list as the screen shows it. `loading` until the first answer; `failed`: the last read failed (`error` says how) and the rows are what was read before; `denied`: the
/// service says this person maintains none of the Bots, so there is nothing to show and nothing to offer.
struct TeamHeldList: Equatable, Sendable {
    enum Status: Equatable, Sendable { case loading, ready, failed, denied }
    var status: Status = .loading
    /// Newest first.
    var rows: [TeamHeldRow] = []
    var error: String? = nil
    /// The time the rows are shown at, in epoch milliseconds: it moves on every `TeamHeldRules.tick` and at each reply's end, so that the time left is never staler than that.
    var now: Double = 0
    func row(_ taskId: String) -> TeamHeldRow? { rows.first { $0.reply.taskId == taskId } }
}

/// The decisions: each is one command, sent with the id it was given.
struct TeamHeldDecide: Sendable {
    var approve: @Sendable (_ taskId: String, _ text: String?, _ commandId: String) async throws -> TeamHeldDecision
    var reject: @Sendable (_ taskId: String, _ commandId: String) async throws -> TeamHeldDecision
}

/// The held replies a screen shows and the decisions on them. It reads the list when asked to (`load`, one read at a time; a request that comes while one is going on is answered
/// by one more read after it) and keeps the time (`keepTime`): the time left moves on every `TeamHeldRules.tick`, and the list is read again when the first reply runs out.
///
/// A reply that leaves the list stays on screen when someone is told something about it: while a decision on it is on its way, after the service refused one because it was
/// decided or ran out of time, and after its time ran out while it was shown. It is told about once (`dismiss`), and then it is gone. One that leaves for no such reason (it was
/// decided on another screen, or a newer message dropped it) simply goes. A read or a decision that answers after the board was closed changes nothing.
@MainActor
final class TeamHeldBoard {
    private struct Entry { var reply: TeamHeldReply; var busy: TeamHeldRow.Busy?; var error: String?; var ended: String? }
    private var entries: [String: Entry] = [:]
    /// The command id of the latest attempt at each reply, for the decision it was: a retry of the same decision is the same command, a different decision is a new one.
    private var attempts: [String: (decision: String, commandId: String)] = [:]
    private var status = TeamHeldList.Status.loading
    private var error: String?
    private var now: Double
    private(set) var list = TeamHeldList()
    private var pending: Task<Void, Never>?
    private var again = false
    private var closed = false
    private let read: @Sendable () async throws -> [TeamHeldReply]
    private let decide: TeamHeldDecide
    private let changed: (TeamHeldList) -> Void
    private let clock: () -> Double
    private let newId: () -> String
    private let sleep: @Sendable (Double) async throws -> Void

    init(read: @escaping @Sendable () async throws -> [TeamHeldReply], decide: TeamHeldDecide, changed: @escaping (TeamHeldList) -> Void,
         clock: @escaping () -> Double = { Date().timeIntervalSince1970 * 1000 }, newId: @escaping () -> String = { UUID().uuidString },
         sleep: @escaping @Sendable (Double) async throws -> Void = { try await Task.sleep(for: .seconds($0)) }) {
        self.read = read; self.decide = decide; self.changed = changed; self.clock = clock; self.newId = newId; self.sleep = sleep
        now = clock()
        list = build()
    }

    /// Nothing that is still on its way changes the list after this.
    func close() { closed = true }

    /// Reads the list (again).
    func load() async {
        if closed { return }
        if let pending { again = true; await pending.value; return }
        let work = Task { @MainActor [self] in
            repeat { again = false; await pass() } while again && !closed
            pending = nil
        }
        pending = work
        await work.value
    }

    /// Lets the reply go, with the maintainer's text when they changed it. Whether the service took the decision.
    func approve(_ taskId: String, text: String? = nil) async -> Bool {
        await decision(taskId, .approve, key: "approve:" + (text ?? "")) { [decide] id in
            let decision = try await decide.approve(taskId, text, id)
            // Accepted, and posted nothing: the reply could not be posted any more. It says so.
            return decision.state == "declined" && decision.declineReason == "revoked" ? TeamHeldRules.notPosted : nil
        }
    }
    /// Drops the reply. Whether the service took the decision.
    func reject(_ taskId: String) async -> Bool {
        await decision(taskId, .reject, key: "reject") { [decide] id in _ = try await decide.reject(taskId, id); return nil }
    }
    /// The person has read what became of a reply that was shown: it goes.
    func dismiss(_ taskId: String) {
        guard entries[taskId]?.ended != nil else { return }
        entries[taskId] = nil
        publish()
    }

    /// Keeps the time until the task is cancelled or the board is closed: wakes every `TeamHeldRules.tick`, sooner when the first reply that waits runs out, and then reads the
    /// list again, which no longer has it.
    func keepTime() async {
        while !closed, !Task.isCancelled {
            let current = clock()
            let next = entries.values.filter { $0.ended == nil && $0.reply.expiresAt > current }.map(\.reply.expiresAt).min()
            let wait = next.map { min(TeamHeldRules.tick, max(0, ($0 - current) / 1000) + TeamHeldRules.expirySlack) } ?? TeamHeldRules.tick
            do { try await sleep(wait) } catch { return }
            if closed || Task.isCancelled { return }
            let before = now
            now = clock()
            publish()
            // A reply ran out since the time was last brought up to date.
            if entries.values.contains(where: { $0.ended == nil && $0.reply.expiresAt > before && $0.reply.expiresAt <= now }) { await load() }
        }
    }

    private func decision(_ taskId: String, _ kind: TeamHeldRow.Busy, key: String, send: (String) async throws -> String?) async -> Bool {
        guard var entry = entries[taskId], entry.busy == nil, entry.ended == nil, !closed else { return false }
        entry.busy = kind; entry.error = nil; entries[taskId] = entry; publish()
        let attempt = attempts[taskId].flatMap { $0.decision == key ? $0 : nil } ?? (decision: key, commandId: newId())
        attempts[taskId] = attempt
        do {
            let ended = try await send(attempt.commandId)
            attempts[taskId] = nil
            if closed { return true }
            if let ended { entries[taskId]?.busy = nil; entries[taskId]?.ended = ended } else { entries[taskId] = nil }
            publish()
            Task { await load() }
            return true
        } catch {
            if closed { return false }
            let code = TeamHeldRules.code(error)
            entries[taskId]?.busy = nil
            // The reply is not there to be decided: say so, and read the list, which no longer has it. Anything else may be tried again, as the same command.
            let gone = TeamHeldRules.ended.contains(code)
            if gone { entries[taskId]?.ended = code; attempts[taskId] = nil } else { entries[taskId]?.error = code }
            publish()
            if gone { Task { await load() } }
            return false
        }
    }

    private func pass() async {
        do {
            let replies = try await read()
            if closed { return }
            merge(replies)
            status = .ready; error = nil
        } catch {
            if closed { return }
            let code = TeamHeldRules.code(error)
            if code == "MAINTAINER_REQUIRED" { entries = [:]; status = .denied } else { status = .failed }
            self.error = code
        }
        now = clock()
        publish()
    }

    private func merge(_ replies: [TeamHeldReply]) {
        let current = clock()
        let listed = Set(replies.map(\.taskId))
        for (taskId, entry) in entries where !listed.contains(taskId) && entry.busy == nil && entry.ended == nil {
            if entry.reply.expiresAt <= current { entries[taskId]?.ended = TeamHeldRules.expired } else { entries[taskId] = nil }
        }
        for reply in replies {
            if entries[reply.taskId] != nil { entries[reply.taskId]?.reply = reply } else { entries[reply.taskId] = Entry(reply: reply) }
        }
    }

    private func build() -> TeamHeldList {
        let rows = entries.values.sorted { $0.reply.heldAt != $1.reply.heldAt ? $0.reply.heldAt > $1.reply.heldAt : $0.reply.taskId < $1.reply.taskId }
            .map { TeamHeldRow(reply: $0.reply, busy: $0.busy, error: $0.error, ended: $0.ended) }
        return TeamHeldList(status: status, rows: rows, error: error, now: now)
    }
    private func publish() { list = build(); changed(list) }
}
