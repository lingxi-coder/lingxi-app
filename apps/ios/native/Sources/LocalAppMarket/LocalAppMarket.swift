import Foundation
import Combine
import WebKit

@MainActor
final class LocalAppMarket: ObservableObject {
    enum Phase: Equatable { case downloading(Double), verifying, committing, removing, failed(String), idle }
    @Published private(set) var apps: [MarketApp] = []
    @Published private(set) var installed: [MarketInstallation] = []
    @Published private(set) var updates: [MarketUpdate] = []
    @Published private(set) var operations: [String: Phase] = [:]
    @Published private(set) var isLoading = false
    @Published private(set) var error: String?
    @Published private(set) var runtimeErrors: [String: String] = [:]
    let runtimeCapability = "persistent_isolated_webkit"
    @Published private(set) var hasNextPage = false
    let api: MarketAPI
    let repository: MarketRepository
    let runtime: MarketRuntime
    private var query = ""
    private var cursor: String?
    private var epoch = UUID()
    private var jobs: [String: Task<Void, Never>] = [:]
    private var committing = Set<String>()
    init(baseURL: URL, root: URL? = nil, sessionConfiguration: URLSessionConfiguration = .ephemeral) throws {
        api = try MarketAPI(baseURL: baseURL, configuration: sessionConfiguration)
        let directory = try root ?? FileManager.default.url(for: .applicationSupportDirectory, in: .userDomainMask, appropriateFor: nil, create: true).appendingPathComponent("LocalAppMarket")
        repository = try MarketRepository(root: directory)
        runtime = MarketRuntime(repository: repository)
    }
    func initialize() async {
        installed = await repository.installations()
        for record in installed where record.removing {
            var acquired = false
            do { try await repository.acquire(record.appId); acquired = true; try await runtime.clear(record); try await repository.finishRemoval(record); await repository.release(record.appId) }
            catch { if acquired { await repository.release(record.appId) }; operations[record.appId] = .failed(error.localizedDescription) }
        }
        installed = await repository.installations()
    }
    func search(_ query: String) async {
        self.query = query.trimmingCharacters(in: .whitespacesAndNewlines)
        epoch = UUID(); cursor = nil; apps = []; hasNextPage = false
        isLoading = false
        await loadNextPage()
    }
    func loadNextPage() async {
        guard !isLoading, apps.isEmpty || hasNextPage else { return }
        let requestEpoch = epoch
        isLoading = true; error = nil
        defer { if epoch == requestEpoch { isLoading = false } }
        do {
            let page = try await api.catalog(query: query, cursor: cursor)
            guard epoch == requestEpoch else { return }
            var seen = Set(apps.map(\.id))
            apps += page.data.filter { $0.platforms.contains("ios") && seen.insert($0.id).inserted }
            cursor = page.pagination.nextCursor; hasNextPage = cursor != nil
        } catch { if epoch == requestEpoch { self.error = error.localizedDescription } }
    }
    func detail(_ id: String) async throws -> MarketApp { try await api.detail(id) }
    func versions(_ id: String, cursor: String? = nil) async throws -> MarketPage<MarketVersion> { try await api.versions(id, cursor: cursor) }
    func checkUpdates() async {
        do { updates = try await api.updates(installed.filter { !$0.removing }); error = nil }
        catch { self.error = error.localizedDescription }
    }
    func install(appId: String, versionId: String) {
        guard jobs[appId] == nil else { return }
        jobs[appId] = Task {
            var acquired = false
            var staging: URL?
            defer { jobs.removeValue(forKey: appId); committing.remove(appId) }
            do {
                try await repository.acquire(appId); acquired = true
                let app = try await api.detail(appId)
                guard app.platforms.contains("ios") else { throw MarketFailure("PLATFORM_UNSUPPORTED") }
                let distribution = try await api.distribution(appId, versionId)
                if let old = await repository.installations().first(where: { $0.appId == appId }) {
                    guard old.source == api.baseURL, distribution.sequence > old.sequence else { throw MarketFailure("VERSION_NOT_NEWER_OR_SOURCE_MISMATCH") }
                }
                operations[appId] = .downloading(0)
                staging = try await repository.stage(distribution, api: api, progress: { [weak self] progress in
                    await self?.setProgress(appId, progress)
                }, verifying: { [weak self] in await self?.setVerifying(appId) })
                try Task.checkCancellation()
                committing.insert(appId); operations[appId] = .committing
                await repository.blockOpen(appId)
                await runtime.close(appId)
                _ = try await repository.commit(distribution, staged: staging!, source: api.baseURL)
                operations[appId] = .idle
                installed = await repository.installations()
            } catch { operations[appId] = .failed(error.localizedDescription) }
            if let staging { await repository.discard(staging) }
            if acquired { await repository.release(appId) }
        }
    }
    private func setVerifying(_ id: String) { operations[id] = .verifying }
    private func setProgress(_ id: String, _ value: Double) { operations[id] = .downloading(value) }
    func cancel(_ appId: String) { if !committing.contains(appId) { jobs[appId]?.cancel() } }
    func update(_ appId: String) async throws {
        guard let record = installed.first(where: { $0.appId == appId }) else { throw MarketFailure("APP_NOT_INSTALLED") }
        let result = try await api.updates([record])
        guard let update = result.first, update.status == .update_available, let version = update.currentVersionId else { throw MarketFailure("NO_UPDATE_AVAILABLE") }
        install(appId: appId, versionId: version)
    }
    func uninstall(_ appId: String) async throws {
        try await repository.acquire(appId)
        operations[appId] = .removing
        do {
            let record = try await repository.beginRemoval(appId)
            await runtime.close(appId)
            try await runtime.clear(record)
            try await repository.finishRemoval(record)
            operations[appId] = .idle
            installed = await repository.installations()
            await repository.release(appId)
        } catch {
            installed = await repository.installations(); operations[appId] = .failed(error.localizedDescription)
            await repository.release(appId); throw error
        }
    }
    func open(_ appId: String) async throws -> WKWebView {
        do { let view = try await runtime.open(appId); runtimeErrors.removeValue(forKey: appId); return view }
        catch { runtimeErrors[appId] = error.localizedDescription; throw error }
    }
    func close(_ appId: String) async { await runtime.close(appId) }
    func suspend(_ appId: String) { runtime.suspend(appId) }
    func resume(_ appId: String) { runtime.resume(appId) }
}
