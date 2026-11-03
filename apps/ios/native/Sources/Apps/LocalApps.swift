import Foundation
import Observation

/// One service for the local Linux user, independent of Firebase and chat engines.
actor LocalAppServiceClient {
    private struct TargetCompatibility: Decodable, Sendable { let compatible: Bool }
    private var handle: MobileLocalAppServiceHandle?
    private var linuxAttached = false
    func request<T: Decodable & Sendable>(_ fields: [String: LocalAppJSON], as type: T.Type = T.self,
                                          purpose: LocalAppRequestPurpose = .ordinary) async throws -> T {
        try LocalAppDevelopmentAdmission.requireServiceRequest(fields, purpose: purpose)
        if handle == nil {
            let service = try buildIosLocalAppService(configHome: MobileUserStorage.directories().configHome, runtime: nil)
            let target = await MainActor.run { UnifiedAppMarket.targets.last! }
            let _: LocalAppJSON = try await send(service, ["op": .string("configureTarget"), "target": try .encode(target)])
            handle = service
        }
        guard var service = handle else { throw LocalAppServiceFailure("SERVICE_UNAVAILABLE", "Local apps are unavailable.") }
        if !linuxAttached, ["start", "resume", "restart", "repair"].contains(fields["op"]?.text ?? ""), let id = fields["appId"]?.text {
            let record: UnifiedAppRecord = try await send(service, ["op": .string("get"), "appId": .string(id)])
            let revision = fields["revision"] ?? .number(Double(record.currentRevision))
            let release: UnifiedAppRelease = try await send(service, ["op": .string("getRelease"), "appId": .string(id), "revision": revision])
            // Never even prepare the Linux adapter before exact-content consent.
            if release.manifest.executesCode, record.executionGrants.contains(release.digest) {
                // Reject Store-channel/target mismatches before preparing rootfs
                // resources or constructing any Linux runtime adapter.
                guard let target = await MainActor.run(body: { UnifiedAppMarket.targets.first { $0.environment == "mobile-linux" } }) else {
                    throw LocalAppServiceFailure("UNSUPPORTED_CHANNEL", "此发行渠道不支持安装、构建或运行应用服务。")
                }
                let compatibility: TargetCompatibility = try await send(service, ["op": .string("checkTarget"), "appId": .string(id), "revision": revision, "target": try .encode(target)])
                guard compatibility.compatible else { throw LocalAppServiceFailure("UNSUPPORTED_TARGET", "此版本不支持当前设备或发行渠道。") }
                LXISHRuntimeResourceBootstrap.prepare()
                let metadata = try LXISHRuntimeBundleMetadata.current()
                let workspace = try MobileUserStorage.prepareProjectsDirectory()
                let directories = MobileUserStorage.directories()
                let config = IosMobileLinuxConfigFfi(mode: .mobileLinux,
                    brandId: directories.brandId, linuxUser: directories.linuxUser, configDirName: directories.configDirName,
                    managedRoot: LXISHDefaultWorkspace.managedRootPath(),
                    workspaceHostPath: workspace.path, stableWorkspaceId: LXISHDefaultWorkspace.stableID(),
                    abi: "arm64", rootfsVersion: metadata.rootfsVersion, archiveSha256: metadata.archiveSha256,
                    authorizationFile: LXISHRuntimeBundleResources.authorizationManifestURL()?.path,
                    appSandboxRoot: ConversationSourceFactory.appSandboxRoot())
                let runtime = try createIosMobileLinuxRuntime(config: config)
                service = try buildIosLocalAppService(configHome: MobileUserStorage.directories().configHome, runtime: runtime)
                handle = service; linuxAttached = true
            }
        }
        return try await send(service, fields)
    }
    // Trusted native UI only; page calls use the authenticated external-browser gateway.
    func agentHost(_ request: String, authoringMode: SessionMode? = nil) async throws -> LocalAppJSON {
        try LocalAppDevelopmentAdmission.requireAgentHostRequest(request, mode: authoringMode)
        switch LocalAppAgentHostPolicy.requirement(for: request) {
        case .none:
            break
        case .available:
            try await LocalAppAgentModelHost.shared.ensureAvailable()
        case .currentConfiguration:
            try await LocalAppAgentModelHost.shared.ensure()
        }
        let _: LocalAppJSON = try await self.request(["op": .string("list")])
        guard let handle else { throw LocalAppServiceFailure("SERVICE_UNAVAILABLE", "Local apps are unavailable.") }
        let response = try await handle.agentHostJson(request: request)
        return try JSONDecoder().decode(LocalAppServiceEnvelope<LocalAppJSON>.self, from: Data(response.utf8)).result()
    }
    func agentHost<T: Decodable & Sendable>(_ fields: [String: LocalAppJSON], as type: T.Type,
                                            authoringMode: SessionMode? = nil) async throws -> T {
        let value = try await agentHost(String(decoding: JSONEncoder().encode(fields), as: UTF8.self), authoringMode: authoringMode)
        return try JSONDecoder().decode(T.self, from: JSONEncoder().encode(value))
    }
    private func send<T: Decodable & Sendable>(_ service: MobileLocalAppServiceHandle, _ fields: [String: LocalAppJSON]) async throws -> T {
        let data = try JSONEncoder().encode(fields)
        let response = try await service.requestJson(request: String(decoding: data, as: UTF8.self))
        return try JSONDecoder().decode(LocalAppServiceEnvelope<T>.self, from: Data(response.utf8)).result()
    }
}

