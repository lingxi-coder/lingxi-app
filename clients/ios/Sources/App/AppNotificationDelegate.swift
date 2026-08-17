import UIKit
import UserNotifications

extension Notification.Name {
    static let lingxiCronNotificationOpened = Notification.Name("LingxiCronNotificationOpened")
}

final class AppNotificationDelegate: NSObject, UIApplicationDelegate, UNUserNotificationCenterDelegate {
    static var cronBackgroundBridge: CronBackgroundTaskBridge = .shared
    static var cronBackgroundRegistrarFactory: () -> any CronBackgroundTaskRegistrar = {
        LiveCronBackgroundTaskRegistrar()
    }

    func application(
        _ application: UIApplication,
        didFinishLaunchingWithOptions launchOptions: [UIApplication.LaunchOptionsKey: Any]? = nil
    ) -> Bool {
        UNUserNotificationCenter.current().delegate = self
        Self.cronBackgroundBridge.registerAtLaunch(
            taskIdentifier: cronBackgroundTaskIdentifier,
            registrar: Self.cronBackgroundRegistrarFactory()
        )
        LocalAppBackgroundTaskBridge.shared.registerAtLaunch()
        return true
    }

    func userNotificationCenter(
        _ center: UNUserNotificationCenter,
        willPresent notification: UNNotification
    ) async -> UNNotificationPresentationOptions {
        [.banner, .sound, .list]
    }

    func userNotificationCenter(
        _ center: UNUserNotificationCenter,
        didReceive response: UNNotificationResponse
    ) async {
        await MainActor.run {
            NotificationCenter.default.post(
                name: .lingxiCronNotificationOpened,
                object: nil,
                userInfo: response.notification.request.content.userInfo
            )
        }
    }
}
