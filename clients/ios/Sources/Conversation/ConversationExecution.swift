import Foundation

enum ConversationRenderItem: Identifiable, Equatable {
    case message(Message)
    case run(ConversationExecutionRun)
    /// An interactive `AskUserQuestion` questionnaire waiting for the user,
    /// appended after the messages while pending.
    case question(ConversationPendingQuestion)
    /// A standalone transcript notice (e.g. a background task finishing after
    /// its turn already ended, so no run row exists to attach it to).
    case notice(ConversationExecutionNotice)

    var id: String {
        switch self {
        case let .message(message):
            return "message:\(message.id.uuidString)"
        case let .run(run):
            return "run:\(run.id)"
        case let .question(question):
            return "question:\(question.requestId)"
        case let .notice(notice):
            return "notice:\(notice.id)"
        }
    }
}

// MARK: - AskUserQuestion (FFI-independent mirror of the wire payload)

/// One selectable answer of an interactive question — mirrors `AskOptionDto`
/// without requiring the generated bindings, so previews/mock builds compile.
struct ConversationAskOption: Equatable, Hashable {
    let label: String
    let description: String
    let preview: String?
}

/// One question of an interactive questionnaire — mirrors `AskQuestionDto`.
/// `question` is the complete question text AND the answer-map key.
struct ConversationAskQuestion: Equatable, Hashable {
    let question: String
    let header: String
    let options: [ConversationAskOption]
    let multiSelect: Bool
}

/// One background task announced by the engine — a Workflow build segment,
/// a background bash job, … — the row the pinned tasks panel renders. Fed by
/// `TaskRow` (TaskList replies, which carry the description) and
/// `TaskStatusChanged` pushes (which may arrive first — the panel shows the
/// bare id until the row backfills).
struct BackgroundTaskSnapshot: Identifiable, Equatable, Hashable {
    enum Status: Equatable, Hashable {
        case pending
        case running
        case completed
        case failed
        case cancelled

        var isTerminal: Bool {
            switch self {
            case .pending, .running: return false
            case .completed, .failed, .cancelled: return true
            }
        }
    }

    /// 9-char engine task id.
    let id: String
    /// Human-readable description; empty until a `TaskRow` supplies it.
    var descriptionText: String
    var status: Status
}

/// One pending interactive `AskUserQuestion` request — mirrors
/// `AskUserQuestionRequestDto`. `requestId` is the connection-scoped
/// correlator echoed by the answer/cancel command.
struct ConversationPendingQuestion: Identifiable, Equatable, Hashable {
    let requestId: UInt64
    let questions: [ConversationAskQuestion]
    /// Idle auto-continue window in seconds; `nil` means wait indefinitely.
    let timeoutSecs: UInt64?

    var id: UInt64 { requestId }
}

struct ConversationMessageDetail: Equatable {
    let blocks: [ConversationMessageBlock]
}

enum ConversationMessageBlock: Equatable {
    case text(String)
    case thinking(text: String, signature: String?)
    case redactedThinking
    case compactBoundary(messagesBefore: Int, messagesAfter: Int, summary: String)
    case toolUse(id: String, tool: String, inputSummary: String, inputJson: String)
    case toolResult(
        id: String,
        tool: String,
        isError: Bool,
        summary: String,
        resultJson: String,
        oldString: String?,
        newString: String?,
        filePath: String?
    )
}

enum ConversationExecutionStatus: String, Equatable {
    case running
    case completed
    case failed
    case cancelled
    case maxTurns

    var label: String {
        switch self {
        case .running: return String(localized: "chat_status_running")
        case .completed: return String(localized: "chat_status_completed")
        case .failed: return String(localized: "chat_status_failed")
        case .cancelled: return String(localized: "chat_status_cancelled")
        case .maxTurns: return String(localized: "chat_status_max_turns")
        }
    }
}

enum ConversationToolStatus: Equatable {
    case running
    case completed
    case failed
    case cancelled

    var label: String {
        switch self {
        case .running: return String(localized: "chat_status_running")
        case .completed: return String(localized: "chat_status_completed")
        case .failed: return String(localized: "chat_status_failed")
        case .cancelled: return String(localized: "chat_status_cancelled")
        }
    }
}

struct ConversationToolTrace: Identifiable, Equatable {
    let id: String
    var tool: String
    var status: ConversationToolStatus
    var inputSummary: String?
    var outputSummary: String?
    var elapsedMs: UInt64?
}

enum ConversationShellStatus: Equatable {
    case running
    case completed
    case failed
    case timedOut
    case cancelled

