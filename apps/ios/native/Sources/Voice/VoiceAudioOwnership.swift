import Foundation

/// One UI audio owner; an explicit request may reserve the lane while an
/// automatic reply stops, but cannot interrupt a capture or Flow turn.
@MainActor
final class VoiceAudioOwnership<Owner: AnyObject> {
    enum Kind: Equatable { case explicit, automatic }

    private(set) var owner: Owner?
    private(set) var kind: Kind?
    private var reservation: Owner?
    private var waiting: [Owner] = []
    var waitingCount: Int { waiting.count }

    func acquire(_ candidate: Owner, kind: Kind) -> Bool {
        guard reservation == nil || reservation === candidate,
              owner == nil || owner === candidate else { return false }
        if owner === candidate, self.kind != kind { return false }
        owner = candidate
        self.kind = kind
        if reservation === candidate { reservation = nil }
        return true
    }

    func reserveExplicit(_ candidate: Owner) -> Owner? {
        guard reservation == nil, let owner, kind == .automatic else { return nil }
        reservation = candidate
        return owner
    }

    func finishReservation(_ candidate: Owner, canAcquire: Bool) -> Bool {
        guard reservation === candidate else { return false }
        reservation = nil
        guard canAcquire, owner == nil else { return false }
        return acquire(candidate, kind: .explicit)
    }

    @discardableResult
    func release(_ candidate: Owner) -> Bool {
        guard owner === candidate else { return false }
        owner = nil
        kind = nil
        return true
    }

    func enqueueAutomatic(_ candidate: Owner) {
        guard !waiting.contains(where: { $0 === candidate }) else { return }
        waiting.append(candidate)
    }

    func removeAutomatic(_ candidate: Owner) {
        waiting.removeAll { $0 === candidate }
    }

    func nextAutomatic() -> Owner? {
        guard owner == nil, reservation == nil, !waiting.isEmpty else { return nil }
        return waiting.removeFirst()
    }
}

/// A cancelled native caller may outlive its stop request. Do not wait forever
/// or confuse its cancellation with confirmation that it has drained.
@MainActor
enum VoiceAudioStopDeadline {
    static func completed(_ task: Task<Void, Never>, within timeout: Duration) async -> Bool {
        await withCheckedContinuation { continuation in
            let waiter = VoiceAudioStopWaiter(continuation)
            let observer = Task { @MainActor in
                await task.value
                waiter.resolve(true)
            }
            let timer = Task { @MainActor in
                do { try await Task.sleep(for: timeout) } catch { return }
                waiter.resolve(false)
            }
            waiter.install(observer: observer, timer: timer)
        }
    }
}

@MainActor
private final class VoiceAudioStopWaiter {
    private var continuation: CheckedContinuation<Bool, Never>?
    private var observer: Task<Void, Never>?
    private var timer: Task<Void, Never>?

    init(_ continuation: CheckedContinuation<Bool, Never>) { self.continuation = continuation }
    func install(observer: Task<Void, Never>, timer: Task<Void, Never>) {
        guard continuation != nil else { observer.cancel(); timer.cancel(); return }
        self.observer = observer
        self.timer = timer
    }
    func resolve(_ completed: Bool) {
        guard let continuation else { return }
        self.continuation = nil
        observer?.cancel()
        timer?.cancel()
        observer = nil
        timer = nil
        continuation.resume(returning: completed)
    }
}
