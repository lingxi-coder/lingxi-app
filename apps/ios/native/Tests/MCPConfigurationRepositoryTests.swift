import Foundation
import XCTest

@testable import LingxiCode

@MainActor
final class MCPConfigurationRepositoryTests: XCTestCase {
    private var temporaryDirectory: URL!

    override func setUpWithError() throws {
        temporaryDirectory = FileManager.default.temporaryDirectory
            .appendingPathComponent("mcp-repository-tests-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(
            at: temporaryDirectory,
            withIntermediateDirectories: true
        )
    }

    override func tearDownWithError() throws {
        if let temporaryDirectory {
            try? FileManager.default.removeItem(at: temporaryDirectory)
        }
    }

    func testSaveRefusesToOverwriteMalformedSettings() throws {
        let settingsURL = temporaryDirectory
            .appendingPathComponent(".lingxi", isDirectory: true)
            .appendingPathComponent("settings.json")
        try FileManager.default.createDirectory(
            at: settingsURL.deletingLastPathComponent(),
            withIntermediateDirectories: true
        )
        let original = Data("{ this is not JSON".utf8)
        try original.write(to: settingsURL)

        let repository = MCPConfigurationRepository(appSandboxRoot: temporaryDirectory)

        XCTAssertThrowsError(try repository.save(server()))
        XCTAssertEqual(try Data(contentsOf: settingsURL), original)
    }

    func testWebSocketTransportUsesCanonicalConfigType() throws {
        let settingsURL = temporaryDirectory
            .appendingPathComponent(".lingxi", isDirectory: true)
            .appendingPathComponent("settings.json")
        try FileManager.default.createDirectory(
            at: settingsURL.deletingLastPathComponent(),
            withIntermediateDirectories: true
        )
        let initial = #"{"mcpServers":{"socket":{"type":"websocket","url":"ws://example.test"}}}"#
        try Data(initial.utf8).write(to: settingsURL)

        let repository = MCPConfigurationRepository(appSandboxRoot: temporaryDirectory)
        let loaded = try XCTUnwrap(repository.loadServers().first)
        XCTAssertEqual(loaded.transport, "ws")

        try repository.save(loaded)

        let object = try XCTUnwrap(
            JSONSerialization.jsonObject(with: Data(contentsOf: settingsURL)) as? [String: Any]
        )
        let servers = try XCTUnwrap(object["mcpServers"] as? [String: Any])
        let socket = try XCTUnwrap(servers["socket"] as? [String: Any])
        XCTAssertEqual(socket["type"] as? String, "ws")
    }

    private func server() -> MCPServer {
        MCPServer(
            id: "new-server",
            name: "server",
            url: "https://example.test/mcp",
            command: "",
            args: [],
            env: [:],
            headers: [:],
            tools: nil,
            status: .idle,
            enabled: true,
            transport: "http",
            scope: "user"
        )
    }
}
