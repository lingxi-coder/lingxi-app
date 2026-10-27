import BackgroundTasks
import CryptoKit
import Foundation
import OSLog

enum LocalAppAgentCancellationReason: String, Sendable {
    case explicit
    case requestReplaced
    case releaseChanged
    case monitorDeadline
    case monitorCancelled
    case systemExpiration
    case acquireFailure
    case intentCancellation
    case intentDeadline
}

/// A browser request may wake the app, but only this foreground action can
/// install the iOS 26 continued-processing lease that authorizes that request.
@MainActor
final class LocalAppAgentBackgroundController {
    private nonisolated static let backgroundLog = Logger(
        subsystem: "com.lingxi.code",
        category: "LocalAppAgentBackground"
    )

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
    private let browserDelivery = LocalAppBrowserDeliveryController.live()

    func openBrowserPage(
        scope: LocalAppBrowserDeliveryScope,
        open: @MainActor () -> Void
    ) -> LocalAppBrowserDeliveryController.Acquisition {
        browserDelivery.openPage(scope: scope, open: open)
    }

    func endBrowserDelivery(_ scope: LocalAppBrowserDeliveryScope) {
        browserDelivery.end(scope)
    }

    func cancel(
        appID: String,
        revision: UInt64,
        client: LocalAppServiceClient,
        reason: LocalAppAgentCancellationReason = .explicit
    ) async {
        let scope = LocalAppBrowserDeliveryScope(appID: appID, revision: revision)
        browserDelivery.end(scope)
        let wakes = contexts.values
            .filter { $0.wake.appID == appID && $0.wake.revision == revision }
            .map(\.wake)
        for wake in wakes {
            await cancel(wake, client: client, reason: reason)
        }
    }

    func cancel(
        appID: String,
        client: LocalAppServiceClient,
        reason: LocalAppAgentCancellationReason = .explicit
    ) async {
        browserDelivery.endAll(appID: appID)
        let wakes = contexts.values
            .filter { $0.wake.appID == appID }
            .map(\.wake)
        for wake in wakes {
            await cancel(wake, client: client, reason: reason)
        }
    }

    func cancelOutdated(
        appID: String,
        keepingRevision revision: UInt64,
        client: LocalAppServiceClient
    ) async {
        browserDelivery.endStale(appID: appID, keepingRevision: revision)
        let wakes = contexts.values
            .filter { $0.wake.appID == appID && $0.wake.revision != revision }
            .map(\.wake)
        for wake in wakes {
            await cancel(wake, client: client, reason: .releaseChanged)
        }
    }

