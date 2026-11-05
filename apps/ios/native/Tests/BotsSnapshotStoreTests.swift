import XCTest
@testable import LingxiCode

final class BotsSnapshotStoreTests: XCTestCase {
    @MainActor func testPersistenceForgetAndAccountIsolation() async throws {
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        let suite = "BotsTests." + UUID().uuidString
        let defaults = UserDefaults(suiteName: suite)!
        defer { try? FileManager.default.removeItem(at: directory); defaults.removePersistentDomain(forName: suite) }
        let files = BotsSnapshotFiles(directory: directory)
        let store = BotsSnapshotStore(files: files, defaults: defaults)
        await store.configure(uid: "one")
        store.receive(bots: [BotsItem(id: "b", name: "Bot")], hostID: "h", nowMs: 1)
        store.receive(chats: [BotsChat(id: "new"), BotsChat(id: "old")], hostID: "h", botID: "b", nowMs: 2)
        store.connected("h"); store.toggleHost("h"); store.toggleBot("h/b")
        await store.flush()

        let restored = BotsSnapshotStore(files: files, defaults: defaults)
        await restored.configure(uid: "one")
        XCTAssertEqual(restored.snapshots["h"]?.bots.first?.chats?.map(\.id), ["new", "old"])
        XCTAssertEqual(restored.lastHostID, "h")
        XCTAssertTrue(restored.collapsedHosts.contains("h")); XCTAssertTrue(restored.expandedBots.contains("h/b"))
        await restored.forget("h")
        XCTAssertTrue(restored.snapshots.isEmpty); XCTAssertNil(restored.lastHostID)
        XCTAssertTrue(restored.expandedBots.isEmpty)
        XCTAssertTrue(try FileManager.default.contentsOfDirectory(atPath: directory.path).isEmpty)

        restored.receive(bots: [BotsItem(id: "private")], hostID: "h", nowMs: 3)
        await restored.flush()
        await restored.configure(uid: "two")
        XCTAssertTrue(restored.snapshots.isEmpty)
        XCTAssertNil(defaults.object(forKey: "bots.lastHost.one"))
        await restored.configure(uid: nil)
        XCTAssertTrue(try FileManager.default.contentsOfDirectory(atPath: directory.path).isEmpty)
    }

    func testStaleAccountConfigurationCannotDeleteNewUsersSnapshots() async throws {
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: directory) }
        let files = BotsSnapshotFiles(directory: directory)
        _ = try await files.configure(uid: "new", generation: 2)
        let snapshot = BotsSnapshot(uid: "new", hostID: "h", savedAtMs: 2, bots: [BotsItem(id: "new")])
        try await files.save([snapshot], uid: "new", generation: 2)
        _ = try await files.configure(uid: "old", generation: 1)
        let loaded = try await files.configure(uid: "new", generation: 3)
        XCTAssertEqual(loaded["h"], snapshot)
    }

    func testLateSaveCannotRecreateSignedOutUsersFiles() async throws {
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: directory) }
        let files = BotsSnapshotFiles(directory: directory)
        _ = try await files.configure(uid: "one", generation: 1)
        let snapshot = BotsSnapshot(uid: "one", hostID: "h", savedAtMs: 1, bots: [])
        _ = try await files.configure(uid: nil, generation: 2)
        try await files.save([snapshot], uid: "one", generation: 1)
        XCTAssertTrue(try FileManager.default.contentsOfDirectory(atPath: directory.path).isEmpty)
    }
}
