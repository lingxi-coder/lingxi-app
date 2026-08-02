import Foundation
@testable import LingxiCode

actor FakeTerminalRuntimeClient: TerminalRuntimeClient {
    let capability: TerminalCapabilitySnapshot
    let status: TerminalStatusSnapshot
    let tasksSeed: [TerminalTaskSnapshot]
    let openSession: TerminalPtySession
    private var queuedBatches: [[TerminalStreamEvent]]
    private(set) var openRequests: [TerminalPtyOpenRequest] = []
    private(set) var eventReadCursors: [UInt64?] = []

    init(
        capability: TerminalCapabilitySnapshot = TerminalCapabilitySnapshot(
            available: false,
            backend: "stub",
            mode: .mobileLinux,
            reason: "unavailable",
            streamingOutput: false,
            backgroundProcesses: false,
            pty: false,
            bindMounts: false,
            rootfsIntegrity: false
        ),
        status: TerminalStatusSnapshot = TerminalStatusSnapshot(
            state: .unsupported,
            backend: "stub",
            mode: .mobileLinux,
            platform: "ios",
            abi: "arm64",
            version: nil,
            managedRoot: nil,
            activeRoot: nil,
            stagedRoot: nil,
            archiveSha256: nil,
            installedSizeBytes: nil,
            writableGuestPaths: [],
            lastError: nil
        ),
        tasks: [TerminalTaskSnapshot] = [],
        openSession: TerminalPtySession = TerminalPtySession(id: "pty-test", available: true, detail: nil),
        eventBatches: [[TerminalStreamEvent]] = []
    ) {
        self.capability = capability
        self.status = status
        self.tasksSeed = tasks
        self.openSession = openSession
        self.queuedBatches = eventBatches
    }

    func probe(config: TerminalRuntimeConfig?) async -> TerminalCapabilitySnapshot {
        capability
    }

    func status(config: TerminalRuntimeConfig?) async -> TerminalStatusSnapshot {
        status
    }

    func listTasks(config: TerminalRuntimeConfig?) async throws -> [TerminalTaskSnapshot] {
        tasksSeed
    }

    func openPty(config: TerminalRuntimeConfig?, request: TerminalPtyOpenRequest) async throws -> TerminalPtySession {
        openRequests.append(request)
        return openSession
    }

    func readEvents(config: TerminalRuntimeConfig?, afterSequence: UInt64?, limit: UInt32?) async throws -> [TerminalStreamEvent] {
        eventReadCursors.append(afterSequence)
        guard !queuedBatches.isEmpty else { return [] }
        return queuedBatches.removeFirst()
    }

    func writePty(config: TerminalRuntimeConfig?, sessionId: String, data: Data) async throws {}

    func resizePty(config: TerminalRuntimeConfig?, sessionId: String, cols: UInt16, rows: UInt16) async throws {}

    func closePty(config: TerminalRuntimeConfig?, sessionId: String) async throws {}

    func recordedOpenRequestCount() -> Int {
        openRequests.count
    }

    func recordedEventReadCursors() -> [UInt64?] {
        eventReadCursors
    }
}
