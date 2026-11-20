#if DEBUG && targetEnvironment(simulator)
import FirebaseAuth
import FirebaseCore
import FirebaseFirestore
import SwiftUI

struct LingxiDataStagingConfiguration {
    let auth: Int
    let firestore: Int
    let api: Int
    let account: String

    static var current: Self? {
        guard IsolatedSimulatorTesting.enabled else { return nil }
        let env = ProcessInfo.processInfo.environment
        guard let auth = port(env["LINGXI_DATA_STAGING_AUTH"]),
              let firestore = port(env["LINGXI_DATA_STAGING_FIRESTORE"]),
              let api = port(env["LINGXI_DATA_STAGING_API"]),
              let account = env["LINGXI_DATA_STAGING_ACCOUNT"], ["reader", "owner"].contains(account) else { return nil }
        return Self(auth: auth, firestore: firestore, api: api, account: account)
    }

    private static func port(_ raw: String?) -> Int? {
        guard let port = raw.flatMap(Int.init), (1024...65535).contains(port) else { return nil }
        return port
    }
}

struct LingxiDataStagingView: View {
    let configuration: LingxiDataStagingConfiguration
    @State private var workspace: TeamsWorkspace?
    @State private var auth: Auth?
    @State private var error = ""
    @State private var navigation = AppNavigationModel()
    @State private var tabs = TabCoordinator()

    var body: some View {
        Group {
            if let workspace {
                TeamsTab()
                    .environment(workspace)
                    .environment(navigation)
                    .environment(tabs)
            } else if !error.isEmpty {
                Text(verbatim: error).accessibilityIdentifier("teams.staging.error")
            } else {
                ProgressView().accessibilityIdentifier("teams.staging.loading")
            }
        }
        .onAppear(perform: connect)
    }

    private func connect() {
        guard auth == nil else { return }
        let name = "lingxi-data-staging"
        let options = FirebaseOptions(googleAppID: "1:123456789:ios:0000000000000000", gcmSenderID: "123456789")
        options.projectID = "demo-lingxi-data"
        // Firebase Installations validates the format before Auth can connect to its emulator.
        options.apiKey = "AIza" + String(repeating: "0", count: 35)
        FirebaseApp.configure(name: name, options: options)
        guard let fixture = FirebaseApp.app(name: name) else { error = "Cannot configure staging"; return }
        let auth = Auth.auth(app: fixture)
        auth.useEmulator(withHost: "127.0.0.1", port: configuration.auth)
        self.auth = auth

        let db = Firestore.firestore(app: fixture)
        let settings = FirestoreSettings()
        settings.host = "127.0.0.1:\(configuration.firestore)"
        settings.isSSLEnabled = false
        settings.cacheSettings = MemoryCacheSettings()
        db.settings = settings

        auth.signIn(withEmail: configuration.account + "@fixture.invalid", password: "AccountFixture123") { result, failure in
            Task { @MainActor in
                if let failure { error = failure.localizedDescription; return }
                guard result?.user.uid == auth.currentUser?.uid, auth.currentUser != nil else {
                    error = "The staging account is unavailable"
                    return
                }
                let workspace = TeamsWorkspace(apiRoot: "http://127.0.0.1:\(configuration.api)/v1/teams", auth: auth, db: db)
                workspace.start()
                self.workspace = workspace
            }
        }
    }
}
#endif
