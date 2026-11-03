import CryptoKit
import Darwin
import Foundation

private struct CronHistoryEnvelope: Codable {
    let version: Int
    let payload: String
    let sha256: String
}

private struct CronHistoryPayload: Codable {
    let version: Int
    let runs: [CronRunRecord]
}

actor CronRunHistoryStore {
    private let fileURL: URL
    private let now: @Sendable () -> UInt64
    private let newID: @Sendable () -> String
    private let writer: ProjectAtomicWriting

    init(
        fileURL: URL,
        now: @escaping @Sendable () -> UInt64 = { UInt64(Date().timeIntervalSince1970 * 1000) },
        newID: @escaping @Sendable () -> String = { UUID().uuidString.lowercased() },
        writer: ProjectAtomicWriting = DefaultProjectAtomicWriter()
    ) {
        self.fileURL = fileURL
        self.now = now
        self.newID = newID
        self.writer = writer
    }

    /// Startup admission uses the same checksummed history format before an
    /// empty generated-session index can be established.
    nonisolated static func storedSessionReceipts(fileURL: URL) throws -> [CronRunRecord] {
        let expectation = try ownerCatalogExpectation(fileURL)
        try validateHistoryWitnessIfPresent(fileURL)
        guard let data = expectation.bytes else {
            guard try !ownerCatalogFileExists(historyWitnessURL(fileURL)) else { throw historyMissingError() }
            return []
        }
        try UniqueJSONKeyGuard.validate(data)
        let envelope = try JSONDecoder().decode(CronHistoryEnvelope.self, from: data)
        guard envelope.version == 2, envelope.sha256 == sha256(envelope.payload) else {
            throw CronExecutionError(kind: .system, message: "cron history checksum mismatch", statusOverride: .failed)
        }
        let payloadData = Data(envelope.payload.utf8)
        try UniqueJSONKeyGuard.validate(payloadData)
        let payload = try JSONDecoder().decode(CronHistoryPayload.self, from: payloadData)
        guard payload.version == 1 else {
            throw CronExecutionError(kind: .system, message: "unsupported cron history version", statusOverride: .failed)
        }
        try withNonblockingHistoryLock(fileURL) {
            guard try ownerCatalogExpectation(fileURL) == expectation else { throw historyMissingError() }
            if try !ownerCatalogFileExists(historyWitnessURL(fileURL)) { try ensureHistoryWitness(fileURL) }
            guard try ownerCatalogExpectation(fileURL) == expectation else { throw historyMissingError() }
        }
        return payload.runs.filter { $0.sessionID != nil }
    }

    func records() async throws -> [CronRunRecord] {
        try await withHistoryLock {
            try readRecords().sorted { lhs, rhs in
                if lhs.triggeredAtMs == rhs.triggeredAtMs { return lhs.runID > rhs.runID }
                return lhs.triggeredAtMs > rhs.triggeredAtMs
            }
        }
    }

    func record(runID: String) async throws -> CronRunRecord? {
        try await withHistoryLock { try readRecords().first { $0.runID == runID } }
    }

    func occurrence(
        scopeID: String,
        taskID: String,
        scheduledAtMs: UInt64,
        manual: Bool
    ) async throws -> CronRunRecord? {
        try await withHistoryLock {
            try readRecords().first {
                $0.scopeID == scopeID &&
                    $0.taskID == taskID &&
                    $0.scheduledAtMs == scheduledAtMs &&
                    $0.manual == manual
            }
        }
    }

    func claim(
        scope: CronScope,
        taskID: String,
        prompt: String,
        scheduledAtMs: UInt64,
        triggeredAtMs: UInt64? = nil,
        manual: Bool = false,
        notificationPolicy: CronNotificationPolicy? = nil
    ) async throws -> CronRunRecord? {
        return try await withHistoryLock {
            var (current, expected) = try readRecordsWithExpectation()
            if current.contains(where: {
                $0.scopeID == scope.scopeID &&
                    $0.taskID == taskID &&
                    $0.scheduledAtMs == scheduledAtMs &&
                    $0.manual == manual
            }) {
                return nil
            }
            if current.contains(where: {
                $0.scopeID == scope.scopeID &&
                    $0.taskID == taskID &&
                    !$0.status.isTerminal
            }) {
                return nil
            }
            let record = CronRunRecord(
                runID: newID(),
                taskID: taskID,
                scopeID: scope.scopeID,
                projectID: scope.projectID,
                projectName: scope.projectName,
                prompt: prompt,
                scheduledAtMs: scheduledAtMs,
                triggeredAtMs: triggeredAtMs ?? now(),
                startedAtMs: nil,
                finishedAtMs: nil,
                status: .queued,
                attempt: 0,
                resultText: nil,
                errorMessage: nil,
                errorKind: nil,
                manual: manual,
                notificationPolicy: notificationPolicy
            )
            current.append(record)
            try writeRecords(current, expected: expected)
            return record
        }
    }

    func cancelQueued(scopeID: String, taskID: String) async throws -> Int {
        return try await withHistoryLock {
            var (records, expected) = try readRecordsWithExpectation()
            var count = 0
            for i in records.indices where records[i].scopeID == scopeID && records[i].taskID == taskID && records[i].status == .queued {
                records[i].status = .cancelled
                records[i].finishedAtMs = now()
                count += 1
            }
            if count > 0 { try writeRecords(records, expected: expected) }
            return count
        }
    }

    /// Capture policy for pre-snapshot queued/running rows before they finish.
    /// Existing terminal history remains untouched to avoid retroactive alerts.
    func snapshotNotificationPolicy(runID: String, policy: CronNotificationPolicy) async throws {
        _ = try await update(runID: runID) { run in
            guard !run.status.isTerminal, run.notificationPolicy == nil else { return run }
            var updated = run
            updated.notificationPolicy = policy
            return updated
        }
    }

    @discardableResult
    func markRunning(runID: String, attempt: Int) async throws -> CronRunRecord? {
        try await update(runID: runID) { run in
            guard !run.status.isTerminal else { return run }
            var updated = run
            updated.status = .running
            updated.startedAtMs = run.startedAtMs ?? now()
            updated.attempt = attempt
            updated.errorMessage = nil
            updated.errorKind = nil
            return updated
        }
    }

    /// Park a run that failed transiently so a later wake resumes it, keeping
    /// the attempt count and the reason. Mirrors Android `markRetry`, whose
    /// `Result.retry()` hands the same job back to WorkManager.
    @discardableResult
    func markRetry(runID: String, attempt: Int, message: String) async throws -> CronRunRecord? {
        try await update(runID: runID) { run in
            guard !run.status.isTerminal else { return run }
            var updated = run
            updated.status = .queued
            updated.attempt = attempt
            updated.errorMessage = truncateUtf8(message, maxBytes: maxCronResultBytes)
            return updated
        }
    }

    @discardableResult
    func markTerminal(
        runID: String,
        status: CronRunStatus,
        resultText: String? = nil,
        errorMessage: String? = nil,
        errorKind: CronRunErrorKind? = nil,
        sessionID: String? = nil,
        sessionTranscriptPath: String? = nil,
        sessionMode: SessionMode? = nil,
        sessionScopeKey: String? = nil,
        actualModel: String? = nil
    ) async throws -> CronRunRecord? {
        precondition(status.isTerminal)
        return try await update(runID: runID) { run in
            guard !run.status.isTerminal else { return run }
            var updated = run
            updated.status = status
            updated.startedAtMs = run.startedAtMs ?? now()
            updated.finishedAtMs = now()
            updated.resultText = resultText.map { truncateUtf8($0, maxBytes: maxCronResultBytes) }
            updated.errorMessage = errorMessage.map { truncateUtf8($0, maxBytes: maxCronResultBytes) }
            updated.errorKind = errorKind
            updated.sessionID = sessionID ?? run.sessionID
            updated.sessionTranscriptPath = sessionTranscriptPath ?? run.sessionTranscriptPath
            updated.sessionMode = sessionMode ?? run.sessionMode
            updated.sessionScopeKey = sessionScopeKey ?? run.sessionScopeKey
            updated.actualModel = actualModel ?? run.actualModel
            return updated
        }
    }

    /// Persist the delivery reservation before touching the notification service.
    /// Repeated recovery and reopened stores cannot deliver the same run again.
    func claimTerminalNotification(runID: String) async throws -> CronRunRecord? {
        return try await withHistoryLock {
            let current = try readRecords()
            pruneNotificationClaims(retaining: current)
            guard let index = current.firstIndex(where: { $0.runID == runID }),
                  current[index].status.isTerminal else { return nil }
            // The shared history lock makes retention and reservation one transaction.
            // Exclusive creation additionally preserves existing delivery receipts.
            // A stale caller must re-read the retained history before reserving.
            let claims = fileURL.appendingPathExtension("notification-claims")
            try FileManager.default.createDirectory(at: claims, withIntermediateDirectories: true)
            let marker = claims.appendingPathComponent(sha256(runID))
            let descriptor = marker.path.withCString { open($0, O_WRONLY | O_CREAT | O_EXCL, 0o600) }
            guard descriptor >= 0 else {
                if errno == EEXIST { return nil }
                throw POSIXError(POSIXErrorCode(rawValue: errno) ?? .EIO)
            }
            close(descriptor)
            return current[index]
        }
    }

    /// Give a reservation back when the notification service refused to show it.
    ///
    /// 🚨 Without this the claim is a one-way door: `claimTerminalNotification`
    /// writes the marker BEFORE delivery, and `UserNotificationCronNotifier`
    /// silently shows nothing when the scheduled-run preference is off, when
    /// authorization is denied, or when the request is rejected. The run would
    /// then be marked notified forever — turning the preference on later, or
    /// granting permission later, re-enters `recoverTerminalNotifications` and
    /// hits `EEXIST`. Both mirrors already return the key: Android's
    /// `CronRunHistoryStore.releaseNotificationClaim` and Electron's
    /// `this.delivered.delete(key)`.
    func releaseTerminalNotification(runID: String) async throws {
        try await withHistoryLock {
            let claims = fileURL.appendingPathExtension("notification-claims")
            let marker = claims.appendingPathComponent(sha256(runID))
            // Missing is the same outcome as removed: the next recovery may claim.
            try? FileManager.default.removeItem(at: marker)
        }
    }

    @discardableResult
    func cancelUnfinished(scopeID: String, taskID: String, message: String) async throws -> [CronRunRecord] {
        return try await withHistoryLock {
            var (current, expected) = try readRecordsWithExpectation()
            var cancelled: [CronRunRecord] = []
            for index in current.indices {
                guard current[index].scopeID == scopeID, current[index].taskID == taskID, !current[index].status.isTerminal else {
                    continue
                }
                current[index].status = .cancelled
                current[index].startedAtMs = current[index].startedAtMs ?? now()
                current[index].finishedAtMs = now()
                current[index].errorMessage = truncateUtf8(message, maxBytes: maxCronResultBytes)
                current[index].errorKind = .cancelled
                cancelled.append(current[index])
            }
            if !cancelled.isEmpty {
                try writeRecords(current, expected: expected)
            }
            return cancelled
        }
    }

    private func update(
        runID: String,
        transform: (CronRunRecord) -> CronRunRecord
    ) async throws -> CronRunRecord? {
        return try await withHistoryLock {
            var (current, expected) = try readRecordsWithExpectation()
            guard let index = current.firstIndex(where: { $0.runID == runID }) else { return nil }
            let next = transform(current[index])
            current[index] = next
            try writeRecords(current, expected: expected)
            return next
        }
    }

    /// No suspension is allowed inside a transaction. Separate actors and app
    /// processes share this lock, so a writer cannot restore a pruned record.
    private func withHistoryLock<T>(_ operation: () throws -> T) async throws -> T {
        try FileManager.default.createDirectory(at: fileURL.deletingLastPathComponent(), withIntermediateDirectories: true)
        let lockURL = fileURL.appendingPathExtension("lock")
        let descriptor = lockURL.path.withCString { open($0, O_WRONLY | O_CREAT | O_NOFOLLOW, 0o600) }
        guard descriptor >= 0 else { throw POSIXError(POSIXErrorCode(rawValue: errno) ?? .EIO) }
        defer { close(descriptor) }
        try Task.checkCancellation()
        while flock(descriptor, LOCK_EX | LOCK_NB) != 0 {
            let failure = errno
            if failure == EINTR { continue }
            guard failure == EWOULDBLOCK || failure == EAGAIN else {
                throw POSIXError(POSIXErrorCode(rawValue: failure) ?? .EIO)
            }
            // Contention can span processes. Suspend rather than block every
            // cooperative Swift worker while another task holds the lock.
            try await Task.sleep(for: .milliseconds(10))
        }
        defer { flock(descriptor, LOCK_UN) }
        try Task.checkCancellation()
        return try operation()
    }

    /// Called only while holding the history lock, after the retained history
    /// has been committed. Failed cleanup is safe and retried on the next write
    /// or reservation; a missing record can no longer reserve a notification.
    private func pruneNotificationClaims(retaining records: [CronRunRecord]) {
        let claims = fileURL.appendingPathExtension("notification-claims")
        guard let markers = try? FileManager.default.contentsOfDirectory(at: claims, includingPropertiesForKeys: nil) else { return }
        let retained = Set(records.map { sha256($0.runID) })
        for marker in markers where !retained.contains(marker.lastPathComponent) {
            try? FileManager.default.removeItem(at: marker)
        }
    }

    private func readRecords() throws -> [CronRunRecord] {
        try readRecordsWithExpectation().0
    }

    private func readRecordsWithExpectation() throws -> ([CronRunRecord], OwnerCatalogExpectation) {
        let expected = try ownerCatalogExpectation(fileURL)
        guard let data = expected.bytes else {
            guard try !ownerCatalogFileExists(historyWitnessURL(fileURL)) else { throw historyMissingError() }
            try ensureHistoryWitness(fileURL)
            try writeRecords([], expected: .absent)
            return try readRecordsWithExpectation()
        }
        try UniqueJSONKeyGuard.validate(data)
        let envelope = try JSONDecoder().decode(CronHistoryEnvelope.self, from: data)
        guard envelope.version == 2, envelope.sha256 == sha256(envelope.payload) else {
            throw CronExecutionError(
                kind: .system,
                message: "cron history checksum mismatch",
                statusOverride: .failed
            )
        }
        let payloadData = Data(envelope.payload.utf8)
        try UniqueJSONKeyGuard.validate(payloadData)
        let payload = try JSONDecoder().decode(CronHistoryPayload.self, from: payloadData)
        guard payload.version == 1 else {
            throw CronExecutionError(kind: .system, message: "unsupported cron history version", statusOverride: .failed)
        }
        try ensureHistoryWitness(fileURL)
        guard try ownerCatalogExpectation(fileURL) == expected else { throw historyMissingError() }
        return (payload.runs, expected)
    }

    private func writeRecords(_ records: [CronRunRecord], expected: OwnerCatalogExpectation) throws {
        // The witness is durable before the first history publication. A crash
        // in between is an interrupted initialization, never a fresh install.
        try ensureHistoryWitness(fileURL)
        let retained = retainCronRunHistory(records)
        let payload = CronHistoryPayload(version: 1, runs: retained)
        let payloadData = try JSONEncoder().encode(payload)
        guard let payloadString = String(data: payloadData, encoding: .utf8) else {
            throw CronExecutionError(
                kind: .system,
                message: "cron history payload encoding failed",
                statusOverride: .failed
            )
        }
        let envelope = CronHistoryEnvelope(
            version: 2,
            payload: payloadString,
            sha256: sha256(payloadString)
        )
        let data = try JSONEncoder().encode(envelope)
        try writer.writeData(data, to: fileURL, expected: expected, validate: { staged in
            let decoded = try JSONDecoder().decode(CronHistoryEnvelope.self, from: staged)
            guard decoded.sha256 == sha256(decoded.payload) else {
                throw CronExecutionError(
                    kind: .system,
                    message: "cron history checksum mismatch after staging",
                    statusOverride: .failed
                )
            }
        }, commit: { try $0() })
        try synchronizeHistory(fileURL, expectedBytes: data)
        pruneNotificationClaims(retaining: retained)
    }
}

