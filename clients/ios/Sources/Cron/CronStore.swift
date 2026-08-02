import Foundation

private enum CronStoreError: LocalizedError {
    case invalidExpression(String)
    case missingTask(String)

    var errorDescription: String? {
        switch self {
        case .invalidExpression(let value):
            return "无效 Cron 表达式：\(value)"
        case .missingTask(let id):
            return "未找到定时任务：\(id)"
        }
    }
}

private struct CronField {
    let allowed: Set<Int>
    let wildcard: Bool

    func matches(_ value: Int) -> Bool { wildcard || allowed.contains(value) }
}

private struct CronExpression {
    let raw: String
    let minute: CronField
    let hour: CronField
    let day: CronField
    let month: CronField
    let weekday: CronField

    init(_ raw: String) throws {
        let parts = raw
            .split(whereSeparator: \.isWhitespace)
            .map(String.init)
        guard parts.count == 5 else { throw CronStoreError.invalidExpression(raw) }
        self.raw = raw
        minute = try CronExpression.parse(parts[0], min: 0, max: 59)
        hour = try CronExpression.parse(parts[1], min: 0, max: 23)
        day = try CronExpression.parse(parts[2], min: 1, max: 31)
        month = try CronExpression.parse(parts[3], min: 1, max: 12)
        weekday = try CronExpression.parse(parts[4], min: 0, max: 7, weekday: true)
    }

    func nextFire(after epochMs: UInt64, calendar: Calendar) -> UInt64? {
        let zoneCalendar = calendar
        let current = Date(timeIntervalSince1970: TimeInterval(epochMs) / 1000)
        let rounded = zoneCalendar.date(
            bySetting: .second,
            value: 0,
            of: current
        ) ?? current
        guard let candidateStart = zoneCalendar.date(
            byAdding: DateComponents(minute: 1),
            to: rounded
        ) else { return nil }
        let limitMinutes = 366 * 24 * 60 * 5
        var candidate = candidateStart
        for _ in 0..<limitMinutes {
            if matches(candidate, calendar: zoneCalendar) {
                return UInt64(candidate.timeIntervalSince1970 * 1000)
            }
            guard let next = zoneCalendar.date(byAdding: .minute, value: 1, to: candidate) else {
                return nil
            }
            candidate = next
        }
        return nil
    }

    func humanDescription() -> String {
        switch raw {
        case "*/15 * * * *": return "每 15 分钟"
        case "0 * * * *": return "每小时整点"
        case "0 9 * * *": return "每天 09:00"
        case "0 9 * * 1-5": return "工作日 09:00"
        case "0 9 * * 1": return "每周一 09:00"
        default: return raw
        }
    }

    private func matches(_ date: Date, calendar: Calendar) -> Bool {
        let components = calendar.dateComponents([.minute, .hour, .day, .month, .weekday], from: date)
        guard
            let minuteValue = components.minute,
            let hourValue = components.hour,
            let dayValue = components.day,
            let monthValue = components.month,
            let weekdayComponent = components.weekday
        else {
            return false
        }
        let weekdayValue = (weekdayComponent + 6) % 7
        let monthMatches = month.matches(monthValue)
        let hourMatches = hour.matches(hourValue)
        let minuteMatches = minute.matches(minuteValue)
        let dayMatches = day.matches(dayValue)
        let weekdayMatches = weekday.matches(weekdayValue)
        let combinedDayMatch: Bool
        switch (day.wildcard, weekday.wildcard) {
        case (true, true): combinedDayMatch = true
        case (true, false): combinedDayMatch = weekdayMatches
        case (false, true): combinedDayMatch = dayMatches
        case (false, false): combinedDayMatch = dayMatches || weekdayMatches
        }
        return monthMatches && hourMatches && minuteMatches && combinedDayMatch
    }

