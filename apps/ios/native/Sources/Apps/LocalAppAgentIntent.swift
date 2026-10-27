import AppIntents
import Foundation

private struct LocalAppAgentNativeStart: Decodable, Sendable {
    let runId: String
}

struct LocalAppAgentNativeEvents: Decodable, Sendable {
    let events: [LocalAppJSON]
    let sequence: UInt64
}

enum LocalAppAgentHostRequests {
    /// Native task cancellation accepts the public AG-UI id as requestId.
    /// Native runId is reserved for the host's internal task identity.
    static func cancelPublicRequest(
        appID: String,
        revision: UInt64,
        requestID: String
    ) -> [String: LocalAppJSON] {
        [
            "op": .string("tasks.cancel"),
            "appId": .string(appID),
            "revision": .number(Double(revision)),
            "requestId": .string(requestID)
        ]
    }
}

enum LocalAppAgentIntentInput {
    static let maximumInputBytes = 70 * 1024 * 1024

    static func isWithinInputLimit(_ byteCount: Int) -> Bool {
        byteCount <= maximumInputBytes
    }

    static func runAgentInput(_ text: String) throws -> (value: LocalAppJSON, threadID: String, runID: String) {
        guard isWithinInputLimit(text.utf8.count),
              let value = try? JSONDecoder().decode(LocalAppJSON.self, from: Data(text.utf8)),
              case let .object(fields) = value,
              case let .string(threadID)? = fields["threadId"],
              case let .string(runID)? = fields["runId"] else {
            throw LocalAppServiceFailure("INVALID_AGENT_INPUT", "Provide a RunAgentInput JSON object with threadId and runId fields.")
        }
        return (value, threadID, runID)
    }
}

enum LocalAppAgentRunEvent {
    static func type(_ event: LocalAppJSON) -> String? {
        guard case let .object(fields) = event, case let .string(type)? = fields["type"] else { return nil }
        return type
    }

    static func isTerminal(_ event: LocalAppJSON) -> Bool {
        ["RUN_FINISHED", "RUN_ERROR"].contains(type(event) ?? "")
    }

    static func terminalEvent(in events: [LocalAppJSON]) -> LocalAppJSON? {
        events.first(where: Self.isTerminal)
    }

    static func matches(_ event: LocalAppJSON, runID: String, threadID: String) -> Bool {
        guard case let .object(fields) = event,
              case let .string(eventThreadID)? = fields["threadId"] else { return false }
        return matchesRun(event, runID: runID) && eventThreadID == threadID
    }

    static func matchesRun(_ event: LocalAppJSON, runID: String) -> Bool {
        guard case let .object(fields) = event,
              case let .string(eventRunID)? = fields["runId"] else { return false }
        return eventRunID == runID
    }

    static func isSuccessful(_ event: LocalAppJSON) -> Bool {
        guard type(event) == "RUN_FINISHED", case let .object(fields) = event else { return false }
        guard let outcome = fields["outcome"] else { return true }
        guard case let .object(outcomeFields) = outcome,
              case let .string(type)? = outcomeFields["type"] else { return false }
        return type == "success"
    }

    static func isSuccessfulTerminal(
        _ event: LocalAppJSON?,
        runID: String,
        threadID: String? = nil
    ) -> Bool {
        guard let event, type(event) == "RUN_FINISHED", matchesRun(event, runID: runID) else { return false }
        if let threadID, !matches(event, runID: runID, threadID: threadID) { return false }
        return isSuccessful(event)
    }
}

/// Counts only newly observed AG-UI stream output for one native run. The
/// transport cursor is per event and pages are contiguous; no output is kept.
struct LocalAppAgentOutputProgress: Sendable {
    private(set) var sequence: UInt64 = 0
    private(set) var outputBytes: Int64 = 0

