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
    private(set) var writes: [Data] = []
    private(set) var closedSessions: [String] = []
    /// The real bridge grants ONE interactive PTY per managed root; a fake
    /// that opened unlimited sessions kept the restart tests green even with
    /// the close-before-reopen ordering deleted, because the exact prod
    /// failure ("only one interactive PTY session…") could never occur here.
    private var activePtyID: String?
    private var openCount = 0
    /// Lets a test park `closePty` mid-flight so teardown races (close during
    /// restart) become schedulable instead of timing-dependent.
    let closePtyDelay: Duration

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
        eventBatches: [[TerminalStreamEvent]] = [],
        closePtyDelay: Duration = .zero
    ) {
        self.capability = capability
        self.status = status
        self.tasksSeed = tasks
        self.openSession = openSession
        self.queuedBatches = eventBatches
        self.closePtyDelay = closePtyDelay
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
        guard activePtyID == nil else {
            throw TerminalRuntimeError.unavailable(
                "only one interactive PTY session is supported per managed root"
            )
        }
        openCount += 1
        // The first open returns the seeded id, which the queued fixtures
        // address by `streamId`. Reopens get a deterministic suffix
        // ("<id>-r2", "<id>-r3", …) so a recycled id cannot satisfy the
        // model's stale-loop guards by accident, as it never can in prod.
        let id = openCount == 1 ? openSession.id : "\(openSession.id)-r\(openCount)"
        if openSession.available { activePtyID = id }
        return TerminalPtySession(id: id, available: openSession.available, detail: openSession.detail)
    }

    func readEvents(config: TerminalRuntimeConfig?, afterSequence: UInt64?, limit: UInt32?) async throws -> [TerminalStreamEvent] {
        eventReadCursors.append(afterSequence)
        guard !queuedBatches.isEmpty else { return [] }
        let batch = queuedBatches.removeFirst()
        // The (post-fix) runtime releases the one-PTY slot when a session
        // dies: the reader closes natively on death, and a clean `pty_closed`
        // only ever comes from an explicit close.
        if batch.contains(where: { $0.kind == .exit && $0.streamId == activePtyID }) {
            activePtyID = nil
        }
        return batch
    }

    func writePty(config: TerminalRuntimeConfig?, sessionId: String, data: Data) async throws {
        writes.append(data)
    }

    func resizePty(config: TerminalRuntimeConfig?, sessionId: String, cols: UInt16, rows: UInt16) async throws {}

    func closePty(config: TerminalRuntimeConfig?, sessionId: String) async throws {
        if closePtyDelay > .zero {
            try? await Task.sleep(for: closePtyDelay)
        }
        closedSessions.append(sessionId)
        if activePtyID == sessionId { activePtyID = nil }
    }

    func recordedOpenRequestCount() -> Int {
        openRequests.count
    }

    func recordedClosedSessions() -> [String] {
        closedSessions
    }

    func currentActivePtyID() -> String? {
        activePtyID
    }

    func recordedEventReadCursors() -> [UInt64?] {
        eventReadCursors
    }

    func recordedWrites() -> [String] {
        writes.map { String(decoding: $0, as: UTF8.self) }
    }
}
