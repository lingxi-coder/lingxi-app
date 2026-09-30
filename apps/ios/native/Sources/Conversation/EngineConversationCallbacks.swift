import Combine
import Foundation
import OSLog
import SwiftUI

#if canImport(harness_runtimeFFI)
import AuthenticationServices
import UIKit
import harness_runtimeFFI
#endif

#if canImport(harness_runtimeFFI)
final class OAuthPresentationContextProvider: NSObject, ASWebAuthenticationPresentationContextProviding {
    func presentationAnchor(for session: ASWebAuthenticationSession) -> ASPresentationAnchor {
        let windows = UIApplication.shared.connectedScenes
            .compactMap { $0 as? UIWindowScene }
            .flatMap(\.windows)
        return windows.first(where: \.isKeyWindow) ?? windows.first ?? UIWindow()
    }
}
#endif

#if canImport(harness_runtimeFFI)
/// Swift implementation of the `IosPermissionSink` UniFFI callback interface
/// (SHIP-BLOCKER #3). Rust calls `onRequest(_:)` on the engine's runtime when a
/// tool needs approval; we hop to the main actor and enqueue the request so the
/// app-level presenter can prompt. Returns promptly — the engine's turn parks
/// on its own oneshot and is resolved later by `ApprovePermission` /
/// `DenyPermission`.
final class EnginePermissionSink: IosPermissionSink {
    private let listener: EngineListener

    init(listener: EngineListener) {
        self.listener = listener
    }

    func onRequest(request: PermissionRequest) async {
        // Do not await the main actor here. The Rust engine constructor is
        // synchronous from Swift and may emit an event/permission while it
        // is running on the main actor. Awaiting `MainActor.run` from this
        // callback would make Rust wait for the main actor while the main
        // actor is waiting for the constructor: a startup deadlock that
        // leaves every control unresponsive. Enqueue and return so the
        // engine can finish construction; the actor preserves UI state
        // mutation on the main thread.
        // Share the event listener's FIFO. A separately scheduled actor task
        // can otherwise present this request after an earlier pump has already
        // applied its cancellation/expiry event.
        listener.enqueuePermission(request)
    }
}
#endif

#if canImport(harness_runtimeFFI)
/// Swift implementation of the `IosEventListener` UniFFI callback interface.
/// Rust calls `onEvent(_:)` on the engine's runtime; we hop to the main actor
/// and forward to the source so all state mutation is main-actor-confined.
final class EngineListener: IosEventListener {
    private weak var source: EngineConversationSource?
    private static let drainBatchSize = 256
    private enum PendingItem {
        case event(ClientEvent)
        case permission(PermissionRequest)
        case workflowProgress(
            originSessionId: String,
            taskId: String,
            runId: String,
            progress: WorkflowProgressDto
        )
    }

    /// Callbacks can arrive from the engine runtime while the main actor is
    /// synchronously constructing the handle. Keep the callback fire-and-
    /// return, but serialize all later projection through one FIFO pump.
    /// The lock protects enqueueing from concurrent runtime callbacks; the
    /// pump itself only touches the source on MainActor.
    private let queueLock = NSLock()
    private var queue: [PendingItem] = []
    private var queueHead = 0
    private var pumpScheduled = false
    private var idleWaiters: [CheckedContinuation<Void, Never>] = []
    /// A gate can be cancelled after parking its entry but before notifying
    /// Swift. IDs are monotonic for this listener's engine generation; retain
    /// its terminal IDs until that listener is disposed, including across
    /// transcript switches, so a late notification cannot revive a dead ask.
    /// Evicting an ID early would admit an arbitrarily delayed notification.
    @MainActor private var resolvedPermissionIDs: Set<UInt64> = []
    /// Test-only counters make the batch/yield contract deterministic to
    /// assert without exposing scheduling hooks to production callers.
    private(set) var pumpInvocationsForTesting = 0
    private(set) var pumpYieldCountForTesting = 0

    var isIdleForTesting: Bool {
        queueLock.lock()
        defer { queueLock.unlock() }
        return queueHead == queue.count && !pumpScheduled
    }