struct LocalAppAgentConfigurationScopeSnapshot: Sendable {
    let wake: LocalAppAgentWakeRequest?
    let token: UUID?
    let contexts: [LocalAppAgentContextSnapshot]

    func matches(currentWake: LocalAppAgentWakeRequest?, currentToken: UUID?) -> Bool {
        guard let wake, let token else { return false }
        return currentWake == wake && currentToken == token
    }
}

enum LocalAppAgentHostPolicy {
    enum Requirement: Equatable {
        case none
        case available
        case currentConfiguration
    }

    static func requiresCode(for request: String) -> Bool {
        LocalAppDevelopmentAdmission.requiresCode(forAgentHostRequest: request)
    }

    static func requirement(for request: String) -> Requirement {
        guard let data = request.data(using: .utf8),
              let fields = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
              let op = fields["op"] as? String else { return .currentConfiguration }
        switch op {
        case "registerAuthoring", "getAuthoringGrant", "prepareAuthoringSource", "bindAuthoringSource",
             "createAuthoring", "restoreAuthoringSource", "remixAuthoringSource", "importRemixSource":
            return .none
        case "configure" where fields["settings"] is NSNull:
            return .available
        case "tasks.list", "eventsNative", "tasks.cancel", "wake.clear",
             "interactions", "answer", "settings":
            return .available
        default:
            return .currentConfiguration
        }
    }
}

enum LocalAppAgentHandoffCancellationPolicy {
    static func shouldCancelPreparation(
        wake: LocalAppAgentWakeRequest,
        token: UUID,
        currentWake: LocalAppAgentWakeRequest?,
        currentToken: UUID?,
        currentRevision: UInt64?
    ) -> Bool {
        // Removal or a release change revokes the old task even if an equal
        // public run ID has since been queued again.
        guard currentRevision == wake.revision else { return true }
        if currentWake == wake, let currentToken, currentToken != token { return false }
        return true
    }

    static func staleReason(
        wake: LocalAppAgentWakeRequest,
        currentWake: LocalAppAgentWakeRequest?,
        currentRevision: UInt64?
    ) -> LocalAppAgentCancellationReason {
        if let currentWake, currentWake != wake { return .requestReplaced }
        if let currentRevision, currentRevision != wake.revision { return .releaseChanged }
        return .acquireFailure
    }
}

/// Owns each browser handoff until preparation fails or the browser-open
/// callback finishes. A second caller must never clean up the first caller's
/// reservation while it is being prepared or handed to the browser.
@MainActor
final class LocalAppAgentHandoffPreparations {
    private enum State { case preparing, ready }
    private struct Entry { let token: UUID; var state: State }
    private var states: [String: Entry] = [:]

    func prepare<Value>(
        _ wake: LocalAppAgentWakeRequest,
        token: UUID,
        operation: () async throws -> Value
    ) async throws -> Value {
        let key = wake.contextKey
        guard states[key] == nil else {
            throw LocalAppServiceFailure("AGENT_WAKE_IN_PROGRESS", "This browser request is already being prepared. Wait briefly before returning to it.")
        }
        states[key] = Entry(token: token, state: .preparing)
        do {
            let value = try await operation()
            states[key]?.state = .ready
            return value
        } catch {
            if states[key]?.token == token { states.removeValue(forKey: key) }
            throw error
        }
    }

    func isReady(_ wake: LocalAppAgentWakeRequest, token: UUID) -> Bool {
        guard let entry = states[wake.contextKey], entry.token == token else { return false }
        if case .ready = entry.state { return true }
        return false
    }

    @discardableResult
    func finish(_ wake: LocalAppAgentWakeRequest, token: UUID) -> Bool {
        let key = wake.contextKey
        // An in-progress owner must retain its slot until it unwinds, even if
        // cancellation or replacement arrives during an awaited operation.
        guard isReady(wake, token: token) else { return false }
        states.removeValue(forKey: key)
        return true
    }
}