    private static func parse(
        _ raw: String,
        min: Int,
        max: Int,
        weekday: Bool = false
    ) throws -> CronField {
        if raw == "*" {
            return CronField(allowed: Set(min...max), wildcard: true)
        }
        var values = Set<Int>()
        for segment in raw.split(separator: ",") {
            let parts = segment.split(separator: "/", omittingEmptySubsequences: false)
            guard !parts.isEmpty else { throw CronStoreError.invalidExpression(raw) }
            let base = String(parts[0])
            let step = parts.count == 2 ? Int(parts[1]) : nil
            if parts.count > 2 || (parts.count == 2 && (step == nil || step == 0)) {
                throw CronStoreError.invalidExpression(raw)
            }
            let range: ClosedRange<Int>
            if base == "*" {
                range = min...max
            } else if let dash = base.firstIndex(of: "-") {
                guard
                    let start = Int(base[..<dash]),
                    let end = Int(base[base.index(after: dash)...])
                else {
                    throw CronStoreError.invalidExpression(raw)
                }
                range = start...end
            } else if let value = Int(base) {
                range = value...value
            } else {
                throw CronStoreError.invalidExpression(raw)
            }
            guard range.lowerBound >= min, range.upperBound <= max else {
                throw CronStoreError.invalidExpression(raw)
            }
            let strideValue = step ?? 1
            var current = range.lowerBound
            while current <= range.upperBound {
                values.insert(weekday && current == 7 ? 0 : current)
                current += strideValue
            }
        }
        guard !values.isEmpty else { throw CronStoreError.invalidExpression(raw) }
        return CronField(allowed: values, wildcard: false)
    }
}

private struct CronTasksEnvelope: Codable {
    let version: Int
    let tasks: [CronTaskRecord]
}

actor JsonCronStoreClient: CronStoreClient {
    private let fileURL: URL
    private let now: @Sendable () -> UInt64
    private let makeID: @Sendable () -> String
    private let calendar: Calendar

    init(
        fileURL: URL,
        now: @escaping @Sendable () -> UInt64,
        makeID: @escaping @Sendable () -> String,
        calendar: Calendar
    ) {
        self.fileURL = fileURL
        self.now = now
        self.makeID = makeID
        self.calendar = calendar
    }

    func list() async throws -> [CronTaskRecord] {
        try readTasks().sorted(by: taskSort)
    }

    func create(cronExpr: String, prompt: String, recurring: Bool) async throws -> CronTaskRecord {
        let expr = try CronExpression(cronExpr)
        let timestamp = now()
        var tasks = try readTasks()
        let record = CronTaskRecord(
            id: makeID(),
            cron: cronExpr.trimmingCharacters(in: .whitespacesAndNewlines),
            prompt: prompt.trimmingCharacters(in: .whitespacesAndNewlines),
            createdAtMs: timestamp,
            lastFiredAtMs: nil,
            recurring: recurring,
            nextFireMs: expr.nextFire(after: timestamp, calendar: calendar),
            human: expr.humanDescription()
        )
        tasks.append(record)
        try writeTasks(tasks)
        return record
    }

    func update(taskID: String, cronExpr: String, prompt: String, recurring: Bool) async throws -> CronTaskRecord {
        let expr = try CronExpression(cronExpr)
        let timestamp = now()
        var tasks = try readTasks()
        guard let index = tasks.firstIndex(where: { $0.id == taskID }) else {
            throw CronStoreError.missingTask(taskID)
        }
        let previous = tasks[index]
        let anchor = max(timestamp, previous.lastFiredAtMs ?? previous.createdAtMs)
        let updated = CronTaskRecord(
            id: previous.id,
            cron: cronExpr.trimmingCharacters(in: .whitespacesAndNewlines),
            prompt: prompt.trimmingCharacters(in: .whitespacesAndNewlines),
            createdAtMs: previous.createdAtMs,
            lastFiredAtMs: previous.lastFiredAtMs,
            recurring: recurring,
            nextFireMs: expr.nextFire(after: anchor, calendar: calendar),
            human: expr.humanDescription()
        )
        tasks[index] = updated
        try writeTasks(tasks)
        return updated
    }

    func delete(taskID: String) async throws -> Bool {
        let tasks = try readTasks()
        let filtered = tasks.filter { $0.id != taskID }
        guard filtered.count != tasks.count else { return false }
        try writeTasks(filtered)
        return true
    }

    func nextFireTime() async throws -> UInt64? {
        try readTasks()
            .compactMap(\.nextFireMs)
            .min()
    }

    func dueOccurrences(nowMs: UInt64) async throws -> [CronOccurrence] {
        var occurrences: [CronOccurrence] = []
        for task in try readTasks().sorted(by: taskSort) {
            guard let firstDue = task.nextFireMs else { continue }
            guard firstDue <= nowMs else { continue }
            var nextDue = firstDue
            let expr = try CronExpression(task.cron)
            var emitted = 0
            while nextDue <= nowMs, emitted < maxCronOccurrencesPerPass {
                occurrences.append(CronOccurrence(taskID: task.id, scheduledAtMs: nextDue))
                emitted += 1
                guard task.recurring else { break }
                guard let next = expr.nextFire(after: nextDue, calendar: calendar) else { break }
                nextDue = next
            }
        }
        return occurrences.sorted { lhs, rhs in
            if lhs.scheduledAtMs == rhs.scheduledAtMs {
                return lhs.taskID < rhs.taskID
            }
            return lhs.scheduledAtMs < rhs.scheduledAtMs
        }
    }

    func acknowledgeOccurrence(taskID: String, scheduledAtMs: UInt64) async throws -> Bool {
        var tasks = try readTasks()
        guard let index = tasks.firstIndex(where: { $0.id == taskID }) else { return false }
        var task = tasks[index]
        guard let currentNext = task.nextFireMs, currentNext <= scheduledAtMs else {
            return false
        }
        task.lastFiredAtMs = scheduledAtMs
        if task.recurring {
            let expr = try CronExpression(task.cron)
            task.nextFireMs = expr.nextFire(after: scheduledAtMs, calendar: calendar)
        } else {
            task.nextFireMs = nil
        }
        tasks[index] = task
        try writeTasks(tasks)
        return true
    }

    private func readTasks() throws -> [CronTaskRecord] {
        guard FileManager.default.fileExists(atPath: fileURL.path) else { return [] }
        let data = try Data(contentsOf: fileURL)
        let envelope = try JSONDecoder().decode(CronTasksEnvelope.self, from: data)
        return envelope.tasks
    }

    private func writeTasks(_ tasks: [CronTaskRecord]) throws {
        let envelope = CronTasksEnvelope(version: 1, tasks: tasks.sorted(by: taskSort))
        let data = try JSONEncoder().encode(envelope)
        try DefaultProjectAtomicWriter().writeData(data, to: fileURL) { staged in
            _ = try JSONDecoder().decode(CronTasksEnvelope.self, from: staged)
        }
    }

    private func taskSort(lhs: CronTaskRecord, rhs: CronTaskRecord) -> Bool {
        switch (lhs.nextFireMs, rhs.nextFireMs) {
        case let (l?, r?):
            if l == r { return lhs.createdAtMs > rhs.createdAtMs }
            return l < r
        case (.some, .none):
            return true
        case (.none, .some):
            return false
        case (.none, .none):
            if lhs.createdAtMs == rhs.createdAtMs { return lhs.id < rhs.id }
            return lhs.createdAtMs > rhs.createdAtMs
        }
    }
}

