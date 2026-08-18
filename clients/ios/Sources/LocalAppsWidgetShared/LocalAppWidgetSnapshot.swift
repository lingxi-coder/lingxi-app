import Foundation
#if canImport(WidgetKit)
    import WidgetKit
#endif

struct LocalAppWidgetSnapshot: Codable, Equatable, Sendable {
    struct App: Codable, Equatable, Identifiable, Sendable {
        let id: String
        let name: String
        let brief: String
        let workflow: String
        let runtimeState: String
        let updatedAtMs: Int64
    }

    let version: Int
    let apps: [App]

    static let currentVersion = 1
    static let empty = LocalAppWidgetSnapshot(version: currentVersion, apps: [])
}

enum LocalAppWidgetSnapshotStore {
    static let appGroupIdentifier = "group.com.lingxi.code"
    static let snapshotFileName = "local_apps_widget_snapshot_v1.json"
    static let widgetKind = "LocalAppWidget"

    static func snapshotURL(fileManager: FileManager = .default) -> URL? {
        fileManager
            .containerURL(
                forSecurityApplicationGroupIdentifier: appGroupIdentifier
            )?
            .appendingPathComponent(snapshotFileName, isDirectory: false)
    }

    static func load(
        from url: URL? = snapshotURL()
    ) -> LocalAppWidgetSnapshot {
        guard
            let url,
            let data = try? Data(contentsOf: url),
            let snapshot = try? JSONDecoder().decode(LocalAppWidgetSnapshot.self, from: data),
            snapshot.version == LocalAppWidgetSnapshot.currentVersion
        else {
            return .empty
        }
        var seenIDs = Set<String>()
        let apps = snapshot.apps.filter { app in
            isValidAppID(app.id) && seenIDs.insert(app.id).inserted
        }
        return LocalAppWidgetSnapshot(version: snapshot.version, apps: apps)
    }

    static func read() -> LocalAppWidgetSnapshot {
        load()
    }

    enum SnapshotError: Error {
        case containerUnavailable
    }

    static func write(
        _ snapshot: LocalAppWidgetSnapshot,
        to url: URL? = snapshotURL()
    ) throws {
        guard let url else { throw SnapshotError.containerUnavailable }
        let data = try JSONEncoder().encode(snapshot)
        try data.write(to: url, options: .atomic)
    }

    @discardableResult
    static func publish(_ snapshot: LocalAppWidgetSnapshot) -> Error? {
        do {
            try write(snapshot)
            #if canImport(WidgetKit)
                WidgetCenter.shared.reloadTimelines(ofKind: widgetKind)
            #endif
            return nil
        } catch {
            return error
        }
    }

    static func isValidAppID(_ value: String) -> Bool {
        let pattern = "^[a-z0-9][a-z0-9-]{0,63}$"
        return value.range(of: pattern, options: .regularExpression) != nil
    }

    static func makeOpenURL(
        appID: String,
        destination: String = "preview",
        autostart: Bool = true,
        source: String = "widget"
    ) -> URL? {
        guard isValidAppID(appID) else { return nil }
        var components = URLComponents()
        components.scheme = "lingxi"
        components.host = "open_local_app"
        components.queryItems = [
            URLQueryItem(name: "appId", value: appID),
            URLQueryItem(name: "destination", value: destination),
            URLQueryItem(name: "autostart", value: autostart ? "1" : "0"),
            URLQueryItem(name: "source", value: source),
        ]
        return components.url
    }
}
