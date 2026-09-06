import Foundation

enum ConversationRenderItem: Identifiable, Equatable {
    case message(Message)
    /// Structured output produced by a local/display slash command. This is a
    /// transcript artifact, not an assistant message, and is therefore never
    /// fed into text-to-speech.
    case commandOutput(ConversationCommandOutput)
    case run(ConversationExecutionRun)
    /// One restored tool call — the `toolUse` block of an assistant turn merged
    /// with the `toolResult` block that answers it from the FOLLOWING user turn.
    /// Live turns render their tools inside the run card instead; this case
    /// exists so a resumed transcript can interleave tool rows between messages
    /// without a `tool_result` ever reaching the right-aligned user bubble.
    case toolCall(ConversationToolTrace)
    /// A standalone transcript notice (e.g. a background task finishing after
    /// its turn already ended, so no run row exists to attach it to).
    case notice(ConversationExecutionNotice)

    var id: String {
        switch self {
        case let .message(message):
            return "message:\(message.id.uuidString)"
        case let .commandOutput(output):
            return "command-output:\(output.id)"
        case let .run(run):
            return "run:\(run.id)"
        case let .toolCall(trace):
            return "tool:\(trace.id)"
        case let .notice(notice):
            return "notice:\(notice.id)"
        }
    }
}

/// Splits transient asynchronous agent execution from durable transcript rows.
/// Runs are activity projections rather than transcript rows. `pinnedRun` is
/// retained for older surfaces, while new surfaces should consume timeline
/// groups.
enum ConversationRenderLayout {
    private static func hasLiveWork(_ run: ConversationExecutionRun) -> Bool {
        run.status == .running || run.activeWorkers > 0
    }

    static func transcriptItems(_ items: [ConversationRenderItem]) -> [ConversationRenderItem] {
        items.filter { item in
            if case .run = item { return false }
            return true
        }
    }

    static func pinnedRun(_ items: [ConversationRenderItem]) -> ConversationExecutionRun? {
        for item in items.reversed() {
            if case let .run(run) = item, hasLiveWork(run) { return run }
        }
        return nil
    }