    /// Returns true only when this validated page increases the byte count.
    @discardableResult
    mutating func consume(
        _ batch: LocalAppAgentNativeEvents,
        runID: String,
        threadID: String? = nil
    ) throws -> Bool {
        guard batch.sequence >= sequence else {
            throw LocalAppServiceFailure("INVALID_AGENT_EVENT", "The Local App event sequence moved backwards.")
        }
        // A repeated page, including a non-empty replay, must never count twice.
        guard batch.sequence > sequence else { return false }
        guard UInt64(batch.events.count) == batch.sequence - sequence else {
            throw LocalAppServiceFailure("INVALID_AGENT_EVENT", "The Local App event page does not match its sequence.")
        }

        var nextBytes = outputBytes
        var terminalSeen = false
        for event in batch.events {
            guard !terminalSeen, case let .object(fields) = event,
                  case let .string(type)? = fields["type"], !type.isEmpty else {
                throw LocalAppServiceFailure("INVALID_AGENT_EVENT", "The Local App host returned an invalid AG-UI event.")
            }
            if type == "RUN_STARTED" || type == "RUN_FINISHED" {
                guard case let .string(eventThreadID)? = fields["threadId"],
                      LocalAppAgentRunEvent.matchesRun(event, runID: runID),
                      threadID == nil || eventThreadID == threadID else {
                    throw LocalAppServiceFailure("INVALID_AGENT_EVENT", "The AG-UI event belongs to a different thread or run.")
                }
            } else if type == "RUN_ERROR" {
                guard case .string? = fields["message"] else {
                    throw LocalAppServiceFailure("INVALID_AGENT_EVENT", "The Local App host returned an invalid AG-UI error event.")
                }
            }
            if type == "TEXT_MESSAGE_CONTENT" || type == "TOOL_CALL_ARGS" {
                let idField = type == "TEXT_MESSAGE_CONTENT" ? "messageId" : "toolCallId"
                guard case .string? = fields[idField], case let .string(delta)? = fields["delta"] else {
                    throw LocalAppServiceFailure("INVALID_AGENT_EVENT", "The Local App host returned an invalid AG-UI output event.")
                }
                nextBytes = Self.saturatingAdd(nextBytes, delta.utf8.count)
            }
            terminalSeen = type == "RUN_FINISHED" || type == "RUN_ERROR"
        }
        // Commit only after the entire page has been validated.
        let increased = nextBytes > outputBytes
        sequence = batch.sequence
        outputBytes = nextBytes
        return increased
    }

    static func saturatingAdd(_ current: Int64, _ byteCount: Int) -> Int64 {
        let amount = Int64(clamping: max(0, byteCount))
        let (sum, overflow) = current.addingReportingOverflow(amount)
        return overflow ? Int64.max : sum
    }
}

/// Shares the native Local App broker with browser runs. The intent carries the
/// complete bounded input and never waits for an external browser handoff.
@available(iOS 27.0, *)
struct RunLocalAppAgentIntent: AppIntent, LongRunningIntent, CancellableIntent {
    static let title: LocalizedStringResource = "Run Local App Agent"
    static let description = IntentDescription("Run a selected Local App Agent request from a complete AG-UI RunAgentInput.")
    static var supportedModes: IntentModes { .background }
    static var authenticationPolicy: IntentAuthenticationPolicy { .requiresAuthentication }
    static var parameterSummary: some ParameterSummary {
        Summary("Run Local App Agent for app \(\.$appID)") {
            \.$revision
            \.$inputJSON
        }
    }

    @Parameter(title: "App ID") var appID: String
    @Parameter(title: "Release revision") var revision: Int
    @Parameter(title: "AG-UI RunAgentInput JSON") var inputJSON: String

