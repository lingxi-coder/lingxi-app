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
    final class LocationImpl: NSObject, IosLocation, CLLocationManagerDelegate, @unchecked Sendable {
        /// The manager must outlive the request — a deallocated manager
        /// silently never calls back.
        private let manager = CLLocationManager()
        private var continuation: CheckedContinuation<LocationFixFfi, Error>?
        /// Set while we are waiting for `.notDetermined` to resolve, so the
        /// authorization callback knows whether to start a request or ignore.
        private var awaitingAuthorization = false
        private let lock = NSLock()

        override init() {
            super.init()
            manager.delegate = self
            manager.desiredAccuracy = kCLLocationAccuracyHundredMeters
        }

        func currentLocation() async throws -> LocationFixFfi {
            guard CLLocationManager.locationServicesEnabled() else {
                throw LocationFfiError.Unavailable
            }
            return try await withCheckedThrowingContinuation { cont in
                lock.lock()
                guard continuation == nil else {
                    lock.unlock()
                    cont.resume(throwing: LocationFfiError.Other(
                        message: "another location request is already in flight"))
                    return
                }
                continuation = cont
                lock.unlock()

                DispatchQueue.main.async { [weak self] in
                    guard let self else { return }
                    switch self.manager.authorizationStatus {
                    case .notDetermined:
                        self.awaitingAuthorization = true
                        self.manager.requestWhenInUseAuthorization()
                    case .authorizedWhenInUse, .authorizedAlways:
                        self.manager.requestLocation()
                    default:
                        self.finish(.failure(LocationFfiError.PermissionDenied))
                    }
                }
            }
        }

        func locationManagerDidChangeAuthorization(_ manager: CLLocationManager) {
            guard awaitingAuthorization else { return }
            switch manager.authorizationStatus {
            case .notDetermined:
                return  // the sheet is still up
            case .authorizedWhenInUse, .authorizedAlways:
                awaitingAuthorization = false
                manager.requestLocation()
            default:
                awaitingAuthorization = false
                finish(.failure(LocationFfiError.PermissionDenied))
            }
        }

        func locationManager(_ manager: CLLocationManager, didUpdateLocations locations: [CLLocation]) {
            guard let location = locations.last else {
                finish(.failure(LocationFfiError.Unavailable))
                return
            }
            finish(.success(LocationFixFfi(
                latitude: location.coordinate.latitude,
                longitude: location.coordinate.longitude,
                accuracyM: location.horizontalAccuracy >= 0 ? location.horizontalAccuracy : nil,
                timestampMs: UInt64(max(0, location.timestamp.timeIntervalSince1970 * 1000)))))
        }

        func locationManager(_ manager: CLLocationManager, didFailWithError error: Error) {
            let failure: LocationFfiError
            switch (error as? CLError)?.code {
            case .denied: failure = .PermissionDenied
            case .locationUnknown: failure = .Unavailable
            default: failure = .Other(message: error.localizedDescription)
            }
            finish(.failure(failure))
        }

        private func finish(_ result: Result<LocationFixFfi, Error>) {
            lock.lock()
            let cont = continuation
            continuation = nil
            awaitingAuthorization = false
            lock.unlock()
            guard let cont else { return }
            switch result {
            case let .success(fix): cont.resume(returning: fix)
            case let .failure(error): cont.resume(throwing: error)
            }
        }
    }
#endif