@MainActor
@Observable
final class UnifiedLocalApps {
    private(set) var records: [UnifiedAppRecord] = []
    private(set) var runs: [String: UnifiedAppRun] = [:]
    private(set) var busy = Set<String>()
    private(set) var error: String?
    private(set) var initialized = false
    let devicePermissions = LocalAppDevicePermissions()
    @ObservationIgnored let client: LocalAppServiceClient
    @ObservationIgnored private let agentBackground = LocalAppAgentBackgroundController()
    @ObservationIgnored private let handoffPreparations = LocalAppAgentHandoffPreparations()
    var browserReviewAppID: String?
    private(set) var browserAgentWake: LocalAppAgentWakeRequest?
    @ObservationIgnored private var browserAgentWakeToken: UUID?
    @ObservationIgnored private var latestBrowserAgentWakeToken: UUID?
    func consumeBrowserReview(_ id: String) -> Bool {
        guard browserReviewAppID == id else { return false }
        browserReviewAppID = nil
        return true
    }
    func queueBrowserAgentWake(appID: String, revision: UInt64, runID: String) {
        let next = LocalAppAgentWakeRequest(appID: appID, revision: revision, runID: runID)
        if browserAgentWake == next { return }
        if let previous = browserAgentWake, previous != next {
            let previousToken = browserAgentWakeToken
            Task {
                // A later handoff can requeue the same public run before this
                // asynchronous cleanup starts. Do not clear its reservation.
                if let previousToken, !LocalAppAgentHandoffCancellationPolicy.shouldCancelPreparation(
                    wake: previous, token: previousToken, currentWake: browserAgentWake,
                    currentToken: browserAgentWakeToken, currentRevision: record(previous.appID)?.currentRevision
                ) {
                    handoffPreparations.finish(previous, token: previousToken)
                    return
                }
                await agentBackground.cancel(previous, client: client, reason: .requestReplaced)
                if let previousToken { handoffPreparations.finish(previous, token: previousToken) }
            }
        }
        browserAgentWake = next
        let token = UUID()
        browserAgentWakeToken = token
        latestBrowserAgentWakeToken = token
    }
    func browserAgentWake(for appID: String) -> LocalAppAgentWakeRequest? {
        guard browserAgentWake?.appID == appID else { return nil }
        return browserAgentWake
    }
    func browserAgentWakeToken(for wake: LocalAppAgentWakeRequest) -> UUID? {
        browserAgentWake == wake ? browserAgentWakeToken : nil
    }
    func isBrowserAgentHandoffReady(_ wake: LocalAppAgentWakeRequest, token: UUID) -> Bool {
        browserAgentWake == wake && browserAgentWakeToken == token
            && record(wake.appID)?.currentRevision == wake.revision
            && handoffPreparations.isReady(wake, token: token)
    }
    private func clearBrowserAgentWake(_ wake: LocalAppAgentWakeRequest, token: UUID) {
        handoffPreparations.finish(wake, token: token)
        _ = detachBrowserAgentWake(wake, token: token)
    }
    @discardableResult
    private func detachBrowserAgentWake(_ wake: LocalAppAgentWakeRequest, token: UUID) -> Bool {
        guard browserAgentWake == wake, browserAgentWakeToken == token else { return false }
        browserAgentWake = nil
        browserAgentWakeToken = nil
        return true
    }
    func openBrowserPage(
        scope: LocalAppBrowserDeliveryScope,
        open: @MainActor () -> Void
    ) -> LocalAppBrowserDeliveryController.Acquisition {
        agentBackground.openBrowserPage(scope: scope, open: open)
    }
    func endBrowserPageDelivery(_ scope: LocalAppBrowserDeliveryScope) {
        agentBackground.endBrowserDelivery(scope)
    }
    func configurationScopeSnapshot(appID: String, revision: UInt64) -> LocalAppAgentConfigurationScopeSnapshot {
        let wake = browserAgentWake.flatMap { $0.appID == appID && $0.revision == revision ? $0 : nil }
        return LocalAppAgentConfigurationScopeSnapshot(
            wake: wake, token: wake == nil ? nil : browserAgentWakeToken,
            contexts: agentBackground.configurationSnapshot(appID: appID, revision: revision)
        )
    }
    func cancelBrowserAgentScope(_ snapshot: LocalAppAgentConfigurationScopeSnapshot) async {
        if snapshot.matches(currentWake: browserAgentWake, currentToken: browserAgentWakeToken),
           let wake = snapshot.wake, let token = snapshot.token,
           detachBrowserAgentWake(wake, token: token) {
            handoffPreparations.finish(wake, token: token)
        }
        await agentBackground.cancelConfigurationSnapshot(snapshot.contexts, client: client)
    }
    func prepareBrowserAgentHandoff(_ wake: LocalAppAgentWakeRequest, token: UUID) async throws -> (launch: LocalAppLaunch, readyURL: URL, token: UUID) {
        guard browserAgentWake == wake, browserAgentWakeToken == token else {
            throw LocalAppServiceFailure("STALE_AGENT_WAKE", "The app version changed before this Agent request could start.")
        }
        let result = try await handoffPreparations.prepare(wake, token: token) {
            try await self.prepareOwnedBrowserAgentHandoff(wake, token: token)
        }
        return (result.launch, result.readyURL, token)
    }
    private func prepareOwnedBrowserAgentHandoff(_ wake: LocalAppAgentWakeRequest, token: UUID) async throws -> (launch: LocalAppLaunch, readyURL: URL) {
        guard let app = record(wake.appID), app.currentRevision == wake.revision else {
            await agentBackground.cancel(wake, client: client, reason: staleWakeCancellationReason(wake))
            clearBrowserAgentWake(wake, token: token)
            throw LocalAppServiceFailure("STALE_AGENT_WAKE", "The app version changed before this Agent request could start.")
        }
        let release = try await release(wake.appID, revision: wake.revision)
        guard (release.manifest.capabilities ?? []).contains("agent") else {
            await agentBackground.cancel(wake, client: client, reason: .acquireFailure)
            clearBrowserAgentWake(wake, token: token)
            throw LocalAppServiceFailure("AGENT_NOT_DECLARED", "This app version does not declare Agent support.")
        }
        // A retained SwiftUI view can outlive the gateway run that supplied
        // its original launch URL. Resolve the serving URL again before
        // reserving background time; `start` returns the current run when
        // the same app revision is already running.
        let launch: LocalAppLaunch
        do {
            launch = try await LocalAppAgentHandoffLaunchResolver.resolve(wake) {
                try await self.openExternalBrowser(wake.appID)
            }
        } catch {
            await cancelPreparationIfNeeded(wake, token: token, reason: .acquireFailure)
            clearBrowserAgentWake(wake, token: token)
            throw error
        }
        guard browserAgentWake == wake, browserAgentWakeToken == token else {
            await cancelPreparationIfNeeded(wake, token: token, reason: staleWakeCancellationReason(wake))
            throw LocalAppServiceFailure("STALE_AGENT_WAKE", "This browser Agent request was replaced before it could start.")
        }
        let readyURL = try await agentBackground.acquireAndReserve(
            wake,
            appName: app.name,
            launch: launch,
            client: client
        )
        guard browserAgentWake == wake, browserAgentWakeToken == token,
              record(wake.appID)?.currentRevision == wake.revision else {
            await cancelPreparationIfNeeded(wake, token: token, reason: staleWakeCancellationReason(wake))
            throw LocalAppServiceFailure("STALE_AGENT_WAKE", "The app version or browser request changed before the handoff completed.")
        }
        return (launch, readyURL)
    }
    private func cancelPreparationIfNeeded(
        _ wake: LocalAppAgentWakeRequest,
        token: UUID,
        reason: LocalAppAgentCancellationReason
    ) async {
        guard LocalAppAgentHandoffCancellationPolicy.shouldCancelPreparation(
            wake: wake, token: token, currentWake: browserAgentWake,
            currentToken: browserAgentWakeToken, currentRevision: record(wake.appID)?.currentRevision
        ) else { return }
        await agentBackground.cancel(wake, client: client, reason: reason)
    }
    private func staleWakeCancellationReason(_ wake: LocalAppAgentWakeRequest) -> LocalAppAgentCancellationReason {
        LocalAppAgentHandoffCancellationPolicy.staleReason(
            wake: wake,
            currentWake: browserAgentWake,
            currentRevision: record(wake.appID)?.currentRevision
        )
    }
    func cancelBrowserAgentHandoff(
        _ wake: LocalAppAgentWakeRequest,
        token: UUID,
        reason: LocalAppAgentCancellationReason = .explicit
    ) async {
        guard detachBrowserAgentWake(wake, token: token) else { return }
        await agentBackground.cancel(wake, client: client, reason: reason)
        handoffPreparations.finish(wake, token: token)
    }
    func finishBrowserAgentHandoff(_ wake: LocalAppAgentWakeRequest, token: UUID, accepted: Bool) async -> Bool {
        guard browserAgentWake == wake, browserAgentWakeToken == token,
              handoffPreparations.isReady(wake, token: token) else { return false }
        if !accepted {
            guard detachBrowserAgentWake(wake, token: token) else { return false }
            await agentBackground.cancel(wake, client: client, reason: .acquireFailure)
            handoffPreparations.finish(wake, token: token)
            return latestBrowserAgentWakeToken == token
        }
        guard browserAgentWake == wake, browserAgentWakeToken == token else { return false }
        clearBrowserAgentWake(wake, token: token)
        return true
    }
    func openExternalBrowser(_ id: String) async throws -> LocalAppLaunch {
        guard let app = record(id) else { throw LocalAppServiceFailure("APP_NOT_FOUND", "The local app was not found.") }
        let release = try await release(id, revision: app.currentRevision)
        // Opening a page can be a read-only recovery of an existing Agent
        // run. Refreshing the model configuration here replaces the executor
        // and stops its tasks. New work still checks current configuration at
        // the native admission boundary (for example, wake.reserve).
        if (release.manifest.capabilities ?? []).contains("agent") { try await LocalAppAgentModelHost.shared.ensureAvailable() }
        let previousRun = runs[id]
        let currentRun = try await launch(id)
        if let previousRun, previousRun.phase == "running",
           previousRun.runId != currentRun.run.runId
            || previousRun.revision != currentRun.run.revision {
            let reason: LocalAppAgentCancellationReason = previousRun.revision != currentRun.run.revision
                ? .releaseChanged : .explicit
            await cancelAgentApp(id, reason: reason)
        }
        return currentRun
    }