private func historyWitnessURL(_ fileURL: URL) -> URL { fileURL.appendingPathExtension("observed") }
private let historyWitnessBytes = Data("lingxi-cron-history-observed-v1\n".utf8)

private func validateHistoryWitnessIfPresent(_ fileURL: URL) throws {
    if let marker = try ownerCatalogExpectation(historyWitnessURL(fileURL)).bytes,
       marker != historyWitnessBytes { throw historyMissingError() }
}

private func historyMissingError() -> CronExecutionError {
    CronExecutionError(kind: .system, message: "cron history is missing or changed after it was observed", statusOverride: .failed)
}

/// Admission cannot wait synchronously on an actor or another process. A busy
/// history transaction is rejected and can be retried after that writer ends.
private func withNonblockingHistoryLock<T>(_ fileURL: URL, _ operation: () throws -> T) throws -> T {
    let lockURL = fileURL.appendingPathExtension("lock")
    let descriptor = lockURL.path.withCString { open($0, O_WRONLY | O_CREAT | O_NOFOLLOW | O_CLOEXEC, 0o600) }
    guard descriptor >= 0 else { throw POSIXError(POSIXErrorCode(rawValue: errno) ?? .EIO) }
    defer { close(descriptor) }
    guard flock(descriptor, LOCK_EX | LOCK_NB) == 0 else { throw POSIXError(POSIXErrorCode(rawValue: errno) ?? .EIO) }
    defer { flock(descriptor, LOCK_UN) }
    return try operation()
}

