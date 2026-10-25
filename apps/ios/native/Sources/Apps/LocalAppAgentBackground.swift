import BackgroundTasks
import CryptoKit
import Foundation

/// A browser request may wake the app, but only this foreground action can
/// install the iOS 26 continued-processing lease that authorizes that request.
@MainActor
final class LocalAppAgentBackgroundController {
    private struct Context {
        let wake: LocalAppAgentWakeRequest
        let appName: String
        let identifier: String
        let client: LocalAppServiceClient
        var reservationExpiry: Date?
        var finished = false
    }

    private var contexts: [String: Context] = [:]
    private var requestIDsByIdentifier: [String: String] = [:]
    private var attachmentWaiters: [String: CheckedContinuation<Void, Error>] = [:]
    private var attachmentTimeouts: [String: Task<Void, Never>] = [:]
    private var tasks: [String: BGContinuedProcessingTask] = [:]
    private var monitors: [String: Task<Void, Never>] = [:]

    func acquireAndReserve(
        _ wake: LocalAppAgentWakeRequest,
        appName: String,
        returnURL: URL,
        origin: String,
        client: LocalAppServiceClient
    ) async throws -> URL {
        guard #available(iOS 26.0, *) else {
            throw LocalAppServiceFailure("BACKGROUND_UNAVAILABLE", "Browser Agent handoff requires iOS 26 or later.")
        }
        guard !wake.runID.isEmpty, wake.runID.utf8.count <= 128,
              !wake.runID.unicodeScalars.contains(where: { CharacterSet.controlCharacters.contains($0) }) else {
            throw LocalAppServiceFailure("INVALID_AGENT_WAKE", "This Agent request has an invalid run id.")
        }
        guard LocalAppExternalBrowserURL.matches(returnURL, origin: origin) else {
            throw LocalAppServiceFailure("INVALID_APP_ORIGIN", "The app launch URL is not its registered local origin.")
        }
        guard let bundleID = Bundle.main.bundleIdentifier else {
            throw LocalAppServiceFailure("BACKGROUND_UNAVAILABLE", "The app identifier is unavailable.")
        }
        let contextKey = wake.contextKey
        if let existing = contexts[contextKey], !existing.finished {
            if tasks[contextKey] != nil, existing.reservationExpiry.map({ $0 > Date() }) == true {
                return try LocalAppAgentWakeURL.ready(returnURL, origin: origin, runID: wake.runID)
            }
            throw LocalAppServiceFailure("AGENT_WAKE_IN_PROGRESS", "This browser request is already being prepared. Wait briefly before returning to it.")
        }

        let identifier = Self.backgroundIdentifier(bundleID: bundleID, wake: wake)
        let context = Context(wake: wake, appName: appName, identifier: identifier, client: client)
        contexts[contextKey] = context
        requestIDsByIdentifier[identifier] = contextKey
        do { try register(identifier) }
        catch {
            contexts.removeValue(forKey: contextKey)
            requestIDsByIdentifier.removeValue(forKey: identifier)
            throw error
        }

        let request = BGContinuedProcessingTaskRequest(
            identifier: identifier,
            title: String(localized: "Local App Agent · \(appName)"),
            subtitle: "Waiting for this browser request"
        )
        request.strategy = .fail