    init(client: LocalAppServiceClient = LocalAppServiceClient()) { self.client = client }
    func initialize() async { await refresh(); initialized = true }
    func refresh() async {
        do {
            let previousRuns = runs
            let previousAppIDs = Set(records.map(\.appId))
            let rows: [UnifiedAppRecord] = try await client.request(["op": .string("list")])
            records = rows.filter { !$0.removed }.sorted { $0.updatedAtMs > $1.updatedAtMs }
            let currentAppIDs = Set(records.map(\.appId))
            for removedAppID in previousAppIDs.subtracting(currentAppIDs) {
                await cancelAgentApp(removedAppID)
            }
            runs = runs.filter { key, _ in records.contains(where: { $0.appId == key }) }
            for record in records {
                let status: LocalAppRuntimeStatus = try await client.request(["op": .string("status"), "appId": .string(record.appId)])
                if let current = status.run {
                    if current.phase == "running" {
                        await agentBackground.cancelOutdated(
                            appID: record.appId,
                            keepingRevision: current.revision,
                            client: client
                        )
                        if let previous = previousRuns[record.appId],
                           previous.phase == "running",
                           (previous.runId != current.runId || previous.revision != current.revision) {
                            let reason: LocalAppAgentCancellationReason = previous.revision != current.revision
                                ? .releaseChanged : .explicit
                            await cancelAgentApp(record.appId, reason: reason)
                        }
                    }
                    runs[record.appId] = current
                }
                else {
                    let history: [UnifiedAppRun] = try await client.request(["op": .string("runs"), "appId": .string(record.appId)])
                    runs[record.appId] = history.max(by: { $0.startedAtMs < $1.startedAtMs })
                    if previousRuns[record.appId]?.phase == "running" {
                        await cancelAgentApp(record.appId)
                    }
                }
            }
            error = nil
        } catch { self.error = error.localizedDescription }
    }
    func runIsCurrent(appID: String, runID: String) async throws -> Bool {
        // A failed/newer preparation is history, not the serving owner. The
        // previous release keeps its page and device capabilities while it runs.
        let status: LocalAppRuntimeStatus = try await client.request(["op": .string("status"), "appId": .string(appID)])
        guard let current = status.run else { runs.removeValue(forKey: appID); return false }
        runs[appID] = current
        return current.runId == runID && current.phase == "running"
    }
    func record(_ id: String) -> UnifiedAppRecord? { records.first { $0.appId == id } }
    func releases(_ id: String) async throws -> [UnifiedAppRelease] {
        try await client.request(["op": .string("releases"), "appId": .string(id)])
    }
    func release(_ id: String, revision: UInt64) async throws -> UnifiedAppRelease {
        try await client.request(["op": .string("getRelease"), "appId": .string(id), "revision": .number(Double(revision))])
    }
    func launch(_ id: String) async throws -> LocalAppLaunch {
        guard busy.insert(id).inserted else { throw LocalAppServiceFailure("APP_BUSY", "An operation is already in progress.") }
        defer { busy.remove(id) }
        let result: LocalAppLaunch = try await client.request(["op": .string("start"), "appId": .string(id)])
        runs[id] = result.run
        return result
    }
    func stop(_ id: String) async throws {
        let _: LocalAppJSON = try await client.request(["op": .string("stop"), "appId": .string(id)])
        await cancelAgentApp(id)
        await refresh()
    }
    func action(_ operation: String, id: String, extra: [String: LocalAppJSON] = [:]) async throws {
        guard busy.insert(id).inserted else { throw LocalAppServiceFailure("APP_BUSY", "An operation is already in progress.") }
        defer { busy.remove(id) }
        var request = extra; request["op"] = .string(operation); request["appId"] = .string(id)
        let _: LocalAppJSON = try await client.request(request)
        if ["stop", "remove", "revoke"].contains(operation) {
            await cancelAgentApp(id)
        }
        if operation == "remove" { devicePermissions.revoke(appID: id) }
        await refresh()
    }
    func grant(_ id: String, digest: String) async throws {
        try await action("grant", id: id, extra: ["digest": .string(digest)])
    }
    func logs(_ id: String, runID: String) async throws -> String {
        let value: LocalAppJSON = try await client.request(["op": .string("logs"), "appId": .string(id), "runId": .string(runID)])
        if let text = value.text { return text }
        let encoder = JSONEncoder(); encoder.outputFormatting = [.prettyPrinted, .sortedKeys]
        return String(decoding: try encoder.encode(value), as: UTF8.self)
    }
    func layout(_ id: String) async throws -> LocalAppLayout {
        try await client.request(["op": .string("layout"), "appId": .string(id)])
    }
    func prepareAuthoring(_ id: String, sessions: [String: LocalAppAuthoringSessionBinding],
                          mode: SessionMode) async throws -> LocalAppAuthoringResolution {
        try LocalAppDevelopmentAdmission.requireCode(mode)
        guard let app = record(id), let authoring = app.authoring else {
            throw LocalAppServiceFailure("SOURCE_MISSING", "Choose or import an editable source copy before continuing development.")
        }
        let host = try Self.hostSourceDirectory(authoring.sourceDirectory)
        var directory: ObjCBool = false
        let exists = FileManager.default.fileExists(atPath: host.path, isDirectory: &directory) && directory.boolValue
        return try await client.request([
            "op": .string("prepareAuthoring"),
            "appId": .string(id),
            "sourceExists": .bool(exists),
            "sessionBindings": .object(try sessions.mapValues { try LocalAppJSON.encode($0) })
        ], purpose: .development(mode))
    }
    func createAuthoring(name: String, requestID: String, mode: SessionMode) async throws -> LocalAppCreatedAuthoringSource {
        try LocalAppDevelopmentAdmission.requireCode(mode)
        guard UUID(uuidString: requestID) != nil else {
            throw LocalAppServiceFailure("INVALID_AUTHORING", "The app creation request is incomplete.")
        }
        let result = try await client.agentHost([
            "op": .string("createAuthoring"), "name": .string(name),
            "requestId": .string(canonicalSessionID(requestID))
        ], as: LocalAppCreatedAuthoringSource.self, authoringMode: mode)
        guard canonicalSessionID(result.requestId) == canonicalSessionID(requestID) else {
            throw LocalAppServiceFailure("AUTHORING_CHANGED", "The app service returned a different creation request.")
        }
        try Self.validateAuthoringSource(result)
        return result
    }

