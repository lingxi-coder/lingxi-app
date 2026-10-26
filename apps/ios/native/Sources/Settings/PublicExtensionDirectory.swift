import Foundation
import SwiftUI

enum PublicExtensionKind: String, Hashable {
    case mcp
    case plugin
}

enum PublicExtensionCategory: String, CaseIterable, Identifiable, Hashable {
    case browser
    case development
    case productivity

    var id: String { rawValue }

    var localizationKey: String {
        switch self {
        case .browser: "public_extensions_category_browser"
        case .development: "public_extensions_category_development"
        case .productivity: "public_extensions_category_productivity"
        }
    }

    var displayName: String {
        String(localized: String.LocalizationValue(stringLiteral: localizationKey))
    }
}

struct PublicExtensionMarketplace {
    let source: String
    let name: String
    let plugin: String?
}

enum PublicMCPTemplateMatch: Equatable {
    case notConfigured
    case sameConfiguration
    case nameConflict
}

/// MCP snapshots expose a scope's `mcpServers` map, where the map key is the
/// server name and each value is its raw JSON config. A name match alone is not
/// enough to call a public template configured.
func publicMCPTemplateMatch(name: String, template: [String: Any], configuredServers: [String: Any]) -> PublicMCPTemplateMatch {
    guard let existing = configuredServers[name] as? [String: Any] else { return .notConfigured }
    guard let currentJSON = canonicalJSON(existing),
          let templateJSON = canonicalJSON(template) else { return .nameConflict }
    return currentJSON == templateJSON ? .sameConfiguration : .nameConflict
}

enum PublicMarketplaceRegistration: Equatable {
    case notRegistered
    case registered(name: String)
    case nameConflict(existingName: String)
}

/// Compare the curated GitHub shorthand with the engine's actual marketplace
/// DTO. `name` alone is insufficient: the registry can already contain that
/// name for a different source, in which case `marketplace_add` keeps the old
/// source instead of replacing it.
func publicMarketplaceRegistration(
    _ candidate: PublicExtensionMarketplace,
    registered: [PluginMarketplace]
) -> PublicMarketplaceRegistration {
    let candidateRepo = githubShorthandIdentity(candidate.source)
    if let candidateRepo,
       let match = registered.first(where: { marketplace in
           registeredGitHubRepository(marketplace) == candidateRepo
       }) {
        return .registered(name: match.name)
    }
    if let conflict = registered.first(where: { $0.name == candidate.name }) {
        return .nameConflict(existingName: conflict.name)
    }
    return .notRegistered
}

private func canonicalJSON(_ object: [String: Any]) -> String? {
    guard let data = try? JSONSerialization.data(withJSONObject: object, options: [.sortedKeys, .fragmentsAllowed]) else { return nil }
    return String(data: data, encoding: .utf8)
}

private func registeredGitHubRepository(_ marketplace: PluginMarketplace) -> String? {
    guard let source = (try? JSONSerialization.jsonObject(with: Data(marketplace.sourceJSON.utf8))) as? [String: Any],
          !source.keys.contains("ref"),
          !source.keys.contains("path"),
          !source.keys.contains("sparsePaths") else { return nil }

    guard let sourceKind = source["source"] as? String else { return nil }
    switch sourceKind {
    case "github":
        guard let repo = source["repo"] as? String else { return nil }
        return githubShorthandIdentity(repo)
    case "git":
        guard let url = source["url"] as? String else { return nil }
        return githubRemoteIdentity(url)
    default:
        return nil
    }
}

private func githubShorthandIdentity(_ source: String) -> String? {
    let value = source.trimmingCharacters(in: .whitespacesAndNewlines)
    guard !value.contains(":") && !value.contains("#") && !value.contains("@") && !value.contains("?") else { return nil }
    let parts = value.split(separator: "/", omittingEmptySubsequences: false)
    guard parts.count == 2, !parts[0].isEmpty, !parts[1].isEmpty else { return nil }
    return "\(parts[0].lowercased())/\(parts[1].lowercased())"
}

