import Foundation

/// Reads and writes the mobile MCP config using the same files consumed by the
/// Rust engine: the app-private `settings.json` and the active project's
/// `.mcp.json`.
/// Secrets already present in `headers`/`env` are preserved but never rendered
/// by the UI.
@MainActor
final class MCPConfigurationRepository {
    static let shared = MCPConfigurationRepository(
        appSandboxRoot: URL(fileURLWithPath: ConversationSourceFactory.appSandboxRoot(), isDirectory: true)
    )

    private let settingsURL: URL

    init(appSandboxRoot: URL) {
        settingsURL = appSandboxRoot
            .appendingPathComponent(".lingxi", isDirectory: true)
            .appendingPathComponent("settings.json", isDirectory: false)
    }

    func loadServers(projectCwd: String? = nil) -> [MCPServer] {
        var merged: [String: MCPServer] = [:]
        for (name, entry) in entries(at: settingsURL, allowBareMap: false) {
            if let server = makeServer(name: name, entry: entry, scope: "user") {
                merged[name] = server
            }
        }
        if let projectCwd {
            let projectURL = URL(fileURLWithPath: projectCwd, isDirectory: true)
                .appendingPathComponent(".mcp.json", isDirectory: false)
            for (name, entry) in entries(at: projectURL, allowBareMap: true) {
                if let server = makeServer(name: name, entry: entry, scope: "project") {
                    // Project config has higher precedence than user config,
                    // matching the Rust loader used by the engine.
                    merged[name] = server
                }
            }
        }
        return merged.values.sorted {
            $0.name.localizedStandardCompare($1.name) == .orderedAscending
        }
    }

    func save(_ server: MCPServer, projectCwd: String? = nil) throws {
        let name = server.name.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !name.isEmpty, server.isConfigured else {
            throw NSError(domain: "MCPConfigurationRepository", code: 1, userInfo: [
                NSLocalizedDescriptionKey: "MCP server name and endpoint are required"
            ])
        }

        let targetURL = configurationURL(for: server, projectCwd: projectCwd)
        var root = try rootForWrite(at: targetURL)
        let allowBareMap = targetURL != settingsURL
        var servers = entries(in: root, allowBareMap: allowBareMap)
        let previousName = server.id.hasPrefix("new-") ? nil : server.id
        var entry = servers[previousName ?? name] ?? [:]
        let transport = MCPServer.normalizedTransport(server.transport)

        entry.removeValue(forKey: "url")
        entry.removeValue(forKey: "command")
        entry.removeValue(forKey: "args")
        entry.removeValue(forKey: "type")
        entry["disabled"] = !server.enabled

        if transport == "stdio" {
            entry["command"] = server.command.trimmingCharacters(in: .whitespacesAndNewlines)
            entry["args"] = server.args
        } else {
            entry["url"] = server.url?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
            entry["type"] = transport
        }

        if let previousName, previousName != name {
            servers.removeValue(forKey: previousName)
        }
        servers[name] = entry
        if targetURL == settingsURL || root["mcpServers"] != nil {
            root["mcpServers"] = servers
        } else {
            root = servers
        }
        try writeRoot(root, to: targetURL)
    }

    func delete(_ server: MCPServer, projectCwd: String? = nil) throws {
        let targetURL = configurationURL(for: server, projectCwd: projectCwd)
        guard let root = try rootForDelete(at: targetURL) else { return }
        var mutableRoot = root
        let allowBareMap = targetURL != settingsURL
        var servers = entries(in: root, allowBareMap: allowBareMap)
        if !server.id.hasPrefix("new-") {
            servers.removeValue(forKey: server.id)
        }
        servers.removeValue(forKey: server.name)
        if targetURL == settingsURL || root["mcpServers"] != nil {
            mutableRoot["mcpServers"] = servers
        } else {
            mutableRoot = servers
        }
        try writeRoot(mutableRoot, to: targetURL)
    }