    func prepareAuthoringSource(sourceID: String, sourceDirectory: String,
                                mode: SessionMode) async throws -> LocalAppVerifiedAuthoringSource {
        try LocalAppDevelopmentAdmission.requireCode(mode)
        guard UUID(uuidString: sourceID) != nil else {
            throw LocalAppServiceFailure("INVALID_AUTHORING", "The app source receipt is incomplete.")
        }
        let result = try await client.agentHost([
            "op": .string("prepareAuthoringSource"), "sourceId": .string(canonicalSessionID(sourceID)),
            "sourceDirectory": .string(sourceDirectory)
        ], as: LocalAppVerifiedAuthoringSource.self, authoringMode: mode)
        guard canonicalSessionID(result.sourceId) == canonicalSessionID(sourceID),
              result.sourceDirectory == sourceDirectory else {
            throw LocalAppServiceFailure("AUTHORING_CHANGED", "The app source receipt does not match this project.")
        }
        try Self.validateAuthoringSource(sourceID: result.sourceId, sourceDirectory: result.sourceDirectory, sourcePath: result.sourcePath)
        return result
    }

    func importMarketRemix(_ input: LocalAppMarketRemixInput, archivePath: String,
                           mode: SessionMode) async throws -> LocalAppMarketRemixResult {
        let request = try LocalAppAuthoringHostRequest.importRemix(input, archivePath: archivePath, mode: mode)
        let result = try await client.agentHost(request, as: LocalAppMarketRemixResult.self, authoringMode: mode)
        guard result.requestId == input.requestId, result.release.appId == result.record.appId,
              !result.record.appId.isEmpty else {
            throw LocalAppServiceFailure("AUTHORING_CHANGED", "The market source receipt does not match this request.")
        }
        try Self.validateAuthoringSource(sourceID: result.sourceId, sourceDirectory: result.sourceDirectory,
                                         sourcePath: result.sourcePath)
        return result
    }

