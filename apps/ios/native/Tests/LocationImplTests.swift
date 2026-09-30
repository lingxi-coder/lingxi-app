import CoreLocation
import Foundation
import XCTest

@testable import LingxiCode

@MainActor
final class LocationImplTests: XCTestCase {
    func testDetachedConstructionDefersManagerCreationToMainActor() async throws {
        let requested = expectation(description: "location requested")
        var manager: FakeLocationManager?
        var factoryCalls = 0
        let factory: @MainActor @Sendable () -> CLLocationManager = {
            XCTAssertTrue(Thread.isMainThread)
            factoryCalls += 1
            let created = FakeLocationManager(status: grantedLocationAuthorization)
            created.onRequest = { requested.fulfill() }
            manager = created
            return created
        }
        let (location, builtOnMain) = await Task.detached {
            Self.constructOffActor(factory: factory)
        }.value
        XCTAssertFalse(builtOnMain)
        XCTAssertEqual(factoryCalls, 0)

        let request = Task { try await location.currentLocation() }
        await fulfillment(of: [requested], timeout: 2)
        XCTAssertEqual(factoryCalls, 1)
        let created = try XCTUnwrap(manager)
        deliverFix(to: location, manager: created, latitude: 12)
        let fix = try await request.value
        XCTAssertEqual(fix.latitude, 12)
        XCTAssertNil(created.delegate)
        XCTAssertEqual(created.stopCalls, 1)
    }

    func testAuthorizationChangeStartsRequestExactlyOnce() async throws {
        let authorization = expectation(description: "authorization requested")
        let manager = FakeLocationManager(status: .notDetermined)
        manager.onAuthorization = { authorization.fulfill() }
        let location = makeLocation(manager: manager)
        let request = Task { try await location.currentLocation() }
        await fulfillment(of: [authorization], timeout: 2)
        XCTAssertEqual(manager.requestCalls, 0)

        location.locationManagerDidChangeAuthorization(manager)
        XCTAssertEqual(manager.requestCalls, 0)
        manager.status = grantedLocationAuthorization
        location.locationManagerDidChangeAuthorization(manager)
        location.locationManagerDidChangeAuthorization(manager)
        XCTAssertEqual(manager.requestCalls, 1)
        deliverFix(to: location, manager: manager, latitude: 20)
        let fix = try await request.value
        XCTAssertEqual(fix.latitude, 20)
    }

    func testConcurrentRequestIsRejectedWithoutReplacingOwner() async throws {
        let requested = expectation(description: "first request admitted")
        let manager = FakeLocationManager(status: grantedLocationAuthorization)
        manager.onRequest = { requested.fulfill() }
        let location = makeLocation(manager: manager)
        let first = Task { try await location.currentLocation() }
        await fulfillment(of: [requested], timeout: 2)

        do {
            _ = try await location.currentLocation()
            XCTFail("A second request must be refused")
        } catch LocationFfiError.Other(let message) {
            XCTAssertEqual(message, "another location request is already in flight")
        }
        XCTAssertEqual(manager.requestCalls, 1)
        deliverFix(to: location, manager: manager, latitude: 30)
        let fix = try await first.value
        XCTAssertEqual(fix.latitude, 30)
    }

    func testTimeoutReleasesOwnerAndIgnoresOldManagerCallbacks() async throws {
        let admitted = expectation(description: "first request admitted")
        let secondAdmitted = expectation(description: "second request admitted")
        let firstManager = FakeLocationManager(status: grantedLocationAuthorization)
        let secondManager = FakeLocationManager(status: grantedLocationAuthorization)
        firstManager.onRequest = { admitted.fulfill() }
        secondManager.onRequest = { secondAdmitted.fulfill() }
        let deadline = AsyncStream<Void>.makeStream()
        var managers = [firstManager, secondManager]
        let location = LocationImpl(
            managerFactory: { managers.removeFirst() },
            locationServicesEnabled: { true },
            sleep: { _ in
                for await _ in deadline.stream { return }
            }
        )
        let first = Task { try await location.currentLocation() }
        await fulfillment(of: [admitted], timeout: 2)
        deadline.continuation.yield(())
        do {
            _ = try await first.value
            XCTFail("The deadline must finish the request")
        } catch LocationFfiError.Timeout {}
        XCTAssertNil(firstManager.delegate)
        XCTAssertEqual(firstManager.stopCalls, 1)

        let second = Task { try await location.currentLocation() }
        await fulfillment(of: [secondAdmitted], timeout: 2)
        // Model callbacks already queued before the old delegate was detached.
        deliverFix(to: location, manager: firstManager, latitude: 999)
        location.locationManager(firstManager, didFailWithError: CLError(.denied))
        location.locationManagerDidChangeAuthorization(firstManager)
        deliverFix(to: location, manager: secondManager, latitude: 40)
        let fix = try await second.value
        XCTAssertEqual(fix.latitude, 40)
        XCTAssertEqual(secondManager.stopCalls, 1)
        deadline.continuation.finish()
    }

