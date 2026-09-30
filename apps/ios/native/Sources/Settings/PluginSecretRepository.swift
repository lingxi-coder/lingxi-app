import Foundation
import Observation

/// Matches protocol::SecureStorageData as serialized by IosSecureStorageBridge.
/// Deliberately encodes UTF-8 bytes as JSON integers, not Codable Data/base64.
/// This codec never writes to settings files and exposes no decoding API.
enum PluginSecretEnvelope {
    static func account(plugin: String, key: String) throws -> String {
        guard !plugin.isEmpty, !key.isEmpty, !plugin.contains("/"), !key.contains("/"),
              !plugin.contains("\0"), !key.contains("\0") else { throw PluginSecretError.invalidIdentity }
        return "plugin-secret-\(plugin)/\(key)"
    }

    static func encode(secret: String, plugin: String, key: String, createdAt: Date) throws -> Data {
        _ = try account(plugin: plugin, key: key)
        guard !secret.isEmpty else { throw PluginSecretError.emptySecret }
        let seconds = createdAt.timeIntervalSince1970
        guard seconds.isFinite, seconds >= 0, seconds < Double(UInt64.max) else { throw PluginSecretError.invalidTimestamp }
        let whole = floor(seconds)
        let nanos = UInt32(min(999_999_999, max(0, ((seconds - whole) * 1_000_000_000).rounded())))
        // SecretKindDto is an opaque string, so retain Rust's declared field
        // order (plugin, then key), as well as valid JSON string escaping.
        let pluginJSON = try JSONSerialization.data(withJSONObject: plugin, options: [.fragmentsAllowed, .withoutEscapingSlashes])
        let keyJSON = try JSONSerialization.data(withJSONObject: key, options: [.fragmentsAllowed, .withoutEscapingSlashes])
        guard let pluginLiteral = String(data: pluginJSON, encoding: .utf8),
              let keyLiteral = String(data: keyJSON, encoding: .utf8) else { throw PluginSecretError.invalidIdentity }
        let kind = "{\"PluginSecret\":{\"plugin\":\(pluginLiteral),\"key\":\(keyLiteral)}}"
        let object: [String: Any] = [
            "bytes": Array(secret.utf8),
            "metadata": [
                "created_at": ["secs_since_epoch": UInt64(whole), "nanos_since_epoch": nanos],
                "last_accessed": NSNull(),
                "kind": kind,
            ],
        ]
        return try JSONSerialization.data(withJSONObject: object, options: [.sortedKeys])
    }
}

private enum PluginSecretError: LocalizedError {
    case invalidIdentity, emptySecret, invalidTimestamp, unconfirmed
    var errorDescription: String? {
        switch self {
        case .invalidIdentity: "Plugin and field identifiers must be nonempty and contain no slash or NUL."
        case .emptySecret: "Enter a secret value before saving."
        case .invalidTimestamp: "The device clock cannot be encoded for secure storage."
        case .unconfirmed: "Secure storage did not confirm the requested change."
        }
    }
}

/// Reuses the exact native backend injected into buildIosEngine. Presence is
/// queried using list() only: stored values are never fetched or rendered.
@MainActor @Observable
final class PluginSecretRepository {
    static let shared = PluginSecretRepository(storage: SecureStorageImpl())
    private(set) var busy = false
    private(set) var loaded = false
    private(set) var needsReconnect = false
    private(set) var configuredAccounts: Set<String> = []
    private(set) var message: String?
    private(set) var errorMessage: String?
    @ObservationIgnored private let storage: any IosSecureStorage
    @ObservationIgnored private let now: () -> Date

    init(storage: any IosSecureStorage, now: @escaping () -> Date = Date.init) {
        self.storage = storage
        self.now = now
    }

    func isConfigured(plugin: String, key: String) -> Bool? {
        guard loaded, let account = try? PluginSecretEnvelope.account(plugin: plugin, key: key) else { return nil }
        return configuredAccounts.contains(account)
    }

    func refresh() async {
        guard !busy else { return }
        busy = true
        defer { busy = false }
        do {
            configuredAccounts = Set(try await storage.list(service: "lingxi"))
            loaded = true
            errorMessage = nil
        } catch {
            loaded = false
            errorMessage = error.localizedDescription
        }
    }

    func save(plugin: String, key: String, secret: String) async {
        guard !busy else { return }
        busy = true
        message = nil
        errorMessage = nil
        defer { busy = false }
        do {
            let account = try PluginSecretEnvelope.account(plugin: plugin, key: key)
            var blob = try PluginSecretEnvelope.encode(secret: secret, plugin: plugin, key: key, createdAt: now())
            defer { blob.resetBytes(in: 0..<blob.count) }
            try await storage.store(service: "lingxi", account: account, blob: blob)
            configuredAccounts = Set(try await storage.list(service: "lingxi"))
            loaded = true
            guard configuredAccounts.contains(account) else { throw PluginSecretError.unconfirmed }
            needsReconnect = true
            message = String(localized: "settings_parity_secret_saved_reconnect")
        } catch {
            loaded = false
            errorMessage = error.localizedDescription
        }
    }

    func delete(plugin: String, key: String) async {
        guard !busy else { return }
        busy = true
        message = nil
        errorMessage = nil
        defer { busy = false }
        do {
            let account = try PluginSecretEnvelope.account(plugin: plugin, key: key)
            try await storage.delete(service: "lingxi", account: account)
            configuredAccounts = Set(try await storage.list(service: "lingxi"))
            loaded = true
            guard !configuredAccounts.contains(account) else { throw PluginSecretError.unconfirmed }
            needsReconnect = true
            message = String(localized: "settings_parity_secret_deleted_reconnect")
        } catch {
            loaded = false
            errorMessage = error.localizedDescription
        }
    }

    func reconnect(using action: () async throws -> Void) async {
        guard !busy else { return }
        busy = true
        defer { busy = false }
        do {
            try await action()
            needsReconnect = false
            message = "The active engine reconnected. Other existing engine connections may still need to reconnect."
            errorMessage = nil
        } catch { errorMessage = error.localizedDescription }
    }
}