    static func validateAuthoringSource(_ result: LocalAppCreatedAuthoringSource) throws {
        guard UUID(uuidString: result.requestId) != nil else {
            throw LocalAppServiceFailure("INVALID_AUTHORING", "The app service returned an invalid source request.")
        }
        try validateAuthoringSource(sourceID: result.sourceId, sourceDirectory: result.sourceDirectory, sourcePath: result.sourcePath)
    }

    static func validateAuthoringSource(sourceID: String, sourceDirectory: String, sourcePath: String) throws {
        let expected = try Self.hostSourceDirectory(sourceDirectory).standardizedFileURL.path
        guard UUID(uuidString: sourceID) != nil,
              expected == URL(fileURLWithPath: sourcePath, isDirectory: true).resolvingSymlinksInPath().standardizedFileURL.path,
              sourceDirectory.hasPrefix(MobileUserStorage.directories().appsGuestPath + "/") else {
            throw LocalAppServiceFailure("INVALID_AUTHORING", "The app service returned an invalid source directory.")
        }
    }

    func exactAuthoringBinding(sourceDirectory: String, projectID: String, sessionID: String,
                               mode: SessionMode, expectedSourceID: String? = nil) async throws -> LocalAppAuthoring {
        let request = try LocalAppAuthoringHostRequest.getGrant(sourceDirectory: sourceDirectory, projectID: projectID,
                                                                sessionID: sessionID, mode: mode)
        let receipt = try await client.agentHost(request, as: LocalAppAuthoringReceipt.self, authoringMode: mode)
        guard receipt.sessionMode == mode, receipt.matchesSourceID(expectedSourceID) else {
            throw LocalAppServiceFailure("AUTHORING_CHANGED", "The saved app binding has a different mode or source.")
        }
        return receipt.authoring
    }
    private func cancelAgentApp(
        _ appID: String,
        reason: LocalAppAgentCancellationReason = .explicit
    ) async {
        let wake = browserAgentWake
        let token = browserAgentWakeToken
        if let wake = browserAgentWake, wake.appID == appID {
            browserAgentWake = nil
            browserAgentWakeToken = nil
        }
        await agentBackground.cancel(appID: appID, client: client, reason: reason)
        if let wake, let token, wake.appID == appID { handoffPreparations.finish(wake, token: token) }
    }
    /// Native-only immutable owner receipt for a new or published app. This
    /// trusted channel is never exposed to the external-browser gateway.
    func registerAuthoring(_ authoring: LocalAppAuthoring, sourceID: String, requestID: String,
                           mode: SessionMode, setCurrent: Bool) async throws {
        let request = try LocalAppAuthoringHostRequest.register(authoring, sourceID: sourceID,
                                                                 requestID: requestID, mode: mode, setCurrent: setCurrent)
        let receipt = try await client.agentHost(request, as: LocalAppAuthoringReceipt.self, authoringMode: mode)
        guard receipt.sessionMode == mode, receipt.matchesSourceID(sourceID),
              case .mobileProject(let projectID)? = authoring.projectRef,
              let sessionID = authoring.authoringSessionId,
              LocalAppAuthoringOwner(sourceDirectory: authoring.sourceDirectory, projectID: projectID,
                                     sessionID: sessionID, appID: authoring.appId).matches(receipt.authoring) else {
            throw LocalAppServiceFailure("AUTHORING_CHANGED", "The registered app binding has a different owner.")
        }
    }
    static func hostSourceDirectory(_ source: String) throws -> URL {
        let paths = MobileUserStorage.directories()
        let guestHome = paths.guestHome
        guard source == guestHome || source.hasPrefix(guestHome + "/") else {
            throw LocalAppServiceFailure("SOURCE_OUTSIDE_USER_HOME", "Select a source folder in the local Linux user home.")
        }
        let relative = String(source.dropFirst(guestHome.count)).trimmingCharacters(in: CharacterSet(charactersIn: "/"))
        let base = URL(fileURLWithPath: paths.home, isDirectory: true).resolvingSymlinksInPath()
        let url = base.appendingPathComponent(relative, isDirectory: true).resolvingSymlinksInPath()
        guard url.path == base.path || url.path.hasPrefix(base.path + "/") else { throw LocalAppServiceFailure("INVALID_SOURCE", "Source path escaped the local user home.") }
        return url
    }
    func bindAuthoringSource(requestID: String, sourceID: String, sourceDirectory: String,
                             appID: String, mode: SessionMode) async throws -> LocalAppCreatedAuthoringSource {
        try LocalAppDevelopmentAdmission.requireCode(mode)
        guard UUID(uuidString: requestID) != nil, UUID(uuidString: sourceID) != nil, !appID.isEmpty else {
            throw LocalAppServiceFailure("INVALID_AUTHORING", "The app source binding request is incomplete.")
        }
        let bound = try await client.agentHost(["op": .string("bindAuthoringSource"),
            "requestId": .string(canonicalSessionID(requestID)), "sourceId": .string(canonicalSessionID(sourceID)),
            "sourceDirectory": .string(sourceDirectory), "targetAppId": .string(appID)], as: LocalAppBoundAuthoringSource.self,
            authoringMode: mode)
        guard canonicalSessionID(bound.requestId) == canonicalSessionID(requestID),
              canonicalSessionID(bound.sourceId) == canonicalSessionID(sourceID),
              bound.sourceDirectory == sourceDirectory, bound.targetAppId == appID else {
            throw LocalAppServiceFailure("AUTHORING_CHANGED", "The app source binding changed.")
        }
        let result = LocalAppCreatedAuthoringSource(sourceId: bound.sourceId, requestId: bound.requestId,
            sourceDirectory: bound.sourceDirectory, sourcePath: bound.sourcePath)
        try Self.validateAuthoringSource(result)
        return result
    }

