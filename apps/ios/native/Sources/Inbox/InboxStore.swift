import FirebaseAuth
import FirebaseFirestore
import Foundation
import Observation
import UserNotifications

@MainActor
@Observable
final class InboxStore {
    static let shared = InboxStore()
    static let pageSize = 50

    private(set) var uid: String?
    private(set) var visible: [InboxRow] = []
    private(set) var unreadBadge = 0
    private(set) var offline = true
    private(set) var loading = false
    private(set) var issue: Issue?
    private(set) var notice: InboxPush?
    private(set) var inlineNotice: InboxPush?
    private(set) var route: InboxRow?
    private(set) var hasMore = false
    private(set) var assistantSync = false
    private(set) var dmReceipts = false
    private(set) var markingAllRead = false
    var canMarkAllRead: Bool { uid != nil && !offline && storageAvailable && !markingAllRead }
    private(set) var personalRevision = 0
    private(set) var unreadByTeam: [String: Int] = [:]
    private(set) var activeRow: InboxRow?
    var filter: InboxFilter = .all { didSet { if filter != oldValue { resetQuery() } } }
    var category: InboxCategory? { didSet { if category != oldValue { resetQuery() } } }
    var search = "" {
        didSet {
            if InboxTitleSearch.prefix(search) != InboxTitleSearch.prefix(oldValue) { resetQuery() }
        }
    }
    var showArchived = false { didSet { if showArchived != oldValue { resetQuery() } } }

    enum Issue { case storage, backpressure, sync, unavailable, readBatchUnavailable, preferenceConflict }

    @ObservationIgnored private var initialized = false
    @ObservationIgnored private var epoch = 0
    @ObservationIgnored private var query = 0
    @ObservationIgnored private var files = InboxFiles(uid: nil)
    @ObservationIgnored private var state = InboxAccountState()
    @ObservationIgnored private var pendingByKey: [String: [InboxOperation]] = [:]
    private(set) var storageAvailable = false
    @ObservationIgnored private var remote: [InboxRow] = []
    @ObservationIgnored private var cached: [InboxRow] = []
    @ObservationIgnored private var cloudVerification: [InboxRef: Int64] = [:]
    @ObservationIgnored private var cloudExpiration: Task<Void, Never>?
    @ObservationIgnored private var cloudRenewal: Task<Void, Never>?
    @ObservationIgnored private var lastCloudRenewal: Int64?
    @ObservationIgnored private var loadedCloudPages = 0
    @ObservationIgnored private var readBatchTask: Task<Void, Never>?
    @ObservationIgnored private var readBatchTaskRevision = 0
    @ObservationIgnored private var cloudAuthorityRevision = 0
    @ObservationIgnored private var local: [String: InboxRow] = [:]
    @ObservationIgnored private var localActivity: [String: InboxAssistantActivity] = [:]
    @ObservationIgnored private var availableRemoteRows: [InboxRow] = []
    @ObservationIgnored private var availableRemoteActivity: [String: InboxAssistantActivity] = [:]
    @ObservationIgnored private var published: [String: Int] = [:]
    @ObservationIgnored private var next: InboxCursor?
    @ObservationIgnored private var limit = pageSize
    @ObservationIgnored private var signals: ListenerRegistration?
    @ObservationIgnored private var badges: ListenerRegistration?
    @ObservationIgnored private var refresh: Task<Void, Never>?
    @ObservationIgnored private var flush: Task<Void, Never>?
    @ObservationIgnored private var cacheWrite: Task<Void, Never>?
    @ObservationIgnored private var inFlight: String?
    @ObservationIgnored private var badgeFromServer = 0
    @ObservationIgnored private var foreground = false
    @ObservationIgnored private var current: InboxRef?
    @ObservationIgnored private var readTask: Task<Void, Never>?
    @ObservationIgnored private var presence: Task<Void, Never>?
    @ObservationIgnored private var activeRefresh: Task<Void, Never>?
    @ObservationIgnored private var noticeRefresh: Task<Void, Never>?
    @ObservationIgnored private var verifiedActive: InboxRow?
    @ObservationIgnored private var pendingPush: InboxPush?
    @ObservationIgnored private var stagedRead: InboxRead?
    @ObservationIgnored private var routeRequest = 0

    var activeRef: InboxRef? { current }
    func activity(_ ref: InboxRef) -> InboxAssistantActivity? { localActivity[ref.key] ?? availableRemoteActivity[ref.key] }
    var localSourceIDs: Set<String> { Set(local.values.map(\.ref.sourceId)) }

    func configure(_ account: String?) async {
        if initialized && uid == account { return }
        initialized = true
        epoch += 1
        let life = epoch
        closeListeners()
        uid = account
        state = InboxAccountState()
        pendingByKey = [:]
        activeRow = nil; verifiedActive = nil
        files = InboxFiles(uid: account)
        storageAvailable = false
        local = [:]; localActivity = [:]; availableRemoteRows = []; availableRemoteActivity = [:]
        published = [:]; remote = []; cached = []; cloudVerification = [:]
        cloudAuthorityRevision &+= 1
        route = nil; notice = nil; inlineNotice = nil; current = nil
        assistantSync = false; dmReceipts = false; markingAllRead = false; stagedRead = nil; issue = nil
        lastCloudRenewal = nil; loadedCloudPages = 0
        badgeFromServer = 0; unreadBadge = 0; offline = true; visible = []
        setApplicationBadge(0)
        let storage = files
        let work = Task.detached(priority: .userInitiated) {
            (Result { try storage.load() }, storage.cached(now: InboxPush.now))
        }
        let (loaded, cache) = await work.value
        guard life == epoch else { return }
        switch loaded {
        case let .success(value):
            state = value
            pendingByKey = Dictionary(grouping: value.queue, by: \.key)
            storageAvailable = true
        case .failure: issue = .storage
        }
        assistantSync = state.assistantSync
        dmReceipts = state.peerReceipts
        markingAllRead = state.readBatch != nil
        personalRevision &+= 1
        cached = cache.map(\.row)
        cloudVerification = Dictionary(cache.map { ($0.row.ref, $0.verifiedAt) }, uniquingKeysWith: max)
        rebuild()
        scheduleCloudExpiration()
        if let account, !IsolatedSimulatorTesting.enabled {
            installSubscriptions(account, life: life)
            resetQuery()
            flushSoon()
            updatePresence()
            resumeReadBatch()
            Task { [weak self] in await self?.getAccountPreferences(account, life: life) }
            if let push = pendingPush, push.recipient == account {
                pendingPush = nil
                Task { [weak self] in await self?.open(push) }
            }
        }
    }

