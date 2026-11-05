import FirebaseAuth
import FirebaseFirestore
import Foundation

/// The directory-only listener of a team the viewer belongs to but has not opened (spec §6.6.3; Desktop
/// `watchMessages(scope, '')`): the viewer's `members/{uid}` document must carry the team generation, then
/// the `conversations` the viewer is a member of and the viewer's `reads/{uid}` marks of each give the unread
/// total. No messages, no Bots. That total feeds the team switcher and the Teams tab badge.
///
/// The total is reported once every listed conversation has delivered its read mark: a mark that has not
/// arrived yet must not make its whole conversation look unread. A conversation document that breaks the
/// stored contract, a revoked membership or a failing listener stops counting (`onStopped`); the workspace
/// starts the watcher again with its next team-list refresh if the viewer is still a member.
@MainActor
final class TeamsUnreadWatcher {
    let teamID: String
    let generation: Int
    private let uid: String
    private let auth: Auth
    private let db: Firestore
    private let onUnread: (String, Int) -> Void
    private let onStopped: (String) -> Void
    private var memberListener: ListenerRegistration?
    private var directoryListener: ListenerRegistration?
    private var readListeners: [String: ListenerRegistration] = [:]
    /// `lastSeq` of every conversation of the directory.
    private var lastSeq: [String: Int] = [:]
    /// `lastReadSeq` of every conversation whose read mark has arrived.
    private var readSeq: [String: Int] = [:]
    private var authorized = false
    private var stopped = false

    init(uid: String, team: TeamSummary, auth: Auth, db: Firestore, onUnread: @escaping (String, Int) -> Void, onStopped: @escaping (String) -> Void) {
        self.uid = uid; teamID = team.id; generation = team.generation
        self.auth = auth; self.db = db; self.onUnread = onUnread; self.onStopped = onStopped
    }

    func start() {
        guard memberListener == nil, !stopped else { return }
        let root = db.collection("teams").document(teamID)
        memberListener = root.collection("members").document(uid).addSnapshotListener(includeMetadataChanges: true) { [weak self] snapshot, error in
            Task { @MainActor in
                guard let self, self.isCurrent else { return }
                if error != nil { self.fail(); return }
                if (snapshot?.data()?["generation"] as? NSNumber)?.intValue == self.generation {
                    if !self.authorized { self.authorized = true; self.listenDirectory(root) }
                } else if self.authorized || snapshot?.metadata.isFromCache == false {
                    self.fail()
                }
            }
        }
    }

    /// Stops every listener without reporting (the workspace is replacing or dropping this watcher).
    func stop() {
        stopped = true
        memberListener?.remove(); directoryListener?.remove()
        memberListener = nil; directoryListener = nil
        readListeners.values.forEach { $0.remove() }
        readListeners = [:]
    }

    private var isCurrent: Bool { !stopped && auth.currentUser?.uid == uid }

    private func fail() {
        guard !stopped else { return }
        stop()
        onStopped(teamID)
    }

    private func listenDirectory(_ root: DocumentReference) {
        directoryListener = root.collection("conversations").whereField("memberUids", arrayContains: uid)
            .addSnapshotListener(includeMetadataChanges: true) { [weak self] snapshot, error in
                Task { @MainActor in
                    guard let self, self.isCurrent else { return }
                    if error != nil { self.fail(); return }
                    guard let snapshot else { return }
                    var rows: [String: Int] = [:]
                    do {
                        for document in snapshot.documents {
                            let data = document.data().filter { TeamMessagingModel.conversationFields.contains($0.key) }
                            rows[document.documentID] = try TeamsPointerParser.parseConversation(
                                id: document.documentID, data: data, lastReadSeq: 0, millis: TeamMessagingModel.milliseconds).lastSeq
                        }
                    } catch {
                        // Never a half-parsed count: this team stops counting.
                        self.fail()
                        return
                    }
                    self.lastSeq = rows
                    for id in Array(self.readListeners.keys) where rows[id] == nil {
                        self.readListeners.removeValue(forKey: id)?.remove()
                        self.readSeq.removeValue(forKey: id)
                    }
                    for id in rows.keys.sorted() where self.readListeners[id] == nil {
                        self.readListeners[id] = root.collection("conversations").document(id).collection("reads").document(self.uid)
                            .addSnapshotListener(includeMetadataChanges: true) { [weak self] mark, error in
                                Task { @MainActor in
                                    guard let self, self.isCurrent else { return }
                                    if error != nil { self.fail(); return }
                                    let data = mark?.data()
                                    let marked = (data?["generation"] as? NSNumber)?.intValue == self.generation ? (data?["lastReadSeq"] as? NSNumber)?.intValue ?? 0 : 0
                                    // A read mark only moves forward.
                                    self.readSeq[id] = max(self.readSeq[id] ?? 0, marked)
                                    self.publish()
                                }
                            }
                    }
                    self.publish()
                }
            }
    }

    private func publish() {
        guard authorized, !stopped, lastSeq.keys.allSatisfy({ readSeq[$0] != nil }) else { return }
        onUnread(teamID, lastSeq.reduce(0) { $0 + max(0, $1.value - (readSeq[$1.key] ?? 0)) })
    }
}