    /// Project execution activity into stable, compact timeline groups. New
    /// runs use their wire-order activity ledger; older runs fall back to the
    /// legacy reasoning/tools/notices buckets.
    static func timelineGroups(_ items: [ConversationRenderItem]) -> [ConversationTimelineGroup] {
        var groups: [ConversationTimelineGroup] = []
        var consumedBoundaryMessages: Set<UUID> = []
        let messagesByID: [UUID: Message] = Dictionary(uniqueKeysWithValues: items.compactMap { item in
            guard case let .message(message) = item else { return nil }
            return (message.id, message)
        })
        let boundaryMessageIDs: Set<UUID> = Set(items.flatMap { item -> [UUID] in
            guard case let .run(run) = item else { return [] }
            return run.activities.compactMap { activity in
                guard case let .textBoundary(_, messageID) = activity else { return nil }
                return messageID
            }
        })

        func appendGroup(id: String, runID: String?, status: ConversationExecutionStatus? = nil, rows: [ConversationTimelineRow]) {
            guard !rows.isEmpty else { return }
            groups.append(ConversationTimelineGroup(id: id, runID: runID, rows: rows, status: status))
        }

        for item in items {
            switch item {
            case let .message(message):
                guard !boundaryMessageIDs.contains(message.id),
                      !consumedBoundaryMessages.contains(message.id)
                else { continue }
                appendGroup(id: item.id, runID: nil, rows: [.message(message)])
            case let .commandOutput(output):
                appendGroup(id: item.id, runID: nil, rows: [.commandOutput(output)])
            case let .notice(notice):
                appendGroup(id: item.id, runID: nil, rows: [.notice(runID: nil, notice: notice)])
            case let .toolCall(trace):
                appendGroup(id: item.id, runID: nil, rows: [.tool(runID: nil, trace: trace)])
            case let .run(run):
                var pendingTools: [ConversationToolTrace] = []
                func flushTools() {
                    guard !pendingTools.isEmpty else { return }
                    appendGroup(
                        id: "run:\(run.id):tools:\(pendingTools[0].id)",
                        runID: run.id,
                        status: run.status,
                        rows: pendingTools.map { .tool(runID: run.id, trace: $0) }
                    )
                    pendingTools.removeAll(keepingCapacity: true)
                }
                func appendReasoning(_ id: String, _ text: String) {
                    flushTools()
                    guard !text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else { return }
                    appendGroup(id: id, runID: run.id, status: run.status, rows: [.reasoning(runID: run.id, activityID: id, text: text)])
                }

                let activities: [ConversationExecutionActivity]
                if run.activities.isEmpty {
                    var legacy: [ConversationExecutionActivity] = []
                    if !run.reasoning.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
                        legacy.append(.reasoning(id: "run:\(run.id):reasoning", text: run.reasoning))
                    }
                    legacy.append(contentsOf: run.tools.map { .tool(id: $0.id) })
                    legacy.append(contentsOf: run.notices.map { .notice(id: $0.id) })
                    activities = legacy
                } else {
                    activities = run.activities
                }
                for activity in activities {
                    switch activity {
                    case let .reasoning(id, text): appendReasoning(id, text)
                    case let .tool(id):
                        if let trace = run.tools.first(where: { $0.id == id }) { pendingTools.append(trace) }
                    case let .notice(id):
                        flushTools()
                        if let notice = run.notices.first(where: { $0.id == id }) {
                            appendGroup(id: "run:\(run.id):notice:\(id)", runID: run.id, status: run.status, rows: [.notice(runID: run.id, notice: notice)])
                        }
                    case let .textBoundary(_, messageID):
                        flushTools()
                        if let boundaryMessage = messagesByID[messageID],
                           !consumedBoundaryMessages.contains(messageID) {
                            appendGroup(
                                id: "message:\(boundaryMessage.id.uuidString)",
                                runID: nil,
                                rows: [.message(boundaryMessage)]
                            )
                            consumedBoundaryMessages.insert(boundaryMessage.id)
                        }
                    }
                }
                flushTools()
            }
        }
        return groups
    }

    static func timelineRows(_ items: [ConversationRenderItem]) -> [ConversationTimelineRow] {
        timelineGroups(items).flatMap(\.rows)
    }

    static func sheetQuestion(
        _ questions: [ConversationPendingQuestion]
    ) -> ConversationPendingQuestion? {
        questions.first
    }
}

enum ConversationTimelineRow: Identifiable, Equatable {
    case message(Message)
    case commandOutput(ConversationCommandOutput)
    case reasoning(runID: String, activityID: String, text: String)
    case tool(runID: String?, trace: ConversationToolTrace)
    case notice(runID: String?, notice: ConversationExecutionNotice)

    var id: String {
        switch self {
        case let .message(message): return "message:\(message.id.uuidString)"
        case let .commandOutput(output): return "command-output:\(output.id)"
        case let .reasoning(runID, activityID, _): return "run:\(runID):reasoning:\(activityID)"
        case let .tool(runID, trace): return "\(runID.map { "run:\($0):" } ?? "")tool:\(trace.id)"
        case let .notice(runID, notice): return "\(runID.map { "run:\($0):" } ?? "")notice:\(notice.id)"
        }
    }
}

struct ConversationTimelineGroup: Identifiable, Equatable {
    let id: String
    let runID: String?
    let status: ConversationExecutionStatus?
    let rows: [ConversationTimelineRow]

    init(id: String, runID: String?, rows: [ConversationTimelineRow], status: ConversationExecutionStatus? = nil) {
        self.id = id
        self.runID = runID
        self.status = status
        self.rows = rows
    }

    var isToolGroup: Bool {
        !rows.isEmpty && rows.allSatisfy {
            if case .tool = $0 { return true }
            return false
        }
    }
}

