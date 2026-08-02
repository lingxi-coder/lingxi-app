import CryptoKit
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

    init(
        fileURL: URL,
        now: @escaping @Sendable () -> UInt64 = { UInt64(Date().timeIntervalSince1970 * 1000) },
        newID: @escaping @Sendable () -> String = { UUID().uuidString.lowercased() }
    ) {
        self.fileURL = fileURL
        self.now = now
        self.newID = newID
    }

    func records() async throws -> [CronRunRecord] {
        try readRecords().sorted { lhs, rhs in
            if lhs.triggeredAtMs == rhs.triggeredAtMs { return lhs.runID > rhs.runID }
            return lhs.triggeredAtMs > rhs.triggeredAtMs
        }
    }

    func record(runID: String) async throws -> CronRunRecord? {
        try readRecords().first { $0.runID == runID }
    }

    func occurrence(
        scopeID: String,
        taskID: String,
        scheduledAtMs: UInt64,
        manual: Bool
    ) async throws -> CronRunRecord? {
        try readRecords().first {
            $0.scopeID == scopeID &&
                $0.taskID == taskID &&
                $0.scheduledAtMs == scheduledAtMs &&
                $0.manual == manual
        }
    }

    func claim(
        scope: CronScope,
        taskID: String,
        prompt: String,
        scheduledAtMs: UInt64,
        triggeredAtMs: UInt64? = nil,
        manual: Bool = false
    ) async throws -> CronRunRecord? {
        var current = try readRecords()
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
            manual: manual
        )
        current.append(record)
        try writeRecords(current)
        return record
    }

    @discardableResult
    func markRunning(runID: String, attempt: Int) async throws -> CronRunRecord? {
        try update(runID: runID) { run in
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

    @discardableResult
    func markTerminal(
        runID: String,
        status: CronRunStatus,
        resultText: String? = nil,
        errorMessage: String? = nil,
        errorKind: CronRunErrorKind? = nil
    ) async throws -> CronRunRecord? {
        precondition(status.isTerminal)
        return try update(runID: runID) { run in
            guard !run.status.isTerminal else { return run }
            var updated = run
            updated.status = status
            updated.startedAtMs = run.startedAtMs ?? now()
            updated.finishedAtMs = now()
            updated.resultText = resultText.map { truncateUtf8($0, maxBytes: maxCronResultBytes) }
            updated.errorMessage = errorMessage.map { truncateUtf8($0, maxBytes: maxCronResultBytes) }
            updated.errorKind = errorKind
            return updated
        }
    }

    @discardableResult
    func cancelUnfinished(scopeID: String, taskID: String, message: String) async throws -> [CronRunRecord] {
        var current = try readRecords()
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
            try writeRecords(current)
        }
        return cancelled
    }

    private func update(
        runID: String,
        transform: (CronRunRecord) -> CronRunRecord
    ) throws -> CronRunRecord? {
        var current = try readRecords()
        guard let index = current.firstIndex(where: { $0.runID == runID }) else { return nil }
        let next = transform(current[index])
        current[index] = next
        try writeRecords(current)
        return next
    }

    private func readRecords() throws -> [CronRunRecord] {
        guard FileManager.default.fileExists(atPath: fileURL.path) else { return [] }
        let data = try Data(contentsOf: fileURL)
        let envelope = try JSONDecoder().decode(CronHistoryEnvelope.self, from: data)
        guard envelope.sha256 == sha256(envelope.payload) else {
            throw CronExecutionError(
                kind: .system,
                message: "cron history checksum mismatch",
                statusOverride: .failed
            )
        }
        let payloadData = Data(envelope.payload.utf8)
        let payload = try JSONDecoder().decode(CronHistoryPayload.self, from: payloadData)
        return payload.runs
    }

    private func writeRecords(_ records: [CronRunRecord]) throws {
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
        try DefaultProjectAtomicWriter().writeData(data, to: fileURL) { staged in
            let decoded = try JSONDecoder().decode(CronHistoryEnvelope.self, from: staged)
            guard decoded.sha256 == sha256(decoded.payload) else {
                throw CronExecutionError(
                    kind: .system,
                    message: "cron history checksum mismatch after staging",
                    statusOverride: .failed
                )
            }
        }
    }
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
