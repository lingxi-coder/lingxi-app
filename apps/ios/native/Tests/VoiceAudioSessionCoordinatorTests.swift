import AVFoundation
import XCTest
@testable import LingxiCode

final class VoiceAudioSessionCoordinatorTests: XCTestCase {
    func testInterruptionInvalidatesLeaseAndNeverResumesIt() async throws {
        let driver = RecordingVoiceAudioSessionDriver()
        let coordinator = VoiceAudioSessionCoordinator(sessionDriver: driver)
        let lease = try await coordinator.acquire(.recording)

        await coordinator.handleInterruption(
            rawType: AVAudioSession.InterruptionType.began.rawValue,
            rawOptions: 0
        )
        await coordinator.handleInterruption(
            rawType: AVAudioSession.InterruptionType.ended.rawValue,
            rawOptions: AVAudioSession.InterruptionOptions.shouldResume.rawValue
        )

        XCTAssertEqual(driver.activationCount, 1)
        XCTAssertEqual(driver.deactivationCount, 1)
        let invalidatedLeaseRemainsReserved = await coordinator.owns(lease)
        XCTAssertTrue(invalidatedLeaseRemainsReserved)
        do {
            _ = try await coordinator.acquire(.playback)
            XCTFail("a replacement must wait until the invalidated operation has stopped")
        } catch VoiceAudioSessionCoordinator.CoordinationError.busy(.recording) {
            // The old operation keeps its reservation until the service stops it.
        }

        await coordinator.release(lease)
        let invalidatedLeaseWasReleased = await coordinator.owns(lease)
        XCTAssertFalse(invalidatedLeaseWasReleased)

        let replacement = try await coordinator.acquire(.playback)
        let replacementIsOwned = await coordinator.owns(replacement)
        XCTAssertTrue(replacementIsOwned)
        await coordinator.release(replacement)
    }

    func testForegroundDoesNotReactivateAnInvalidatedLease() async throws {
        let driver = RecordingVoiceAudioSessionDriver()
        let coordinator = VoiceAudioSessionCoordinator(sessionDriver: driver)
        let lease = try await coordinator.acquire(.recording)
        var invalidations = await coordinator.invalidationEvents().makeAsyncIterator()

        await coordinator.suspendForBackground()
        await coordinator.resumeAfterForeground()
        let invalidation = await invalidations.next()

        XCTAssertEqual(driver.activationCount, 1)
        XCTAssertEqual(driver.deactivationCount, 1)
        XCTAssertEqual(invalidation?.lease, lease)
        XCTAssertEqual(invalidation?.reason, .background)
        let leaseRemainsReservedUntilOwnerStops = await coordinator.owns(lease)
        XCTAssertTrue(leaseRemainsReservedUntilOwnerStops)
        await coordinator.release(lease)
    }

    func testLateReleaseFromOldLeaseCannotClearNewOwner() async throws {
        let coordinator = VoiceAudioSessionCoordinator(sessionDriver: RecordingVoiceAudioSessionDriver())
        let oldLease = try await coordinator.acquire(.playback)
        await coordinator.release(oldLease)

        let currentLease = try await coordinator.acquire(.recording)
        await coordinator.release(oldLease)

        let currentLeaseIsOwned = await coordinator.owns(currentLease)
        XCTAssertTrue(currentLeaseIsOwned)
        do {
            _ = try await coordinator.acquire(.playback)
            XCTFail("late release from an old lease must not make the active owner idle")
        } catch VoiceAudioSessionCoordinator.CoordinationError.busy(.recording) {
            // The new owner remains active.
        }

        await coordinator.release(currentLease)
    }

    func testFailedDeactivationRetainsExactLeaseUntilConfirmedRetry() async throws {
        let driver = RecordingVoiceAudioSessionDriver()
        let coordinator = VoiceAudioSessionCoordinator(sessionDriver: driver)
        let lease = try await coordinator.acquire(.recording)
        driver.setDeactivationSucceeds(false)

        let first = await coordinator.releaseWithReceipt(lease)
        XCTAssertFalse(first)
        let stillOwned = await coordinator.owns(lease)
        XCTAssertTrue(stillOwned)
        do {
            _ = try await coordinator.acquire(.playback)
            XCTFail("unconfirmed cleanup must retain its audio owner")
        } catch VoiceAudioSessionCoordinator.CoordinationError.busy(.recording) {
        }

        driver.setDeactivationSucceeds(true)
        let second = await coordinator.releaseWithReceipt(lease)
        XCTAssertTrue(second)
        let released = await coordinator.owns(lease)
        XCTAssertFalse(released)
        let replacement = try await coordinator.acquire(.playback)
        await coordinator.release(replacement)
    }

    func testPartialMediaActivationRetainsLeaseUntilCloseIsConfirmed() async throws {
        let driver = RecordingVoiceAudioSessionDriver()
        driver.setActivationSucceeds(false)
        driver.setDeactivationSucceeds(false)
        let coordinator = VoiceAudioSessionCoordinator(sessionDriver: driver)
        guard case let .activationFailed(lease, _) = await coordinator.acquireForMedia(.recording) else {
            return XCTFail("an attempted but failed activation must retain its exact lease")
        }
        let first = await coordinator.releaseWithReceipt(lease)
        XCTAssertFalse(first)
        let stillOwned = await coordinator.owns(lease)
        XCTAssertTrue(stillOwned)
        driver.setDeactivationSucceeds(true)
        let retry = await coordinator.releaseWithReceipt(lease)
        XCTAssertTrue(retry)
    }

    func testBusyMediaAdmissionDoesNotCallNativeActivation() async throws {
        let driver = RecordingVoiceAudioSessionDriver()
        let coordinator = VoiceAudioSessionCoordinator(sessionDriver: driver)
        let lease = try await coordinator.acquire(.recording)
        guard case .notAdmitted(.recording) = await coordinator.acquireForMedia(.playback) else {
            return XCTFail("busy admission must reject before dispatch")
        }
        XCTAssertEqual(driver.activationCount, 1)
        await coordinator.release(lease)
    }
}

private final class RecordingVoiceAudioSessionDriver: VoiceAudioSessionDriving, @unchecked Sendable {
    private let lock = NSLock()
    private var activations = 0
    private var deactivations = 0
    private var deactivationSucceeds = true
    private var activationSucceeds = true

    var activationCount: Int {
        lock.lock()
        defer { lock.unlock() }
        return activations
    }

    var deactivationCount: Int {
        lock.lock()
        defer { lock.unlock() }
        return deactivations
    }

    func activate(_: VoiceAudioSessionCoordinator.Purpose) throws {
        lock.lock()
        defer { lock.unlock() }
        activations += 1
        if !activationSucceeds { throw TestActivationFailure() }
    }

    func deactivate() {
        lock.lock()
        defer { lock.unlock() }
        deactivations += 1
    }

    func setDeactivationSucceeds(_ succeeds: Bool) {
        lock.lock()
        defer { lock.unlock() }
        deactivationSucceeds = succeeds
    }

    func setActivationSucceeds(_ succeeds: Bool) {
        lock.lock()
        defer { lock.unlock() }
        activationSucceeds = succeeds
    }

    func deactivateWithReceipt() -> Bool {
        lock.lock()
        defer { lock.unlock() }
        deactivations += 1
        return deactivationSucceeds
    }
}

private struct TestActivationFailure: Error {}