// MARK: - Engine-derived tool presentation
//
// The engine derives — ONCE, in Rust — how every tool call should be presented
// and ships it on additive wire fields (`ToolHeaderDto` / `ToolResultDisplayDto`
// / `PlanTaskDto`). The types below mirror those DTOs WITHOUT importing the
// generated bindings, exactly like `ConversationAskOption` above, so the views
// and previews still compile in a checkout that has not built the xcframework.
// The lowering lives in `EngineConversationSource` behind the FFI guard.
//
// Clients must NOT re-parse `input_json` / `result_json` to rebuild any of this:
// four independent re-derivations drifting apart is the bug this deletes.

/// Stable verb identity for a tool header. Mirrors `ToolVerbDto`; localizing
/// clients key their strings off this instead of the English `label`.
enum ConversationToolVerb: Equatable, Hashable {
    case update, create, read, search, shell, output, kill, fetch, task, todo, skill, generic
}

/// Semantic icon identity for a tool header. This model-level mirror avoids a
/// dependency on SwiftUI's `LXIconName` while preserving the engine verb and
/// the legacy raw-tool fallback in one place.
enum ConversationToolIcon: Equatable, Hashable {
    /// Mirrors the Rust `ToolIconDto` variants one-for-one.
    case read, search, list, edit, terminal, globe, workflow, listChecks, sparkles, plug, output, stop, wrench

    static func resolve(verb: ConversationToolVerb?, tool: String) -> Self {
        if let verb {
            switch verb {
            case .update, .create: return .edit
            case .read: return .read
            case .search: return .search
            case .shell: return .terminal
            case .output: return .output
            case .kill: return .stop
            case .fetch: return .globe
            case .task: return .workflow
            case .skill: return .sparkles
            case .todo: return .listChecks
            case .generic: break
            }
        }
        let normalized = tool.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
        if normalized.contains("shell") || normalized.contains("bash") || normalized.contains("terminal") { return .terminal }
        if normalized.contains("fetch") || normalized.contains("web") || normalized.contains("url") || normalized.contains("browser") { return .globe }
        if normalized.contains("read") || normalized.contains("file") || normalized.contains("grep") { return .read }
        if normalized.contains("search") || normalized.contains("documentation") || normalized.contains("docs") { return .search }
        if normalized.contains("write") || normalized.contains("create") || normalized.contains("edit") || normalized.contains("update") { return .edit }
        if normalized.contains("task") || normalized.contains("agent") || normalized.contains("workflow") { return .workflow }
        if normalized.contains("skill") { return .sparkles }
        if normalized.contains("todo") || normalized.contains("plan") { return .listChecks }
        if normalized.contains("kill") || normalized.contains("stop") || normalized.contains("cancel") { return .stop }
        if normalized.contains("plugin") || normalized.contains("mcp") { return .plug }
        if normalized.contains("output") { return .output }
        return .wrench
    }
}

/// A header sub-line with its own glyph, e.g. `("$", "cargo test --all")`.
/// Mirrors `ToolSubLineDto`. `text` is already collapsed to a single line.
struct ConversationToolSubLine: Equatable, Hashable {
    let prefix: String
    let text: String
}

/// The parameterized tool-call header — `Update(src/host.rs)`. Mirrors
/// `ToolHeaderDto`.
struct ConversationToolHeader: Equatable, Hashable {
    let verb: ConversationToolVerb
    /// Optional semantic icon from the current engine. `nil` is expected from
    /// older engines and falls back to `verb`/raw tool name via `icon(for:)`.
    let icon: ConversationToolIcon?
    /// English label. Localizing clients compose from `verb` instead — EXCEPT
    /// when the engine set an override the verb cannot express (a subagent
    /// type, `REPL`, `Web Search`, an MCP tool name), which is why the raw
    /// label is retained here.
    let label: String
    let primary: String?
    /// Suffix rendered after the parentheses; carries its own leading space.
    let qualifier: String?
    let count: UInt32?
    let subLine: ConversationToolSubLine?
    /// Pre-composed English `label(primary)qualifier`, for non-localizing surfaces.
    let title: String

