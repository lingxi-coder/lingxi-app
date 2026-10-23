import FirebaseAuth
import FirebaseMessaging
import FirebaseInstallations
import Foundation
import Network
import UIKit
import UserNotifications

/// Main actor ownership follows UIApplication, notification authorization, and
/// Firebase account lifecycle. Network calls suspend; no blocking work/locks.
@MainActor
final class PushRegistration: NSObject, MessagingDelegate {
    static let shared = PushRegistration()
    private var listener: AuthStateDidChangeListenerHandle?
    private var task: Task<Void, Never>?
    private let monitor = NWPathMonitor()
    private var identity: PushInstallationIdentity?
    private let identityURL: URL
    private(set) var lastFailure: String?
    private var registeredFID: String?
    /// The same installation identifier registered with the push service.
    var remoteInstallationId: String? { identity?.id }

    private override init() {
        identityURL = URL.applicationSupportDirectory.appendingPathComponent("push-installation.json")
        super.init()
        do {
            try FileManager.default.createDirectory(at: identityURL.deletingLastPathComponent(), withIntermediateDirectories: true)
            if FileManager.default.fileExists(atPath: identityURL.path) {
                identity = try JSONDecoder().decode(PushInstallationIdentity.self, from: Data(contentsOf: identityURL))
            } else {
                identity = .create()
                try persist()
            }
        } catch { lastFailure = "installation_storage_unavailable" }
    }

    func start() {
        guard listener == nil else { return }
        Messaging.messaging().delegate = self
        listener = Auth.auth().addStateDidChangeListener { [weak self] _, _ in
            Task { @MainActor in
                self?.clearDeliveredNotifications()
                self?.refresh()
            }
        }
        monitor.pathUpdateHandler = { [weak self] path in
            if path.status == .satisfied { Task { @MainActor in self?.refresh() } }
        }
        monitor.start(queue: DispatchQueue(label: "com.lingxi.push-connectivity"))
    }

    func signedOut() {
        UIApplication.shared.unregisterForRemoteNotifications()
        Messaging.messaging().apnsToken = nil
        Messaging.messaging().isAutoInitEnabled = false
        clearDeliveredNotifications()
        refresh()
    }

    func refresh() {
        guard listener != nil, var identity else { return }
        let uid = Auth.auth().currentUser?.uid
        if uid != nil {
            Messaging.messaging().isAutoInitEnabled = true
            // APNs can deliver a token before UIApplication updates its flag.
            // Re-registering from that callback cancels the binding upload forever.
            if Self.needsAPNsRegistration(hasToken: Messaging.messaging().apnsToken != nil,
                                           systemRegistered: UIApplication.shared.isRegisteredForRemoteNotifications) {
                UIApplication.shared.registerForRemoteNotifications()
            }
        } else {
            Messaging.messaging().isAutoInitEnabled = false
            UIApplication.shared.unregisterForRemoteNotifications()
            Messaging.messaging().apnsToken = nil
        }
        identity.revision += 1
        self.identity = identity
        do { try persist() } catch { lastFailure = "installation_storage_unavailable"; return }
        // Cancellation plus a durable monotonic revision handles both local
        // suspension races and requests that reach the server out of order.
        let previous = task
        previous?.cancel()
        task = Task { [weak self] in
            await previous?.value
            guard let self else { return }
            for delay in [0, 2, 10, 30] {
                do {
                    if delay > 0 { try await Task.sleep(for: .seconds(delay)) }
                    try Task.checkCancellation()
                    guard self.identity?.revision == identity.revision else { return }
                    try await self.synchronize(identity: identity, uid: uid, refreshAuth: delay > 0)
                    self.lastFailure = nil
                    return
                } catch is CancellationError { return }
                catch { self.lastFailure = "registration_sync_failed" }
            }
        }
    }

