import Foundation
import CoreFoundation

struct ProviderImportEntry: Identifiable {
    let id = UUID()
    var name: String
    var draft: [String: Any]
    var credential: String?
    var selected: Bool
    let conflict: Bool
    var errors: [String] = []
    var warnings: [String] = []
}

struct ProviderImportDocument {
    var entries: [ProviderImportEntry] = []
    var errors: [String] = []
    var warnings: [String] = []
}

/// Supports the same LingXi and OpenCode JSON shapes as Desktop's importer.
/// Diagnostics deliberately never interpolate source values or unknown keys.
enum ProviderBulkImport {
    static let maximumBytes = 2_097_152
    static let supportedTypes = ["openai", "openai-responses", "anthropic", "gemini", "azure-openai", "bedrock-claude", "vertex-claude", "vertex-gemini", "foundry-claude"]
    /// Keys carried through from an imported provider. Anything absent here is
    /// dropped with a warning, so a provider reachable several ways MUST list
    /// `connections` / `credentialIds` / `fallback` — otherwise importing a
    /// multi-connection config silently yields a single-connection provider.
    private static let providerFields: Set<String> = ["type", "baseUrl", "apiKeyEnv", "models", "region", "apiVersion", "supportsWebsockets", "supportsWebsocketCompression", "websocketConnectTimeoutMs", "visionDelegate", "pricing", "billingMode", "connections", "credentialIds", "fallback"]
    private static let fallbackTriggers: Set<String> = ["rate_limit", "overloaded", "server_error", "network", "auth"]
    private static let modelFields: Set<String> = ["id", "aliases", "capabilities", "metadata"]
    private static let forbidden: Set<String> = ["__proto__", "prototype", "constructor"]

    static func parse(_ text: String, existing: [String: Any]) -> ProviderImportDocument {
        guard text.utf8.count <= maximumBytes else { return .init(errors: [localized("settings_parity_import_too_large")]) }
        guard let raw = try? JSONSerialization.jsonObject(with: Data(text.utf8)), let root = raw as? [String: Any] else {
            return .init(errors: [localized("settings_parity_import_invalid")])
        }
        guard !(root["provider"] != nil && root["providers"] != nil) else {
            return .init(errors: [localized("settings_parity_import_invalid_provider")])
        }
        let openCode = root["provider"] != nil
        let wrapped = openCode || root["providers"] != nil
        let candidate: Any? = openCode ? root["provider"] : root["providers"] ?? root
        guard let map = candidate as? [String: Any], !map.isEmpty else {
            return .init(errors: [localized("settings_parity_import_invalid_provider")])
        }
        var result = ProviderImportDocument()
        if wrapped && root.keys.contains(where: { !["provider", "providers", "$schema"].contains($0) }) {
            result.warnings.append(localized("settings_parity_import_top_ignored"))
        }
        for name in map.keys.sorted() {
            let conflict = existing[name] != nil
            var entry = ProviderImportEntry(name: name, draft: [:], selected: !conflict, conflict: conflict)
            guard let provider = map[name] as? [String: Any] else {
                entry.errors.append(localized("settings_parity_import_invalid_provider")); result.entries.append(entry); continue
            }
            if openCode { convertOpenCode(provider, entry: &entry) }
            else { convertNative(provider, entry: &entry) }
            result.entries.append(entry)
        }
        return result
    }

    /// Merge one connection over the provider-level defaults.
    ///
    /// Shallow, matching the engine's desugaring: a connection that restates
    /// `models` means "this endpoint serves exactly these", not "add to them".
    private static func mergedConnection(_ draft: [String: Any], _ connection: [String: Any]) -> [String: Any] {
        var merged = draft
        merged.removeValue(forKey: "connections")
        merged.removeValue(forKey: "fallback")
        merged.removeValue(forKey: "credentialIds")
        for (key, value) in connection where key != "id" && key != "credentialIds" { merged[key] = value }
        return merged
    }

    private static func validateCredentialIds(_ value: Any?) -> String? {
        guard let value else { return nil }
        guard let keys = value as? [Any], !keys.isEmpty else { return localized("settings_parity_provider_validation_invalid") }
        let ids = keys.compactMap { $0 as? String }.map { $0.trimmingCharacters(in: .whitespacesAndNewlines) }
        guard ids.count == keys.count, ids.allSatisfy({ !$0.isEmpty }), Set(ids).count == ids.count else {
            return localized("settings_parity_provider_validation_invalid")
        }
        return nil
    }

