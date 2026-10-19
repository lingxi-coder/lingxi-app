import Foundation

/// The native adapter for packages/deep-links. Parsing only describes a destination.
struct CanonicalDeepLink: Codable, Equatable, Sendable, Identifiable {
    let route: String
    let params: [String: String]
    let payloadJSON: String?
    var id: String { route + params.sorted { $0.key < $1.key }.description + (payloadJSON ?? "") }
    var parts: [String] { route.split(separator: "/").map(String.init) }
    var payload: [String: Any] {
        guard let payloadJSON, let data = payloadJSON.data(using: .utf8) else { return [:] }
        return (try? JSONSerialization.jsonObject(with: data)) as? [String: Any] ?? [:]
    }

    func needsContextRebind(workspace: String, mode: String) -> Bool {
        guard !["session", "new", "project", "market", "apps"].contains(parts[0]) else { return false }
        return params["workspace"].map { $0 != workspace } == true
            || params["mode"].map { $0 != mode } == true
    }

    static func parse(_ raw: String) -> Self? {
        guard raw.range(of: "%(?![0-9a-fA-F]{2})", options: .regularExpression) == nil,
              !raw.unicodeScalars.contains(where: { $0.value <= 32 || $0.value == 127 }),
              let url = URL(string: raw) else { return nil }
        return parse(url)
    }

    static func parse(_ url: URL) -> Self? {
        let raw = url.absoluteString
        guard raw.utf8.count <= 32768,
              !raw.unicodeScalars.contains(where: { $0.value <= 32 || $0.value == 127 }),
              raw.range(of: "%(?![0-9a-fA-F]{2})", options: .regularExpression) == nil,
              let components = URLComponents(url: url, resolvingAgainstBaseURL: false),
              components.user == nil, components.password == nil, components.port == nil else { return nil }
        let encoded: String
        if components.scheme?.lowercased() == "lingxi" {
            encoded = (components.host ?? "") + components.percentEncodedPath
        } else if components.scheme?.lowercased() == "https", components.host?.lowercased() == "lingxios.net", components.percentEncodedPath.hasPrefix("/open/") {
            encoded = String(components.percentEncodedPath.dropFirst(6))
        } else { return nil }
        let segments = encoded.split(separator: "/", omittingEmptySubsequences: false).map { String($0).removingPercentEncoding }
        guard segments.allSatisfy({ value in
            guard let value else { return false }
            return !value.isEmpty && value != "." && value != ".." && !value.contains("/") && !value.contains("\\") && !value.unicodeScalars.contains(where: { $0.value < 32 || $0.value == 127 })
        }) else { return nil }
        let parts = segments.compactMap { $0 }
        let simple: Set<String> = ["session", "project", "settings", "provider", "terminal", "apps", "market"]
        let valid = parts == ["new"] || (parts.count == 2 && simple.contains(parts[0]))
            || (parts.count == 3 && parts[0] == "extensions" && ["mcp", "skill", "plugin"].contains(parts[1]))
            || (parts.count == 3 && parts[0] == "cron" && ["job", "run"].contains(parts[1]))
            || (parts.count == 3 && parts[0] == "runtime" && ["agent", "task", "plan", "review", "activity"].contains(parts[1]))
            || (parts.count == 2 && parts[0] == "import" && ["mcp", "plugin", "skill", "provider"].contains(parts[1]))
        guard valid else { return nil }
        var params: [String: String] = [:]
        for item in components.queryItems ?? [] {
            guard ["workspace", "message", "mode", "scope"].contains(item.name), params[item.name] == nil,
                  let value = item.value, !value.isEmpty, value.count <= 512,
                  !value.unicodeScalars.contains(where: { $0.value < 32 || $0.value == 127 }) else { return nil }
            params[item.name] = value
        }
        guard params["mode"].map({ ["chat", "code"].contains($0) }) ?? true else { return nil }
        if let message = params["message"] {
            guard parts[0] == "session", message.range(of: "^[0-9a-fA-F]{8}(-[0-9a-fA-F]{4}){3}-[0-9a-fA-F]{12}$", options: .regularExpression) != nil else { return nil }
        }
        if let message = params["message"] { params["message"] = message.lowercased() }
        var payloadJSON: String?
        if let fragment = components.percentEncodedFragment, !fragment.isEmpty {
            guard ["new", "terminal", "import"].contains(parts[0]),
                  let fields = URLComponents(string: "?" + fragment.replacingOccurrences(of: "+", with: "%20"))?.queryItems,
                  fields.count == 1, fields[0].name == "payload", let text = fields[0].value,
                  let data = text.data(using: .utf8),
                  let payload = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any] else { return nil }
            if parts[0] == "import" {
                guard safeImportValue(payload) else { return nil }
            } else {
                guard payload.keys.allSatisfy({ $0 == (parts[0] == "new" ? "prompt" : "command") }), payload.values.allSatisfy({ $0 is String }) else { return nil }
            }
            payloadJSON = String(data: (try? JSONSerialization.data(withJSONObject: payload, options: [.sortedKeys])) ?? data, encoding: .utf8)
        }
        return Self(route: parts.joined(separator: "/"), params: params, payloadJSON: payloadJSON)
    }

    private static func safeImportValue(_ value: Any, depth: Int = 0) -> Bool {
        guard depth <= 16 else { return false }
        if let object = value as? [String: Any] {
            for (key, child) in object {
                if key.range(of: "^(api[-_]?key|token|access[-_]?token|refresh[-_]?token|authorization|password|secret|headers|env)$", options: [.regularExpression, .caseInsensitive]) != nil { return false }
                if !safeImportValue(child, depth: depth + 1) { return false }
            }
        } else if let array = value as? [Any] {
            return array.allSatisfy { safeImportValue($0, depth: depth + 1) }
        } else if let string = value as? String, string.lowercased().hasPrefix("http://") || string.lowercased().hasPrefix("https://") {
            guard let url = URLComponents(string: string), url.user == nil, url.password == nil, url.query == nil else { return false }
        }
        return true
    }
}