    private func synchronize(identity: PushInstallationIdentity, uid: String?, refreshAuth: Bool) async throws {
        var body: [String: Any] = ["installationSecret": identity.secret, "revision": identity.revision]
        var bearer: String?
        if let uid {
            guard let user = Auth.auth().currentUser, user.uid == uid,
                  Messaging.messaging().apnsToken != nil else { return }
            let center = UNUserNotificationCenter.current()
            var settings = await center.notificationSettings()
            if settings.authorizationStatus == .notDetermined {
                _ = try await center.requestAuthorization(options: [.alert, .sound, .badge])
                settings = await center.notificationSettings()
            }
            try await Messaging.messaging().register()
            let fid = try await Installations.installations().installationID()
            bearer = try await user.getIDToken(forcingRefresh: refreshAuth)
            try await send(identity: identity, method: "POST", body: ["installationSecret": identity.secret], bearer: bearer, suffix: "/initialize")
            guard self.identity?.revision == identity.revision, Auth.auth().currentUser?.uid == uid else { return }
            self.identity?.mayBeBound = true
            try persist() // Persist before sending: a lost response can still have committed.
            let allowed: [UNAuthorizationStatus] = [.authorized, .provisional, .ephemeral]
            body.merge([
                "fcmInstallationId": fid, "platform": "ios", "appId": Bundle.main.bundleIdentifier ?? "com.lingxi.code",
                "appVersion": Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String ?? "0.1.0",
                "osVersion": UIDevice.current.systemVersion, "deviceModel": UIDevice.current.model,
                "locale": Locale.current.identifier,
                "notificationsEnabled": allowed.contains(settings.authorizationStatus) && NotificationPreferencesStore.load().enabled,
            ]) { _, new in new }
        } else if !identity.mayBeBound { return }
        guard self.identity?.revision == identity.revision else { return }
        try await send(identity: identity, method: uid == nil ? "DELETE" : "PUT", body: body, bearer: bearer)
        if uid == nil && self.identity?.revision == identity.revision {
            self.identity?.mayBeBound = false
            try persist()
        }
    }

    private func send(identity: PushInstallationIdentity, method: String, body: [String: Any], bearer: String?, suffix: String = "") async throws {
        let url = URL(string: "https://us-central1-lingxi-5e4b2.cloudfunctions.net/pushApi/v1/push/installations/\(identity.id)\(suffix)")!
        var request = URLRequest(url: url, timeoutInterval: 15)
        request.httpMethod = method
        request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        if let bearer { request.setValue("Bearer \(bearer)", forHTTPHeaderField: "Authorization") }
        request.httpBody = try JSONSerialization.data(withJSONObject: body)
        let (_, response) = try await URLSession.shared.data(for: request)
        guard let http = response as? HTTPURLResponse, (200..<300).contains(http.statusCode) else { throw SyncError.rejected }
    }

    private func persist() throws {
        guard let identity else { throw SyncError.storage }
        try JSONEncoder().encode(identity).write(to: identityURL, options: [.atomic, .completeFileProtectionUntilFirstUserAuthentication])
        var url = identityURL
        var values = URLResourceValues()
        values.isExcludedFromBackup = true
        try url.setResourceValues(values)
    }

    private func clearDeliveredNotifications() {
        UNUserNotificationCenter.current().getDeliveredNotifications { notifications in
            let ids = notifications.filter { PushMessagePolicy.isRemote($0.request.content.userInfo) }.map { $0.request.identifier }
            UNUserNotificationCenter.current().removeDeliveredNotifications(withIdentifiers: ids)
        }
    }

    nonisolated func messaging(_ messaging: Messaging, didReceiveRegistration installationId: String?) {
        guard let installationId else { return }
        Task { @MainActor in
            let registration = PushRegistration.shared
            // Manual register() also invokes this callback. A repeated FID
            // must not restart the upload that requested the registration.
            guard registration.registeredFID != installationId else { return }
            registration.registeredFID = installationId
            registration.refresh()
        }
    }
    static func needsAPNsRegistration(hasToken: Bool, systemRegistered: Bool) -> Bool {
        !hasToken && !systemRegistered
    }

    private enum SyncError: Error { case rejected, storage }
}
