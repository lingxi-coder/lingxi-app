import FirebaseAuth
import Foundation
import Observation

/// Bot services on a phone (Desktop and Web: `packages/teams/HostedServices.tsx`): the services the person entered, a
/// service looked up by its link, and the open session with its Bot. A phone hosts no Bots and offers no service.
///
/// Nothing of a service can be watched: the open session is read through the API on a timer, every 2 s while the Bot
/// is answering and every 10 s otherwise, and not at all while the screen is away. Another account closes everything.
@MainActor @Observable
final class HostedServicesModel {
    private(set) var entered: [HostedServiceView] = []
    private(set) var listed = false
    /// A service looked up and not entered: its terms are shown, nothing is sent until the person agrees.
    var candidate: HostedServiceView?
    /// The service whose sessions are on screen.
    private(set) var open: HostedServiceView?
    private(set) var sessions: [HostedServiceSession] = []
    private(set) var session: String?
    private(set) var messages: [TeamMessage] = []
    private(set) var tasks: [TeamTask] = []
    /// The last refusal, as a code (`HostedService.errorKey` gives its words).
    var error = ""
    private(set) var busy = false

    @ObservationIgnored private var log = HostedServiceLog()
    @ObservationIgnored private var poll: Task<Void, Never>?
    @ObservationIgnored private var lifecycle = 0
    @ObservationIgnored private var authListener: NSObjectProtocol?
    @ObservationIgnored private var uid = ""
    private let auth: Auth
    private let base: URL
    private let http = URLSession(configuration: .ephemeral, delegate: TeamMessageRedirectPolicy(), delegateQueue: nil)

    init(base: URL = URL(string: "https://lingxios.net")!, auth: Auth = Auth.auth()) {
        self.base = base; self.auth = auth
    }

    var signedIn: Bool { !uid.isEmpty }
    var viewer: String { uid }
    /// A request of the open session is under way: the session is read quickly.
    var live: Bool { tasks.contains { !$0.terminal } }
    /// The Bot's question to the viewer that the next message answers.
    var answering: Bool {
        guard let open, let session, let generation = open.generation else { return false }
        return tasks.contains { $0.conversationId == session && TeamTaskActions.answerableWait($0, viewer: TeamMemberRef(uid: uid, generation: generation), pendingWaitIds: []) != nil }
    }
    var approval: TeamTask? { open?.generation.flatMap { HostedServiceCommands.approval(in: tasks, uid: uid, generation: $0) } }

    /// The screen is on: the account is followed, the list is read, and the link the person followed is looked up.
    func start(link: String? = nil) {
        if authListener == nil {
            authListener = auth.addStateDidChangeListener { [weak self] _, user in
                let current = user?.uid ?? ""
                Task { @MainActor in guard let self, current != self.uid else { return }; self.reset(uid: current) }
            }
        }
        if uid != (auth.currentUser?.uid ?? "") { reset(uid: auth.currentUser?.uid ?? "") }
        Task {
            await attempt { try await self.refresh() }
            if let link { await lookup(link) }
        }
        if open != nil { watch() }
    }
    /// The screen is away: nothing is read until it is back.
    func pause() { poll?.cancel(); poll = nil }
    func stop() {
        pause()
        if let authListener { auth.removeStateDidChangeListener(authListener) }
        authListener = nil
    }
    private func reset(uid: String) {
        pause(); lifecycle += 1; self.uid = uid
        entered = []; listed = false; candidate = nil; closeService(); error = ""; busy = false
    }

    // MARK: The service