    /// A provider reachable several ways is validated CONNECTION BY CONNECTION:
    /// the provider entry itself only supplies defaults, so it need not satisfy
    /// `models` / `baseUrl` on its own, and no endpoint can pass a rule another
    /// one fails.
    private static func validateConnections(_ entry: ProviderImportEntry, credentialConfigured: Bool) -> String? {
        let draft = entry.draft
        guard let connections = draft["connections"] as? [[String: Any]], !connections.isEmpty else {
            return localized("settings_parity_provider_validation_invalid")
        }
        if let error = validateCredentialIds(draft["credentialIds"]) { return error }
        var seen: Set<String> = []
        for connection in connections {
            guard let rawID = connection["id"] as? String else { return localized("settings_parity_provider_validation_invalid") }
            let id = rawID.trimmingCharacters(in: .whitespacesAndNewlines)
            // The id becomes part of a qualified model reference, so a
            // separator in it would produce a reference that cannot be routed.
            guard !id.isEmpty, !id.contains("/"), !id.contains(":"), !id.contains("#"), !seen.contains(id) else {
                return localized("settings_parity_provider_validation_invalid")
            }
            seen.insert(id)
            if let error = validateCredentialIds(connection["credentialIds"]) { return error }
            var merged = entry
            merged.draft = mergedConnection(draft, connection)
            if let error = validate(merged, credentialConfigured: credentialConfigured) { return error }
        }
        if let fallback = draft["fallback"] {
            guard let map = fallback as? [String: Any] else { return localized("settings_parity_provider_validation_invalid") }
            if let on = map["on"] {
                guard let triggers = on as? [Any] else { return localized("settings_parity_provider_validation_invalid") }
                for trigger in triggers {
                    guard let name = trigger as? String, fallbackTriggers.contains(name) else {
                        return localized("settings_parity_provider_validation_invalid")
                    }
                }
            }
        }
        return nil
    }

    static func validate(_ entry: ProviderImportEntry, credentialConfigured: Bool) -> String? {
        guard entry.name.range(of: "^[a-z0-9][a-z0-9._-]{0,63}$", options: .regularExpression) != nil,
              !forbidden.contains(entry.name), !["builtin", "claude"].contains(entry.name) else { return localized("settings_parity_provider_validation_invalid") }
        if let error = entry.errors.first { return error }
        let draft = entry.draft
        if draft["connections"] != nil { return validateConnections(entry, credentialConfigured: credentialConfigured) }
        if let error = validateCredentialIds(draft["credentialIds"]) { return error }
        guard let type = draft["type"] as? String, supportedTypes.contains(type) else { return localized("settings_parity_provider_validation_protocol") }
        guard let models = draft["models"] as? [[String: Any]], !models.isEmpty else { return localized("settings_parity_provider_validation_models") }
        let ids = models.compactMap { $0["id"] as? String }.map { $0.trimmingCharacters(in: .whitespacesAndNewlines) }
        guard ids.count == models.count, ids.allSatisfy({ !$0.isEmpty }), Set(ids).count == ids.count else { return localized("settings_parity_provider_validation_models") }
        for model in models {
            if let aliases = model["aliases"], !nonemptyStrings(aliases) { return localized("settings_parity_provider_validation_models") }
            if let capabilities = model["capabilities"] {
                guard let map = capabilities as? [String: Any], map.values.allSatisfy(isBoolean) else { return localized("settings_parity_provider_validation_invalid") }
            }
            if let metadata = model["metadata"], !validMetadata(metadata) { return localized("settings_parity_provider_validation_invalid") }
        }
        if type == "bedrock-claude" {
            guard nonempty(draft["region"]) else { return localized("settings_parity_provider_validation_invalid") }
        } else if !nonempty(draft["baseUrl"]) { return localized("settings_parity_provider_validation_url") }
        if let base = draft["baseUrl"] {
            guard let raw = base as? String, let url = URLComponents(string: raw), ["http", "https"].contains(url.scheme ?? ""),
                  url.host?.isEmpty == false, url.user == nil, url.password == nil, url.fragment == nil,
                  !raw.contains("{env:"), !raw.contains("{file:") else { return localized("settings_parity_provider_validation_url") }
        }
        if let env = draft["apiKeyEnv"] {
            guard let env = env as? String, env.range(of: "^[A-Za-z_][A-Za-z0-9_]*$", options: .regularExpression) != nil else { return localized("settings_parity_provider_validation_env") }
        }
        if type == "azure-openai" && !nonempty(draft["apiVersion"]) { return localized("settings_parity_provider_validation_invalid") }
        if let value = draft["visionDelegate"], !nonempty(value) { return localized("settings_parity_provider_validation_invalid") }
        if let billing = draft["billingMode"], !validBilling(billing) { return localized("settings_parity_provider_validation_pricing") }
        if let flag = draft["supportsWebsockets"], !isBoolean(flag) { return localized("settings_parity_provider_validation_invalid") }
        if draft["supportsWebsockets"] as? Bool == true && type != "openai-responses" { return localized("settings_parity_provider_validation_protocol") }
        if let flag = draft["supportsWebsocketCompression"], !isBoolean(flag) || flag as? Bool != false { return localized("settings_parity_provider_validation_invalid") }
        if let timeout = draft["websocketConnectTimeoutMs"], !nonnegativeInteger(timeout) { return localized("settings_parity_provider_validation_invalid") }
        if let pricing = draft["pricing"] {
            guard let prices = pricing as? [String: [String: Any]] else { return localized("settings_parity_provider_validation_pricing") }
            let allowed: Set<String> = ["inputPerMtok", "outputPerMtok", "cacheWritePerMtok", "cacheReadPerMtok", "reasoningPerMtok"]
            for (model, values) in prices {
                guard ids.contains(model), values["inputPerMtok"] != nil, values["outputPerMtok"] != nil,
                      values.allSatisfy({ allowed.contains($0.key) && nonnegativeNumber($0.value) }) else { return localized("settings_parity_provider_validation_pricing") }
            }
        }
        if type != "bedrock-claude", !nonempty(entry.credential), !nonempty(draft["apiKeyEnv"]), !credentialConfigured {
            return localized("settings_parity_import_credentials_required")
        }
        return nil
    }