    private func closeListeners() {
        signals?.remove(); signals = nil
        badges?.remove(); badges = nil
        refresh?.cancel(); refresh = nil
        flush?.cancel(); flush = nil; inFlight = nil
        readTask?.cancel(); readTask = nil
        presence?.cancel(); presence = nil
        cacheWrite?.cancel(); cacheWrite = nil
        activeRefresh?.cancel(); activeRefresh = nil
        noticeRefresh?.cancel(); noticeRefresh = nil
        cloudExpiration?.cancel(); cloudExpiration = nil
        cloudRenewal?.cancel(); cloudRenewal = nil
        pauseReadBatch()
    }

    private func installSubscriptions(_ uid: String, life: Int) {
        let account = Firestore.firestore().collection("users").document(uid)
        signals = account.collection("inboxSignals").limit(to: 16).addSnapshotListener { [weak self] _, error in
            Task { @MainActor in
                guard let self, self.epoch == life, Auth.auth().currentUser?.uid == uid else { return }
                if self.subscriptionUnauthorized(error) { self.purgeCloud() }
                else if error == nil { self.requestRefresh(); self.refreshActive(); self.refreshNotices() }
            }
        }
        badges = account.collection("inboxBadges").limit(to: 16).addSnapshotListener(includeMetadataChanges: true) { [weak self] snapshot, error in
            let total = snapshot?.documents.reduce(0) { count, doc in
                count + min(100, max(0, (doc.data()["count"] as? NSNumber)?.intValue ?? 0))
            }
            Task { @MainActor in
                guard let self, self.epoch == life, Auth.auth().currentUser?.uid == uid else { return }
                if self.subscriptionUnauthorized(error) { self.purgeCloud(); return }
                guard error == nil, snapshot?.metadata.isFromCache == false, let total else { return }
                self.badgeFromServer = total
                self.updateBadge()
            }
        }
    }

    private func getAccountPreferences(_ account: String, life: Int) async {
        guard let prefs: InboxTransport.AccountPreferences = try? await InboxTransport(uid: account).request(["action": "accountPreferences"]),
              epoch == life, !state.queue.contains(where: { $0.action == .peerReceipts }) else { return }
        var candidate = state
        candidate.peerReceipts = prefs.dmReceipts
        if commit(candidate) { dmReceipts = prefs.dmReceipts }
    }

    func setForeground(_ value: Bool) {
        if foreground != value {
            foreground = value
            if !value { leaveReadContext() }
        }
        updatePresence()
        if value { rebuild(); requestRefresh(); refreshActive(); flushSoon(); refreshNotices(); resumeReadBatch() }
        else { noticeRefresh?.cancel(); noticeRefresh = nil; pauseReadBatch() }
        scheduleCloudExpiration()
    }

    func setCurrent(_ ref: InboxRef?, replacing expected: InboxRef? = nil) {
        if let expected, current != expected { return }
        guard current != ref else { return }
        leaveReadContext()
        current = ref
        verifiedActive = nil
        activeRow = ref.flatMap { row($0) }
        inlineNotice = nil
        refreshActive()
        updatePresence()
    }

    func refreshNow() { requestRefresh(delay: .zero) }
    func clearIssue() { issue = nil }
    func clearNotice() { notice = nil }
    func clearRoute() { route = nil }

    private var searchPrefix: String? { InboxTitleSearch.prefix(search) }

    private func resetQuery() {
        loading = false
        limit = Self.pageSize
        loadedCloudPages = 0
        next = nil
        remote = []
        rebuild()
        requestRefresh()
    }

    private func requestRefresh(delay: Duration = .milliseconds(200)) {
        guard uid != nil else { return }
        refresh?.cancel()
        query &+= 1
        let currentQuery = query
        refresh = Task { [weak self] in
            do { try await Task.sleep(for: delay) } catch { return }
            guard let self else { return }
            await self.fetch(after: nil, query: currentQuery)
        }
    }

    func loadMore() {
        guard !loading else { return }
        limit += Self.pageSize
        rebuild()
        guard let next else { return }
        query &+= 1
        let currentQuery = query
        refresh = Task { [weak self] in await self?.fetch(after: next, query: currentQuery) }
    }

    private func subscriptionUnauthorized(_ error: Error?) -> Bool {
        guard let code = (error as NSError?)?.code else { return false }
        return code == FirestoreErrorCode.permissionDenied.rawValue || code == FirestoreErrorCode.unauthenticated.rawValue
    }