    private func makeServer(name: String, entry: [String: Any], scope: String) -> MCPServer? {
        let command = (entry["command"] as? String)?.trimmingCharacters(in: .whitespacesAndNewlines)
        let url = (entry["url"] as? String)?.trimmingCharacters(in: .whitespacesAndNewlines)
        guard command?.isEmpty == false || url?.isEmpty == false else { return nil }

        let transport: String
        if command?.isEmpty == false {
            transport = "stdio"
        } else {
            transport = MCPServer.normalizedTransport((entry["type"] as? String) ?? "http")
        }
        let args = entry["args"] as? [String] ?? []
        let env = entry["env"] as? [String: String] ?? [:]
        let headers = entry["headers"] as? [String: String] ?? [:]
        let auth = entry["oauth"] != nil ? "oauth" : nil
        let enabled = !(entry["disabled"] as? Bool ?? false)
        return MCPServer(
            id: name,
            name: name,
            url: url,
            command: command ?? "",
            args: args,
            env: env,
            headers: headers,
            tools: nil,
            status: .idle,
            enabled: enabled,
            transport: transport,
            auth: auth,
            scope: scope
        )
    }

    private func configurationURL(for server: MCPServer, projectCwd: String?) -> URL {
        guard server.scope == "project", let projectCwd else { return settingsURL }
        return URL(fileURLWithPath: projectCwd, isDirectory: true)
            .appendingPathComponent(".mcp.json", isDirectory: false)
    }

    private func entries(at url: URL, allowBareMap: Bool) -> [String: [String: Any]] {
        guard case let .valid(root) = readRoot(from: url) else { return [:] }
        return entries(in: root, allowBareMap: allowBareMap)
    }

    private func entries(in root: [String: Any], allowBareMap: Bool) -> [String: [String: Any]] {
        if let servers = root["mcpServers"] as? [String: Any] {
            return servers.compactMapValues { $0 as? [String: Any] }
        }
        guard allowBareMap else { return [:] }
        return root.compactMapValues { $0 as? [String: Any] }
    }

    private enum RootReadResult {
        case missing
        case valid([String: Any])
        case invalid(Error)
    }

    private func readRoot(from url: URL) -> RootReadResult {
        let data: Data
        do {
            data = try Data(contentsOf: url)
        } catch {
            let nsError = error as NSError
            if nsError.domain == NSCocoaErrorDomain,
               nsError.code == CocoaError.fileNoSuchFile.rawValue {
                return .missing
            }
            return .invalid(error)
        }

        do {
            let object = try JSONSerialization.jsonObject(with: data)
            guard let root = object as? [String: Any] else {
                return .invalid(NSError(
                    domain: "MCPConfigurationRepository",
                    code: 2,
                    userInfo: [NSLocalizedDescriptionKey: "MCP configuration root must be a JSON object"]
                ))
            }
            return .valid(root)
        } catch {
            return .invalid(error)
        }
    }

    private func rootForWrite(at url: URL) throws -> [String: Any] {
        switch readRoot(from: url) {
        case .missing:
            return [:]
        case let .valid(root):
            return root
        case let .invalid(error):
            throw NSError(
                domain: "MCPConfigurationRepository",
                code: 3,
                userInfo: [
                    NSLocalizedDescriptionKey: "MCP configuration could not be read safely",
                    NSUnderlyingErrorKey: error,
                ]
            )
        }
    }

    private func rootForDelete(at url: URL) throws -> [String: Any]? {
        switch readRoot(from: url) {
        case .missing:
            return nil
        case let .valid(root):
            return root
        case let .invalid(error):
            throw NSError(
                domain: "MCPConfigurationRepository",
                code: 3,
                userInfo: [
                    NSLocalizedDescriptionKey: "MCP configuration could not be read safely",
                    NSUnderlyingErrorKey: error,
                ]
            )
        }
    }

    private func writeRoot(_ root: [String: Any], to url: URL) throws {
        let directory = url.deletingLastPathComponent()
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        let data = try JSONSerialization.data(withJSONObject: root, options: [.prettyPrinted, .sortedKeys])
        try data.write(to: url, options: .atomic)
    }
}
