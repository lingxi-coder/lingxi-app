import Foundation

// The auto-reply log of a Bot, for the person who maintains it (Desktop's Bots tab, `packages/teams/triage-log.ts`): what the Bot's host decided about each
// message nobody asked the Bot about, and why, in codes. Newest first, a page at a time, for every Chat of the Team or for one. Only the decision and the
// reason are kept, never what a model said. This file has no UI and no network: the rules, the decoding and the paged list, which the logic tests run.

/// Where a judged message ended up, in the words of the log: it was answered, the Bot stayed silent (it declined, or the task was canceled before it spoke),
/// it failed, it is still going on, or its reply waits for the maintainer to approve it. A reply that was held and then decided is `replied` or `silent` like any
/// other and says so in `approval`.
enum TeamTriageOutcome: Equatable, Sendable { case replied, silent, failed, waiting, approval }

/// What the Bot's host decided and why. A reason or a probability this version does not know is left out; the decision is kept.
struct TeamTriageVerdict: Decodable, Equatable, Sendable {
    let action: String
    let source: String
    /// The service's clock, in epoch milliseconds.
    let at: Double
    var reason: String? = nil
    /// What the Decision service gave, from 0 to 1; only with `source` `decision`.
    var probability: Double? = nil

    init(action: String, source: String, at: Double, reason: String? = nil, probability: Double? = nil) {
        self.action = action; self.source = source; self.at = at; self.reason = reason; self.probability = probability
    }
    private enum CodingKeys: String, CodingKey { case action, source, at, reason, probability }
    init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        let action = try values.decode(String.self, forKey: .action), source = try values.decode(String.self, forKey: .source), at = try values.decode(Double.self, forKey: .at)
        guard TeamTriageRules.actions.contains(action), TeamTriageRules.sources.contains(source), TeamTriageRules.moment(at) else {
            throw DecodingError.dataCorrupted(.init(codingPath: decoder.codingPath, debugDescription: "A verdict this version cannot read."))
        }
        let reason = (try? values.decodeIfPresent(String.self, forKey: .reason)).flatMap { TeamTriageRules.reasons.contains($0) ? $0 : nil }
        let probability = (try? values.decodeIfPresent(Double.self, forKey: .probability)).flatMap { (0...1).contains($0) ? $0 : nil }
        self.init(action: action, source: source, at: at, reason: reason, probability: probability)
    }
}

/// What became of the Bot when several could answer the message (a round of Bots, possibly on other computers): `won`, `lost`, `abstained`, `promoted` or
/// `superseded`, and the tier the Bot bid when its computer did. Other Bots' bids are never in it. A tier this version does not know is left out; the outcome is kept.
struct TeamTriageRound: Equatable, Sendable {
    let outcome: String
    var tier: String? = nil
}

/// One message the Bot's host judged, as its maintainer reads the log. `state` is where the task stands now: it settles after the verdict.
/// `approval` is what became of a reply held for approval: `held`, `approved`, `rejected`, `timeout` or `superseded`; a code this version does not know is left out.
/// `round` is what became of the Bot among several that could answer; a message that was no such round, or an outcome this version does not know, has none.
/// `delegation` is there when another Bot asked for the task (Bot v3 B8): the message was no one's judgement, a Bot passed it on.
struct TeamTriageItem: Decodable, Equatable, Identifiable, Sendable {
    let taskId: String
    let conversationId: String
    let messageId: String
    let createdAt: Double
    let triage: TeamTriageVerdict
    let state: String
    var conversationTitle: String? = nil
    var approval: String? = nil
    var round: TeamTriageRound? = nil
    var delegation: TeamTaskDelegation? = nil
    var id: String { taskId }