    func refresh() async throws {
        let list: HostedServiceList = try await call("/v1/services", try HostedServiceCommands.service("listServices"))
        entered = list.entered; listed = true
    }
    /// Looks a service up by a link or an id. Looking enters nothing.
    func lookup(_ input: String) async {
        guard let serviceId = HostedService.serviceId(from: input) else { error = "INVALID_SERVICE_LINK"; return }
        await attempt {
            switch try await self.call("/v1/services", try HostedServiceCommands.service("getService", serviceId: serviceId)) as HostedServiceLookup {
            case .mine: self.candidate = nil; self.error = "OWN_SERVICE"
            case let .service(view): if view.entered { self.candidate = nil; self.show(view) } else { self.candidate = view }
            }
        }
    }
    /// The person agreed to the terms they were shown.
    func enter() async {
        guard let candidate else { return }
        await attempt {
            let entry: HostedServiceEntry = try await self.call("/v1/services", try HostedServiceCommands.service("enterService", serviceId: candidate.serviceId, more: ["acknowledged": true]))
            var view = entry.service; view.entered = true; view.generation = entry.generation
            self.candidate = nil; self.show(view)
            try? await self.refresh()
        }
    }
    func show(_ view: HostedServiceView) {
        closeService(); open = view
        Task { await attempt { try await self.loadSessions(selectLast: true) } }
        watch()
    }
    func closeService() {
        pause(); open = nil; sessions = []; session = nil; messages = []; tasks = []; log = HostedServiceLog()
    }
    func leave() async -> Bool {
        guard let open else { return false }
        return await attempt {
            let _: HostedServiceAck = try await self.call("/v1/services", try HostedServiceCommands.service("leaveService", serviceId: open.serviceId))
            self.closeService(); try? await self.refresh()
        }
    }
    func report(reason: String, text: String, withSession: Bool, reportId: String) async -> Bool {
        guard let open else { return false }
        return await attempt {
            let _: HostedServiceAck = try await self.call("/v1/services", try HostedServiceCommands.report(serviceId: open.serviceId, reportId: reportId, reason: reason, text: text,
                                                                                                         sessionId: withSession ? self.session : nil))
        }
    }

    // MARK: Sessions

    private func loadSessions(selectLast: Bool) async throws {
        guard let open, let generation = open.generation else { return }
        let found: HostedServiceSessions = try await call("/v1/teams/messages", try HostedServiceCommands.sessions(serviceId: open.serviceId, generation: generation))
        guard self.open?.serviceId == open.serviceId else { return }
        sessions = found.conversations
        if selectLast, session == nil, let last = found.conversations.last { select(last.id) }
    }
    func select(_ id: String) {
        guard id != session else { return }
        session = id; log = HostedServiceLog(); messages = []; tasks = []
        watch()
    }
    func newSession(title: String) async -> Bool {
        await attempt { _ = try await self.makeSession(title: title) }
    }
    private func makeSession(title: String) async throws -> String {
        guard let open, let generation = open.generation else { throw TeamMessageFailure(code: "SERVICE_ENTRY_REQUIRED") }
        let id = UUID().uuidString
        let _: HostedServiceAck = try await call("/v1/teams/messages", try HostedServiceCommands.newSession(serviceId: open.serviceId, generation: generation, sharedBotId: open.sharedBotId,
                                                                                                        sessionId: id, title: title, uid: uid))
        // The session exists from here on, whatever becomes of the request: a refused request is sent again into it.
        try await loadSessions(selectLast: false); select(id)
        return id
    }
    /// Sends what was typed: the answer to the Bot's question, else a new request. Returns whether it was taken; the
    /// text stays in the box when it was not.
    func send(_ text: String) async -> Bool {
        let body = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !body.isEmpty, let open, let generation = open.generation else { return false }
        return await attempt {
            var id = self.session ?? ""
            if id.isEmpty { id = try await self.makeSession(title: HostedService.sessionTitle(body)) }
            let _: HostedServiceAck = try await self.call("/v1/teams/messages", try HostedServiceCommands.send(serviceId: open.serviceId, generation: generation, sharedBotId: open.sharedBotId,
                                                                                                          sessionId: id, text: body, tasks: self.tasks, uid: self.uid))
            await self.readNow()
        }
    }
    func decide(approve: Bool) async {
        guard let open, let generation = open.generation, let session, let task = approval, let wait = task.wait else { return }
        await attempt {
            let _: HostedServiceAck = try await self.call("/v1/teams/messages", try HostedServiceCommands.task(serviceId: open.serviceId, generation: generation, action: "decideWait", payload: [
                "conversationId": session, "waitId": wait.waitId, "messageId": UUID().uuidString, "changeId": UUID().uuidString, "decision": approve ? "approve" : "deny"]))
            await self.readNow()
        }
    }
    func cancel(_ taskId: String) async {
        guard let open, let generation = open.generation else { return }
        await attempt {
            do { let _: HostedServiceAck = try await self.call("/v1/teams/messages", try HostedServiceCommands.task(serviceId: open.serviceId, generation: generation, action: "cancelTask", payload: ["taskId": taskId])) }
            catch let failure as TeamMessageFailure where failure.code == "TASK_FINISHED" {}
            await self.readNow()
        }
    }

