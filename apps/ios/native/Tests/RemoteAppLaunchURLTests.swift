import XCTest
@testable import LingxiCode

final class RemoteAppLaunchURLTests: XCTestCase {
    @MainActor
    func testForgettingCurrentHostInvalidatesPendingLaunchBeforeForgetCompletes() async {
        let epoch = RemoteAppRequestEpoch()
        let pending = epoch.value
        let forgetStarted = expectation(description: "forget entered FFI")
        var finishForget: CheckedContinuation<Void, Never>?
        let forget = Task {
            await epoch.forget(hostID: "current", currentHostID: "current") {
                await withCheckedContinuation { continuation in
                    finishForget = continuation
                    forgetStarted.fulfill()
                }
            }
        }
        await fulfillment(of: [forgetStarted], timeout: 1)
        XCTAssertNotEqual(pending, epoch.value, "a delayed open reply must be rejected while forget is suspended")
        XCTAssertFalse(epoch.permits(hostID: "current"), "new requests cannot use the stale ready state")
        XCTAssertTrue(epoch.permits(hostID: "other"))
        epoch.observe(hostID: "current", ready: false)
        epoch.observe(hostID: "current", ready: true)
        XCTAssertFalse(epoch.permits(hostID: "current"), "reconnection must not unblock an unfinished forget")
        guard let finishForget else { forget.cancel(); return }
        finishForget.resume()
        await forget.value
        XCTAssertTrue(epoch.permits(hostID: "current"), "a later connection can admit new requests")

        let stillCurrent = epoch.value
        await epoch.forget(hostID: "other", currentHostID: "current") { }
        XCTAssertEqual(stillCurrent, epoch.value, "forgetting another host must not invalidate this host's request")

        await epoch.forget(hostID: "current", currentHostID: "current") { }
        epoch.reset()
        XCTAssertTrue(epoch.permits(hostID: "current"), "a replacement account or client must not inherit the block")
    }

    @MainActor
    func testConcurrentForgetsAndResetKeepTheirOwnBarriers() async {
        let epoch = RemoteAppRequestEpoch()
        var completions: [CheckedContinuation<Void, Never>] = []
        let started = expectation(description: "both forgets entered")
        started.expectedFulfillmentCount = 2
        func start() -> Task<Void, Never> {
            Task {
                await epoch.forget(hostID: "current", currentHostID: "current") {
                    await withCheckedContinuation { continuation in
                        completions.append(continuation)
                        started.fulfill()
                    }
                }
            }
        }
        let first = start(), second = start()
        await fulfillment(of: [started], timeout: 1)
        epoch.observe(hostID: "current", ready: false)
        completions[0].resume(); await Task.yield()
        XCTAssertFalse(epoch.permits(hostID: "current"))
        epoch.reset()
        await epoch.forget(hostID: "current", currentHostID: "current") { }
        completions[1].resume(); await first.value; await second.value
        XCTAssertFalse(epoch.permits(hostID: "current"), "old completion must not remove a replacement barrier")
        epoch.observe(hostID: "current", ready: false)
        XCTAssertTrue(epoch.permits(hostID: "current"))
    }

    @MainActor
    func testNewForgetAfterReconnectWaitsForItsOwnDeparture() async {
        let epoch = RemoteAppRequestEpoch()
        var firstGate: CheckedContinuation<Void, Never>?
        let first = Task {
            await epoch.forget(hostID: "current", currentHostID: "current") {
                await withCheckedContinuation { firstGate = $0 }
            }
        }
        while firstGate == nil { await Task.yield() }
        epoch.observe(hostID: "current", ready: false)
        epoch.observe(hostID: "current", ready: true)
        await epoch.forget(hostID: "current", currentHostID: "current") { }
        firstGate!.resume(); await first.value
        XCTAssertFalse(epoch.permits(hostID: "current"))
        epoch.observe(hostID: "current", ready: false)
        XCTAssertTrue(epoch.permits(hostID: "current"))
    }

    func testPresentedBrowserSurvivesConnectionLossAndStaleLaunchDoesNotReplaceIt() {
        let original = URL(string: "https://first.example/#ticket")!
        let stale = URL(string: "https://second.example/#ticket")!
        var browser = RemoteAppBrowserLifetime()
        browser.present(original, generation: browser.generation)
        let originalID = browser.ticket?.id
        let pending = browser.generation

        // A catalog disappears on disconnect, but its presented browser remains.
        browser.invalidatePending()
        browser.present(stale, generation: pending)
        XCTAssertEqual(browser.ticket?.id, originalID)
        XCTAssertEqual(browser.ticket?.url, original)

        browser.dismiss()
        XCTAssertNil(browser.ticket)
        browser.present(stale, generation: browser.generation)
        XCTAssertEqual(browser.ticket?.url, stale)
    }

    func testOnlyUnexpiredHTTPSLaunchURLsAreAccepted() {
        let now = Date(timeIntervalSince1970: 1_000)
        XCTAssertNotNil(RemoteAppLaunchURL.validate("https://a0123456789abcdef0123456789abcdef-0123456789abcdef0123.desktop.apps.example.com/__lingxi_remote_bootstrap#ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopq", expiresAt: 1_001_000, now: now))
        for raw in ["http://example.com/app", "https://user:pass@example.com", "https://example.com/", "https:///missing-host", "javascript:alert(1)"] {
            XCTAssertNil(RemoteAppLaunchURL.validate(raw, expiresAt: 1_001_000, now: now))
        }
        XCTAssertNil(RemoteAppLaunchURL.validate("https://example.com/#ticket", expiresAt: 1_000_000, now: now))
    }
}