    init(
        verb: ConversationToolVerb,
        icon: ConversationToolIcon? = nil,
        label: String,
        primary: String?,
        qualifier: String?,
        count: UInt32?,
        subLine: ConversationToolSubLine?,
        title: String
    ) {
        self.verb = verb
        self.icon = icon
        self.label = label
        self.primary = primary
        self.qualifier = qualifier
        self.count = count
        self.subLine = subLine
        self.title = title
    }

    func icon(for tool: String) -> ConversationToolIcon {
        icon ?? ConversationToolIcon.resolve(verb: verb, tool: tool.isEmpty ? label : tool)
    }
}

/// Syntax class of one diff segment. Mirrors `SyntaxClassDto`; `op` is the
/// mirror spelling of the DTO's backticked `` `operator` `` case.
enum ConversationSyntaxClass: Equatable, Hashable {
    case plain, keyword, typeName, function, stringLit, number
    case comment, punctuation, op, variable, constant, attribute
}

/// One PRE-SPLIT run of a diff row's text. Mirrors `CodeSegmentDto`.
///
/// Concatenating a row's `segments[].text` reproduces the line EXACTLY — never
/// index into the string. Rust indexes by UTF-8 byte, Swift by grapheme; that
/// mismatch is precisely why no offsets cross the wire.
struct ConversationCodeSegment: Equatable, Hashable {
    let text: String
    /// The syntax class — the ONLY correct source of foreground color.
    let syntax: ConversationSyntaxClass
    /// Terminal-resolved foreground packed `0x00RRGGBB`, baked against ONE dark
    /// theme. Usable only as a dark-mode fallback for `plain`.
    let rgb: UInt32?
    let bold: Bool
    let italic: Bool
    let underline: Bool
    /// A changed word of a word-diffed pair — gets the stronger intra-line
    /// emphasis background.
    let emph: Bool
}

/// Whether a diff row was added, removed, or is unchanged context.
enum ConversationDiffLineKind: Equatable, Hashable {
    case add, remove, context
}

/// One diff row: gutter metadata plus its content runs. Mirrors `DiffRowDto`.
struct ConversationDiffRow: Equatable, Hashable {
    let kind: ConversationDiffLineKind
    /// New-file line number for add/context; old-file for remove.
    let lineNo: UInt32
    /// 0-based hunk index. A CHANGE between consecutive rows is where the `⋯`
    /// separator belongs — there is no separator row kind on the wire.
    let hunk: UInt32
    let wordDiffed: Bool
    let segments: [ConversationCodeSegment]
}

/// A complete structured diff. Mirrors `StructuredDiffDto`.
struct ConversationStructuredDiff: Equatable, Hashable {
    let filePath: String?
    let language: String?
    /// Width of the right-aligned line-number gutter, across ALL hunks.
    let gutterWidth: UInt32
    let additions: UInt32
    let removals: UInt32
    /// Rows dropped by the wire cap; `0` when complete.
    let truncatedRows: UInt32
    let rows: [ConversationDiffRow]
}

/// What a result headline says, for clients that localize. Mirrors
/// `HeadlineKindDto`; the numeric slots arrive in `headlineArgs`.
enum ConversationResultHeadlineKind: Equatable, Hashable {
    case added, removed, addedRemoved
    case linesRead, linesReadPartial
    case filesFound, filesFoundTruncated, linesFound, matchesFound
    case interrupted, noContent, failed, plain
}

/// The pre-derived `⎿` block for one tool result. Mirrors `ToolResultDisplayDto`.
struct ConversationToolResultDisplay: Equatable, Hashable {
    /// English headline. Absent when there is nothing to say.
    let headline: String?
    let headlineKind: ConversationResultHeadlineKind?
    let headlineArgs: [UInt32]
    let diff: ConversationStructuredDiff?
    /// Plain-text body for the expanded view, already clamped to the wire caps.
    let body: String?
    /// Line count BEFORE clamping — drives "show N more lines" with no measuring.
    let bodyLines: UInt32
    let bodyTruncated: Bool
    /// The body exceeds the inline budget — render it collapsed.
    let collapsed: Bool
}

