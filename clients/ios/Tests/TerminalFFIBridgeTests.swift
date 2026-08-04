import Foundation
import XCTest
@testable import LingxiCode

@MainActor
final class TerminalFFIBridgeTests: XCTestCase {
    func testFfiClientReusesOneHandleAcrossPtyLifecycle() async throws {
        let bridge = FakeTerminalRuntimeBridge()
        let client = FfiTerminalRuntimeClient(bridge: bridge)
        let config = TerminalRuntimeConfig(
            mode: .mobileLinux,
            managedRoot: "/managed",
            workspaceHostPath: "/workspace-host",
            stableWorkspaceId: "project",
            abi: "arm64",
            rootfsVersion: "1.0.0",
            archiveSha256: nil,
            authorizationFile: nil
        )
        let request = TerminalPtyOpenRequest(
            command: "/bin/sh",
            args: ["-i"],
            cwd: "/workspace/project",
            env: [:],
            cols: 80,
            rows: 24,
            mounts: []
        )

        _ = await client.probe(config: config)
        _ = await client.status(config: config)
        _ = try await client.openPty(config: config, request: request)
        _ = try await client.readEvents(config: config, afterSequence: nil, limit: nil)
        try await client.writePty(config: config, sessionId: "pty-1", data: Data("pwd\n".utf8))
        try await client.resizePty(config: config, sessionId: "pty-1", cols: 100, rows: 30)
        try await client.closePty(config: config, sessionId: "pty-1")

        let snapshot = bridge.snapshot()
        XCTAssertEqual(snapshot.createCount, 1)
        XCTAssertEqual(snapshot.handleOpenCount, 1)
        XCTAssertEqual(snapshot.handleReadCount, 1)
        XCTAssertEqual(snapshot.handleWriteCount, 1)
        XCTAssertEqual(snapshot.handleResizeCount, 1)
        XCTAssertEqual(snapshot.handleCloseCount, 1)
        XCTAssertEqual(snapshot.handleStatusCount, 1)
        XCTAssertEqual(snapshot.handleCapabilityCount, 1)
    }

    func testDescriptorMakeRejectsRequestedCwdOutsideWorkspace() {
        let project = ProjectSnapshot(
            record: ProjectRecord(
                id: "12345678-1234-4abc-8def-1234567890ab",
                name: "Project",
                storageKind: .internal,
                createdAt: .distantPast,
                updatedAt: .distantPast
            ),
            workspace: ProjectWorkspace(
                projectId: "12345678-1234-4abc-8def-1234567890ab",
                hostURL: URL(fileURLWithPath: "/tmp/project", isDirectory: true)
            ),
            sessions: []
        )
        var runtime = LinuxRuntimeState()
        runtime.selectedMode = .mobileLinux

        let descriptor = TerminalRuntimeDescriptor.make(
            appSandboxRoot: "/tmp/app",
            project: project,
            linuxRuntime: runtime,
            requestedCwd: .guestPath("/tmp")
        )

        XCTAssertNil(descriptor.launchCwd)
        XCTAssertEqual(descriptor.invalidRequestedCwdMessage, "请求的 cwd 超出 workspace：/tmp")
    }

    func testDescriptorMakeFallsBackToLinuxRuntimeDraftCommand() {
        let project = ProjectSnapshot(
            record: ProjectRecord(
                id: "12345678-1234-4abc-8def-1234567890ab",
                name: "Project",
                storageKind: .internal,
                createdAt: .distantPast,
                updatedAt: .distantPast
            ),
            workspace: ProjectWorkspace(
                projectId: "12345678-1234-4abc-8def-1234567890ab",
                hostURL: URL(fileURLWithPath: "/tmp/project", isDirectory: true)
            ),
            sessions: []
        )
        var runtime = LinuxRuntimeState()
        runtime.selectedMode = .mobileLinux
        runtime.terminal.draftCommand = "python3 --version"

        let descriptor = TerminalRuntimeDescriptor.make(
            appSandboxRoot: "/tmp/app",
            project: project,
            linuxRuntime: runtime,
            initialCommand: nil,
            requestedCwd: .workspaceRelative("src")
        )

        XCTAssertEqual(descriptor.initialCommand, "python3 --version")
        XCTAssertEqual(descriptor.requestedCwdDisplay, "src")
        XCTAssertEqual(descriptor.launchCwd, "/workspace/12345678-1234-4abc-8def-1234567890ab/src")
    }
}