    func perform() async throws -> some ReturnsValue<String> & ProvidesDialog {
        guard revision > 0, let revisionValue = UInt64(exactly: revision) else {
            throw LocalAppServiceFailure("INVALID_AGENT_INPUT", "Use a positive release revision.")
        }
        guard !appID.isEmpty, appID.utf8.count <= 128,
              !appID.unicodeScalars.contains(where: { $0.value < 32 || $0.value == 127 }) else {
            throw LocalAppServiceFailure("INVALID_AGENT_INPUT", "The Local App ID is invalid.")
        }
        let input = try LocalAppAgentIntentInput.runAgentInput(inputJSON)
        let client = LocalApps.library.client
        let cancellation = LocalAppAgentIntentCancellation(client: client, appID: appID, revision: revisionValue)
        let taskProgress = progress
        LocalAppAgentProgressReporting.update(taskProgress, outputBytes: 0)

        let terminalEvent = try await performBackgroundTask(operation: {
            let started = try await client.agentHost([
                "op": .string("startNative"),
                "appId": .string(self.appID),
                "revision": .number(Double(revisionValue)),
                "input": input.value
            ], as: LocalAppAgentNativeStart.self)
            guard started.runId == input.runID else {
                throw LocalAppServiceFailure("INVALID_AGENT_TASK", "The Local App host changed this AG-UI runId.")
            }
            try await cancellation.didStart(started.runId)
            return try await Self.waitForCompletion(
                client: client,
                cancellation: cancellation,
                appID: self.appID,
                revision: revisionValue,
                runID: started.runId,
                threadID: input.threadID,
                progress: taskProgress
            )
        }, onCancel: { _ in
            Task { await cancellation.cancel(reason: .intentCancellation) }
        })
        let data = try JSONEncoder().encode(terminalEvent)
        let value = String(decoding: data, as: UTF8.self)
        let didSucceed = LocalAppAgentRunEvent.isSuccessfulTerminal(
            terminalEvent,
            runID: input.runID,
            threadID: input.threadID
        )
        return .result(value: value, dialog: IntentDialog(didSucceed ? "The Local App Agent run finished." : "The Local App Agent run ended without success."))
    }

    private static func waitForCompletion(
        client: LocalAppServiceClient,
        cancellation: LocalAppAgentIntentCancellation,
        appID: String,
        revision: UInt64,
        runID: String,
        threadID: String,
        progress: Progress
    ) async throws -> LocalAppJSON {
        var deadline = Date().addingTimeInterval(3_600)
        var outputProgress = LocalAppAgentOutputProgress()
        let contextKey = LocalAppAgentWakeRequest(appID: appID, revision: revision, runID: runID).contextKey
        while Date() < deadline {
            try Task.checkCancellation()
            if await cancellation.isCancelled { throw CancellationError() }
            let response = try await client.agentHost([
                "op": .string("tasks.list"),
                "appId": .string(appID),
                "revision": .number(Double(revision)),
                "requestId": .string(runID)
            ], as: LocalAppAgentTaskListResponse.self)
            guard response.appId == appID, response.revision == revision,
                  let task = response.tasks.first(where: { $0.requestId == runID }) else {
                throw LocalAppServiceFailure("AGENT_TASK_MISSING", "The Local App host did not return the admitted task.")
            }
            if let observed = task.progress, observed.turnLimit > 0 {
                LocalAppAgentProgressReporting.update(
                    progress,
                    outputBytes: outputProgress.outputBytes,
                    completedModelTurns: observed.completedModelTurns,
                    turnLimit: observed.turnLimit
                )
            } else {
                LocalAppAgentProgressReporting.update(
                    progress,
                    outputBytes: outputProgress.outputBytes,
                    description: task.phase ?? "Agent task \(task.state); waiting for observed model progress"
                )
            }
            deadline = Date(timeIntervalSince1970: TimeInterval(task.createdAt) / 1_000)
                .addingTimeInterval(TimeInterval(min(task.limits.timeoutMs, 3_600_000)) / 1_000)

            let batch = try await client.agentHost([
                "op": .string("eventsNative"),
                "appId": .string(appID),
                "revision": .number(Double(revision)),
                "runId": .string(runID),
                "afterSequence": .number(Double(outputProgress.sequence))
            ], as: LocalAppAgentNativeEvents.self)
            let previousSequence = outputProgress.sequence
            let outputIncreased = try outputProgress.consume(batch, runID: runID, threadID: threadID)
            let events = batch.sequence > previousSequence ? batch.events : []
            if outputIncreased {
                LocalAppAgentProgressReporting.update(progress, outputBytes: outputProgress.outputBytes)
                LocalAppAgentBackgroundController.logObservedOutput(
                    contextKey: contextKey,
                    outputBytes: outputProgress.outputBytes
                )
            }
            if let terminalEvent = LocalAppAgentRunEvent.terminalEvent(in: events),
               let type = LocalAppAgentRunEvent.type(terminalEvent) {
                if type == "RUN_FINISHED" {
                    guard LocalAppAgentRunEvent.matches(terminalEvent, runID: runID, threadID: threadID) else {
                        throw LocalAppServiceFailure("INVALID_AGENT_EVENT", "The AG-UI terminal event belongs to a different thread or run.")
                    }
                    LocalAppAgentProgressReporting.finish(
                        progress,
                        success: LocalAppAgentRunEvent.isSuccessfulTerminal(
                            terminalEvent,
                            runID: runID,
                            threadID: threadID
                        )
                    )
                    return terminalEvent
                }
                if type == "RUN_ERROR" {
                    LocalAppAgentProgressReporting.finish(progress, success: false)
                    return terminalEvent
                }
            }
            if task.isTerminal {
                LocalAppAgentProgressReporting.finish(progress, success: false)
                throw LocalAppServiceFailure(
                    task.error?.code ?? "AGENT_TERMINAL_EVENT_MISSING",
                    "The Agent task ended as \(task.state) without a terminal AG-UI event."
                )
            }
            do { try await Task.sleep(for: .seconds(1)) } catch { throw CancellationError() }
        }
        await cancellation.cancel(reason: .intentDeadline)
        LocalAppAgentProgressReporting.finish(progress, success: false)
        throw LocalAppServiceFailure("AGENT_TASK_TIMEOUT", "The Local App Agent task exceeded its configured time limit.")
    }
}

