import FirebaseAuth
import FirebaseFirestore
import Foundation
import Observation

struct TeamSummary: Decodable, Identifiable {
    let id: String
    let name: String
    let memberCount: Int
    let role: String
    let generation: Int
}
struct TeamMember: Decodable, Identifiable {
    let uid: String
    let displayName: String
    let role: String
    let generation: Int
    var id: String { uid }
}
struct TeamInvite: Decodable, Identifiable {
    let id: String
    let role: String
    let expiresAt: String
    let uses: Int
    let maxUses: Int
    let revoked: Bool
}
struct TeamDetail: Decodable { let team: TeamSummary; let members: [TeamMember]; let invites: [TeamInvite] }
private struct TeamsList: Decodable { let teams: [TeamSummary] }
private struct TeamResult: Decodable { let teamId: String?; let code: String? }


/// HTTP responses are authoritative. SDK listeners only invalidate views; cached
/// snapshots never grant membership or repopulate data after an account change.
@MainActor @Observable
final class TeamsModel {
    private(set) var uid: String?
    private(set) var teams: [TeamSummary] = []
    private(set) var detail: TeamDetail?
    private(set) var busy = false
    private(set) var loading = false
    private(set) var errorKey: String?
    private(set) var invitationLink = ""
    private(set) var selectedID = ""
    @ObservationIgnored private var authListener: AuthStateDidChangeListenerHandle?
    @ObservationIgnored private var listeners: [String: ListenerRegistration] = [:]
    @ObservationIgnored private var refreshTask: Task<Void, Never>?
    @ObservationIgnored private var epoch = 0
    @ObservationIgnored private var revision = 0
    @ObservationIgnored private var authority = 0
    @ObservationIgnored private var started = false
    @ObservationIgnored private var lifecycle = 0
    private let baseURL = "https://lingxios.net/v1/teams"

