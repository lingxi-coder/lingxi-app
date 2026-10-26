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
/// server name and each value is its raw JSON config. Only the connection
/// identity matters here; user headers, environment and other settings survive
/// when a matching existing server is opened.
func publicMCPTemplateMatch(name: String, template: [String: Any], configuredServers: [String: Any]) -> PublicMCPTemplateMatch {
    guard let existing = configuredServers[name] as? [String: Any] else { return .notConfigured }
    guard let currentIdentity = mcpConnectionIdentity(existing),
          let templateIdentity = mcpConnectionIdentity(template) else { return .nameConflict }
    return currentIdentity == templateIdentity ? .sameConfiguration : .nameConflict
}

private enum PublicMCPConnectionIdentity: Equatable {
    case url(type: String, value: String)
    case stdio(type: String, command: String, args: [String])
}

private func mcpConnectionIdentity(_ config: [String: Any]) -> PublicMCPConnectionIdentity? {
    var explicitType: String?
    for key in ["type", "transport"] where config.keys.contains(key) {
        guard let value = config[key] as? String else { return nil }
        let normalized = value.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !normalized.isEmpty else { return nil }
        if let explicitType, explicitType != normalized { return nil }
        explicitType = normalized
    }

    let hasURL = config.keys.contains("url")
    let hasCommand = config.keys.contains("command")
    guard hasURL != hasCommand else { return nil }

    if hasURL {
        guard let url = config["url"] as? String else { return nil }
        let endpoint = url.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !endpoint.isEmpty,
              let components = URLComponents(string: endpoint),
              let scheme = components.scheme?.lowercased(), ["http", "https", "ws", "wss"].contains(scheme),
              components.host?.isEmpty == false,
              components.user == nil,
              components.password == nil,
              components.fragment == nil else { return nil }
        let type = explicitType ?? "http"
        guard ["http", "streamable-http", "sse", "ws"].contains(type) else { return nil }
        guard (type == "ws" && ["ws", "wss"].contains(scheme))
            || (type != "ws" && ["http", "https"].contains(scheme)) else { return nil }
        return .url(type: type, value: endpoint)
    }

    guard let command = config["command"] as? String else { return nil }
    let executable = command.trimmingCharacters(in: .whitespacesAndNewlines)
    guard !executable.isEmpty else { return nil }
    if let explicitType, explicitType != "stdio" { return nil }
    let args: [String]
    if let rawArgs = config["args"] {
        guard let values = rawArgs as? [String] else { return nil }
        args = values
    } else {
        args = []
    }
    return .stdio(type: "stdio", command: executable, args: args)
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
    publicMarketplaceConfirmationRegistration(
        candidateName: candidate.name,
        enteredSource: candidate.source,
        registered: registered
    )
}

/// Re-evaluate the source text currently in the confirmation field against a
/// freshly loaded catalog. This keeps a stale alert from submitting a source
/// that is now present or collides with the curated marketplace name.
func publicMarketplaceConfirmationRegistration(
    candidateName: String,
    enteredSource: String,
    registered: [PluginMarketplace]
) -> PublicMarketplaceRegistration {
    let candidateRepo = githubRepositoryIdentity(enteredSource)
    if let conflict = registered.first(where: { marketplace in
        marketplace.name == candidateName
            && (candidateRepo == nil || registeredGitHubRepository(marketplace) != candidateRepo)
    }) {
        return .nameConflict(existingName: conflict.name)
    }
    if let candidateRepo,
       let match = registered.first(where: { marketplace in
           registeredGitHubRepository(marketplace) == candidateRepo
       }) {
        return .registered(name: match.name)
    }
    return .notRegistered
}

func publicMarketplaceConfirmationIsCurrent(
    capturedGeneration: Int,
    currentGeneration: Int,
    connected: Bool
) -> Bool {
    connected && capturedGeneration == currentGeneration
}

