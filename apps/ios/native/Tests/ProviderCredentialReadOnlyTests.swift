import Foundation
import Security
import XCTest
@testable import LingxiCode

@MainActor
final class ProviderCredentialReadOnlyTests: XCTestCase {
    func testRepeatedAnthropicStatusRefreshOnlyListsAndPreservesPendingSecret() async throws {
        let sandbox = FileManager.default.temporaryDirectory.appendingPathComponent("credential-status-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: sandbox, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: sandbox) }
        let persistenceURL = sandbox.appendingPathComponent("providers.json")
        var commands: [ClientCommand] = []
        let repository = ProviderRepository(persistenceURL: persistenceURL)
        let anthropic = repository.addProfile(presetID: "anthropic")
        repository.stageSecret("unsubmitted-draft-secret", for: anthropic)
        let persistedBefore = try Data(contentsOf: persistenceURL)
        let modifiedBefore = try FileManager.default.attributesOfItem(atPath: persistenceURL.path)[.modificationDate] as? Date
        repository.configure(submitCommand: { command in
            commands.append(command)
        })

        for _ in 0..<3 {
            await repository.refreshCredentialStatus()
            guard case let .listProviderCredentials(operationID, providerIDs, previews) =
                try XCTUnwrap(commands.last)
            else { return XCTFail("Status refresh must only list credentials") }
            XCTAssertEqual(providerIDs, ["anthropic"])
            XCTAssertTrue(previews.isEmpty)
            repository.handle(event: .providerCredentialStatus(
                operationId: operationID, configuredProviderIds: ["anthropic"],
                unavailableProviderIds: [], storageEncrypted: true,
                credentialPreviews: [:], error: nil
            ))
        }

        XCTAssertEqual(commands.count, 3)
        for command in commands {
            guard case .listProviderCredentials = command else {
                return XCTFail("Refreshing must never write, delete, or submit a pending secret")
            }
        }
        XCTAssertEqual(repository.state(for: anthropic)?.pendingSecret, "unsubmitted-draft-secret")
        XCTAssertEqual(try Data(contentsOf: persistenceURL), persistedBefore)
        XCTAssertEqual(try FileManager.default.attributesOfItem(atPath: persistenceURL.path)[.modificationDate] as? Date,
                       modifiedBefore)
    }

    func testLiveAnthropicStatusRefreshDoesNotWriteExistingKeychainRecords() async throws {
        let sandbox = FileManager.default.temporaryDirectory
            .appendingPathComponent("credential-readonly-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(at: sandbox, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: sandbox) }
        let backend = ReadOnlySystemKeychainBackend()
        let before = try backend.anthropicRecords()
        print("ANTHROPIC_READONLY_EXISTING_RECORD_COUNT=\(before.count)")
        let repository = ProviderRepository(persistenceURL: sandbox.appendingPathComponent("providers.json"))
        _ = repository.addProfile(presetID: "anthropic")
        let listed = expectation(description: "Three native credential status responses")
        listed.expectedFulfillmentCount = 3
        let listener = CredentialReadOnlyListener(credentialStatus: { event in
            await MainActor.run { repository.handle(event: event) }
            listed.fulfill()
        })
        let mediaHostComposition = try IOSNativeMediaHostComposition(
            audioConfigurationJSON: try XCTUnwrap(AudioConfigurationStore.shared.mediaRuntimeConfigurationJSON)
        )
        let handle = try buildIosEngine(
            apiBase: "https://api.anthropic.com", apiKey: "", model: "",
            appSandboxRoot: sandbox.path, listener: listener,
            mediaHost: mediaHostComposition.nativeHost, share: ShareImpl(),
            notifications: NotificationImpl(), clipboard: ClipboardImpl(),
            permissions: CredentialReadOnlyPermissionSink(), mobileLinux: try mobileWorkspaceTestConfiguration(sandboxRoot: sandbox),
            secureStorage: SecureStorageImpl(backend: backend), deviceControl: nil
        )
        repository.configure(submitCommand: { command in
            guard case .listProviderCredentials = command else {
                throw NSError(domain: "ReadOnlyCredentialTest", code: 1)
            }
            try await handle.submit(command: command)
        }, oauthState: { provider in
            let state = try await handle.authState(provider: provider)
            return ProviderOAuthState(provider: state.provider, signedIn: state.signedIn,
                accountLabel: state.accountLabel, accountID: state.accountId,
                organizationID: state.organizationId, fedramp: state.fedramp)
        })
        for _ in 0..<3 { await repository.refreshCredentialStatus() }
        await fulfillment(of: [listed], timeout: 20)
        XCTAssertEqual(backend.writeAttempts, 0)
        // Boolean assertion avoids printing credential data even on a failure.
        XCTAssertTrue(try backend.anthropicRecords().isEqual(before),
                      "Existing credential bytes and Keychain attributes must remain unchanged")
    }

}

private final class CredentialReadOnlyListener: IosEventListener, @unchecked Sendable {
    private let credentialStatus: @Sendable (ClientEvent) async -> Void
    init(credentialStatus: @escaping @Sendable (ClientEvent) async -> Void) {
        self.credentialStatus = credentialStatus
    }
    func onEvent(event: ClientEvent) async {
        if case .providerCredentialStatus = event { await credentialStatus(event) }
    }
    func onWorkflowProgress(originSessionId: String, taskId: String, runId: String, progress: WorkflowProgressDto) async {}
    func onWorkflowProgress(taskId: String, runId: String, progress: WorkflowProgressDto) async {}
}

private final class CredentialReadOnlyPermissionSink: IosPermissionSink, @unchecked Sendable {
    func onRequest(request: PermissionRequest) async {}
}

/// Reads the real app Keychain but rejects and counts every attempted mutation.
private final class ReadOnlySystemKeychainBackend: SecureStorageKeychainBackend, @unchecked Sendable {
    private let lock = NSLock()
    private var attempts = 0
    var writeAttempts: Int { lock.lock(); defer { lock.unlock() }; return attempts }
    private func rejectWrite() -> OSStatus {
        lock.lock(); defer { lock.unlock() }; attempts += 1
        return errSecInteractionNotAllowed
    }
    func update(_ query: CFDictionary, attributes: CFDictionary) -> OSStatus { rejectWrite() }
    func add(_ query: CFDictionary) -> OSStatus { rejectWrite() }
    func delete(_ query: CFDictionary) -> OSStatus { rejectWrite() }
    func copyMatching(_ query: CFDictionary, result: UnsafeMutablePointer<CFTypeRef?>) -> OSStatus {
        SecItemCopyMatching(query, result)
    }
    func anthropicRecords() throws -> NSArray {
        var records: [NSDictionary] = []
        for (service, account) in [("lingxi", "anthropic-api-key"),
                                   (Keychain.service, Keychain.Item.apiKey.rawValue)] {
            let query: [String: Any] = [
                kSecClass as String: kSecClassGenericPassword,
                kSecAttrService as String: service, kSecAttrAccount as String: account,
                kSecReturnData as String: true, kSecReturnAttributes as String: true,
                kSecMatchLimit as String: kSecMatchLimitOne,
            ]
            var result: CFTypeRef?
            let status = SecItemCopyMatching(query as CFDictionary, &result)
            if status == errSecItemNotFound { continue }
            guard status == errSecSuccess, let record = result as? NSDictionary else {
                throw NSError(domain: NSOSStatusErrorDomain, code: Int(status))
            }
            records.append(record)
        }
        return records as NSArray
    }
}