/// Lifecycle state of one plan item. Mirrors `PlanTaskStateDto`.
enum ConversationPlanTaskState: Equatable, Hashable {
    case pending, inProgress, completed

    /// The checklist glyph, matching the terminal exactly.
    var glyph: String {
        switch self {
        case .pending: return "◻"
        case .inProgress: return "◼"
        case .completed: return "✔"
        }
    }

    var label: String {
        switch self {
        case .pending: return String(localized: "chat_plan_state_pending")
        case .inProgress: return String(localized: "chat_plan_state_in_progress")
        case .completed: return String(localized: "chat_plan_state_completed")
        }
    }
}

/// One item of the model-managed working plan. Mirrors `PlanTaskDto`.
///
/// `taskId` is the stable V2 id; TodoWrite V1 items have none, so the render
/// identity falls back to the position-independent subject.
struct ConversationPlanTask: Identifiable, Equatable, Hashable {
    let taskId: String?
    let subject: String
    /// Present-continuous label, for the status line — not the list row.
    let activeForm: String?
    let state: ConversationPlanTaskState

    var id: String { taskId ?? subject }
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
/// bare id until the row backfills). A failed row additionally carries the
/// engine's `error` reason.
struct BackgroundTaskSnapshot: Identifiable, Equatable, Hashable {
    enum Status: Equatable, Hashable {
        case pending
        case running
        case paused
        case completed
        case failed
        case cancelled

        var isTerminal: Bool {
            switch self {
            case .pending, .running, .paused: return false
            case .completed, .failed, .cancelled: return true
            }
        }

        /// A paused workflow is durable checkpoint state awaiting an explicit
        /// resume, not work that can make forward progress under an iOS
        /// background assertion.
        var requiresExecutionLease: Bool {
            switch self {
            case .pending, .running: return true
            case .paused, .completed, .failed, .cancelled: return false
            }
        }
    }

    /// 9-char engine task id.
    let id: String
    /// Human-readable description; empty until a `TaskRow` supplies it.
    var descriptionText: String
    var status: Status
    /// Explicit resume affordance supplied by the engine for adopted paused
    /// workflows. This is a hint only; the engine remains authoritative.
    var canResume: Bool = false
    var startedAtMs: UInt64? = nil
    /// Terminal failure reason reported by the engine (`TaskRowDto.error` /
    /// `TaskStatusChanged.error`). Only a `.failed` row carries one; the panel
    /// renders it under the description so a failed task explains itself even
    /// when the transient conversation notice was never shown (a status push
    /// for a task whose origin session is no longer the visible one is dropped
    /// by design — the row list is how the user still learns why).
    var errorText: String? = nil
    /// Structured Claude-style workflow progress for this task when the
    /// engine exposes it as connection-scoped events.
    var workflow: ConversationWorkflowRunSnapshot? = nil
}

enum WorkflowResumeState: Equatable {
    case idle
    case resuming(taskID: String)
    case succeeded(taskID: String)
    case failed(taskID: String, message: String)
}

enum ConversationWorkflowProgressKind: String, Equatable, Hashable {
    case workflowPhase = "workflow_phase"
    case workflowLog = "workflow_log"
    case workflowAgent = "workflow_agent"
}

enum ConversationWorkflowAgentState: String, Equatable, Hashable {
    case start
    case progress
    case done
    case error
    case cached

    var isTerminal: Bool {
        switch self {
        case .done, .error, .cached:
            return true
        case .start, .progress:
            return false
        }
    }

    var isSuccessLike: Bool {
        switch self {
        case .done, .cached:
            return true
        case .start, .progress, .error:
            return false
        }
    }

