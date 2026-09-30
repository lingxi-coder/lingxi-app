// SecureStorageImpl.swift — iOS native Keychain-backed secure store.
//
// Conforms to the generated `IosSecureStorage` UniFFI callback interface (the
// Rust bridge in `apps/ios-framework` adapts it to `traits::SecureStorage`). The
// engine's serialized `SecureStorageData` crosses the seam as an opaque `blob`
// keyed by `(service, account)`; we persist it as a Keychain GenericPassword
// item with `kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly` — encrypted at
// rest by the data-protection/Secure Enclave and EXCLUDED from iCloud/iTunes
// backups (`...ThisDeviceOnly`). Injecting this store (via `buildIosEngine`'s
// `secureStorage:` param) flips the engine's `oauth_supported` true so OAuth
// `/login` can persist its tokens instead of failing at the persist step.
//
// DEVICE-VERIFY: this is native code; build + exercise on a device/simulator
// (the Rust host build cannot compile Swift). The Keychain calls are synchronous
// and fast, so the `async` methods run them inline (mirroring the other
// `Capabilities/*.swift` callback impls).

import Foundation

#if canImport(Security)
    import Security

    /// Synchronous backend seam: tests exercise storage transactions without
    /// reading or writing the device Keychain.
    protocol SecureStorageKeychainBackend: Sendable {
        func update(_ query: CFDictionary, attributes: CFDictionary) -> OSStatus
        func add(_ query: CFDictionary) -> OSStatus
        func copyMatching(_ query: CFDictionary, result: UnsafeMutablePointer<CFTypeRef?>) -> OSStatus
        func delete(_ query: CFDictionary) -> OSStatus
    }

    private struct SystemSecureStorageKeychainBackend: SecureStorageKeychainBackend {
        func update(_ query: CFDictionary, attributes: CFDictionary) -> OSStatus {
            SecItemUpdate(query, attributes)
        }

        func add(_ query: CFDictionary) -> OSStatus {
            SecItemAdd(query, nil)
        }

        func copyMatching(_ query: CFDictionary, result: UnsafeMutablePointer<CFTypeRef?>) -> OSStatus {
            SecItemCopyMatching(query, result)
        }

        func delete(_ query: CFDictionary) -> OSStatus {
            SecItemDelete(query)
        }
    }

    /// Native secure store over the iOS Keychain (`SecItem*`).
    final class SecureStorageImpl: IosSecureStorage, @unchecked Sendable {
        private let backend: any SecureStorageKeychainBackend

        convenience init() {
            self.init(backend: SystemSecureStorageKeychainBackend())
        }

        init(backend: any SecureStorageKeychainBackend) {
            self.backend = backend
        }

        func store(service: String, account: String, blob: Data) async throws {
            let base: [String: Any] = [
                kSecClass as String: kSecClassGenericPassword,
                kSecAttrService as String: service,
                kSecAttrAccount as String: account,
            ]
            let attributes: [String: Any] = [kSecValueData as String: blob]
            // Updating preserves the existing record if the backend rejects the
            // replacement. Delete-then-add would lose that valid fallback.
            var status = backend.update(base as CFDictionary, attributes: attributes as CFDictionary)
            if status == errSecItemNotFound {
                var add = base
                add[kSecValueData as String] = blob
                add[kSecAttrAccessible as String] = kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly
                status = backend.add(add as CFDictionary)
                if status == errSecDuplicateItem {
                    // Another writer inserted between our Update and Add.
                    // Replace its entry without deleting it first.
                    status = backend.update(base as CFDictionary, attributes: attributes as CFDictionary)
                }
            }
            guard status == errSecSuccess else { throw Self.ffiError(status, "store") }
        }

        func retrieve(service: String, account: String) async throws -> Data? {
            let query: [String: Any] = [
                kSecClass as String: kSecClassGenericPassword,
                kSecAttrService as String: service,
                kSecAttrAccount as String: account,
                kSecReturnData as String: true,
                kSecMatchLimit as String: kSecMatchLimitOne,
            ]
            var out: CFTypeRef?
            let status = backend.copyMatching(query as CFDictionary, result: &out)
            switch status {
            case errSecSuccess: return out as? Data
            case errSecItemNotFound: return nil
            default: throw Self.ffiError(status, "retrieve")
            }
        }

        func delete(service: String, account: String) async throws {
            let query: [String: Any] = [
                kSecClass as String: kSecClassGenericPassword,
                kSecAttrService as String: service,
                kSecAttrAccount as String: account,
            ]
            let status = backend.delete(query as CFDictionary)
            // Removing a non-existent entry is not an error (matches the trait).
            guard status == errSecSuccess || status == errSecItemNotFound else {
                throw Self.ffiError(status, "delete")
            }
        }

        func list(service: String) async throws -> [String] {
            let query: [String: Any] = [
                kSecClass as String: kSecClassGenericPassword,
                kSecAttrService as String: service,
                kSecReturnAttributes as String: true,
                kSecMatchLimit as String: kSecMatchLimitAll,
            ]
            var out: CFTypeRef?
            let status = backend.copyMatching(query as CFDictionary, result: &out)
            switch status {
            case errSecSuccess:
                let items = (out as? [[String: Any]]) ?? []
                return items.compactMap { $0[kSecAttrAccount as String] as? String }
            case errSecItemNotFound: return []
            default: throw Self.ffiError(status, "list")
            }
        }

        /// Map a Keychain `OSStatus` onto the generated flat `SecureStorageFfiError`.
        private static func ffiError(_ status: OSStatus, _ op: String) -> SecureStorageFfiError {
            let message = "keychain \(op) failed: OSStatus \(status)"
            switch status {
            case errSecAuthFailed, errSecInteractionNotAllowed, errSecUserCanceled:
                return .PermissionDenied(message: message)
            case errSecNotAvailable:
                return .BackendUnavailable(message: message)
            default:
                return .Io(message: message)
            }
        }
    }
#endif