    private func cloudAuthorized(_ ref: InboxRef, at now: Int64 = InboxPush.now) -> Bool {
        guard let verified = cloudVerification[ref], now >= verified else { return false }
        return now - verified < InboxCachedRow.lease
    }

    private func verifyCloud(_ rows: [InboxRow]) {
        let now = InboxPush.now
        for row in rows where row.valid { cloudVerification[row.ref] = now }
        scheduleCloudExpiration()
    }

    private func persistCloudCache() {
        let now = InboxPush.now
        let snapshot = cached.compactMap { row -> InboxCachedRow? in
            guard let time = cloudVerification[row.ref], cloudAuthorized(row.ref, at: now) else { return nil }
            return InboxCachedRow(row: row, verifiedAt: time)
        }
        let previous = cacheWrite
        previous?.cancel()
        let storage = files
        cacheWrite = Task.detached(priority: .utility) {
            await previous?.value
            if !Task.isCancelled { storage.saveCache(snapshot) }
        }
    }

    private func scheduleCloudExpiration() {
        cloudExpiration?.cancel()
        cloudRenewal?.cancel()
        guard let first = cloudVerification.values.min() else { return }
        let life = epoch
        cloudExpiration = Task { [weak self] in
            do { try await Task.sleep(for: .milliseconds(max(1, first + InboxCachedRow.lease - InboxPush.now))) } catch { return }
            guard let self, self.epoch == life else { return }
            let now = InboxPush.now
            let expired = Set(self.cloudVerification.keys.filter { !self.cloudAuthorized($0, at: now) })
            let shownExpired = self.remote.contains { expired.contains($0.ref) } ||
                self.verifiedActive.map { expired.contains($0.ref) } == true
            self.dropCloud(where: expired.contains)
            self.scheduleCloudExpiration()
            self.rebuild()
            if shownExpired && self.foreground && !self.loading {
                self.requestRefresh(delay: .zero)
                self.refreshActive()
            }
        }
        // Renew the loaded pages plus the selected conversation while they are
        // still valid. Only authenticated responses extend the hard deadline.
        guard foreground else { return }
        let visibleRefs = remote.map(\.ref) + (verifiedActive.map { [$0.ref] } ?? [])
        guard let firstVisible = visibleRefs.compactMap({ cloudVerification[$0] }).min() else { return }
        let due = InboxCachedRow.renewAt(verifiedAt: firstVisible, attemptedAt: lastCloudRenewal)
        cloudRenewal = Task { [weak self] in
            do { try await Task.sleep(for: .milliseconds(max(1, due - InboxPush.now))) } catch { return }
            guard let self, self.epoch == life, self.foreground else { return }
            self.lastCloudRenewal = InboxPush.now
            if !self.loading { self.requestRefresh(delay: .zero) }
            self.refreshActive()
            self.scheduleCloudExpiration()
        }
    }

    private func dropCloud(where rejected: (InboxRef) -> Bool) {
        remote.removeAll { rejected($0.ref) }
        cached.removeAll { rejected($0.ref) }
        if let row = verifiedActive, rejected(row.ref) { verifiedActive = nil }
        if let row = activeRow, rejected(row.ref) { activeRow = nil }
        if let row = route, rejected(row.ref) { route = nil }
        if let notice, rejected(notice.ref) { self.notice = nil }
        if let inlineNotice, rejected(inlineNotice.ref) { self.inlineNotice = nil }
        for ref in Array(cloudVerification.keys) where rejected(ref) { cloudVerification.removeValue(forKey: ref) }
    }

    private func purgeCloud(rejected: InboxRef? = nil, failure: InboxAuthorityFailure? = nil) {
        query &+= 1
        cloudAuthorityRevision &+= 1
        loading = false
        next = nil
        if let rejected, let failure {
            dropCloud { failure.revokes($0, rejected: rejected) }
        } else {
            dropCloud { _ in true }
            badgeFromServer = 0
            offline = true
        }
        persistCloudCache()
        scheduleCloudExpiration()
        rebuild()
    }

    private func fetch(after: InboxCursor?, query expected: Int) async {
        guard let uid else { return }
        let life = epoch
        loading = true
        defer { if epoch == life && query == expected { loading = false } }
        let prefix = searchPrefix
        let session = InboxTransport(uid: uid)
        let pages = after == nil ? max(1, loadedCloudPages) : 1
        var more = after
        var rows = after == nil ? [] : remote
        let priorRowCount = rows.count
        var failures: Error?
        var successfulPages = 0
        for _ in 0..<pages {
            let body = InboxListRequest(filter: filter, category: category, archived: showArchived,
                                        searchPrefix: prefix, after: more).requestValues
            do {
                let page: InboxPage = try await session.request(body)
                guard epoch == life, query == expected else { return }
                rows.append(contentsOf: page.items.filter(\.valid))
                successfulPages += 1
                more = page.next
                if more == nil { break }
            } catch {
                failures = error
                break
            }
        }
        guard epoch == life, query == expected, !Task.isCancelled else { return }
        if let error = failures {
            if (error as? InboxTransportError)?.authority != nil {
                purgeCloud()
            } else if after != nil, case let .response(failure)? = error as? InboxTransportError,
                      failure.code == "SEARCH_CURSOR_EXPIRED" {
                resetQuery()
            } else {
                offline = true
                rebuild()
            }
        } else {
            offline = false
            remote = rows
            loadedCloudPages = after == nil ? successfulPages : loadedCloudPages + successfulPages
            next = more
            // A continuation response only renewed the newly appended page.
            verifyCloud(Array(rows.dropFirst(priorRowCount)))

            if filter == .all && category == nil && prefix == nil && !showArchived {
                cached = rows
                persistCloudCache()
            }
            rebuild()
            resumeReadBatch()
        }
    }

