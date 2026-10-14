import Foundation

/// What an installed plugin's own files say about it, for the plugins pages.
/// The engine's catalog carries ids, versions and paths only, so the vendor and
/// the component lists come from the plugin directory itself, read-only.
struct PluginDetails: Equatable {
    /// `author.name` (or a string `author`) from plugin.json, else the marketplace's `owner.name`.
    var vendor: String?
    var license: String?
    var homepage: String?
    var repository: String?
    var skills: [PluginComponent] = []
    var commands: [PluginComponent] = []
    var agents: [PluginComponent] = []
    /// Hook event names, such as `PreToolUse`.
    var hooks: [String] = []
    var mcpServers: [String] = []
}

struct PluginComponent: Equatable, Hashable {
    let name: String
    let description: String
}

/// Reads the standard plugin layout: `.lingxi-plugin/plugin.json`, then the
/// default component locations (`skills/`, `commands/`, `agents/`,
/// `hooks/hooks.json`, `.mcp.json`) plus any extra paths the manifest declares.
/// Paths that leave the plugin directory are ignored.
enum PluginDetailsReader {
    static func read(pluginDir: URL, marketplaceDir: URL? = nil) -> PluginDetails {
        let manifest = readJSON(pluginDir.appending(path: "\(manifestDir)/plugin.json")) ?? [:]
        let repository = nonBlank(manifest["repository"])
            ?? nonBlank((manifest["repository"] as? [String: Any])?["url"])
        return PluginDetails(
            vendor: author(manifest) ?? marketplaceDir.flatMap(marketplaceOwner),
            license: nonBlank(manifest["license"]),
            homepage: nonBlank(manifest["homepage"]),
            repository: repository,
            skills: unique(componentPaths(pluginDir, manifest, "skills").flatMap(skills(in:))),
            commands: unique(componentPaths(pluginDir, manifest, "commands").flatMap(markdown(in:))),
            agents: unique(componentPaths(pluginDir, manifest, "agents").flatMap(markdown(in:))),
            hooks: configNames(pluginDir, manifest, key: "hooks", defaultPath: "hooks/hooks.json", wrapper: "hooks") { $0 is [Any] },
            mcpServers: configNames(pluginDir, manifest, key: "mcpServers", defaultPath: ".mcp.json", wrapper: "mcpServers") {
                $0 is [String: Any]
            }
        )
    }

    /// Only the vendor, for list rows.
    static func vendor(pluginDir: URL, marketplaceDir: URL? = nil) -> String? {
        author(readJSON(pluginDir.appending(path: "\(manifestDir)/plugin.json")) ?? [:])
            ?? marketplaceDir.flatMap(marketplaceOwner)
    }

    private static let manifestDir = ".lingxi-plugin"

    /// Frontmatter only ever sits at the top; a long body is never read.
    private static let frontmatterLimit = 16 * 1024

    private static func author(_ manifest: [String: Any]) -> String? {
        nonBlank(manifest["author"]) ?? nonBlank((manifest["author"] as? [String: Any])?["name"])
    }

    private static func marketplaceOwner(_ marketplaceDir: URL) -> String? {
        let marketplace = readJSON(marketplaceDir.appending(path: "\(manifestDir)/marketplace.json"))
        return nonBlank((marketplace?["owner"] as? [String: Any])?["name"])
    }

    private static func unique(_ components: [PluginComponent]) -> [PluginComponent] {
        var seen = Set<String>()
        return components.filter { seen.insert($0.name).inserted }
    }

    /// The default location named `key` plus the manifest's extra paths for it, existing and inside `root`.
    private static func componentPaths(_ root: URL, _ manifest: [String: Any], _ key: String) -> [URL] {
        let declared: [String] = switch manifest[key] {
        case let value as String: [value]
        case let values as [Any]: values.compactMap { $0 as? String }
        default: []
        }
        var seen = Set<String>()
        return ([key] + declared).compactMap { inside(root, $0) }
            .filter { FileManager.default.fileExists(atPath: $0.path) && seen.insert($0.path).inserted }
    }

    /// A skills location is either one skill (it holds SKILL.md) or a directory of skills.
    private static func skills(in location: URL) -> [PluginComponent] {
        let skillDirs: [URL]
        if isFile(location.appending(path: "SKILL.md")) {
            skillDirs = [location]
        } else {
            let children = (try? FileManager.default.contentsOfDirectory(at: location, includingPropertiesForKeys: nil)) ?? []
            skillDirs = children.filter { isFile($0.appending(path: "SKILL.md")) }
                .sorted { $0.lastPathComponent < $1.lastPathComponent }
        }
        return skillDirs.map { dir in
            let fields = frontmatter(head(dir.appending(path: "SKILL.md")))
            return PluginComponent(name: nonBlank(fields["name"]) ?? dir.lastPathComponent, description: fields["description"] ?? "")
        }
    }

    /// Commands and agents are Markdown files, one per entry, in subdirectories too.
    private static func markdown(in location: URL) -> [PluginComponent] {
        markdownFiles(in: location, depth: 0).sorted { $0.path < $1.path }.map { file in
            let fields = frontmatter(head(file))
            return PluginComponent(
                name: nonBlank(fields["name"]) ?? file.deletingPathExtension().lastPathComponent,
                description: fields["description"] ?? ""
            )
        }
    }

