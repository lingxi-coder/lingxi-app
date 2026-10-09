// LocationImpl.swift — iOS one-shot location capability.
//
// Conforms to the generated `IosLocation` UniFFI callback interface. A local
// app that declared the `location` capability reaches this through the
// `device.getLocation` bridge op; the engine has already run its own
// permission gate by the time we are called, so what happens here is purely
// the SYSTEM authorization and the fix itself.
//
// One-shot only (`requestLocation`), never continuous: streaming updates
// would need a host-to-page push channel that does not exist, and a
// background-location entitlement nobody has asked for. When-in-use only.

import Foundation

#if canImport(CoreLocation)
    import CoreLocation

    /// Native one-shot location over `CLLocationManager`.
    @MainActor
    final class LocationImpl: NSObject, IosLocation, CLLocationManagerDelegate {
        private let managerFactory: @MainActor @Sendable () -> CLLocationManager
        nonisolated private let locationServicesEnabled: @Sendable () -> Bool
        private let sleep: @Sendable (UInt64) async throws -> Void
        private let timeoutNanoseconds: UInt64
        /// Core Location delivers callbacks on the manager's creation RunLoop.
        /// Create and retain a fresh manager on MainActor for each request, so a
        /// callback from a cancelled or timed-out request cannot finish its successor.
        private var manager: CLLocationManager?
        private var requestID: UUID?
        private var continuation: CheckedContinuation<LocationFixFfi, Error>?
        /// Set while we are waiting for `.notDetermined` to resolve, so the
        /// authorization callback knows whether to start a request or ignore.
        private var awaitingAuthorization = false
        /// Finish independently if neither a fix nor a denial arrives, before
        /// the engine's own 30s budget abandons the native callback.
        private var timeout: Task<Void, Never>?
        /// Must stay under the engine's own location-callback budget.
        nonisolated private static let timeoutNanoseconds: UInt64 = 20_000_000_000

        /// Engine construction runs in a detached task. This initializer must
        /// remain cheap and must not construct any RunLoop-bound native object.
        nonisolated override init() {
            managerFactory = { CLLocationManager() }
            locationServicesEnabled = { CLLocationManager.locationServicesEnabled() }
            sleep = { try await Task.sleep(nanoseconds: $0) }
            timeoutNanoseconds = Self.timeoutNanoseconds
            super.init()
        }

        nonisolated init(
            managerFactory: @escaping @MainActor @Sendable () -> CLLocationManager,
            locationServicesEnabled: @escaping @Sendable () -> Bool,
            timeoutNanoseconds: UInt64 = 20_000_000_000,
            sleep: @escaping @Sendable (UInt64) async throws -> Void = {
                try await Task.sleep(nanoseconds: $0)
            }
        ) {
            self.managerFactory = managerFactory
            self.locationServicesEnabled = locationServicesEnabled
            self.timeoutNanoseconds = timeoutNanoseconds
            self.sleep = sleep
            super.init()
        }

        nonisolated func currentLocation() async throws -> LocationFixFfi {
            try Task.checkCancellation()
            // This system query may perform synchronous IPC; keep it off the
            // UI executor while confining the RunLoop-bound manager to MainActor.
            guard locationServicesEnabled() else {
                throw LocationFfiError.Unavailable
            }
            return try await requestOnMainActor()
        }

        private func requestOnMainActor() async throws -> LocationFixFfi {
            let id = UUID()
            return try await withTaskCancellationHandler {
                try Task.checkCancellation()
                return try await withCheckedThrowingContinuation { cont in
                    guard continuation == nil else {
                        cont.resume(throwing: LocationFfiError.Other(
                            message: "another location request is already in flight"))
                        return
                    }
                    let manager = managerFactory()
                    self.manager = manager
                    requestID = id
                    continuation = cont
                    manager.delegate = self
                    manager.desiredAccuracy = kCLLocationAccuracyHundredMeters
                    let sleep = self.sleep
                    let duration = timeoutNanoseconds
                    timeout = Task { [weak self] in
                        do { try await sleep(duration) }
                        catch { return }
                        guard !Task.isCancelled else { return }
                        self?.finish(id: id, .failure(LocationFfiError.Timeout))
                    }
                    switch manager.authorizationStatus {
                    case .notDetermined:
                        awaitingAuthorization = true
                        manager.requestWhenInUseAuthorization()
                    case .authorizedWhenInUse, .authorizedAlways:
                        manager.requestLocation()
                    default:
                        finish(id: id, .failure(LocationFfiError.PermissionDenied))
                    }
                }
            } onCancel: {
                Task { @MainActor [weak self] in
                    self?.finish(id: id, .failure(CancellationError()))
                }
            }
        }

        nonisolated func locationManagerDidChangeAuthorization(_ manager: CLLocationManager) {
            // Managers are created on MainActor, whose RunLoop receives these
            // delegate calls. Keep the synchronous delegate requirements while
            // asserting the same isolation as our request state.
            MainActor.assumeIsolated {
                guard manager === self.manager, awaitingAuthorization, let id = requestID else { return }
                switch manager.authorizationStatus {
                case .notDetermined:
                    return  // the sheet is still up
                case .authorizedWhenInUse, .authorizedAlways:
                    awaitingAuthorization = false
                    manager.requestLocation()
                default:
                    finish(id: id, .failure(LocationFfiError.PermissionDenied))
                }
            }
        }

        nonisolated func locationManager(_ manager: CLLocationManager, didUpdateLocations locations: [CLLocation]) {
            MainActor.assumeIsolated {
                guard manager === self.manager, let id = requestID else { return }
                guard let location = locations.last else {
                    finish(id: id, .failure(LocationFfiError.Unavailable))
                    return
                }
                finish(id: id, .success(LocationFixFfi(
                    latitude: location.coordinate.latitude,
                    longitude: location.coordinate.longitude,
                    accuracyM: location.horizontalAccuracy >= 0 ? location.horizontalAccuracy : nil,
                    timestampMs: UInt64(max(0, location.timestamp.timeIntervalSince1970 * 1000)))))
            }
        }

        nonisolated func locationManager(_ manager: CLLocationManager, didFailWithError error: Error) {
            MainActor.assumeIsolated {
                guard manager === self.manager, let id = requestID else { return }
                let failure: LocationFfiError
                switch (error as? CLError)?.code {
                case .denied: failure = .PermissionDenied
                case .locationUnknown: failure = .Unavailable
                default: failure = .Other(message: error.localizedDescription)
                }
                finish(id: id, .failure(failure))
            }
        }

        private func finish(id: UUID, _ result: Result<LocationFixFfi, Error>) {
            guard requestID == id, let cont = continuation else { return }
            requestID = nil
            continuation = nil
            awaitingAuthorization = false
            let pending = timeout
            timeout = nil
            let manager = self.manager
            self.manager = nil
            manager?.delegate = nil
            manager?.stopUpdatingLocation()
            pending?.cancel()
            switch result {
            case let .success(fix): cont.resume(returning: fix)
            case let .failure(error): cont.resume(throwing: error)
            }
        }
    }
#endif
