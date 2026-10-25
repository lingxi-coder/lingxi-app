import Foundation

/// Lightweight presentation history. Application identity comes exclusively
/// from the standalone library; marketplace and authoring are provenance.
struct InstalledAppInfo: Codable, Equatable, Sendable {
    let appId: String
    var name: String
    var summary: String = ""
    var category: String = ""
    var lastOpenedAt: Date?
}
struct MyApp: Identifiable, Equatable, Sendable {
    var id: String { appId }
    let appId: String
    let name: String
    let category: String
    let lastOpenedAt: Date?
}
enum AppsModel {
    static func myApps(_ records: [UnifiedAppRecord], info: [String: InstalledAppInfo]) -> [MyApp] {
        records.filter { !$0.removed }.map {
            MyApp(appId:$0.appId,name:$0.name,category:"",lastOpenedAt:info[$0.appId]?.lastOpenedAt)
        }.sorted {
            if $0.lastOpenedAt != $1.lastOpenedAt { return ($0.lastOpenedAt ?? .distantPast) > ($1.lastOpenedAt ?? .distantPast) }
            let order = $0.name.localizedStandardCompare($1.name)
            return order == .orderedSame ? $0.appId < $1.appId : order == .orderedAscending
        }
    }
    static func recentApps(_ apps: [MyApp], limit: Int = 3) -> [MyApp] {
        Array(apps.filter { $0.lastOpenedAt != nil }.sorted { ($0.lastOpenedAt ?? .distantPast) > ($1.lastOpenedAt ?? .distantPast) }.prefix(max(0,limit)))
    }
    static func matchingApps(_ apps: [MyApp], query: String) -> [MyApp] {
        let needle = query.trimmingCharacters(in:.whitespacesAndNewlines)
        guard !needle.isEmpty else { return [] }; return apps.filter { $0.name.localizedStandardContains(needle) }
    }
    static func initial(_ name: String) -> String { name.trimmingCharacters(in:.whitespacesAndNewlines).first.map { String($0).uppercased() } ?? "?" }
}
