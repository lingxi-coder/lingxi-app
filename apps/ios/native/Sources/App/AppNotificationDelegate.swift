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
        return true
    }

    func userNotificationCenter(
        _ center: UNUserNotificationCenter,
        willPresent notification: UNNotification
    ) async -> UNNotificationPresentationOptions {
        conversationAction(from: notification.request.content.userInfo) == nil
            ? [.banner, .sound, .list]
            : []
    }

    func userNotificationCenter(
        _ center: UNUserNotificationCenter,
        didReceive response: UNNotificationResponse
    ) async {
        let userInfo = response.notification.request.content.userInfo
        if let action = conversationAction(from: userInfo) {
            await LingxiAppActionStore.shared.enqueue(action)
            return
        }
        await MainActor.run {
            NotificationCenter.default.post(
                name: .lingxiCronNotificationOpened,
                object: nil,
                userInfo: userInfo
            )
        }
    }

    private func conversationAction(from userInfo: [AnyHashable: Any]) -> LingxiAppAction? {
        ConversationNotificationRoute.appAction(from: userInfo)
    }
}