    init(taskId: String, conversationId: String, messageId: String, createdAt: Double, triage: TeamTriageVerdict, state: String, conversationTitle: String? = nil, approval: String? = nil, round: TeamTriageRound? = nil, delegation: TeamTaskDelegation? = nil) {
        self.taskId = taskId; self.conversationId = conversationId; self.messageId = messageId; self.createdAt = createdAt
        self.triage = triage; self.state = state; self.conversationTitle = conversationTitle; self.approval = approval; self.round = round; self.delegation = delegation
    }
    private enum CodingKeys: String, CodingKey { case taskId, conversationId, messageId, createdAt, triage, state, conversationTitle, approval, round, delegation }
    private enum RoundKeys: String, CodingKey { case outcome, tier }
    /// A row is never half read: what this version needs of it is there and known, or the row is not one.
    init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        let taskId = try values.decode(String.self, forKey: .taskId), conversationId = try values.decode(String.self, forKey: .conversationId)
        let messageId = try values.decode(String.self, forKey: .messageId), createdAt = try values.decode(Double.self, forKey: .createdAt)
        let triage = try values.decode(TeamTriageVerdict.self, forKey: .triage), state = try values.decode(String.self, forKey: .state)
        guard !taskId.isEmpty, !conversationId.isEmpty, !messageId.isEmpty, TeamTriageRules.moment(createdAt), TeamTriageRules.outcomes[state] != nil else {
            throw DecodingError.dataCorrupted(.init(codingPath: decoder.codingPath, debugDescription: "A row this version cannot read."))
        }
        let title = (try? values.decodeIfPresent(String.self, forKey: .conversationTitle)).flatMap { $0.isEmpty ? nil : $0 }
        let approval = (try? values.decodeIfPresent(String.self, forKey: .approval)).flatMap { TeamTriageRules.approvals.contains($0) ? $0 : nil }
        // A round is its outcome and, when the Bot's computer bid, its tier: what cannot be read as that is no round, and never a reason to drop the row.
        let round = (try? values.nestedContainer(keyedBy: RoundKeys.self, forKey: .round)).flatMap { inner -> TeamTriageRound? in
            guard let outcome = try? inner.decode(String.self, forKey: .outcome), TeamTriageRules.roundOutcomes.contains(outcome) else { return nil }
            return TeamTriageRound(outcome: outcome, tier: (try? inner.decodeIfPresent(String.self, forKey: .tier)).flatMap { TeamTriageRules.bidTiers.contains($0) ? $0 : nil })
        }
        // A delegation is the Bot that asked and the Bots from the one the person asked: what cannot be read as that is none, and never a reason to drop the row.
        let delegation = (try? values.decodeIfPresent(TeamTaskDelegation.self, forKey: .delegation)).flatMap { $0.parentBotId.isEmpty || $0.chain.isEmpty ? nil : $0 }
        self.init(taskId: taskId, conversationId: conversationId, messageId: messageId, createdAt: createdAt, triage: triage, state: state, conversationTitle: title, approval: approval, round: round, delegation: delegation)
    }
}

/// A page of the log, newest verdict first. A row that this version cannot read is left out, and the page it came in is still shown.
struct TeamTriagePage: Decodable, Equatable, Sendable {
    let items: [TeamTriageItem]
    /// Pass it back to read the page after this one; absent at the end of the log.
    var nextCursor: String? = nil

    init(items: [TeamTriageItem], nextCursor: String? = nil) { self.items = items; self.nextCursor = nextCursor }
    private enum CodingKeys: String, CodingKey { case items, nextCursor }
    /// Takes the place of a row that was not read, so the list goes on to the next one.
    private struct Skipped: Decodable {}
    init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        var rows = try values.nestedUnkeyedContainer(forKey: .items)
        var items: [TeamTriageItem] = []
        while !rows.isAtEnd {
            if let item = try? rows.decode(TeamTriageItem.self) { items.append(item) } else { _ = try rows.decode(Skipped.self) }
        }
        let cursor = try values.decodeIfPresent(String.self, forKey: .nextCursor)
        self.init(items: items, nextCursor: cursor.flatMap { $0.isEmpty ? nil : $0 })
    }
}

enum TeamTriageRules {
    /// How many rows a page of the log has: the service gives at most `TeamExecutionContract.triageLimit`.
    static let page = 20
    /// How many pages of 50 rows are read to find the verdict of a task before it is given up.
    static let findPages = 4
    static let actions: Set<String> = ["decline", "run", "run_may_decline"]
    static let sources: Set<String> = ["quiet", "cooldown", "rule", "decision", "fallback"]
    static let reasons: Set<String> = ["quiet_hours", "cooldown", "keyword", "addressed", "below_threshold", "between_thresholds", "above_threshold", "decision_unavailable", "message_unavailable"]
    /// What became of a reply held for approval: still waiting, posted, dropped by the maintainer, dropped when its time ran out, dropped for a newer message.
    static let approvals: Set<String> = ["held", "approved", "rejected", "timeout", "superseded"]
    /// What became of a Bot among several that could answer one message, and the tiers a Bot bids (best first in the contract); the service's lists, in `execution-vectors.json`.
    static let roundOutcomes: Set<String> = ["won", "lost", "abstained", "promoted", "superseded"]
    static let bidTiers: Set<String> = ["addressed", "rule", "run", "may_decline", "decline"]
    /// Every state a task can be in, and where it leaves the judged message. A state this table does not have is a row this version cannot read.
    static let outcomes: [String: TeamTriageOutcome] = [
        "completed": .replied, "declined": .silent, "cancelled": .silent, "queued": .waiting, "running": .waiting, "waiting": .waiting, "awaiting_approval": .approval,
        "failed": .failed, "expired": .failed, "interrupted": .failed, "authorization_required": .failed, "revoked": .failed, "needs_reconciliation": .failed,
    ]
    /// A time in epoch milliseconds that a date can hold.
    static func moment(_ value: Double) -> Bool { value.isFinite && abs(value) <= 8.64e15 }
    /// A probability the Decision service gave, as the whole percent it is shown as.
    static func percent(_ probability: Double) -> Int { Int((probability * 100).rounded()) }

