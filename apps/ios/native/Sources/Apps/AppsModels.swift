import Foundation

/// What the pages remember about an installed app. The market's registry keeps
/// only ids and versions, so without this an app opened offline would have no
/// name, and nothing would know which apps were used last.
struct InstalledAppInfo: Codable, Equatable, Sendable {
    let appId: String
    var name: String
    var summary: String = ""
    var category: String = ""
    var lastOpenedAt: Date?
}

/// A market operation as the pages show it.
enum AppOperation: Equatable, Sendable {
    case downloading(Double)
    case installing
    case removing
    case failed(String)

    var isActive: Bool {
        if case .failed = self { return false }
        return true
    }
}

/// One installed app as the 应用 page and the sidebar show it.
struct MyApp: Identifiable, Equatable, Sendable {
    var id: String { appId }
    let appId: String
    let name: String
    let category: String
    let lastOpenedAt: Date?
    let updateAvailable: Bool
    /// Only a download, install or uninstall that is still running.
    let activeOperation: AppOperation?
}

/// What an app's main button does right now.
enum AppAction: Equatable, Sendable {
    case get
    case open
    case update(versionID: String)
    /// Tapping cancels the download.
    case downloading(Double)
    case installing
    case removing
    /// Not installed and the market has no version to install.
    case unavailable
}

enum AppsModel {
    /// The installed apps, most recently opened first, then by name. An app the
    /// pages know nothing about yet is shown by its id.
    static func myApps(
        installedIDs: [String],
        info: [String: InstalledAppInfo],
        updatable: Set<String>,
        operations: [String: AppOperation]
    ) -> [MyApp] {
        installedIDs.map { id in
            let known = info[id]
            let name = known.map(\.name).flatMap { $0.isEmpty ? nil : $0 } ?? id
            return MyApp(
                appId: id,
                name: name,
                category: known?.category ?? "",
                lastOpenedAt: known?.lastOpenedAt,
                updateAvailable: updatable.contains(id),
                activeOperation: operations[id].flatMap { $0.isActive ? $0 : nil }
            )
        }
        .sorted { a, b in
            if a.lastOpenedAt != b.lastOpenedAt {
                return (a.lastOpenedAt ?? .distantPast) > (b.lastOpenedAt ?? .distantPast)
            }
            let byName = a.name.localizedCaseInsensitiveCompare(b.name)
            if byName != .orderedSame { return byName == .orderedAscending }
            return a.appId < b.appId
        }
    }

    /// The sidebar's 应用 row: the last few apps that were actually opened.
    static func recentApps(_ apps: [MyApp], limit: Int = 3) -> [MyApp] {
        Array(
            apps.filter { $0.lastOpenedAt != nil }
                .sorted { ($0.lastOpenedAt ?? .distantPast) > ($1.lastOpenedAt ?? .distantPast) }
                .prefix(limit)
        )
    }

    static func matchingApps(_ apps: [MyApp], query: String) -> [MyApp] {
        let needle = query.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !needle.isEmpty else { return [] }
        return apps.filter { $0.name.localizedCaseInsensitiveContains(needle) }
    }

    /// The single character of an app's cover, like the market's own covers.
    static func initial(_ name: String) -> String {
        let trimmed = name.trimmingCharacters(in: .whitespacesAndNewlines)
        guard let first = trimmed.first else { return "?" }
        return String(first).uppercased()
    }

    /// The main button for one app. A newer version counts when the update check
    /// said so or when the listing's current version is past the installed one.
    static func action(
        currentVersionID: String?,
        currentSequence: Int?,
        installedSequence: Int?,
        updateVersionID: String?,
        operation: AppOperation?
    ) -> AppAction {
        switch operation {
        case .downloading(let progress): return .downloading(progress)
        case .installing: return .installing
        case .removing: return .removing
        case .failed, nil: break
        }
        guard let installedSequence else { return currentVersionID == nil ? .unavailable : .get }
        if let updateVersionID { return .update(versionID: updateVersionID) }
        if let currentVersionID, let currentSequence, currentSequence > installedSequence {
            return .update(versionID: currentVersionID)
        }
        return .open
    }
}