    static func merge(_ entries: [ProviderImportEntry], into existing: [String: Any], configured: Set<String>) throws -> [String: Any] {
        let selected = entries.filter(\.selected)
        guard !selected.isEmpty else { throw ImportError(localized("settings_parity_import_select_required")) }
        guard Set(selected.map(\.name)).count == selected.count else { throw ImportError(localized("settings_parity_provider_validation_invalid")) }
        var merged = existing
        for entry in selected {
            if let error = validate(entry, credentialConfigured: configured.contains(entry.name)) { throw ImportError(error) }
            var sanitized = ProviderImportEntry(name: entry.name, draft: [:], selected: true, conflict: false)
            convertNative(entry.draft, entry: &sanitized)
            guard sanitized.errors.isEmpty, sanitized.credential == nil else { throw ImportError(localized("settings_parity_provider_validation_secret")) }
            merged[entry.name] = sanitized.draft
        }
        return merged
    }

    struct ImportError: LocalizedError {
        let message: String
        init(_ message: String) { self.message = message }
        var errorDescription: String? { message }
    }

    /// Normalize a `models` value: accept `"id"` strings or objects, keep only
    /// known model fields, and trim ids. Returns `nil` when the value is not a
    /// list, having already recorded the error.
    private static func normalizedModels(_ value: Any, entry: inout ProviderImportEntry) -> [[String: Any]]? {
        guard let models = value as? [Any] else {
            entry.errors.append(localized("settings_parity_provider_validation_models")); return nil
        }
        return models.map { value -> [String: Any] in
            if let string = value as? String { return ["id": string.trimmingCharacters(in: .whitespacesAndNewlines)] }
            guard let model = value as? [String: Any] else {
                entry.errors.append(localized("settings_parity_provider_validation_models")); return [:]
            }
            var sanitized: [String: Any] = [:]
            for (field, value) in model {
                guard modelFields.contains(field) else { entry.warnings.append(localized("settings_parity_import_models_warning")); continue }
                sanitized[field] = safe(value, errors: &entry.errors)
            }
            if let id = sanitized["id"] as? String { sanitized["id"] = id.trimmingCharacters(in: .whitespacesAndNewlines) }
            return sanitized
        }
    }