        do {
            try await submit(request)
            try await waitForAttachment(contextKey: contextKey)
            guard !contexts[contextKey, default: context].finished,
                  tasks[contextKey] != nil else {
                throw LocalAppServiceFailure("BACKGROUND_LEASE_EXPIRED", "iOS ended this Agent handoff before it was ready.")
            }
            let reservation = try await client.agentHost([
                "op": .string("wake.reserve"),
                "appId": .string(wake.appID),
                "revision": .number(Double(wake.revision)),
                "runId": .string(wake.runID),
                "receiveTimeoutMs": .number(120_000)
            ], as: LocalAppWakeReservation.self)
            guard reservation.appId == wake.appID,
                  reservation.revision == wake.revision,
                  reservation.runId == wake.runID,
                  reservation.expiresAtMs > UInt64(Date().timeIntervalSince1970 * 1_000) else {
                throw LocalAppServiceFailure("INVALID_AGENT_WAKE", "The host did not confirm this app request.")
            }
            guard !contexts[contextKey, default: context].finished,
                  tasks[contextKey] != nil else {
                try? await clearReservation(wake, client: client)
                throw LocalAppServiceFailure("BACKGROUND_LEASE_EXPIRED", "iOS ended this Agent handoff before it was ready.")
            }
            contexts[contextKey]?.reservationExpiry = Date(
                timeIntervalSince1970: TimeInterval(reservation.expiresAtMs) / 1_000
            )
            monitors[contextKey] = Task { @MainActor [weak self] in
                await self?.monitor(contextKey, client: client)
            }
            return try LocalAppAgentWakeURL.ready(returnURL, origin: origin, runID: wake.runID)
        } catch {
            await cancel(wake, client: client)
            throw error
        }
    }

    func cancel(_ wake: LocalAppAgentWakeRequest, client: LocalAppServiceClient) async {
        let contextKey = wake.contextKey
        guard var context = contexts[contextKey], context.wake == wake, !context.finished else {
            try? await clearReservation(wake, client: client)
            return
        }
        context.finished = true
        contexts[contextKey] = context
        attachmentTimeouts.removeValue(forKey: contextKey)?.cancel()
        monitors.removeValue(forKey: contextKey)?.cancel()
        attachmentWaiters.removeValue(forKey: contextKey)?.resume(throwing: CancellationError())
        BGTaskScheduler.shared.cancel(taskRequestWithIdentifier: context.identifier)
        try? await client.agentHost([
            "op": .string("tasks.cancel"),
            "appId": .string(wake.appID),
            "revision": .number(Double(wake.revision)),
            "runId": .string(wake.runID)
        ], as: LocalAppJSON.self)
        try? await clearReservation(wake, client: client)
        finishTask(contextKey: contextKey, success: false)
        contexts.removeValue(forKey: contextKey)
        requestIDsByIdentifier.removeValue(forKey: context.identifier)
    }

    private func submit(_ request: BGContinuedProcessingTaskRequest) async throws {
        if #available(iOS 27.0, *) {
            try await BGTaskScheduler.shared.submitTaskRequest(request)
        } else {
            try BGTaskScheduler.shared.submit(request)
        }
    }

    private func register(_ identifier: String) throws {
        let registered = BGTaskScheduler.shared.register(forTaskWithIdentifier: identifier, using: .main) { [weak self] task in
            Task { @MainActor [weak self] in
                self?.receive(task)
            }
        }
        guard registered else {
            throw LocalAppServiceFailure("BACKGROUND_UNAVAILABLE", "iOS did not register the Agent background task.")
        }
    }

    private func waitForAttachment(contextKey: String) async throws {
        if tasks[contextKey] != nil { return }
        try await withCheckedThrowingContinuation { continuation in
            attachmentWaiters[contextKey] = continuation
            attachmentTimeouts[contextKey] = Task { @MainActor [weak self] in
                do { try await Task.sleep(for: .seconds(12)) } catch { return }
                guard let self, let waiter = self.attachmentWaiters.removeValue(forKey: contextKey) else { return }
                if let identifier = self.contexts[contextKey]?.identifier {
                    BGTaskScheduler.shared.cancel(taskRequestWithIdentifier: identifier)
                }
                waiter.resume(throwing: LocalAppServiceFailure(
                    "BACKGROUND_LEASE_UNAVAILABLE",
                    "iOS could not start this Agent task immediately. Return to the app and try again."
                ))
            }
        }
    }

    private func receive(_ task: BGTask) {
        guard let contextKey = requestIDsByIdentifier[task.identifier],
              contexts[contextKey] != nil,
              let continued = task as? BGContinuedProcessingTask else {
            task.setTaskCompleted(success: false)
            return
        }
        tasks[contextKey] = continued
        continued.updateTitle(
            String(localized: "Local App Agent · \(contexts[contextKey]?.appName ?? "")"),
            subtitle: "Waiting for this browser request"
        )
        continued.expirationHandler = { [weak self] in
            Task { @MainActor [weak self] in
                await self?.expire(contextKey)
            }
        }
        attachmentTimeouts.removeValue(forKey: contextKey)?.cancel()
        attachmentWaiters.removeValue(forKey: contextKey)?.resume()
    }

    private func monitor(_ contextKey: String, client: LocalAppServiceClient) async {
        guard let context = contexts[contextKey], !context.finished,
              let receiveDeadline = context.reservationExpiry else { return }
        let runID = context.wake.runID
        var runDeadline: Date?
        var afterSequence: UInt64 = 0

        while !Task.isCancelled, Date() < (runDeadline ?? receiveDeadline) {
            do {
                let response = try await client.agentHost([
                    "op": .string("tasks.list"),
                    "appId": .string(context.wake.appID),
                    "revision": .number(Double(context.wake.revision)),
                    "requestId": .string(runID)
                ], as: LocalAppAgentTaskListResponse.self)
                guard response.appId == context.wake.appID,
                      response.revision == context.wake.revision else {
                    throw LocalAppServiceFailure("INVALID_AGENT_TASK", "The host returned a task for another app version.")
                }
                if let agentTask = response.tasks.first(where: { $0.requestId == runID }) {
                    if runDeadline == nil {
                        let timeoutMs = min(agentTask.limits.timeoutMs, 3_600_000)
                        runDeadline = Date(timeIntervalSince1970: TimeInterval(agentTask.createdAt) / 1_000)
                            .addingTimeInterval(TimeInterval(timeoutMs) / 1_000)
                    }
                    report(agentTask, appName: context.appName, contextKey: contextKey)
                } else if Date() >= receiveDeadline {
                    break
                }

                let batch = try await client.agentHost([
                    "op": .string("eventsNative"),
                    "appId": .string(context.wake.appID),
                    "revision": .number(Double(context.wake.revision)),
                    "runId": .string(runID),
                    "afterSequence": .number(Double(afterSequence))
                ], as: LocalAppAgentNativeEvents.self)
                guard batch.sequence >= afterSequence else {
                    throw LocalAppServiceFailure("INVALID_AGENT_EVENT", "The Local App event sequence moved backwards.")
                }
                afterSequence = batch.sequence
                for event in batch.events {
                    guard LocalAppAgentRunEvent.type(event) != nil else {
                        throw LocalAppServiceFailure("INVALID_AGENT_EVENT", "The Local App host returned an invalid AG-UI event.")
                    }
                }
                if let terminalEvent = LocalAppAgentRunEvent.terminalEvent(in: batch.events),
                   let type = LocalAppAgentRunEvent.type(terminalEvent) {
                    if type == "RUN_FINISHED" {
                        guard LocalAppAgentRunEvent.matchesRun(terminalEvent, runID: runID) else {
                            throw LocalAppServiceFailure("INVALID_AGENT_EVENT", "The AG-UI terminal event belongs to a different run.")
                        }
                        await complete(
                            context.wake,
                            success: LocalAppAgentRunEvent.isSuccessfulTerminal(terminalEvent, runID: runID),
                            client: client
                        )
                        return
                    }
                    if type == "RUN_ERROR" {
                        await complete(context.wake, success: false, client: client)
                        return
                    }
                }
                if let agentTask = response.tasks.first(where: { $0.requestId == runID }), agentTask.isTerminal {
                    await complete(context.wake, success: false, client: client)
                    return
                }
            } catch is CancellationError {
                return
            } catch let failure as LocalAppServiceFailure where Self.isPermanentMonitorFailure(failure) {
                await complete(context.wake, success: false, client: client)
                return
            } catch {
                // Transient local transport failures remain bounded by the
                // request receive deadline or the admitted task's own timeout.
            }
            do { try await Task.sleep(for: .milliseconds(750)) } catch { return }
        }

        try? await client.agentHost([
            "op": .string("tasks.cancel"),
            "appId": .string(context.wake.appID),
            "revision": .number(Double(context.wake.revision)),
            "runId": .string(runID)
        ], as: LocalAppJSON.self)
        await complete(context.wake, success: false, client: client)
    }

    private func report(_ agentTask: LocalAppAgentTaskSummary, appName: String, contextKey: String) {
        guard let task = tasks[contextKey] else { return }
        if let progress = agentTask.progress, progress.turnLimit > 0 {
            LocalAppAgentProgressReporting.update(
                task.progress,
                completedModelTurns: progress.completedModelTurns,
                turnLimit: progress.turnLimit
            )
            task.updateTitle(
                String(localized: "Local App Agent · \(appName)"),
                subtitle: "Completed \(progress.completedModelTurns) model turns; limit \(progress.turnLimit)"
            )
        } else if ["queued", "running"].contains(agentTask.state) {
            LocalAppAgentProgressReporting.update(
                task.progress,
                description: agentTask.phase ?? "\(agentTask.state.capitalized); waiting for observed model progress"
            )
            task.updateTitle(
                String(localized: "Local App Agent · \(appName)"),
                subtitle: agentTask.phase ?? "\(agentTask.state.capitalized); waiting for observed model progress"
            )
        } else if let phase = agentTask.phase, !phase.isEmpty {
            task.updateTitle(String(localized: "Local App Agent · \(appName)"), subtitle: phase)
        }
    }

    private func expire(_ contextKey: String) async {
        guard let context = contexts[contextKey], !context.finished else { return }
        await cancel(context.wake, client: context.client)
    }

    private func complete(_ wake: LocalAppAgentWakeRequest, success: Bool, client: LocalAppServiceClient) async {
        let contextKey = wake.contextKey
        guard var context = contexts[contextKey], context.wake == wake, !context.finished else { return }
        context.finished = true
        contexts[contextKey] = context
        monitors.removeValue(forKey: contextKey)?.cancel()
        attachmentTimeouts.removeValue(forKey: contextKey)?.cancel()
        try? await clearReservation(wake, client: client)
        finishTask(contextKey: contextKey, success: success)
        contexts.removeValue(forKey: contextKey)
        requestIDsByIdentifier.removeValue(forKey: context.identifier)
    }

    private func clearReservation(_ wake: LocalAppAgentWakeRequest, client: LocalAppServiceClient) async throws {
        let _: LocalAppJSON = try await client.agentHost([
            "op": .string("wake.clear"),
            "appId": .string(wake.appID),
            "revision": .number(Double(wake.revision)),
            "runId": .string(wake.runID)
        ], as: LocalAppJSON.self)
    }

    private func finishTask(contextKey: String, success: Bool) {
        guard let task = tasks.removeValue(forKey: contextKey) else { return }
        task.expirationHandler = nil
        LocalAppAgentProgressReporting.finish(task.progress, success: success)
        task.setTaskCompleted(success: success)
    }

    static func isPermanentMonitorFailure(_ error: Error) -> Bool {
        guard let failure = error as? LocalAppServiceFailure else { return false }
        return [
            "release_changed",
            "release_unavailable",
            "app_not_found",
            "agent_not_declared",
            "agent_not_authorized",
            "agent_host_replaced",
            "invalid_revision",
            "INVALID_AGENT_TASK",
            "INVALID_AGENT_EVENT",
        ].contains(failure.code)
    }

    private static func backgroundIdentifier(bundleID: String, wake: LocalAppAgentWakeRequest) -> String {
        let identity = "\(wake.appID)\u{0}\(wake.revision)\u{0}\(wake.runID)\u{0}\(UUID().uuidString)"
        let digest = SHA256.hash(data: Data(identity.utf8)).map { String(format: "%02x", $0) }.joined()
        return "\(bundleID).local-app.\(digest)"
    }
}