private func githubRemoteIdentity(_ source: String) -> String? {
    guard let components = URLComponents(string: source),
          components.scheme?.lowercased() == "https",
          components.host?.lowercased() == "github.com",
          components.user == nil,
          components.password == nil,
          components.port == nil,
          components.query == nil,
          components.fragment == nil else { return nil }
    var path = components.path.trimmingCharacters(in: CharacterSet(charactersIn: "/"))
    if path.lowercased().hasSuffix(".git") { path.removeLast(4) }
    let parts = path.split(separator: "/", omittingEmptySubsequences: false)
    guard parts.count == 2, !parts[0].isEmpty, !parts[1].isEmpty else { return nil }
    return "\(parts[0].lowercased())/\(parts[1].lowercased())"
}

/// An entry from the bundled, curated directory. MCP config stays as a raw
/// JSON object so fields the current editor does not know about survive when
/// the user copies a template into the existing raw-config editor.
struct PublicExtensionItem: Identifiable {
    let id: String
    let kind: PublicExtensionKind
    let name: String
    let publisher: String
    let category: PublicExtensionCategory
    let platforms: [String]
    let description: [String: String]
    let requirements: [String: String]?
    let sourceURL: String
    let documentationURL: String
    let version: String?
    let mcpName: String?
    let mcpConfig: [String: Any]?
    let marketplace: PublicExtensionMarketplace?

    func localizedDescription(locale: Locale) -> String {
        localized(description, locale: locale, fallback: name)
    }

    func localizedRequirements(locale: Locale) -> String? {
        guard let requirements else { return nil }
        return localized(requirements, locale: locale, fallback: "")
    }

    private func localized(_ values: [String: String], locale: Locale, fallback: String) -> String {
        let language = locale.language.languageCode?.identifier ?? "en"
        let script = locale.language.script?.identifier
        let regionalKey = script.map { "\(language)-\($0)" }
        let keys = [locale.identifier, regionalKey, language, "en"].compactMap { $0 }
        return keys.lazy.compactMap { values[$0] }.first ?? fallback
    }
}

struct PublicExtensionDirectory {
    let items: [PublicExtensionItem]

    static func load(bundle: Bundle = .main) -> PublicExtensionDirectory? {
        guard let url = bundle.url(forResource: "public-directory", withExtension: "json"),
              let data = try? Data(contentsOf: url) else { return nil }
        return parse(data)
    }

    static func parse(_ data: Data) -> PublicExtensionDirectory? {
        guard let root = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
              root["schemaVersion"] as? Int == 1,
              let rawItems = root["items"] as? [[String: Any]] else { return nil }

        let items = rawItems.compactMap(parseItem)
            .filter { $0.platforms.contains("ios") }
        return PublicExtensionDirectory(items: items)
    }

    private static func parseItem(_ value: [String: Any]) -> PublicExtensionItem? {
        guard let id = value["id"] as? String, !id.isEmpty,
              let kindValue = value["kind"] as? String,
              let kind = PublicExtensionKind(rawValue: kindValue),
              let name = value["name"] as? String, !name.isEmpty,
              let publisher = value["publisher"] as? String,
              let categoryValue = value["category"] as? String,
              let category = PublicExtensionCategory(rawValue: categoryValue),
              let platforms = value["platforms"] as? [String],
              let description = value["description"] as? [String: String],
              let sourceURL = value["sourceUrl"] as? String,
              let documentationURL = value["documentationUrl"] as? String else { return nil }

        let requirements = value["requirements"] as? [String: String]
        let version = value["version"] as? String
        let rawMCP = value["mcp"] as? [String: Any]
        let mcpName = rawMCP?["name"] as? String
        let mcpConfig = rawMCP?["config"] as? [String: Any]
        let rawMarketplace = value["marketplace"] as? [String: Any]
        let marketplace: PublicExtensionMarketplace?
        if let rawMarketplace,
           let source = rawMarketplace["source"] as? String,
           let marketplaceName = rawMarketplace["name"] as? String {
            marketplace = PublicExtensionMarketplace(
                source: source,
                name: marketplaceName,
                plugin: rawMarketplace["plugin"] as? String
            )
        } else {
            marketplace = nil
        }

        return PublicExtensionItem(
            id: id,
            kind: kind,
            name: name,
            publisher: publisher,
            category: category,
            platforms: platforms,
            description: description,
            requirements: requirements,
            sourceURL: sourceURL,
            documentationURL: documentationURL,
            version: version,
            mcpName: mcpName,
            mcpConfig: mcpConfig,
            marketplace: marketplace
        )
    }
}

