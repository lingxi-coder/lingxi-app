// Keychain.swift — SHIP-BLOCKER #1 (iOS).
//
// A shipped mobile app has NO process environment, so the engine can no longer
// read the Anthropic API key from `ANTHROPIC_API_KEY`. This helper persists the
// key (and an optional base URL) in the iOS Keychain — the platform's encrypted,
// app-scoped secret store — using the raw Security framework (`SecItemAdd` /
// `SecItemCopyMatching` / `SecItemUpdate` / `SecItemDelete`). No third-party
// dependency; this is the canonical Apple-supported path.
//
// Scope: a `kSecClassGenericPassword` item keyed by (service, account). The
// service id is app-scoped (the bundle identifier + a fixed suffix) so the
// secret is namespaced to this app and survives reinstall-from-the-same-app but
// never leaks across apps. Items use
// `kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly`: readable by the engine
// after the first unlock following a boot (so a turn started from a notification
// or background launch works) and NEVER migrated to a backup or another device.
//
// SECRETS: the value lives ONLY in the Keychain. It is never logged here, never
// written to UserDefaults / DataStore, and never returned in any error.

import Foundation
import Security

/// App-scoped secure storage for the engine's LLM credentials.
///
/// Three slots, each a separate Keychain item under the same service:
///   - `apiKey`  — the Anthropic (or compatible) API key.
///   - `apiBase` — an optional base URL override (proxy / mirror); blank ⇒ default.
///   - `model`   — the user's last-picked real model id (SHIP-BLOCKER #2). Blank /
///                 unset ⇒ the engine starts on `MobileConfig.default_model`; never
///                 a branded mock id. Persisted so a relaunch resumes that model.
///
/// All accessors are static + synchronous: Keychain calls are fast and the call
/// sites (settings writes, engine config build) are already off the render path.
enum Keychain {
    /// The stored slots. The account string is the Keychain item's
    /// `kSecAttrAccount`, stable across versions.
    enum Item: String {
        case apiKey = "anthropic.apiKey"
        case apiBase = "anthropic.apiBase"
        case model = "anthropic.model"
    }

    /// App-scoped service id. Prefer the running bundle identifier so the secret
    /// is namespaced to whichever build (app / tests) created it; fall back to a
    /// fixed constant for previews / hosts without a bundle id.
    static let service: String = {
        let base = Bundle.main.bundleIdentifier ?? "com.lingxi.code"
        return base + ".engine-secrets"
    }()

    // MARK: get / set / clear

    /// The stored value for `item`, or `nil` when nothing is stored (or the read
    /// failed). An empty string is normalized to `nil` so a blanked field reads
    /// the same as "unset".
    static func get(_ item: Item) -> String? {
        var query = baseQuery(item)
        query[kSecReturnData as String] = true
        query[kSecMatchLimit as String] = kSecMatchLimitOne

        var result: CFTypeRef?
        let status = SecItemCopyMatching(query as CFDictionary, &result)
        guard status == errSecSuccess,
              let data = result as? Data,
              let value = String(data: data, encoding: .utf8),
              !value.isEmpty
        else { return nil }
        return value
    }

    /// Store (or replace) `item`'s value. A blank/whitespace-only value clears the
    /// item instead (so "save an empty key" deletes it rather than persisting "").
    /// Returns `true` on success.
    @discardableResult
    static func set(_ item: Item, _ value: String) -> Bool {
        let trimmed = value.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else { return clear(item) }
        guard let data = trimmed.data(using: .utf8) else { return false }

        let query = baseQuery(item)
        // Update first (common case: changing an existing key); add if absent.
        let attrs: [String: Any] = [kSecValueData as String: data]
        let updateStatus = SecItemUpdate(query as CFDictionary, attrs as CFDictionary)
        if updateStatus == errSecSuccess { return true }
        if updateStatus == errSecItemNotFound {
            var addQuery = query
            addQuery[kSecValueData as String] = data
            addQuery[kSecAttrAccessible as String] = kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly
            return SecItemAdd(addQuery as CFDictionary, nil) == errSecSuccess
        }
        return false
    }

    /// Delete `item` from the Keychain. Returns `true` when it is gone afterward
    /// (deleted now, or already absent).
    @discardableResult
    static func clear(_ item: Item) -> Bool {
        let status = SecItemDelete(baseQuery(item) as CFDictionary)
        return status == errSecSuccess || status == errSecItemNotFound
    }

    // MARK: internals

    /// The (class, service, account) tuple that identifies one item.
    private static func baseQuery(_ item: Item) -> [String: Any] {
        [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: item.rawValue,
        ]
    }
}