extension LocalAppAgentWakeRequest {
    var contextKey: String { "\(appID)\u{0}\(revision)\u{0}\(runID)" }
}

enum LocalAppAgentProgressReporting {
    static func update(
        _ progress: Progress,
        completedModelTurns: UInt64? = nil,
        turnLimit: UInt64? = nil,
        description: String? = nil
    ) {
        // A turn limit is a ceiling, not a forecast. Keep the system progress
        // indeterminate and report only observed turn counts in its text.
        progress.totalUnitCount = -1
        progress.completedUnitCount = Int64(completedModelTurns ?? 0)
        progress.localizedDescription = "Local App Agent"
        if let completedModelTurns, let turnLimit {
            progress.localizedAdditionalDescription = "Completed \(completedModelTurns) model turns; limit \(turnLimit)"
        } else {
            progress.localizedAdditionalDescription = description ?? "Waiting for observed model progress"
        }
    }

    static func finish(_ progress: Progress, success: Bool) {
        let observation = progress.localizedAdditionalDescription
        progress.localizedDescription = "Local App Agent"
        if success {
            let completed = max(1, progress.completedUnitCount)
            progress.totalUnitCount = completed
            progress.completedUnitCount = completed
            if let observation, observation.hasPrefix("Completed ") {
                progress.localizedAdditionalDescription = observation
            } else {
                progress.localizedAdditionalDescription = "Completed"
            }
        } else {
            progress.totalUnitCount = -1
            if let observation, observation.hasPrefix("Completed ") {
                progress.localizedAdditionalDescription = "\(observation); task ended"
            } else {
                progress.localizedAdditionalDescription = "Task ended"
            }
        }
    }
}