    func restoreAuthoringSource(appID: String, revision: Int, requestID: String,
                                mode: SessionMode) async throws -> LocalAppCreatedAuthoringSource {
        try LocalAppDevelopmentAdmission.requireCode(mode)
        guard UUID(uuidString: requestID) != nil, !appID.isEmpty, revision > 0 else {
            throw LocalAppServiceFailure("INVALID_AUTHORING", "The app source restore request is incomplete.")
        }
        let request: [String: LocalAppJSON] = ["op": .string("restoreAuthoringSource"),
            "appId": .string(appID), "requestId": .string(canonicalSessionID(requestID)), "revision": .number(Double(revision))]
        let result = try await client.agentHost(request, as: LocalAppRestoredAuthoringSource.self, authoringMode: mode)
        try Self.validateAuthoringSource(LocalAppCreatedAuthoringSource(sourceId: result.sourceId, requestId: result.requestId,
            sourceDirectory: result.sourceDirectory, sourcePath: result.sourcePath))
        guard result.record.appId == appID, canonicalSessionID(result.requestId) == canonicalSessionID(requestID) else {
            throw LocalAppServiceFailure("AUTHORING_CHANGED", "The restored source does not match this app.")
        }
        return LocalAppCreatedAuthoringSource(sourceId: result.sourceId, requestId: result.requestId,
            sourceDirectory: result.sourceDirectory, sourcePath: result.sourcePath)
    }

}