    private static func convertNative(_ raw: [String: Any], entry: inout ProviderImportEntry) {
        for (key, value) in raw where key != "apiKey" {
            guard providerFields.contains(key) else {
                if unsafeKey(key) { entry.errors.append(localized("settings_parity_provider_validation_secret")) }
                else { entry.warnings.append(localized("settings_parity_import_unsupported")) }
                continue
            }
            if key == "models" {
                guard let models = normalizedModels(value, entry: &entry) else { continue }
                entry.draft[key] = models
            } else if key == "connections" {
                guard let connections = value as? [Any] else { entry.errors.append(localized("settings_parity_provider_validation_invalid")); continue }
                // A connection carries the same editable fields as the provider
                // row, so it needs the same normalization — otherwise a
                // connection written with `models: ["m"]` survives import in a
                // shape the engine rejects.
                entry.draft[key] = connections.map { element -> [String: Any] in
                    guard let connection = element as? [String: Any] else {
                        entry.errors.append(localized("settings_parity_provider_validation_invalid")); return [:]
                    }
                    var sanitized: [String: Any] = [:]
                    for (field, value) in connection {
                        if field == "models" {
                            if let models = normalizedModels(value, entry: &entry) { sanitized[field] = models }
                        } else if ["id", "baseUrl", "apiKeyEnv"].contains(field), let text = value as? String {
                            sanitized[field] = text.trimmingCharacters(in: .whitespacesAndNewlines)
                        } else if field != "credentialIds" && unsafeKey(field) {
                            // `credentialIds` is a REFERENCE — keychain ids, not
                            // secrets — so it is named as an exception here the
                            // way `apiKeyEnv` is at the provider level. Every
                            // other credential-shaped key is still refused.
                            entry.errors.append(localized("settings_parity_provider_validation_secret"))
                        } else {
                            sanitized[field] = safe(value, errors: &entry.errors)
                        }
                    }
                    return sanitized
                }
            } else if ["baseUrl", "apiKeyEnv"].contains(key), let text = value as? String {
                entry.draft[key] = text.trimmingCharacters(in: .whitespacesAndNewlines)
            } else { entry.draft[key] = safe(value, errors: &entry.errors) }
        }
        if let value = raw["apiKey"] { readCredential(value, entry: &entry) }
    }

    private static func convertOpenCode(_ raw: [String: Any], entry: inout ProviderImportEntry) {
        let sdks = ["@ai-sdk/openai-compatible": "openai", "@ai-sdk/openai": "openai-responses", "@ai-sdk/anthropic": "anthropic", "@ai-sdk/google": "gemini"]
        entry.draft["type"] = (raw["npm"] as? String).flatMap { sdks[$0] } ?? ""
        if !nonempty(entry.draft["type"]) { entry.errors.append(localized("settings_parity_provider_validation_protocol")) }
        if let value = raw["options"] {
            if let options = value as? [String: Any] {
                for (key, value) in options {
                    switch key {
                    case "baseURL": entry.draft["baseUrl"] = (value as? String)?.trimmingCharacters(in: .whitespacesAndNewlines) ?? value
                    case "apiKey": readCredential(value, entry: &entry)
                    default: entry.errors.append(localized("settings_parity_import_unsupported"))
                    }
                }
            } else { entry.errors.append(localized("settings_parity_import_invalid_provider")) }
        }
        if let models = raw["models"] as? [String: [String: Any]] {
            entry.draft["models"] = models.keys.sorted().map { key -> [String: Any] in
                let model = models[key] ?? [:]
                let id = model["id"] as? String ?? key
                let metadata: Set<String> = ["name", "cost", "limit", "modalities", "release_date", "attachment", "reasoning", "temperature", "tool_call", "knowledge", "open_weights", "status"]
                for field in model.keys where field != "id" {
                    if metadata.contains(field) { entry.warnings.append(localized("settings_parity_import_models_warning")) }
                    else { entry.errors.append(localized("settings_parity_import_unsupported")) }
                }
                return id == key ? ["id": id] : ["id": id, "aliases": [key]]
            }
        } else { entry.errors.append(localized("settings_parity_provider_validation_models")) }
        for key in raw.keys where !["npm", "options", "models"].contains(key) {
            if ["name", "whitelist", "blacklist"].contains(key) { entry.warnings.append(localized("settings_parity_import_filters_warning")) }
            else { entry.errors.append(localized("settings_parity_import_unsupported")) }
        }
    }

