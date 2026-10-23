import UIKit
import UserNotifications
import FirebaseAuth
import FirebaseMessaging

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
        PushRegistration.shared.start()
        Self.cronBackgroundBridge.registerAtLaunch(
            taskIdentifier: cronBackgroundTaskIdentifier,
            registrar: Self.cronBackgroundRegistrarFactory()
        )
        return true
    }

    func application(_ application: UIApplication, didRegisterForRemoteNotificationsWithDeviceToken deviceToken: Data) {
        Messaging.messaging().apnsToken = deviceToken
        PushRegistration.shared.refresh()
    }

    func userNotificationCenter(
        _ center: UNUserNotificationCenter,
        willPresent notification: UNNotification
    ) async -> UNNotificationPresentationOptions {
        if PushMessagePolicy.isRemote(notification.request.content.userInfo) {
            return await MainActor.run {
                PushMessagePolicy.accepts(notification.request.content.userInfo,
                    uid: Auth.auth().currentUser?.uid, enabled: NotificationPreferencesStore.load().enabled)
                    ? [.banner, .sound, .list] : []
            }
        }
        return conversationAction(from: notification.request.content.userInfo) == nil
            ? [.banner, .sound, .list]
            : []
    }

    func userNotificationCenter(
        _ center: UNUserNotificationCenter,
        didReceive response: UNNotificationResponse
    ) async {
        let userInfo = response.notification.request.content.userInfo
        if PushMessagePolicy.isRemote(userInfo) {
            let action = await MainActor.run {
                PushMessagePolicy.remoteControlAction(userInfo, uid: Auth.auth().currentUser?.uid,
                    enabled: NotificationPreferencesStore.load().enabled)
            }
            if let action { await LingxiAppActionStore.shared.enqueue(action) }
            return
        }
        if let action = conversationAction(from: userInfo) {
            await LingxiAppActionStore.shared.enqueue(action)
            return
        }
        if let raw = userInfo["lingxi.url"] as? String, let url = URL(string: raw), let action = LingxiDeepLink.action(from: url) {
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
