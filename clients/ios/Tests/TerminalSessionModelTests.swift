import XCTest
@testable import LingxiCode

@MainActor
final class TerminalSessionModelTests: XCTestCase {
    func testStartIfNeededRejectsWrongWorkspaceInsteadOfFallingBack() async {
        let client = FakeTerminalRuntimeClient(
            capability: TerminalCapabilitySnapshot(
                available: true,
                backend: "ios-ish",
                mode: .mobileLinux,
                reason: nil,
                streamingOutput: true,
                backgroundProcesses: true,
                pty: true,
                bindMounts: true,
                rootfsIntegrity: true
            ),
            status: TerminalStatusSnapshot(
                state: .ready,
                backend: "ios-ish",
                mode: .mobileLinux,
                platform: "ios",
                abi: "arm64",
                version: "1.0.0",
                managedRoot: nil,
                activeRoot: nil,
                stagedRoot: nil,
                archiveSha256: nil,
                installedSizeBytes: nil,
                writableGuestPaths: ["/workspace/other"],
                lastError: nil
            )
        )
        let model = TerminalSessionModel(
            descriptor: TerminalRuntimeDescriptor(
                config: nil,
                workspace: TerminalWorkspaceDescriptor(hostPath: nil, guestPath: "/workspace/project", displayName: "Project")
            ),
            client: client
        )

        await model.startIfNeeded()

        XCTAssertEqual(model.availability, .workspaceUnavailable("当前项目 workspace 未挂载到 guest，已拒绝回退到错误工作区"))
        XCTAssertNil(model.activeSessionID)
        let openRequestCount = await client.recordedOpenRequestCount()
        XCTAssertEqual(openRequestCount, 0)
    }

    func testLifecycleConsumesEventsAndMarksExit() async {
        let client = FakeTerminalRuntimeClient(
            capability: TerminalCapabilitySnapshot(
                available: true,
                backend: "ios-ish",
                mode: .mobileLinux,
                reason: nil,
                streamingOutput: true,
                backgroundProcesses: true,
                pty: true,
                bindMounts: true,
                rootfsIntegrity: true
            ),
            status: TerminalStatusSnapshot(
                state: .ready,
                backend: "ios-ish",
                mode: .mobileLinux,
                platform: "ios",
                abi: "arm64",
                version: "1.0.0",
                managedRoot: nil,
                activeRoot: nil,
                stagedRoot: nil,
                archiveSha256: nil,
                installedSizeBytes: nil,
                writableGuestPaths: ["/workspace/project"],
                lastError: nil
            ),
            tasks: [TerminalTaskSnapshot(id: "t1", title: "shell", state: .running, detail: nil)],
            openSession: TerminalPtySession(id: "pty-1", available: true, detail: nil),
            eventBatches: [[
                TerminalStreamEvent(
                    sequence: 1,
                    taskId: "t1",
                    streamId: "pty-1",
                    source: .pty,
                    kind: .stdoutLine,
                    text: "hello\n",
                    data: nil,
                    exitCode: nil,
                    timedOut: false
                ),
                TerminalStreamEvent(
                    sequence: 2,
                    taskId: "t1",
                    streamId: "pty-1",
                    source: .pty,
                    kind: .exit,
                    text: nil,
                    data: nil,
                    exitCode: 0,
                    timedOut: false
                ),
            ]]
        )
        let model = TerminalSessionModel(
            descriptor: TerminalRuntimeDescriptor(
                config: nil,
                workspace: TerminalWorkspaceDescriptor(hostPath: nil, guestPath: "/workspace/project", displayName: "Project"),
                pollInterval: .milliseconds(1),
                taskRefreshInterval: 1
            ),
            client: client
        )

        await model.startIfNeeded()
        try? await Task.sleep(for: .milliseconds(30))

        XCTAssertEqual(model.buffer.plainText, "hello\n")
        XCTAssertEqual(model.lastExitStatus, TerminalExitStatus(code: 0, timedOut: false))
        XCTAssertEqual(model.availability, .closed)
        XCTAssertFalse(model.isPolling)

        let readsAfterExit = await client.recordedEventReadCursors().count
        try? await Task.sleep(for: .milliseconds(10))
        let readsAfterSettling = await client.recordedEventReadCursors().count
        XCTAssertEqual(readsAfterSettling, readsAfterExit)
    }