    func testCancellationDuringAuthorizationReleasesOwnerForNextRequest() async throws {
        let authorization = expectation(description: "authorization requested")
        let secondAdmitted = expectation(description: "successor admitted")
        let firstManager = FakeLocationManager(status: .notDetermined)
        let secondManager = FakeLocationManager(status: grantedLocationAuthorization)
        firstManager.onAuthorization = { authorization.fulfill() }
        secondManager.onRequest = { secondAdmitted.fulfill() }
        var managers = [firstManager, secondManager]
        let location = LocationImpl(
            managerFactory: { managers.removeFirst() },
            locationServicesEnabled: { true }
        )
        let first = Task { try await location.currentLocation() }
        await fulfillment(of: [authorization], timeout: 2)
        first.cancel()
        do {
            _ = try await first.value
            XCTFail("Cancellation must resolve the continuation")
        } catch is CancellationError {}
        XCTAssertNil(firstManager.delegate)
        XCTAssertEqual(firstManager.stopCalls, 1)

        let second = Task { try await location.currentLocation() }
        await fulfillment(of: [secondAdmitted], timeout: 2)
        firstManager.status = grantedLocationAuthorization
        location.locationManagerDidChangeAuthorization(firstManager)
        XCTAssertEqual(firstManager.requestCalls, 0)
        deliverFix(to: location, manager: firstManager, latitude: 999)
        deliverFix(to: location, manager: secondManager, latitude: 50)
        let fix = try await second.value
        XCTAssertEqual(fix.latitude, 50)
    }

    func testAlreadyCancelledRequestDoesNotConstructManager() async throws {
        var factoryCalls = 0
        let location = LocationImpl(
            managerFactory: {
                factoryCalls += 1
                return FakeLocationManager(status: grantedLocationAuthorization)
            },
            locationServicesEnabled: { true }
        )
        let request = Task {
            withUnsafeCurrentTask { $0?.cancel() }
            return try await location.currentLocation()
        }
        do {
            _ = try await request.value
            XCTFail("Already cancelled work must not start location services")
        } catch is CancellationError {}
        XCTAssertEqual(factoryCalls, 0)
    }

    func testDeniedAuthorizationAndDisabledServicesRemainTypedErrors() async throws {
        let denied = FakeLocationManager(status: .denied)
        let location = makeLocation(manager: denied)
        do {
            _ = try await location.currentLocation()
            XCTFail("Denied authorization must fail")
        } catch LocationFfiError.PermissionDenied {}
        XCTAssertEqual(denied.requestCalls, 0)
        XCTAssertNil(denied.delegate)

        let unavailable = LocationImpl(
            managerFactory: { XCTFail("Disabled services must not construct a manager"); return denied },
            locationServicesEnabled: { false }
        )
        do {
            _ = try await unavailable.currentLocation()
            XCTFail("Disabled services must fail")
        } catch LocationFfiError.Unavailable {}
    }

    private nonisolated static func constructOffActor(
        factory: @escaping @MainActor @Sendable () -> CLLocationManager
    ) -> (LocationImpl, Bool) {
        (LocationImpl(managerFactory: factory, locationServicesEnabled: {
            XCTAssertFalse(Thread.isMainThread)
            return true
        }), Thread.isMainThread)
    }

    private func makeLocation(manager: FakeLocationManager) -> LocationImpl {
        LocationImpl(managerFactory: { manager }, locationServicesEnabled: { true })
    }

    private func deliverFix(to location: LocationImpl, manager: CLLocationManager, latitude: Double) {
        location.locationManager(manager, didUpdateLocations: [CLLocation(
            coordinate: CLLocationCoordinate2D(latitude: latitude, longitude: 10),
            altitude: 0,
            horizontalAccuracy: 4,
            verticalAccuracy: -1,
            timestamp: Date(timeIntervalSince1970: 1)
        )])
    }
}

/// The real manager is created on MainActor, but requests are intercepted: no
/// system authorization sheet, location service, or device hardware is used.
private final class FakeLocationManager: CLLocationManager {
    var status: CLAuthorizationStatus
    var onRequest: (() -> Void)?
    var onAuthorization: (() -> Void)?
    private(set) var requestCalls = 0
    private(set) var stopCalls = 0

    init(status: CLAuthorizationStatus) {
        self.status = status
        super.init()
        XCTAssertTrue(Thread.isMainThread)
    }

    override var authorizationStatus: CLAuthorizationStatus { status }
    override func requestWhenInUseAuthorization() { onAuthorization?() }
    override func requestLocation() {
        XCTAssertTrue(Thread.isMainThread)
        requestCalls += 1
        onRequest?()
    }
    override func stopUpdatingLocation() { stopCalls += 1 }
}

private let grantedLocationAuthorization: CLAuthorizationStatus = {
    #if os(iOS)
        return .authorizedWhenInUse
    #else
        return .authorizedAlways
    #endif
}()