    private static func readCredential(_ value: Any, entry: inout ProviderImportEntry) {
        if value is NSNull || (value as? String) == "" { return }
        guard let secret = value as? String else { entry.errors.append(localized("settings_parity_provider_validation_secret")); return }
        if secret.range(of: "^\\{env:[A-Za-z_][A-Za-z0-9_]*\\}$", options: .regularExpression) != nil {
            entry.draft["apiKeyEnv"] = String(secret.dropFirst(5).dropLast())
        } else if secret.contains("{env:") || secret.contains("{file:") { entry.errors.append(localized("settings_parity_import_credentials_required")) }
        else { entry.credential = secret.trimmingCharacters(in: .whitespacesAndNewlines) }
    }
    private static func safe(_ value: Any, errors: inout [String], depth: Int = 0) -> Any {
        guard depth <= 32 else { errors.append(localized("settings_parity_import_invalid_provider")); return NSNull() }
        if let array = value as? [Any] { return array.map { safe($0, errors: &errors, depth: depth + 1) } }
        if let object = value as? [String: Any] {
            var result: [String: Any] = [:]
            for (key, value) in object {
                if unsafeKey(key) { errors.append(localized("settings_parity_provider_validation_secret")) }
                else { result[key] = safe(value, errors: &errors, depth: depth + 1) }
            }
            return result
        }
        return value
    }
    private static func unsafeKey(_ key: String) -> Bool {
        forbidden.contains(key) || key.range(of: "(?:api.?key|secret|password|authorization|credential|headers)|^(?:access|refresh|auth|bearer)?[_-]?token$", options: [.regularExpression, .caseInsensitive]) != nil
    }
    private static func localized(_ key: String) -> String { String(localized: String.LocalizationValue(key)) }
    private static func nonempty(_ value: Any?) -> Bool { (value as? String)?.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty == false }
    private static func nonemptyStrings(_ value: Any) -> Bool { (value as? [String])?.allSatisfy { nonempty($0) } == true }
    private static func isBoolean(_ value: Any) -> Bool { guard let n = value as? NSNumber else { return false }; return CFGetTypeID(n) == CFBooleanGetTypeID() }
    private static func nonnegativeNumber(_ value: Any) -> Bool { guard !isBoolean(value), let n = value as? NSNumber else { return false }; return n.doubleValue.isFinite && n.doubleValue >= 0 }
    private static func nonnegativeInteger(_ value: Any) -> Bool { nonnegativeNumber(value) && (value as! NSNumber).doubleValue.rounded(.towardZero) == (value as! NSNumber).doubleValue }
    private static func validBilling(_ value: Any) -> Bool { (value as? String).map { ["perToken", "subscription", "free", "unknown"].contains($0) } == true }
    private static func validMetadata(_ value: Any) -> Bool {
        guard let object = value as? [String: Any] else { return false }
        for (key, value) in object where !(value is NSNull) {
            if ["family", "status", "releaseDate", "lastUpdated", "knowledgeCutoff"].contains(key), !(value is String) { return false }
            if ["contextWindowTokens", "maxInputTokens", "maxOutputTokens"].contains(key), !nonnegativeInteger(value) { return false }
            if ["openWeights", "attachments", "temperatureControl"].contains(key), !isBoolean(value) { return false }
            if ["inputModalities", "outputModalities"].contains(key), !(value is [String]) { return false }
            if key == "pricing", !validModelPricing(value) { return false }
        }
        return true
    }
    private static func validModelPricing(_ value: Any) -> Bool {
        guard let object = value as? [String: Any] else { return false }
        for (key, value) in object where !(value is NSNull) {
            if ["inputPerMillion", "outputPerMillion", "cacheReadPerMillion", "cacheWritePerMillion", "reasoningPerMillion"].contains(key), !nonnegativeNumber(value) { return false }
            if key == "billingMode", !validBilling(value) { return false }
            if key == "source", !(value is String) { return false }
            if key == "tiers" {
                guard let tiers = value as? [[String: Any]], tiers.allSatisfy({ $0["contextThresholdTokens"].map(nonnegativeInteger) == true && validModelPricing($0) }) else { return false }
            }
        }
        return true
    }
}