/// Shared search/category list for the public MCP and plugin sources.
struct PublicExtensionDirectoryList: View {
    let kind: PublicExtensionKind
    let actionTitle: (PublicExtensionItem) -> String
    let action: (PublicExtensionItem) -> Void
    let canAct: (PublicExtensionItem) -> Bool

    @Environment(\.theme) private var theme
    @Environment(LocalizationManager.self) private var localization
    @State private var directory = PublicExtensionDirectory.load()
    @State private var search = ""
    @State private var category: PublicExtensionCategory?

    init(
        kind: PublicExtensionKind,
        actionTitle: @escaping (PublicExtensionItem) -> String,
        action: @escaping (PublicExtensionItem) -> Void,
        canAct: @escaping (PublicExtensionItem) -> Bool = { _ in true }
    ) {
        self.kind = kind
        self.actionTitle = actionTitle
        self.action = action
        self.canAct = canAct
    }

    private var locale: Locale { localization.effectiveLocale() }

    private var visibleItems: [PublicExtensionItem] {
        guard let items = directory?.items else { return [] }
        return items.filter { item in
            guard item.kind == kind,
                  category.map({ item.category == $0 }) ?? true else { return false }
            guard !search.isEmpty else { return true }
            let query = search.trimmingCharacters(in: .whitespacesAndNewlines)
            return [item.name, item.publisher, item.localizedDescription(locale: locale),
                    item.localizedRequirements(locale: locale) ?? ""]
                .contains { $0.localizedStandardContains(query) }
        }
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            TextField(String(localized: "public_extensions_search"), text: $search)
                .textFieldStyle(.roundedBorder)
                .autocorrectionDisabled()

            Picker(String(localized: "public_extensions_category"), selection: $category) {
                Text("public_extensions_category_all").tag(PublicExtensionCategory?.none)
                ForEach(PublicExtensionCategory.allCases) { category in
                    Text(LocalizedStringKey(category.localizationKey)).tag(Optional(category))
                }
            }
            .pickerStyle(.menu)

            if case nil = directory {
                Label(String(localized: "public_extensions_unavailable"), systemImage: "exclamationmark.triangle")
                    .font(.footnote).foregroundStyle(.secondary)
            } else if visibleItems.isEmpty {
                Text("public_extensions_empty")
                    .font(.footnote).foregroundStyle(.secondary)
            } else {
                ForEach(visibleItems) { item in
                    itemRow(item)
                }
            }
        }
    }

    private func itemRow(_ item: PublicExtensionItem) -> some View {
        VStack(alignment: .leading, spacing: 7) {
            HStack(alignment: .firstTextBaseline) {
                Text(item.name).font(.body.weight(.medium)).foregroundStyle(theme.text)
                Spacer(minLength: 8)
                if let version = item.version, !version.isEmpty {
                    Text(version).font(.caption.monospaced()).foregroundStyle(.secondary)
                }
            }
            Text("\(item.publisher) · \(item.category.displayName)")
                .font(.caption).foregroundStyle(.secondary)
            Text(item.localizedDescription(locale: locale))
                .font(.footnote).foregroundStyle(theme.text2)
                .fixedSize(horizontal: false, vertical: true)
            if let requirements = item.localizedRequirements(locale: locale), !requirements.isEmpty {
                Text(String(format: String(localized: "public_extensions_requirements_format"), requirements))
                    .font(.caption).foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }
            Button(actionTitle(item)) { action(item) }
                .font(.footnote.weight(.semibold))
                .foregroundStyle(theme.accent)
                .padding(.top, 2)
                .disabled(!canAct(item))
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(13)
        .background(theme.surface, in: RoundedRectangle(cornerRadius: 12))
        .overlay(RoundedRectangle(cornerRadius: 12).stroke(theme.border, lineWidth: 0.5))
    }
}