    var label: String {
        switch self {
        case .running: return String(localized: "chat_status_running")
        case .completed: return String(localized: "chat_status_completed")
        case .failed: return String(localized: "chat_status_failed")
        case .timedOut: return String(localized: "chat_status_timed_out")
        case .cancelled: return String(localized: "chat_status_cancelled")
        }
    }

    var asToolStatus: ConversationToolStatus {
        switch self {
        case .running: return .running
        case .completed: return .completed
        case .failed, .timedOut: return .failed
        case .cancelled: return .cancelled
        }
    }
}

struct ConversationShellCard: Identifiable, Equatable {
    let sessionId: String
    let turnId: UInt64?
    let taskId: String
    var command: String
    var cwd: String?
    var stdout: String = ""
    var stderr: String = ""
    var exitCode: Int? = nil
    var durationMs: UInt64? = nil
    var status: ConversationShellStatus = .running
    var truncated: Bool = false

    var id: String { taskId }
    var requestedCwd: TerminalRouteCwd? { TerminalRouteCwd(cwd) }
}

struct ConversationShellLaunchRequest: Equatable {
    let taskId: String
    let command: String
    let cwd: TerminalRouteCwd?
}

struct ConversationUsageSnapshot: Equatable {
    var inputTokens: UInt64
    var outputTokens: UInt64
    var cacheReadTokens: UInt64
    var cacheCreationTokens: UInt64
}

struct ConversationRetrySnapshot: Equatable {
    var message: String
    var attempt: UInt32
    var maxRetries: UInt32
    var delayMs: UInt64
}

struct ConversationCompactionSnapshot: Identifiable, Equatable {
    let id = UUID()
    var messagesBefore: UInt32
    var messagesAfter: UInt32
    var bytesSaved: UInt64
}

struct ConversationCoordinatorWorker: Identifiable, Equatable {
    let id: String
    var name: String
    var agentType: String
    var status: String
}

struct ConversationExecutionRun: Identifiable, Equatable {
    let id: String
    let sessionId: String
    let turnId: UInt64?
    var status: ConversationExecutionStatus
    var reasoning: String = ""
    var notices: [ConversationExecutionNotice] = []
    var tools: [ConversationToolTrace] = []
    var shellCards: [ConversationShellCard] = []
    var usage: ConversationUsageSnapshot? = nil
    var retry: ConversationRetrySnapshot? = nil
    var costFormatted: String? = nil
    var compactions: [ConversationCompactionSnapshot] = []
    var coordinatorTeam: String? = nil
    var activeWorkers: UInt32 = 0
    var workers: [ConversationCoordinatorWorker] = []
}

struct ConversationExecutionNotice: Identifiable, Equatable {
    enum Kind: Equatable {
        case info
        case warning
        case error
    }

    let id: String
    var kind: Kind
    var text: String
}

enum ConversationExecutionParsing {
    static func isShellTool(_ name: String) -> Bool {
        let normalized = name.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
        return normalized == "shell" || normalized == "bash" || normalized == "terminal"
    }

    static func summarizeToolInput(_ inputJson: String) -> String? {
        guard let object = jsonObject(from: inputJson) else {
            return inputJson.trimmingCharacters(in: .whitespacesAndNewlines).nilIfBlank
        }
        for key in ["command", "cmd", "path", "file_path", "cwd", "query", "pattern", "url"] {
            if let value = stringValue(object[key])?.nilIfBlank {
                return value
            }
        }
        if let first = object.keys.sorted().first,
           let value = stringValue(object[first])?.nilIfBlank {
            return value
        }
        return inputJson.trimmingCharacters(in: .whitespacesAndNewlines).nilIfBlank
    }

    static func summarizeToolResult(_ resultJson: String, isError: Bool, tool: String) -> String {
        if isShellTool(tool) {
            if let finished = shellFinished(id: tool, resultJson: resultJson, isError: isError) {
                return shellStatusLabel(finished.status)
            }
        }
        guard let object = nestedDataObject(from: resultJson) ?? jsonObject(from: resultJson) else {
            return isError ? String(localized: "chat_tool_result_failed") : String(localized: "chat_tool_result_completed")
        }
        if let error = stringValue(object["error"])?.nilIfBlank {
            return error
        }
        for key in ["message", "summary", "result", "output"] {
            if let value = stringValue(object[key])?.nilIfBlank {
                return value
            }
        }
        return isError ? String(localized: "chat_tool_result_failed") : String(localized: "chat_tool_result_completed")
    }

    static func isCancellationResult(_ resultJson: String) -> Bool {
        guard let object = jsonObject(from: resultJson),
              let kind = stringValue(object["tool_denial_kind"])?.lowercased()
        else { return false }
        return kind == "interrupted" || kind == "cancelled"
    }