    var label: String {
        switch self {
        case .start: return String(localized: "cron_status_queued")
        case .progress: return String(localized: "chat_status_running")
        case .done: return String(localized: "common_done")
        case .error: return String(localized: "cron_error_section")
        case .cached: return String(localized: "chat_workflow_agent_cached")
        }
    }
}

struct ConversationWorkflowProgressPayload: Equatable, Hashable {
    var kind: ConversationWorkflowProgressKind
    var index: UInt64?
    var title: String?
    var message: String?
    var label: String?
    var phaseIndex: UInt32?
    var phaseTitle: String?
    var agentId: String?
    var agentType: String?
    var model: String?
    var fallbackModel: String?
    var state: ConversationWorkflowAgentState?
    var error: String?
    var toolUseId: String?
    var startedAtMs: UInt64?
    var queuedAtMs: UInt64?
    var lastProgressAtMs: UInt64?
    var attempt: UInt32?
    var lastAttemptReason: String?
    var tokens: UInt64?
    var toolCalls: UInt64?
    var lastToolName: String?
    var lastToolSummary: String?
    var promptPreview: String?
}

struct ConversationWorkflowPhaseSnapshot: Identifiable, Equatable, Hashable {
    let id: String
    var index: UInt32?
    var title: String
    var label: String?
    var message: String?
    var updatedAtMs: UInt64?
}

struct ConversationWorkflowLogSnapshot: Identifiable, Equatable, Hashable {
    let id: String
    var phaseIndex: UInt32?
    var phaseTitle: String?
    var label: String?
    var message: String?
    var updatedAtMs: UInt64?
}

struct ConversationWorkflowAgentSnapshot: Identifiable, Equatable, Hashable {
    let id: String
    var index: UInt64
    var title: String?
    var message: String?
    var label: String?
    var phaseIndex: UInt32?
    var phaseTitle: String?
    var agentId: String?
    var agentType: String?
    var model: String?
    var fallbackModel: String?
    var state: ConversationWorkflowAgentState
    var error: String?
    var toolUseId: String?
    var startedAtMs: UInt64?
    var queuedAtMs: UInt64?
    var lastProgressAtMs: UInt64?
    var attempt: UInt32?
    var lastAttemptReason: String?
    var tokens: UInt64?
    var toolCalls: UInt64?
    var lastToolName: String?
    var lastToolSummary: String?
    var promptPreview: String?

    var displayTitle: String {
        if let title, !title.isEmpty { return title }
        if let agentType, !agentType.isEmpty { return agentType }
        if let agentId, !agentId.isEmpty { return agentId }
        return String(localized: "chat_workflow_agent_fallback \(index + 1)")
    }

    var activityLine: String? {
        if let error, !error.isEmpty { return error }
        if let message, !message.isEmpty { return message }
        if let lastToolSummary, !lastToolSummary.isEmpty { return lastToolSummary }
        if let lastToolName, !lastToolName.isEmpty { return lastToolName }
        if let label, !label.isEmpty { return label }
        return nil
    }

    func durationMs(nowMs: UInt64) -> UInt64? {
        let start = startedAtMs ?? queuedAtMs
        guard let start else { return nil }
        let end = state.isTerminal ? (lastProgressAtMs ?? nowMs) : nowMs
        guard end >= start else { return nil }
        return end - start
    }
}

struct ConversationWorkflowRunSnapshot: Equatable, Hashable {
    let taskId: String
    var runId: String
    var phases: [ConversationWorkflowPhaseSnapshot] = []
    var logs: [ConversationWorkflowLogSnapshot] = []
    var agents: [ConversationWorkflowAgentSnapshot] = []
    var lastUpdatedAtMs: UInt64?

    var totalAgents: Int { agents.count }
    var queuedAgents: Int { agents.filter { $0.state == .start }.count }
    var runningAgents: Int { agents.filter { $0.state == .progress }.count }
    var succeededAgents: Int { agents.filter { $0.state.isSuccessLike }.count }
    var failedAgents: Int { agents.filter { $0.state == .error }.count }
    /// Compatibility spelling retained for callers that render a compact
    /// success fraction. Errors are deliberately excluded.
    var doneAgents: Int { succeededAgents }

    var currentPhaseTitle: String? {
        if let running = agents
            .filter({ !$0.state.isTerminal })
            .sorted(by: Self.agentSort)
            .first,
           let phaseTitle = running.phaseTitle ?? phase(for: running.phaseIndex)?.title,
           !phaseTitle.isEmpty {
            return phaseTitle
        }
        return sortedPhases.last?.title
    }