@MainActor
enum LocalApps {
    static let library = UnifiedLocalApps()
    static let info = InstalledAppInfoStore()
    static let market = UnifiedAppMarket(library: library)
    static func initializeOnce() async { if !library.initialized { await library.initialize() } }
}

@MainActor
@Observable
final class InstalledAppInfoStore {
    private(set) var apps: [String: InstalledAppInfo] = [:]
    private let file: URL?
    init(file: URL? = nil) {
        self.file = file ?? FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask).first?.appendingPathComponent("local-app-info-v2.json")
        if let file = self.file, let bytes = try? Data(contentsOf: file), let rows = try? JSONDecoder().decode([InstalledAppInfo].self, from: bytes) { apps = Dictionary(rows.map { ($0.appId, $0) }, uniquingKeysWith: { _, last in last }) }
    }
    func markOpened(_ record: UnifiedAppRecord) {
        apps[record.appId] = InstalledAppInfo(appId: record.appId, name: record.name, summary: record.summary, lastOpenedAt: Date())
        persist()
    }
    func forget(_ id: String) { apps.removeValue(forKey: id); persist() }
    private func persist() {
        guard let file, let bytes = try? JSONEncoder().encode(Array(apps.values)) else { return }
        try? bytes.write(to: file, options: .atomic)
    }
}
