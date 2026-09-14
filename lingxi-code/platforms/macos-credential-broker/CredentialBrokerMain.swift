import Foundation
import Security

@objc protocol LingXiCredentialBrokerXPC {
    func perform(_ requestData: NSData, withReply reply: @escaping (NSData?, NSString?) -> Void)
}

actor PayloadCache {
    struct CacheKey: Hashable {
        let service: String
        let account: String
    }

    private var values: [CacheKey: Data] = [:]

    func value(for key: CacheKey) -> Data? { values[key] }
    func set(_ value: Data, for key: CacheKey) { values[key] = value }
    func remove(_ key: CacheKey) { values.removeValue(forKey: key) }
}

final class CredentialStore {
    private let manifest: BrokerManifest
    private let cache = PayloadCache()

    init(manifest: BrokerManifest) {
        self.manifest = manifest
    }

    func handle(_ request: BrokerRequest) async -> BrokerResponse {
        do {
            switch request.op {
            case "health":
                return successResponse(buildVersion: manifest.version)
            case "store":
                let service = try validatedService(request.service)
                let account = try validatedAccount(request.account, service: service)
                guard let payload = request.payload,
                      let data = payload.data(using: .utf8),
                      !data.isEmpty,
                      data.count <= maxBrokerMessageBytes else {
                    throw BrokerFailure.invalidRequest("invalid payload")
                }
                try store(data: data, service: service, account: account)
                await cache.set(data, for: cacheKey(service: service, account: account))
                return successResponse()
            case "retrieve":
                let service = try validatedService(request.service)
                let account = try validatedAccount(request.account, service: service)
                let payload = try await retrieve(service: service, account: account)
                return successResponse(
                    present: payload != nil,
                    payload: payload.flatMap { String(data: $0, encoding: .utf8) }
                )
            case "contains":
                let service = try validatedService(request.service)
                let account = try validatedAccount(request.account, service: service)
                return successResponse(present: try contains(service: service, account: account))
            case "preview":
                let service = try validatedService(request.service)
                let account = try validatedAccount(request.account, service: service)
                let payload = try await retrieve(service: service, account: account)
                let value = payload.flatMap { String(data: $0, encoding: .utf8) }
                return successResponse(
                    present: value != nil,
                    payload: value.map(maskedPreview)
                )
            case "delete":
                let service = try validatedService(request.service)
                let account = try validatedAccount(request.account, service: service)
                try delete(service: service, account: account)
                await cache.remove(cacheKey(service: service, account: account))
                return successResponse()
            case "list":
                let service = try validatedService(request.service)
                return successResponse(accounts: try listAccounts(service: service))
            default:
                throw BrokerFailure.invalidRequest("unsupported broker operation")
            }
        } catch let failure as BrokerFailure {
            return failure.response
        } catch {
            return BrokerFailure.internalError(error.localizedDescription).response
        }
    }

    private func cacheKey(service: String, account: String) -> PayloadCache.CacheKey {
        PayloadCache.CacheKey(service: service, account: account)
    }

    private func validatedService(_ raw: String?) throws -> String {
        let service = try validateStorageComponent(raw ?? "", label: "service")
        let providerService = manifest.channel == "production"
            ? "com.lingxi.provider-credentials.v1"
            : "com.lingxi.provider-credentials.v1.development"
        let pluginSecretService = manifest.channel == "production"
            ? "com.lingxi.plugin-secrets.v1"
            : "com.lingxi.plugin-secrets.v1.development"
        let secureStoragePrefix = manifest.channel == "production"
            ? "com.lingxi.secure-storage.v1.production."
            : "com.lingxi.secure-storage.v1.development."
        guard service == providerService
                || service == pluginSecretService
                || service.hasPrefix(secureStoragePrefix) else {
            throw BrokerFailure.permission("credential service is outside the \(manifest.channel) channel")
        }
        return service
    }

    private func validatedAccount(_ raw: String?, service: String) throws -> String {
        let account = try validateStorageComponent(raw ?? "", label: "account")
        let providerService = manifest.channel == "production"
            ? "com.lingxi.provider-credentials.v1"
            : "com.lingxi.provider-credentials.v1.development"
        if service == providerService {
            let scalars = account.unicodeScalars
            guard scalars.count <= 64,
                  let first = scalars.first,
                  (first.value >= 97 && first.value <= 122) || (first.value >= 48 && first.value <= 57),
                  scalars.allSatisfy({ scalar in
                      (scalar.value >= 97 && scalar.value <= 122)
                          || (scalar.value >= 48 && scalar.value <= 57)
                          || scalar == "." || scalar == "_" || scalar == "-" || scalar == ":"
                  }) else {
                throw BrokerFailure.invalidRequest("invalid normalized provider identifier")
            }
        }
        return account
    }

    private func query(service: String, account: String?) -> [CFString: Any] {
        var query: [CFString: Any] = [
            kSecClass: kSecClassGenericPassword,
            kSecAttrService: service,
            kSecUseDataProtectionKeychain: true,
        ]
        if let account { query[kSecAttrAccount] = account }
        return query
    }

    private func retrieve(service: String, account: String) async throws -> Data? {
        let key = cacheKey(service: service, account: account)
        if let cached = await cache.value(for: key) { return cached }
        var query = query(service: service, account: account)
        query[kSecReturnData] = true
        query[kSecMatchLimit] = kSecMatchLimitOne
        var item: CFTypeRef?
        let status = SecItemCopyMatching(query as CFDictionary, &item)
        if status == errSecItemNotFound { return nil }
        guard status == errSecSuccess else {
            throw mapKeychainError(status, action: "retrieve secure storage item")
        }
        guard let data = item as? Data else {
            throw BrokerFailure.internalError("retrieved secure storage payload is invalid")
        }
        await cache.set(data, for: key)
        return data
    }