    /// Reads the open session on a timer while the screen is on.
    private func watch() {
        pause()
        guard open != nil else { return }
        let life = lifecycle
        poll = Task { [weak self] in
            while !Task.isCancelled {
                guard let self, self.lifecycle == life else { return }
                await self.read()
                let wait: Duration = self.live ? .seconds(2) : .seconds(10)
                do { try await Task.sleep(for: wait) } catch { return }
            }
        }
    }
    private func readNow() async { await read() }
    private func read() async {
        guard let open, let generation = open.generation else { return }
        do {
            if let session {
                var next = log
                while try await next.apply(call("/v1/teams/messages", try HostedServiceCommands.sync(serviceId: open.serviceId, generation: generation, sessionId: session, after: next.after)),
                                           uid: uid, sharedBotId: open.sharedBotId) {}
                let listed: HostedServiceTasks = try await call("/v1/teams/messages", try HostedServiceCommands.task(serviceId: open.serviceId, generation: generation, action: "listTasks",
                                                                                                                  payload: ["conversationId": session]))
                guard self.open?.serviceId == open.serviceId, self.session == session else { return }
                log = next; messages = next.messages; tasks = listed.tasks.sorted { $0.createdAt < $1.createdAt }
            }
            switch try await call("/v1/services", try HostedServiceCommands.service("getService", serviceId: open.serviceId)) as HostedServiceLookup {
            case let .service(now) where self.open?.serviceId == open.serviceId:
                var view = now; view.generation = view.generation ?? generation; self.open = view
            default: break
            }
        } catch {
            // A broken page is read again from the start next time.
            if (error as? TeamMessageFailure)?.code == "CHANGE_GAP" || (error as? TeamMessageFailure)?.code == "INVALID_DATA" { log = HostedServiceLog() }
            fail(error)
        }
    }

    // MARK: The API

    @discardableResult
    private func attempt(_ step: @escaping () async throws -> Void) async -> Bool {
        busy = true; error = ""
        defer { busy = false }
        do { try await step(); return true } catch { fail(error); return false }
    }
    private func fail(_ failure: Error) {
        let code = (failure as? TeamMessageFailure)?.code ?? (failure is DecodingError ? "INVALID_DATA" : "SERVICE_UNAVAILABLE")
        error = code
        // Out of the service: its sessions are no longer shown.
        if open != nil, HostedService.isAccessLoss(code) { closeService(); Task { try? await self.refresh() } }
    }
    private func call<T: Decodable>(_ path: String, _ body: [String: Any]) async throws -> T {
        let life = lifecycle
        guard let user = auth.currentUser, user.uid == uid, !uid.isEmpty else { throw TeamMessageFailure(code: "SIGN_IN_REQUIRED") }
        let token = try await user.getIDToken()
        var request = URLRequest(url: base.appendingPathComponent(path))
        request.httpMethod = "POST"; request.timeoutInterval = 20; request.cachePolicy = .reloadIgnoringLocalCacheData
        request.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization"); request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        request.httpBody = try JSONSerialization.data(withJSONObject: body)
        let (data, response) = try await http.data(for: request)
        // An answer for an account that is no longer signed in here is dropped.
        guard life == lifecycle, auth.currentUser?.uid == uid else { throw TeamMessageFailure(code: "SIGN_IN_REQUIRED") }
        guard let status = (response as? HTTPURLResponse)?.statusCode, (200...299).contains(status) else {
            let object = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any]
            throw TeamMessageFailure(code: (object?["error"] as? [String: Any])?["code"] as? String ?? "SERVICE_UNAVAILABLE")
        }
        return try JSONDecoder().decode(T.self, from: data)
    }
}