actor JsonCronStoreProvider: CronStoreProviding {
    private var stores: [String: JsonCronStoreClient] = [:]
    private let now: @Sendable () -> UInt64
    private let makeID: @Sendable () -> String
    private let calendar: Calendar

    init(
        now: @escaping @Sendable () -> UInt64 = { UInt64(Date().timeIntervalSince1970 * 1000) },
        makeID: @escaping @Sendable () -> String = {
            let suffix = String(UInt64.random(in: 0..<(36 * 36 * 36 * 36 * 36 * 36 * 36 * 36)), radix: 36)
            return "d" + suffix.lowercased().leftPadding(toLength: 8, withPad: "0")
        },
        calendar: Calendar = {
            var calendar = Calendar(identifier: .gregorian)
            calendar.timeZone = .current
            return calendar
        }()
    ) {
        self.now = now
        self.makeID = makeID
        self.calendar = calendar
    }

    func store(for scope: CronScope, appSandboxRoot: String) async throws -> any CronStoreClient {
        if let cached = stores[scope.scopeID] {
            return cached
        }
        let base = URL(fileURLWithPath: appSandboxRoot, isDirectory: true)
        let fileURL = base
            .appendingPathComponent("cron", isDirectory: true)
            .appendingPathComponent(scope.scopeID, isDirectory: true)
            .appendingPathComponent("scheduled_tasks.json", isDirectory: false)
        let client = JsonCronStoreClient(
            fileURL: fileURL,
            now: now,
            makeID: makeID,
            calendar: calendar
        )
        stores[scope.scopeID] = client
        return client
    }
}

private extension String {
    func leftPadding(toLength length: Int, withPad character: Character) -> String {
        guard count < length else { return String(suffix(length)) }
        return String(repeating: String(character), count: length - count) + self
    }
}
