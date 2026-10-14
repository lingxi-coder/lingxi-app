import Combine
import Foundation

/// The app market's anonymous API. The market's web front end proxies `/api/*`
/// to its backend; while that backend is not deployed every call fails with the
/// server's own error, which the pages show as is.
enum LocalAppMarketConfig {
    static let apiRoot = URL(string: "https://local-app-market.luolingfeng-lingxi.workers.dev/api/v1")!
}

/// The process's one Local App Market (its README asks for one long-lived
/// instance per root) and what the pages remember about installed apps.
@MainActor
enum LocalApps {
    /// nil only when the market cannot be created at all (no Application
    /// Support directory); the 应用 page then says the market is unavailable.
    static let market: LocalAppMarket? = try? LocalAppMarket(baseURL: LocalAppMarketConfig.apiRoot)
    static let info = InstalledAppInfoStore()
    private static var initialized = false

    /// Restores the installed registry and finishes interrupted uninstalls,
    /// once, before anything opens an app.
    static func initializeOnce() async {
        guard !initialized, let market else { return }
        initialized = true
        await market.initialize()
    }
}

extension AppOperation {
    init?(_ phase: LocalAppMarket.Phase) {
        switch phase {
        case .downloading(let progress): self = .downloading(progress)
        case .verifying, .committing: self = .installing
        case .removing: self = .removing
        case .failed(let message): self = .failed(message)
        case .idle: return nil
        }
    }
}

extension LocalAppMarket {
    /// The installed apps as the pages show them, with what [info] remembers.
    func myApps(info: [String: InstalledAppInfo]) -> [MyApp] {
        AppsModel.myApps(
            installedIDs: installed.filter { !$0.removing }.map(\.appId),
            info: info,
            updatable: Set(updates.filter { $0.status == .update_available }.map(\.appId)),
            operations: operations.compactMapValues(AppOperation.init)
        )
    }

    func action(for appId: String, current: MarketVersion?) -> AppAction {
        let update = updates.first { $0.appId == appId && $0.status == .update_available }
        return AppsModel.action(
            currentVersionID: current?.id,
            currentSequence: current?.sequence,
            installedSequence: installed.first { $0.appId == appId && !$0.removing }?.sequence,
            updateVersionID: update?.currentVersionId,
            operation: operations[appId].flatMap(AppOperation.init)
        )
    }
}

/// [InstalledAppInfo] per app id, kept in one JSON file in Application Support.
@MainActor
final class InstalledAppInfoStore: ObservableObject {
    @Published private(set) var apps: [String: InstalledAppInfo] = [:]
    private let file: URL?

    init(file: URL? = InstalledAppInfoStore.defaultFile()) {
        self.file = file
        if let file, let data = try? Data(contentsOf: file),
           let stored = try? JSONDecoder().decode([InstalledAppInfo].self, from: data) {
            apps = Dictionary(stored.map { ($0.appId, $0) }, uniquingKeysWith: { _, last in last })
        }
    }

    nonisolated static func defaultFile() -> URL? {
        try? FileManager.default
            .url(for: .applicationSupportDirectory, in: .userDomainMask, appropriateFor: nil, create: true)
            .appendingPathComponent("local-app-info.json")
    }

    /// Keeps the market's name, summary and category of an app being installed or already installed.
    func remember(_ app: MarketApp) {
        var known = apps[app.id] ?? InstalledAppInfo(appId: app.id, name: app.name)
        known.name = app.name
        known.summary = app.summary
        known.category = app.category
        apps[app.id] = known
        save()
    }

    func markOpened(_ appId: String, at date: Date = Date()) {
        var known = apps[appId] ?? InstalledAppInfo(appId: appId, name: appId)
        known.lastOpenedAt = date
        apps[appId] = known
        save()
    }

    func forget(_ appId: String) {
        apps.removeValue(forKey: appId)
        save()
    }

    private func save() {
        guard let file else { return }
        let items = apps.values.sorted { $0.appId < $1.appId }
        guard let data = try? JSONEncoder().encode(items) else { return }
        try? data.write(to: file, options: .atomic)
    }
}
