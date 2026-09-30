// NotificationImpl.swift — iOS native local-notification capability (parity with
// Android NotificationController.kt).
//
// Conforms to the generated `IosNotification` UniFFI callback interface. The
// engine (tool-notification) calls `notify(title:body:tag:)`; we request
// notification authorization and add a `UNNotificationRequest` to the
// `UNUserNotificationCenter` with an immediate (nil) trigger. `tag` becomes the
// request identifier so a later post with the same tag replaces the earlier one.
// Errors map onto the generated `NotificationFfiError`. Engine-driven only — no
// user-facing affordance.

import Foundation

#if canImport(UserNotifications)
    import UserNotifications

    /// Native notifications over `UNUserNotificationCenter`.
    final class NotificationImpl: IosNotification, @unchecked Sendable {
        func notify(title: String, body: String, tag: String?) async throws {
            try await requestAuthorization()

            let content = UNMutableNotificationContent()
            content.title = title
            content.body = body
            content.sound = .default

            let identifier = tag ?? UUID().uuidString
            let request = UNNotificationRequest(identifier: identifier, content: content, trigger: nil)

            do {
                try await UNUserNotificationCenter.current().add(request)
            } catch {
                throw NotificationFfiError.Other(message: error.localizedDescription)
            }
        }

        private func requestAuthorization() async throws {
            let center = UNUserNotificationCenter.current()
            let settings = await center.notificationSettings()
            switch settings.authorizationStatus {
            case .authorized, .provisional, .ephemeral:
                return
            case .denied:
                throw NotificationFfiError.PermissionDenied
            default:
                do {
                    let granted = try await center.requestAuthorization(options: [.alert, .sound, .badge])
                    guard granted else { throw NotificationFfiError.PermissionDenied }
                } catch let e as NotificationFfiError {
                    throw e
                } catch {
                    throw NotificationFfiError.Other(message: error.localizedDescription)
                }
            }
        }
    }
#endif