    // MARK: Words (the shared `teams_triage_*` keys)

    static func actionKey(_ action: String) -> String? {
        switch action {
        case "decline": "teams_triage_action_decline"
        case "run": "teams_triage_action_run"
        case "run_may_decline": "teams_triage_action_maybe"
        default: nil
        }
    }
    /// Why, in words; nothing for a reason this version does not know.
    static func reasonKey(_ reason: String?) -> String? {
        guard let reason, reasons.contains(reason) else { return nil }
        return "teams_triage_reason_" + reason
    }
    static func outcomeKey(_ outcome: TeamTriageOutcome) -> String {
        switch outcome {
        case .replied: "teams_triage_state_replied"
        case .silent: "teams_triage_state_silent"
        case .failed: "teams_triage_state_failed"
        case .waiting: "teams_triage_state_waiting"
        case .approval: "teams_task_waiting_approval"
        }
    }
    /// How a held reply came out once it was decided, in words; nothing for a reply that never was held, one that still waits (its outcome says so), or a code this version does not know.
    static func approvalKey(_ approval: String?) -> String? {
        switch approval {
        case "approved": "teams_approval_reply_approved"
        case "rejected": "teams_approval_reply_declined"
        case "timeout": "teams_triage_approval_timeout"
        case "superseded": "teams_triage_approval_superseded"
        default: nil
        }
    }
    /// What became of the Bot in a round of several, in words; nothing for a message that was not one, or an outcome this version does not know.
    static func roundKey(_ round: TeamTriageRound?) -> String? {
        guard let round, roundOutcomes.contains(round.outcome) else { return nil }
        return "teams_triage_round_" + round.outcome
    }
    /// The SF Symbol of a verdict: stay silent, answer, or leave it to the Bot.
    static func symbol(_ action: String) -> String {
        switch action {
        case "run": "play.circle"
        case "run_may_decline": "exclamationmark.circle"
        default: "pause.circle"
        }
    }
    /// Every key the log says anything with: each has text in every language.
    static let allKeys: [String] = [
        "teams_triage_title", "teams_triage_intro", "teams_triage_loading", "teams_triage_empty", "teams_triage_more", "teams_triage_chat_untitled",
        "teams_triage_action_decline", "teams_triage_action_run", "teams_triage_action_maybe",
        "teams_triage_reason_quiet_hours", "teams_triage_reason_cooldown", "teams_triage_reason_keyword", "teams_triage_reason_addressed", "teams_triage_reason_below_threshold",
        "teams_triage_reason_between_thresholds", "teams_triage_reason_above_threshold", "teams_triage_reason_decision_unavailable", "teams_triage_reason_message_unavailable",
        "teams_triage_state_replied", "teams_triage_state_silent", "teams_triage_state_failed", "teams_triage_state_waiting", "teams_task_waiting_approval",
        "teams_approval_reply_approved", "teams_approval_reply_declined", "teams_triage_approval_timeout", "teams_triage_approval_superseded",
        "teams_triage_round_won", "teams_triage_round_lost", "teams_triage_round_abstained", "teams_triage_round_promoted", "teams_triage_round_superseded",
        "teams_triage_delegated_by %@", "teams_triage_probability %lld", "teams_list_refresh", "common_retry", "common_close",
    ]

    // MARK: Whose log

    /// Whether this person maintains the Bot, by the membership they hold now.
    static func maintains(_ card: TeamBotCard, viewer: TeamMemberRef) -> Bool { card.maintainer == viewer }
    /// The Bots of a Chat whose log this person reads there: the ones they maintain that answer on their own in it, or are published to. A maintainer need not
    /// take part in a Chat to be shown it, so this does not depend on that.
    static func bots(_ conversation: TeamMessageConversation, cards: [TeamBotCard], viewer: TeamMemberRef) -> [TeamBotCard] {
        let on = conversation.autoReply?.targets ?? []
        return cards.filter { card in
            conversation.participants.contains("shared_bot:" + card.sharedBotId) && maintains(card, viewer: viewer) && (card.autoReply != nil || on.contains(card.sharedBotId))
        }
    }
    /// The Bot of a Bot DM when this person maintains it: the only one whose silences they are told the reasons of.
    static func reasonBot(_ conversation: TeamMessageConversation, cards: [TeamBotCard], viewer: TeamMemberRef) -> TeamBotCard? {
        guard conversation.kind == "bot_dm", let id = conversation.effectiveSharedBotId else { return nil }
        return cards.first { $0.sharedBotId == id && maintains($0, viewer: viewer) }
    }
    /// The tasks of this Bot that stayed silent: the ones whose reasons are asked for.
    static func silentTasks(_ tasks: [TeamTask], of sharedBotId: String) -> [String] {
        tasks.filter { $0.state == "declined" && $0.sharedBotId == sharedBotId }.map(\.taskId)
    }
    /// The line under "Did not reply" for the maintainer: the reason, then how likely the Decision service thought a reply was wanted. Nothing without a row.
    static func reasonParts(_ item: TeamTriageItem?) -> (reasonKey: String?, percent: Int?) {
        guard let item else { return (nil, nil) }
        return (reasonKey(item.triage.reason), item.triage.probability.map(percent))
    }
}

