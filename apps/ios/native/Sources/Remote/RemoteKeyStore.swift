import CryptoKit
import Foundation
import Security

#if canImport(harness_runtimeFFI)
/// Account-scoped device identity and pinned host keys. The actor serializes
/// read/update operations, including callbacks arriving from Rust threads.
actor RemoteKeyStore: IosRemoteKeyStore {
    private let service: String

    init(uid: String) {
        let account = SHA256.hash(data: Data(uid.utf8)).map { String(format: "%02x", $0) }.joined()
        service = (Bundle.main.bundleIdentifier ?? "com.lingxi.code") + ".remote-control." + account
    }

    func loadIdentity() async throws -> Data? { try read("identity") }
    func storeIdentity(pkcs8: Data) async throws { try write("identity", data: pkcs8) }
    func deleteIdentity() async throws { try remove("identity") }

    func loadPinnedHost(hostId: String) async throws -> RemotePinnedHostFfi? {
        guard let data = try read("host." + hostId) else { return nil }
        do {
            let pin = try JSONDecoder().decode(Pin.self, from: data)
            guard pin.hostId == hostId else { throw RemoteFfiError.KeyStore(message: "Invalid host pin") }
            return RemotePinnedHostFfi(hostId: pin.hostId, hostKey: pin.hostKey, name: pin.name, pairedAtMs: pin.pairedAtMs)
        } catch { throw RemoteFfiError.KeyStore(message: "Invalid stored host pin") }
    }

    func storePinnedHost(host: RemotePinnedHostFfi) async throws {
        let pin = Pin(hostId: host.hostId, hostKey: host.hostKey, name: host.name, pairedAtMs: host.pairedAtMs)
        try write("host." + host.hostId, data: JSONEncoder().encode(pin))
    }

    func deletePinnedHost(hostId: String) async throws { try remove("host." + hostId) }

    private struct Pin: Codable {
        let hostId: String
        let hostKey: String
        let name: String
        let pairedAtMs: UInt64
    }

    private func query(_ account: String) -> [String: Any] {
        [kSecClass as String: kSecClassGenericPassword, kSecAttrService as String: service,
         kSecAttrAccount as String: account, kSecAttrSynchronizable as String: false]
    }

    private func read(_ account: String) throws -> Data? {
        var request = query(account)
        request[kSecReturnData as String] = true
        request[kSecMatchLimit as String] = kSecMatchLimitOne
        var value: CFTypeRef?
        let status = SecItemCopyMatching(request as CFDictionary, &value)
        if status == errSecItemNotFound { return nil }
        guard status == errSecSuccess, let data = value as? Data else { throw error(status) }
        return data
    }

    private func write(_ account: String, data: Data) throws {
        let attributes: [String: Any] = [kSecValueData as String: data,
            kSecAttrAccessible as String: kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly]
        let request = query(account)
        let status = SecItemUpdate(request as CFDictionary, attributes as CFDictionary)
        if status == errSecItemNotFound {
            let result = SecItemAdd(request.merging(attributes) { _, new in new } as CFDictionary, nil)
            guard result == errSecSuccess else { throw error(result) }
        } else if status != errSecSuccess { throw error(status) }
    }

    private func remove(_ account: String) throws {
        let status = SecItemDelete(query(account) as CFDictionary)
        guard status == errSecSuccess || status == errSecItemNotFound else { throw error(status) }
    }

    private func error(_ status: OSStatus) -> RemoteFfiError {
        .KeyStore(message: "Keychain status \(status)")
    }
}
#endif