    func testPollingAdvancesGlobalCursorPastUnrelatedStreams() async {
        let client = FakeTerminalRuntimeClient(
            capability: TerminalCapabilitySnapshot(
                available: true,
                backend: "ios-ish",
                mode: .mobileLinux,
                reason: nil,
                streamingOutput: true,
                backgroundProcesses: true,
                pty: true,
                bindMounts: true,
                rootfsIntegrity: true
            ),
            status: TerminalStatusSnapshot(
                state: .ready,
                backend: "ios-ish",
                mode: .mobileLinux,
                platform: "ios",
                abi: "arm64",
                version: "1.0.0",
                managedRoot: nil,
                activeRoot: nil,
                stagedRoot: nil,
                archiveSha256: nil,
                installedSizeBytes: nil,
                writableGuestPaths: ["/workspace/project"],
                lastError: nil
            ),
            openSession: TerminalPtySession(id: "pty-current", available: true, detail: nil),
            eventBatches: [
                [
                    TerminalStreamEvent(
                        sequence: 41,
                        taskId: nil,
                        streamId: "runtime",
                        source: .run,
                        kind: .error,
                        text: "unrelated runtime event",
                        data: nil,
                        exitCode: nil,
                        timedOut: false
                    )
                ],
                [
                    TerminalStreamEvent(
                        sequence: 42,
                        taskId: nil,
                        streamId: "pty-current",
                        source: .pty,
                        kind: .stdoutLine,
                        text: "ready\n",
                        data: nil,
                        exitCode: nil,
                        timedOut: false
                    ),
                    TerminalStreamEvent(
                        sequence: 43,
                        taskId: nil,
                        streamId: "pty-current",
                        source: .pty,
                        kind: .exit,
                        text: nil,
                        data: nil,
                        exitCode: 0,
                        timedOut: false
                    ),
                ],
            ]
        )
        let model = TerminalSessionModel(
            descriptor: TerminalRuntimeDescriptor(
                config: nil,
                workspace: TerminalWorkspaceDescriptor(
                    hostPath: nil,
                    guestPath: "/workspace/project",
                    displayName: "Project"
                ),
                pollInterval: .milliseconds(1)
            ),
            client: client
        )

        await model.startIfNeeded()
        try? await Task.sleep(for: .milliseconds(30))

        XCTAssertEqual(model.buffer.plainText, "ready\n")
        XCTAssertEqual(model.lastSequence, 43)
        let cursors = await client.recordedEventReadCursors()
        XCTAssertGreaterThanOrEqual(cursors.count, 2)
        XCTAssertNil(cursors[0])
        XCTAssertEqual(cursors[1], 41)
    }

    func testLifecyclePrefersBinaryPayloadWhenTextAndDataBothPresent() async {
        let client = FakeTerminalRuntimeClient(
            capability: TerminalCapabilitySnapshot(
                available: true,
                backend: "ios-ish",
                mode: .mobileLinux,
                reason: nil,
                streamingOutput: true,
                backgroundProcesses: true,
                pty: true,
                bindMounts: true,
                rootfsIntegrity: true
            ),
            status: TerminalStatusSnapshot(
                state: .ready,
                backend: "ios-ish",
                mode: .mobileLinux,
                platform: "ios",
                abi: "arm64",
                version: "1.0.0",
                managedRoot: nil,
                activeRoot: nil,
                stagedRoot: nil,
                archiveSha256: nil,
                installedSizeBytes: nil,
                writableGuestPaths: ["/workspace/project"],
                lastError: nil
            ),
            openSession: TerminalPtySession(id: "pty-dup", available: true, detail: nil),
            eventBatches: [[
                TerminalStreamEvent(
                    sequence: 1,
                    taskId: nil,
                    streamId: "pty-dup",
                    source: .pty,
                    kind: .stdoutLine,
                    text: "dup",
                    data: Data("dup".utf8),
                    exitCode: nil,
                    timedOut: false
                )
            ]]
        )
        let model = TerminalSessionModel(
            descriptor: TerminalRuntimeDescriptor(
                config: nil,
                workspace: TerminalWorkspaceDescriptor(hostPath: nil, guestPath: "/workspace/project", displayName: "Project"),
                pollInterval: .milliseconds(1)
            ),
            client: client
        )

        await model.startIfNeeded()
        try? await Task.sleep(for: .milliseconds(20))

        XCTAssertEqual(model.buffer.plainText, "dup")
    }

    func testInvalidRequestedCwdIsSurfacedBeforeOpen() async {
        let client = FakeTerminalRuntimeClient(
            capability: TerminalCapabilitySnapshot(
                available: true,
                backend: "ios-ish",
                mode: .mobileLinux,
                reason: nil,
                streamingOutput: true,
                backgroundProcesses: true,
                pty: true,
                bindMounts: true,
                rootfsIntegrity: true
            ),
            status: TerminalStatusSnapshot(
                state: .ready,
                backend: "ios-ish",
                mode: .mobileLinux,
                platform: "ios",
                abi: "arm64",
                version: "1.0.0",
                managedRoot: nil,
                activeRoot: nil,
                stagedRoot: nil,
                archiveSha256: nil,
                installedSizeBytes: nil,
                writableGuestPaths: ["/workspace/project"],
                lastError: nil
            )
        )
        let descriptor = TerminalRuntimeDescriptor(
            config: nil,
            workspace: TerminalWorkspaceDescriptor(hostPath: nil, guestPath: "/workspace/project", displayName: "Project"),
            launchCwd: nil,
            invalidRequestedCwdMessage: "请求的 cwd 超出当前项目 workspace：/tmp"
        )
        let model = TerminalSessionModel(descriptor: descriptor, client: client)

        await model.startIfNeeded()

        XCTAssertEqual(model.availability, .invalidRequest("请求的 cwd 超出当前项目 workspace：/tmp"))
        let openRequestCount = await client.recordedOpenRequestCount()
        XCTAssertEqual(openRequestCount, 0)
    }
}