private func synchronizeHistory(_ url: URL, expectedBytes: Data) throws {
    try synchronizeOwnerCatalog(url, expectedBytes: expectedBytes)
}

/// Must be called under the history lock. Publication of this witness is
/// durable before first creating history, so an interrupted initialization is
/// distinguishable from an installation that has never stored history.
private func ensureHistoryWitness(_ fileURL: URL) throws {
    let witness = historyWitnessURL(fileURL)
    let marker = historyWitnessBytes
    if let current = try ownerCatalogExpectation(witness).bytes {
        guard current == marker else { throw historyMissingError() }
        try synchronizeHistory(witness, expectedBytes: marker)
        return
    }
    let parent = witness.deletingLastPathComponent()
    try FileManager.default.createDirectory(at: parent, withIntermediateDirectories: true)
    let descriptor = witness.path.withCString { open($0, O_WRONLY | O_CREAT | O_EXCL | O_NOFOLLOW | O_CLOEXEC, 0o600) }
    guard descriptor >= 0 else { throw POSIXError(POSIXErrorCode(rawValue: errno) ?? .EIO) }
    defer { close(descriptor) }
    var offset = 0
    try marker.withUnsafeBytes { raw in
        while offset < raw.count {
            let count = Darwin.write(descriptor, raw.baseAddress!.advanced(by: offset), raw.count - offset)
            guard count > 0 else { throw POSIXError(POSIXErrorCode(rawValue: errno) ?? .EIO) }
            offset += count
        }
    }
    guard fsync(descriptor) == 0 else { throw POSIXError(POSIXErrorCode(rawValue: errno) ?? .EIO) }
    let parentDescriptor = parent.path.withCString { open($0, O_RDONLY | O_CLOEXEC) }
    guard parentDescriptor >= 0 else { throw POSIXError(POSIXErrorCode(rawValue: errno) ?? .EIO) }
    defer { close(parentDescriptor) }
    guard fsync(parentDescriptor) == 0 else { throw POSIXError(POSIXErrorCode(rawValue: errno) ?? .EIO) }
}

