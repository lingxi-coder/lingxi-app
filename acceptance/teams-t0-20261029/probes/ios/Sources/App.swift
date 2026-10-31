import UIKit
import FirebaseCore
import FirebaseAuth
#if TEAM_FIRESTORE_PROBE
import FirebaseFirestore
#endif

@main
final class AppDelegate: UIResponder, UIApplicationDelegate {
    var window: UIWindow?

    func application(_ application: UIApplication, didFinishLaunchingWithOptions options: [UIApplication.LaunchOptionsKey: Any]?) -> Bool {
        let firebase = FirebaseOptions(googleAppID: "1:123:ios:teamsspike", gcmSenderID: "123")
        firebase.apiKey = "emulator-only"
        firebase.projectID = "demo-lingxi-teams-t0"
        FirebaseApp.configure(name: "teams-size-probe", options: firebase)
        let app = FirebaseApp.app(name: "teams-size-probe")!
        _ = Auth.auth(app: app)
        let label = UILabel()
#if TEAM_FIRESTORE_PROBE
        _ = Firestore.firestore(app: app).collection("teams").document("synthetic")
            .collection("conversations").document("synthetic").collection("changes")
            .whereField("seq", isGreaterThan: 1).order(by: "seq")
        label.text = "Firestore SDK linked"
#else
        label.text = "Baseline Firebase SDK linked"
#endif
        let controller = UIViewController()
        controller.view = label
        window = UIWindow(frame: UIScreen.main.bounds)
        window?.rootViewController = controller
        window?.makeKeyAndVisible()
        return true
    }
}