    func acquireAndReserve(
        _ wake: LocalAppAgentWakeRequest,
        appName: String,
        launch: LocalAppLaunch,
        client: LocalAppServiceClient
    ) async throws -> URL {
        guard #available(iOS 26.0, *) else {
            throw LocalAppServiceFailure("BACKGROUND_UNAVAILABLE", "Browser Agent handoff requires iOS 26 or later.")
        }
        guard !wake.runID.isEmpty, wake.runID.utf8.count <= 128,
              !wake.runID.unicodeScalars.contains(where: { CharacterSet.controlCharacters.contains($0) }) else {
            throw LocalAppServiceFailure("INVALID_AGENT_WAKE", "This Agent request has an invalid run id.")
        }
        let readyURL = try LocalAppAgentWakeURL.ready(launch, agentRunID: wake.runID)
        guard let bundleID = Bundle.main.bundleIdentifier else {
            throw LocalAppServiceFailure("BACKGROUND_UNAVAILABLE", "The app identifier is unavailable.")
        }
        let contextKey = wake.contextKey
        if let existing = contexts[contextKey], !existing.finished {
            if tasks[contextKey] != nil, existing.reservationExpiry.map({ $0 > Date() }) == true {
                Self.logPhase("ready_url_returned", contextKey: contextKey, success: true)
                return readyURL
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
            Self.logFailure(error, contextKey: contextKey)
            throw error
        }

        Self.logPhase("registered", contextKey: contextKey, success: true)
        let request = BGContinuedProcessingTaskRequest(
            identifier: identifier,
            title: String(localized: "Local App Agent · \(appName)"),
            subtitle: "Waiting for this browser request"
        )
        request.strategy = .fail

        do {
            try await submit(request)
            Self.logPhase("submitted", contextKey: contextKey, success: true)
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
                do {
                    try await clearReservation(wake, client: client)
                } catch {
                    Self.logFailure(error, contextKey: contextKey)
                }
                throw LocalAppServiceFailure("BACKGROUND_LEASE_EXPIRED", "iOS ended this Agent handoff before it was ready.")
            }
            contexts[contextKey]?.reservationExpiry = Date(
                timeIntervalSince1970: TimeInterval(reservation.expiresAtMs) / 1_000
            )
            monitors[contextKey] = Task { @MainActor [weak self] in
                await self?.monitor(contextKey, client: client)
            }
            Self.logPhase("wake_reserve", contextKey: contextKey, success: true)
            Self.logPhase("ready_url_returned", contextKey: contextKey, success: true)
            return readyURL
        } catch {
            await cancel(wake, client: client, reason: .acquireFailure)
            Self.logFailure(error, contextKey: contextKey)
            throw error
        }
    }

    func cancel(
        _ wake: LocalAppAgentWakeRequest,
        client: LocalAppServiceClient,
        reason: LocalAppAgentCancellationReason = .explicit
    ) async {
        let contextKey = wake.contextKey
        browserDelivery.end(wake.deliveryScope)
        guard var context = contexts[contextKey], context.wake == wake, !context.finished else {
            do {
                try await clearReservation(wake, client: client)
            } catch {
                Self.logFailure(error, contextKey: contextKey)
            }
            Self.logPhase("cleanup_requested", contextKey: contextKey, success: nil)
            return
        }
        context.finished = true
        contexts[contextKey] = context
        Self.logCancellation(reason, contextKey: contextKey)
        attachmentTimeouts.removeValue(forKey: contextKey)?.cancel()
        monitors.removeValue(forKey: contextKey)?.cancel()
        attachmentWaiters.removeValue(forKey: contextKey)?.resume(throwing: CancellationError())
        BGTaskScheduler.shared.cancel(taskRequestWithIdentifier: context.identifier)
        do {
            let _: LocalAppJSON = try await client.agentHost(
                LocalAppAgentHostRequests.cancelPublicRequest(
                    appID: wake.appID,
                    revision: wake.revision,
                    requestID: wake.runID
                ),
                as: LocalAppJSON.self
            )
        } catch {
            Self.logFailure(error, contextKey: contextKey)
        }
        do {
            try await clearReservation(wake, client: client)
        } catch {
            Self.logFailure(error, contextKey: contextKey)
        }
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
        Self.logPhase("task_attached", contextKey: contextKey, success: true)
        LocalAppAgentProgressReporting.update(
            continued.progress,
            outputBytes: 0,
            description: "Waiting for this browser request"
        )
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
        var outputProgress = LocalAppAgentOutputProgress()

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
                let agentTask = response.tasks.first(where: { $0.requestId == runID })
                if let agentTask {
                    if runDeadline == nil {
                        let timeoutMs = min(agentTask.limits.timeoutMs, 3_600_000)
                        runDeadline = Date(timeIntervalSince1970: TimeInterval(agentTask.createdAt) / 1_000)
                            .addingTimeInterval(TimeInterval(timeoutMs) / 1_000)
                    }
                    report(agentTask, outputBytes: outputProgress.outputBytes, appName: context.appName, contextKey: contextKey)
                } else if Date() >= receiveDeadline {
                    break
                }

                let batch = try await client.agentHost([
                    "op": .string("eventsNative"),
                    "appId": .string(context.wake.appID),
                    "revision": .number(Double(context.wake.revision)),
                    "runId": .string(runID),
                    "afterSequence": .number(Double(outputProgress.sequence))
                ], as: LocalAppAgentNativeEvents.self)
                let previousSequence = outputProgress.sequence
                let outputIncreased = try outputProgress.consume(batch, runID: runID)
                let events = batch.sequence > previousSequence ? batch.events : []
                if outputIncreased {
                    report(agentTask, outputBytes: outputProgress.outputBytes, appName: context.appName, contextKey: contextKey)
                    Self.logObservedOutput(contextKey: contextKey, outputBytes: outputProgress.outputBytes)
                }
                if let terminalEvent = LocalAppAgentRunEvent.terminalEvent(in: events),
                   let type = LocalAppAgentRunEvent.type(terminalEvent) {
                    if type == "RUN_FINISHED" {
                        guard LocalAppAgentRunEvent.matchesRun(terminalEvent, runID: runID) else {
                            throw LocalAppServiceFailure("INVALID_AGENT_EVENT", "The AG-UI terminal event belongs to a different run.")
                        }
                        let success = LocalAppBrowserResultDeliveryPolicy.isSuccessfulRun(
                            taskState: agentTask?.state,
                            terminalEvent: terminalEvent,
                            runID: runID
                        )
                        await complete(
                            context.wake,
                            success: success,
                            deliverResult: LocalAppBrowserResultDeliveryPolicy.shouldDeliver(
                                taskState: agentTask?.state,
                                terminalEvent: terminalEvent
                            ),
                            client: client
                        )
                        return
                    }
                    if type == "RUN_ERROR" {
                        await complete(
                            context.wake,
                            success: false,
                            deliverResult: LocalAppBrowserResultDeliveryPolicy.shouldDeliver(
                                taskState: agentTask?.state,
                                terminalEvent: terminalEvent
                            ),
                            client: client
                        )
                        return
                    }
                }
                if let agentTask, agentTask.isTerminal {
                    await complete(
                        context.wake,
                        success: agentTask.state == "succeeded",
                        deliverResult: LocalAppBrowserResultDeliveryPolicy.shouldDeliver(
                            taskState: agentTask.state,
                            terminalEvent: nil
                        ),
                        client: client
                    )
                    return
                }
            } catch is CancellationError {
                return
            } catch let failure as LocalAppServiceFailure where Self.isPermanentMonitorFailure(failure) {
                Self.logFailure(failure, contextKey: contextKey)
                await complete(context.wake, success: false, deliverResult: false, client: client)
                return
            } catch {
                // Transient local transport failures remain bounded by the
                // request receive deadline or the admitted task's own timeout.
            }
            do { try await Task.sleep(for: .milliseconds(750)) } catch { return }
        }

        await cancel(
            context.wake,
            client: client,
            reason: Task.isCancelled ? .monitorCancelled : .monitorDeadline
        )
    }

    private func report(
        _ agentTask: LocalAppAgentTaskSummary?,
        outputBytes: Int64,
        appName: String,
        contextKey: String
    ) {
        guard let task = tasks[contextKey] else { return }
        let observed = agentTask?.progress
        let hasTurnLimit = (observed?.turnLimit ?? 0) > 0
        let fallback = agentTask?.phase
            ?? agentTask.map { "\($0.state.capitalized); waiting for observed model progress" }
        let subtitle = LocalAppAgentProgressReporting.update(
            task.progress,
            outputBytes: outputBytes,
            completedModelTurns: hasTurnLimit ? observed?.completedModelTurns : nil,
            turnLimit: hasTurnLimit ? observed?.turnLimit : nil,
            description: fallback
        )
        task.updateTitle(String(localized: "Local App Agent · \(appName)"), subtitle: subtitle)
    }

    private func expire(_ contextKey: String) async {
        guard let context = contexts[contextKey], !context.finished else { return }
        Self.logPhase("expire", contextKey: contextKey, success: nil)
        await cancel(context.wake, client: context.client, reason: .systemExpiration)
    }

    private func complete(
        _ wake: LocalAppAgentWakeRequest,
        success: Bool,
        deliverResult: Bool,
        client: LocalAppServiceClient
    ) async {
        let contextKey = wake.contextKey
        guard var context = contexts[contextKey], context.wake == wake, !context.finished else { return }
        context.finished = true
        contexts[contextKey] = context
        monitors.removeValue(forKey: contextKey)?.cancel()
        attachmentTimeouts.removeValue(forKey: contextKey)?.cancel()
        if deliverResult {
            let acquisition = await browserDelivery.finishBackgroundTaskAfterStartingResultDelivery(
                scope: wake.deliveryScope
            ) {
                self.finishTask(contextKey: contextKey, success: success)
                do {
                    try await self.clearReservation(wake, client: client)
                } catch {
                    Self.logFailure(error, contextKey: contextKey)
                }
            }
            if acquisition == .unavailable {
                Self.logFailure(
                    LocalAppServiceFailure(
                        "BROWSER_DELIVERY_UNAVAILABLE",
                        "iOS could not reserve a short window to deliver the Local App result."
                    ),
                    contextKey: contextKey
                )
            }
        } else {
            browserDelivery.end(wake.deliveryScope)
            finishTask(contextKey: contextKey, success: success)
            do {
                try await clearReservation(wake, client: client)
            } catch {
                Self.logFailure(error, contextKey: contextKey)
            }
        }
        contexts.removeValue(forKey: contextKey)
        requestIDsByIdentifier.removeValue(forKey: context.identifier)
        Self.logPhase("complete", contextKey: contextKey, success: success)
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

    private static func logPhase(_ phase: String, contextKey: String, success: Bool?) {
        if let success {
            backgroundLog.log(
                "phase=\(phase, privacy: .public) keyhash=\(keyHash(contextKey), privacy: .public) success=\(success, privacy: .public)"
            )
        } else {
            backgroundLog.log(
                "phase=\(phase, privacy: .public) keyhash=\(keyHash(contextKey), privacy: .public)"
            )
        }
    }

    nonisolated static func logCancellation(_ reason: LocalAppAgentCancellationReason, contextKey: String) {
        backgroundLog.log(
            "phase=cancel_requested keyhash=\(keyHash(contextKey), privacy: .public) reason=\(reason.rawValue, privacy: .public)"
        )
    }

    nonisolated static func logObservedOutput(contextKey: String, outputBytes: Int64) {
        backgroundLog.log(
            "phase=output_observed keyhash=\(keyHash(contextKey), privacy: .public) outputBytes=\(outputBytes, privacy: .public)"
        )
    }

    private static func logFailure(_ error: Error, contextKey: String) {
        let digest = keyHash(contextKey)
        if let failure = error as? LocalAppServiceFailure {
            backgroundLog.error(
                "phase=failure keyhash=\(digest, privacy: .public) success=false failureCode=\(failure.code, privacy: .public)"
            )
        } else {
            let nsError = error as NSError
            backgroundLog.error(
                "phase=failure keyhash=\(digest, privacy: .public) success=false errorDomain=\(nsError.domain, privacy: .public) errorCode=\(nsError.code, privacy: .public)"
            )
        }
    }

    private nonisolated static func keyHash(_ contextKey: String) -> String {
        SHA256.hash(data: Data(contextKey.utf8))
            .prefix(8)
            .map { String(format: "%02x", $0) }
            .joined()
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
    var deliveryScope: LocalAppBrowserDeliveryScope {
        LocalAppBrowserDeliveryScope(appID: appID, revision: revision)
    }
}

@MainActor
enum LocalAppAgentHandoffLaunchResolver {
    static func resolve(
        _ wake: LocalAppAgentWakeRequest,
        currentLaunch: () async throws -> LocalAppLaunch
    ) async throws -> LocalAppLaunch {
        let launch = try await currentLaunch()
        guard launch.run.appId == wake.appID,
              launch.run.revision == wake.revision,
              launch.run.phase == "running" else {
            throw LocalAppServiceFailure("STALE_AGENT_WAKE", "The current app run no longer matches this Agent request.")
        }
        _ = try LocalAppAgentWakeURL.ready(launch, agentRunID: wake.runID)
        return launch
    }
}

enum LocalAppAgentProgressReporting {
    @discardableResult
    static func update(
        _ progress: Progress,
        outputBytes: Int64,
        completedModelTurns: UInt64? = nil,
        turnLimit: UInt64? = nil,
        description: String? = nil
    ) -> String {
        // The output length is unknown until the run ends. A turn limit is a
        // ceiling, so it can be explanatory text but cannot be progress units.
        progress.totalUnitCount = -1
        progress.completedUnitCount = max(0, outputBytes)
        progress.localizedDescription = "Local App Agent"
        let observation: String
        if outputBytes > 0 {
            observation = "Received \(outputBytes) output bytes; total unknown"
        } else if let completedModelTurns, let turnLimit {
            observation = "Completed \(completedModelTurns) model turns; limit \(turnLimit)"
        } else {
            observation = description ?? "Waiting for observed model progress"
        }
        progress.localizedAdditionalDescription = observation
        return observation
    }

    static func finish(_ progress: Progress, success: Bool) {
        let observation = progress.localizedAdditionalDescription
        progress.localizedDescription = "Local App Agent"
        if success {
            let completed = max(1, progress.completedUnitCount)
            progress.totalUnitCount = completed
            progress.completedUnitCount = completed
            if let observation, observation.hasPrefix("Received ") {
                progress.localizedAdditionalDescription = "Received \(progress.completedUnitCount) output bytes; complete"
            } else if let observation, observation.hasPrefix("Completed ") {
                progress.localizedAdditionalDescription = observation
            } else {
                progress.localizedAdditionalDescription = "Completed"
            }
        } else {
            progress.totalUnitCount = -1
            if let observation, observation.hasPrefix("Completed ") || observation.hasPrefix("Received ") {
                progress.localizedAdditionalDescription = "\(observation); task ended"
            } else {
                progress.localizedAdditionalDescription = "Task ended"
            }
        }
    }
}

enum LocalAppAgentWakeURL {
    static func ready(_ launch: LocalAppLaunch, agentRunID: String) throws -> URL {
        let url = launch.url
        guard launch.run.phase == "running",
              hasAppRunPath(url, runID: launch.run.runId) else {
            throw LocalAppServiceFailure("STALE_APP_RUN_URL", "The app launch URL does not match its current run.")
        }
        guard let parts = URLComponents(url: url, resolvingAgainstBaseURL: false),
              parts.scheme?.lowercased() == "http", parts.host == "127.0.0.1",
              parts.user == nil, parts.password == nil, let port = parts.port, (1...65_535).contains(port),
              !agentRunID.isEmpty, agentRunID.utf8.count <= 128,
              !agentRunID.unicodeScalars.contains(where: { CharacterSet.controlCharacters.contains($0) }) else {
            throw LocalAppServiceFailure("INVALID_APP_ORIGIN", "The app launch URL is not a trusted local browser address.")
        }
        if !LocalAppExternalBrowserURL.matches(url, origin: launch.origin) {
            throw LocalAppServiceFailure("INVALID_APP_ORIGIN", "The app page does not match its registered local origin.")
        }
        var updated = parts
        var query = updated.queryItems ?? []
        query.removeAll { $0.name == "__lingxi_ready" }
        query.append(URLQueryItem(name: "__lingxi_ready", value: agentRunID))
        updated.queryItems = query
        guard let result = updated.url else {
            throw LocalAppServiceFailure("INVALID_APP_ORIGIN", "The app launch URL could not be prepared for return.")
        }
        return result
    }

    private static func hasAppRunPath(_ url: URL, runID: String) -> Bool {
        let isValidRunID = !runID.isEmpty && runID.utf8.count <= 128 && runID.utf8.allSatisfy { byte in
            (65...90).contains(byte) || (97...122).contains(byte) || (48...57).contains(byte)
                || byte == 45 || byte == 95
        }
        guard isValidRunID,
              let parts = URLComponents(url: url, resolvingAgainstBaseURL: false) else { return false }
        return parts.percentEncodedPath.hasPrefix("/__run/\(runID)/")
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