func retainCronRunHistory(
    _ records: [CronRunRecord],
    perTaskLimit: Int = maxCronRunsPerTask,
    totalLimit: Int = maxCronRunsTotal
) -> [CronRunRecord] {
    precondition(perTaskLimit > 0 && totalLimit > 0)
    let unique = Array(Dictionary(grouping: records, by: \.runID).values.compactMap { group in
        group.max(by: { lhs, rhs in lhs.triggeredAtMs < rhs.triggeredAtMs })
    })
    let active = unique
        .filter { !$0.status.isTerminal }
        .sorted { lhs, rhs in lhs.triggeredAtMs > rhs.triggeredAtMs }
    var perTaskCounts: [String: Int] = [:]
    active.forEach { run in
        perTaskCounts["\(run.scopeID)|\(run.taskID)", default: 0] += 1
    }
    let terminalAllowance = max(0, totalLimit - active.count)
    var retainedTerminal: [CronRunRecord] = []
    let terminal = unique
        .filter(\.status.isTerminal)
        .sorted(by: { lhs, rhs in lhs.triggeredAtMs > rhs.triggeredAtMs })
    let activeTaskKeys = Set(active.map { "\($0.scopeID)|\($0.taskID)" })

    // Preserve recent context for currently queued/running tasks before the
    // global cap is filled by unrelated tasks. This keeps per-task history
    // useful while still enforcing the hard total bound.
    for run in terminal where activeTaskKeys.contains("\(run.scopeID)|\(run.taskID)") {
        if retainedTerminal.count >= terminalAllowance { break }
        let key = "\(run.scopeID)|\(run.taskID)"
        let current = perTaskCounts[key, default: 0]
        guard current < perTaskLimit else { continue }
        retainedTerminal.append(run)
        perTaskCounts[key] = current + 1
    }
    let retainedIDs = Set(retainedTerminal.map(\.runID))
    for run in terminal where !retainedIDs.contains(run.runID) {
        if retainedTerminal.count >= terminalAllowance { break }
        let key = "\(run.scopeID)|\(run.taskID)"
        let current = perTaskCounts[key, default: 0]
        guard current < perTaskLimit else { continue }
        retainedTerminal.append(run)
        perTaskCounts[key] = current + 1
    }
    return (active + retainedTerminal)
        .sorted { lhs, rhs in
            if lhs.triggeredAtMs == rhs.triggeredAtMs { return lhs.runID > rhs.runID }
            return lhs.triggeredAtMs > rhs.triggeredAtMs
        }
}

func truncateUtf8(_ value: String, maxBytes: Int) -> String {
    precondition(maxBytes >= 0)
    guard value.lengthOfBytes(using: .utf8) > maxBytes else { return value }
    var low = 0
    var high = value.count
    while low < high {
        let middle = (low + high + 1) >> 1
        let candidate = String(value.prefix(middle))
        if candidate.lengthOfBytes(using: .utf8) <= maxBytes {
            low = middle
        } else {
            high = middle - 1
        }
    }
    return String(value.prefix(low))
}

private func sha256(_ value: String) -> String {
    SHA256.hash(data: Data(value.utf8))
        .map { String(format: "%02x", $0) }
        .joined()
}
