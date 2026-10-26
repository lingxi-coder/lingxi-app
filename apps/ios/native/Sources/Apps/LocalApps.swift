import Foundation
import Observation

/// One service for the local Linux user, independent of Firebase and chat engines.
actor LocalAppServiceClient {
    private struct TargetCompatibility: Decodable, Sendable { let compatible: Bool }
    private var handle: MobileLocalAppServiceHandle?
    private var linuxAttached = false

    func request<T: Decodable & Sendable>(_ fields: [String: LocalAppJSON], as type: T.Type = T.self) async throws -> T {
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
                let metadata = LXISHRuntimeBundleMetadata.current()
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
    func agentHost(_ request: String) async throws -> LocalAppJSON {
        if LocalAppAgentHostPolicy.requiresModelHost(request) {
            try await LocalAppAgentModelHost.shared.ensure()
        }
        let _: LocalAppJSON = try await self.request(["op": .string("list")])
        guard let handle else { throw LocalAppServiceFailure("SERVICE_UNAVAILABLE", "Local apps are unavailable.") }
        let response = try await handle.agentHostJson(request: request)
        return try JSONDecoder().decode(LocalAppServiceEnvelope<LocalAppJSON>.self, from: Data(response.utf8)).result()
    }
    func agentHost<T: Decodable & Sendable>(_ fields: [String: LocalAppJSON], as type: T.Type) async throws -> T {
        let value = try await agentHost(String(decoding: JSONEncoder().encode(fields), as: UTF8.self))
        return try JSONDecoder().decode(T.self, from: JSONEncoder().encode(value))
    }
    private func send<T: Decodable & Sendable>(_ service: MobileLocalAppServiceHandle, _ fields: [String: LocalAppJSON]) async throws -> T {
        let data = try JSONEncoder().encode(fields)
        let response = try await service.requestJson(request: String(decoding: data, as: UTF8.self))
        return try JSONDecoder().decode(LocalAppServiceEnvelope<T>.self, from: Data(response.utf8)).result()
    }
}

enum LocalAppAgentHostPolicy {
    static func requiresModelHost(_ request: String) -> Bool {
        guard let data = request.data(using: .utf8),
              let fields = try? JSONSerialization.jsonObject(with: data) as? [String: Any] else { return true }
        return fields["op"] as? String != "registerAuthoring"
    }
}

@MainActor
@Observable
final class UnifiedLocalApps {
    private(set) var records: [UnifiedAppRecord] = []
    private(set) var runs: [String: UnifiedAppRun] = [:]
    private(set) var legacyArchives: [LocalAppLegacyArchive] = []
    private(set) var legacyEntries: [LocalAppLegacyEntry] = []
    private(set) var busy = Set<String>()
    private(set) var error: String?
    private(set) var initialized = false
    let devicePermissions = LocalAppDevicePermissions()
    @ObservationIgnored let client: LocalAppServiceClient
    @ObservationIgnored private let agentBackground = LocalAppAgentBackgroundController()
    var browserReviewAppID: String?
    private(set) var browserAgentWake: LocalAppAgentWakeRequest?
    func consumeBrowserReview(_ id: String) -> Bool {
        guard browserReviewAppID == id else { return false }
        browserReviewAppID = nil
        return true
    }
    func queueBrowserAgentWake(appID: String, revision: UInt64, runID: String) {
        let next = LocalAppAgentWakeRequest(appID: appID, revision: revision, runID: runID)
        if let previous = browserAgentWake, previous != next {
            Task { await agentBackground.cancel(previous, client: client) }
        }
        browserAgentWake = next
    }
    func browserAgentWake(for appID: String) -> LocalAppAgentWakeRequest? {
        guard browserAgentWake?.appID == appID else { return nil }
        return browserAgentWake
    }
    func clearBrowserAgentWake(_ wake: LocalAppAgentWakeRequest) {
        guard browserAgentWake == wake else { return }
        browserAgentWake = nil
    }
    func prepareBrowserAgentHandoff(_ wake: LocalAppAgentWakeRequest) async throws -> (launch: LocalAppLaunch, readyURL: URL) {
        guard browserAgentWake == wake, let app = record(wake.appID), app.currentRevision == wake.revision else {
            await agentBackground.cancel(wake, client: client)
            clearBrowserAgentWake(wake)
            throw LocalAppServiceFailure("STALE_AGENT_WAKE", "The app version changed before this Agent request could start.")
        }
        let release = try await release(wake.appID, revision: wake.revision)
        guard (release.manifest.capabilities ?? []).contains("agent") else {
            await agentBackground.cancel(wake, client: client)
            clearBrowserAgentWake(wake)
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
            await agentBackground.cancel(wake, client: client)
            clearBrowserAgentWake(wake)
            throw error
        }
        guard browserAgentWake == wake else {
            await agentBackground.cancel(wake, client: client)
            throw LocalAppServiceFailure("STALE_AGENT_WAKE", "This browser Agent request was replaced before it could start.")
        }
        let readyURL = try await agentBackground.acquireAndReserve(
            wake,
            appName: app.name,
            launch: launch,
            client: client
        )
        guard browserAgentWake == wake,
              record(wake.appID)?.currentRevision == wake.revision else {
            await agentBackground.cancel(wake, client: client)
            throw LocalAppServiceFailure("STALE_AGENT_WAKE", "The app version or browser request changed before the handoff completed.")
        }
        return (launch, readyURL)
    }
    func cancelBrowserAgentHandoff(_ wake: LocalAppAgentWakeRequest) async {
        await agentBackground.cancel(wake, client: client)
        clearBrowserAgentWake(wake)
    }
    func openExternalBrowser(_ id: String) async throws -> LocalAppLaunch {
        guard let app = record(id) else { throw LocalAppServiceFailure("APP_NOT_FOUND", "The local app was not found.") }
        let release = try await release(id, revision: app.currentRevision)
        if (release.manifest.capabilities ?? []).contains("agent") { try await LocalAppAgentModelHost.shared.ensure() }
        return try await launch(id)
    }

    init(client: LocalAppServiceClient = LocalAppServiceClient()) { self.client = client }
    func initialize() async { await refresh(); initialized = true }
    func refresh() async {
        do {
            let rows: [UnifiedAppRecord] = try await client.request(["op": .string("list")])
            records = rows.filter { !$0.removed }.sorted { $0.updatedAtMs > $1.updatedAtMs }
            let coreArchives: [LocalAppLegacyArchive] = try await client.request(["op": .string("legacyArchives")])
            let support = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask).first
            let nativeArchives = ["LocalAppMarket", "chat-apps.json", "local-app-info.json"].compactMap { name -> LocalAppLegacyArchive? in
                guard let path = support?.appendingPathComponent(name).path, FileManager.default.fileExists(atPath: path) else { return nil }
                return LocalAppLegacyArchive(path: path, kind: "nativeArchive", status: "preservedNotMigrated")
            }
            legacyArchives = coreArchives + nativeArchives
            legacyEntries = try await client.request(["op": .string("legacyEntries")])
            runs = runs.filter { key, _ in records.contains(where: { $0.appId == key }) }
            for record in records {
                let status: LocalAppRuntimeStatus = try await client.request(["op": .string("status"), "appId": .string(record.appId)])
                if let current = status.run { runs[record.appId] = current }
                else {
                    let history: [UnifiedAppRun] = try await client.request(["op": .string("runs"), "appId": .string(record.appId)])
                    runs[record.appId] = history.max(by: { $0.startedAtMs < $1.startedAtMs })
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
        await refresh()
    }
    func action(_ operation: String, id: String, extra: [String: LocalAppJSON] = [:]) async throws {
        guard busy.insert(id).inserted else { throw LocalAppServiceFailure("APP_BUSY", "An operation is already in progress.") }
        defer { busy.remove(id) }
        var request = extra; request["op"] = .string(operation); request["appId"] = .string(id)
        let _: LocalAppJSON = try await client.request(request)
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
    func prepareAuthoring(_ id: String, sessions: [String: LocalAppAuthoringSessionBinding]) async throws -> LocalAppAuthoringResolution {
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
        ])
    }
    func bindAuthoring(_ id: String, authoring: LocalAppAuthoring) async throws {
        try await action("bindAuthoring", id: id, extra: ["authoring": try .encode(authoring)])
    }
    /// Native-only pending binding for a newly-created app. This uses the
    /// trusted App Agent host channel; it is not part of the browser API.
    func registerPendingAuthoring(_ authoring: LocalAppAuthoring) async throws {
        let request = try LocalAppAuthoringHostRequest.registerPending(authoring)
        let _: LocalAppJSON = try await client.agentHost(request, as: LocalAppJSON.self)
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
    static func newAuthoringDirectory(name: String, create: Bool = true) throws -> String {
        let clean = name.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !clean.isEmpty, clean.count <= 100 else { throw LocalAppServiceFailure("INVALID_NAME", "Enter an app name of 1–100 characters.") }
        let slug = clean.unicodeScalars.map { CharacterSet.alphanumerics.contains($0) ? String($0) : "-" }.joined()
        let guest = MobileUserStorage.directories().appsGuestPath + "/" + slug + "-" + UUID().uuidString.prefix(8).lowercased()
        if create { try FileManager.default.createDirectory(at: hostSourceDirectory(guest), withIntermediateDirectories: true) }
        return guest
    }
    func importLegacy(_ entry: LocalAppLegacyEntry) async throws -> UnifiedAppRecord {
        let guest = try Self.newAuthoringDirectory(name: entry.name, create: false)
        let result: LocalAppPublishResult = try await client.request(["op": .string("importLegacy"), "legacyId": .string(entry.legacyId), "sourcePath": .string(try Self.hostSourceDirectory(guest).path), "authoring": try .encode(LocalAppAuthoring(sourceDirectory: guest))])
        await refresh(); return result.record
    }
    func restoreAuthoring(_ app: UnifiedAppRecord) async throws -> UnifiedAppRecord {
        let guest = try Self.newAuthoringDirectory(name: app.name, create: false)
        let result: UnifiedAppRecord = try await client.request(["op": .string("restoreAuthoring"), "appId": .string(app.appId), "sourcePath": .string(try Self.hostSourceDirectory(guest).path), "authoring": try .encode(LocalAppAuthoring(sourceDirectory: guest))])
        await refresh(); return result
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