    func start() {
        guard !started else { return }
        started = true; lifecycle += 1
        let lifecycle = lifecycle
        authListener = Auth.auth().addStateDidChangeListener { [weak self] _, user in
            let uid = user?.uid
            Task { @MainActor in
                guard let self, self.started, self.lifecycle == lifecycle else { return }
                self.accountChanged(uid)
            }
        }
        accountChanged(Auth.auth().currentUser?.uid)
    }
    func stop() {
        started = false; lifecycle += 1
        if let authListener { Auth.auth().removeStateDidChangeListener(authListener) }
        authListener = nil
        reset()
        uid = nil
    }
    private func reset() {
        epoch += 1; revision += 1
        refreshTask?.cancel(); refreshTask = nil
        for listener in listeners.values { listener.remove() }
        listeners.removeAll()
        teams = []; detail = nil; invitationLink = ""; selectedID = ""; busy = false; errorKey = nil
    }
    private func accountChanged(_ value: String?) {
        guard started else { return }
        if value != uid { reset(); uid = value }
        guard let uid, listeners["index"] == nil else { return }
        let epoch = epoch
        listeners["index"] = Firestore.firestore().collection("users").document(uid).collection("teams")
            .addSnapshotListener(includeMetadataChanges: true) { [weak self] snapshot, error in
                let invalidated = snapshot?.metadata.isFromCache != false || error != nil
                Task { @MainActor in self?.changed(epoch: epoch, invalidated: invalidated) }
            }
        scheduleRefresh()
    }
    private func changed(epoch: Int, invalidated: Bool) {
        guard epoch == self.epoch else { return }
        if invalidated { revision += 1; authority += 1; teams = []; detail = nil; invitationLink = "" }
        scheduleRefresh()
    }
    func scheduleRefresh() {
        refreshTask?.cancel()
        refreshTask = Task { [weak self] in
            do { try await Task.sleep(for: .milliseconds(80)) } catch { return }
            await self?.refresh()
        }
    }
    func select(_ teamID: String) {
        selectedID = teamID; detail = nil; invitationLink = ""
        if listeners[teamID] == nil {
            let epoch = epoch
            listeners[teamID] = Firestore.firestore().collection("teams").document(teamID).collection("members")
                .addSnapshotListener(includeMetadataChanges: true) { [weak self] snapshot, error in
                    let invalidated = snapshot?.metadata.isFromCache != false || error != nil
                    Task { @MainActor in
                        guard let self, epoch == self.epoch else { return }
                        if error != nil { self.listeners.removeValue(forKey: teamID)?.remove() }
                        self.changed(epoch: epoch, invalidated: invalidated)
                    }
                }
        }
        scheduleRefresh()
    }
    func refresh() async {
        guard uid != nil else { return }
        revision += 1
        let request = revision, epoch = epoch, selected = selectedID
        loading = true
        defer { if request == revision { loading = false } }
        do {
            let list: TeamsList = try await call()
            guard request == revision, epoch == self.epoch, !Task.isCancelled else { return }
            teams = list.teams; errorKey = nil
            if !selected.isEmpty {
                do {
                    let current: TeamDetail = try await call("/\(selected)")
                    guard request == revision, epoch == self.epoch, !Task.isCancelled else { return }
                    detail = current
                } catch {
                    guard request == revision, epoch == self.epoch, !Task.isCancelled else { return }
                    detail = nil; selectedID = ""; invitationLink = ""; errorKey = key(error)
                }
            }
        } catch {
            guard request == revision, epoch == self.epoch, !Task.isCancelled else { return }
            teams = []; detail = nil; errorKey = key(error)
        }
    }
    @discardableResult
    func execute(_ input: [String: Any]) async -> Bool {
        guard !busy, uid != nil else { return false }
        let epoch = epoch, authority = authority
        busy = true; errorKey = nil
        defer { if epoch == self.epoch { busy = false } }
        do {
            let result: TeamResult = try await call(body: input)
            guard epoch == self.epoch else { return false }
            if ["leave", "delete"].contains(input["action"] as? String ?? "") { selectedID = ""; detail = nil }
            guard authority == self.authority else { scheduleRefresh(); return false }
            if let code = result.code { invitationLink = TeamInvitation.link(code) }
            if let teamID = result.teamId, ["create", "join"].contains(input["action"] as? String ?? "") { select(teamID) }
            await refresh()
            return true
        } catch {
            if epoch == self.epoch { errorKey = key(error) }
            return false
        }
    }
    private func call<T: Decodable>(_ path: String = "", body: [String: Any]? = nil) async throws -> T {
        guard let uid, let user = Auth.auth().currentUser, user.uid == uid else { throw Failure(code: "INVALID_SESSION") }
        let token = try await user.getIDToken()
        guard Auth.auth().currentUser?.uid == uid else { throw Failure(code: "INVALID_SESSION") }
        var request = URLRequest(url: URL(string: baseURL + path)!)
        request.timeoutInterval = 20
        request.cachePolicy = .reloadIgnoringLocalCacheData
        request.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
        if let body {
            request.httpMethod = "POST"
            request.setValue("application/json", forHTTPHeaderField: "Content-Type")
            request.httpBody = try JSONSerialization.data(withJSONObject: body)
        }
        let (data, response) = try await URLSession.shared.data(for: request)
        guard Auth.auth().currentUser?.uid == uid else { throw Failure(code: "INVALID_SESSION") }
        guard let http = response as? HTTPURLResponse, (200...299).contains(http.statusCode) else {
            let object = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any]
            throw Failure(code: (object?["error"] as? [String: Any])?["code"] as? String ?? "SERVICE_UNAVAILABLE")
        }
        return try JSONDecoder().decode(T.self, from: data)
    }
    private struct Failure: Error { let code: String }
    private func key(_ error: Error) -> String {
        switch (error as? Failure)?.code {
        case "INVITE_UNAVAILABLE": "teams_invite_error"
        case "STALE_MEMBERSHIP", "MEMBERSHIP_REQUIRED": "teams_stale_error"
        default: "teams_error"
        }
    }
}