    var sortedPhases: [ConversationWorkflowPhaseSnapshot] {
        phases.sorted { lhs, rhs in
            switch (lhs.index, rhs.index) {
            case let (left?, right?) where left != right:
                return left < right
            case (.some, .none):
                return true
            case (.none, .some):
                return false
            default:
                return lhs.title.localizedCaseInsensitiveCompare(rhs.title) == .orderedAscending
            }
        }
    }

    var sortedAgents: [ConversationWorkflowAgentSnapshot] {
        agents.sorted(by: Self.agentSort)
    }

    func phase(for index: UInt32?) -> ConversationWorkflowPhaseSnapshot? {
        guard let index else { return nil }
        return phases.first { $0.index == index }
    }

    static func agentSort(
        _ lhs: ConversationWorkflowAgentSnapshot,
        _ rhs: ConversationWorkflowAgentSnapshot
    ) -> Bool {
        switch (lhs.phaseIndex, rhs.phaseIndex) {
        case let (left?, right?) where left != right:
            return left < right
        case (.some, .none):
            return true
        case (.none, .some):
            return false
        default:
            return lhs.index < rhs.index
        }
    }
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

enum ConversationToolExpansionKey {
    private static let structuredPrefix = "structured:"

    static func structured(messageID: UUID, toolID: String) -> String {
        "\(structuredPrefix)\(messageID.uuidString):\(toolID)"
    }

    static func structuredToolIDs(in keys: Set<String>, messageID: UUID) -> Set<String> {
        let prefix = "\(structuredPrefix)\(messageID.uuidString):"
        return Set(keys.compactMap { key in
            guard key.hasPrefix(prefix) else { return nil }
            return String(key.dropFirst(prefix.count))
        })
    }
}

enum ConversationLLMActivityState: Equatable {
    case hidden
    case running
    case stopping
    case paused

    static func resolve(
        hasConversation: Bool,
        streaming: Bool,
        isCancelling: Bool
    ) -> Self {
        if isCancelling { return .stopping }
        if streaming { return .running }
        guard hasConversation else { return .hidden }
        return .paused
    }
}

enum ConversationMessageBlock: Equatable {
    case text(String)
    case thinking(text: String, signature: String?)
    case redactedThinking
    case compactBoundary(messagesBefore: Int, messagesAfter: Int, summary: String)
    case toolUse(
        id: String,
        tool: String,
        inputSummary: String,
        inputJson: String,
        /// Engine-derived header; `nil` on an older engine, where `inputSummary`
        /// (the legacy client-side summarizer) is the fallback.
        header: ConversationToolHeader?
    )
    case toolResult(
        id: String,
        tool: String,
        isError: Bool,
        summary: String,
        resultJson: String,
        oldString: String?,
        newString: String?,
        filePath: String?,
        /// Engine-derived `⎿` block; `nil` on an older engine. Supersedes the
        /// three legacy diff fields above, which carry only the raw pair.
        display: ConversationToolResultDisplay?
    )
}

enum ConversationExecutionStatus: String, Equatable {
    case running
    case completed
    case failed
    case cancelled
    case maxTurns
    /// A terminal row rebuilt from SessionResumed history whose exact live
    /// outcome was not persisted by MessageDto.
    case restored

    var label: String {
        switch self {
        case .running: return String(localized: "chat_status_running")
        case .completed: return String(localized: "chat_status_completed")
        case .failed: return String(localized: "chat_status_failed")
        case .cancelled: return String(localized: "chat_status_cancelled")
        case .maxTurns: return String(localized: "chat_status_max_turns")
        case .restored: return String(localized: "chat_status_restored")
        }
    }