    private func matches(_ row: InboxRow) -> Bool {
        filter.accepts(row) && (category == nil || row.category == category) && row.archived == showArchived
    }

    private func rebuild() {
        var rows: [String: InboxRow] = [:]
        let now = InboxPush.now
        for row in (offline ? cached + remote : remote)
            where cloudAuthorized(row.ref, at: now) && (local[row.key] == nil || allowsAssistantSync(row.key)) {
            rows[row.key] = applyingPending(row)
        }
        for input in Array(local.values) + availableRemoteRows {
            var row = input
            if local[row.key] != nil { row.ref = localAssistantRef(row.ref) }
            let server = rows[row.key] ?? verifiedActive.flatMap { $0.key == row.key &&
                (local[row.key] == nil || allowsAssistantSync(row.key)) ? $0 : nil }
            if let server {
                var merged = row
                merged.ref = server.ref
                merged.pinOrder = server.pinOrder; merged.archived = server.archived; merged.notificationMode = server.notificationMode
                merged.preferencesRevision = server.preferencesRevision
                merged.manualUnread = server.manualUnread; merged.manualUnreadRevision = server.manualUnreadRevision
                merged.readThrough = max(row.readThrough, min(row.sourceCursor, server.readThrough))
                merged.unreadCount = max(0, merged.sourceCursor - merged.readThrough)
                rows[row.key] = applyingPending(merged)
            } else {
                rows[row.key] = applyingPending(row)
            }
        }
        let matches = orderedMatches(rows.values)
        visible = Array(matches.prefix(limit))
        hasMore = next != nil || matches.count > limit
        if let current {
            activeRow = rows[current.key].flatMap { $0.ref == current ? $0 : nil }
                ?? verifiedActive.map(applyingPending)
        }
        unreadByTeam = Dictionary(grouping: rows.values.filter { $0.category == .team && !$0.archived }, by: \.ref.sourceId)
            .mapValues { $0.reduce(0) { min(100, $0 + max($1.manualUnread ? 1 : 0, $1.unreadCount)) } }
        updateBadge()
    }

    private func orderedMatches(_ rows: Dictionary<String, InboxRow>.Values) -> [InboxRow] {
        guard let prefix = searchPrefix else { return rows.filter(matches).sorted(by: InboxRow.ordered) }
        return rows.filter(matches)
            .map { (title: InboxTitleSearch.normalize($0.title), row: $0) }
            .filter { $0.title.hasPrefix(prefix) }
            .sorted { lhs, rhs in
                if lhs.title != rhs.title { return lhs.title.utf8.lexicographicallyPrecedes(rhs.title.utf8) }
                return lhs.row.key < rhs.row.key
            }
            .map { $0.row }
    }

    private func applyingPending(_ original: InboxRow) -> InboxRow {
        var row = original
        if row.ref.kind == .assistant, let own = state.personal[row.key] {
            row.readThrough = min(row.sourceCursor, max(row.readThrough, own.readThrough))
            row.unreadCount = max(0, row.sourceCursor - row.readThrough)
            let hasServerState = !offline && (remote.contains { $0.ref == row.ref } || verifiedActive?.ref == row.ref)
            if !hasServerState {
                if row.manualUnreadRevision <= own.manualUnreadRevision {
                    row.manualUnread = own.manualUnread
                    row.manualUnreadRevision = own.manualUnreadRevision
                }
                row.pinOrder = own.pinOrder; row.notificationMode = own.notificationMode; row.archived = own.archived
                row.preferencesRevision = own.preferencesRevision
            }
        }
        for op in pendingByKey[row.key] ?? [] where op.generation == row.ref.generation {
            switch op.action {
            case .markRead:
                let through = op.through ?? 0
                row.readThrough = min(row.sourceCursor, max(row.readThrough, through))
                if through >= row.sourceCursor { row.unreadCount = 0; row.mentionCount = 0 }
                if op.manualUnreadRevision == row.manualUnreadRevision { row.manualUnread = false }
            case .markUnread:
                if op.expectedManualUnreadRevision == row.manualUnreadRevision {
                    row.manualUnread = true
                    // Pending operations have provisional revisions. A read
                    // acquired earlier cannot clear a subsequent reminder.
                    row.manualUnreadRevision += 1
                }
            case .preferences:
                let revision = row.preferencesRevision ?? 0
                if let patch = op.preferences, op.expectedPreferencesRevision == revision {
                    row.preferencesRevision = revision + 1
                    if patch.clearPin == true { row.pinOrder = nil }
                    if let order = patch.pinOrder { row.pinOrder = order }
                    if let mode = patch.notificationMode { row.notificationMode = mode }
                    if let archived = patch.archived { row.archived = archived }
                }
            case .assistantSummary, .assistantRemove, .assistantSync, .peerReceipts: break
            }
        }
        return row
    }

    func row(_ ref: InboxRef) -> InboxRow? {
        if let row = activeRow, row.ref == ref { return row }
        if let row = visible.first(where: { $0.ref == ref }) { return row }
        if cloudAuthorized(ref), local[ref.key] == nil || allowsAssistantSync(ref.key),
           let row = (remote + cached).first(where: { $0.ref == ref }) { return applyingPending(row) }
        if var row = local[ref.key] {
            row.ref = localAssistantRef(row.ref)
            if row.ref == ref { return applyingPending(row) }
        }
        if let row = availableRemoteRows.first(where: { $0.ref == ref }) { return applyingPending(row) }
        return nil
    }

