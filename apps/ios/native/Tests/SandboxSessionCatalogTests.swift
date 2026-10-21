import Foundation
import XCTest
#if canImport(LingxiCode)
@testable import LingxiCode
#endif

final class SandboxSessionCatalogTests: XCTestCase {
    private let oldRoot = "/private/var/mobile/Containers/Data/Application/11111111-1111-1111-1111-111111111111/Library/Application Support/LingxiCode"
    private let newRoot = "/private/var/mobile/Containers/Data/Application/22222222-2222-2222-2222-222222222222/Library/Application Support/LingxiCode"
    private let session = "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa"
    private var temporary: URL!

    override func setUpWithError() throws {
        temporary = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: temporary, withIntermediateDirectories: true)
    }

    override func tearDownWithError() throws {
        try FileManager.default.removeItem(at: temporary)
    }

    private func file(_ cwd: String, name: String? = nil, bytes: Data) throws -> URL {
        let directory = temporary.appendingPathComponent(SandboxSessionCatalog.directoryName(cwd))
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        let result = directory.appendingPathComponent(name ?? (session + ".jsonl"))
        try bytes.write(to: result)
        return result
    }

    func testRebindPreservesRawHistorySidecarsAndIsIdempotent() throws {
        let raw = Data("{\"type\":\"user\",\"text\":\"\\ud800\"}\n".utf8)
        let original = try file(oldRoot, bytes: raw)
        let identity = try file(oldRoot, name: session + ".jsonl.identity-log", bytes: Data("identity\n".utf8))
        try SandboxSessionCatalog.rebind(projects: temporary, currentAppRoot: newRoot, workspaceSuffixes: [""])
        let copied = temporary.appendingPathComponent(SandboxSessionCatalog.directoryName(newRoot)).appendingPathComponent(session + ".jsonl")
        let result = try Data(contentsOf: copied)
        XCTAssertTrue(result.starts(with: raw))
        let line = result.split(separator: 10).last!
        let marker = try XCTUnwrap(JSONSerialization.jsonObject(with: Data(line)) as? [String: String])
        XCTAssertEqual(marker, ["type": "relocated", "relocatedCwd": newRoot, "sessionId": session])
        XCTAssertEqual(try Data(contentsOf: original), raw)
        let copiedIdentity = copied.deletingLastPathComponent().appendingPathComponent(identity.lastPathComponent)
        XCTAssertEqual(try Data(contentsOf: copiedIdentity), try Data(contentsOf: identity))
        try SandboxSessionCatalog.rebind(projects: temporary, currentAppRoot: newRoot, workspaceSuffixes: [""])
        XCTAssertEqual(try Data(contentsOf: copied), result)
    }

    func testPrefixCopiesSelectCompleteHistoryBeforeAppendingMarker() throws {
        let raw = Data("anchor\nuser\nassistant\n".utf8)
        _ = try file(oldRoot, bytes: Data("anchor\n".utf8))
        let thirdRoot = oldRoot.replacingOccurrences(of: "11111111-1111-1111-1111-111111111111", with: "33333333-3333-3333-3333-333333333333")
        _ = try file(thirdRoot, bytes: raw)
        let current = try file(newRoot, bytes: Data("anchor\nuser\n".utf8))
        try SandboxSessionCatalog.rebind(projects: temporary, currentAppRoot: newRoot, workspaceSuffixes: [""])
        XCTAssertTrue(try Data(contentsOf: current).starts(with: raw))
    }

    func testConflictsLeaveEveryOriginalAndDestinationUntouched() throws {
        let old = try file(oldRoot, bytes: Data("old branch\n".utf8))
        let current = try file(newRoot, bytes: Data("current branch\n".utf8))
        _ = try file(oldRoot, name: "unique.json", bytes: Data("do not partially copy".utf8))
        XCTAssertThrowsError(try SandboxSessionCatalog.rebind(projects: temporary, currentAppRoot: newRoot, workspaceSuffixes: [""]))
        XCTAssertEqual(try Data(contentsOf: old), Data("old branch\n".utf8))
        XCTAssertEqual(try Data(contentsOf: current), Data("current branch\n".utf8))
        XCTAssertFalse(FileManager.default.fileExists(atPath: current.deletingLastPathComponent().appendingPathComponent("unique.json").path))
    }

    func testCompleteConversationSupersedesItsEmptyPlaceholderWithoutReencoding() throws {
        let anchor = Data(("{\"type\":\"custom-title\",\"customTitle\":\"New\",\"sessionId\":\"" + session + "\",\"mobileEmptySession\":1}\n").utf8)
        let placeholder = anchor + Data(("{\"type\":\"permission-mode\",\"permissionMode\":\"default\",\"sessionId\":\"" + session + "\"}\n").utf8)
        let original = try file(oldRoot, bytes: placeholder)
        let raw = anchor + Data("{\"type\":\"user\",\"text\":\"\\ud800\"}\n".utf8)
        let thirdRoot = oldRoot.replacingOccurrences(of: "11111111-1111-1111-1111-111111111111", with: "33333333-3333-3333-3333-333333333333")
        _ = try file(thirdRoot, bytes: raw)
        try SandboxSessionCatalog.rebind(projects: temporary, currentAppRoot: newRoot, workspaceSuffixes: [""])
        let current = temporary.appendingPathComponent(SandboxSessionCatalog.directoryName(newRoot)).appendingPathComponent(session + ".jsonl")
        XCTAssertTrue(try Data(contentsOf: current).starts(with: raw))
        XCTAssertEqual(try Data(contentsOf: original), placeholder)
    }

    func testManagedWorkspacesStaySeparateAndLongPathsUseRuntimeHash() throws {
        let project = "bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb"
        let suffix = "/Projects/\(project)/workspace"
        let global = Data("global\n".utf8)
        let scoped = Data("scoped\n".utf8)
        _ = try file(oldRoot, bytes: global)
        _ = try file(oldRoot + suffix, bytes: scoped)
        let excluded = try file(oldRoot + "/external/workspace", bytes: Data("excluded".utf8))
        try SandboxSessionCatalog.rebind(projects: temporary, currentAppRoot: newRoot, workspaceSuffixes: ["", suffix])
        for (tail, expected) in [("", global), (suffix, scoped)] {
            let target = temporary.appendingPathComponent(SandboxSessionCatalog.directoryName(newRoot + tail)).appendingPathComponent(session + ".jsonl")
            XCTAssertTrue(try Data(contentsOf: target).starts(with: expected))
        }
        XCTAssertEqual(try Data(contentsOf: excluded), Data("excluded".utf8))
        XCTAssertFalse(FileManager.default.fileExists(atPath: temporary.appendingPathComponent(SandboxSessionCatalog.directoryName(newRoot + "/external/workspace")).path))
        // Independent fixture for the Runtime's 200-character / signed i32 rule.
        XCTAssertEqual(SandboxSessionCatalog.directoryName("/" + String(repeating: "x", count: 220)), "-" + String(repeating: "x", count: 199) + "-7q0khb")
    }

    func testSymlinksCannotReadOrReplaceDataOutsideTheCatalog() throws {
        _ = try file(oldRoot, bytes: Data("history\n".utf8))
        let outside = temporary.appendingPathComponent("outside")
        try Data("preserve".utf8).write(to: outside)
        let directory = temporary.appendingPathComponent(SandboxSessionCatalog.directoryName(newRoot))
        try FileManager.default.createSymbolicLink(at: directory, withDestinationURL: temporary)
        XCTAssertThrowsError(try SandboxSessionCatalog.rebind(projects: temporary, currentAppRoot: newRoot, workspaceSuffixes: [""]))
        XCTAssertEqual(try Data(contentsOf: outside), Data("preserve".utf8))
    }
}