    private static func markdownFiles(in location: URL, depth: Int) -> [URL] {
        if isFile(location) { return location.pathExtension == "md" ? [location] : [] }
        guard depth < 4 else { return [] }
        let children = (try? FileManager.default.contentsOfDirectory(at: location, includingPropertiesForKeys: nil)) ?? []
        return children.flatMap { markdownFiles(in: $0, depth: depth + 1) }
    }

    /// Names under a hooks or MCP configuration: the default file, a manifest
    /// path, or an object inlined in the manifest. Either shape is accepted,
    /// with or without the wrapping `wrapper` key.
    private static func configNames(
        _ root: URL,
        _ manifest: [String: Any],
        key: String,
        defaultPath: String,
        wrapper: String,
        isEntry: (Any) -> Bool
    ) -> [String] {
        func document(_ item: Any) -> [String: Any]? {
            switch item {
            case let object as [String: Any]: object
            case let path as String: inside(root, path).flatMap(readJSON)
            default: nil
            }
        }
        let declared: [[String: Any]] = switch manifest[key] {
        case let items as [Any]: items.compactMap(document)
        case let .some(item): document(item).map { [$0] } ?? []
        case .none: []
        }
        let documents = (inside(root, defaultPath).flatMap(readJSON).map { [$0] } ?? []) + declared
        let names = documents.flatMap { document -> [String] in
            let entries = document[wrapper] as? [String: Any] ?? document
            return entries.filter { isEntry($0.value) }.map(\.key)
        }
        return Array(Set(names)).sorted()
    }

    private static func inside(_ root: URL, _ relative: String) -> URL? {
        let base = root.standardizedFileURL.resolvingSymlinksInPath()
        var path = relative
        if path.hasPrefix("./") { path.removeFirst(2) }
        let file = base.appending(path: path).standardizedFileURL.resolvingSymlinksInPath()
        return file.path == base.path || file.path.hasPrefix(base.path + "/") ? file : nil
    }

    private static func isFile(_ url: URL) -> Bool {
        var directory: ObjCBool = false
        return FileManager.default.fileExists(atPath: url.path, isDirectory: &directory) && !directory.boolValue
    }

    private static func readJSON(_ url: URL) -> [String: Any]? {
        guard isFile(url), let data = try? Data(contentsOf: url) else { return nil }
        return (try? JSONSerialization.jsonObject(with: data)) as? [String: Any]
    }

    private static func head(_ url: URL) -> String {
        guard let handle = try? FileHandle(forReadingFrom: url) else { return "" }
        defer { try? handle.close() }
        let data = (try? handle.read(upToCount: frontmatterLimit)) ?? Data()
        return String(decoding: data, as: UTF8.self)
    }
}

/// The `key: value` pairs of a leading `---` frontmatter block. Handles quoted
/// values, `>`/`|` block scalars and indented continuation lines, which is all
/// that skill, command and agent files use; anything else is skipped.
func frontmatter(_ text: String) -> [String: String] {
    let lines = text.components(separatedBy: "\n").map { $0.hasSuffix("\r") ? String($0.dropLast()) : $0 }
    guard lines.first?.trimmingCharacters(in: .whitespaces) == "---",
          let end = lines.indices.dropFirst().first(where: { lines[$0].trimmingCharacters(in: .whitespaces) == "---" })
    else { return [:] }
    var fields: [String: String] = [:]
    let keyLine = #/([A-Za-z0-9_-]+):(.*)/#
    var index = 1
    while index < end {
        let match = lines[index].wholeMatch(of: keyLine)
        index += 1
        guard let match else { continue }
        let key = String(match.1)
        let value = match.2.trimmingCharacters(in: .whitespaces)
        var continuation: [String] = []
        while index < end, lines[index].trimmingCharacters(in: .whitespaces).isEmpty || lines[index].first?.isWhitespace == true {
            continuation.append(lines[index].trimmingCharacters(in: .whitespaces))
            index += 1
        }
        if value.hasPrefix("|") {
            fields[key] = continuation.joined(separator: "\n").trimmingCharacters(in: .whitespacesAndNewlines)
        } else if value.hasPrefix(">") {
            fields[key] = continuation.filter { !$0.isEmpty }.joined(separator: " ")
        } else {
            fields[key] = unquote(([value] + continuation.filter { !$0.isEmpty }).joined(separator: " ").trimmingCharacters(in: .whitespaces))
        }
    }
    return fields
}

private func unquote(_ value: String) -> String {
    guard value.count >= 2 else { return value }
    if value.first == "\"", value.last == "\"" {
        return String(value.dropFirst().dropLast()).replacingOccurrences(of: "\\\"", with: "\"").replacingOccurrences(of: "\\\\", with: "\\")
    }
    if value.first == "'", value.last == "'" {
        return String(value.dropFirst().dropLast()).replacingOccurrences(of: "''", with: "'")
    }
    return value
}