    func updateRemoteAssistant(rows: [InboxRow], activity: [String: InboxAssistantActivity]) {
        availableRemoteRows = rows.filter(\.valid)
        availableRemoteActivity = activity
        rebuild()
    }

    func localAssistantRef(_ ref: InboxRef) -> InboxRef {
        _ = personalRevision
        guard allowsAssistantSync(ref.key), let generation = state.assistantGenerations[ref.sourceId] else { return ref }
        return InboxRef(kind: .assistant, sourceId: ref.sourceId, conversationId: ref.conversationId, generation: generation)
    }

    func remoteAssistantRef(host: String, session: String) -> InboxRef? {
        _ = visible
        let sessionID = canonicalSessionID(session)
        let cloud = (remote + cached).first {
            $0.remoteHostId == host && $0.ref.conversationId == sessionID && cloudAuthorized($0.ref)
        }
        return cloud?.ref ?? availableRemoteRows.first { $0.remoteHostId == host && $0.ref.conversationId == sessionID }?.ref
    }

    func updateLocal(rows: [InboxRow], activity: [String: InboxAssistantActivity]) {
        local = Dictionary(rows.filter(\.valid).map { ($0.key, $0) }, uniquingKeysWith: { a, b in a.sourceRevision >= b.sourceRevision ? a : b })
        localActivity = activity
        rebuild()
        enqueueSummaries()
    }

    private func enqueueSummaries() {
        guard uid != nil, state.assistantSync, storageAvailable else { return }
        let needsPublish = localActivity.filter { key, activity in
            state.sharesAssistant(key) && (published[key] ?? -1) < activity.sourceRevision
        }
        let queue = needsPublish.compactMap { key, activity in
            local[key].map { row in
                var operation = InboxOperation(action: .assistantSummary, ref: row.ref,
                                               summary: InboxAssistantSummary(activity: activity))
                operation.consentRevision = state.assistantConsentRevision
                operation.conversationConsentRevision = state.assistantSharing?[key]?.revision
                return operation
            }
        }
        if !queue.isEmpty, admit(queue) {
            for (key, activity) in needsPublish { published[key] = activity.sourceRevision }
        }
    }

    private func allowsAssistantSync(_ key: String) -> Bool {
        state.assistantSync && state.sharesAssistant(key)
    }

    func localAssistantSharing(_ row: InboxRow) -> Bool? {
        _ = personalRevision
        guard uid != nil, row.ref.kind == .assistant, local[row.key] != nil else { return nil }
        return state.sharesAssistant(row.key)
    }

    func setLocalAssistantSharing(_ enabled: Bool, row: InboxRow) {
        guard let sharing = localAssistantSharing(row), sharing != enabled else { return }
        var candidate = state
        let choice = candidate.setSharing(enabled, for: row.key)
        if !enabled {
            let nextUpload = candidate.queue.contains { $0.key == row.key && $0.action == .assistantSummary && $0.id == inFlight }
            let wasRegistered = candidate.assistantGenerations[row.ref.sourceId] != nil ||
                (remote + cached).contains { $0.key == row.key }
            // Revoking cloud permission retires pending copies of the local
            // read and preference mutations. Their local effects are durable.
            candidate.queue.removeAll { op in
                op.key == row.key && op.action != .assistantRemove && op.id != inFlight
            }
            if candidate.assistantSync && (wasRegistered || nextUpload) {
                var removal = InboxOperation(action: .assistantRemove, ref: row.ref)
                removal.sourceRevision = max(row.sourceRevision, localActivity[row.key]?.sourceRevision ?? 0)
                removal.conversationConsentRevision = choice.revision
                do { try candidate.enqueue([removal], inFlight: inFlight) }
                catch { issue = .backpressure; return }
            }
        }
        guard commit(candidate) else { return }
        personalRevision &+= 1
        published.removeValue(forKey: row.key)
        if !enabled {
            dropCloud { $0.key == row.key }
            persistCloudCache()
            scheduleCloudExpiration()
        }
        enqueueSummaries()
        flushSoon(delay: .zero)
        rebuild()
    }

    func setAssistantSync(_ enabled: Bool) {
        guard uid != nil, state.assistantSync != enabled else { return }
        var candidate = state
        candidate.assistantSync = enabled
        candidate.assistantConsentRevision = (state.assistantConsentRevision ?? 0) + 1
        let consentRevision = candidate.assistantConsentRevision
        if !enabled {
            let oldSources = candidate.queue.compactMap { $0.summary?.activity.sourceId }
            let sourceIDs = localSourceIDs.union(oldSources).union(candidate.assistantGenerations.keys)
            candidate.queue.removeAll { $0.action == .assistantSummary && $0.id != inFlight }
            do {
                try candidate.enqueue(sourceIDs.map { source in
                    var op = InboxOperation(action: .assistantSync, ref: InboxRef(kind: .assistant, sourceId: source,
                                                                                   conversationId: "source", generation: 1))
                    op.enabled = false
                    op.consentRevision = consentRevision
                    return op
                }, inFlight: inFlight)
            } catch { issue = .backpressure; return }
            published = [:]
        }
        guard commit(candidate) else { return }
        assistantSync = enabled
        enqueueSummaries()
        flushSoon()
        rebuild()
    }

    func setDMReceipts(_ enabled: Bool) {
        guard uid != nil, state.peerReceipts != enabled else { return }
        var candidate = state
        do { try candidate.enqueue([InboxOperation(action: .peerReceipts, enabled: enabled)], inFlight: inFlight) }
        catch { issue = .backpressure; return }
        candidate.peerReceipts = enabled
        if commit(candidate) { dmReceipts = enabled; flushSoon() }
    }