enum LocalAppAgentWakeURL {
    static func ready(_ url: URL, origin: String, runID: String) throws -> URL {
        guard let parts = URLComponents(url: url, resolvingAgainstBaseURL: false),
              parts.scheme?.lowercased() == "http", parts.host == "127.0.0.1",
              parts.user == nil, parts.password == nil, let port = parts.port, (1...65_535).contains(port),
              !runID.isEmpty, runID.utf8.count <= 128,
              !runID.unicodeScalars.contains(where: { CharacterSet.controlCharacters.contains($0) }) else {
            throw LocalAppServiceFailure("INVALID_APP_ORIGIN", "The app launch URL is not a trusted local browser address.")
        }
        if !LocalAppExternalBrowserURL.matches(url, origin: origin) {
            throw LocalAppServiceFailure("INVALID_APP_ORIGIN", "The app page does not match its registered local origin.")
        }
        var updated = parts
        var query = updated.queryItems ?? []
        query.removeAll { $0.name == "__lingxi_ready" }
        query.append(URLQueryItem(name: "__lingxi_ready", value: runID))
        updated.queryItems = query
        guard let result = updated.url else {
            throw LocalAppServiceFailure("INVALID_APP_ORIGIN", "The app launch URL could not be prepared for return.")
        }
        return result
    }
}

enum LocalAppExternalBrowserURL {
    static func matches(_ url: URL, origin rawOrigin: String) -> Bool {
        guard let origin = URL(string: rawOrigin),
              origin.scheme?.lowercased() == "http", origin.host == "127.0.0.1",
              origin.user == nil, origin.password == nil, let port = origin.port,
              ["", "/"].contains(origin.path), origin.query == nil, origin.fragment == nil,
              let parts = URLComponents(url: url, resolvingAgainstBaseURL: false) else { return false }
        return parts.scheme?.lowercased() == "http" && parts.host == "127.0.0.1"
            && parts.port == port && parts.user == nil && parts.password == nil
    }
}
