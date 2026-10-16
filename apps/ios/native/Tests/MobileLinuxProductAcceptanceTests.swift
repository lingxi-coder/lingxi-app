import Foundation
import XCTest
@testable import LingxiCode

@MainActor
final class MobileLinuxProductAcceptanceTests: XCTestCase {
    func testBundledRuntimeDefaultUserProjectTerminalAndPersistenceOnDevice() async throws {
        #if targetEnvironment(simulator)
        throw XCTSkip("The complete product runtime requires a physical iPhone")
        #else
        let appRoot = ConversationSourceFactory.appSandboxRoot()
        let fixture = URL(fileURLWithPath: appRoot).appendingPathComponent("runtime-acceptance-\(UUID().uuidString)")
        let repository = ProjectRepository(projectsRoot: fixture.appendingPathComponent("Projects"))
        let project = try repository.createInternal(name: "Runtime acceptance")
        defer { try? FileManager.default.removeItem(at: fixture) }
        var state = LinuxRuntimeState()
        state.selectedMode = .mobileLinux
        let descriptor = TerminalRuntimeDescriptor.make(appSandboxRoot: appRoot, project: project, linuxRuntime: state)
        let config = try XCTUnwrap(descriptor.config)
        let runtime = try createIosMobileLinuxRuntime(config: makeIosMobileLinuxConfig(config, appSandboxRoot: appRoot))
        let status = try await runtime.boot()
        XCTAssertEqual(status.state, .ready, status.lastError ?? "runtime not ready")
        let marker = "product-acceptance-\(UUID().uuidString)"
        func command(_ script: String, cwd: String? = nil) async throws -> String {
            let result = try await runtime.runCommand(request: MobileLinuxCommandRequestFfi(
                command: "/bin/sh", args: ["-lc", "set -e; \(script)"], cwd: cwd, env: [:], stdin: nil,
                timeoutMs: 30000, network: .disabled, mounts: []))
            XCTAssertEqual(result.exitCode, 0, result.stderr)
            return result.stdout.trimmingCharacters(in: .whitespacesAndNewlines)
        }
        let identity = try await command("id -un; id -u; id -g; printf '%s\\n' \"$HOME\" \"$USER\"; pwd; sudo -n id -u; sudo -ni sh -c 'printf %s \"$HOME\"'")
        XCTAssertEqual(identity, "lingxi\n1000\n1000\n/home/lingxi\nlingxi\n/home/lingxi/lingxi\n0\n/root")
        _ = try await command("printf '\(marker)' > \"$HOME/\(marker)\"; printf project-ok > product-proof", cwd: project.workspace.guestPath)
        XCTAssertEqual(try String(contentsOf: project.workspace.hostURL.appendingPathComponent("product-proof"), encoding: .utf8), "project-ok")
        _ = try await runtime.spawnTask(request: MobileLinuxCommandRequestFfi(
            command: "/bin/sh", args: ["-lc", "id -u > background-proof"], cwd: project.workspace.guestPath,
            env: [:], stdin: nil, timeoutMs: 10000, network: .disabled, mounts: []))
        let backgroundProof = project.workspace.hostURL.appendingPathComponent("background-proof")
        let backgroundDeadline = Date().addingTimeInterval(15)
        while Date() < backgroundDeadline && (try? String(contentsOf: backgroundProof, encoding: .utf8).trimmingCharacters(in: .whitespacesAndNewlines)) != "1000" {
            try await Task.sleep(for: .milliseconds(100))
        }
        XCTAssertEqual(try String(contentsOf: backgroundProof, encoding: .utf8).trimmingCharacters(in: .whitespacesAndNewlines), "1000")
        let terminal = FfiTerminalRuntimeClient()
        let session = try await terminal.openPty(config: config, request: TerminalPtyOpenRequest(
            command: "/bin/sh", args: ["-i"], cwd: project.workspace.guestPath, env: [:], cols: 80, rows: 24, mounts: []))
        try await terminal.writePty(config: config, sessionId: session.id, data: Data("printf 'PRODUCT_PTY:%s:%s:%s\\n' \"$(id -u)\" \"$HOME\" \"$PWD\"\n".utf8))
        var output = ""
        var cursor: UInt64? = nil
        let expected = "PRODUCT_PTY:1000:/home/lingxi:\(project.workspace.guestPath)"
        let deadline = Date().addingTimeInterval(20)
        while Date() < deadline && !output.contains(expected) {
            let events = try await terminal.readEvents(config: config, afterSequence: cursor, limit: 256)
            for event in events {
                cursor = event.sequence
                output += event.text ?? event.data.map { String(decoding: $0, as: UTF8.self) } ?? ""
            }
            try await Task.sleep(for: .milliseconds(100))
        }
        try await terminal.closePty(config: config, sessionId: session.id)
        XCTAssertTrue(output.contains(expected), output)
        _ = try await runtime.shutdown()
        _ = try await runtime.boot()
        let persisted = try await command("cat \"$HOME/\(marker)\"; cp \"$HOME/\(marker)\" \"$HOME/.lingxi-product-acceptance-cold-\(marker)\"; rm \"$HOME/\(marker)\"")
        XCTAssertEqual(persisted, marker)
        _ = try await runtime.shutdown()
        print("PRODUCT_MOBILE_LINUX_ACCEPTANCE PASS ios uid=1000 home=/home/lingxi workspace=~/lingxi project=\(project.record.id)")
        #endif
    }
}