    func peerReceipt(_ ref: InboxRef) async -> Int? {
        guard dmReceipts, ref.kind == .team, let uid else { return nil }
        let result: InboxTransport.PeerReceipt? = try? await InboxTransport(uid: uid).request(
            ["action": "peerReceipt", "key": ref.key, "generation": ref.generation]
        )
        guard self.uid == uid, dmReceipts else { return nil }
        return result?.readThrough
    }

    func markRead(_ rows: [InboxRow]) {
        // Capture the current rows before queue admission, including reminder
        // revisions, so a later message or manual reminder survives.
        let operations = rows.map { ($0, InboxOperation($0.frozenRead())) }
        apply(operations)
    }

    func markUnread(_ rows: [InboxRow]) {
        apply(rows.map { ($0, InboxOperation(markingUnread: $0)) })
    }

    func markAllRead() {
        guard canMarkAllRead, let uid else { return }
        let batch = InboxReadBatch(requestedAt: InboxPush.now)
        var candidate = state
        for localRow in local.values {
            var row = localRow
            row.ref = localAssistantRef(row.ref)
            row = self.row(row.ref) ?? applyingPending(row)
            if !row.archived && row.isUnread {
                candidate.applyLocal(InboxOperation(row.frozenRead()), row: row)
            }
        }
        candidate.readBatch = batch
        guard commit(candidate) else { return }
        markingAllRead = true
        rebuild()
        runReadBatch(batch, account: uid, submit: true)
    }

    private func pauseReadBatch() {
        readBatchTaskRevision &+= 1
        readBatchTask?.cancel()
        readBatchTask = nil
    }

    private func resumeReadBatch() {
        guard foreground, readBatchTask == nil, let uid, let batch = state.readBatch else { return }
        runReadBatch(batch, account: uid, submit: false)
    }

    private func runReadBatch(_ batch: InboxReadBatch, account: String, submit: Bool) {
        let life = epoch
        readBatchTaskRevision &+= 1
        let revision = readBatchTaskRevision
        readBatchTask = Task { [weak self] in
            guard let self else { return }
            defer {
                if self.readBatchTaskRevision == revision { self.readBatchTask = nil }
            }
            let api = InboxTransport(uid: account)
            var firstRequest = submit
            while self.epoch == life, self.readBatchTaskRevision == revision, !Task.isCancelled {
                let body: [String: Any] = firstRequest ? batch.createRequest : batch.statusRequest
                firstRequest = false
                do {
                    let status: InboxTransport.ReadBatchStatus = try await api.request(body)
                    guard self.epoch == life, self.readBatchTaskRevision == revision, status.jobId == batch.id else { return }
                    switch status.state {
                    case .complete, .expired:
                        self.finishReadBatch(batch, failed: status.state == .expired)
                        return
                    case .queued, .running:
                        break
                    }
                } catch {
                    guard self.epoch == life, self.readBatchTaskRevision == revision, !Task.isCancelled else { return }
                    if let transport = error as? InboxTransportError, case let .response(failure) = transport {
                        if failure.accountUnauthorized { self.purgeCloud(); return }
                        if failure.status == 404 || failure.status == 403 {
                            self.finishReadBatch(batch, failed: true)
                            return
                        }
                    }
                }
                do { try await Task.sleep(for: .seconds(2)) } catch { return }
            }
        }
    }

    private func finishReadBatch(_ batch: InboxReadBatch, failed: Bool) {
        guard state.readBatch == batch else { return }
        var candidate = state
        candidate.readBatch = nil
        guard commit(candidate) else { return }
        markingAllRead = false
        if failed { issue = .readBatchUnavailable }
        requestRefresh(delay: .zero)
        refreshActive()
    }


    func preferences(_ rows: [InboxRow], patch: InboxPreferencePatch) {
        apply(rows.map { ($0, InboxOperation(preferences: patch, row: $0)) })
    }

    func canMovePin(_ row: InboxRow, by offset: Int) -> Bool { adjacentPin(row, by: offset) != nil }

    func movePin(_ row: InboxRow, by offset: Int) {
        guard let other = adjacentPin(row, by: offset), let first = row.pinOrder, let second = other.pinOrder else { return }
        apply([
            (row, InboxOperation(preferences: .init(pinOrder: second), row: row)),
            (other, InboxOperation(preferences: .init(pinOrder: first), row: other))
        ])
    }

    private func adjacentPin(_ row: InboxRow, by offset: Int) -> InboxRow? {
        let pinned = visible.filter { $0.pinOrder != nil }.sorted(by: InboxRow.ordered)
        guard let current = pinned.firstIndex(where: { $0.ref == row.ref }),
              pinned.indices.contains(current + offset),
              pinned[current + offset].pinOrder != row.pinOrder else { return nil }
        return pinned[current + offset]
    }

    private func apply(_ operations: [(InboxRow, InboxOperation)], flushDelay: Duration = .seconds(1)) {
        var candidate = state
        var network: [InboxOperation] = []
        for (row, op) in operations {
            if row.ref.kind == .assistant && (local[row.key] != nil || availableRemoteActivity[row.key] != nil) {
                candidate.applyLocal(op, row: row)
                if local[row.key] != nil {
                    if allowsAssistantSync(row.key) && localActivity[row.key] != nil { network.append(op) }
                } else if (remote + cached).contains(where: { $0.ref == row.ref }) { network.append(op) }
            } else if uid != nil {
                network.append(op)
            }
        }
        do { try candidate.enqueue(network, inFlight: inFlight) }
        catch { issue = .backpressure; return }
        if commit(candidate) { rebuild(); flushSoon(delay: flushDelay) }
    }