private func registeredGitHubRepository(_ marketplace: PluginMarketplace) -> String? {
    guard let source = (try? JSONSerialization.jsonObject(with: Data(marketplace.sourceJSON.utf8))) as? [String: Any] else { return nil }

    guard let sourceKind = source["source"] as? String else { return nil }
    switch sourceKind {
    case "github":
        guard Set(source.keys) == Set(["source", "repo"]),
              let repo = source["repo"] as? String else { return nil }
        return githubShorthandIdentity(repo)
    case "git":
        guard Set(source.keys) == Set(["source", "url"]),
              let url = source["url"] as? String else { return nil }
        return githubRemoteIdentity(url)
    default:
        return nil
    }
}

private func githubShorthandIdentity(_ source: String) -> String? {
    let value = source.trimmingCharacters(in: .whitespacesAndNewlines)
    guard !value.contains(":") && !value.contains("#") && !value.contains("@") && !value.contains("?") else { return nil }
    guard value.range(of: #"^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+(?:\.git)?$"#, options: .regularExpression) != nil else { return nil }
    let parts = value.replacingOccurrences(of: #"\.git$"#, with: "", options: .regularExpression)
        .split(separator: "/", omittingEmptySubsequences: false)
    guard parts.count == 2, parts.allSatisfy({ !$0.isEmpty && $0 != "." && $0 != ".." }) else { return nil }
    return "\(parts[0].lowercased())/\(parts[1].lowercased())"
}

private func githubRemoteIdentity(_ source: String) -> String? {
    let value = source.trimmingCharacters(in: .whitespacesAndNewlines)
    if let match = value.range(of: #"^git@github\.com:([A-Za-z0-9_.-]+)/([A-Za-z0-9_.-]+)(?:\.git)?$"#, options: [.regularExpression, .caseInsensitive]) {
        let scp = String(value[match])
        let parts = scp.dropFirst("git@github.com:".count).replacingOccurrences(of: #"\.git$"#, with: "", options: [.regularExpression, .caseInsensitive]).split(separator: "/")
        guard parts.count == 2, parts.allSatisfy({ !$0.isEmpty && $0 != "." && $0 != ".." }) else { return nil }
        return "\(parts[0].lowercased())/\(parts[1].lowercased())"
    }
    guard !value.contains("\\"),
          let schemeDelimiter = value.range(of: "://") else { return nil }
    let authorityAndPath = value[schemeDelimiter.upperBound...]
    let rawPath: String
    if let pathStart = authorityAndPath.firstIndex(of: "/") {
        rawPath = String(authorityAndPath[pathStart...])
    } else {
        rawPath = ""
    }
    if rawPath.contains("%") { return nil }
    let rawSegments = rawPath.trimmingCharacters(in: CharacterSet(charactersIn: "/"))
        .split(separator: "/", omittingEmptySubsequences: false)
    if rawSegments.contains(where: { $0 == "." || $0 == ".." }) { return nil }

    guard let components = URLComponents(string: value),
          let scheme = components.scheme?.lowercased(), ["https", "ssh"].contains(scheme),
          components.host?.lowercased() == "github.com",
          components.query == nil,
          components.fragment == nil else { return nil }
    if scheme == "https" {
        guard components.user == nil, components.password == nil,
              components.port == nil || components.port == 443 else { return nil }
    } else {
        guard components.password == nil,
              components.user == nil || components.user == "git",
              components.port == nil || components.port == 22 else { return nil }
    }
    var path = components.percentEncodedPath.trimmingCharacters(in: CharacterSet(charactersIn: "/"))
    if path.lowercased().hasSuffix(".git") { path.removeLast(4) }
    let parts = path.split(separator: "/", omittingEmptySubsequences: false)
    guard parts.count == 2,
          parts.allSatisfy({ !$0.isEmpty && $0 != "." && $0 != ".." && $0.range(of: #"^[A-Za-z0-9_.-]+$"#, options: .regularExpression) != nil }) else { return nil }
    return "\(parts[0].lowercased())/\(parts[1].lowercased())"
}

private func githubRepositoryIdentity(_ source: String) -> String? {
    githubShorthandIdentity(source) ?? githubRemoteIdentity(source)
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
