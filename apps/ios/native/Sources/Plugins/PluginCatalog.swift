import Foundation

/// The engine's plugin catalog (`PluginAdmin get_catalog`), parsed once for the
/// plugins pages and the marketplace seeder. Field names follow the engine's
/// rows; nothing here decides anything the engine does not.
struct PluginCatalog: Equatable {
    var installed: [InstalledPlugin] = []
    var available: [AvailablePlugin] = []
    var marketplaces: [PluginMarketplace] = []
    /// The effective `enabledPlugins` map across every settings layer.
    var enabled: [String: Bool] = [:]
    /// The revision a write to each settings layer (`user`, `project`, `local`) must name.
    var revisions: [String: String] = [:]

    /// The engine's rule: an explicit `enabledPlugins` entry wins, else the manifest's `defaultEnabled`.
    func isEnabled(_ plugin: InstalledPlugin) -> Bool { enabled[plugin.id] ?? plugin.defaultEnabled }

    func installed(_ id: String) -> InstalledPlugin? { installed.first { $0.id == id } }

    func marketplace(_ name: String?) -> PluginMarketplace? { marketplaces.first { $0.name == name } }

    /// Marketplace plugins that are not installed yet.
    var installable: [AvailablePlugin] { available.filter { !$0.installed } }

    static func parse(_ json: String?) -> PluginCatalog {
        guard let json, let root = (try? JSONSerialization.jsonObject(with: Data(json.utf8))) as? [String: Any] else {
            return PluginCatalog()
        }
        let enabledJSON = nonBlank(root["enabled_json"]) ?? "{}"
        let enabled = (try? JSONSerialization.jsonObject(with: Data(enabledJSON.utf8))) as? [String: Any] ?? [:]
        let revisions = root["revisions"] as? [String: Any] ?? [:]
        return PluginCatalog(
            installed: objects(root["installed"]).map { row in
                let name = row["name"] as? String ?? ""
                return InstalledPlugin(
                    id: row["id"] as? String ?? "",
                    name: name,
                    displayName: nonBlank(row["display_name"]) ?? name,
                    version: row["version"] as? String ?? "",
                    path: row["path"] as? String ?? "",
                    defaultEnabled: row["default_enabled"] as? Bool ?? true,
                    description: row["description"] as? String ?? "",
                    dependencies: (row["dependencies"] as? [Any] ?? []).compactMap { $0 as? String },
                    configSchemaJSON: nonBlank(row["config_schema_json"]) ?? "null"
                )
            }.filter { !$0.id.trimmingCharacters(in: .whitespaces).isEmpty },
            available: objects(root["available"]).map { row in
                AvailablePlugin(
                    id: row["id"] as? String ?? "",
                    name: row["name"] as? String ?? "",
                    marketplace: row["marketplace"] as? String ?? "",
                    version: row["version"] as? String ?? "",
                    description: row["description"] as? String ?? "",
                    installed: row["installed"] as? Bool ?? false,
                    upgradeAvailable: row["upgrade_available"] as? Bool ?? false
                )
            }.filter { !$0.id.trimmingCharacters(in: .whitespaces).isEmpty },
            marketplaces: objects(root["marketplaces"]).map { row in
                PluginMarketplace(
                    name: row["name"] as? String ?? "",
                    sourceJSON: nonBlank(row["source_json"]) ?? "{}",
                    installLocation: nonBlank(row["install_location"]),
                    lastUpdated: nonBlank(row["last_updated"])
                )
            }.filter { !$0.name.trimmingCharacters(in: .whitespaces).isEmpty },
            enabled: enabled.compactMapValues { $0 as? Bool },
            revisions: revisions.compactMapValues(nonBlank)
        )
    }

    private static func objects(_ value: Any?) -> [[String: Any]] {
        (value as? [Any] ?? []).compactMap { $0 as? [String: Any] }
    }
}

/// One installed plugin, as the engine discovered it on disk.
struct InstalledPlugin: Equatable, Identifiable {
    /// `name@marketplace`, or the bare name for a plugin installed without one.
    let id: String
    let name: String
    let displayName: String
    let version: String
    /// The install directory in the engine's plugin cache.
    let path: String
    let defaultEnabled: Bool
    let description: String
    let dependencies: [String]
    let configSchemaJSON: String

    /// The marketplace the plugin was installed from, from its `name@marketplace` id.
    var marketplace: String? {
        guard let at = id.firstIndex(of: "@") else { return nil }
        let name = id[id.index(after: at)...]
        return name.isEmpty ? nil : String(name)
    }
}

/// One plugin a registered marketplace lists.
struct AvailablePlugin: Equatable, Identifiable {
    let id: String
    let name: String
    let marketplace: String
    /// The marketplace entry's own `version`; empty when the entry names none.
    let version: String
    let description: String
    let installed: Bool
    let upgradeAvailable: Bool
}

/// One registered marketplace.
struct PluginMarketplace: Equatable, Identifiable {
    let name: String
    let sourceJSON: String
    let installLocation: String?
    let lastUpdated: String?

    var id: String { name }

    private var source: [String: Any] {
        (try? JSONSerialization.jsonObject(with: Data(sourceJSON.utf8))) as? [String: Any] ?? [:]
    }

    /// `directory`, `github`, `git` or `url`.
    var sourceKind: String { source["source"] as? String ?? "" }

    /// What the source points at: a directory path, an `owner/repo`, or a URL.
    var sourceLocation: String {
        let source = source
        return nonBlank(source["path"]) ?? nonBlank(source["repo"]) ?? nonBlank(source["url"]) ?? ""
    }
}

/// A string value, or nil when it is absent, JSON null or blank.
func nonBlank(_ value: Any?) -> String? {
    guard let string = value as? String, !string.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else { return nil }
    return string
}