    @discardableResult
    private func admit(_ operations: [InboxOperation]) -> Bool {
        var candidate = state
        do { try candidate.enqueue(operations, inFlight: inFlight) }
        catch { issue = .backpressure; return false }
        guard commit(candidate) else { return false }
        flushSoon()
        return true
    }

    @discardableResult
    private func commit(_ candidate: InboxAccountState) -> Bool {
        guard storageAvailable else { issue = .storage; return false }
        do {
            try files.save(candidate)
            state = candidate
            pendingByKey = Dictionary(grouping: candidate.queue, by: \.key)
            return true
        } catch { issue = .storage; return false }
    }

    func draft(for key: String, uid expected: String?) -> Data? {
        _ = personalRevision
        guard expected == uid else { return nil }
        return state.drafts[key]
    }

    func saveDraft(_ data: Data?, for key: String, uid expected: String?) {
        guard expected == uid, state.drafts[key] != data else { return }
        var candidate = state
        candidate.drafts[key] = data
        if commit(candidate) { personalRevision &+= 1 }
    }

    private func flushSoon(delay: Duration = .seconds(1)) {
        guard let uid, !state.queue.isEmpty, flush == nil else { return }
        let life = epoch
        flush = Task { [weak self] in
            do { try await Task.sleep(for: delay) } catch { return }
            await self?.drain(uid: uid, life: life)
        }
    }

    private func drain(uid: String, life: Int) async {
        var retry = false
        defer {
            if epoch == life {
                flush = nil; inFlight = nil
                if retry { flushSoon(delay: .seconds(5)) }
                else if !state.queue.isEmpty { flushSoon() }
                requestRefresh()
            }
        }
        let api = InboxTransport(uid: uid)
        while epoch == life, !Task.isCancelled, let operation = state.queue.first {
            inFlight = operation.id
            var request = operation
            if let ref = InboxRef(key: request.key, generation: request.generation),
               ref.kind == .assistant, local[ref.key] != nil,
               let boundGeneration = state.assistantGenerations[ref.sourceId] {
                request.generation = boundGeneration
            }
            let authority = cloudAuthorityRevision
            var source: InboxTransport.AssistantSource?
            do {
                let result = try await api.operation(request)
                if cloudAuthorityRevision == authority { source = result.source }
                if let row = result.row, epoch == life, cloudAuthorityRevision == authority {
                    verifyCloud([row])
                    if let index = remote.firstIndex(where: { $0.ref == row.ref }) { remote[index] = row }
                    if let index = cached.firstIndex(where: { $0.ref == row.ref }) { cached[index] = row }
                    if current == row.ref { verifiedActive = row }
                }
            }
            catch {
                guard epoch == life, !Task.isCancelled else { return }
                if let transport = error as? InboxTransportError, case let .response(failure) = transport {
                    let manualConflict = operation.action == .markUnread &&
                        ["MANUAL_REVISION_REQUIRED", "MANUAL_REVISION_CONFLICT"].contains(failure.code ?? "")
                    let preferenceConflict = operation.action == .preferences &&
                        ["PREFERENCE_REVISION_REQUIRED", "PREFERENCE_REVISION_CONFLICT"].contains(failure.code ?? "")
                    if manualConflict || preferenceConflict {
                        var candidate = state
                        candidate.queue.removeAll { pending in
                            pending.id == operation.id || (preferenceConflict && pending.action == .preferences &&
                                pending.key == operation.key && pending.generation == operation.generation)
                        }
                        guard commit(candidate) else { retry = true; return }
                        issue = preferenceConflict ? .preferenceConflict : .unavailable
                        continue
                    }
                }
                if let failure = (error as? InboxTransportError)?.authority {
                    issue = .unavailable
                    if failure.accountUnauthorized {
                        purgeCloud()
                        retry = true
                        return
                    }
                    if let rejected = InboxRef(key: request.key, generation: request.generation) {
                        purgeCloud(rejected: rejected, failure: failure)
                        var candidate = state
                        candidate.discardRejected(operation, rejected: rejected, failure: failure)
                        guard commit(candidate) else { retry = true; return }
                        continue
                    }
                    purgeCloud()
                } else {
                    issue = .sync
                    retry = true
                    return
                }
            }
            guard epoch == life else { return }
            var candidate = state
            candidate.queue.removeAll { $0.id == operation.id }
            if let source, let ref = InboxRef(key: operation.key, generation: operation.generation) {
                candidate.assistantGenerations[ref.sourceId] = source.enabled ? source.generation : nil
            }
            guard commit(candidate) else { retry = true; return }
            if source != nil { personalRevision &+= 1 }
        }
        rebuild()
    }

    /// The caller supplies a contiguous, visibly read cursor. Reference and
    /// search previews never call this method.
    func sawContent(_ row: InboxRow, through: Int) {
        guard foreground, current == row.ref, through > row.readThrough else { return }
        let read = row.frozenRead(through: through)
        if let stagedRead, stagedRead.key == read.key, stagedRead.through >= read.through { return }
        stagedRead = read
        readTask?.cancel()
        readTask = Task { [weak self] in
            do { try await Task.sleep(for: .seconds(1)) } catch { return }
            self?.leaveReadContext()
        }
    }

    func leaveReadContext() {
        readTask?.cancel(); readTask = nil
        guard let read = stagedRead else { return }
        stagedRead = nil
        guard let ref = InboxRef(key: read.key, generation: read.generation), let original = row(ref) else { return }
        apply([(original, InboxOperation(read))], flushDelay: .zero)
    }

