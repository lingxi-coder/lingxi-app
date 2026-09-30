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
        /// Fires if neither a fix nor a denial arrives.
        ///
        /// `withCheckedThrowingContinuation` does not observe Task
        /// cancellation, so when the engine's own 30s budget expires it
        /// simply stops awaiting — leaving `continuation` non-nil forever and
        /// every later request refused as "already in flight". Location was
        /// then dead for the life of the process. Deliberately SHORTER than
        /// the engine's budget so this side finishes first and reports a
        /// real `Timeout` instead of being abandoned mid-call.
        private var timeout: Task<Void, Never>?
        /// Must stay under `LOCATION_TIMEOUT` in `local_apps_host_device`.
        private static let timeoutSeconds: UInt64 = 20

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
                timeout = Task { [weak self] in
                    try? await Task.sleep(nanoseconds: Self.timeoutSeconds * 1_000_000_000)
                    guard !Task.isCancelled else { return }
                    self?.finish(.failure(LocationFfiError.Timeout))
                }
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
            let pending = timeout
            timeout = nil
            lock.unlock()
            // Cancel AFTER clearing state: a timeout that fires concurrently
            // finds `continuation` nil and becomes a no-op, so a late real
            // fix and the deadline can never both resume.
            pending?.cancel()
            guard let cont else { return }
            switch result {
            case let .success(fix): cont.resume(returning: fix)
            case let .failure(error): cont.resume(throwing: error)
            }
        }
    }
#endif