/// One read of the log: the rows after `cursor` (from the newest when there is none), at most `limit`.
typealias TeamTriageRead = @Sendable (_ cursor: String?, _ limit: Int) async throws -> TeamTriagePage

/// The list as the screen shows it.
struct TeamTriageList: Equatable, Sendable {
    /// `loading`: the first page is being read; `more`: the next one is; `failed`: the last read failed (`error` says how) and the rows are what was read before;
    /// `denied`: the service says this person does not maintain the Bot, so there is no log to show and nothing to offer.
    enum Status: Equatable, Sendable { case loading, ready, more, failed, denied }
    var status: Status = .loading
    /// Newest first, each task once.
    var items: [TeamTriageItem] = []
    /// Pass it back to read the page after the last one; `nil` at the end of the log.
    var nextCursor: String? = nil
    var error = ""
    var busy: Bool { status == .loading || status == .more }
}

/// The paged list behind the screen. A read that was overtaken by a newer one, or that answers after the list was closed, does not change what the newer
/// one showed.
@MainActor
final class TeamTriageReader {
    private(set) var list = TeamTriageList()
    private var run = 0
    private let read: TeamTriageRead
    private let limit: Int
    private let changed: (TeamTriageList) -> Void

    init(limit: Int = TeamTriageRules.page, read: @escaping TeamTriageRead, changed: @escaping (TeamTriageList) -> Void) {
        self.limit = limit; self.read = read; self.changed = changed
    }

    /// Reads the first page. Also what "refresh" and "try again" do: the list starts over.
    func load() async {
        run += 1; let mine = run
        set(TeamTriageList())
        await page(mine, after: nil)
    }
    /// Reads the page after the last one. Nothing happens while a read is going on, at the end of the log, or once the service said no.
    func more() async {
        guard let cursor = list.nextCursor, !list.busy, list.status != .denied else { return }
        run += 1; let mine = run
        var next = list; next.status = .more; next.error = ""
        set(next)
        await page(mine, after: cursor)
    }
    /// Nothing that is still being read changes the list after this.
    func close() { run += 1 }

    private func page(_ mine: Int, after cursor: String?) async {
        do {
            let page = try await read(cursor, limit)
            guard mine == run else { return }
            // Each task once: a page that repeats a row of the one before (the log moved on between the two reads) adds nothing.
            var items = cursor == nil ? [] : list.items
            var seen = Set(items.map(\.taskId))
            for item in page.items where seen.insert(item.taskId).inserted { items.append(item) }
            set(TeamTriageList(status: .ready, items: items, nextCursor: page.nextCursor, error: ""))
        } catch {
            guard mine == run, !Task.isCancelled else { return }
            let code = (error as? TeamMessageFailure)?.code ?? "SERVICE_UNAVAILABLE"
            if code == "MAINTAINER_REQUIRED" {
                set(TeamTriageList(status: .denied, items: [], nextCursor: nil, error: code))
            } else {
                var next = list; next.status = .failed; next.error = code
                set(next)
            }
        }
    }
    private func set(_ next: TeamTriageList) { list = next; changed(next) }
}

enum TeamTriageSearch {
    /// The rows of the log for these tasks, read from the newest verdict down until every task has its row, the log ends, or `pages` pages were read. A task that
    /// has no row is not in the answer. A read that fails ends the search with what it has found: the reason of a silence is a courtesy, never an error.
    static func find(_ taskIds: [String], pages: Int = TeamTriageRules.findPages, read: TeamTriageRead) async -> [String: TeamTriageItem] {
        var found: [String: TeamTriageItem] = [:], missing = Set(taskIds)
        var cursor: String?
        do {
            var index = 0
            while index < pages, !missing.isEmpty {
                let page = try await read(cursor, TeamExecutionContract.triageLimit)
                for item in page.items where missing.remove(item.taskId) != nil { found[item.taskId] = item }
                guard let next = page.nextCursor, next != cursor else { break }
                cursor = next; index += 1
            }
        } catch {}
        return found
    }
}