actor LocalAppAgentIntentCancellation {
    private let appID: String
    private let revision: UInt64
    private let sendHostRequest: @Sendable ([String: LocalAppJSON]) async -> Void
    private var runID: String?
    private var firstCancellationReason: LocalAppAgentCancellationReason?
    private var cancellationLogged = false
    private(set) var isCancelled = false

    init(client: LocalAppServiceClient, appID: String, revision: UInt64) {
        self.appID = appID
        self.revision = revision
        self.sendHostRequest = { request in
            _ = try? await client.agentHost(request, as: LocalAppJSON.self)
        }
    }

    init(
        appID: String,
        revision: UInt64,
        sendHostRequest: @escaping @Sendable ([String: LocalAppJSON]) async -> Void
    ) {
        self.appID = appID
        self.revision = revision
        self.sendHostRequest = sendHostRequest
    }

    func didStart(_ runID: String) async throws {
        self.runID = runID
        if isCancelled {
            logCancellationIfNeeded(runID)
            await cancelRun(runID)
            throw CancellationError()
        }
    }

    func cancel(reason: LocalAppAgentCancellationReason = .intentCancellation) async {
        isCancelled = true
        if firstCancellationReason == nil { firstCancellationReason = reason }
        if let runID {
            logCancellationIfNeeded(runID)
            await cancelRun(runID)
        }
    }

    private func logCancellationIfNeeded(_ runID: String) {
        guard !cancellationLogged, let reason = firstCancellationReason else { return }
        cancellationLogged = true
        let contextKey = LocalAppAgentWakeRequest(appID: appID, revision: revision, runID: runID).contextKey
        LocalAppAgentBackgroundController.logCancellation(reason, contextKey: contextKey)
    }

    private func cancelRun(_ runID: String) async {
        await sendHostRequest(LocalAppAgentHostRequests.cancelPublicRequest(
            appID: appID,
            revision: revision,
            requestID: runID
        ))
    }
}