    static func shellStarted(id: String, inputJson: String) -> ConversationShellCard {
        let object = jsonObject(from: inputJson) ?? [:]
        return ConversationShellCard(
            sessionId: "",
            turnId: nil,
            taskId: id,
            command: stringValue(object["command"])?.nilIfBlank
                ?? stringValue(object["cmd"])?.nilIfBlank
                ?? inputJson,
            cwd: stringValue(object["cwd"])?.nilIfBlank
        )
    }

    static func shellFinished(
        id: String,
        resultJson: String,
        isError: Bool
    ) -> ConversationShellCard? {
        guard let object = nestedDataObject(from: resultJson) ?? jsonObject(from: resultJson) else {
            return nil
        }
        let timedOut = boolValue(object["timed_out"]) == true
        let cancelled = boolValue(object["cancelled"]) == true || boolValue(object["interrupted"]) == true
        let reportedError = boolValue(object["is_error"]) == true
        let exitCode = intValue(object["exit_code"]) ?? intValue(object["code"])
        let status: ConversationShellStatus
        if timedOut {
            status = .timedOut
        } else if cancelled {
            status = .cancelled
        } else if isError || reportedError || (exitCode != nil && exitCode != 0) {
            status = .failed
        } else {
            status = .completed
        }
        return ConversationShellCard(
            sessionId: "",
            turnId: nil,
            taskId: id,
            command: "",
            stdout: stringValue(object["stdout"]) ?? "",
            stderr: (stringValue(object["stderr"]) ?? "").ifBlank(
                stringValue((jsonObject(from: resultJson) ?? [:])["error"]) ?? ""
            ),
            exitCode: exitCode,
            durationMs: uint64Value(object["duration_ms"]) ?? uint64Value(object["elapsed_ms"]),
            status: status,
            truncated: boolValue(object["truncated"]) == true
        )
    }

    static func shellStatusLabel(_ status: ConversationShellStatus) -> String {
        switch status {
        case .running: return String(localized: "chat_shell_running")
        case .completed: return String(localized: "chat_shell_completed")
        case .failed: return String(localized: "chat_shell_failed")
        case .timedOut: return String(localized: "chat_shell_timed_out")
        case .cancelled: return String(localized: "chat_shell_cancelled")
        }
    }

    static func formatDuration(_ ms: UInt64?) -> String? {
        guard let ms else { return nil }
        if ms < 1_000 { return "\(ms)ms" }
        let seconds = Double(ms) / 1_000
        if seconds < 60 {
            return String(format: "%.1fs", seconds)
        }
        let minutes = Int(seconds) / 60
        let remain = Int(seconds) % 60
        return "\(minutes)m \(remain)s"
    }

    private static func nestedDataObject(from json: String) -> [String: Any]? {
        guard let root = jsonObject(from: json) else { return nil }
        return root["data"] as? [String: Any]
    }

    private static func jsonObject(from json: String) -> [String: Any]? {
        guard let data = json.data(using: .utf8),
              let object = try? JSONSerialization.jsonObject(with: data),
              let dict = object as? [String: Any]
        else {
            return nil
        }
        return dict
    }

    private static func stringValue(_ value: Any?) -> String? {
        switch value {
        case let value as String:
            return value
        case let value as NSNumber:
            return value.stringValue
        default:
            return nil
        }
    }

    private static func boolValue(_ value: Any?) -> Bool? {
        switch value {
        case let value as Bool:
            return value
        case let value as NSNumber:
            return value.boolValue
        case let value as String:
            return Bool(value.lowercased())
        default:
            return nil
        }
    }

    private static func intValue(_ value: Any?) -> Int? {
        switch value {
        case let value as Int:
            return value
        case let value as NSNumber:
            return value.intValue
        case let value as String:
            return Int(value)
        default:
            return nil
        }
    }

    private static func uint64Value(_ value: Any?) -> UInt64? {
        switch value {
        case let value as UInt64:
            return value
        case let value as Int:
            return value >= 0 ? UInt64(value) : nil
        case let value as NSNumber:
            return value.int64Value >= 0 ? UInt64(value.int64Value) : nil
        case let value as String:
            return UInt64(value)
        default:
            return nil
        }
    }
}

private extension String {
    var nilIfBlank: String? {
        trimmingCharacters(in: .whitespacesAndNewlines).isEmpty ? nil : self
    }

    func ifBlank(_ fallback: String) -> String {
        trimmingCharacters(in: .whitespacesAndNewlines).isEmpty ? fallback : self
    }
}
