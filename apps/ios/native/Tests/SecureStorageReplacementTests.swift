import Foundation
import Security
import XCTest

@testable import LingxiCode

final class SecureStorageReplacementTests: XCTestCase {
    func testAccountReplacementAndDeletionPreserveOtherAccountsAndScopes() async throws {
        let store = SecureStorageImpl(backend: AccountIsolatedKeychainBackend())
        try await store.store(service: "channel-a", account: "acct.account-a", blob: Data("key-a".utf8))
        try await store.store(service: "channel-a", account: "acct.account-b", blob: Data("grant-b".utf8))
        try await store.store(service: "channel-b", account: "acct.account-a", blob: Data("other-channel".utf8))
        try await store.store(service: "channel-a", account: "acct.account-a", blob: Data("rotated-a".utf8))
        let bBeforeDelete = try await store.retrieve(service: "channel-a", account: "acct.account-b")
        XCTAssertEqual(bBeforeDelete, Data("grant-b".utf8))
        try await store.delete(service: "channel-a", account: "acct.account-a")
        let removed = try await store.retrieve(service: "channel-a", account: "acct.account-a")
        let bAfterDelete = try await store.retrieve(service: "channel-a", account: "acct.account-b")
        let otherChannel = try await store.retrieve(service: "channel-b", account: "acct.account-a")
        XCTAssertNil(removed)
        XCTAssertEqual(bAfterDelete, Data("grant-b".utf8))
        XCTAssertEqual(otherChannel, Data("other-channel".utf8))
    }

    func testAccountListingUsesAttributesWithoutLoadingSecretPayloads() async throws {
        let backend = AccountIsolatedKeychainBackend()
        let store = SecureStorageImpl(backend: backend)
        try await store.store(service: "isolated-scope", account: "acct.a", blob: Data("fixture-a".utf8))
        try await store.store(service: "isolated-scope", account: "acct.b", blob: Data("fixture-b".utf8))
        let accounts = try await store.list(service: "isolated-scope")
        XCTAssertEqual(Set(accounts), ["acct.a", "acct.b"])
        XCTAssertEqual(backend.payloadReadCount, 0)
    }