    private func contains(service: String, account: String) throws -> Bool {
        var query = query(service: service, account: account)
        query[kSecReturnAttributes] = true
        query[kSecMatchLimit] = kSecMatchLimitOne
        var item: CFTypeRef?
        let status = SecItemCopyMatching(query as CFDictionary, &item)
        if status == errSecItemNotFound { return false }
        guard status == errSecSuccess else {
            throw mapKeychainError(status, action: "check secure storage item")
        }
        return true
    }

    private func store(data: Data, service: String, account: String) throws {
        let updateStatus = SecItemUpdate(
            query(service: service, account: account) as CFDictionary,
            [kSecValueData: data] as CFDictionary
        )
        if updateStatus == errSecSuccess { return }
        if updateStatus != errSecItemNotFound {
            throw mapKeychainError(updateStatus, action: "update secure storage item")
        }
        var add = query(service: service, account: account)
        add[kSecValueData] = data
        add[kSecAttrAccessible] = kSecAttrAccessibleWhenUnlockedThisDeviceOnly
        let addStatus = SecItemAdd(add as CFDictionary, nil)
        guard addStatus == errSecSuccess else {
            throw mapKeychainError(addStatus, action: "store secure storage item")
        }
    }

    private func delete(service: String, account: String) throws {
        let status = SecItemDelete(query(service: service, account: account) as CFDictionary)
        guard status == errSecSuccess || status == errSecItemNotFound else {
            throw mapKeychainError(status, action: "delete secure storage item")
        }
    }

    private func listAccounts(service: String) throws -> [String] {
        var query = query(service: service, account: nil)
        query[kSecReturnAttributes] = true
        query[kSecMatchLimit] = kSecMatchLimitAll
        var item: CFTypeRef?
        let status = SecItemCopyMatching(query as CFDictionary, &item)
        if status == errSecItemNotFound { return [] }
        guard status == errSecSuccess else {
            throw mapKeychainError(status, action: "list secure storage accounts")
        }
        if let rows = item as? [[String: Any]] {
            return rows.compactMap { $0[kSecAttrAccount as String] as? String }.sorted()
        }
        if let row = item as? [String: Any], let account = row[kSecAttrAccount as String] as? String {
            return [account]
        }
        return []
    }

    private func mapKeychainError(_ status: OSStatus, action: String) -> BrokerFailure {
        switch status {
        case errSecInteractionNotAllowed:
            return .locked(securityMessage(status, action: action))
        case errSecAuthFailed, errSecUserCanceled:
            return .permission(securityMessage(status, action: action))
        default:
            return .unavailable(securityMessage(status, action: action))
        }
    }

    private func maskedPreview(_ secret: String) -> String {
        secret.count > 4 ? "••••\(secret.suffix(4))" : "••••"
    }
}

final class BrokerService: NSObject, LingXiCredentialBrokerXPC {
    private let store: CredentialStore

    init(store: CredentialStore) {
        self.store = store
    }

    func perform(_ requestData: NSData, withReply reply: @escaping (NSData?, NSString?) -> Void) {
        let data = requestData as Data
        guard data.count > 0, data.count <= maxBrokerMessageBytes else {
            let encoded = try? JSONEncoder().encode(BrokerFailure.invalidRequest("invalid broker request size").response)
            reply(encoded as NSData?, nil)
            return
        }
        Task {
            do {
                let request = try JSONDecoder().decode(BrokerRequest.self, from: data)
                let response = await store.handle(request)
                let encoded = try JSONEncoder().encode(response)
                if encoded.count <= maxBrokerMessageBytes {
                    reply(encoded as NSData, nil)
                } else {
                    let bounded = try? JSONEncoder().encode(
                        BrokerFailure.unavailable("credential broker response exceeds the size limit").response
                    )
                    reply(bounded as NSData?, nil)
                }
            } catch {
                let encoded = try? JSONEncoder().encode(BrokerFailure.invalidRequest("invalid broker request").response)
                reply(encoded as NSData?, nil)
            }
        }
    }
}

final class BrokerDelegate: NSObject, NSXPCListenerDelegate {
    private let store: CredentialStore
    private let teamId: String
    private let clientIdentifier: String

    init(store: CredentialStore, teamId: String, channel: String) {
        self.store = store
        self.teamId = teamId
        self.clientIdentifier = BrokerSecurity.clientIdentifier(channel: channel)
    }

    func listener(_ listener: NSXPCListener, shouldAcceptNewConnection newConnection: NSXPCConnection) -> Bool {
        guard newConnection.effectiveUserIdentifier == getuid() else { return false }
        newConnection.exportedInterface = NSXPCInterface(with: LingXiCredentialBrokerXPC.self)
        newConnection.exportedObject = BrokerService(store: store)
        // Enforce the exact client identity against the XPC peer's audit token
        // before accepting messages. A separate PID-based check repeats the
        // expensive trust evaluation on the serial listener queue and can hold
        // every client behind one cold verification. The connection requirement
        // is authoritative and avoids relying on a reusable process identifier.
        newConnection.setCodeSigningRequirement(
            requirementString(teamId: teamId, identifier: clientIdentifier)
        )
        newConnection.resume()
        return true
    }
}

@main
struct CredentialBrokerMain {
    static func main() throws {
        let manifest = try loadManifestForBrokerBundle()
        let teamId = try currentTeamIdentifier()
        let store = CredentialStore(manifest: manifest)
        let delegate = BrokerDelegate(store: store, teamId: teamId, channel: manifest.channel)
        let listener = NSXPCListener(machServiceName: BrokerSecurity.machService(channel: manifest.channel))
        listener.delegate = delegate
        listener.resume()
        RunLoop.main.run()
    }
}