private final class FakeTerminalRuntimeBridge: TerminalRuntimeFFIBridge, @unchecked Sendable {
    private let handle = FakeTerminalRuntimeHandle()
    private var createCount = 0

    func createRuntime(config: TerminalRuntimeConfig) throws -> any TerminalRuntimeHandle {
        createCount += 1
        return handle
    }

    func probe(config: TerminalRuntimeConfig?) -> MobileLinuxCapabilityFfi {
        MobileLinuxCapabilityFfi(
            available: true,
            backend: "ios-ish",
            mode: .mobileLinux,
            reason: nil,
            streamingOutput: true,
            backgroundProcesses: true,
            pty: true,
            bindMounts: true,
            rootfsIntegrity: true
        )
    }

    func status(config: TerminalRuntimeConfig?) -> MobileLinuxStatusFfi {
        MobileLinuxStatusFfi(
            state: .ready,
            backend: "ios-ish",
            mode: .mobileLinux,
            platform: "ios",
            abi: "arm64",
            version: "1.0.0",
            managedRoot: "/managed",
            activeRoot: nil,
            stagedRoot: nil,
            archiveSha256: nil,
            installedSizeBytes: nil,
            writableGuestPaths: ["/workspace/project"],
            lastError: nil
        )
    }

    func snapshot() -> BridgeSnapshot {
        BridgeSnapshot(
            createCount: createCount,
            handleCapabilityCount: handle.capabilityCount,
            handleStatusCount: handle.statusCount,
            handleOpenCount: handle.openCount,
            handleReadCount: handle.readCount,
            handleWriteCount: handle.writeCount,
            handleResizeCount: handle.resizeCount,
            handleCloseCount: handle.closeCount
        )
    }
}

private struct BridgeSnapshot: Equatable {
    var createCount: Int
    var handleCapabilityCount: Int
    var handleStatusCount: Int
    var handleOpenCount: Int
    var handleReadCount: Int
    var handleWriteCount: Int
    var handleResizeCount: Int
    var handleCloseCount: Int
}

private final class FakeTerminalRuntimeHandle: @unchecked Sendable, TerminalRuntimeHandle {
    private(set) var capabilityCount = 0
    private(set) var statusCount = 0
    private(set) var openCount = 0
    private(set) var readCount = 0
    private(set) var writeCount = 0
    private(set) var resizeCount = 0
    private(set) var closeCount = 0

    func capability() async -> MobileLinuxCapabilityFfi {
        capabilityCount += 1
        return MobileLinuxCapabilityFfi(
            available: true,
            backend: "ios-ish",
            mode: .mobileLinux,
            reason: nil,
            streamingOutput: true,
            backgroundProcesses: true,
            pty: true,
            bindMounts: true,
            rootfsIntegrity: true
        )
    }
    func closePty(sessionId: String) async throws { closeCount += 1 }
    func listTasks() async throws -> [MobileLinuxTaskFfi] { [] }
    func openPty(request: MobileLinuxPtyOpenRequestFfi) async throws -> MobileLinuxPtySessionFfi {
        openCount += 1
        return MobileLinuxPtySessionFfi(id: "pty-1", available: true, detail: nil)
    }
    func readEvents(afterSequence: UInt64?, limit: UInt32?) async throws -> [MobileLinuxStreamEventFfi] {
        readCount += 1
        return []
    }
    func resizePty(sessionId: String, cols: UInt16, rows: UInt16) async throws { resizeCount += 1 }
    func status() async throws -> MobileLinuxStatusFfi {
        statusCount += 1
        return MobileLinuxStatusFfi(
            state: .ready,
            backend: "ios-ish",
            mode: .mobileLinux,
            platform: "ios",
            abi: "arm64",
            version: "1.0.0",
            managedRoot: "/managed",
            activeRoot: nil,
            stagedRoot: nil,
            archiveSha256: nil,
            installedSizeBytes: nil,
            writableGuestPaths: ["/workspace/project"],
            lastError: nil
        )
    }
    func writePty(sessionId: String, data: Data) async throws { writeCount += 1 }
}
