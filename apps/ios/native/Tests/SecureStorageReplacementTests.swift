import Foundation
import Security
import XCTest

@testable import LingxiCode

final class SecureStorageReplacementTests: XCTestCase {
    #if canImport(harness_runtimeFFI)
    func testEngineLaunchKeepsPersistedCredentialsOutOfScalarConfiguration() {
        let normal = EngineConfig.fromEnvironment(appSandboxRoot: "/fixture", model: "", environment: [:])
        XCTAssertTrue(normal.apiKey.isEmpty)

        let development = EngineConfig.fromEnvironment(
            appSandboxRoot: "/fixture", model: "", environment: ["ANTHROPIC_API_KEY": "fixture-development"])
        XCTAssertTrue(development.apiKey == "fixture-development")
    }

    func testLegacyMigrationKeepsCanonicalCredentialAndRetiresOldSlot() async throws {
        let fixture = LegacyCredentialMigrationFixture(canonical: "fixture-current")
        try await migrate(fixture, using: LegacyAnthropicCredentialMigration())
        let state = await fixture.snapshot()

        XCTAssertTrue(state.canonical == "fixture-current")
        XCTAssertFalse(state.hasLegacy)
        XCTAssertEqual(state.writes, 0)
        XCTAssertEqual(state.retirements, 1)
    }

    func testLegacyMigrationDoesNotResurrectAnExplicitlyDeletedCredential() async throws {
        let fixture = LegacyCredentialMigrationFixture()
        let migration = LegacyAnthropicCredentialMigration()
        try await migrate(fixture, using: migration)
        await fixture.deleteCanonical()
        try await migrate(fixture, using: migration)
        let state = await fixture.snapshot()

        XCTAssertNil(state.canonical)
        XCTAssertFalse(state.hasLegacy)
        XCTAssertEqual(state.writes, 1)
    }

    func testFailedLegacyMigrationPreservesOldSlotAndCanRetry() async throws {
        let fixture = LegacyCredentialMigrationFixture(failWrite: true)
        let migration = LegacyAnthropicCredentialMigration()
        do {
            try await migrate(fixture, using: migration)
            XCTFail("A failed canonical write must fail the bootstrap")
        } catch {}
        let failed = await fixture.snapshot()
        XCTAssertTrue(failed.hasLegacy)
        XCTAssertNil(failed.canonical)
        XCTAssertEqual(failed.retirements, 0)

        await fixture.allowWrites()
        try await migrate(fixture, using: migration)
        let recovered = await fixture.snapshot()
        XCTAssertFalse(recovered.hasLegacy)
        XCTAssertNotNil(recovered.canonical)
        XCTAssertEqual(recovered.writes, 2)
    }

    func testLegacyMigrationRequiresConfirmedStorageBeforeRetiringOldSlot() async throws {
        let fixture = LegacyCredentialMigrationFixture(confirmWrite: false)
        do {
            try await migrate(fixture, using: LegacyAnthropicCredentialMigration())
            XCTFail("Dispatch without a stored record cannot retire the old key")
        } catch {}
        let state = await fixture.snapshot()

        XCTAssertTrue(state.hasLegacy)
        XCTAssertNil(state.canonical)
        XCTAssertEqual(state.retirements, 0)
    }

    func testConcurrentBootstrapsShareOneLegacyMigration() async throws {
        let started = expectation(description: "Canonical credential migration started")
        let fixture = LegacyCredentialMigrationFixture(blockWrite: true, onWrite: { started.fulfill() })
        let migration = LegacyAnthropicCredentialMigration()
        async let first: Void = migrate(fixture, using: migration)
        async let second: Void = migrate(fixture, using: migration)
        await fulfillment(of: [started], timeout: 1)
        await fixture.releaseWrite()
        _ = try await (first, second)
        let state = await fixture.snapshot()

        XCTAssertFalse(state.hasLegacy)
        XCTAssertNotNil(state.canonical)
        XCTAssertEqual(state.writes, 1)
        XCTAssertEqual(state.retirements, 1)
    }

    private func migrate(
        _ fixture: LegacyCredentialMigrationFixture,
        using migration: LegacyAnthropicCredentialMigration
    ) async throws {
        try await migration.migrate(
            readLegacy: { await fixture.readLegacy() },
            canonicalExists: { await fixture.canonicalExists() },
            storeCanonical: { try await fixture.store($0) },
            retireLegacy: { await fixture.retireLegacy() })
    }
    #endif

    func testRepeatedAnthropicReadsNeverRewriteExistingKeychainRecord() async throws {
        let original = Data("existing-anthropic-credential-record".utf8)
        let backend = FakeSecureStorageKeychainBackend(payload: original)
        let store = SecureStorageImpl(backend: backend)

        for _ in 0..<3 {
            let retrieved = try await store.retrieve(service: "lingxi", account: "anthropic-api-key")
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
private actor LegacyCredentialMigrationFixture {
    struct Snapshot: Sendable {
        let hasLegacy: Bool
        let canonical: String?
        let writes: Int
        let retirements: Int
    }

    private var legacy: String? = "fixture-legacy"
    private var canonical: String?
    private var failWrite: Bool
    private let confirmWrite: Bool
    private let blockWrite: Bool
    private let onWrite: @Sendable () -> Void
    private var writeGate: CheckedContinuation<Void, Never>?
    private var writes = 0
    private var retirements = 0

    init(
        canonical: String? = nil,
        failWrite: Bool = false,
        confirmWrite: Bool = true,
        blockWrite: Bool = false,
        onWrite: @escaping @Sendable () -> Void = {}
    ) {
        self.canonical = canonical
        self.failWrite = failWrite
        self.confirmWrite = confirmWrite
        self.blockWrite = blockWrite
        self.onWrite = onWrite
    }

    func readLegacy() -> String? { legacy }
    func canonicalExists() -> Bool { canonical != nil }
    func allowWrites() { failWrite = false }
    func deleteCanonical() { canonical = nil }
    func retireLegacy() { legacy = nil; retirements += 1 }
    func releaseWrite() { writeGate?.resume(); writeGate = nil }

    func store(_ secret: String) async throws {
        writes += 1
        if blockWrite {
            await withCheckedContinuation { continuation in
                writeGate = continuation
                onWrite()
            }
        } else { onWrite() }
        if failWrite { throw CancellationError() }
        if confirmWrite { canonical = secret }
    }

    func snapshot() -> Snapshot {
        Snapshot(hasLegacy: legacy != nil, canonical: canonical, writes: writes, retirements: retirements)
    }
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