    #if canImport(harness_runtimeFFI)
    @MainActor
    func testNativeAccountFacadePersistsIndependentKeysAndPriorityAcrossEngines() async throws {
        let sandbox = FileManager.default.temporaryDirectory.appendingPathComponent("native-account-roundtrip-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: sandbox, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: sandbox) }
        let storage = SecureStorageImpl(backend: AccountIsolatedKeychainBackend())
        let media = try IOSNativeMediaHostComposition(
            audioConfigurationJSON: try XCTUnwrap(AudioConfigurationStore.shared.mediaRuntimeConfigurationJSON))
        @MainActor func buildHandle() throws -> MobileEngineHandle {
            let directories = MobileUserStorage.directories(managedRoot: sandbox.appendingPathComponent("runtime").path)
            return try buildIosEngineWithConfig(config: IosEngineLaunchConfigFfi(
                appVersion: nil, appAgentHost: false,
                brandId: directories.brandId, linuxUser: directories.linuxUser,
                configDirName: directories.configDirName, nativeConfigHome: directories.configHome,
                apiBase: "https://api.anthropic.com", model: "", sessionMode: .code, initialSessionId: nil, initialSessionTranscriptPath: nil, schedulerController: false,
                visionDelegationEnabled: false, appSandboxRoot: sandbox.path, projectCwd: nil,
                projectAdditionalDirectories: [], projectMountGrants: [], providerConfig: nil,
                mobileLinux: try mobileWorkspaceTestConfiguration(sandboxRoot: sandbox),
                physicalMemoryBytes: 0, hostEnvironment: nil), listener: AccountRoundtripListener(),
                mediaHost: media.nativeHost, share: ShareImpl(), notifications: NotificationImpl(),
                clipboard: ClipboardImpl(), permissions: AccountRoundtripPermissions(),
                secureStorage: storage, deviceControl: nil)
        }
        let handle = try buildHandle()
        var snapshot = try JSONDecoder().decode(ProviderAccountSnapshot.self,
            from: Data(try await handle.providerAccountsSnapshot().utf8))
        let firstID = UUID().uuidString.lowercased()
        let secondID = UUID().uuidString.lowercased()
        @MainActor func send(_ request: ProviderAccountCommand, credential: String? = nil) async throws {
            var command = request
            command.expectedRevision = snapshot.configRevision
            let json = try await handle.providerAccountCommand(requestJson: command.encoded(), credential: credential)
            XCTAssertFalse(json.contains("native-fixture-key"))
            snapshot = try JSONDecoder().decode(ProviderAccountSnapshot.self, from: Data(json.utf8))
        }
        try await send(.init(action: "add", providerId: "openai", accountId: firstID,
            displayName: "Primary", executionBindingId: "openai", authKind: "api_key"), credential: "native-fixture-key-a")
        try await send(.init(action: "add", providerId: "openai", accountId: secondID,
            displayName: "Secondary", executionBindingId: "openai", authKind: "api_key"), credential: "native-fixture-key-b")
        try await send(.init(action: "reorder", providerId: "openai", accountIds: [secondID, firstID]))
        try await send(.init(action: "update", accountId: firstID, displayName: "Renamed primary"))
        try await send(.init(action: "set_enabled", accountId: firstID, enabled: false))
        let reloaded = try buildHandle()
        let reloadedSnapshot = try JSONDecoder().decode(ProviderAccountSnapshot.self,
            from: Data(try await reloaded.providerAccountsSnapshot().utf8))
        XCTAssertEqual(reloadedSnapshot.accounts.sorted { $0.userPriority < $1.userPriority }.map(\.accountId), [secondID, firstID])
        XCTAssertEqual(reloadedSnapshot.accounts.first { $0.accountId == firstID }?.displayName, "Renamed primary")
        XCTAssertEqual(reloadedSnapshot.accounts.first { $0.accountId == firstID }?.enabled, false)
        try await send(.init(action: "remove", accountId: firstID))
        let remaining = try JSONDecoder().decode(ProviderAccountSnapshot.self,
            from: Data(try await handle.providerAccountsSnapshot().utf8))
        XCTAssertEqual(remaining.accounts.map(\.accountId), [secondID])
        XCTAssertEqual(remaining.health.first { $0.accountId == secondID }?.status, "ready")
    }

    @MainActor
    func testLogicalOpenAIConfigurationExposesOAuthOnlyHostModelProjection() async throws {
        let sandbox = FileManager.default.temporaryDirectory.appendingPathComponent("native-oauth-projection-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: sandbox, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: sandbox) }
        let backend = AccountIsolatedKeychainBackend()
        let storage = SecureStorageImpl(backend: backend)
        let directories = MobileUserStorage.directories(managedRoot: sandbox.appendingPathComponent("runtime").path)
        let media = try IOSNativeMediaHostComposition(
            audioConfigurationJSON: try XCTUnwrap(AudioConfigurationStore.shared.mediaRuntimeConfigurationJSON))
        @MainActor func buildHandle() throws -> MobileEngineHandle {
            // Explicitly omit appVersion: the native carrier leaves startup_catalog None,
            // so this fixture uses only built-in local discovery and cannot prefetch a catalog.
            try buildIosEngineWithConfig(config: IosEngineLaunchConfigFfi(
                appVersion: nil, appAgentHost: false,
                brandId: directories.brandId, linuxUser: directories.linuxUser,
                configDirName: directories.configDirName, nativeConfigHome: directories.configHome,
                apiBase: "https://api.anthropic.com", model: "", sessionMode: .code, initialSessionId: nil, initialSessionTranscriptPath: nil, schedulerController: false,
                visionDelegationEnabled: false, appSandboxRoot: sandbox.path, projectCwd: nil,
                projectAdditionalDirectories: [], projectMountGrants: [],
                providerConfig: IosProviderConfigFfi(providerProfilesJson: "{}",
                    routingJson: #"{"mobileEnabledProfiles":["openai"]}"#),
                mobileLinux: try mobileWorkspaceTestConfiguration(sandboxRoot: sandbox),
                physicalMemoryBytes: 0, hostEnvironment: nil),
                listener: AccountRoundtripListener(), mediaHost: media.nativeHost, share: ShareImpl(),
                notifications: NotificationImpl(), clipboard: ClipboardImpl(),
                permissions: AccountRoundtripPermissions(), secureStorage: storage, deviceControl: nil)
        }
        let handle = try buildHandle()
        let initial = try JSONDecoder().decode(ProviderAccountSnapshot.self,
            from: Data(try await handle.providerAccountsSnapshot().utf8))
        let accountID = UUID().uuidString.lowercased()
        let command = ProviderAccountCommand(action: "add", expectedRevision: initial.configRevision,
            providerId: "openai", accountId: accountID, displayName: "Isolated OAuth fixture",
            executionBindingId: "openai-chatgpt", authKind: "oauth")
        let addedJSON = try await handle.providerAccountCommand(requestJson: command.encoded(), credential: nil)
        let added = try JSONDecoder().decode(ProviderAccountSnapshot.self, from: Data(addedJSON.utf8))
        let account = try XCTUnwrap(added.accounts.first { $0.accountId == accountID })
        let raw = try XCTUnwrap(JSONSerialization.jsonObject(with: Data(addedJSON.utf8)) as? [String: Any])
        let rawAccounts = try XCTUnwrap(raw["accounts"] as? [[String: Any]])
        let reference = try XCTUnwrap(rawAccounts.first?["credentialRef"] as? [String: Any])
        let scope = try XCTUnwrap(reference["storeScope"] as? String)
        let secretRef = try XCTUnwrap(reference["secretRef"] as? String)

        // Seed only this fake vault using the shared serialized credential/storage contract.
        // No OAuth request, real token, production setter, or system Keychain is involved.
        let timestamp: [String: Any] = ["secs_since_epoch": Int(Date().timeIntervalSince1970), "nanos_since_epoch": 0]
        let expiry: [String: Any] = ["secs_since_epoch": Int(Date().timeIntervalSince1970) + 3600, "nanos_since_epoch": 0]
        let credential: [String: Any] = [
            "credential_revision": 1, "identity_generation": account.identityGeneration,
            "kind": ["kind": "oauth", "access_token": "synthetic-native-projection-fixture",
                "refresh_token": NSNull(), "expires_at": expiry, "scopes": [],
                "issuer": "https://auth.openai.com", "subject": "isolated-user",
                "organization_id": "isolated-workspace", "email": NSNull(), "fedramp": false]
        ]
        let credentialBytes = try JSONSerialization.data(withJSONObject: credential)
        let wrapper: [String: Any] = ["bytes": Array(credentialBytes),
            "metadata": ["created_at": timestamp, "last_accessed": NSNull(), "kind": "provider_account_credential"]]
        try await storage.store(service: "provider-accounts.\(scope)", account: secretRef,
            blob: JSONSerialization.data(withJSONObject: wrapper))
        let ledger = URL(fileURLWithPath: directories.configHome)
            .appendingPathComponent("provider-accounts/versions/\(scope)/\(accountID).json")
        try FileManager.default.createDirectory(at: ledger.deletingLastPathComponent(), withIntermediateDirectories: true)
        try JSONSerialization.data(withJSONObject: ["accountId": accountID,
            "identityGeneration": account.identityGeneration, "credentialRevision": 1]).write(to: ledger)

        let reloaded = try buildHandle()
        let accounts = ProviderAccountRepository()
        accounts.configure(load: { try await reloaded.providerAccountsSnapshot() },
            command: { request, credential in
                try await reloaded.providerAccountCommand(requestJson: request, credential: credential)
            })
        let payloadReadsBeforeSnapshot = backend.payloadReadCount
        await accounts.refresh()
        XCTAssertEqual(backend.payloadReadCount, payloadReadsBeforeSnapshot,
            "A native account/model snapshot must use attributes without retrieving secret payloads")
        XCTAssertNil(accounts.errorMessage)
        XCTAssertEqual(accounts.snapshot.accounts.map(\.executionBindingId), ["openai-chatgpt"])
        let available = try XCTUnwrap(accounts.snapshot.modelAvailability.first {
            $0.executionBindingId == "openai-chatgpt" && $0.eligibleAccountIds.contains(accountID)
        }, "Logical OpenAI enablement must retain the canonical OAuth binding")
        XCTAssertTrue(available.unverifiedAccountIds.contains(accountID))
        XCTAssertFalse(available.confirmedAccountIds.contains(accountID), "Discovery is not inference authorization")
        XCTAssertFalse(accounts.snapshot.modelAvailability.contains {
            $0.executionBindingId == "openai" && $0.eligibleAccountIds.contains(accountID)
        }, "An OAuth-only account does not silently authorize a metered binding")
        let referenceID = "\(available.executionBindingId)/\(available.modelId)"
        let providers = providerConfigurationTestRepository(
            persistenceURL: sandbox.appendingPathComponent("provider-ui.json"), accounts: accounts)
        _ = providers.addProfile(presetID: "openai")
        XCTAssertEqual(providers.makeLaunchSnapshot().enabledProfileIDs, ["openai"])
        XCTAssertTrue(accounts.modelIsAvailable(reference: referenceID))
        XCTAssertEqual(providers.visibleModelReferences([referenceID]), [referenceID])
    }

    #endif

    func testRepeatedAccountReadsNeverRewriteExistingKeychainRecord() async throws {
        let original = Data("existing-account-credential-record".utf8)
        let backend = FakeSecureStorageKeychainBackend(payload: original)
        let store = SecureStorageImpl(backend: backend)

        for _ in 0..<3 {
            let retrieved = try await store.retrieve(service: "lingxi", account: "acct.fixture")
            XCTAssertEqual(retrieved, original)
        }

        XCTAssertEqual(backend.calls, ["retrieve", "retrieve", "retrieve"])
    }

    func testReplacementFailurePreservesExistingRecord() async throws {
        let original = Data("original-record".utf8)
        let backend = FakeSecureStorageKeychainBackend(
            payload: original,
            updateStatuses: [errSecNotAvailable]
        )
        let store = SecureStorageImpl(backend: backend)

        do {
            try await store.store(service: "test", account: "record", blob: Data("replacement".utf8))
            XCTFail("A failed backend update must be reported")
        } catch {}

        let retained = try await store.retrieve(service: "test", account: "record")
        XCTAssertEqual(retained, original)
        XCTAssertEqual(backend.calls, ["update", "retrieve"])
    }

    func testExistingRecordIsUpdatedWithoutDeleteOrAdd() async throws {
        let replacement = Data("replacement".utf8)
        let backend = FakeSecureStorageKeychainBackend(payload: Data("original-record".utf8))
        let store = SecureStorageImpl(backend: backend)

        try await store.store(service: "test", account: "record", blob: replacement)

        let persisted = try await store.retrieve(service: "test", account: "record")
        XCTAssertEqual(persisted, replacement)
        XCTAssertEqual(backend.calls, ["update", "retrieve"])
    }

    func testMissingRecordIsAddedWithDeviceOnlyAccessibility() async throws {
        let replacement = Data("new-record".utf8)
        let backend = FakeSecureStorageKeychainBackend()
        let store = SecureStorageImpl(backend: backend)

        try await store.store(service: "test", account: "record", blob: replacement)

        let persisted = try await store.retrieve(service: "test", account: "record")
        XCTAssertEqual(persisted, replacement)
        XCTAssertTrue(backend.usedDeviceOnlyAccessibility)
        XCTAssertEqual(backend.calls, ["update", "add", "retrieve"])
    }

    func testFailedAddLeavesMissingRecordMissing() async throws {
        let backend = FakeSecureStorageKeychainBackend(addStatus: errSecNotAvailable)
        let store = SecureStorageImpl(backend: backend)

        do {
            try await store.store(service: "test", account: "record", blob: Data("new-record".utf8))
            XCTFail("A failed backend insertion must be reported")
        } catch {}

        let persisted = try await store.retrieve(service: "test", account: "record")
        XCTAssertNil(persisted)
        XCTAssertEqual(backend.calls, ["update", "add", "retrieve"])
    }

    func testConcurrentInsertRetriesUpdateWithoutDeletingWinner() async throws {
        let replacement = Data("replacement".utf8)
        let backend = FakeSecureStorageKeychainBackend(insertBeforeAdd: Data("concurrent-winner".utf8))
        let store = SecureStorageImpl(backend: backend)

        try await store.store(service: "test", account: "record", blob: replacement)

        let persisted = try await store.retrieve(service: "test", account: "record")
        XCTAssertEqual(persisted, replacement)
        XCTAssertEqual(backend.calls, ["update", "add", "update", "retrieve"])
    }

    func testFailedConcurrentReplacementPreservesWinningRecord() async throws {
        let winner = Data("concurrent-winner".utf8)
        let backend = FakeSecureStorageKeychainBackend(
            updateStatuses: [errSecItemNotFound, errSecNotAvailable],
            insertBeforeAdd: winner
        )
        let store = SecureStorageImpl(backend: backend)

        do {
            try await store.store(service: "test", account: "record", blob: Data("replacement".utf8))
            XCTFail("The failed retry must be reported")
        } catch {}

        let persisted = try await store.retrieve(service: "test", account: "record")
        XCTAssertEqual(persisted, winner)
        XCTAssertEqual(backend.calls, ["update", "add", "update", "retrieve"])
    }
}

#if canImport(harness_runtimeFFI)
private final class AccountRoundtripListener: IosEventListener, @unchecked Sendable {
    func onEvent(event: ClientEvent) async {}
    func onWorkflowProgress(originSessionId: String, taskId: String, runId: String, progress: WorkflowProgressDto) async {}
}

private final class AccountRoundtripPermissions: IosPermissionSink, @unchecked Sendable {
    func onRequest(request: PermissionRequest) async {}
}
#endif

/// Inject the exact update-not-found / competing-insert / duplicate-add
/// interleaving deterministically, without touching any system credentials.
private final class FakeSecureStorageKeychainBackend: SecureStorageKeychainBackend, @unchecked Sendable {
    private let lock = NSLock()
    private var payload: Data?
    private var updateStatuses: [OSStatus]
    private var addStatus: OSStatus?
    private var insertBeforeAdd: Data?
    private var operations: [String] = []
    private var deviceOnlyAccessibility = false

    init(
        payload: Data? = nil,
        updateStatuses: [OSStatus] = [],
        addStatus: OSStatus? = nil,
        insertBeforeAdd: Data? = nil
    ) {
        self.payload = payload
        self.updateStatuses = updateStatuses
        self.addStatus = addStatus
        self.insertBeforeAdd = insertBeforeAdd
    }

    var calls: [String] {
        lock.lock()
        defer { lock.unlock() }
        return operations
    }

    var usedDeviceOnlyAccessibility: Bool {
        lock.lock()
        defer { lock.unlock() }
        return deviceOnlyAccessibility
    }

    func update(_ query: CFDictionary, attributes: CFDictionary) -> OSStatus {
        lock.lock()
        defer { lock.unlock() }
        operations.append("update")
        if !updateStatuses.isEmpty {
            let status = updateStatuses.removeFirst()
            if status != errSecSuccess { return status }
        }
        guard payload != nil else { return errSecItemNotFound }
        payload = (attributes as NSDictionary)[kSecValueData as String] as? Data
        return errSecSuccess
    }

    func add(_ query: CFDictionary) -> OSStatus {
        lock.lock()
        defer { lock.unlock() }
        operations.append("add")
        if let insertBeforeAdd {
            payload = insertBeforeAdd
            self.insertBeforeAdd = nil
        }
        if let addStatus { return addStatus }
        guard payload == nil else { return errSecDuplicateItem }
        let values = query as NSDictionary
        payload = values[kSecValueData as String] as? Data
        deviceOnlyAccessibility = values[kSecAttrAccessible as String] as? String
            == kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly as String
        return errSecSuccess
    }

    func copyMatching(_ query: CFDictionary, result: UnsafeMutablePointer<CFTypeRef?>) -> OSStatus {
        lock.lock()
        defer { lock.unlock() }
        operations.append("retrieve")
        guard let payload else { return errSecItemNotFound }
        result.pointee = payload as CFData
        return errSecSuccess
    }

    func delete(_ query: CFDictionary) -> OSStatus {
        lock.lock()
        defer { lock.unlock() }
        operations.append("delete")
        guard payload != nil else { return errSecItemNotFound }
        payload = nil
        return errSecSuccess
    }
}

private final class AccountIsolatedKeychainBackend: SecureStorageKeychainBackend, @unchecked Sendable {
    private let lock = NSLock()
    private var records: [String: [String: Data]] = [:]
    private var payloadReads = 0

    var payloadReadCount: Int {
        lock.lock(); defer { lock.unlock() }
        return payloadReads
    }

    private func identity(_ query: CFDictionary) -> (String, String)? {
        let values = query as NSDictionary
        guard values[kSecClass as String] as? String == kSecClassGenericPassword as String,
              let service = values[kSecAttrService as String] as? String,
              let account = values[kSecAttrAccount as String] as? String else { return nil }
        return (service, account)
    }

    func update(_ query: CFDictionary, attributes: CFDictionary) -> OSStatus {
        lock.lock(); defer { lock.unlock() }
        guard let (service, account) = identity(query), records[service]?[account] != nil else { return errSecItemNotFound }
        records[service]?[account] = (attributes as NSDictionary)[kSecValueData as String] as? Data
        return errSecSuccess
    }

    func add(_ query: CFDictionary) -> OSStatus {
        lock.lock(); defer { lock.unlock() }
        guard let (service, account) = identity(query),
              let data = (query as NSDictionary)[kSecValueData as String] as? Data else { return errSecParam }
        guard records[service]?[account] == nil else { return errSecDuplicateItem }
        records[service, default: [:]][account] = data
        return errSecSuccess
    }

    func copyMatching(_ query: CFDictionary, result: UnsafeMutablePointer<CFTypeRef?>) -> OSStatus {
        lock.lock(); defer { lock.unlock() }
        let values = query as NSDictionary
        if values[kSecReturnAttributes as String] as? Bool == true {
            guard values[kSecClass as String] as? String == kSecClassGenericPassword as String,
                  let service = values[kSecAttrService as String] as? String,
                  let accounts = records[service], !accounts.isEmpty else { return errSecItemNotFound }
            result.pointee = accounts.keys.map { [kSecAttrAccount as String: $0] } as CFArray
            return errSecSuccess
        }
        guard let (service, account) = identity(query), let data = records[service]?[account] else { return errSecItemNotFound }
        payloadReads += 1
        result.pointee = data as CFData
        return errSecSuccess
    }

    func delete(_ query: CFDictionary) -> OSStatus {
        lock.lock(); defer { lock.unlock() }
        guard let (service, account) = identity(query), records[service]?.removeValue(forKey: account) != nil else { return errSecItemNotFound }
        return errSecSuccess
    }
}
