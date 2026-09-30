import Foundation
import Security
import XCTest

@testable import LingxiCode

final class SecureStorageReplacementTests: XCTestCase {
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