    init(source: EngineConversationSource) {
        self.source = source
    }

    func onEvent(event: ClientEvent) async {
        enqueue(.event(event))
    }

    func enqueuePermission(_ request: PermissionRequest) {
        enqueue(.permission(request))
    }

    func onWorkflowProgress(
        originSessionId: String,
        taskId: String,
        runId: String,
        progress: WorkflowProgressDto
    ) async {
        enqueue(.workflowProgress(
            originSessionId: originSessionId,
            taskId: taskId,
            runId: runId,
            progress: progress
        ))
    }

    /// Compatibility with an older generated callback. It intentionally
    /// drops unscoped progress rather than reintroducing cross-session rows.
    func onWorkflowProgress(
        taskId _: String,
        runId _: String,
        progress _: WorkflowProgressDto
    ) async {}

    /// Wait for all callbacks enqueued before this call to be applied. This
    /// is also used by durable reattach so terminal recovery state is seen
    /// before replay envelopes can project onto the restored transcript.
    func waitUntilIdle() async {
        await withCheckedContinuation { continuation in
            queueLock.lock()
            if queueHead == queue.count, !pumpScheduled {
                queueLock.unlock()
                continuation.resume()
            } else {
                idleWaiters.append(continuation)
                queueLock.unlock()
            }
        }
    }

    /// Test seam for asserting that event order is preserved without
    /// constructing the Rust engine.
    func enqueueForTesting(_ event: ClientEvent) {
        enqueue(.event(event))
    }

    private func enqueue(_ item: PendingItem) {
        queueLock.lock()
        queue.append(item)
        let shouldSchedulePump = !pumpScheduled
        if shouldSchedulePump {
            pumpScheduled = true
        }
        queueLock.unlock()

        guard shouldSchedulePump else { return }
        Task { @MainActor [weak self] in
            self?.drain()
        }
    }

    @MainActor
    private func drain() {
        pumpInvocationsForTesting += 1
        var processed = 0
        while processed < Self.drainBatchSize {
            queueLock.lock()
            guard queueHead < queue.count else {
                queue.removeAll(keepingCapacity: true)
                queueHead = 0
                pumpScheduled = false
                let waiters = idleWaiters
                idleWaiters.removeAll(keepingCapacity: true)
                queueLock.unlock()
                for waiter in waiters {
                    waiter.resume()
                }
                return
            }
            let item = queue[queueHead]
            queueHead += 1
            if queueHead > 256, queueHead * 2 > queue.count {
                queue.removeFirst(queueHead)
                queueHead = 0
            }
            queueLock.unlock()

            processed += 1
            guard let source else { continue }
            switch item {
            case let .event(event):
                if case let .permissionRequestResolved(requestId, _) = event {
                    resolvedPermissionIDs.insert(requestId)
                }
                source.apply(event)
            case let .permission(request):
                guard !resolvedPermissionIDs.contains(request.requestId) else { continue }
                source.enqueuePermission(request)
            case let .workflowProgress(originSessionId, taskId, runId, progress):
                source.applyWorkflowProgress(
                    originSessionId: originSessionId,
                    taskId: taskId,
                    runId: runId,
                    progress: progress
                )
            }
        }

        // Keep the lock-held `pumpScheduled` bit set while handing off to
        // exactly one next MainActor task. Producers therefore append to
        // this same FIFO without starting a parallel pump, and the actor
        // gets a scheduling point between bounded batches.
        queueLock.lock()
        guard queueHead < queue.count else {
            queue.removeAll(keepingCapacity: true)
            queueHead = 0
            pumpScheduled = false
            let waiters = idleWaiters
            idleWaiters.removeAll(keepingCapacity: true)
            queueLock.unlock()
            for waiter in waiters {
                waiter.resume()
            }
            return
        }
        pumpYieldCountForTesting += 1
        queueLock.unlock()
        Task { @MainActor [weak self] in
            self?.drain()
        }
    }
}
#endif