    private func updatePresence() {
        presence?.cancel()
        guard let uid, let device = PushRegistration.shared.remoteInstallationId else { return }
        let life = epoch
        let active = foreground
        let ref = current.flatMap { ref in
            ref.kind == .assistant && local[ref.key] != nil && !allowsAssistantSync(ref.key) ? nil : ref
        }
        presence = Task { [weak self] in
            repeat {
                guard self?.epoch == life, !Task.isCancelled else { return }
                var body: [String: Any] = [
                    "action": "presence", "deviceId": device, "key": ref?.key as Any? ?? NSNull(), "foreground": active
                ]
                if let ref { body["generation"] = ref.generation }
                let _: InboxTransport.Ack? = try? await InboxTransport(uid: uid).request(body)
                guard active else { return }
                do { try await Task.sleep(for: .seconds(25)) } catch { return }
            } while !Task.isCancelled
        }
    }

    private func updateBadge() {
        let localCount = local.values.lazy.map(applyingPending).filter { row in
            !row.archived && (row.isUnread || row.needsAttention) &&
                !(self.allowsAssistantSync(row.key) && self.state.assistantGenerations[row.ref.sourceId] != nil)
        }.count
        let total = min(100, badgeFromServer + localCount)
        guard unreadBadge != total else { return }
        unreadBadge = total
        setApplicationBadge(total)
    }

    private func setApplicationBadge(_ value: Int) {
        guard !IsolatedSimulatorTesting.enabled else { return }
        Task { try? await UNUserNotificationCenter.current().setBadgeCount(value) }
    }

    private func refreshNotices(delay: Duration = .milliseconds(250)) {
        noticeRefresh?.cancel()
        guard foreground, let uid, let device = PushRegistration.shared.remoteInstallationId else { return }
        let life = epoch
        noticeRefresh = Task { [weak self] in
            do { try await Task.sleep(for: delay) } catch { return }
            guard let self, NotificationPreferencesStore.load().enabled,
                  let page: InboxTransport.Notices = try? await InboxTransport(uid: uid).request(["action": "notices", "deviceId": device]),
                  self.epoch == life, self.foreground, !Task.isCancelled else { return }
            let now = InboxPush.now
            var next: Int64?
            for item in page.items.reversed() where item.deliverable && self.state.delivered[item.id] == nil {
                guard let ref = InboxRef(key: item.key, generation: item.generation) else { continue }
                if item.deliverAfter > now {
                    next = min(next ?? item.deliverAfter, item.deliverAfter)
                    continue
                }
                await self.receive(InboxPush(ref: ref, recipient: uid, through: item.through, eventKind: item.kind,
                                            eventID: item.id, expiresAt: item.lastActivityAt + 3_600_000))
            }
            if let next, self.epoch == life, self.foreground, !Task.isCancelled {
                self.refreshNotices(delay: .milliseconds(max(1, next - InboxPush.now)))
            }
        }
    }

    func receive(_ push: InboxPush) async {
        guard foreground, uid == push.recipient, state.delivered[push.eventID] == nil else { return }
        let life = epoch
        guard let row = await authorized(push.ref), life == epoch, row.notificationMode != .none,
              row.readThrough < push.through || push.isAction && row.pendingActionCount > 0 else { return }
        var candidate = state
        guard candidate.admitDelivery(push.eventID, now: InboxPush.now, expiresAt: push.expiresAt),
              commit(candidate) else { return }
        if current == push.ref { inlineNotice = push }
        else { notice = push }
    }

    func open(_ push: InboxPush) async {
        guard push.expiresAt > InboxPush.now else { return }
        guard initialized && uid == push.recipient else {
            pendingPush = push
            return
        }
        routeRequest &+= 1
        let request = routeRequest
        let life = epoch
        notice = nil
        guard let row = await authorized(push.ref), life == epoch, routeRequest == request else { return }
        route = row
    }

    private func refreshActive() {
        activeRefresh?.cancel()
        guard let ref = current, uid != nil else { return }
        if ref.kind == .assistant && local[ref.key] != nil && !allowsAssistantSync(ref.key) { return }
        let life = epoch
        activeRefresh = Task { [weak self] in
            guard let self, let row = await self.authorized(ref), self.epoch == life, self.current == ref, !Task.isCancelled else { return }
            self.verifiedActive = row
            self.scheduleCloudExpiration()
            self.rebuild()
        }
    }

    func authorized(_ ref: InboxRef) async -> InboxRow? {
        guard let uid, local[ref.key] == nil || allowsAssistantSync(ref.key) else { return nil }
        let life = epoch
        let authority = cloudAuthorityRevision
        do {
            let response: InboxTransport.Item = try await InboxTransport(uid: uid).request([
                "action": "get", "key": ref.key, "generation": ref.generation
            ])
            guard epoch == life, cloudAuthorityRevision == authority, response.item.valid, response.item.ref == ref else { return nil }
            verifyCloud([response.item])
            return response.item
        } catch {
            if epoch == life, cloudAuthorityRevision == authority, let failure = (error as? InboxTransportError)?.authority {
                if failure.accountUnauthorized { purgeCloud() }
                else {
                    purgeCloud(rejected: ref, failure: failure)
                    var candidate = state
                    candidate.queue.removeAll {
                        guard $0.id != inFlight, let target = InboxRef(key: $0.key, generation: $0.generation) else { return false }
                        return failure.revokes(target, rejected: ref)
                    }
                    _ = commit(candidate)
                }
            }
            return nil
        }
    }
}