    var tone: ConversationExecutionTone {
        switch self {
        case .running: return .running
        case .completed: return .completed
        case .failed: return .failed
        case .cancelled: return .cancelled
        case .maxTurns: return .maxTurns
        case .restored: return .restored
        }
    }
}

/// Semantic visual role for one run status. Keeping this separate from raw
/// colors makes the one-tone-per-status contract unit-testable, including the
/// neutral historical state whose exact outcome was not persisted.
enum ConversationExecutionTone: Hashable {
    case running
    case completed
    case failed
    case cancelled
    case maxTurns
    case restored
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
    /// LEGACY fallback, produced by `summarizeToolInput` when the engine ships
    /// no `header`. Never used to rebuild a header when one is present.
    var inputSummary: String?
    /// LEGACY fallback, produced by `summarizeToolResult` when the engine ships
    /// no `display`.
    var outputSummary: String?
    var elapsedMs: UInt64?
    /// The engine-derived header. Present on a current engine.
    var header: ConversationToolHeader? = nil
    /// The engine-derived `⎿` result block. Present once the tool returns on a
    /// current engine.
    var display: ConversationToolResultDisplay? = nil
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

enum ConversationCompactionStatus: Equatable {
    case queued
    case running(phase: String, startedAt: Date, phaseStartedAt: Date, unknownPhase: Bool = false)
    case completed(messagesBefore: UInt32?, messagesAfter: UInt32?, bytesSaved: UInt64?)
    case skipped
    case failed(detail: String)

    var isActive: Bool {
        switch self {
        case .queued, .running: true
        case .completed, .skipped, .failed: false
        }
    }

    static func reducing(
        _ previous: Self?, phase: String, error: String?, now: Date = Date()
    ) -> Self? {
        switch phase {
        case "preparing", "summarizing", "restoring":
            let phases = ["preparing", "summarizing", "restoring"]
            if case let .running(current, startedAt, phaseStartedAt, _) = previous {
                if (phases.firstIndex(of: phase) ?? -1) <= (phases.firstIndex(of: current) ?? -1) {
                    return .running(phase: current, startedAt: startedAt, phaseStartedAt: phaseStartedAt)
                }
                return .running(phase: phase, startedAt: startedAt, phaseStartedAt: now)
            }
            return .running(phase: phase, startedAt: now, phaseStartedAt: now)
        case "complete":
            if case .skipped = previous { return previous }
            if case .completed = previous { return previous }
            return .completed(messagesBefore: nil, messagesAfter: nil, bytesSaved: nil)
        case "skipped":
            return .skipped
        case "error":
            return .failed(detail: error ?? "")
        case "cancelled":
            return nil
        default:
            if case let .running(current, startedAt, phaseStartedAt, _) = previous {
                return .running(phase: current, startedAt: startedAt, phaseStartedAt: phaseStartedAt, unknownPhase: true)
            }
            return previous ?? .running(phase: phase, startedAt: now, phaseStartedAt: now, unknownPhase: true)
        }
    }
}

enum ConversationCompactionProgress {
    /// Phase-bounded estimate; only engine-confirmed completion reaches 100%.
    static func percent(phase: String, elapsed: TimeInterval) -> Int? {
        if phase == "complete" { return 100 }
        let base: Double, span: Double, tau: Double, cap: Int
        switch phase {
        case "preparing": (base, span, tau, cap) = (0, 10, 5, 9)
        case "summarizing": (base, span, tau, cap) = (10, 75, 90, 84)
        case "restoring": (base, span, tau, cap) = (85, 14, 10, 99)
        default: return nil
        }
        return min(cap, Int(base) + Int((span * (1 - exp(-max(0, elapsed) / tau))).rounded()))
    }
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
    /// Wire-order activity ledger for compact timeline rendering. Older runs
    /// may omit it; projections then fall back to the legacy buckets above.
    var activities: [ConversationExecutionActivity] = []
}

enum ConversationExecutionActivity: Equatable, Identifiable {
    case reasoning(id: String, text: String)
    case tool(id: String)
    case notice(id: String)
    /// A narrative assistant text boundary. It binds the wire-order activity
    /// to its concrete message row so restored turns can interleave multiple
    /// narrative segments and tools without guessing from item adjacency.
    case textBoundary(id: String, messageID: UUID)

    var id: String {
        switch self {
        case let .reasoning(id, _), let .tool(id), let .notice(id), let .textBoundary(id, _): return id
        }
    }
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
